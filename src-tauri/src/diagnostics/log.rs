//! P9 D12 (ADR-0017): a JSON-lines log that accepts only typed values.
//!
//! [`f3d_log!`] takes a level, a `domain.camelCase` code, and `key = value`
//! pairs, and every value must implement the sealed [`LogSafe`] trait. The
//! trait's implementors are exactly the closed list below, so a `String`,
//! `&str`, `PathBuf`, `Url`, or an error's `Display` can't compile into a
//! line. Values of kind id go in `ids` (a bundle rewrites them to
//! pseudonyms); everything else goes in `fields`.
//!
//! A bounded channel feeds one writer thread. Logging never blocks and never
//! panics: a full channel or a write error drops the line and counts it, and
//! the next successful write first records `log.dropped` with the count.
//!
//! ```
//! use farm3d_lib::diagnostics::log::LogId;
//! farm3d_lib::f3d_log!(info, "cameras.captureStarted", printer_id = LogId::printer("prn-1"), attempt = 1u32);
//! ```
//!
//! ```compile_fail
//! // A `String` is not `LogSafe`, so free text can't reach the log.
//! let text = String::from("http://operator:secret@192.0.2.19/");
//! farm3d_lib::f3d_log!(warn, "cameras.captureFailed", detail = text);
//! ```
//!
//! ```compile_fail
//! // Neither is a string slice.
//! farm3d_lib::f3d_log!(warn, "cameras.captureFailed", detail = "free text");
//! ```
//!
//! ```compile_fail
//! // Nor a path.
//! let path = std::path::PathBuf::from("/home/operator");
//! farm3d_lib::f3d_log!(warn, "cameras.captureFailed", detail = path);
//! ```

use std::fs::{self, File, OpenOptions};
use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, SyncSender, TrySendError};
use std::sync::{Arc, Mutex, RwLock};
use std::thread::JoinHandle;
use std::time::Duration;

use chrono::{DateTime, SecondsFormat, Utc};

use super::pseudonym::Pseudonym;
use crate::contracts::command::ErrorCode;
use crate::persistence::{RepositoryError, StorageError};

/// The active file's name; rotated files are `farm3d.1.log`..`farm3d.4.log`.
pub const LOG_FILE: &str = "farm3d.log";
/// Rotation threshold (D12).
pub const ROTATE_BYTES: u64 = 2 * 1024 * 1024;
/// Files kept: the active one plus four rotated ones.
pub const MAX_FILES: usize = 5;
/// The writer channel's bound.
pub const CHANNEL_CAPACITY: usize = 1024;
/// A line's size limit, newline excluded.
pub const MAX_LINE_BYTES: usize = 4096;
/// Entries kept in each of `ids` and `fields`.
pub const MAX_ENTRIES: usize = 16;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LogLevel {
    Error,
    Warn,
    Info,
}

impl LogLevel {
    fn as_str(self) -> &'static str {
        match self {
            Self::Error => "error",
            Self::Warn => "warn",
            Self::Info => "info",
        }
    }
}

/// A rendered value: an id, or any other typed field.
#[derive(Clone, Debug, PartialEq)]
pub enum LogValue {
    Id(String),
    Field(serde_json::Value),
}

mod sealed {
    /// Private supertrait: nothing outside this file can implement it, so
    /// nothing outside this file can implement [`super::LogSafe`].
    pub trait Sealed {}
}

/// A value that can't carry free text.
pub trait LogSafe: sealed::Sealed {
    fn to_log_value(&self) -> LogValue;
}

impl<T: LogSafe + ?Sized> sealed::Sealed for &T {}
impl<T: LogSafe + ?Sized> LogSafe for &T {
    fn to_log_value(&self) -> LogValue {
        (**self).to_log_value()
    }
}

/// A typed id. Built only by the constructors below, each of which names
/// what the id identifies.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LogId(String);

macro_rules! log_id_constructors {
    ($($name:ident),* $(,)?) => {
        impl LogId {
            $(
                pub fn $name(id: &str) -> Self {
                    Self(id.to_string())
                }
            )*
        }
    };
}

log_id_constructors!(
    printer,
    job,
    spool,
    model,
    project,
    slice_revision,
    queue_entry,
    host_operation,
    incident,
    attention,
    snapshot,
    slice_operation,
    operation,
    staging,
    journal,
    backup,
);

impl LogId {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl sealed::Sealed for LogId {}
impl LogSafe for LogId {
    fn to_log_value(&self) -> LogValue {
        LogValue::Id(self.0.clone())
    }
}

impl sealed::Sealed for Pseudonym {}
impl LogSafe for Pseudonym {
    fn to_log_value(&self) -> LogValue {
        LogValue::Field(serde_json::Value::String(self.as_str().to_string()))
    }
}

impl sealed::Sealed for ErrorCode {}
impl LogSafe for ErrorCode {
    fn to_log_value(&self) -> LogValue {
        serde_name(self)
    }
}

/// The serde name of a fieldless enum.
fn serde_name<T: serde::Serialize>(value: &T) -> LogValue {
    match serde_json::to_value(value) {
        Ok(text @ serde_json::Value::String(_)) => LogValue::Field(text),
        _ => LogValue::Field(serde_json::Value::String("unknown".into())),
    }
}

/// Opts a fieldless, `Serialize` domain enum into the log (it logs its
/// serde name). One line per enum, here and nowhere else.
macro_rules! log_safe_enum {
    ($($ty:ty),* $(,)?) => {
        $(
            impl sealed::Sealed for $ty {}
            impl LogSafe for $ty {
                fn to_log_value(&self) -> LogValue {
                    serde_name(self)
                }
            }
        )*
    };
}

log_safe_enum!(
    crate::connections::StatusCacheWarningOperation,
    crate::backup::InstallerStep,
    crate::diagnostics::bundle::DiagnosticsSection,
);

/// The leading identifier of a `Debug` rendering: an error's variant name,
/// never its fields.
fn variant_name(debug: &str) -> String {
    debug
        .chars()
        .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
        .collect()
}

/// Errors log their variant name only, never `Display` or `Debug` output.
macro_rules! log_safe_error {
    ($($ty:ty),* $(,)?) => {
        $(
            impl sealed::Sealed for $ty {}
            impl LogSafe for $ty {
                fn to_log_value(&self) -> LogValue {
                    LogValue::Field(serde_json::Value::String(variant_name(&format!("{self:?}"))))
                }
            }
        )*
    };
}

log_safe_error!(
    StorageError,
    RepositoryError,
    crate::connections::status_repository::StatusCacheError,
    crate::library::content::ContentError,
);

macro_rules! log_safe_number {
    ($($ty:ty),* $(,)?) => {
        $(
            impl sealed::Sealed for $ty {}
            impl LogSafe for $ty {
                fn to_log_value(&self) -> LogValue {
                    LogValue::Field(serde_json::Value::from(*self))
                }
            }
        )*
    };
}

log_safe_number!(u8, u16, u32, u64, i8, i16, i32, i64, usize, bool);

impl sealed::Sealed for Duration {}
impl LogSafe for Duration {
    fn to_log_value(&self) -> LogValue {
        LogValue::Field(serde_json::Value::from(
            u64::try_from(self.as_millis()).unwrap_or(u64::MAX),
        ))
    }
}

impl sealed::Sealed for DateTime<Utc> {}
impl LogSafe for DateTime<Utc> {
    fn to_log_value(&self) -> LogValue {
        LogValue::Field(serde_json::Value::String(
            self.to_rfc3339_opts(SecondsFormat::Millis, true),
        ))
    }
}

/// The names of every `LogSafe` implementor, for the sealed-list test.
pub const LOG_SAFE_IMPLEMENTORS: &[&str] = &[
    "LogId",
    "Pseudonym",
    "ErrorCode",
    "StatusCacheWarningOperation",
    "InstallerStep",
    "DiagnosticsSection",
    "StorageError",
    "RepositoryError",
    "StatusCacheError",
    "ContentError",
    "u8",
    "u16",
    "u32",
    "u64",
    "i8",
    "i16",
    "i32",
    "i64",
    "usize",
    "bool",
    "Duration",
    "DateTime<Utc>",
];

// ---------------------------------------------------------------- lines

/// `<domain>.<camelCase>`: one dot, both parts alphanumeric and starting
/// with a lowercase letter.
pub fn is_valid_code(code: &str) -> bool {
    let mut parts = code.split('.');
    let (Some(domain), Some(name), None) = (parts.next(), parts.next(), parts.next()) else {
        return false;
    };
    [domain, name].iter().all(|part| {
        part.chars().next().is_some_and(|c| c.is_ascii_lowercase())
            && part.chars().all(|c| c.is_ascii_alphanumeric())
    })
}

/// `snake_case` to `camelCase`.
fn camel_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len());
    let mut upper = false;
    for c in key.chars() {
        if c == '_' {
            upper = true;
        } else if !c.is_ascii_alphanumeric() {
            continue;
        } else if upper && !out.is_empty() {
            out.push(c.to_ascii_uppercase());
            upper = false;
        } else {
            out.push(c);
            upper = false;
        }
    }
    out
}

fn render_object(entries: &[(String, serde_json::Value)]) -> String {
    let mut out = String::from("{");
    for (index, (key, value)) in entries.iter().enumerate() {
        if index > 0 {
            out.push(',');
        }
        out.push_str(&serde_json::to_string(key).unwrap_or_else(|_| "\"\"".into()));
        out.push(':');
        out.push_str(&serde_json::to_string(value).unwrap_or_else(|_| "null".into()));
    }
    out.push('}');
    out
}

/// One line, without its newline.
pub fn render_line(
    level: LogLevel,
    code: &'static str,
    entries: &[(&'static str, LogValue)],
    now: DateTime<Utc>,
) -> String {
    let code = if is_valid_code(code) {
        code
    } else {
        debug_assert!(false, "invalid log code {code:?}");
        "log.invalidCode"
    };
    let mut ids = Vec::new();
    let mut fields = Vec::new();
    for (key, value) in entries {
        let key = camel_case(key);
        match value {
            LogValue::Id(id) => {
                if ids.len() < MAX_ENTRIES {
                    ids.push((key, serde_json::Value::String(id.clone())));
                }
            }
            LogValue::Field(field) => {
                if fields.len() < MAX_ENTRIES {
                    fields.push((key, field.clone()));
                }
            }
        }
    }
    let line = format!(
        "{{\"ts\":\"{}\",\"level\":\"{}\",\"code\":\"{}\",\"ids\":{},\"fields\":{}}}",
        now.to_rfc3339_opts(SecondsFormat::Millis, true),
        level.as_str(),
        code,
        render_object(&ids),
        render_object(&fields),
    );
    if line.len() <= MAX_LINE_BYTES {
        return line;
    }
    format!(
        "{{\"ts\":\"{}\",\"level\":\"{}\",\"code\":\"{}\",\"ids\":{{}},\"fields\":{{\"truncated\":true}}}}",
        now.to_rfc3339_opts(SecondsFormat::Millis, true),
        level.as_str(),
        code,
    )
}

// --------------------------------------------------------------- writer

#[derive(Clone, Debug)]
pub struct LoggerOptions {
    pub rotate_bytes: u64,
    pub max_files: usize,
    pub capacity: usize,
    /// Debug builds mirror each line to stderr.
    pub mirror_stderr: bool,
}

impl Default for LoggerOptions {
    fn default() -> Self {
        Self {
            rotate_bytes: ROTATE_BYTES,
            max_files: MAX_FILES,
            capacity: CHANNEL_CAPACITY,
            mirror_stderr: cfg!(debug_assertions),
        }
    }
}

enum Message {
    Line(String),
    Flush(mpsc::Sender<()>),
    Stop,
}

#[derive(Default)]
struct Counters {
    /// Every line dropped since the logger opened.
    total: AtomicU64,
    /// Dropped lines not yet recorded by a `log.dropped` line.
    pending: AtomicU64,
}

impl Counters {
    fn drop_one(&self) {
        self.total.fetch_add(1, Ordering::Relaxed);
        self.pending.fetch_add(1, Ordering::Relaxed);
    }
}

/// A log writer over one directory.
pub struct Logger {
    sender: SyncSender<Message>,
    counters: Arc<Counters>,
    mirror_stderr: bool,
    handle: Mutex<Option<JoinHandle<()>>>,
}

impl Logger {
    /// Opens the log. Never fails: a directory or file that can't be opened
    /// now is retried on each write, and the lines in between are counted.
    pub fn open(dir: &Path, options: LoggerOptions) -> Self {
        let (sender, receiver) = mpsc::sync_channel(options.capacity.max(1));
        let counters = Arc::new(Counters::default());
        let mirror_stderr = options.mirror_stderr;
        let mut writer = Writer {
            dir: dir.to_path_buf(),
            options,
            file: None,
            size: 0,
            counters: Arc::clone(&counters),
        };
        let handle = std::thread::Builder::new()
            .name("farm3d-log".into())
            .spawn(move || writer.run(receiver))
            .ok();
        Self {
            sender,
            counters,
            mirror_stderr,
            handle: Mutex::new(handle),
        }
    }

    /// Queues one line. Never blocks and never panics.
    pub fn log(&self, level: LogLevel, code: &'static str, entries: &[(&'static str, LogValue)]) {
        let line = render_line(level, code, entries, Utc::now());
        if self.mirror_stderr {
            mirror(&line);
        }
        match self.sender.try_send(Message::Line(line)) {
            Ok(()) => {}
            Err(TrySendError::Full(_) | TrySendError::Disconnected(_)) => self.counters.drop_one(),
        }
    }

    /// Lines dropped since the logger opened.
    pub fn dropped_total(&self) -> u64 {
        self.counters.total.load(Ordering::Relaxed)
    }

    /// Waits until every line queued before this call has been handled.
    pub fn flush(&self) {
        let (done, wait) = mpsc::channel();
        if self.sender.send(Message::Flush(done)).is_ok() {
            let _ = wait.recv();
        }
    }
}

impl Drop for Logger {
    fn drop(&mut self) {
        let _ = self.sender.send(Message::Stop);
        if let Some(handle) = self.handle.lock().ok().and_then(|mut h| h.take()) {
            let _ = handle.join();
        }
    }
}

fn mirror(line: &str) {
    let mut stderr = std::io::stderr().lock();
    let _ = stderr.write_all(line.as_bytes());
    let _ = stderr.write_all(b"\n");
}

struct Writer {
    dir: PathBuf,
    options: LoggerOptions,
    file: Option<File>,
    size: u64,
    counters: Arc<Counters>,
}

impl Writer {
    fn run(&mut self, receiver: Receiver<Message>) {
        while let Ok(message) = receiver.recv() {
            match message {
                Message::Line(line) => self.handle(&line),
                Message::Flush(done) => {
                    let _ = done.send(());
                }
                Message::Stop => return,
            }
        }
    }

    fn handle(&mut self, line: &str) {
        let pending = self.counters.pending.swap(0, Ordering::Relaxed);
        if pending > 0 {
            let notice = render_line(
                LogLevel::Warn,
                "log.dropped",
                &[("count", LogValue::Field(serde_json::Value::from(pending)))],
                Utc::now(),
            );
            if self.write(&notice).is_err() {
                self.counters.pending.fetch_add(pending, Ordering::Relaxed);
                self.counters.drop_one();
                return;
            }
        }
        if self.write(line).is_err() {
            self.counters.drop_one();
        }
    }

    fn path(&self, index: usize) -> PathBuf {
        if index == 0 {
            self.dir.join(LOG_FILE)
        } else {
            self.dir.join(format!("farm3d.{index}.log"))
        }
    }

    fn open(&mut self) -> std::io::Result<()> {
        fs::create_dir_all(&self.dir)?;
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.path(0))?;
        self.size = file.metadata()?.len();
        self.file = Some(file);
        Ok(())
    }

    fn rotate(&mut self) -> std::io::Result<()> {
        self.file = None;
        let last = self.options.max_files.saturating_sub(1).max(1);
        for index in (1..last).rev() {
            let from = self.path(index);
            if from.exists() {
                let to = self.path(index + 1);
                let _ = fs::remove_file(&to);
                fs::rename(from, to)?;
            }
        }
        let first = self.path(1);
        let _ = fs::remove_file(&first);
        fs::rename(self.path(0), first)?;
        Ok(())
    }

    fn write(&mut self, line: &str) -> std::io::Result<()> {
        let result = self.write_inner(line);
        if result.is_err() {
            // Reopen from scratch next time.
            self.file = None;
        }
        result
    }

    fn write_inner(&mut self, line: &str) -> std::io::Result<()> {
        if self.file.is_none() {
            self.open()?;
        }
        let bytes = line.len() as u64 + 1;
        if self.size > 0 && self.size + bytes > self.options.rotate_bytes {
            self.rotate()?;
            self.open()?;
        }
        let file = self.file.as_mut().ok_or(std::io::ErrorKind::NotFound)?;
        let mut buffer = Vec::with_capacity(line.len() + 1);
        buffer.extend_from_slice(line.as_bytes());
        buffer.push(b'\n');
        file.write_all(&buffer)?;
        self.size += bytes;
        Ok(())
    }
}

// ---------------------------------------------------------------- global

static GLOBAL: RwLock<Option<Arc<Logger>>> = RwLock::new(None);

/// Starts (or restarts, in another directory) the process log. Call once at
/// startup, after the installer and before `Storage::open`.
pub fn init(log_root: &Path) {
    let logger = Arc::new(Logger::open(log_root, LoggerOptions::default()));
    let previous = match GLOBAL.write() {
        Ok(mut slot) => slot.replace(logger),
        Err(_) => None,
    };
    drop(previous);
}

/// Stops the process log, flushing what is queued.
pub fn shutdown() {
    let previous = GLOBAL.write().ok().and_then(|mut slot| slot.take());
    drop(previous);
}

/// Waits until every line queued so far has been written.
pub fn flush() {
    let logger = GLOBAL.read().ok().and_then(|slot| slot.clone());
    if let Some(logger) = logger {
        logger.flush();
    }
}

/// Lines the process log has dropped since it started.
pub fn dropped_total() -> u64 {
    GLOBAL
        .read()
        .ok()
        .and_then(|slot| slot.as_ref().map(|logger| logger.dropped_total()))
        .unwrap_or(0)
}

/// What [`f3d_log!`] calls. Before `init`, debug builds mirror the line to
/// stderr and release builds discard it.
pub fn emit(level: LogLevel, code: &'static str, entries: &[(&'static str, LogValue)]) {
    let logger = GLOBAL.read().ok().and_then(|slot| slot.clone());
    match logger {
        Some(logger) => logger.log(level, code, entries),
        None if cfg!(debug_assertions) => {
            mirror(&render_line(level, code, entries, Utc::now()));
        }
        None => {}
    }
}

/// Logs one typed line: `f3d_log!(warn, "cameras.captureFailed", printer_id
/// = LogId::printer(&id), error = error)`. Every value must be `LogSafe`.
#[macro_export]
macro_rules! f3d_log {
    (error, $($rest:tt)*) => { $crate::f3d_log!(@emit Error, $($rest)*) };
    (warn, $($rest:tt)*) => { $crate::f3d_log!(@emit Warn, $($rest)*) };
    (info, $($rest:tt)*) => { $crate::f3d_log!(@emit Info, $($rest)*) };
    (@emit $level:ident, $code:expr $(, $key:ident = $value:expr)* $(,)?) => {{
        $crate::diagnostics::log::emit(
            $crate::diagnostics::log::LogLevel::$level,
            $code,
            &[$((
                stringify!($key),
                $crate::diagnostics::log::LogSafe::to_log_value(&$value),
            )),*],
        )
    }};
}
