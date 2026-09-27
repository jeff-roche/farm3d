//! The P7 dispatch rig shared by `p7_jobs.rs` (Task 8a's handoff tests) and
//! `p7_restart_matrix.rs`: durable roots (a leased Storage, a credential
//! directory, and a `FakeMoonraker` Printer with one Material Slot), a farm3d
//! Slice Revision whose G-code is in the content store and whose facts match
//! the Printer, and [`boot`], which builds `RuntimeServices` over those roots
//! the way `build_runtime_services` does: host-ops recovery, then Job
//! recovery, then the runtimes. Every boot after the first is a restart
//! (`p6_tracer.rs`'s pattern).
//!
//! Each test crate uses a different subset, hence the `dead_code` allow.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::connections::capabilities::{
    ArtifactStaging, CapabilityEvidence, CapabilityMap, CapabilityState, EvidenceTier, HostFacts,
    HostStateQuery, PrintControl, PrinterCapabilities,
};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::supervisor::{ConnectionManager, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND,
};
use farm3d_lib::host_ops::repository as host_ops_repo;
use farm3d_lib::host_ops::{
    self, CapabilityFactory, Clock, HostOperation, HostOperationServices, HostOperationState,
    HostOpsTimings, SystemClock,
};
use farm3d_lib::library::content::ContentStore;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::operational::{OperationalState, TelemetryFreshness};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{StartSafety, StoredPrinter};
use farm3d_lib::slicing::facts::{Farm3dFacts, ProfileSnapshot};
use farm3d_lib::slicing::repository::{insert_farm3d_revision, NewFarm3dRevision};
use farm3d_lib::slicing::{
    RuntimeChannel, SliceControls, SliceEstimates, SlicePlateRef, SliceRevisionTarget,
    SliceRuntimeInfo, SliceTarget,
};
use farm3d_lib::spools::ledger::AmountEntry;
use farm3d_lib::spools::slots;
use farm3d_lib::spools::{
    repository as spools_repository, AmountConfidence, FilamentDiameter, MaterialFamily,
    SpoolFields,
};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;

use crate::common;
use crate::common::fake_moonraker::FakeMoonraker;

pub const PRINTER: &str = "prn-fake";
pub const SLR: &str = "slr-dispatch";
pub const HOST_PATH: &str = "farm3d/slr-dispatch.gcode";
/// 12.5 g, the revision's slice estimate.
pub const ESTIMATE_MG: i64 = 12_500;
pub const SECRET: &str = "SEEDED-P7-DISPATCH-KEY-7c1e";
const CREDENTIAL_REF: &str = "farm3d/printer/prn-fake/apikey";
const NOW: &str = "2026-09-27T00:00:00Z";
/// How long any one wait may take.
pub const WAIT: Duration = Duration::from_secs(15);

// --- capabilities: every capability supported with sim evidence ---------------

struct SimFactory;

fn short_moonraker_timings() -> MoonrakerTimings {
    MoonrakerTimings {
        connect: Duration::from_millis(500),
        query: Duration::from_millis(1500),
        control: Duration::from_millis(1500),
        transfer_base: Duration::from_millis(1500),
        transfer_per_started_mib: Duration::from_millis(10),
    }
}

fn adapter(
    config: &ConnectionConfig,
    key: Option<zeroize::Zeroizing<String>>,
) -> Option<MoonrakerCapabilities> {
    (config.kind == MOONRAKER_KIND)
        .then(|| MoonrakerCapabilities::new(config, key, short_moonraker_timings()))
}

impl CapabilityFactory for SimFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        PrinterCapabilities {
            printer_id: printer.id.clone(),
            adapter_kind: Some(MOONRAKER_KIND.to_string()),
            capabilities: CapabilityMap::complete(|_| CapabilityState::Supported {
                evidence: CapabilityEvidence {
                    source: "tests/p7_dispatch_rig".to_string(),
                    tier: EvidenceTier::Sim,
                    verified_host_versions: Vec::new(),
                },
            }),
            host_facts: host_facts.cloned(),
            observed_at: None,
        }
    }

    fn staging(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn ArtifactStaging>> {
        adapter(config, key).map(|adapter| Box::new(adapter) as Box<dyn ArtifactStaging>)
    }

    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        adapter(config, key).map(|adapter| Box::new(adapter) as Box<dyn PrintControl>)
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        adapter(config, key).map(|adapter| Box::new(adapter) as Box<dyn HostStateQuery>)
    }
}

/// A short verification window, and automatic retries an hour out so none
/// races a test's own steps.
fn host_ops_timings() -> HostOpsTimings {
    HostOpsTimings {
        verify_window: Duration::from_millis(800),
        verify_poll_interval: Duration::from_millis(50),
        backoff: |_| Duration::from_secs(3600),
        ..HostOpsTimings::default()
    }
}

fn unused_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

pub fn gcode() -> Vec<u8> {
    let mut bytes = b"; farm3d P7 dispatch artifact\n".to_vec();
    for line in 0..200 {
        bytes.extend_from_slice(format!("G1 X{line} Y{line}\n").as_bytes());
    }
    bytes
}

fn profile_snapshot() -> ProfileSnapshot {
    ProfileSnapshot::new(
        common::a_ref(),
        &farm3d_lib::catalog::PrinterProfile::from(&common::a_catalog().models[0].variants[0]),
    )
}

/// A farm3d Slice Revision (PLA 1.75, 0.4 mm, 12.5 g) whose G-code is in
/// the content store, so an upload has real bytes to send.
fn seed_slice_revision(storage: &Arc<Storage>) {
    let bytes = gcode();
    let content = ContentStore::open(storage.paths().content_root()).unwrap();
    let staged = content
        .stage_bytes(&bytes, "seed-dispatch", "part.gcode")
        .unwrap();
    let sha256 = staged.sha256.clone();
    let size = bytes.len() as i64;
    content
        .place_and_commit(storage, &[&staged], |tx| {
            tx.execute_batch(&format!(
                "INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
                   VALUES ('mdl-dispatch', 1, 'Bracket', 'stl', 'managed', '{NOW}', '{NOW}');
                 INSERT INTO model_source_revisions(
                   id, model_id, sequence, content_sha256, size_bytes, format, origin,
                   source_file_name, source_path, captured_at, inspector_version, inspection_json
                 ) VALUES ('msr-dispatch', 'mdl-dispatch', 1, '{sha256}', {size}, 'stl', 'import',
                           'part.stl', '/src/part.stl', '{NOW}', 1, '{{}}');"
            ))
            .map_err(RepositoryError::from)?;
            insert_farm3d_revision(
                tx,
                &NewFarm3dRevision {
                    id: SLR.to_string(),
                    source_revision_id: "msr-dispatch".to_string(),
                    plate: SlicePlateRef {
                        plate_key: "plate-1".to_string(),
                        plate_index: 1,
                        plate_name: None,
                    },
                    gcode_sha256: sha256.clone(),
                    gcode_size: size,
                    target: SliceRevisionTarget {
                        target: SliceTarget::Profile {
                            catalog_ref: common::a_ref(),
                        },
                        profile: profile_snapshot(),
                        machine_preset: "Test Printer 0.4 nozzle".to_string(),
                        process_preset: "0.20mm Standard".to_string(),
                        filament_preset: "Generic PLA".to_string(),
                        controls: SliceControls::default(),
                    },
                    facts: Farm3dFacts::new(profile_snapshot(), 0.4, MaterialFamily::Pla, None, 1.75),
                    estimates: SliceEstimates {
                        print_seconds: Some(60),
                        filament_grams: Some(12.5),
                        ..SliceEstimates::none()
                    },
                    runtime: SliceRuntimeInfo {
                        engine_version: "2.4.2".to_string(),
                        engine_channel: RuntimeChannel::Release,
                        preset_source_version: "2.4.2".to_string(),
                        preset_source_channel: RuntimeChannel::Release,
                    },
                    blobs: Vec::new(),
                },
            )?;
            Ok(())
        })
        .unwrap();
}

// --- roots ---------------------------------------------------------------------

/// What survives a restart: the data roots, the lease, the credential store,
/// and the printer itself.
pub struct Roots {
    _temp: tempfile::TempDir,
    pub paths: StoragePaths,
    pub lease: MetadataRootLease,
    credentials: tempfile::TempDir,
    pub fake: FakeMoonraker,
}

impl Roots {
    /// One Moonraker Printer on the fake (one Material Slot, the given
    /// Start-safety rule) and the rig's Slice Revision.
    pub fn new(start_safety: StartSafety) -> Self {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let fake = FakeMoonraker::start();
        fake.with_state(|state| state.api_key = Some(SECRET.to_string()));
        let credentials = tempfile::tempdir().unwrap();
        CredentialStore::file_backed(credentials.path().to_path_buf())
            .set(CREDENTIAL_REF, SECRET)
            .unwrap();
        let storage = Arc::new(Storage::open(paths.clone(), &lease).unwrap());
        let mut config = fake.config();
        config.credential_ref = Some(CREDENTIAL_REF.to_string());
        PrinterRepository::new(Arc::clone(&storage))
            .create_with_layout(
                StoredPrinter {
                    name: "Alpha".to_string(),
                    connection: Some(config),
                    start_safety,
                    ..common::a_stored_printer(PRINTER)
                },
                None,
                &slots::default_layout(),
                &[],
            )
            .unwrap();
        seed_slice_revision(&storage);
        Self {
            _temp: temp,
            paths,
            lease,
            credentials,
            fake,
        }
    }

    pub fn uploads(&self) -> usize {
        self.count_requests("POST", "/server/files/upload")
    }

    pub fn starts(&self) -> usize {
        self.count_requests("POST", "/printer/print/start")
    }

    pub fn count_requests(&self, method: &str, path: &str) -> usize {
        self.fake
            .requests()
            .iter()
            .filter(|request| request.method == method && request.path() == path)
            .count()
    }
}

// --- a running app ---------------------------------------------------------------

pub struct Running {
    pub app: tauri::App<MockRuntime>,
    pub webview: tauri::WebviewWindow<MockRuntime>,
    pub manager: Arc<ConnectionManager<MockRuntime>>,
    pub services: Arc<RuntimeServices<MockRuntime>>,
    pub storage: Arc<Storage>,
    pub events: Arc<Mutex<Vec<String>>>,
}

/// How [`boot`] leaves the runtime.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Driver {
    /// `start_jobs_runtime` runs, as in production.
    Started,
    /// The driver never starts: nothing applies a Host Operation's outcome
    /// to its Job (a crash point just before `apply_host_outcome`).
    Off,
}

/// Boots an app over `roots`, running startup recovery first as
/// `build_runtime_services` does (host ops, then Jobs). The Printer is
/// Online, Ready, and fresh before the driver starts.
pub fn boot(roots: &Roots, driver: Driver) -> Running {
    let storage = Arc::new(Storage::open(roots.paths.clone(), &roots.lease).unwrap());
    host_ops::recover_after_restart(&storage, SystemClock.now()).unwrap();
    let recovered = farm3d_lib::jobs::recover_after_restart(&storage, SystemClock.now()).unwrap();
    let factory: Arc<dyn CapabilityFactory> = Arc::new(SimFactory);
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let (app, webview, manager, services) = common::runtime_with(
        tauri::generate_handler![
            farm3d_lib::queue::commands::list_queue,
            farm3d_lib::queue::commands::add_to_queue,
            farm3d_lib::jobs::commands::assign_queue_entry,
            farm3d_lib::jobs::commands::stage_job,
            farm3d_lib::jobs::commands::start_job,
            farm3d_lib::jobs::commands::pause_job,
            farm3d_lib::jobs::commands::resume_job,
            farm3d_lib::jobs::commands::cancel_job,
            farm3d_lib::jobs::commands::release_job,
            farm3d_lib::jobs::commands::get_job_history,
            farm3d_lib::host_ops::commands::reconcile_host_operation,
            farm3d_lib::host_ops::commands::abandon_host_operation,
            farm3d_lib::spools::commands::move_spool,
        ],
        Arc::clone(&storage),
        Arc::new(common::a_catalog()),
        roots.credentials.path().to_path_buf(),
        unused_factory,
        move |services| {
            services.host_ops = Arc::new(HostOperationServices::new(
                Arc::clone(&services.storage),
                Arc::clone(&services.library.content),
                Arc::clone(&services.manager),
                factory,
                clock,
                host_ops_timings(),
            ));
        },
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    app.listen(STATUS_EVENT, move |event| {
        sink.lock().unwrap().push(event.payload().to_string());
    });
    manager.seed(PRINTER, status_of(OperationalState::Ready));
    farm3d_lib::start_host_ops_runtime(&services, app.handle());
    services.jobs.set_recovered(recovered);
    if driver == Driver::Started {
        farm3d_lib::start_jobs_runtime(&services, app.handle());
    }
    Running {
        app,
        webview,
        manager,
        services,
        storage,
        events,
    }
}

/// A live status: Online and fresh in `state`, or Offline for
/// `OperationalState::Offline`.
pub fn status_of(state: OperationalState) -> PrinterStatus {
    let connection = if state == OperationalState::Offline {
        ConnectionState::Offline
    } else {
        ConnectionState::Online
    };
    let mut status = PrinterStatus::new(connection);
    status.operational_state = state;
    status.freshness = TelemetryFreshness::Fresh;
    status
}

pub fn id(value: &Value) -> String {
    value["id"].as_str().expect("an id").to_string()
}

impl Running {
    pub fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    pub fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    pub fn job_command(&self, command: &str, operation_id: &str, job_id: &str) -> Result<Value, Value> {
        self.call(command, json!({"operationId": operation_id, "jobId": job_id}))
    }

    pub fn start(&self, operation_id: &str, job_id: &str, prior: &str) -> Result<Value, Value> {
        self.call(
            "start_job",
            json!({
                "operationId": operation_id,
                "jobId": job_id,
                "priorState": prior,
                "acknowledgement": "bedClear",
            }),
        )
    }

    /// Sets the Printer's live status (Online, fresh).
    pub fn status(&self, state: OperationalState) {
        self.manager.seed(PRINTER, status_of(state));
    }

    /// A Spool in storage, PLA 1.75, 1 kg on it.
    pub fn spool(&self) -> String {
        let fields = SpoolFields {
            manufacturer: "Polymaker".to_string(),
            product: None,
            material_family: MaterialFamily::Pla,
            material_other: None,
            color_name: "Black".to_string(),
            color_hex: None,
            diameter: FilamentDiameter::D175,
            nominal_mg: 1_000_000,
            low_threshold_mg: 10_000,
            tare_id: None,
            notes: None,
        };
        let entry = AmountEntry::Net {
            net_mg: 1_000_000,
            confidence: AmountConfidence::Estimated,
        };
        self.storage
            .write_repo(|tx| spools_repository::insert_spool(tx, &fields, &entry, None))
            .unwrap()
            .id
    }

    /// Loads `spool_id` into the Printer's one slot through `move_spool`.
    pub fn load(&self, spool_id: &str) {
        let (revision, slot_id): (i64, String) = self
            .storage
            .read(|connection| {
                Ok((
                    connection.query_row(
                        "SELECT revision FROM spools WHERE id = ?1",
                        [spool_id],
                        |row| row.get(0),
                    )?,
                    connection.query_row(
                        "SELECT id FROM material_slots WHERE printer_id = ?1",
                        [PRINTER],
                        |row| row.get(0),
                    )?,
                ))
            })
            .unwrap();
        self.ok(
            "move_spool",
            json!({
                "operationId": format!("load-{}", uuid::Uuid::new_v4()),
                "spoolId": spool_id,
                "expectedSpoolRevision": revision,
                "destination": {"kind": "slot", "slotId": slot_id, "expectedOccupantSpoolId": null},
            }),
        );
    }

    /// Adds one copy of the rig's revision (Recommended) and assigns it to
    /// the Printer with `spool_id`: the Job id.
    pub fn assign(&self, spool_id: &str) -> String {
        let operation = uuid::Uuid::new_v4();
        let added = self.ok(
            "add_to_queue",
            json!({
                "operationId": format!("add-{operation}"),
                "sliceRevisionId": SLR,
                "quantity": 1,
                "policy": "recommended",
                "preference": "loadedFirst",
            }),
        );
        let entry = id(&added["entries"][0]);
        let change = self.ok(
            "assign_queue_entry",
            json!({
                "operationId": format!("assign-{operation}"),
                "entryId": entry,
                "printerId": PRINTER,
                "spoolId": spool_id,
            }),
        );
        id(&change["jobs"][0])
    }

    /// The Job as `get_job_history` reports it (with live `startBlockers`).
    pub fn job(&self, job_id: &str) -> Value {
        self.ok("get_job_history", json!({"jobId": job_id}))["job"].clone()
    }

    pub fn history(&self, job_id: &str) -> Value {
        self.ok("get_job_history", json!({"jobId": job_id}))
    }

    /// Waits until the Job's stored state is `state`.
    pub fn wait_job(&self, job_id: &str, state: &str) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let job = self.job(job_id);
            if job["state"] == state {
                return job;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {state}: {job}"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits until `done` holds for the Job as `get_job_history` reports it.
    pub fn wait_job_until(&self, job_id: &str, done: impl Fn(&Value) -> bool) -> Value {
        let deadline = Instant::now() + WAIT;
        loop {
            let job = self.job(job_id);
            if done(&job) {
                return job;
            }
            assert!(Instant::now() < deadline, "timed out waiting on {job}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Waits for the Job's first linked Host Operation (the driver's
    /// stage runs in the background).
    pub fn first_op(&self, job_id: &str) -> HostOperation {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(op) = self.ops(job_id).into_iter().next() {
                return op;
            }
            assert!(Instant::now() < deadline, "no Host Operation for {job_id}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn row(&self, id: &str) -> HostOperation {
        self.storage
            .read(|connection| Ok(host_ops_repo::load(connection, id)))
            .unwrap()
            .unwrap()
            .expect("row exists")
    }

    pub fn wait_op(&self, id: &str, done: impl Fn(&HostOperation) -> bool) -> HostOperation {
        let deadline = Instant::now() + WAIT;
        loop {
            let row = self.row(id);
            if done(&row) {
                return row;
            }
            assert!(Instant::now() < deadline, "timed out waiting on {row:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    pub fn wait_resolved(&self, id: &str, state: HostOperationState) -> HostOperation {
        let row = self.wait_op(id, |row| {
            !matches!(
                row.state,
                HostOperationState::Dispatching | HostOperationState::Reconciling
            )
        });
        assert_eq!(row.state, state, "{row:?}");
        row
    }

    /// The Job's linked Host Operations, oldest first.
    pub fn ops(&self, job_id: &str) -> Vec<HostOperation> {
        let ids: Vec<String> = self
            .storage
            .read(|connection| {
                let mut statement = connection.prepare(
                    "SELECT id FROM host_operations WHERE job_id = ?1 ORDER BY created_at, rowid",
                )?;
                let ids = statement
                    .query_map([job_id], |row| row.get(0))?
                    .collect::<rusqlite::Result<Vec<String>>>()?;
                Ok(ids)
            })
            .unwrap();
        ids.iter().map(|id| self.row(id)).collect()
    }

    pub fn scalar(&self, sql: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    pub fn text(&self, sql: &str) -> Option<String> {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    pub fn event_kinds(&self, job_id: &str) -> Vec<String> {
        self.history(job_id)["events"]
            .as_array()
            .unwrap()
            .iter()
            .map(|event| event["kind"].as_str().unwrap().to_string())
            .collect()
    }

    /// The captured `queue.job.changed` payloads for `job_id`, in order.
    pub fn job_events(&self, job_id: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str::<Value>(text).unwrap())
            .filter(|event| event["type"] == "queue.job.changed" && event["subject"]["id"] == job_id)
            .map(|event| event["payload"]["job"].clone())
            .collect()
    }

    /// A Job staged and awaiting start, with its Spool loaded: the Job id.
    pub fn awaiting_start(&self) -> String {
        let spool = self.spool();
        self.load(&spool);
        let job = self.assign(&spool);
        self.wait_job(&job, "awaitingStart");
        job
    }

    /// A Job printing (started by the operator): the Job id.
    pub fn printing(&self) -> String {
        let job = self.awaiting_start();
        self.start(&format!("start-{job}"), &job, "ready")
            .expect("start_job");
        self.wait_job(&job, "printing");
        self.status(OperationalState::Printing);
        job
    }
}
