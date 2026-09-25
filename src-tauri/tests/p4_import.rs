//! P4 Tasks 5-6: Library import (spec D6, D7, D11, D13, D14, D17). Task 5
//! covers selection, staging, and inspection: the native picker (through a
//! fake `ModelFileIo`), a window drop, the path policy, progress events,
//! cancellation, and expiry. Task 6 covers the commit: managed and linked
//! Models, duplicate resolution, idempotency, G-code retention, and failure
//! and cancellation during the commit. Every command runs through the real
//! IPC handler, and a recording listener on `farm3d-event-v1` captures the
//! `library.*` events.
//!
//! Fixtures (preflight S6): the real OrcaSlicer exports `orca-two-plates.3mf`
//! and `orca-cube.gcode` cover the unacknowledged-then-acknowledged 3MF
//! import and G-code retention. Two in-test stand-ins remain, each for an
//! edge the real files lack: the "rich" 3MF (`core-two-objects.3mf` plus a
//! `Metadata/project_settings.config` part) because headless Orca embeds no
//! thumbnail, and an Orca-style G-code whose first half has no commands, so
//! a staged copy truncated to half its length fails inspection.

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant, SystemTime};

use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::contracts::command::ErrorCode;
use farm3d_lib::library::selection::{handle_drop, SelectionPurpose};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;

use common::{a_catalog, invoke, FakeModelFileIo};

const CUBE_BINARY_SHA256: &str = "19f725d85d26793c9681fdf8d070c8b78e13d05f1b06188b4d83cfd0fdc9017c";
const CORE_3MF_SHA256: &str = "0712090c29fed95a750f831dcb7be12a3372648719e3978d0a4cc83e45e3e0ab";

// --- Fixture -------------------------------------------------------------------

struct Env {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Arc<Storage>,
    /// Where the test's source files live: outside farm3d's own trees.
    sources: tempfile::TempDir,
    app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    events: Arc<Mutex<Vec<Value>>>,
    _credentials: tempfile::TempDir,
}

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

fn library_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/library")
        .join(name)
}

/// An environment whose picker returns copies of `picked` fixtures (or
/// `None`, a cancelled dialog).
fn new_env(picked: Option<&[&str]>) -> Env {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    let sources = tempfile::tempdir().unwrap();
    let picks = picked.map(|names| {
        names
            .iter()
            .map(|name| copy_fixture(sources.path(), name))
            .collect::<Vec<_>>()
    });
    let credentials = tempfile::tempdir().unwrap();
    let (app, webview, _manager, services) = common::runtime_with_file_io(
        tauri::generate_handler![
            farm3d_lib::library::commands::pick_model_files,
            farm3d_lib::library::commands::inspect_import_selection,
            farm3d_lib::library::commands::cancel_import_selection,
            farm3d_lib::library::commands::import_models,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials.path().to_path_buf(),
        no_connection,
        Arc::new(FakeModelFileIo { picks }),
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    app.listen(STATUS_EVENT, move |event| {
        let value: Value = serde_json::from_str(event.payload()).unwrap();
        recorded.lock().unwrap().push(value);
    });
    Env {
        _temp: temp,
        _lease: lease,
        storage,
        sources,
        app,
        webview,
        services,
        events,
        _credentials: credentials,
    }
}

fn copy_fixture(directory: &Path, name: &str) -> PathBuf {
    let path = directory.join(name);
    fs::copy(library_fixture(name), &path).unwrap();
    path
}

/// A valid binary STL of `triangles` identical facets.
fn binary_stl(triangles: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(84 + 50 * triangles as usize);
    let mut header = b"farm3d import progress fixture".to_vec();
    header.resize(80, b' ');
    bytes.extend_from_slice(&header);
    bytes.extend_from_slice(&triangles.to_le_bytes());
    let facet: [f32; 12] = [0., 0., 1., 0., 0., 0., 10., 0., 0., 0., 10., 0.];
    for _ in 0..triangles {
        for value in facet {
            bytes.extend_from_slice(&value.to_le_bytes());
        }
        bytes.extend_from_slice(&[0, 0]);
    }
    bytes
}

/// A little over 5 MiB, so the copy takes several 1 MiB chunks.
const LARGE_TRIANGLES: u32 = 104_858;

impl Env {
    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        invoke(&self.webview, command, body).map(|response| response["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    fn err(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .expect_err(&format!("{command} should have failed"))
    }

    fn source(&self, name: &str) -> PathBuf {
        copy_fixture(self.sources.path(), name)
    }

    fn write_source(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.sources.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    /// Registers `paths` exactly as the drop handler does (clarification 2),
    /// returning the summary as JSON.
    fn register(&self, paths: Vec<PathBuf>) -> Value {
        let summary = self
            .services
            .library
            .selections
            .register(SelectionPurpose::Import, paths);
        serde_json::to_value(summary).unwrap()
    }

    fn inspect(&self, selection_id: &str) -> Value {
        self.ok(
            "inspect_import_selection",
            json!({ "selectionId": selection_id }),
        )
    }

    fn staging(&self, selection_id: &str) -> PathBuf {
        self.storage
            .paths()
            .content_root()
            .join("staging")
            .join(selection_id)
    }

    /// The `.part` files (staged sources, P9) under `staging/<id>/`, sorted.
    fn staged_parts(&self, selection_id: &str) -> Vec<PathBuf> {
        let mut parts: Vec<PathBuf> = match fs::read_dir(self.staging(selection_id)) {
            Ok(entries) => entries
                .map(|entry| entry.unwrap().path())
                .filter(|path| path.to_string_lossy().ends_with(".part"))
                .collect(),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Vec::new(),
            Err(error) => panic!("staging unreadable: {error}"),
        };
        parts.sort();
        parts
    }

    /// `library.*` events recorded since the last call, then clears them.
    fn take_library_events(&self) -> Vec<Value> {
        std::mem::take(&mut *self.events.lock().unwrap())
            .into_iter()
            .filter(|event| {
                event["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("library."))
            })
            .collect()
    }
}

fn selection_id(summary: &Value) -> String {
    summary["selectionId"].as_str().unwrap().to_string()
}

// --- 1. Picker -----------------------------------------------------------------

#[test]
fn the_picker_registers_a_selection_and_a_cancelled_picker_registers_nothing() {
    let env = new_env(Some(&[
        "cube-binary.stl",
        "core-two-objects.3mf",
        "binary.bgcode",
    ]));
    let summary = env.ok("pick_model_files", json!({ "purpose": "import" }));
    let id = selection_id(&summary);
    assert!(id.starts_with("sel-"), "{id}");
    assert_eq!(
        summary,
        json!({
            "selectionId": id,
            "purpose": "import",
            "files": [
                { "fileIndex": 0, "fileName": "cube-binary.stl", "sizeBytes": 684 },
                { "fileIndex": 1, "fileName": "core-two-objects.3mf", "sizeBytes": 1406 },
                { "fileIndex": 2, "fileName": "binary.bgcode", "sizeBytes": 16 },
            ],
        })
    );
    assert_eq!(env.services.library.selections.len(), 1);
    assert!(env.services.library.selections.get(&id).is_ok());

    let cancelled = new_env(None);
    let result = cancelled.ok("pick_model_files", json!({ "purpose": "import" }));
    assert_eq!(result, Value::Null);
    assert!(cancelled.services.library.selections.is_empty());
}

// --- 2. Inspection --------------------------------------------------------------

#[test]
fn inspection_stages_and_inspects_each_file_once_and_rejects_binary_gcode() {
    let env = new_env(Some(&[
        "cube-binary.stl",
        "core-two-objects.3mf",
        "binary.bgcode",
    ]));
    let summary = env.ok("pick_model_files", json!({ "purpose": "import" }));
    let id = selection_id(&summary);

    let inspection = env.inspect(&id);
    assert_eq!(inspection["selectionId"], json!(id));
    let items = inspection["items"].as_array().unwrap();
    assert_eq!(items.len(), 3);

    assert_eq!(items[0]["status"], "ready");
    assert_eq!(items[0]["fileIndex"], 0);
    assert_eq!(items[0]["fileName"], "cube-binary.stl");
    assert_eq!(items[0]["format"], "stl");
    assert_eq!(items[0]["sizeBytes"], 684);
    assert_eq!(items[0]["sha256"], CUBE_BINARY_SHA256);
    assert_eq!(
        items[0]["summary"],
        json!({
            "format": "stl",
            "triangleCount": 12,
            "boundsMm": { "min": [0.0, 0.0, 0.0], "max": [10.0, 10.0, 10.0] },
            "unitsAssumed": true,
        })
    );
    assert_eq!(items[0]["duplicates"], json!([]));
    assert_eq!(items[0]["warnings"], json!([]));
    assert_eq!(items[0]["unsupported"], json!([]));

    assert_eq!(items[1]["status"], "ready", "{}", items[1]);
    assert_eq!(items[1]["format"], "3mf");
    assert_eq!(items[1]["sha256"], CORE_3MF_SHA256);
    assert_eq!(items[1]["summary"]["objectCount"], 2);

    assert_eq!(items[2]["status"], "rejected");
    assert_eq!(items[2]["fileIndex"], 2);
    assert_eq!(items[2]["fileName"], "binary.bgcode");
    assert_eq!(items[2]["code"], "UNSUPPORTED_FORMAT");

    // P9: the 3MF's staged thumbnail isn't a `.part`, so the count is the
    // two ready sources only.
    let parts = env.staged_parts(&id);
    assert_eq!(parts.len(), 2, "{parts:?}");
    assert!(env.staging(&id).join("1.thumb.png").is_file());
    let mtimes: Vec<SystemTime> = parts
        .iter()
        .map(|part| fs::metadata(part).unwrap().modified().unwrap())
        .collect();

    // D13: a second call returns the stored result without re-reading.
    std::thread::sleep(Duration::from_millis(20));
    let again = env.inspect(&id);
    assert_eq!(again, inspection);
    assert_eq!(env.staged_parts(&id), parts);
    let again_mtimes: Vec<SystemTime> = parts
        .iter()
        .map(|part| fs::metadata(part).unwrap().modified().unwrap())
        .collect();
    assert_eq!(again_mtimes, mtimes);
}

// --- 3. Path policy ------------------------------------------------------------

#[cfg(unix)]
#[test]
fn farm3d_trees_are_refused_and_a_symlink_elsewhere_keeps_its_own_name() {
    let env = new_env(None);
    let content_root = env.storage.paths().content_root().to_path_buf();
    let metadata_root = env.storage.paths().metadata_root().to_path_buf();

    let inside_content = content_root.join("inside.stl");
    fs::copy(library_fixture("cube-binary.stl"), &inside_content).unwrap();
    let inside_metadata = metadata_root.join("secret.stl");
    fs::copy(library_fixture("cube-binary.stl"), &inside_metadata).unwrap();
    let to_metadata = env.sources.path().join("looks-harmless.stl");
    std::os::unix::fs::symlink(&inside_metadata, &to_metadata).unwrap();
    let target = env.source("cube-binary.stl");
    let to_elsewhere = env.sources.path().join("linked-cube.stl");
    std::os::unix::fs::symlink(&target, &to_elsewhere).unwrap();

    let summary = env.register(vec![inside_content, to_metadata, to_elsewhere]);
    assert_eq!(summary["files"][2]["fileName"], "linked-cube.stl");
    let inspection = env.inspect(&selection_id(&summary));
    let items = inspection["items"].as_array().unwrap();

    assert_eq!(items[0]["status"], "rejected");
    assert_eq!(items[0]["code"], "PATH_NOT_ALLOWED");
    assert_eq!(items[1]["status"], "rejected");
    assert_eq!(items[1]["code"], "PATH_NOT_ALLOWED");
    assert_eq!(items[2]["status"], "ready");
    assert_eq!(items[2]["fileName"], "linked-cube.stl");
    assert_eq!(items[2]["sha256"], CUBE_BINARY_SHA256);
}

#[test]
fn a_relative_or_missing_path_is_rejected_without_reading() {
    let env = new_env(None);
    let missing = env.sources.path().join("missing.stl");
    let summary = env.register(vec![PathBuf::from("relative.stl"), missing]);
    assert_eq!(summary["files"][0]["sizeBytes"], Value::Null);
    assert_eq!(summary["files"][1]["sizeBytes"], Value::Null);
    let inspection = env.inspect(&selection_id(&summary));
    assert_eq!(inspection["items"][0]["code"], "PATH_NOT_ALLOWED");
    assert_eq!(inspection["items"][1]["code"], "SOURCE_UNREADABLE");
}

// --- 4. Directories ------------------------------------------------------------

#[test]
fn a_dropped_directory_is_not_a_file_and_is_not_walked() {
    let env = new_env(None);
    let directory = env.sources.path().join("models");
    fs::create_dir(&directory).unwrap();
    fs::copy(
        library_fixture("cube-binary.stl"),
        directory.join("nested.stl"),
    )
    .unwrap();
    let file = env.source("cube-binary.stl");

    let summary = handle_drop(
        env.app.handle(),
        &env.services.library,
        vec![directory, file],
    );
    let summary = serde_json::to_value(summary).unwrap();
    assert_eq!(summary["files"].as_array().unwrap().len(), 2);
    assert_eq!(summary["files"][0]["fileName"], "models");
    assert_eq!(summary["files"][0]["sizeBytes"], Value::Null);

    let inspection = env.inspect(&selection_id(&summary));
    assert_eq!(inspection["items"][0]["status"], "rejected");
    assert_eq!(inspection["items"][0]["code"], "NOT_A_FILE");
    assert_eq!(inspection["items"][1]["status"], "ready");
}

// --- 5. Progress ----------------------------------------------------------------

#[test]
fn inspecting_a_large_file_emits_progress_on_the_library_stream() {
    let env = new_env(None);
    let bytes = binary_stl(LARGE_TRIANGLES);
    let total = bytes.len() as u64;
    let source = env.write_source("large.stl", &bytes);
    let summary = env.register(vec![source]);
    let id = selection_id(&summary);
    env.take_library_events();

    let inspection = env.inspect(&id);
    assert_eq!(inspection["items"][0]["status"], "ready");

    let events = env.take_library_events();
    let progress: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "library.import.progress")
        .collect();
    assert!(!progress.is_empty(), "no progress events: {events:?}");
    let stream_id = env.services.library.stream.stream_id();
    for event in &progress {
        assert_eq!(event["streamId"], stream_id);
        assert_eq!(event["subject"], json!({ "kind": "selection", "id": id }));
        assert_eq!(event["payload"]["fileIndex"], 0);
        assert_eq!(event["payload"]["bytesTotal"], total);
    }
    // S5(a): the first chunk and the final byte count always go out.
    assert_eq!(progress[0]["payload"]["bytesDone"], 1024 * 1024);
    assert_eq!(progress.last().unwrap()["payload"]["bytesDone"], total);
    let sequences: Vec<u64> = progress
        .iter()
        .map(|event| event["sequence"].as_u64().unwrap())
        .collect();
    assert!(
        sequences.windows(2).all(|pair| pair[0] < pair[1]),
        "{sequences:?}"
    );
}

// --- 6. Cancellation -------------------------------------------------------------

#[test]
fn cancelling_during_inspection_cancels_unfinished_items_and_discards_the_selection() {
    let env = new_env(None);
    let source = env.write_source("large.stl", &binary_stl(LARGE_TRIANGLES));
    let summary = env.register(vec![source]);
    let id = selection_id(&summary);

    // S5(b): hold the copy after its first chunk while the cancel lands.
    let pause = env.services.library.content.pause_after_first_chunk_once();
    let webview = env.webview.clone();
    let body = json!({ "contractVersion": 1, "selectionId": id });
    let inspecting = std::thread::spawn(move || invoke(&webview, "inspect_import_selection", body));
    pause.wait_until_reached();
    assert_eq!(
        env.ok("cancel_import_selection", json!({ "selectionId": id })),
        json!({})
    );
    pause.release();

    let inspection = inspecting.join().unwrap().unwrap()["data"].clone();
    assert_eq!(inspection["items"][0]["status"], "rejected");
    assert_eq!(inspection["items"][0]["code"], "CANCELLED");
    assert!(
        !env.staging(&id).exists(),
        "staging for a cancelled selection is removed"
    );

    let expired = env.err("inspect_import_selection", json!({ "selectionId": id }));
    assert_eq!(expired["code"], "SELECTION_EXPIRED");

    // Cancelling an unknown selection is a no-op success.
    assert_eq!(
        env.ok(
            "cancel_import_selection",
            json!({ "selectionId": "sel-unknown" })
        ),
        json!({})
    );
}

// --- 7. Expiry and limits ----------------------------------------------------------

#[test]
fn a_selection_expires_thirty_minutes_after_its_last_use() {
    let env = new_env(None);
    let summary = env.register(vec![env.source("cube-binary.stl")]);
    let id = selection_id(&summary);
    let selections = &env.services.library.selections;

    selections.sweep_expired(Instant::now() + Duration::from_secs(29 * 60));
    assert!(selections.get(&id).is_ok());

    selections.sweep_expired(Instant::now() + Duration::from_secs(31 * 60));
    let error = selections.get(&id).map(|_| ()).unwrap_err();
    assert_eq!(error.code, ErrorCode::SelectionExpired);
    assert!(!error.retryable);
    let expired = env.err("inspect_import_selection", json!({ "selectionId": id }));
    assert_eq!(expired["code"], "SELECTION_EXPIRED");
    assert_eq!(expired["recovery"], json!(["RELOAD"]));
}

#[test]
fn a_selection_holds_at_most_one_hundred_files() {
    let env = new_env(None);
    let source = env.source("cube-binary.stl");
    let summary = env.register(vec![source; 101]);
    assert_eq!(summary["files"].as_array().unwrap().len(), 100);
}

// --- 8. Drop -----------------------------------------------------------------------

#[test]
fn a_window_drop_registers_the_files_and_emits_a_path_free_summary() {
    let env = new_env(None);
    let paths = vec![env.source("cube-binary.stl"), env.source("binary.bgcode")];
    env.take_library_events();

    let dropped = handle_drop(env.app.handle(), &env.services.library, paths.clone());
    let dropped = serde_json::to_value(dropped).unwrap();
    let registered = env.register(paths);

    let events = env.take_library_events();
    assert_eq!(events.len(), 1, "{events:?}");
    let event = &events[0];
    assert_eq!(event["type"], "library.selection.dropped");
    assert_eq!(event["streamId"], env.services.library.stream.stream_id());
    assert_eq!(
        event["subject"],
        json!({ "kind": "selection", "id": dropped["selectionId"] })
    );
    assert_eq!(event["payload"], dropped);

    // Equal to `register`'s summary for the same paths, apart from the id.
    let mut without_id = dropped.clone();
    without_id["selectionId"] = registered["selectionId"].clone();
    assert_eq!(without_id, registered);

    // D6/D7: the event carries basenames only.
    let serialized = serde_json::to_string(event).unwrap();
    let prefix = env.sources.path().to_string_lossy().to_string();
    assert!(!serialized.contains(&prefix), "{serialized}");

    // The dropped selection is live for inspection.
    let inspection = env.inspect(&selection_id(&dropped));
    assert_eq!(inspection["items"][0]["status"], "ready");
}

// --- Duplicates (D14) ----------------------------------------------------------------

#[test]
fn inspection_reports_every_model_holding_the_same_bytes() {
    let env = new_env(None);
    env.storage
        .write(|transaction| {
            transaction.execute_batch(&format!(
                "INSERT INTO content_blobs (sha256, size_bytes, created_at)
                   VALUES ('{CUBE_BINARY_SHA256}', 684, '2026-09-24T00:00:00Z'),
                          ('{CORE_3MF_SHA256}', 1406, '2026-09-24T00:00:00Z');
                 INSERT INTO library_projects (id, revision, name, created_at, updated_at)
                   VALUES ('prj-b', 1, 'Brackets', '2026-09-24T00:00:00Z', '2026-09-24T00:00:00Z'),
                          ('prj-a', 1, 'Anchors', '2026-09-24T00:00:00Z', '2026-09-24T00:00:00Z');
                 INSERT INTO library_models (id, revision, name, format, storage_mode, created_at, updated_at)
                   VALUES ('mdl-current', 1, 'Cube', 'stl', 'managed', '2026-09-24T00:00:00Z', '2026-09-24T00:00:00Z'),
                          ('mdl-older', 1, 'Old cube', 'stl', 'managed', '2026-09-24T00:00:00Z', '2026-09-24T00:00:00Z');
                 INSERT INTO project_models (project_id, model_id, added_at)
                   VALUES ('prj-b', 'mdl-current', '2026-09-24T00:00:00Z'),
                          ('prj-a', 'mdl-current', '2026-09-24T00:00:00Z');
                 INSERT INTO model_source_revisions
                   (id, model_id, sequence, content_sha256, size_bytes, format, origin,
                    source_file_name, source_path, captured_at, inspector_version, inspection_json)
                   VALUES
                   ('msr-current-1', 'mdl-current', 1, '{CUBE_BINARY_SHA256}', 684, 'stl', 'import',
                    'cube.stl', '/elsewhere/cube.stl', '2026-09-24T00:00:00Z', 1, '{{}}'),
                   ('msr-older-1', 'mdl-older', 1, '{CUBE_BINARY_SHA256}', 684, 'stl', 'import',
                    'cube.stl', '/elsewhere/cube.stl', '2026-09-24T00:00:00Z', 1, '{{}}'),
                   ('msr-older-2', 'mdl-older', 2, '{CORE_3MF_SHA256}', 1406, 'stl', 'addedRevision',
                    'other.stl', '/elsewhere/other.stl', '2026-09-24T00:00:00Z', 1, '{{}}');"
            ))?;
            Ok(())
        })
        .unwrap();

    let summary = env.register(vec![env.source("cube-binary.stl")]);
    let inspection = env.inspect(&selection_id(&summary));
    assert_eq!(
        inspection["items"][0]["duplicates"],
        json!([
            {
                "modelId": "mdl-current",
                "modelName": "Cube",
                "projectIds": ["prj-a", "prj-b"],
                "revisionId": "msr-current-1",
                "sequence": 1,
                "isCurrent": true,
            },
            {
                "modelId": "mdl-older",
                "modelName": "Old cube",
                "projectIds": [],
                "revisionId": "msr-older-1",
                "sequence": 1,
                "isCurrent": false,
            },
        ])
    );
}

// --- 9. No raw paths (D7, M7) ----------------------------------------------------------

/// Field names declared in a generated TypeScript type (`name:` or
/// `name?:`), outside string literals.
fn declared_fields(text: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let bytes = text.as_bytes();
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index].is_ascii_alphabetic() || bytes[index] == b'_' {
            let start = index;
            while index < bytes.len()
                && (bytes[index].is_ascii_alphanumeric() || bytes[index] == b'_')
            {
                index += 1;
            }
            let mut after = index;
            if after < bytes.len() && bytes[after] == b'?' {
                after += 1;
            }
            if after < bytes.len() && bytes[after] == b':' {
                fields.push(text[start..index].to_string());
            }
        } else {
            index += 1;
        }
    }
    fields
}

/// Capitalised identifiers in `text`: the type names it references.
fn referenced_types(text: &str) -> Vec<String> {
    text.split(|character: char| !character.is_ascii_alphanumeric() && character != '_')
        .filter(|word| word.chars().next().is_some_and(|c| c.is_ascii_uppercase()))
        .map(str::to_string)
        .collect()
}

#[test]
fn no_command_request_carries_a_filesystem_path() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/generated/contracts");
    let contracts = fs::read_to_string(root.join("command/CommandContracts.ts")).unwrap();
    let request_lines: Vec<&str> = contracts
        .lines()
        .filter(|line| {
            line.split_whitespace()
                .nth(2)
                .is_some_and(|name| name.ends_with("Request"))
        })
        .collect();
    for command in [
        "PickModelFiles",
        "InspectImportSelection",
        "CancelImportSelection",
        "ImportModels",
    ] {
        assert!(
            request_lines
                .iter()
                .any(|line| line.contains(&format!("{command}Request ="))),
            "{command}Request missing from CommandContracts.ts"
        );
    }

    // Every request's own fields, then (transitively) every generated type
    // file a request names.
    let mut pending: Vec<String> = Vec::new();
    for line in &request_lines {
        let body = line.split_once('=').map_or("", |(_, body)| body);
        for field in declared_fields(body) {
            assert!(
                !field.to_lowercase().contains("path"),
                "request field `{field}` in: {line}"
            );
        }
        pending.extend(referenced_types(body));
    }
    let mut seen = std::collections::BTreeSet::new();
    let mut scanned = Vec::new();
    while let Some(name) = pending.pop() {
        if !seen.insert(name.clone()) {
            continue;
        }
        let Some(file) = ["command", "domain", "event", "navigation"]
            .iter()
            .map(|directory| root.join(directory).join(format!("{name}.ts")))
            .find(|file| file.is_file())
        else {
            continue;
        };
        let text = fs::read_to_string(&file).unwrap();
        let declaration = text
            .lines()
            .filter(|line| !line.starts_with("import ") && !line.starts_with("//"))
            .collect::<Vec<_>>()
            .join("\n");
        for field in declared_fields(&declaration) {
            assert!(
                !field.to_lowercase().contains("path"),
                "field `{field}` in {} (reachable from a request)",
                file.display()
            );
        }
        pending.extend(referenced_types(&declaration));
        scanned.push(name);
    }
    for reached in ["SelectionPurpose", "ImportItemRequest"] {
        assert!(
            scanned.iter().any(|name| name == reached),
            "the scan follows request types into their own files: {scanned:?}"
        );
    }
}

// === Task 6: import commit (D11, D13, D14, D17) ==========================================

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// One `ImportItemRequest`: managed, Unfiled, no duplicate action, and
/// unsupported content not acknowledged, with `extra` merged over it.
fn request(file_index: u32, name: &str, extra: Value) -> Value {
    let mut request = json!({
        "fileIndex": file_index,
        "name": name,
        "projectIds": [],
        "storageMode": "managed",
        "acknowledgeUnsupported": false,
    });
    for (key, value) in extra.as_object().expect("extra is an object") {
        request[key] = value.clone();
    }
    request
}

/// `core-two-objects.3mf` plus the slicer settings part an Orca or Bambu
/// project carries: a rich 3MF that, unlike headless Orca's
/// `orca-two-plates.3mf`, embeds a thumbnail.
fn rich_3mf() -> Vec<u8> {
    use std::io::{Cursor, Write};
    let base = fs::read(library_fixture("core-two-objects.3mf")).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(base)).unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        writer
            .raw_copy_file(archive.by_index_raw(index).unwrap())
            .unwrap();
    }
    writer
        .start_file(
            "Metadata/project_settings.config",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer
        .write_all(br#"{ "printer_model": "Bambu Lab X1 Carbon" }"#)
        .unwrap();
    writer.finish().unwrap().into_inner()
}

/// An Orca-style G-code file: a long comment-only header, as Orca writes
/// its thumbnails first, then a short command body. Its first half has no
/// commands, so a copy truncated to half its length fails inspection. The
/// real `orca-cube.gcode` has commands from line 19, so it can't show this.
fn orca_style_gcode() -> Vec<u8> {
    let mut text = String::from(
        "; HEADER_BLOCK_START\n; generated by OrcaSlicer 2.3.0\n; total layer number: 5\n; HEADER_BLOCK_END\n",
    );
    for line in 0..200 {
        text.push_str(&format!(
            "; preamble comment line {line:04} of the header block\n"
        ));
    }
    text.push_str("G28\nG90\n");
    for step in 1..=10 {
        text.push_str(&format!("G1 X{} Y{} F3000\n", step * 5, step * 3));
    }
    text.push_str("; printer_model = Bambu Lab X1 Carbon\n");
    text.into_bytes()
}

impl Env {
    fn project(&self, name: &str) -> String {
        let id = farm3d_lib::library::new_id("prj");
        self.storage
            .write_repo(|tx| farm3d_lib::library::repository::insert_project(tx, &id, name))
            .unwrap();
        id
    }

    /// Registers and inspects `paths`, returning the selection id and the
    /// inspection.
    fn inspected(&self, paths: Vec<PathBuf>) -> (String, Value) {
        let id = selection_id(&self.register(paths));
        let inspection = self.inspect(&id);
        (id, inspection)
    }

    fn import(&self, selection_id: &str, operation_id: &str, items: Value) -> Vec<Value> {
        self.ok(
            "import_models",
            json!({ "selectionId": selection_id, "operationId": operation_id, "items": items }),
        )["items"]
            .as_array()
            .unwrap()
            .clone()
    }

    fn count(&self, sql: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    /// Rows that would exist for `sha256` had any of it been written.
    fn rows_for_hash(&self, sha256: &str) -> (i64, i64, i64) {
        (
            self.count(&format!(
                "SELECT COUNT(*) FROM library_models m WHERE EXISTS (
                   SELECT 1 FROM model_source_revisions r
                   WHERE r.model_id = m.id AND r.content_sha256 = '{sha256}')"
            )),
            self.count(&format!(
                "SELECT COUNT(*) FROM model_source_revisions WHERE content_sha256 = '{sha256}'"
            )),
            self.count(&format!(
                "SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{sha256}'"
            )),
        )
    }

    fn blob(&self, sha256: &str) -> PathBuf {
        self.storage
            .paths()
            .content_root()
            .join("blobs/sha256")
            .join(&sha256[..2])
            .join(sha256)
    }

    fn stored_bytes(&self, sha256: &str) -> Vec<u8> {
        use std::io::Read;
        let mut reader = self.services.library.content.open_verified(sha256).unwrap();
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).unwrap();
        bytes
    }
}

// --- T6 1. Managed STL into two Projects ---------------------------------------------------

#[test]
fn a_managed_stl_imports_into_two_projects_with_its_first_revision() {
    let env = new_env(None);
    let quarry = env.project("Quarry");
    let parts = env.project("Parts");
    let (selection, _) = env.inspected(vec![env.source("cube-binary.stl")]);
    env.take_library_events();

    let items = env.import(
        &selection,
        "op-1",
        json!([request(0, "Cube", json!({ "projectIds": [quarry, parts] }))]),
    );

    let item = &items[0];
    assert_eq!(item["fileIndex"], 0);
    assert_eq!(item["outcome"], "imported", "{item}");
    assert_eq!(item["errors"], json!([]));
    assert_eq!(item["warnings"], json!([]));
    let model = &item["model"];
    let model_id = model["id"].as_str().unwrap().to_string();
    assert!(model_id.starts_with("mdl-"), "{model_id}");
    assert_eq!(model["name"], "Cube");
    assert_eq!(model["revision"], 1);
    assert_eq!(model["format"], "stl");
    assert_eq!(model["storageMode"], "managed");
    assert_eq!(model["link"], Value::Null);
    assert_eq!(
        model["projectIds"],
        json!([parts, quarry]),
        "ordered by name"
    );
    assert_eq!(model["revisionCount"], 1);
    let revision = &item["revision"];
    assert_eq!(model["currentRevision"], *revision);
    assert_eq!(revision["modelId"], model_id);
    assert_eq!(revision["sequence"], 1);
    assert_eq!(revision["origin"], "import");
    assert_eq!(revision["sha256"], CUBE_BINARY_SHA256);
    assert_eq!(revision["sizeBytes"], 684);
    assert_eq!(revision["format"], "stl");
    assert_eq!(revision["sourceFileName"], "cube-binary.stl");
    assert_eq!(revision["hasThumbnail"], false);
    assert_eq!(revision["summary"]["triangleCount"], 12);

    assert_eq!(
        env.count("SELECT COUNT(*) FROM library_models WHERE storage_mode = 'managed'"),
        1
    );
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM project_models WHERE model_id = '{model_id}'"
        )),
        2
    );
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM model_source_revisions
             WHERE model_id = '{model_id}' AND sequence = 1 AND origin = 'import'
               AND content_sha256 = '{CUBE_BINARY_SHA256}'"
        )),
        1
    );
    assert_eq!(
        fs::read(env.blob(CUBE_BINARY_SHA256)).unwrap(),
        fs::read(library_fixture("cube-binary.stl")).unwrap()
    );
    assert!(env.staged_parts(&selection).is_empty());

    let events = env.take_library_events();
    let types: Vec<&str> = events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(
        types,
        [
            "library.model.changed",
            "library.revision.created",
            "library.project.changed",
            "library.project.changed",
        ]
    );
    let sequences: Vec<u64> = events
        .iter()
        .map(|event| event["sequence"].as_u64().unwrap())
        .collect();
    assert_eq!(sequences[1], sequences[0] + 1, "{sequences:?}");
    assert_eq!(
        events[0]["subject"],
        json!({ "kind": "model", "id": model_id })
    );
    assert_eq!(events[0]["payload"], *model);
    assert_eq!(
        events[1]["subject"],
        json!({ "kind": "model", "id": model_id })
    );
    assert_eq!(events[1]["payload"], *revision);
    assert_eq!(
        events[2]["subject"],
        json!({ "kind": "project", "id": parts })
    );
    assert_eq!(events[2]["payload"]["modelCount"], 1);
    assert_eq!(
        events[3]["subject"],
        json!({ "kind": "project", "id": quarry })
    );
}

// --- T6 2-3. Linked and unacknowledged 3MF --------------------------------------------------

#[cfg(unix)]
#[test]
fn a_linked_3mf_records_the_selected_path_its_stat_and_its_thumbnail() {
    let env = new_env(None);
    let real = env.write_source("rich.3mf", &rich_3mf());
    // Selected through a symlinked directory: the stored path is the one
    // chosen, not its canonical form (D6).
    let alias = env.sources.path().join("alias");
    std::os::unix::fs::symlink(env.sources.path(), &alias).unwrap();
    let selected = alias.join("rich.3mf");
    let (selection, inspection) = env.inspected(vec![selected.clone()]);
    assert_eq!(inspection["items"][0]["status"], "ready", "{inspection}");

    let items = env.import(
        &selection,
        "op-1",
        json!([request(
            0,
            "Rich",
            json!({ "storageMode": "linked", "acknowledgeUnsupported": true })
        )]),
    );

    let item = &items[0];
    assert_eq!(item["outcome"], "imported", "{item}");
    let model = &item["model"];
    let model_id = model["id"].as_str().unwrap();
    assert_eq!(model["storageMode"], "linked");
    let link = &model["link"];
    assert_eq!(link["path"], selected.to_str().unwrap());
    assert_eq!(link["state"], "ok");
    assert!(link["checkedAt"].is_string(), "{link}");
    assert_eq!(link["watchMode"], "notWatched");
    assert_eq!(item["revision"]["hasThumbnail"], true);

    let metadata = fs::metadata(&real).unwrap();
    let mtime_ns = metadata
        .modified()
        .unwrap()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_nanos() as i64;
    let (linked_path, state, size, observed_mtime, file_id): (
        String,
        String,
        i64,
        i64,
        Option<String>,
    ) = env
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT linked_path, link_state, link_observed_size, link_observed_mtime_ns,
                        link_observed_file_id
                 FROM library_models WHERE id = ?1",
                [model_id],
                |row| {
                    Ok((
                        row.get(0)?,
                        row.get(1)?,
                        row.get(2)?,
                        row.get(3)?,
                        row.get(4)?,
                    ))
                },
            )
        })
        .unwrap();
    assert_eq!(linked_path, selected.to_str().unwrap());
    assert_eq!(state, "ok");
    assert_eq!(size, metadata.len() as i64);
    assert_eq!(observed_mtime, mtime_ns);
    assert!(file_id.is_some());

    let (source, origin_part, width, height, thumbnail_sha): (String, String, i64, i64, String) =
        env.storage
            .read(|connection| {
                connection.query_row(
                    "SELECT t.source, t.origin_part, t.width, t.height, t.content_sha256
                     FROM model_revision_thumbnails t
                     JOIN model_source_revisions r ON r.id = t.revision_id
                     WHERE r.model_id = ?1",
                    [model_id],
                    |row| {
                        Ok((
                            row.get(0)?,
                            row.get(1)?,
                            row.get(2)?,
                            row.get(3)?,
                            row.get(4)?,
                        ))
                    },
                )
            })
            .unwrap();
    assert_eq!(source, "embedded");
    assert_eq!(origin_part, "Metadata/thumbnail.png");
    assert_eq!((width, height), (2, 2));
    assert!(env.blob(&thumbnail_sha).is_file());
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{thumbnail_sha}'"
        )),
        1
    );

    let inspection_json: String = env
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT inspection_json FROM model_source_revisions WHERE model_id = ?1",
                [model_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    let stored: Value = serde_json::from_str(&inspection_json).unwrap();
    assert_eq!(stored["format"], "3mf");
    assert!(
        stored["unsupported"]
            .as_array()
            .unwrap()
            .iter()
            .any(|entry| entry["part"] == "Metadata/project_settings.config"
                && entry["code"] == "SLICER_SETTINGS"),
        "{stored}"
    );
}

#[test]
fn an_unacknowledged_rich_3mf_is_rejected_and_writes_nothing() {
    let env = new_env(None);
    let bytes = rich_3mf();
    let sha256 = sha256_hex(&bytes);
    let (selection, _) = env.inspected(vec![env.write_source("rich.3mf", &bytes)]);
    env.take_library_events();

    let items = env.import(&selection, "op-1", json!([request(0, "Rich", json!({}))]));

    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(
        items[0]["errors"][0]["code"],
        "UNSUPPORTED_NOT_ACKNOWLEDGED"
    );
    assert_eq!(items[0]["model"], Value::Null);
    assert_eq!(env.rows_for_hash(&sha256), (0, 0, 0));
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM content_blobs"), 0);
    assert!(env.take_library_events().is_empty());
}

#[test]
fn the_real_orca_project_imports_its_plates_once_acknowledged() {
    let env = new_env(None);
    let source = env.source("orca-two-plates.3mf");
    let bytes = fs::read(&source).unwrap();
    let sha256 = sha256_hex(&bytes);

    let (selection, inspection) = env.inspected(vec![source.clone()]);
    let item = &inspection["items"][0];
    assert_eq!(item["status"], "ready", "{inspection}");
    assert_eq!(item["format"], "3mf");
    assert_eq!(item["summary"]["objectCount"], 2);
    assert_eq!(item["summary"]["plateCount"], 2);
    assert_eq!(item["summary"]["triangleCount"], 24);
    assert_eq!(item["summary"]["unsupportedCount"], 4);
    assert_eq!(item["unsupported"].as_array().unwrap().len(), 4);
    env.take_library_events();

    let items = env.import(&selection, "op-1", json!([request(0, "Orca", json!({}))]));
    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(
        items[0]["errors"][0]["code"],
        "UNSUPPORTED_NOT_ACKNOWLEDGED"
    );
    assert_eq!(env.rows_for_hash(&sha256), (0, 0, 0));
    assert!(env.take_library_events().is_empty());

    let (selection, _) = env.inspected(vec![source]);
    let items = env.import(
        &selection,
        "op-2",
        json!([request(
            0,
            "Orca",
            json!({ "acknowledgeUnsupported": true })
        )]),
    );
    let item = &items[0];
    assert_eq!(item["outcome"], "imported", "{item}");
    assert_eq!(item["model"]["format"], "3mf");
    assert_eq!(item["revision"]["hasThumbnail"], false);
    assert_eq!(env.rows_for_hash(&sha256), (1, 1, 1));
    assert_eq!(env.stored_bytes(&sha256), bytes);

    let model_id = item["model"]["id"].as_str().unwrap();
    let inspection_json: String = env
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT inspection_json FROM model_source_revisions WHERE model_id = ?1",
                [model_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    let stored: Value = serde_json::from_str(&inspection_json).unwrap();
    assert_eq!(stored["requiredExtensions"], json!(["p"]));
    assert_eq!(
        stored["plates"],
        json!([
            { "index": 1, "objectIds": [2] },
            { "index": 2, "objectIds": [4] }
        ])
    );
}

// --- T6 4. Duplicates (D14) ---------------------------------------------------------------

#[test]
fn duplicate_resolution_is_explicit_for_every_action() {
    let env = new_env(None);
    let parts = env.project("Parts");
    let quarry = env.project("Quarry");
    let racks = env.project("Racks");
    let (first_selection, _) = env.inspected(vec![env.source("cube-binary.stl")]);
    let first = env.import(
        &first_selection,
        "op-first",
        json!([request(0, "Cube", json!({ "projectIds": [parts, quarry] }))]),
    );
    let first_id = first[0]["model"]["id"].as_str().unwrap().to_string();

    let bytes = fs::read(library_fixture("cube-binary.stl")).unwrap();
    let (selection, inspection) = env.inspected(vec![env.write_source("again.stl", &bytes)]);
    assert_eq!(inspection["items"][0]["duplicates"][0]["modelId"], first_id);

    // No action: never silent.
    let items = env.import(
        &selection,
        "op-none",
        json!([request(0, "Again", json!({}))]),
    );
    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(items[0]["errors"][0]["code"], "DUPLICATE_DECISION_REQUIRED");

    // Use existing: no new Model or revision; membership is only added.
    env.take_library_events();
    let items = env.import(
        &selection,
        "op-use",
        json!([request(
            0,
            "Again",
            json!({ "duplicateAction": "useExisting", "targetModelId": first_id, "projectIds": [racks] })
        )]),
    );
    assert_eq!(items[0]["outcome"], "reusedExisting", "{}", items[0]);
    assert_eq!(items[0]["model"]["id"], first_id);
    assert_eq!(
        items[0]["model"]["projectIds"],
        json!([parts, quarry, racks])
    );
    assert_eq!(
        items[0]["model"]["revision"], 2,
        "membership bumps the revision"
    );
    assert_eq!(items[0]["revision"], Value::Null);
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 1);
    assert_eq!(env.count("SELECT COUNT(*) FROM model_source_revisions"), 1);
    let types: Vec<Value> = env
        .take_library_events()
        .iter()
        .map(|event| event["type"].clone())
        .collect();
    assert_eq!(
        types,
        json!(["library.model.changed", "library.project.changed"])
            .as_array()
            .unwrap()
            .clone()
    );

    // Add another: a second Model over the same single blob.
    let items = env.import(
        &selection,
        "op-another",
        json!([request(
            0,
            "Again",
            json!({ "duplicateAction": "addAnother" })
        )]),
    );
    assert_eq!(items[0]["outcome"], "imported", "{}", items[0]);
    assert_ne!(items[0]["model"]["id"], first_id);
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 2);
    assert_eq!(env.rows_for_hash(CUBE_BINARY_SHA256).2, 1);

    // Add as a revision of a Model whose current bytes are these: reused.
    let items = env.import(
        &selection,
        "op-same-bytes",
        json!([request(
            0,
            "Again",
            json!({ "duplicateAction": "addRevision", "targetModelId": first_id, "targetExpectedRevision": 2 })
        )]),
    );
    assert_eq!(items[0]["outcome"], "reusedExisting", "{}", items[0]);
    assert_eq!(items[0]["model"]["revision"], 2);
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM model_source_revisions WHERE model_id = '{first_id}'"
        )),
        1
    );

    // Different bytes as a new revision of the binary-STL Model.
    let (ascii_selection, _) = env.inspected(vec![env.source("cube-ascii.stl")]);
    let items = env.import(
        &ascii_selection,
        "op-revision",
        json!([request(
            0,
            "Cube",
            json!({ "duplicateAction": "addRevision", "targetModelId": first_id, "targetExpectedRevision": 2 })
        )]),
    );
    assert_eq!(items[0]["outcome"], "revisionAdded", "{}", items[0]);
    assert_eq!(items[0]["revision"]["sequence"], 2);
    assert_eq!(items[0]["revision"]["origin"], "addedRevision");
    assert_eq!(items[0]["model"]["revisionCount"], 2);
    assert_eq!(items[0]["model"]["revision"], 3);
    assert_eq!(items[0]["model"]["currentRevision"], items[0]["revision"]);
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM model_source_revisions
             WHERE model_id = '{first_id}' AND sequence = 2 AND origin = 'addedRevision'"
        )),
        1
    );

    // A stale target revision is a typed per-item CONFLICT.
    let items = env.import(
        &selection,
        "op-stale",
        json!([request(
            0,
            "Again",
            json!({ "duplicateAction": "addRevision", "targetModelId": first_id, "targetExpectedRevision": 2 })
        )]),
    );
    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(items[0]["errors"][0]["code"], "CONFLICT");
    assert_eq!(items[0]["errors"][0]["entityId"], first_id);

    // A linked target never takes an added revision.
    let (linked_selection, _) = env.inspected(vec![env.write_source("linked.stl", &binary_stl(3))]);
    let linked = env.import(
        &linked_selection,
        "op-linked",
        json!([request(0, "Linked", json!({ "storageMode": "linked" }))]),
    );
    let linked_id = linked[0]["model"]["id"].as_str().unwrap().to_string();
    let items = env.import(
        &selection,
        "op-into-linked",
        json!([request(
            0,
            "Again",
            json!({ "duplicateAction": "addRevision", "targetModelId": linked_id, "targetExpectedRevision": 1 })
        )]),
    );
    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(items[0]["errors"][0]["code"], "VALIDATION");
    assert_eq!(items[0]["errors"][0]["fieldPath"], "targetModelId");

    // Nor does a Model of another format.
    let (gcode_selection, _) = env.inspected(vec![env.source("plain.gcode")]);
    let items = env.import(
        &gcode_selection,
        "op-format",
        json!([request(
            0,
            "Plain",
            json!({ "duplicateAction": "addRevision", "targetModelId": first_id, "targetExpectedRevision": 3 })
        )]),
    );
    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(items[0]["errors"][0]["code"], "VALIDATION");
    assert_eq!(items[0]["errors"][0]["fieldPath"], "targetModelId");
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM model_source_revisions WHERE model_id = '{first_id}'"
        )),
        2
    );
}

// --- T6 5. Idempotency ---------------------------------------------------------------------

#[test]
fn repeating_an_operation_replays_its_outcomes_without_writing() {
    let env = new_env(None);
    let (selection, _) = env.inspected(vec![
        env.source("cube-binary.stl"),
        env.source("plain.gcode"),
    ]);
    let requests = json!([
        request(0, "Cube", json!({})),
        request(1, "Plain", json!({}))
    ]);
    let first = env.import(&selection, "op-1", requests.clone());
    assert_eq!(first[0]["outcome"], "imported");
    assert_eq!(first[1]["outcome"], "imported");
    env.take_library_events();

    let again = env.import(&selection, "op-1", requests);

    assert_eq!(again, first);
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 2);
    assert_eq!(env.count("SELECT COUNT(*) FROM model_source_revisions"), 2);
    assert!(
        env.take_library_events().is_empty(),
        "a replay emits nothing"
    );
}

#[test]
fn a_different_operation_while_one_is_importing_is_a_conflict() {
    let env = new_env(None);
    let (selection, _) = env.inspected(vec![env.source("cube-binary.stl")]);

    // S6: hold import A just before its placement.
    let pause = env.services.library.content.pause_before_placement_once();
    let webview = env.webview.clone();
    let body = json!({
        "contractVersion": 1,
        "selectionId": selection,
        "operationId": "op-a",
        "items": [request(0, "Cube", json!({}))],
    });
    let importing = std::thread::spawn(move || invoke(&webview, "import_models", body));
    pause.wait_until_reached();

    let conflict = env.err(
        "import_models",
        json!({ "selectionId": selection, "operationId": "op-b", "items": [request(0, "Cube", json!({}))] }),
    );
    assert_eq!(conflict["code"], "CONFLICT", "{conflict}");
    pause.release();

    let finished = importing.join().unwrap().unwrap()["data"].clone();
    assert_eq!(finished["items"][0]["outcome"], "imported");
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 1);
}

#[test]
fn a_blank_operation_id_or_an_unknown_file_index_is_rejected_before_any_write() {
    let env = new_env(None);
    let (selection, _) = env.inspected(vec![env.source("cube-binary.stl")]);

    let blank = env.err(
        "import_models",
        json!({ "selectionId": selection, "operationId": "  ", "items": [request(0, "Cube", json!({}))] }),
    );
    assert_eq!(blank["code"], "VALIDATION");
    assert_eq!(blank["details"]["fieldPath"], "operationId");

    let out_of_range = env.err(
        "import_models",
        json!({ "selectionId": selection, "operationId": "op-1", "items": [request(1, "Cube", json!({}))] }),
    );
    assert_eq!(out_of_range["code"], "VALIDATION");
    assert_eq!(out_of_range["details"]["fieldPath"], "items[0].fileIndex");

    let expired = env.err(
        "import_models",
        json!({ "selectionId": "sel-unknown", "operationId": "op-1", "items": [] }),
    );
    assert_eq!(expired["code"], "SELECTION_EXPIRED");
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 0);
}

#[test]
fn a_locate_selection_cannot_be_inspected_for_import() {
    let env = new_env(None);
    let summary = env.services.library.selections.register(
        SelectionPurpose::Locate,
        vec![env.source("cube-binary.stl")],
    );
    let error = env.err(
        "inspect_import_selection",
        json!({ "selectionId": summary.selection_id }),
    );
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "selectionId");
    assert!(
        !env.staging(&summary.selection_id).exists(),
        "nothing is staged"
    );
}

// --- T6 6. G-code retention (D11) ------------------------------------------------------------

#[test]
fn gcode_is_retained_byte_for_byte_with_its_claims_untrusted() {
    let env = new_env(None);
    let plain = fs::read(library_fixture("plain.gcode")).unwrap();
    let crlf: Vec<u8> = String::from_utf8(plain)
        .unwrap()
        .replace('\n', "\r\n")
        .into_bytes();
    let crlf_source = env.write_source("plain-crlf.gcode", &crlf);
    let prusa_source = env.source("prusa-cube.gcode");
    let prusa = fs::read(&prusa_source).unwrap();
    let orca_source = env.source("orca-cube.gcode");
    let orca = fs::read(&orca_source).unwrap();
    let (selection, _) = env.inspected(vec![crlf_source, prusa_source, orca_source]);

    let items = env.import(
        &selection,
        "op-1",
        json!([
            request(0, "Plain CRLF", json!({})),
            request(1, "Prusa cube", json!({})),
            request(2, "Orca cube", json!({}))
        ]),
    );

    assert_eq!(items[0]["outcome"], "imported", "{}", items[0]);
    assert_eq!(items[1]["outcome"], "imported", "{}", items[1]);
    assert_eq!(items[2]["outcome"], "imported", "{}", items[2]);
    assert_eq!(items[0]["model"]["format"], "gcode");
    assert_eq!(env.stored_bytes(&sha256_hex(&crlf)), crlf);
    assert_eq!(env.stored_bytes(&sha256_hex(&prusa)), prusa);
    assert_eq!(env.stored_bytes(&sha256_hex(&orca)), orca);
    assert_eq!(
        items[1]["revision"]["summary"]["claimedPrinterModel"],
        "MK4S"
    );
    assert_eq!(
        items[2]["revision"]["summary"],
        json!({
            "format": "gcode",
            "producer": { "name": "OrcaSlicer", "version": "2.5.0-dev" },
            "lineCount": 5064,
            "claimedPrinterModel": "Generic Klipper Printer",
            "claimedEstimatedTime": "3m 42s",
        })
    );

    let model_id = items[1]["model"]["id"].as_str().unwrap();
    let inspection_json: String = env
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT inspection_json FROM model_source_revisions WHERE model_id = ?1",
                [model_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    let stored: Value = serde_json::from_str(&inspection_json).unwrap();
    assert_eq!(stored["trusted"], false);
    assert!(
        stored["claims"]
            .as_array()
            .unwrap()
            .iter()
            .any(|claim| claim["key"] == "printer_model" && claim["value"] == "MK4S"),
        "{stored}"
    );

    // P5 adds the slicing tables; Queue data still waits for P7.
    assert_eq!(
        env.count(
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE type = 'table' AND (name LIKE '%queue%' OR name LIKE '%job%')"
        ),
        0
    );
}

// --- T6 7. G-code failure and cancellation ---------------------------------------------------

#[test]
fn a_gcode_inspection_failure_writes_nothing_and_a_fresh_selection_imports() {
    let env = new_env(None);
    let bytes = orca_style_gcode();
    let sha256 = sha256_hex(&bytes);
    let source = env.write_source("orca-cube.gcode", &bytes);

    // (a) The staged copy is damaged, so inspection fails.
    env.services
        .library
        .content
        .inject_failure_once(farm3d_lib::library::content::ContentFailurePoint::TruncateStagedOnce);
    let (selection, inspection) = env.inspected(vec![source.clone()]);
    let code = inspection["items"][0]["code"].clone();
    assert!(
        code == "INVALID_CONTENT" || code == "UNSUPPORTED_FORMAT",
        "{inspection}"
    );
    let items = env.import(
        &selection,
        "op-1",
        json!([request(0, "Orca cube", json!({}))]),
    );
    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(items[0]["errors"][0]["code"], code);
    assert_eq!(env.rows_for_hash(&sha256), (0, 0, 0));
    assert_eq!(sha256_hex(&fs::read(&source).unwrap()), sha256);

    // P11: a new selection over the same source imports.
    let (retry, _) = env.inspected(vec![source]);
    let items = env.import(&retry, "op-2", json!([request(0, "Orca cube", json!({}))]));
    assert_eq!(items[0]["outcome"], "imported", "{}", items[0]);
    assert_eq!(env.stored_bytes(&sha256), bytes);
}

#[test]
fn a_crash_between_placement_and_commit_leaves_only_an_orphan_and_a_retry_imports() {
    let env = new_env(None);
    let source = env.source("prusa-cube.gcode");
    let bytes = fs::read(&source).unwrap();
    let sha256 = sha256_hex(&bytes);
    let (selection, _) = env.inspected(vec![source.clone()]);

    // (b)
    env.services.library.content.inject_failure_once(
        farm3d_lib::library::content::ContentFailurePoint::AfterPlacementBeforeCommit,
    );
    let items = env.import(
        &selection,
        "op-1",
        json!([request(0, "Prusa cube", json!({}))]),
    );
    assert_eq!(items[0]["outcome"], "rejected");
    assert_eq!(items[0]["errors"][0]["code"], "PERSISTENCE_UNAVAILABLE");
    assert_eq!(env.rows_for_hash(&sha256), (0, 0, 0));
    assert!(env.blob(&sha256).is_file(), "the placed blob is an orphan");
    assert_eq!(sha256_hex(&fs::read(&source).unwrap()), sha256);

    // P11: the selection is live and the item has no outcome, so a new
    // operation imports it.
    let items = env.import(
        &selection,
        "op-2",
        json!([request(0, "Prusa cube", json!({}))]),
    );
    assert_eq!(items[0]["outcome"], "imported", "{}", items[0]);
    assert_eq!(env.rows_for_hash(&sha256), (1, 1, 1));
    assert_eq!(env.stored_bytes(&sha256), bytes);
}

#[test]
fn the_startup_sweep_removes_the_orphan_a_failed_commit_leaves() {
    let env = new_env(None);
    let source = env.source("prusa-cube.gcode");
    let sha256 = sha256_hex(&fs::read(&source).unwrap());
    let (selection, _) = env.inspected(vec![source]);
    env.services.library.content.inject_failure_once(
        farm3d_lib::library::content::ContentFailurePoint::AfterPlacementBeforeCommit,
    );
    let items = env.import(
        &selection,
        "op-1",
        json!([request(0, "Prusa cube", json!({}))]),
    );
    assert_eq!(items[0]["outcome"], "rejected");
    assert!(env.blob(&sha256).is_file());

    let report = env
        .services
        .library
        .content
        .startup_sweep(&env.storage)
        .unwrap();

    assert_eq!(report.orphans_removed, 1);
    assert!(!env.blob(&sha256).exists());
    assert_eq!(env.rows_for_hash(&sha256), (0, 0, 0));
}

#[test]
fn cancelling_between_staging_and_placement_writes_nothing() {
    let env = new_env(None);
    let source = env.source("prusa-cube.gcode");
    let bytes = fs::read(&source).unwrap();
    let sha256 = sha256_hex(&bytes);
    let (selection, _) = env.inspected(vec![source.clone()]);

    // (c) Hold the import before placement while the cancel lands.
    let pause = env.services.library.content.pause_before_placement_once();
    let webview = env.webview.clone();
    let body = json!({
        "contractVersion": 1,
        "selectionId": selection,
        "operationId": "op-1",
        "items": [request(0, "Prusa cube", json!({}))],
    });
    let importing = std::thread::spawn(move || invoke(&webview, "import_models", body));
    pause.wait_until_reached();
    env.ok(
        "cancel_import_selection",
        json!({ "selectionId": selection }),
    );
    pause.release();

    let result = importing.join().unwrap().unwrap()["data"].clone();
    assert_eq!(result["items"][0]["outcome"], "cancelled", "{result}");
    assert_eq!(result["items"][0]["model"], Value::Null);
    assert_eq!(env.rows_for_hash(&sha256), (0, 0, 0));
    assert!(!env.blob(&sha256).exists());
    assert!(!env.staging(&selection).exists());
    assert_eq!(sha256_hex(&fs::read(&source).unwrap()), sha256);
    let expired = env.err(
        "import_models",
        json!({ "selectionId": selection, "operationId": "op-2", "items": [] }),
    );
    assert_eq!(expired["code"], "SELECTION_EXPIRED");

    // P11: a new selection over the same source imports.
    let (retry, _) = env.inspected(vec![source]);
    let items = env.import(&retry, "op-3", json!([request(0, "Prusa cube", json!({}))]));
    assert_eq!(items[0]["outcome"], "imported", "{}", items[0]);
    assert_eq!(env.stored_bytes(&sha256), bytes);
}

// --- T6 8-9. Partial success, Unfiled, and row validation ---------------------------------

#[test]
fn one_rejected_item_leaves_the_others_committed() {
    let env = new_env(None);
    let parts = env.project("Parts");
    let ascii = fs::read(library_fixture("cube-ascii.stl")).unwrap();
    let (selection, _) = env.inspected(vec![
        env.source("cube-binary.stl"),
        env.source("cube-ascii.stl"),
        env.source("plain.gcode"),
    ]);

    let items = env.import(
        &selection,
        "op-1",
        json!([
            request(0, "Binary", json!({})),
            request(1, "Ascii", json!({ "projectIds": [parts, "prj-missing"] })),
            request(2, "Plain", json!({})),
        ]),
    );

    assert_eq!(items[0]["outcome"], "imported");
    assert_eq!(items[1]["outcome"], "rejected");
    assert_eq!(items[1]["errors"][0]["code"], "NOT_FOUND");
    assert_eq!(items[1]["errors"][0]["entityId"], "prj-missing");
    assert_eq!(items[2]["outcome"], "imported");
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 2);
    assert_eq!(env.rows_for_hash(&sha256_hex(&ascii)), (0, 0, 0));
    assert_eq!(env.count("SELECT COUNT(*) FROM project_models"), 0);
}

#[test]
fn an_unfiled_import_has_no_membership_and_repeated_project_ids_collapse() {
    let env = new_env(None);
    let parts = env.project("Parts");
    let quarry = env.project("Quarry");
    let (selection, _) = env.inspected(vec![
        env.source("cube-binary.stl"),
        env.source("cube-ascii.stl"),
    ]);
    env.take_library_events();

    let items = env.import(
        &selection,
        "op-1",
        json!([
            request(0, "Unfiled", json!({ "projectIds": [] })),
            request(
                1,
                "Filed",
                json!({ "projectIds": [quarry, parts, quarry, parts] })
            ),
        ]),
    );

    assert_eq!(items[0]["outcome"], "imported");
    assert_eq!(items[0]["model"]["projectIds"], json!([]));
    let unfiled = items[0]["model"]["id"].as_str().unwrap();
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM project_models WHERE model_id = '{unfiled}'"
        )),
        0
    );
    assert_eq!(items[1]["model"]["projectIds"], json!([parts, quarry]));
    assert_eq!(env.count("SELECT COUNT(*) FROM project_models"), 2);
    let project_events: Vec<Value> = env
        .take_library_events()
        .into_iter()
        .filter(|event| event["type"] == "library.project.changed")
        .map(|event| event["subject"]["id"].clone())
        .collect();
    assert_eq!(project_events, vec![json!(parts), json!(quarry)]);
}

#[test]
fn each_row_is_validated_before_it_commits() {
    let env = new_env(None);
    let (selection, _) = env.inspected(vec![env.source("cube-binary.stl")]);
    let too_many: Vec<String> = (0..65).map(|index| format!("prj-{index}")).collect();

    let items = env.import(&selection, "op-1", json!([request(0, "   ", json!({}))]));
    assert_eq!(items[0]["errors"][0]["code"], "VALIDATION");
    assert_eq!(items[0]["errors"][0]["fieldPath"], "name");

    let items = env.import(
        &selection,
        "op-2",
        json!([request(0, "Cube", json!({ "projectIds": too_many }))]),
    );
    assert_eq!(items[0]["errors"][0]["code"], "VALIDATION");
    assert_eq!(items[0]["errors"][0]["fieldPath"], "projectIds");

    let items = env.import(
        &selection,
        "op-3",
        json!([request(
            0,
            "Cube",
            json!({ "duplicateAction": "useExisting" })
        )]),
    );
    assert_eq!(items[0]["errors"][0]["code"], "VALIDATION");
    assert_eq!(items[0]["errors"][0]["fieldPath"], "targetModelId");

    let items = env.import(
        &selection,
        "op-4",
        json!([request(
            0,
            "Cube",
            json!({ "duplicateAction": "addRevision", "targetModelId": "mdl-x" })
        )]),
    );
    assert_eq!(items[0]["errors"][0]["code"], "VALIDATION");
    assert_eq!(items[0]["errors"][0]["fieldPath"], "targetExpectedRevision");
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 0);
    assert_eq!(env.count("SELECT COUNT(*) FROM content_blobs"), 0);
}
