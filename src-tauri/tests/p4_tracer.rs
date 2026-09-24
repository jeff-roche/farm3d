//! P4 Task 14: the end-to-end tracer through the Tauri IPC path.
//!
//! Two Projects; one selection holding a managed STL and a linked two-plate
//! 3MF with unsupported entries; the STL filed in both Projects and the 3MF
//! in one. Then a restart: the first app's link supervisor is shut down
//! (P19: the AppHandle → services → supervisor cycle means dropping it does
//! not stop it), the test drops its handles to the first app, and a new
//! `Storage` and `RuntimeServices` are opened over the same metadata and
//! content roots, with `start_library_runtime` run as `build_runtime_services`
//! runs it. After the restart, the linked source is saved atomically with
//! new bytes (a revision), deleted (missing), and located in a new
//! directory (relinked with no revision). Every stored revision still opens
//! and verifies, and deleting a Project leaves every Model and revision in
//! place.
//!
//! The linked 3MF is the real OrcaSlicer export `orca-two-plates.3mf`: two
//! plates, and four unsupported entries to acknowledge.

mod common;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::library::links::WatchPolicy;
use farm3d_lib::library::selection::SelectionPurpose;
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::test::MockRuntime;
use tauri::Listener;

use common::{a_catalog, invoke, FakeModelFileIo};

const DEADLINE: Duration = Duration::from_secs(10);

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

fn fixture(name: &str) -> Vec<u8> {
    fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/library")
            .join(name),
    )
    .unwrap()
}

fn sha256(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// Polls `probe` until it returns `Some`, or panics at the deadline.
fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One running app: its own `Storage` over the shared roots, its services
/// with a started link supervisor, and the `library.*` events it emitted.
struct Running {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    events: Arc<Mutex<Vec<Value>>>,
    _credentials: tempfile::TempDir,
}

impl Running {
    fn start(paths: &StoragePaths, lease: &MetadataRootLease) -> Self {
        let storage = Arc::new(Storage::open(paths.clone(), lease).unwrap());
        let credentials = tempfile::tempdir().unwrap();
        let (app, webview, _manager, services) = common::runtime_with_file_io(
            tauri::generate_handler![
                farm3d_lib::library::commands::list_library,
                farm3d_lib::library::commands::create_project,
                farm3d_lib::library::commands::delete_project,
                farm3d_lib::library::commands::inspect_import_selection,
                farm3d_lib::library::commands::import_models,
                farm3d_lib::library::commands::list_model_revisions,
                farm3d_lib::library::commands::locate_linked_source,
            ],
            storage,
            Arc::new(a_catalog()),
            credentials.path().to_path_buf(),
            no_connection,
            Arc::new(FakeModelFileIo { picks: None }),
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&events);
        app.listen(STATUS_EVENT, move |event| {
            let value: Value = serde_json::from_str(event.payload()).unwrap();
            if value["type"]
                .as_str()
                .is_some_and(|kind| kind.starts_with("library."))
            {
                recorded.lock().unwrap().push(value);
            }
        });
        farm3d_lib::start_library_runtime(&services, app.handle(), WatchPolicy::native());
        Self {
            _app: app,
            webview,
            services,
            events,
            _credentials: credentials,
        }
    }

    /// Stops the link supervisor (P19), then drops the test's handles to
    /// this app. The supervisor cycle can keep the app's state alive, but
    /// nothing watches or writes after the shutdown.
    fn stop(self) {
        self.services
            .library
            .links
            .get()
            .expect("supervisor started")
            .shutdown();
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        invoke(&self.webview, command, body).map(|response| response["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    fn register(&self, purpose: SelectionPurpose, paths: Vec<PathBuf>) -> String {
        self.services
            .library
            .selections
            .register(purpose, paths)
            .selection_id
    }

    fn wait_until_reconciled(&self) {
        wait_for("the startup pass", || {
            let links = self.services.library.links.get()?;
            links.has_reconciled().then_some(())
        });
    }

    fn model(&self, model_id: &str) -> Value {
        let library = self.ok("list_library", json!({}));
        library["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == model_id)
            .unwrap_or_else(|| panic!("no Model {model_id} in {library}"))
            .clone()
    }

    fn revisions(&self, model_id: &str) -> Vec<Value> {
        self.ok("list_model_revisions", json!({ "modelId": model_id }))
            .as_array()
            .unwrap()
            .clone()
    }

    fn events_of(&self, model_id: &str, event_type: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["subject"]["id"] == model_id && event["type"] == event_type)
            .cloned()
            .collect()
    }

    /// The blob's bytes, read through the verifying reader to the end.
    fn blob(&self, sha256: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.services
            .library
            .content
            .open_verified(sha256)
            .unwrap_or_else(|error| panic!("blob {sha256} does not open: {error:?}"))
            .read_to_end(&mut bytes)
            .unwrap_or_else(|error| panic!("blob {sha256} does not verify: {error}"));
        bytes
    }
}

fn ids(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|id| id.as_str().unwrap().to_string())
        .collect()
}

#[test]
fn the_tracer_imports_restarts_follows_a_linked_source_and_deletes_a_project() {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let sources = tempfile::tempdir().unwrap();
    let running = Running::start(&paths, &lease);

    // 1. Two Projects.
    let brackets = running.ok("create_project", json!({ "name": "Brackets" }))["project"].clone();
    let calibration =
        running.ok("create_project", json!({ "name": "Calibration" }))["project"].clone();
    let brackets_id = brackets["id"].as_str().unwrap().to_string();
    let calibration_id = calibration["id"].as_str().unwrap().to_string();

    // 2. One selection: a managed STL and a linked two-plate 3MF.
    let stl_bytes = fixture("cube-binary.stl");
    let stl_path = sources.path().join("cube-binary.stl");
    fs::write(&stl_path, &stl_bytes).unwrap();
    let plates_bytes = fixture("orca-two-plates.3mf");
    let linked_dir = sources.path().join("linked");
    fs::create_dir(&linked_dir).unwrap();
    let linked_path = linked_dir.join("orca-two-plates.3mf");
    fs::write(&linked_path, &plates_bytes).unwrap();

    let selection = running.register(
        SelectionPurpose::Import,
        vec![stl_path.clone(), linked_path.clone()],
    );
    let inspection = running.ok(
        "inspect_import_selection",
        json!({ "selectionId": selection }),
    );
    let candidates = inspection["items"].as_array().unwrap();
    assert_eq!(candidates.len(), 2, "{inspection}");
    assert_eq!(candidates[0]["status"], "ready", "{inspection}");
    assert_eq!(candidates[0]["unsupported"], json!([]));
    assert_eq!(candidates[1]["status"], "ready", "{inspection}");
    assert_eq!(candidates[1]["summary"]["plateCount"], 2);
    let unsupported: Vec<&Value> = candidates[1]["unsupported"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| &entry["part"])
        .collect();
    assert_eq!(
        unsupported,
        vec![
            "Metadata/filament_sequence.json",
            "Metadata/model_settings.config",
            "Metadata/project_settings.config",
            "Metadata/slice_info.config",
        ]
    );

    let imported = running.ok(
        "import_models",
        json!({
            "selectionId": selection,
            "operationId": "tracer-import",
            "items": [
                {
                    "fileIndex": 0,
                    "name": "Cube",
                    "projectIds": [brackets_id, calibration_id],
                    "storageMode": "managed",
                    "acknowledgeUnsupported": false,
                },
                {
                    "fileIndex": 1,
                    "name": "Two plates",
                    "projectIds": [brackets_id],
                    "storageMode": "linked",
                    "acknowledgeUnsupported": true,
                },
            ],
        }),
    );
    let items = imported["items"].as_array().unwrap();
    assert_eq!(items[0]["outcome"], "imported", "{imported}");
    assert_eq!(items[1]["outcome"], "imported", "{imported}");
    let stl = items[0]["model"].clone();
    let linked = items[1]["model"].clone();
    let stl_id = stl["id"].as_str().unwrap().to_string();
    let linked_id = linked["id"].as_str().unwrap().to_string();
    assert_eq!(stl["storageMode"], "managed");
    assert_eq!(
        ids(&stl["projectIds"]),
        vec![brackets_id.clone(), calibration_id.clone()]
    );
    assert_eq!(linked["storageMode"], "linked");
    assert_eq!(linked["link"]["path"], linked_path.to_str().unwrap());
    assert_eq!(ids(&linked["projectIds"]), vec![brackets_id.clone()]);

    // 3. Both revision-1 hashes.
    let stl_revision_1 = stl["currentRevision"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    let linked_revision_1 = linked["currentRevision"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(stl_revision_1, sha256(&stl_bytes));
    assert_eq!(linked_revision_1, sha256(&plates_bytes));

    // 4. Restart over the same metadata and content roots.
    running.stop();
    let running = Running::start(&paths, &lease);
    running.wait_until_reconciled();
    let library = running.ok("list_library", json!({}));
    let models = library["models"].as_array().unwrap();
    assert_eq!(models.len(), 2, "{library}");
    let stl_after = running.model(&stl_id);
    let linked_after = running.model(&linked_id);
    assert_eq!(stl_after["currentRevision"]["sha256"], stl_revision_1);
    assert_eq!(
        ids(&stl_after["projectIds"]),
        vec![brackets_id.clone(), calibration_id.clone()]
    );
    assert_eq!(linked_after["currentRevision"]["sha256"], linked_revision_1);
    assert_eq!(ids(&linked_after["projectIds"]), vec![brackets_id.clone()]);
    assert_eq!(linked_after["link"]["state"], "ok");
    assert_eq!(linked_after["link"]["watchMode"], "watching");
    assert_eq!(linked_after["revisionCount"], 1);

    // 5. An atomic save of different bytes: exactly one new revision.
    let modified = fixture("core-two-objects.3mf");
    let temporary = linked_dir.join("orca-two-plates.3mf.tmp");
    fs::write(&temporary, &modified).unwrap();
    fs::rename(&temporary, &linked_path).unwrap();
    let created = wait_for("library.revision.created", || {
        running
            .events_of(&linked_id, "library.revision.created")
            .into_iter()
            .next()
    });
    assert_eq!(created["payload"]["sequence"], 2);
    assert_eq!(created["payload"]["origin"], "linkedChange");
    assert_eq!(created["payload"]["sha256"], sha256(&modified));

    // 6. Deleted: missing.
    fs::remove_file(&linked_path).unwrap();
    wait_for("the linked Model to go missing", || {
        (running.model(&linked_id)["link"]["state"] == "missing").then_some(())
    });

    // 7. Located in a new directory with the revision-2 bytes: relinked,
    // with no new revision.
    let moved_dir = sources.path().join("moved");
    fs::create_dir(&moved_dir).unwrap();
    let moved_path = moved_dir.join("two-plates.3mf");
    fs::write(&moved_path, &modified).unwrap();
    let locate = running.register(SelectionPurpose::Locate, vec![moved_path.clone()]);
    let expected_revision = running.model(&linked_id)["revision"].clone();
    let located = running.ok(
        "locate_linked_source",
        json!({
            "modelId": linked_id,
            "expectedRevision": expected_revision,
            "selectionId": locate,
            "fileIndex": 0,
            "acceptDifferentContent": false,
        }),
    );
    let relinked = &located["model"];
    assert_eq!(relinked["link"]["state"], "ok");
    assert_eq!(relinked["link"]["path"], moved_path.to_str().unwrap());
    assert_eq!(
        relinked["revisionCount"], 2,
        "same content adds no revision"
    );
    assert_eq!(relinked["currentRevision"]["sequence"], 2);
    assert_eq!(
        running
            .events_of(&linked_id, "library.revision.created")
            .len(),
        1
    );

    // 8. Every revision of both Models opens and verifies; revision 1 of
    // the linked Model is still the original bytes.
    let linked_revisions = running.revisions(&linked_id);
    assert_eq!(linked_revisions.len(), 2);
    for revision in running
        .revisions(&stl_id)
        .iter()
        .chain(linked_revisions.iter())
    {
        let hash = revision["sha256"].as_str().unwrap();
        assert_eq!(sha256(&running.blob(hash)), hash);
    }
    let first = linked_revisions
        .iter()
        .find(|revision| revision["sequence"] == 1)
        .unwrap();
    assert_eq!(first["sha256"], linked_revision_1);
    assert_eq!(running.blob(&linked_revision_1), plates_bytes);

    // 9. Deleting a Project removes only its memberships.
    let deleted = running.ok(
        "delete_project",
        json!({ "id": calibration_id, "expectedRevision": calibration["revision"] }),
    );
    assert_eq!(ids(&deleted["affectedModelIds"]), vec![stl_id.clone()]);
    assert_eq!(deleted["nowUnfiledModelIds"], json!([]));
    let library = running.ok("list_library", json!({}));
    let projects: Vec<&Value> = library["projects"]
        .as_array()
        .unwrap()
        .iter()
        .map(|project| &project["id"])
        .collect();
    assert_eq!(projects, vec![&json!(brackets_id)]);
    assert_eq!(library["models"].as_array().unwrap().len(), 2);
    assert_eq!(
        ids(&running.model(&stl_id)["projectIds"]),
        vec![brackets_id.clone()]
    );
    assert_eq!(
        ids(&running.model(&linked_id)["projectIds"]),
        vec![brackets_id.clone()]
    );
    assert_eq!(running.revisions(&stl_id).len(), 1);
    assert_eq!(running.revisions(&linked_id).len(), 2);
    assert_eq!(running.blob(&stl_revision_1), stl_bytes);
    assert_eq!(running.blob(&linked_revision_1), plates_bytes);
    assert_eq!(running.blob(&sha256(&modified)), modified);
    running.stop();
}
