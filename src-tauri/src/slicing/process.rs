//! D9: running one OrcaSlicer slice under supervision, and D11's mapping
//! of how it ended to a [`SliceFailureCode`].
//!
//! - **Spawn.** [`process_group::spawn_group`] gives the engine its own
//!   process group and, on Linux, `PR_SET_PDEATHSIG(SIGTERM)`. The working
//!   directory is the work directory, and the environment is exactly
//!   [`SliceCommand::environment`] (the D8 allowlist unless a test adds to
//!   it); farm3d's own environment is never inherited.
//! - **Progress** (Linux only, D24). The FIFO is created and its read end
//!   opened non-blocking *before* the spawn, since OrcaSlicer's writer gives
//!   up after about 1 s without a reader (Gate E). Each line is a JSON
//!   object; malformed lines are skipped, percentages are clamped to
//!   0..=100, and `total_percent` never goes backwards. Other platforms get
//!   one indeterminate "Slicing…" update.
//! - **Log.** stdout and stderr are merged line by line into a ring of at
//!   most [`LOG_MAX_BYTES`] that keeps the first [`LOG_HEAD_BYTES`] and the
//!   tail. Every line is redacted ([`Redactor`]) before it is stored.
//! - **Cancel and timeout.** SIGTERM to the group, [`STOP_GRACE`], then
//!   SIGKILL. After a SIGKILL, a stale AppImage FUSE mount that appeared
//!   during the run is unmounted (Gate E).
//! - **Exit mapping** ([`outcome`]). The signed `return_code` from
//!   `out/result.json` when present, else the exit status as `i8` (Gate F).
//!   A zero return code without `out/plate_1.gcode` is `outputMissing`.
//!   Validating the G-code itself is the publish step's job (D11 checks
//!   3–6).
//!
//! Everything here blocks; the operation layer runs it on a blocking
//! thread.

use std::collections::VecDeque;
use std::ffi::OsString;
use std::fs;
use std::io::{self, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use serde::Deserialize;

use crate::library::content::CancelFlag;

use super::invocation::WorkDir;
use super::process_group::{clear_exited_group, spawn_group, stop_group, wait_until, WaitEnd};
use super::runtime::orca_environment;
use super::{SliceFailure, SliceFailureCode};

/// D9: the wall-clock limit for one plate.
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30 * 60);

/// D9: how long the group gets between SIGTERM and SIGKILL.
pub const STOP_GRACE: Duration = Duration::from_secs(5);

/// D9: the most log text kept for one operation.
pub const LOG_MAX_BYTES: usize = 4 * 1024 * 1024;

/// D9: the start of the log that is always kept.
pub const LOG_HEAD_BYTES: usize = 64 * 1024;

/// The message shown while no FIFO progress exists (off Linux, D24).
pub const INDETERMINATE_MESSAGE: &str = "Slicing…";

/// A line longer than this is stored in pieces (log) or skipped (FIFO).
const MAX_LINE_BYTES: usize = 64 * 1024;

/// A progress message or warning is cut to this many characters.
const MAX_MESSAGE_CHARS: usize = 512;

/// `result.json` is read only up to this size.
const MAX_RESULT_BYTES: u64 = 64 * 1024;

/// How often the supervisor wakes to deliver progress.
const TICK: Duration = Duration::from_millis(50);

/// How long the supervisor waits for the log readers after the group is
/// gone. A process that escaped the group could hold the pipes open.
const READER_JOIN: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// Redaction
// ---------------------------------------------------------------------------

/// Rewrites the absolute paths farm3d knows about to placeholders before a
/// log line is stored: the work directory to `<work>`, the engine's
/// directory to `<engine>`, the preset source to `<presets>`, and `$HOME`
/// to `~` (D9). Longer paths are replaced first, so a work directory under
/// `$HOME` becomes `<work>`, not `~/…`.
#[derive(Clone, Debug, Default)]
pub struct Redactor {
    /// (path bytes, token), longest path first.
    rules: Vec<(Vec<u8>, &'static str)>,
}

impl Redactor {
    pub fn new(
        work: &Path,
        engine: &Path,
        preset_source: Option<&Path>,
        home: Option<&Path>,
    ) -> Self {
        let mut redactor = Self::default();
        redactor.add(work, "<work>");
        if let Some(engine_dir) = engine.parent() {
            redactor.add(engine_dir, "<engine>");
        }
        if let Some(source) = preset_source {
            redactor.add(source, "<presets>");
        }
        if let Some(home) = home {
            redactor.add(home, "~");
        }
        redactor
    }

    /// Adds `path`, and its canonical form when that differs, as `token`.
    fn add(&mut self, path: &Path, token: &'static str) {
        let canonical = fs::canonicalize(path).ok();
        for path in std::iter::once(path).chain(canonical.as_deref()) {
            let bytes = path_bytes(path);
            let bytes = bytes.strip_suffix(b"/").unwrap_or(&bytes).to_vec();
            // `/` or an empty path would redact every separator.
            if bytes.len() > 1 && !self.rules.iter().any(|(rule, _)| *rule == bytes) {
                self.rules.push((bytes, token));
            }
        }
        self.rules
            .sort_by_key(|(rule, _)| std::cmp::Reverse(rule.len()));
    }

    /// The longest path it rewrites, in bytes.
    fn longest(&self) -> usize {
        self.rules.first().map_or(0, |(rule, _)| rule.len())
    }

    /// `text` with every known path replaced. A match counts only when the
    /// next byte ends the path component (so `$HOME=/home/a` leaves
    /// `/home/ab` alone).
    pub fn redact(&self, text: &[u8]) -> Vec<u8> {
        self.redact_prefix(text, text.len()).0
    }

    /// Redacts `text` from the start while the scan position is before
    /// `limit`, in one left-to-right pass that tries the longest path
    /// first. Returns the output and how many input bytes it consumed; a
    /// match that starts before `limit` is consumed whole. The end of
    /// `text` counts as a component boundary.
    fn redact_prefix(&self, text: &[u8], limit: usize) -> (Vec<u8>, usize) {
        let mut out = Vec::with_capacity(text.len());
        let mut at = 0;
        while at < limit.min(text.len()) {
            let rest = &text[at..];
            let rule = self.rules.iter().find(|(path, _)| {
                rest.starts_with(path)
                    && rest
                        .get(path.len())
                        .is_none_or(|&next| !continues_component(next))
            });
            match rule {
                Some((path, token)) => {
                    out.extend_from_slice(token.as_bytes());
                    at += path.len();
                }
                None => {
                    out.push(text[at]);
                    at += 1;
                }
            }
        }
        (out, at)
    }

    pub fn redact_str(&self, text: &str) -> String {
        String::from_utf8_lossy(&self.redact(text.as_bytes())).into_owned()
    }
}

#[cfg(unix)]
fn path_bytes(path: &Path) -> Vec<u8> {
    use std::os::unix::ffi::OsStrExt;
    path.as_os_str().as_bytes().to_vec()
}

#[cfg(not(unix))]
fn path_bytes(path: &Path) -> Vec<u8> {
    path.to_string_lossy().into_owned().into_bytes()
}

fn continues_component(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-' | b'.' | b'~' | b'+') || byte >= 0x80
}

// ---------------------------------------------------------------------------
// Log ring
// ---------------------------------------------------------------------------

/// A stored operation log (D9): redacted text, and whether any of it was
/// dropped.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceLog {
    pub text: String,
    pub truncated: bool,
}

/// Keeps the first `head_cap` bytes of whole lines and as much of the tail
/// as fits in the rest of `max`, dropping from the middle.
#[derive(Debug)]
struct LogRing {
    head: Vec<u8>,
    head_closed: bool,
    head_cap: usize,
    tail: VecDeque<Vec<u8>>,
    tail_bytes: usize,
    tail_cap: usize,
    dropped: u64,
}

/// Room kept for the omission marker, so the text stays within `max`.
const MARKER_RESERVE: usize = 128;

impl LogRing {
    fn new(max: usize, head_cap: usize) -> Self {
        let head_cap = head_cap.min(max);
        Self {
            head: Vec::new(),
            head_closed: false,
            head_cap,
            tail: VecDeque::new(),
            tail_bytes: 0,
            tail_cap: max.saturating_sub(head_cap + MARKER_RESERVE),
            dropped: 0,
        }
    }

    fn push(&mut self, mut line: Vec<u8>) {
        if !self.head_closed {
            if self.head.len() + line.len() <= self.head_cap {
                self.head.extend_from_slice(&line);
                return;
            }
            self.head_closed = true;
        }
        if line.len() > self.tail_cap {
            let cut = line.len() - self.tail_cap;
            self.dropped += cut as u64;
            line.drain(..cut);
        }
        self.tail_bytes += line.len();
        self.tail.push_back(line);
        while self.tail_bytes > self.tail_cap {
            let oldest = self.tail.pop_front().expect("the tail is over its cap");
            self.tail_bytes -= oldest.len();
            self.dropped += oldest.len() as u64;
        }
    }

    fn snapshot(&self) -> SliceLog {
        let mut bytes = self.head.clone();
        if self.dropped > 0 {
            if !bytes.is_empty() && !bytes.ends_with(b"\n") {
                bytes.push(b'\n');
            }
            bytes.extend_from_slice(
                format!("[farm3d: {} bytes of log omitted]\n", self.dropped).as_bytes(),
            );
        }
        for line in &self.tail {
            bytes.extend_from_slice(line);
        }
        SliceLog {
            text: String::from_utf8_lossy(&bytes).into_owned(),
            truncated: self.dropped > 0,
        }
    }
}

/// Splits one stream into lines and redacts each before it reaches the
/// ring. A line longer than [`MAX_LINE_BYTES`] is stored in pieces, keeping
/// back enough bytes that a path split across two pieces is still
/// redacted whole.
struct LineAssembler {
    pending: Vec<u8>,
    redactor: Arc<Redactor>,
}

impl LineAssembler {
    fn new(redactor: Arc<Redactor>) -> Self {
        Self {
            pending: Vec::new(),
            redactor,
        }
    }

    fn feed(&mut self, bytes: &[u8], sink: &mut dyn FnMut(Vec<u8>)) {
        self.pending.extend_from_slice(bytes);
        while let Some(end) = self.pending.iter().position(|&byte| byte == b'\n') {
            let line: Vec<u8> = self.pending.drain(..=end).collect();
            sink(self.redactor.redact(&line));
        }
        if self.pending.len() > MAX_LINE_BYTES {
            // Any path that starts before `limit` fits in `pending`; the
            // bytes after it stay raw until more arrive.
            let keep = self.redactor.longest().saturating_sub(1);
            let limit = self.pending.len().saturating_sub(keep);
            let (piece, consumed) = self.redactor.redact_prefix(&self.pending, limit);
            sink(piece);
            self.pending.drain(..consumed);
        }
    }

    fn finish(&mut self, sink: &mut dyn FnMut(Vec<u8>)) {
        if !self.pending.is_empty() {
            let mut line = self.redactor.redact(&std::mem::take(&mut self.pending));
            line.push(b'\n');
            sink(line);
        }
    }
}

// ---------------------------------------------------------------------------
// Progress
// ---------------------------------------------------------------------------

/// One progress update (D9). Percentages are clamped to 0..=100, and
/// `total_percent` never decreases within a run. Off Linux, the one update
/// has no percentages.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceProgress {
    pub message: String,
    pub plate_index: Option<u32>,
    pub plate_count: Option<u32>,
    pub plate_percent: Option<u8>,
    pub total_percent: Option<u8>,
    pub warning: Option<String>,
}

/// A FIFO line as OrcaSlicer writes it (spike Gate E). Every field is
/// optional so that type mismatches, not missing fields, reject a line.
#[derive(Deserialize)]
struct RawProgress {
    message: Option<String>,
    plate_index: Option<f64>,
    plate_count: Option<f64>,
    plate_percent: Option<f64>,
    total_percent: Option<f64>,
    warning: Option<String>,
}

fn percent(value: Option<f64>) -> Option<u8> {
    value
        .filter(|value| value.is_finite())
        .map(|value| value.clamp(0.0, 100.0).round() as u8)
}

fn count(value: Option<f64>) -> Option<u32> {
    value
        .filter(|value| value.is_finite() && *value >= 0.0)
        .map(|value| value.min(f64::from(u32::MAX)) as u32)
}

fn short(text: String, redactor: &Redactor) -> String {
    let text = redactor.redact_str(&text);
    match text.char_indices().nth(MAX_MESSAGE_CHARS) {
        Some((cut, _)) => text[..cut].to_string(),
        None => text,
    }
}

/// Parses FIFO lines into monotonic progress updates.
// Only the Linux FIFO reader uses it (D24).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
struct ProgressParser {
    last_total: Option<u8>,
    redactor: Arc<Redactor>,
}

impl ProgressParser {
    fn new(redactor: Arc<Redactor>) -> Self {
        Self {
            last_total: None,
            redactor,
        }
    }

    /// The update for one line, or `None` for a malformed line (not a JSON
    /// object with a `message` or a `total_percent`).
    fn parse(&mut self, line: &[u8]) -> Option<SliceProgress> {
        let raw: RawProgress = serde_json::from_slice(line).ok()?;
        if raw.message.is_none() && raw.total_percent.is_none() {
            return None;
        }
        let total = match (percent(raw.total_percent), self.last_total) {
            (Some(total), Some(last)) => Some(total.max(last)),
            (total, last) => total.or(last),
        };
        self.last_total = total;
        Some(SliceProgress {
            message: short(raw.message.unwrap_or_default(), &self.redactor),
            plate_index: count(raw.plate_index),
            plate_count: count(raw.plate_count),
            plate_percent: percent(raw.plate_percent),
            total_percent: total,
            warning: raw
                .warning
                .filter(|warning| !warning.is_empty())
                .map(|warning| short(warning, &self.redactor)),
        })
    }
}

/// Splits FIFO bytes into lines; an overlong line is dropped.
// Only the Linux FIFO reader uses it (D24).
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
#[derive(Default)]
struct FifoLines {
    pending: Vec<u8>,
    discarding: bool,
}

impl FifoLines {
    fn feed(&mut self, bytes: &[u8], each: &mut dyn FnMut(&[u8])) {
        for &byte in bytes {
            if byte == b'\n' {
                if !self.discarding {
                    each(&self.pending);
                }
                self.pending.clear();
                self.discarding = false;
            } else if !self.discarding {
                self.pending.push(byte);
                if self.pending.len() > MAX_LINE_BYTES {
                    self.pending.clear();
                    self.discarding = true;
                }
            }
        }
    }

    fn finish(&mut self, each: &mut dyn FnMut(&[u8])) {
        if !self.discarding && !self.pending.is_empty() {
            each(&self.pending);
        }
        self.pending.clear();
    }
}

// ---------------------------------------------------------------------------
// The FIFO (Linux)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
mod fifo {
    use std::fs::{self, File};
    use std::io::{self, Read};
    use std::path::Path;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::{mpsc, Arc};
    use std::thread::{self, JoinHandle};
    use std::time::Duration;

    use rustix::fs::{mknodat, open, FileType, Mode, OFlags, CWD};

    use super::{FifoLines, ProgressParser, SliceProgress};

    const IDLE: Duration = Duration::from_millis(25);

    /// Creates the FIFO at `path` (replacing a leftover) and opens its read
    /// end non-blocking, so OrcaSlicer's writer finds a reader at once.
    pub fn open_reader(path: &Path) -> io::Result<File> {
        let _ = fs::remove_file(path);
        mknodat(CWD, path, FileType::Fifo, Mode::RUSR | Mode::WUSR, 0)?;
        let fd = open(
            path,
            OFlags::RDONLY | OFlags::NONBLOCK | OFlags::CLOEXEC,
            Mode::empty(),
        )?;
        Ok(File::from(fd))
    }

    /// Reads progress lines until `stop` is set, then drains what is left.
    /// A read of 0 bytes only means no writer is connected right now.
    pub fn spawn_reader(
        mut file: File,
        mut parser: ProgressParser,
        stop: Arc<AtomicBool>,
        updates: mpsc::Sender<SliceProgress>,
    ) -> JoinHandle<()> {
        thread::spawn(move || {
            let mut lines = FifoLines::default();
            let mut buffer = [0u8; 8192];
            let mut deliver = |line: &[u8]| {
                if let Some(update) = parser.parse(line) {
                    let _ = updates.send(update);
                }
            };
            loop {
                let stopping = stop.load(Ordering::Acquire);
                match file.read(&mut buffer) {
                    Ok(0) => {}
                    Ok(read) => {
                        lines.feed(&buffer[..read], &mut deliver);
                        continue;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {}
                    Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
                    Err(_) => break,
                }
                if stopping {
                    break;
                }
                thread::sleep(IDLE);
            }
            lines.finish(&mut deliver);
        })
    }
}

// ---------------------------------------------------------------------------
// Stale AppImage mounts (Linux, Gate E)
// ---------------------------------------------------------------------------

/// An AppImage FUSE mount: its mount point, and the AppImage's file name
/// (the mount's source).
#[derive(Clone, Debug, PartialEq, Eq)]
struct AppImageMount {
    mount_point: PathBuf,
    appimage: String,
}

/// Undoes `/proc/self/mountinfo`'s octal escapes (`\040` for a space).
fn unescape_mountinfo(field: &str) -> String {
    let bytes = field.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut at = 0;
    while at < bytes.len() {
        let digits = bytes
            .get(at + 1..at + 4)
            .and_then(|digits| std::str::from_utf8(digits).ok());
        if let (b'\\', Some(digits)) = (bytes[at], digits) {
            if let Ok(value) = u8::from_str_radix(digits, 8) {
                out.push(value);
                at += 4;
                continue;
            }
        }
        out.push(bytes[at]);
        at += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

/// The `/tmp/.mount_*` FUSE mounts in `mountinfo` whose source is an
/// AppImage called `appimage`. A line looks like
/// `59 54 0:73 / /tmp/.mount_OrcaSlcMaJnp ro,… shared:450 - fuse.<name> <name> ro,…`.
fn appimage_mounts(mountinfo: &str, appimage: &str) -> Vec<AppImageMount> {
    mountinfo
        .lines()
        .filter_map(|line| {
            let (before, after) = line.split_once(" - ")?;
            let mount_point = unescape_mountinfo(before.split(' ').nth(4)?);
            let mut after = after.split(' ');
            let fs_type = after.next()?;
            let source = unescape_mountinfo(after.next()?);
            let is_mount = Path::new(&mount_point)
                .file_name()
                .is_some_and(|name| name.to_string_lossy().starts_with(".mount_"));
            (is_mount && fs_type.starts_with("fuse") && source == appimage).then(|| AppImageMount {
                mount_point: PathBuf::from(mount_point),
                appimage: source,
            })
        })
        .collect()
}

#[cfg(target_os = "linux")]
fn current_appimage_mounts(engine: &Path) -> Vec<AppImageMount> {
    let Some(name) = engine.file_name() else {
        return Vec::new();
    };
    fs::read_to_string("/proc/self/mountinfo")
        .map(|mountinfo| appimage_mounts(&mountinfo, &name.to_string_lossy()))
        .unwrap_or_default()
}

#[cfg(not(target_os = "linux"))]
fn current_appimage_mounts(_engine: &Path) -> Vec<AppImageMount> {
    Vec::new()
}

/// After a SIGKILL: unmounts each engine mount that was not there before
/// the spawn (`fusermount -u`, then `-uz`), and says what happened. A
/// mount that predates the run can belong to an OrcaSlicer the user has
/// open, so it is never touched.
fn clean_stale_mounts(engine: &Path, before: &[AppImageMount]) -> Vec<String> {
    let mut notes = Vec::new();
    for mount in current_appimage_mounts(engine) {
        if before.contains(&mount) {
            continue;
        }
        let name = mount
            .mount_point
            .file_name()
            .map(|name| name.to_string_lossy().into_owned())
            .unwrap_or_default();
        let unmounted = ["-u", "-uz"].iter().find(|flag| {
            ["fusermount", "fusermount3"].iter().any(|tool| {
                Command::new(tool)
                    .arg(flag)
                    .arg(&mount.mount_point)
                    .env_clear()
                    .env("PATH", super::runtime::ORCA_UNIX_PATH)
                    .stdin(Stdio::null())
                    .stdout(Stdio::null())
                    .stderr(Stdio::null())
                    .status()
                    .is_ok_and(|status| status.success())
            })
        });
        notes.push(match unmounted {
            Some(flag) => format!(
                "[farm3d: unmounted a stale {} mount ({name}) with fusermount {flag}]",
                mount.appimage
            ),
            None => format!(
                "[farm3d: could not unmount a stale {} mount ({name})]",
                mount.appimage
            ),
        });
    }
    notes
}

// ---------------------------------------------------------------------------
// Running a slice
// ---------------------------------------------------------------------------

/// Everything needed to run one slice.
#[derive(Clone, Debug)]
pub struct SliceCommand {
    pub engine: PathBuf,
    pub work: WorkDir,
    /// The child's whole environment. [`SliceCommand::new`] sets the D8
    /// allowlist; tests append `FAKE_ORCA_*` variables.
    pub environment: Vec<(OsString, OsString)>,
    /// D9's wall-clock limit ([`DEFAULT_TIMEOUT`]).
    pub timeout: Duration,
    /// SIGTERM → SIGKILL grace ([`STOP_GRACE`]).
    pub grace: Duration,
    pub redactor: Redactor,
}

impl SliceCommand {
    /// A D8 invocation of `engine` in `work`, with the allowlisted
    /// environment, the 30-minute limit, and a redactor for the work
    /// directory, the engine, `preset_source`, and `$HOME`.
    pub fn new(engine: PathBuf, work: WorkDir, preset_source: Option<&Path>) -> Self {
        let home = std::env::var_os("HOME").map(PathBuf::from);
        let redactor = Redactor::new(work.root(), &engine, preset_source, home.as_deref());
        Self {
            engine,
            work,
            environment: orca_environment(),
            timeout: DEFAULT_TIMEOUT,
            grace: STOP_GRACE,
            redactor,
        }
    }
}

/// Told about a running slice as it goes. Both calls happen on the thread
/// that called [`run_slice`].
pub trait SliceObserver {
    /// The engine started with this pid (the group id on Unix).
    fn spawned(&mut self, pid: u32);
    /// A new progress update.
    fn progress(&mut self, update: SliceProgress);
}

/// How the engine process ended.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunExit {
    /// It could not be started. `reason` is redacted.
    SpawnFailed { reason: String },
    /// It exited with this status, read as a signed byte (Gate F).
    Exited { code: i32 },
    /// A signal farm3d did not send ended it.
    Signalled { signal: i32 },
    /// farm3d stopped it on request.
    Cancelled,
    /// farm3d stopped it at the wall-clock limit.
    TimedOut,
}

/// The fields farm3d reads from OrcaSlicer's `result.json`.
#[derive(Clone, Debug, PartialEq, Eq, Deserialize)]
pub struct OrcaResult {
    pub return_code: i32,
    #[serde(default)]
    pub error_string: String,
}

/// Everything one run produced.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SliceRun {
    pub exit: RunExit,
    /// `out/result.json`, when the engine exited by itself and wrote one.
    pub result: Option<OrcaResult>,
    pub log: SliceLog,
}

/// What a run means for the operation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SliceOutcome {
    /// Return code 0 and `out/plate_1.gcode` exists; the publish step
    /// validates it next (D11 checks 3–6).
    OutputWritten,
    Failed(SliceFailure),
    Cancelled,
}

type LogSink = Arc<Mutex<LogRing>>;

fn push_line(ring: &LogSink, line: Vec<u8>) {
    ring.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .push(line);
}

/// Reads one output stream into the ring until it closes, then says so.
fn spawn_log_reader(
    mut stream: impl Read + Send + 'static,
    ring: LogSink,
    redactor: Arc<Redactor>,
    done: mpsc::Sender<()>,
) {
    thread::spawn(move || {
        let mut assembler = LineAssembler::new(redactor);
        let mut sink = |line: Vec<u8>| push_line(&ring, line);
        let mut buffer = vec![0u8; 16 * 1024];
        loop {
            match stream.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => assembler.feed(&buffer[..read], &mut sink),
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(_) => break,
            }
        }
        assembler.finish(&mut sink);
        let _ = done.send(());
    });
}

fn exit_of(child: &mut Child) -> RunExit {
    let Ok(Some(status)) = child.try_wait() else {
        return RunExit::Signalled { signal: 0 };
    };
    if let Some(code) = status.code() {
        return RunExit::Exited {
            code: i32::from(code as u8 as i8),
        };
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::ExitStatusExt;
        if let Some(signal) = status.signal() {
            return RunExit::Signalled { signal };
        }
    }
    RunExit::Signalled { signal: 0 }
}

fn read_result(path: &Path) -> Option<OrcaResult> {
    let file = fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_RESULT_BYTES).read_to_end(&mut bytes).ok()?;
    serde_json::from_slice(&bytes).ok()
}

/// Runs one slice to the end: it exits, `cancel` is set, or the timeout
/// passes. The work directory's `input/` must be written and `out/` and
/// `datadir/` must exist ([`WorkDir::create`]).
pub fn run_slice(
    command: &SliceCommand,
    cancel: &CancelFlag,
    observer: &mut dyn SliceObserver,
) -> SliceRun {
    let redactor = Arc::new(command.redactor.clone());
    let ring: LogSink = Arc::new(Mutex::new(LogRing::new(LOG_MAX_BYTES, LOG_HEAD_BYTES)));
    let (progress_sender, progress_receiver) = mpsc::channel();

    #[cfg(target_os = "linux")]
    let fifo_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    #[cfg(target_os = "linux")]
    let fifo_reader = fifo::open_reader(&command.work.progress_fifo())
        .map(|file| {
            fifo::spawn_reader(
                file,
                ProgressParser::new(redactor.clone()),
                fifo_stop.clone(),
                progress_sender.clone(),
            )
        })
        .ok();
    #[cfg(target_os = "linux")]
    let with_progress = fifo_reader.is_some();
    #[cfg(not(target_os = "linux"))]
    let with_progress = false;
    drop(progress_sender);

    let mounts_before = current_appimage_mounts(&command.engine);
    let mut process = Command::new(&command.engine);
    process
        .args(command.work.arguments(with_progress))
        .current_dir(command.work.root())
        .env_clear()
        .envs(command.environment.iter().map(|(key, value)| (key, value)))
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let started = Instant::now();
    let mut child = match spawn_group(&mut process) {
        Ok(child) => child,
        Err(error) => {
            #[cfg(target_os = "linux")]
            {
                fifo_stop.store(true, std::sync::atomic::Ordering::Release);
                if let Some(reader) = fifo_reader {
                    let _ = reader.join();
                }
            }
            let reason = redactor.redact_str(&format!(
                "{} could not be started: {error}",
                command.engine.display()
            ));
            push_line(&ring, format!("[farm3d: {reason}]\n").into_bytes());
            let log = ring.lock().unwrap_or_else(|p| p.into_inner()).snapshot();
            return SliceRun {
                exit: RunExit::SpawnFailed { reason },
                result: None,
                log,
            };
        }
    };
    observer.spawned(child.id());
    if !with_progress {
        observer.progress(SliceProgress {
            message: INDETERMINATE_MESSAGE.to_string(),
            plate_index: None,
            plate_count: None,
            plate_percent: None,
            total_percent: None,
            warning: None,
        });
    }

    let (done_sender, done_receiver) = mpsc::channel();
    let stdout = child.stdout.take().expect("stdout is piped");
    let stderr = child.stderr.take().expect("stderr is piped");
    spawn_log_reader(stdout, ring.clone(), redactor.clone(), done_sender.clone());
    spawn_log_reader(stderr, ring.clone(), redactor.clone(), done_sender);

    let deadline = started + command.timeout;
    let deliver = |observer: &mut dyn SliceObserver| {
        for update in progress_receiver.try_iter() {
            observer.progress(update);
        }
    };
    let end = loop {
        let step = (Instant::now() + TICK).min(deadline);
        let end = wait_until(&mut child, step, &|| cancel.is_cancelled());
        deliver(observer);
        match end {
            WaitEnd::DeadlinePassed if Instant::now() < deadline => continue,
            end => break end,
        }
    };
    let killed = match end {
        WaitEnd::Exited => clear_exited_group(&mut child, command.grace),
        WaitEnd::StopRequested | WaitEnd::DeadlinePassed => stop_group(&mut child, command.grace),
    };
    let exit = match end {
        WaitEnd::Exited => exit_of(&mut child),
        WaitEnd::StopRequested => RunExit::Cancelled,
        WaitEnd::DeadlinePassed => RunExit::TimedOut,
    };

    #[cfg(target_os = "linux")]
    {
        fifo_stop.store(true, std::sync::atomic::Ordering::Release);
        if let Some(reader) = fifo_reader {
            let _ = reader.join();
        }
    }
    deliver(observer);

    let join_deadline = Instant::now() + READER_JOIN;
    for _ in 0..2 {
        let left = join_deadline.saturating_duration_since(Instant::now());
        if done_receiver.recv_timeout(left).is_err() {
            break;
        }
    }
    if killed {
        for note in clean_stale_mounts(&command.engine, &mounts_before) {
            push_line(&ring, redactor.redact(format!("{note}\n").as_bytes()));
        }
    }
    let result = matches!(exit, RunExit::Exited { .. })
        .then(|| read_result(&command.work.result_json()))
        .flatten()
        .map(|result| OrcaResult {
            error_string: redactor.redact_str(&result.error_string),
            ..result
        });
    let log = ring.lock().unwrap_or_else(|p| p.into_inner()).snapshot();
    SliceRun { exit, result, log }
}

// ---------------------------------------------------------------------------
// Failure mapping (D11, spike Gate F)
// ---------------------------------------------------------------------------

fn failure(code: SliceFailureCode, message: impl Into<String>) -> SliceOutcome {
    SliceOutcome::Failed(SliceFailure {
        code,
        message: message.into(),
    })
}

/// D11's code and text for a nonzero OrcaSlicer return code.
/// `error_string` is `result.json`'s, used only for codes D11 does not
/// name.
pub fn failure_for_return_code(return_code: i32, error_string: Option<&str>) -> SliceFailure {
    let (code, message) = match return_code {
        -50 => (
            SliceFailureCode::ObjectsOutsidePlate,
            "An object is outside the printable area.",
        ),
        -5 => (
            SliceFailureCode::PresetInvalid,
            "OrcaSlicer couldn't read the presets farm3d prepared.",
        ),
        -3 => (
            SliceFailureCode::InputMissing,
            "OrcaSlicer couldn't find its input.",
        ),
        -6 => (
            SliceFailureCode::InputInvalid,
            "OrcaSlicer couldn't read the prepared plate.",
        ),
        -17 => (
            SliceFailureCode::PresetIncompatible,
            "The quality preset isn't compatible with this printer.",
        ),
        _ => {
            let message = error_string
                .map(str::trim)
                .filter(|text| !text.is_empty())
                .map(str::to_string)
                .unwrap_or_else(|| format!("OrcaSlicer failed with return code {return_code}."));
            return SliceFailure {
                code: SliceFailureCode::EngineError { return_code },
                message,
            };
        }
    };
    SliceFailure {
        code,
        message: message.to_string(),
    }
}

/// D11: what `run` means. A zero return code counts only when
/// `out/plate_1.gcode` exists (Gate F: "Success." without output
/// happens).
pub fn outcome(run: &SliceRun, work: &WorkDir) -> SliceOutcome {
    match &run.exit {
        RunExit::SpawnFailed { .. } => failure(
            SliceFailureCode::SpawnFailed,
            "farm3d couldn't start OrcaSlicer.",
        ),
        RunExit::Cancelled => SliceOutcome::Cancelled,
        RunExit::TimedOut => failure(
            SliceFailureCode::Timeout,
            "Slicing took longer than 30 minutes.",
        ),
        RunExit::Signalled { signal } => failure(
            SliceFailureCode::EngineCrashed { signal: *signal },
            "OrcaSlicer stopped unexpectedly.",
        ),
        RunExit::Exited { code } => {
            let return_code = run
                .result
                .as_ref()
                .map_or(*code, |result| result.return_code);
            if return_code != 0 {
                let error_string = run
                    .result
                    .as_ref()
                    .map(|result| result.error_string.as_str());
                return SliceOutcome::Failed(failure_for_return_code(return_code, error_string));
            }
            if fs::symlink_metadata(work.gcode()).is_ok() {
                SliceOutcome::OutputWritten
            } else {
                failure(
                    SliceFailureCode::OutputMissing,
                    "OrcaSlicer reported success but wrote no G-code.",
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn redactor() -> Redactor {
        Redactor::new(
            Path::new("/home/u/.local/share/farm3d/slicing-work/sop-1"),
            Path::new("/home/u/Apps/Orca.AppImage"),
            Some(Path::new("/home/u/.cache/farm3d/orca-profiles/abc")),
            Some(Path::new("/home/u")),
        )
    }

    #[test]
    fn redaction_rewrites_the_longest_known_path_first() {
        let text = "load /home/u/.local/share/farm3d/slicing-work/sop-1/input/plate.3mf \
                    with /home/u/Apps/Orca.AppImage and /home/u/.cache/farm3d/orca-profiles/abc/x.json \
                    in /home/u/other but not /home/user2";
        assert_eq!(
            redactor().redact_str(text),
            "load <work>/input/plate.3mf with <engine>/Orca.AppImage and <presets>/x.json \
             in ~/other but not /home/user2"
        );
    }

    #[test]
    fn a_root_path_is_never_a_redaction_rule() {
        let redactor = Redactor::new(
            Path::new("/w"),
            Path::new("/orca"),
            None,
            Some(Path::new("/")),
        );
        assert_eq!(redactor.redact_str("/usr/bin /w/x"), "/usr/bin <work>/x");
    }

    #[test]
    fn a_path_split_across_long_line_pieces_is_still_redacted() {
        let redactor = Arc::new(redactor());
        let mut assembler = LineAssembler::new(redactor);
        let mut stored = Vec::new();
        let mut sink = |line: Vec<u8>| stored.push(line);
        let filler = vec![b'x'; MAX_LINE_BYTES - 10];
        assembler.feed(&filler, &mut sink);
        assembler.feed(b" /home/u/.local/share/far", &mut sink);
        assembler.feed(b"m3d/slicing-work/sop-1/out tail", &mut sink);
        assembler.finish(&mut sink);
        let text = String::from_utf8(stored.concat()).unwrap();
        assert!(
            text.contains("<work>/out tail"),
            "{}",
            &text[text.len() - 80..]
        );
        assert!(!text.contains("/home/u"));
    }

    #[test]
    fn the_ring_keeps_the_head_and_the_tail() {
        let mut ring = LogRing::new(1000, 100);
        for index in 0..200 {
            ring.push(format!("line {index:04}\n").into_bytes());
        }
        let log = ring.snapshot();
        assert!(log.truncated);
        assert!(log.text.len() <= 1000, "{}", log.text.len());
        assert!(log.text.starts_with("line 0000\n"));
        assert!(log.text.ends_with("line 0199\n"));
        assert!(log.text.contains("bytes of log omitted"));
        assert!(!log.text.contains("line 0100\n"));

        let mut small = LogRing::new(1000, 100);
        small.push(b"only\n".to_vec());
        assert_eq!(
            small.snapshot(),
            SliceLog {
                text: "only\n".into(),
                truncated: false
            }
        );
    }

    #[test]
    fn progress_is_clamped_monotonic_and_skips_garbage() {
        let mut parser = ProgressParser::new(Arc::new(redactor()));
        let line = |total: &str| {
            format!(
                r#"{{"message":"Slicing","plate_index":1,"plate_count":1,"plate_percent":{total},"total_percent":{total}}}"#
            )
        };
        assert_eq!(
            parser.parse(line("30").as_bytes()).unwrap().total_percent,
            Some(30)
        );
        assert_eq!(
            parser.parse(line("10").as_bytes()).unwrap().total_percent,
            Some(30)
        );
        let over = parser.parse(line("250").as_bytes()).unwrap();
        assert_eq!(
            (over.total_percent, over.plate_percent),
            (Some(100), Some(100))
        );
        for garbage in [
            "",
            "not json",
            "[1,2]",
            "{}",
            r#"{"message":5}"#,
            r#"{"total_percent":"x"}"#,
        ] {
            assert_eq!(parser.parse(garbage.as_bytes()), None, "{garbage}");
        }
        let negative = ProgressParser::new(Arc::new(redactor()))
            .parse(br#"{"message":"m","total_percent":-4}"#)
            .unwrap();
        assert_eq!(negative.total_percent, Some(0));
    }

    #[test]
    fn overlong_fifo_lines_are_dropped() {
        let mut lines = FifoLines::default();
        let mut seen = Vec::new();
        let mut each = |line: &[u8]| seen.push(line.to_vec());
        lines.feed(&vec![b'a'; MAX_LINE_BYTES + 5], &mut each);
        lines.feed(b"\nok\npart", &mut each);
        lines.finish(&mut each);
        assert_eq!(seen, [b"ok".to_vec(), b"part".to_vec()]);
    }

    #[test]
    fn appimage_mounts_are_read_from_mountinfo() {
        let mountinfo = "\
59 54 0:73 / /tmp/.mount_OrcaSlcMaJnp ro,nosuid,nodev,relatime shared:450 - fuse.Orca.AppImage Orca.AppImage ro,user_id=1000
60 54 0:74 / /tmp/.mount_Other ro shared:451 - fuse.Other.AppImage Other.AppImage ro
61 54 0:75 / /tmp/not\\040a\\040mount ro shared:452 - fuse.Orca.AppImage Orca.AppImage ro
62 1 8:1 / / rw shared:1 - ext4 /dev/sda1 rw";
        assert_eq!(
            appimage_mounts(mountinfo, "Orca.AppImage"),
            [AppImageMount {
                mount_point: PathBuf::from("/tmp/.mount_OrcaSlcMaJnp"),
                appimage: "Orca.AppImage".into(),
            }]
        );
        assert_eq!(unescape_mountinfo("/tmp/a\\040b"), "/tmp/a b");
    }

    #[test]
    fn return_codes_map_to_d11_failures() {
        let cases = [
            (-50, SliceFailureCode::ObjectsOutsidePlate),
            (-5, SliceFailureCode::PresetInvalid),
            (-3, SliceFailureCode::InputMissing),
            (-6, SliceFailureCode::InputInvalid),
            (-17, SliceFailureCode::PresetIncompatible),
            (-24, SliceFailureCode::EngineError { return_code: -24 }),
        ];
        for (return_code, code) in cases {
            assert_eq!(failure_for_return_code(return_code, Some("x")).code, code);
        }
        assert_eq!(
            failure_for_return_code(-24, Some(" Newer file. ")).message,
            "Newer file."
        );
        assert_eq!(
            failure_for_return_code(7, None).message,
            "OrcaSlicer failed with return code 7."
        );
    }
}
