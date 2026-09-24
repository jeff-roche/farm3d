//! P4 Tasks 5-6: Library import (spec D6, D7, D13, D14, D17). Task 5 covers
//! selection, staging, and inspection: the native picker (through a fake
//! `ModelFileIo`), a window drop, the path policy, progress events,
//! cancellation, and expiry. Every command runs through the real IPC
//! handler, and a recording listener on `farm3d-event-v1` captures the
//! `library.*` events.

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
    assert!(
        scanned.iter().any(|name| name == "SelectionPurpose"),
        "the scan follows request types into their own files: {scanned:?}"
    );
}
