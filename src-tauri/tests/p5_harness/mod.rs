//! The P5 IPC test harness shared by `p5_slicing.rs` and `p5_tracer.rs`:
//! a [`Farm`] of fresh roots with `fake-orca` installed as a configured
//! OrcaSlicer, and [`Running`], one app booted over those roots with its
//! `slicing.*` events recorded.
//!
//! `fake-orca` is installed as `<tmp>/orca/bin/orca-slicer` beside a copy
//! of the TestVendor preset fixtures in `<tmp>/orca/resources/profiles`, so
//! the engine supplies its own presets as an installed OrcaSlicer would.
//! Discovery is kept off this machine's `PATH` and home folders through the
//! services' test seam, and `FAKE_ORCA_*` scenarios reach the engine through
//! the extra-environment seam; production discovery is unchanged.
//!
//! Each test crate uses a different subset, hence the `dead_code` allow.
#![allow(dead_code)]

use std::ffi::OsString;
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
use farm3d_lib::slicing::invocation::WorkDir;
use farm3d_lib::slicing::operations::{recover_after_restart, Recovery};
use farm3d_lib::slicing::repository::{load_runtime_config, save_runtime_config};
use farm3d_lib::slicing::runtime::DiscoveryEnv;
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeResponseBody};
use tauri::test::{MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::Listener;

use crate::common::{a_catalog, a_ref_json, invoke, FakeModelFileIo};

pub const FAKE_ORCA: &str = env!("CARGO_BIN_EXE_fake-orca");
pub const DEADLINE: Duration = Duration::from_secs(20);

pub fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

pub fn fixtures() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

pub fn copy_dir(from: &Path, to: &Path) {
    fs::create_dir_all(to).unwrap();
    for entry in fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let target = to.join(entry.file_name());
        if entry.file_type().unwrap().is_dir() {
            copy_dir(&entry.path(), &target);
        } else {
            fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// `fake-orca` installed as `<root>/bin/orca-slicer`, with the preset
/// fixtures in `<root>/resources/profiles`.
pub fn install_orca(root: &Path) -> PathBuf {
    let bin = root.join("bin");
    fs::create_dir_all(&bin).unwrap();
    let engine = bin.join("orca-slicer");
    fs::copy(FAKE_ORCA, &engine).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&engine, fs::Permissions::from_mode(0o755)).unwrap();
    }
    copy_dir(
        &fixtures().join("profiles"),
        &root.join("resources/profiles"),
    );
    engine
}

/// Polls `probe` until it returns `Some`, or panics at the deadline.
pub fn wait_for<T>(what: &str, mut probe: impl FnMut() -> Option<T>) -> T {
    let deadline = Instant::now() + DEADLINE;
    loop {
        if let Some(found) = probe() {
            return found;
        }
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// One running app over the shared roots, recording its `slicing.*`
/// events.
pub struct Running {
    pub _app: tauri::App<MockRuntime>,
    pub webview: tauri::WebviewWindow<MockRuntime>,
    pub services: Arc<RuntimeServices<MockRuntime>>,
    pub events: Arc<Mutex<Vec<Value>>>,
    pub _credentials: tempfile::TempDir,
}

impl Running {
    /// Boots an app over `paths`, whose engine probe may take up to
    /// `probe_timeout`.
    pub fn boot(paths: &StoragePaths, lease: &MetadataRootLease, probe_timeout: Duration) -> Self {
        let storage = Arc::new(Storage::open(paths.clone(), lease).unwrap());
        let credentials = tempfile::tempdir().unwrap();
        let (app, webview, _manager, services) = crate::common::runtime_with_file_io(
            tauri::generate_handler![
                farm3d_lib::library::commands::inspect_import_selection,
                farm3d_lib::library::commands::import_models,
                farm3d_lib::library::commands::check_linked_sources,
                farm3d_lib::library::commands::delete_model,
                farm3d_lib::library::commands::list_library,
                farm3d_lib::library::commands::list_model_revisions,
                farm3d_lib::slicing::commands::get_slicer_runtime,
                farm3d_lib::slicing::commands::check_slicer_runtime,
                farm3d_lib::slicing::commands::pick_slicer_engine,
                farm3d_lib::slicing::commands::pick_preset_source,
                farm3d_lib::slicing::commands::reset_slicer_runtime,
                farm3d_lib::slicing::commands::list_slice_options,
                farm3d_lib::slicing::commands::get_revision_geometry,
                farm3d_lib::slicing::commands::get_revision_mesh,
                farm3d_lib::slicing::commands::list_slicing,
                farm3d_lib::slicing::commands::create_preparation,
                farm3d_lib::slicing::commands::update_preparation,
                farm3d_lib::slicing::commands::reload_preparation,
                farm3d_lib::slicing::commands::delete_preparation,
                farm3d_lib::slicing::commands::start_slice,
                farm3d_lib::slicing::commands::cancel_slice_operation,
                farm3d_lib::slicing::commands::get_slice_operation_log,
                farm3d_lib::slicing::commands::list_slice_revisions,
                farm3d_lib::slicing::commands::get_slice_revision,
                farm3d_lib::slicing::commands::get_slice_revision_log,
                farm3d_lib::slicing::commands::create_external_slice_revision,
                farm3d_lib::slicing::commands::delete_slice_revision,
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
                .is_some_and(|kind| kind.starts_with("slicing."))
            {
                recorded.lock().unwrap().push(value);
            }
        });
        services.slicing.set_discovery_env(DiscoveryEnv {
            home: None,
            path_var: None,
            probe_timeout,
        });
        farm3d_lib::start_library_runtime(
            &services,
            app.handle(),
            WatchPolicy::PollOnly {
                interval: Duration::from_secs(3600),
            },
        );
        farm3d_lib::start_slicing_runtime(&services, app.handle());
        Self {
            _app: app,
            webview,
            services,
            events,
            _credentials: credentials,
        }
    }

    pub fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        invoke(&self.webview, command, body).map(|response| response["data"].clone())
    }

    pub fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    pub fn error(&self, command: &str, body: Value) -> Value {
        match self.call(command, body) {
            Ok(value) => panic!("{command} unexpectedly succeeded: {value}"),
            Err(error) => error,
        }
    }

    /// `get_revision_mesh`'s raw bytes.
    pub fn mesh(&self, revision_id: &str, object_key: u32) -> Vec<u8> {
        let response = tauri::test::get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: "get_revision_mesh".to_string(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: json!({
                    "contractVersion": 1,
                    "revisionId": revision_id,
                    "objectKey": object_key,
                })
                .into(),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )
        .unwrap_or_else(|error| panic!("get_revision_mesh failed: {error}"));
        match response {
            InvokeResponseBody::Raw(bytes) => bytes,
            InvokeResponseBody::Json(json) => panic!("expected raw bytes, got {json}"),
        }
    }

    /// Sets the `FAKE_ORCA_*` variables the next slices run with.
    pub fn scenario(&self, variables: &[(&str, &str)]) {
        self.services.slicing.set_engine_environment(
            variables
                .iter()
                .map(|(name, value)| (OsString::from(name), OsString::from(value)))
                .collect(),
        );
    }

    /// Imports `path` and returns its Model record.
    pub fn import(&self, path: &Path, storage_mode: &str) -> Value {
        let selection = self
            .services
            .library
            .selections
            .register(SelectionPurpose::Import, vec![path.to_path_buf()])
            .selection_id;
        let inspection = self.ok(
            "inspect_import_selection",
            json!({ "selectionId": selection }),
        );
        assert_eq!(inspection["items"][0]["status"], "ready", "{inspection}");
        let imported = self.ok(
            "import_models",
            json!({
                "selectionId": selection,
                "operationId": format!("import-{}", uuid::Uuid::new_v4()),
                "items": [{
                    "fileIndex": 0,
                    "name": path.file_stem().unwrap().to_string_lossy(),
                    "projectIds": [],
                    "storageMode": storage_mode,
                    "acknowledgeUnsupported": true,
                }],
            }),
        );
        assert_eq!(imported["items"][0]["outcome"], "imported", "{imported}");
        imported["items"][0]["model"].clone()
    }

    pub fn prepare(&self, model_id: &str) -> Value {
        self.ok(
            "create_preparation",
            json!({
                "modelId": model_id,
                "target": { "kind": "profile", "catalogRef": a_ref_json() },
            }),
        )
    }

    /// D16: `create_external_slice_revision`.
    pub fn create_external(
        &self,
        operation_id: &str,
        source_revision_id: &str,
        facts: Value,
    ) -> Value {
        self.ok(
            "create_external_slice_revision",
            json!({
                "operationId": operation_id,
                "sourceRevisionId": source_revision_id,
                "facts": facts,
            }),
        )
    }

    pub fn start(&self, operation_id: &str, preparation: &Value, plates: &[&Value]) -> Vec<Value> {
        self.ok(
            "start_slice",
            json!({
                "operationId": operation_id,
                "preparationId": preparation["id"],
                "expectedRevision": preparation["revision"],
                "plateKeys": plates.iter().map(|plate| plate["plateKey"].clone()).collect::<Vec<_>>(),
            }),
        )["operations"]
            .as_array()
            .unwrap()
            .clone()
    }

    pub fn slicing(&self) -> Value {
        self.ok("list_slicing", json!({}))
    }

    pub fn operation(&self, id: &str) -> Value {
        self.slicing()["activeAndRecentOperations"]
            .as_array()
            .unwrap()
            .iter()
            .find(|operation| operation["id"] == id)
            .cloned()
            .unwrap_or_else(|| panic!("no operation {id}"))
    }

    /// Waits until operation `id` is in `state` and the stream has said
    /// so: the row commits before the worker cleans up and publishes, so
    /// the event is what marks the step finished.
    pub fn wait_state(&self, id: &str, state: &str) -> Value {
        wait_for(&format!("{id} to be {state}"), || {
            let announced = self.events_of(id).iter().any(|event| {
                event["type"] == "slicing.operation.changed" && event["payload"]["state"] == state
            });
            let operation = self.operation(id);
            (announced && operation["state"] == state).then_some(operation)
        })
    }

    /// The recorded engine pid of operation `id`.
    pub fn pid(&self, id: &str) -> i64 {
        self.services
            .storage
            .read(|connection| {
                connection.query_row(
                    "SELECT pid FROM slice_operations WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
            })
            .unwrap()
    }

    /// How many content blobs and Slice Revisions are stored.
    pub fn stored_counts(&self) -> (i64, i64) {
        self.services
            .storage
            .read(|connection| {
                Ok((
                    connection
                        .query_row("SELECT COUNT(*) FROM content_blobs", [], |row| row.get(0))?,
                    connection
                        .query_row("SELECT COUNT(*) FROM slice_revisions", [], |row| row.get(0))?,
                ))
            })
            .unwrap()
    }

    pub fn events_of(&self, id: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| event["subject"]["id"] == id)
            .cloned()
            .collect()
    }

    /// The `gcode_sha256` a Slice Revision row was stored with (there is no
    /// IPC command that exposes it directly).
    pub fn gcode_sha256(&self, revision_id: &str) -> String {
        self.services
            .storage
            .read(|connection| {
                connection.query_row(
                    "SELECT gcode_sha256 FROM slice_revisions WHERE id = ?1",
                    [revision_id],
                    |row| row.get(0),
                )
            })
            .unwrap()
    }

    pub fn blob(&self, sha256: &str) -> Vec<u8> {
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

    pub fn work_dir(&self, operation_id: &str) -> PathBuf {
        WorkDir::for_operation(self.services.storage.paths().content_root(), operation_id)
            .root()
            .to_path_buf()
    }
}

/// Fresh roots, an installed `fake-orca` configured as the engine, and a
/// folder for source files.
pub struct Farm {
    pub _roots: tempfile::TempDir,
    pub paths: StoragePaths,
    pub lease: MetadataRootLease,
    pub orca: tempfile::TempDir,
    pub engine: PathBuf,
    pub sources: tempfile::TempDir,
    /// How long an engine probe may take: 10 s for `fake-orca`, 30 s for a
    /// real OrcaSlicer, whose first AppImage mount can be slow.
    pub probe_timeout: Duration,
}

impl Farm {
    pub fn new() -> Self {
        let roots = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(roots.path().join("metadata"), roots.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let orca = tempfile::tempdir().unwrap();
        let engine = install_orca(orca.path());
        let storage = Storage::open(paths.clone(), &lease).unwrap();
        storage
            .write_repo(|tx| {
                let current = load_runtime_config(tx)?;
                save_runtime_config(tx, current.revision, Some(engine.to_str().unwrap()), None)
            })
            .unwrap();
        Self {
            _roots: roots,
            paths,
            lease,
            orca,
            engine,
            sources: tempfile::tempdir().unwrap(),
            probe_timeout: Duration::from_secs(10),
        }
    }

    /// A Farm whose engine is the real OrcaSlicer named by `FARM3D_ORCA`,
    /// with the TestVendor fixtures as the preset source so the test
    /// catalog's printer resolves (spec D23). Panics when `FARM3D_ORCA` is
    /// unset, so an ignored real-Orca test never passes without an engine.
    pub fn with_real_orca() -> Self {
        let engine = std::env::var_os("FARM3D_ORCA")
            .map(PathBuf::from)
            .expect("FARM3D_ORCA must name an OrcaSlicer engine; run through `just test-orca`");
        let mut farm = Self::new();
        let profiles = farm.orca.path().to_path_buf();
        // Real Orca requires "G92 E0" each layer with relative extrusion,
        // so this copy of the fixture printer adds it.
        let machine =
            profiles.join("resources/profiles/TestVendor/machine/TP/Test Printer 0.4 nozzle.json");
        let mut preset: Value = serde_json::from_slice(&fs::read(&machine).unwrap()).unwrap();
        preset["layer_change_gcode"] = json!("G92 E0");
        fs::write(&machine, serde_json::to_vec_pretty(&preset).unwrap()).unwrap();
        let storage = Storage::open(farm.paths.clone(), &farm.lease).unwrap();
        storage
            .write_repo(|tx| {
                let current = load_runtime_config(tx)?;
                save_runtime_config(
                    tx,
                    current.revision,
                    Some(engine.to_str().unwrap()),
                    Some(profiles.to_str().unwrap()),
                )
            })
            .unwrap();
        farm.engine = engine;
        farm.probe_timeout = Duration::from_secs(30);
        farm
    }

    pub fn start(&self) -> Running {
        Running::boot(&self.paths, &self.lease, self.probe_timeout)
    }

    /// A restart over the same roots: D10's startup recovery on a new
    /// `Storage`, run as `build_runtime_services` runs it before any
    /// command is served, then a new app. An earlier app may still be
    /// running, as a crashed farm3d's engine would be.
    pub fn restart(&self) -> (Recovery, Running) {
        let storage = Storage::open(self.paths.clone(), &self.lease).unwrap();
        let recovery = recover_after_restart(&storage).unwrap();
        (recovery, self.start())
    }

    /// Copies fixture `name` into the sources folder as `as_name`.
    pub fn source(&self, name: &str, as_name: &str) -> PathBuf {
        let path = self.sources.path().join(as_name);
        fs::copy(fixtures().join("library").join(name), &path).unwrap();
        path
    }
}

/// Whether process `pid` still exists and hasn't exited (a zombie has).
pub fn process_alive(pid: i64) -> bool {
    fs::read_to_string(format!("/proc/{pid}/stat"))
        .map(|stat| {
            let state = stat.rsplit_once(')').map(|(_, rest)| rest.trim_start());
            !state.is_some_and(|rest| rest.starts_with('Z'))
        })
        .unwrap_or(false)
}

pub fn plates(preparation: &Value) -> Vec<Value> {
    preparation["document"]["plates"]
        .as_array()
        .unwrap()
        .clone()
}

pub fn ids(operations: &[Value]) -> Vec<String> {
    operations
        .iter()
        .map(|operation| operation["id"].as_str().unwrap().to_string())
        .collect()
}

/// D16: `create_external_slice_revision`'s `facts` with every fact absent.
pub fn absent_facts() -> Value {
    json!({
        "printerProfile": {"kind": "absent"},
        "nozzleDiameterMm": {"kind": "absent"},
        "materialFamily": {"kind": "absent"},
        "filamentDiameterMm": {"kind": "absent"},
    })
}
