//! P4 Task 8: linked-source watching, reconciliation, and recovery (spec
//! D5, D15, D16). Every test uses a real filesystem under a temp directory
//! and, unless it says otherwise, real native `notify` watches. Commands
//! run through the real IPC handler, and a recording listener on
//! `farm3d-event-v1` captures the `library.*` events.
//!
//! Waits poll the recorded events (or the stored record) against a 10 s
//! deadline; nothing sleeps for a fixed time except the two checks that
//! nothing further happens after a change.

mod common;

use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::library::content::ContentFailurePoint;
use farm3d_lib::library::links::WatchPolicy;
use farm3d_lib::library::selection::SelectionPurpose;
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use tauri::test::MockRuntime;
use tauri::Listener;

use common::{a_catalog, invoke, FakeModelFileIo};

const CUBE_BINARY_SHA256: &str = "19f725d85d26793c9681fdf8d070c8b78e13d05f1b06188b4d83cfd0fdc9017c";
const DEADLINE: Duration = Duration::from_secs(10);
/// How long a test waits to be sure no further event follows. The debounce
/// is 750 ms; Gate C saw every batch within 826 ms.
const QUIET: Duration = Duration::from_secs(2);

// --- Fixture -------------------------------------------------------------------

/// What survives a restart: the storage, its lease, and the source files.
struct Host {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Arc<Storage>,
    sources: tempfile::TempDir,
}

impl Host {
    fn new() -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let storage = Arc::new(Storage::open(paths, &lease).unwrap());
        Self {
            _temp: temp,
            _lease: lease,
            storage,
            sources: tempfile::tempdir().unwrap(),
        }
    }

    /// `sources/<relative>`, with its parent directories created.
    fn source(&self, relative: &str, bytes: &[u8]) -> PathBuf {
        let path = self.sources.path().join(relative);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(&path, bytes).unwrap();
        path
    }

    fn start(&self, policy: WatchPolicy) -> Running {
        let credentials = tempfile::tempdir().unwrap();
        let (app, webview, _manager, services) = common::runtime_with_file_io(
            tauri::generate_handler![
                farm3d_lib::library::commands::inspect_import_selection,
                farm3d_lib::library::commands::import_models,
                farm3d_lib::library::commands::list_model_revisions,
                farm3d_lib::library::commands::check_linked_sources,
                farm3d_lib::library::commands::locate_linked_source,
                farm3d_lib::library::commands::convert_model_to_managed,
            ],
            Arc::clone(&self.storage),
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
        farm3d_lib::start_library_runtime(&services, app.handle(), policy);
        Running {
            _app: app,
            webview,
            services,
            events,
            _credentials: credentials,
        }
    }
}

/// One running app over a [`Host`]. Dropping it stops its link supervisor,
/// so a restarted app never shares a watch with the one before it (P19).
struct Running {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    events: Arc<Mutex<Vec<Value>>>,
    _credentials: tempfile::TempDir,
}

impl Drop for Running {
    fn drop(&mut self) {
        if let Some(links) = self.services.library.links.get() {
            links.shutdown();
        }
    }
}

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

impl Running {
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

    fn register(&self, purpose: SelectionPurpose, path: &Path) -> String {
        self.services
            .library
            .selections
            .register(purpose, vec![path.to_path_buf()])
            .selection_id
    }

    /// Imports `path` as a linked Model named by its file stem, then clears
    /// the recorded events, so waits see only what follows the import.
    /// Returns the item result.
    fn import_linked(&self, path: &Path) -> Value {
        let selection_id = self.register(SelectionPurpose::Import, path);
        self.ok(
            "inspect_import_selection",
            json!({ "selectionId": selection_id }),
        );
        let result = self.ok(
            "import_models",
            json!({
                "selectionId": selection_id,
                "operationId": "op-1",
                "items": [{
                    "fileIndex": 0,
                    "name": path.file_stem().unwrap().to_str().unwrap(),
                    "projectIds": [],
                    "storageMode": "linked",
                    "acknowledgeUnsupported": false,
                }],
            }),
        );
        let item = result["items"][0].clone();
        assert_eq!(item["outcome"], "imported", "{item}");
        self.clear_events();
        item
    }

    fn links(&self) -> &farm3d_lib::library::LinkSupervisor<MockRuntime> {
        self.services
            .library
            .links
            .get()
            .expect("supervisor started")
    }

    /// The Model's record as commands return it.
    fn record(&self, model_id: &str) -> Value {
        let record = self
            .services
            .storage
            .read(|connection| Ok(self.services.library.record_for(connection, model_id)))
            .unwrap()
            .unwrap()
            .expect("model exists");
        serde_json::to_value(record).unwrap()
    }

    /// The recorded `library.*` events about Model `model_id`.
    fn model_events(&self, model_id: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["subject"]["id"] == model_id)
            .cloned()
            .collect()
    }

    fn clear_events(&self) {
        self.events.lock().unwrap().clear();
    }

    fn count(&self, model_id: &str, event_type: &str) -> usize {
        self.model_events(model_id)
            .iter()
            .filter(|event| event["type"] == event_type)
            .count()
    }

    /// Waits for a `library.model.changed` for `model_id` whose source
    /// state is `state`, and returns its payload.
    fn wait_for_state(&self, model_id: &str, state: &str) -> Value {
        wait_for(&format!("{model_id} to become {state}"), || {
            self.model_events(model_id)
                .into_iter()
                .filter(|event| event["type"] == "library.model.changed")
                .map(|event| event["payload"].clone())
                .find(|payload| payload["link"]["state"] == state)
        })
    }

    fn wait_until_reconciled(&self) {
        wait_for("the startup pass", || {
            self.links().has_reconciled().then_some(())
        });
    }

    fn blob(&self, sha256: &str) -> Vec<u8> {
        let mut bytes = Vec::new();
        self.services
            .library
            .content
            .open_verified(sha256)
            .unwrap()
            .read_to_end(&mut bytes)
            .unwrap();
        bytes
    }
}

fn model_id(item: &Value) -> String {
    item["model"]["id"].as_str().unwrap().to_string()
}

// --- 1. Import linked, then restart --------------------------------------------

#[test]
fn a_linked_model_survives_a_restart_and_an_edit_while_stopped_is_captured() {
    let host = Host::new();
    let path = host.source("proj/part.stl", &fixture("cube-binary.stl"));
    let running = host.start(WatchPolicy::native());
    let item = running.import_linked(&path);
    let id = model_id(&item);
    assert_eq!(item["model"]["link"]["watchMode"], "watching");
    drop(running);

    let restarted = host.start(WatchPolicy::native());
    restarted.wait_until_reconciled();
    let record = restarted.record(&id);
    assert_eq!(record["link"]["state"], "ok");
    assert_eq!(record["revisionCount"], 1);
    assert_eq!(record["revision"], 1, "an unchanged source writes nothing");
    assert_eq!(record["link"]["watchMode"], "watching");
    assert_eq!(restarted.count(&id, "library.revision.created"), 0);
    drop(restarted);

    fs::write(&path, fixture("cube-ascii.stl")).unwrap();
    let restarted = host.start(WatchPolicy::native());
    restarted.wait_until_reconciled();
    let record = restarted.record(&id);
    assert_eq!(record["revisionCount"], 2);
    assert_eq!(record["currentRevision"]["origin"], "linkedChange");
    assert_eq!(restarted.count(&id, "library.revision.created"), 1);
}

// --- 2. Atomic save ------------------------------------------------------------

#[test]
fn an_atomic_save_adds_exactly_one_revision_and_keeps_revision_1_readable() {
    let host = Host::new();
    let original = fixture("cube-binary.stl");
    let path = host.source("proj/part.stl", &original);
    let running = host.start(WatchPolicy::native());
    let id = model_id(&running.import_linked(&path));

    let temporary = path.with_file_name("x.tmp");
    fs::write(&temporary, fixture("cube-ascii.stl")).unwrap();
    fs::rename(&temporary, &path).unwrap();

    let created = wait_for("library.revision.created", || {
        running
            .model_events(&id)
            .into_iter()
            .find(|event| event["type"] == "library.revision.created")
    });
    assert_eq!(created["payload"]["sequence"], 2);
    assert_eq!(created["payload"]["origin"], "linkedChange");
    assert_eq!(created["payload"]["sourceFileName"], "part.stl");
    std::thread::sleep(QUIET);
    assert_eq!(
        running.count(&id, "library.revision.created"),
        1,
        "one save is one check"
    );
    assert_eq!(
        running.count(&id, "library.model.changed"),
        1,
        "no duplicate library.model.changed"
    );
    assert_eq!(
        running.blob(CUBE_BINARY_SHA256),
        original,
        "revision 1 still opens"
    );
}

// --- 3. Delete, then restore ---------------------------------------------------

#[test]
fn a_deleted_source_goes_missing_and_recovers_when_restored() {
    let host = Host::new();
    let original = fixture("cube-binary.stl");
    let path = host.source("proj/part.stl", &original);
    let running = host.start(WatchPolicy::native());
    let id = model_id(&running.import_linked(&path));

    fs::remove_file(&path).unwrap();
    let missing = running.wait_for_state(&id, "missing");
    assert_eq!(missing["revisionCount"], 1);

    fs::write(&path, &original).unwrap();
    let recovered = running.wait_for_state(&id, "ok");
    assert_eq!(
        recovered["revisionCount"], 1,
        "identical bytes add no revision"
    );
    assert_eq!(running.count(&id, "library.revision.created"), 0);
}

// --- 4. Parent directory -------------------------------------------------------

#[test]
fn a_removed_parent_directory_is_followed_through_its_ancestor() {
    let host = Host::new();
    let original = fixture("cube-binary.stl");
    let path = host.source("proj/part.stl", &original);
    let parent = path.parent().unwrap().to_path_buf();
    let running = host.start(WatchPolicy::native());
    let id = model_id(&running.import_linked(&path));

    fs::remove_dir_all(&parent).unwrap();
    running.wait_for_state(&id, "missing");
    wait_for("the ancestor watch", || {
        let watched = running.links().watched_directories();
        (watched == vec![(host.sources.path().to_path_buf(), 1)]).then_some(())
    });

    fs::create_dir(&parent).unwrap();
    fs::write(&path, &original).unwrap();
    let recovered = running.wait_for_state(&id, "ok");
    assert_eq!(recovered["revisionCount"], 1);
    wait_for("the parent watch", || {
        (running.links().watched_directories() == vec![(parent.clone(), 1)]).then_some(())
    });

    // The restored parent's own watch works: an edit is captured.
    fs::write(&path, fixture("cube-ascii.stl")).unwrap();
    wait_for("the edit's revision", || {
        (running.count(&id, "library.revision.created") == 1).then_some(())
    });
}

// --- 5. Locate -----------------------------------------------------------------

#[test]
fn locate_relinks_same_content_and_needs_consent_for_different_content() {
    let host = Host::new();
    let original = fixture("cube-binary.stl");
    let path = host.source("a/part.stl", &original);
    let running = host.start(WatchPolicy::native());
    let id = model_id(&running.import_linked(&path));

    // Moved elsewhere: missing, then Locate by the same content.
    let moved = host.sources.path().join("b/moved.stl");
    fs::create_dir_all(moved.parent().unwrap()).unwrap();
    fs::rename(&path, &moved).unwrap();
    running.wait_for_state(&id, "missing");
    running.clear_events();
    let revision = running.record(&id)["revision"].clone();
    let selection = running.register(SelectionPurpose::Locate, &moved);
    let relinked = running.ok(
        "locate_linked_source",
        json!({
            "modelId": id, "expectedRevision": revision, "selectionId": selection,
            "fileIndex": 0, "acceptDifferentContent": false,
        }),
    );
    let model = &relinked["model"];
    assert_eq!(model["link"]["path"], moved.to_str().unwrap());
    assert_eq!(model["link"]["state"], "ok");
    assert_eq!(model["link"]["watchMode"], "watching");
    assert_eq!(
        model["revisionCount"], 1,
        "same content relinks with no revision"
    );
    assert_eq!(running.count(&id, "library.revision.created"), 0);
    assert_eq!(running.count(&id, "library.model.changed"), 1);

    // Different content without consent.
    let other_bytes = fixture("cube-ascii.stl");
    let other = host.source("c/other.stl", &other_bytes);
    let selection = running.register(SelectionPurpose::Locate, &other);
    let locate = |accept: bool| {
        json!({
            "modelId": id, "expectedRevision": running.record(&id)["revision"],
            "selectionId": selection, "fileIndex": 0, "acceptDifferentContent": accept,
        })
    };
    let error = running.err("locate_linked_source", locate(false));
    assert_eq!(error["code"], "SOURCE_CONTENT_DIFFERS");
    assert_eq!(
        error["details"],
        json!({
            "currentSha256": CUBE_BINARY_SHA256,
            "locatedSha256": sha256(&other_bytes),
            "locatedFileName": "other.stl",
        })
    );
    let sources = host.sources.path().to_str().unwrap();
    assert!(
        !error.to_string().contains(sources),
        "basenames only: {error}"
    );

    // With consent: relinked, with a `relocate` revision.
    let relocated = running.ok("locate_linked_source", locate(true));
    let model = &relocated["model"];
    assert_eq!(model["link"]["path"], other.to_str().unwrap());
    assert_eq!(model["revisionCount"], 2);
    assert_eq!(model["currentRevision"]["origin"], "relocate");
    assert_eq!(model["currentRevision"]["sequence"], 2);
    assert_eq!(running.count(&id, "library.revision.created"), 1);

    // A 3MF for an STL Model.
    let package = host.source("d/package.3mf", &fixture("core-two-objects.3mf"));
    let selection = running.register(SelectionPurpose::Locate, &package);
    let error = running.err(
        "locate_linked_source",
        json!({
            "modelId": id, "expectedRevision": running.record(&id)["revision"],
            "selectionId": selection, "fileIndex": 0, "acceptDifferentContent": true,
        }),
    );
    assert_eq!(error["code"], "VALIDATION");
    assert_eq!(running.record(&id)["revisionCount"], 2);
}

// --- 6. Convert to managed while missing ---------------------------------------

#[test]
fn converting_a_missing_model_to_managed_stops_following_its_source() {
    let host = Host::new();
    let original = fixture("cube-binary.stl");
    let path = host.source("proj/part.stl", &original);
    let running = host.start(WatchPolicy::native());
    let id = model_id(&running.import_linked(&path));
    let parent = path.parent().unwrap().to_path_buf();
    assert_eq!(running.links().watched_directories(), vec![(parent, 1)]);

    fs::remove_file(&path).unwrap();
    let missing = running.wait_for_state(&id, "missing");
    let converted = running.ok(
        "convert_model_to_managed",
        json!({ "modelId": id, "expectedRevision": missing["revision"] }),
    );
    let model = &converted["model"];
    assert_eq!(model["storageMode"], "managed");
    assert_eq!(model["link"], Value::Null);
    assert_eq!(model["revisionCount"], 1);
    assert!(
        running.links().watched_directories().is_empty(),
        "the directory's watch is released"
    );
    let revisions = running.ok("list_model_revisions", json!({ "modelId": id }));
    assert_eq!(
        revisions.as_array().unwrap().len(),
        1,
        "revisions stay readable"
    );

    running.clear_events();
    fs::write(&path, fixture("cube-ascii.stl")).unwrap();
    std::thread::sleep(QUIET);
    assert!(
        running.model_events(&id).is_empty(),
        "nothing follows the file now"
    );
    assert_eq!(running.record(&id)["storageMode"], "managed");
}

// --- 7. Manual check -----------------------------------------------------------

#[test]
fn check_linked_sources_returns_only_changed_models_without_a_watcher() {
    let host = Host::new();
    let first = host.source("one/a.stl", &fixture("cube-binary.stl"));
    let second = host.source("two/b.stl", &fixture("cube-ascii.stl"));
    let running = host.start(WatchPolicy::PollOnly {
        interval: Duration::from_secs(3600),
    });
    let first_item = running.import_linked(&first);
    let second_item = running.import_linked(&second);
    for item in [&first_item, &second_item] {
        assert_eq!(item["model"]["link"]["watchMode"], "polling");
        assert_eq!(
            item["warnings"],
            json!([]),
            "polling by policy is not a warning"
        );
    }
    let (first_id, second_id) = (model_id(&first_item), model_id(&second_item));

    fs::write(&first, fixture("cube-binary-solid-header.stl")).unwrap();
    let changed = running.ok("check_linked_sources", json!({}));
    let changed = changed.as_array().unwrap();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0]["id"], first_id.as_str());
    assert_eq!(changed[0]["currentRevision"]["sequence"], 2);
    assert_eq!(running.count(&first_id, "library.revision.created"), 1);

    assert_eq!(running.ok("check_linked_sources", json!({})), json!([]));
    assert_eq!(
        running.ok("check_linked_sources", json!({ "modelIds": [second_id] })),
        json!([])
    );
}

#[test]
fn a_failed_check_does_not_stop_check_linked_sources_checking_the_rest() {
    let host = Host::new();
    let first = host.source("one/a.stl", &fixture("cube-binary.stl"));
    let second = host.source("two/b.stl", &fixture("cube-ascii.stl"));
    let running = host.start(WatchPolicy::PollOnly {
        interval: Duration::from_secs(3600),
    });
    let first_id = model_id(&running.import_linked(&first));
    let second_id = model_id(&running.import_linked(&second));

    // farm3d's own store fails while capturing the first Model's change.
    fs::write(&first, fixture("cube-binary-solid-header.stl")).unwrap();
    fs::write(&second, fixture("cube-for-slicers.stl")).unwrap();
    running
        .services
        .library
        .content
        .inject_failure_once(ContentFailurePoint::AfterPlacementBeforeCommit);
    let changed = running.ok(
        "check_linked_sources",
        json!({ "modelIds": [first_id, second_id] }),
    );
    let changed = changed.as_array().unwrap();
    assert_eq!(changed.len(), 1, "{changed:?}");
    assert_eq!(changed[0]["id"], second_id.as_str());
    assert_eq!(running.record(&first_id)["currentRevision"]["sequence"], 1);

    // The failed Model is picked up by the next check.
    let changed = running.ok("check_linked_sources", json!({ "modelIds": [first_id] }));
    assert_eq!(changed[0]["id"], first_id.as_str(), "{changed}");

    // When every check fails there is nothing to return, so the error is.
    fs::write(&first, fixture("cube-ascii.stl")).unwrap();
    running
        .services
        .library
        .content
        .inject_failure_once(ContentFailurePoint::AfterPlacementBeforeCommit);
    let error = running.err("check_linked_sources", json!({ "modelIds": [first_id] }));
    assert_eq!(error["code"], "PERSISTENCE_UNAVAILABLE", "{error}");
}

// --- 8. Watch registration failure ---------------------------------------------

#[test]
fn a_failed_watch_falls_back_to_polling_with_a_warning() {
    let host = Host::new();
    let path = host.source("proj/part.stl", &fixture("cube-binary.stl"));
    let running = host.start(WatchPolicy::native());
    running.links().fail_next_native_watch();

    let item = running.import_linked(&path);

    assert_eq!(item["model"]["link"]["watchMode"], "polling");
    let warnings: Vec<&Value> = item["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|warning| warning["code"] == "WATCH_UNAVAILABLE")
        .collect();
    assert_eq!(warnings.len(), 1, "{item}");
    let sources = host.sources.path().to_str().unwrap();
    assert!(
        !warnings[0].to_string().contains(sources),
        "no path in the warning"
    );
    assert_eq!(
        running.record(&model_id(&item))["link"]["watchMode"],
        "polling"
    );
}
