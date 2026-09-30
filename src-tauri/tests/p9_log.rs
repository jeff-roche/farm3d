//! P9 Task 3: the safe-by-construction diagnostics log (D12, ADR-0017).

mod common;

use std::marker::PhantomData;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use farm3d_lib::diagnostics::log::{
    is_valid_code, LogId, LogLevel, LogSafe, LogValue, Logger, LoggerOptions, LOG_FILE,
    LOG_SAFE_IMPLEMENTORS, MAX_FILES, ROTATE_BYTES,
};
use farm3d_lib::diagnostics::pseudonym::Pseudonym;

fn quiet(rotate_bytes: u64, capacity: usize) -> LoggerOptions {
    LoggerOptions {
        rotate_bytes,
        max_files: MAX_FILES,
        capacity,
        mirror_stderr: false,
    }
}

fn read_lines(path: &Path) -> Vec<serde_json::Value> {
    std::fs::read_to_string(path)
        .unwrap_or_default()
        .lines()
        .map(|line| serde_json::from_str(line).expect("each line is one JSON object"))
        .collect()
}

// ---- sealed implementors (the documented alternative to trybuild) ----

struct Probe<T: ?Sized>(PhantomData<T>);
trait ViaSafe {
    fn is_log_safe(&self) -> bool;
}
impl<T: LogSafe + ?Sized> ViaSafe for Probe<T> {
    fn is_log_safe(&self) -> bool {
        true
    }
}
trait ViaOther {
    fn is_log_safe(&self) -> bool;
}
impl<T: ?Sized> ViaOther for &Probe<T> {
    fn is_log_safe(&self) -> bool {
        false
    }
}
macro_rules! is_log_safe {
    ($ty:ty) => {
        (&Probe::<$ty>(PhantomData)).is_log_safe()
    };
}

#[test]
fn free_text_types_are_not_log_safe() {
    assert!(!is_log_safe!(String));
    assert!(!is_log_safe!(str));
    assert!(!is_log_safe!(&'static str));
    assert!(!is_log_safe!(PathBuf));
    assert!(!is_log_safe!(std::path::Path));
    assert!(!is_log_safe!(std::io::Error));
    assert!(!is_log_safe!(Box<dyn std::error::Error>));
    assert!(!is_log_safe!(serde_json::Value));
    assert!(!is_log_safe!(std::io::ErrorKind));
}

#[test]
fn exactly_the_documented_types_are_log_safe() {
    use farm3d_lib::connections::StatusCacheWarningOperation;
    use farm3d_lib::contracts::command::ErrorCode;
    use farm3d_lib::persistence::{RepositoryError, StorageError};
    assert!(is_log_safe!(LogId));
    assert!(is_log_safe!(Pseudonym));
    assert!(is_log_safe!(ErrorCode));
    assert!(is_log_safe!(StatusCacheWarningOperation));
    assert!(is_log_safe!(farm3d_lib::backup::InstallerStep));
    assert!(is_log_safe!(farm3d_lib::diagnostics::bundle::DiagnosticsSection));
    assert!(is_log_safe!(StorageError));
    assert!(is_log_safe!(RepositoryError));
    assert!(is_log_safe!(u8) && is_log_safe!(u16) && is_log_safe!(u32) && is_log_safe!(u64));
    assert!(is_log_safe!(i8) && is_log_safe!(i16) && is_log_safe!(i32) && is_log_safe!(i64));
    assert!(is_log_safe!(usize) && is_log_safe!(bool));
    assert!(is_log_safe!(Duration));
    assert!(is_log_safe!(chrono::DateTime<chrono::Utc>));
    assert_eq!(
        LOG_SAFE_IMPLEMENTORS.len(),
        22,
        "update this list with the impls"
    );
}

#[test]
fn errors_log_their_variant_name_only() {
    use farm3d_lib::persistence::RepositoryError;
    let error = RepositoryError::NotFound {
        entity_id: "http://operator:s3cr3t-P9@192.0.2.19/".into(),
    };
    let LogValue::Field(value) = error.to_log_value() else {
        panic!("an error is a field");
    };
    assert_eq!(value, serde_json::json!("NotFound"));
}

#[test]
fn a_pseudonym_is_stable_within_a_run_and_hides_its_value() {
    let one = Pseudonym::of("printer", "prn-1");
    assert_eq!(one, Pseudonym::of("printer", "prn-1"));
    assert_ne!(one, Pseudonym::of("printer", "prn-2"));
    assert!(one.as_str().starts_with("printer-"));
    assert_eq!(one.as_str().len(), "printer-".len() + 8);
    assert!(!one.as_str().contains("prn-1"));
    let salt = [7u8; 16];
    assert_ne!(one, Pseudonym::of_with_salt(&salt, "printer", "prn-1"));
}

// ---- line format ----

#[test]
fn a_line_is_one_json_object_with_ids_apart_from_fields() {
    let temp = tempfile::tempdir().unwrap();
    let logger = Logger::open(temp.path(), quiet(1 << 20, 64));
    logger.log(
        LogLevel::Warn,
        "cameras.captureFailed",
        &[
            ("printer_id", LogId::printer("prn-1").to_log_value()),
            ("attempt", 2u32.to_log_value()),
            ("retryable", true.to_log_value()),
            ("elapsed", Duration::from_millis(1500).to_log_value()),
        ],
    );
    logger.flush();
    let lines = read_lines(&temp.path().join(LOG_FILE));
    assert_eq!(lines.len(), 1);
    let line = &lines[0];
    assert_eq!(line["level"], "warn");
    assert_eq!(line["code"], "cameras.captureFailed");
    assert_eq!(line["ids"], serde_json::json!({"printerId": "prn-1"}));
    assert_eq!(
        line["fields"],
        serde_json::json!({"attempt": 2, "retryable": true, "elapsed": 1500})
    );
    let ts = line["ts"].as_str().unwrap();
    assert!(chrono::DateTime::parse_from_rfc3339(ts).is_ok(), "{ts}");
    assert!(ts.ends_with('Z'));
}

#[test]
fn a_line_is_capped_at_four_kib_and_sixteen_entries() {
    let temp = tempfile::tempdir().unwrap();
    let logger = Logger::open(temp.path(), quiet(1 << 20, 64));
    let many: Vec<(&'static str, LogValue)> = (0..40)
        .map(|_| ("n", 1u8.to_log_value()))
        .chain((0..40).map(|_| ("printer_id", LogId::printer("prn").to_log_value())))
        .collect();
    logger.log(LogLevel::Info, "log.many", &many);
    let huge = LogId::job(&"x".repeat(10_000));
    logger.log(
        LogLevel::Info,
        "log.huge",
        &[("job_id", huge.to_log_value())],
    );
    logger.flush();
    let text = std::fs::read_to_string(temp.path().join(LOG_FILE)).unwrap();
    for line in text.lines() {
        assert!(line.len() <= 4096, "{}", line.len());
    }
    let lines = read_lines(&temp.path().join(LOG_FILE));
    assert!(lines[0]["fields"].as_object().unwrap().len() <= 16);
    assert!(lines[0]["ids"].as_object().unwrap().len() <= 16);
    assert_eq!(lines[1]["fields"], serde_json::json!({"truncated": true}));
}

#[test]
fn codes_are_domain_dot_camel_case() {
    assert!(is_valid_code("cameras.captureFailed"));
    assert!(is_valid_code("log.dropped"));
    for bad in [
        "nodot",
        "a.b.c",
        "Cameras.x",
        "cameras.Bad",
        "cameras.",
        ".x",
        "a.b c",
        "a.b-c",
    ] {
        assert!(!is_valid_code(bad), "{bad}");
    }
}

// ---- rotation ----

#[test]
fn rotation_keeps_the_active_file_and_four_rotated_ones() {
    assert_eq!(ROTATE_BYTES, 2 * 1024 * 1024);
    assert_eq!(MAX_FILES, 5);
    let temp = tempfile::tempdir().unwrap();
    let logger = Logger::open(temp.path(), quiet(2048, 64));
    for index in 0..400u32 {
        logger.log(LogLevel::Info, "log.filler", &[("n", index.to_log_value())]);
        logger.flush();
    }
    let mut names: Vec<String> = std::fs::read_dir(temp.path())
        .unwrap()
        .map(|e| e.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    assert_eq!(
        names,
        [
            "farm3d.1.log",
            "farm3d.2.log",
            "farm3d.3.log",
            "farm3d.4.log",
            "farm3d.log"
        ]
    );
    // Newer files hold newer lines, and none passes the threshold.
    let first_n = |name: &str| {
        read_lines(&temp.path().join(name))[0]["fields"]["n"]
            .as_u64()
            .unwrap()
    };
    assert!(first_n("farm3d.4.log") < first_n("farm3d.3.log"));
    assert!(first_n("farm3d.3.log") < first_n("farm3d.2.log"));
    assert!(first_n("farm3d.2.log") < first_n("farm3d.1.log"));
    assert!(first_n("farm3d.1.log") < first_n("farm3d.log"));
    for name in &names {
        assert!(std::fs::metadata(temp.path().join(name)).unwrap().len() <= 2048);
    }
}

// ---- failure and pressure ----

#[test]
fn a_write_failure_drops_and_counts_and_never_panics() {
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("logs");
    // A file where the directory should be: every open fails.
    std::fs::write(&dir, b"not a directory").unwrap();
    let logger = Logger::open(&dir, quiet(1 << 20, 64));
    for _ in 0..5 {
        logger.log(LogLevel::Error, "log.failing", &[]);
    }
    logger.flush();
    assert_eq!(logger.dropped_total(), 5);
    // The directory recovers: the next write first records the drop count.
    std::fs::remove_file(&dir).unwrap();
    logger.log(LogLevel::Info, "log.recovered", &[]);
    logger.flush();
    let lines = read_lines(&dir.join(LOG_FILE));
    assert_eq!(lines[0]["code"], "log.dropped");
    assert_eq!(lines[0]["fields"]["count"], 5);
    assert_eq!(lines[1]["code"], "log.recovered");
}

#[test]
fn a_full_channel_never_blocks_the_caller() {
    let temp = tempfile::tempdir().unwrap();
    let logger = Logger::open(temp.path(), quiet(1 << 30, 1));
    let total = 20_000u64;
    let started = Instant::now();
    for index in 0..total {
        logger.log(LogLevel::Info, "log.burst", &[("n", index.to_log_value())]);
    }
    assert!(started.elapsed() < Duration::from_secs(5));
    logger.flush();
    let written = read_lines(&temp.path().join(LOG_FILE))
        .iter()
        .filter(|line| line["code"] == "log.burst")
        .count() as u64;
    assert_eq!(written + logger.dropped_total(), total);
}

// ---- no eprintln! / println! left in the backend ----

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            if path.file_name().is_some_and(|name| name == "bin") {
                continue;
            }
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

const FORBIDDEN: &[&str] = &[
    "eprintln!",
    "println!",
    "eprint!",
    "print!",
    "dbg!(",
    "io::stderr(",
    "io::stdout(",
];

/// 1-based lines of `text` that hold a forbidden output call outside a
/// `#[cfg(test)] mod` block. A `#[cfg(test)]` on any other item (a `use`,
/// a `fn`, a `impl`) is skipped for that item's attribute only: the walker
/// keeps scanning after it.
fn offending_lines(text: &str) -> Vec<usize> {
    let lines: Vec<&str> = text.lines().collect();
    let mut offenders = Vec::new();
    let mut index = 0;
    while index < lines.len() {
        let trimmed = lines[index].trim();
        if trimmed == "#[cfg(test)]" {
            // The item the attribute guards: the next non-attribute line.
            let mut item = index + 1;
            while item < lines.len() && lines[item].trim_start().starts_with("#[") {
                item += 1;
            }
            let header = lines.get(item).map_or("", |line| line.trim_start());
            let is_mod = header.starts_with("mod ") || header.starts_with("pub mod ")
                || header.starts_with("pub(crate) mod ");
            if is_mod && header.trim_end().ends_with('{') {
                // Skip to the matching close brace.
                let mut depth = 0i32;
                let mut end = item;
                'walk: for (offset, line) in lines[item..].iter().enumerate() {
                    for c in line.chars() {
                        match c {
                            '{' => depth += 1,
                            '}' => {
                                depth -= 1;
                                if depth == 0 {
                                    end = item + offset;
                                    break 'walk;
                                }
                            }
                            _ => {}
                        }
                    }
                    end = item + offset;
                }
                index = end + 1;
                continue;
            }
            index += 1;
            continue;
        }
        let code = lines[index].split("//").next().unwrap_or("");
        if FORBIDDEN.iter().any(|needle| code.contains(needle)) {
            offenders.push(index + 1);
        }
        index += 1;
    }
    offenders
}

#[test]
fn no_eprintln_or_println_remains_outside_test_code() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    assert!(files.len() > 50);
    let mut offenders = Vec::new();
    for file in files {
        // The log's own mirror is the one sanctioned stderr writer.
        if file.ends_with("diagnostics/log.rs") {
            continue;
        }
        let text = std::fs::read_to_string(&file).unwrap();
        for number in offending_lines(&text) {
            offenders.push(format!("{}:{number}", file.display()));
        }
    }
    assert!(offenders.is_empty(), "use f3d_log!: {offenders:#?}");
}

#[test]
fn the_walker_sees_past_a_single_item_cfg_test_and_skips_test_modules() {
    let source = "\
#[cfg(test)]
use std::fmt::Debug;

fn production() {
    eprintln!(\"after a guarded use\");
    print!(\"x\");
    let _ = std::io::stderr();
}

#[cfg(test)]
fn helper() {}

fn more() {
    eprint!(\"y\");
}

#[cfg(test)]
mod tests {
    fn t() {
        println!(\"fine here\");
        if true {
            eprintln!(\"and here\");
        }
    }
}

fn last() {
    println!(\"after the test module\");
}
";
    assert_eq!(offending_lines(source), vec![5, 6, 7, 14, 28]);
}

// ---- shutdown flushes ----

/// The process-wide log is shared: tests that `init` it take turns.
static GLOBAL_LOG: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[test]
fn shutdown_writes_every_queued_line_before_it_returns() {
    let _global = GLOBAL_LOG.lock().unwrap_or_else(|e| e.into_inner());
    let temp = tempfile::tempdir().unwrap();
    let dir = temp.path().join("logs");
    farm3d_lib::diagnostics::log::init(&dir);
    for index in 0..200u32 {
        farm3d_lib::f3d_log!(info, "log.queued", n = index);
    }
    farm3d_lib::diagnostics::log::shutdown();
    let lines = read_lines(&dir.join(LOG_FILE));
    let queued = lines.iter().filter(|l| l["code"] == "log.queued").count();
    assert_eq!(queued, 200);
}

// ---- the corpus scan (P9 Task 3, step 4) ----

/// Drives the paths that used to print `{error:?}` or `{error}`, with the
/// seeded secrets in play, and scans every log file for the corpus.
#[tokio::test]
async fn the_seeded_corpus_never_reaches_a_log_file() {
    let _global = GLOBAL_LOG.lock().unwrap_or_else(|e| e.into_inner());
    use common::secrets::{assert_no_corpus, FULL_CORPUS, PRINTER_NAME, USERINFO_URL};
    use farm3d_lib::connections::moonraker::MoonrakerConnection;
    use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
    use farm3d_lib::library::content::ContentStore;
    use farm3d_lib::persistence::{
        MetadataRootLease, RepositoryError, Storage, StorageError, StoragePaths,
    };

    let temp = tempfile::tempdir().unwrap();
    let log_dir = temp.path().join("logs-under-test");
    farm3d_lib::diagnostics::log::init(&log_dir);

    // 1. A content sweep failure, under a root named after the seeded Printer.
    let root = temp.path().join(PRINTER_NAME);
    let paths = StoragePaths::new(root.join("metadata"), root.join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths, &lease).unwrap();
    let content = ContentStore::open(storage.paths().content_root()).unwrap();
    std::fs::remove_dir_all(storage.paths().content_root().join("blobs")).unwrap();
    let _ = content.startup_sweep(&storage);

    // 2. A Moonraker WebSocket failure against a userinfo URL: the adapter's
    //    error text can hold the URL, and no log call can take it.
    let port = {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        listener.local_addr().unwrap().port()
    };
    let config = ConnectionConfig {
        kind: "moonraker".into(),
        host: "operator:s3cr3t-P9@127.0.0.1".into(),
        port,
        use_tls: false,
        credential_ref: None,
    };
    let failure = MoonrakerConnection::new(config, None)
        .probe()
        .await
        .unwrap_err();
    farm3d_lib::f3d_log!(warn, "connections.probeFailed", port = port);
    let _ = failure;

    // 3. Errors that carry seeded values in their fields log their variant.
    farm3d_lib::f3d_log!(
        warn,
        "library.linkCheckFailed",
        error = RepositoryError::NotFound {
            entity_id: USERINFO_URL.into()
        }
    );
    farm3d_lib::f3d_log!(
        warn,
        "persistence.openFailed",
        error = StorageError::CorruptData {
            source_name: "settings",
            source_sha256: Some(common::secrets::HOST.into()),
        }
    );

    farm3d_lib::diagnostics::log::flush();
    farm3d_lib::diagnostics::log::shutdown();

    let mut scanned = 0;
    let mut sweep_logged = false;
    for entry in std::fs::read_dir(&log_dir).unwrap() {
        let path = entry.unwrap().path();
        let bytes = std::fs::read(&path).unwrap();
        sweep_logged |= String::from_utf8_lossy(&bytes).contains("library.sweepListBlobsFailed");
        assert_no_corpus(&bytes, &path.display().to_string());
        scanned += 1;
    }
    assert!(
        scanned >= 1 && sweep_logged,
        "the sweep failure must have been logged"
    );
    assert!(!FULL_CORPUS.is_empty());
}
