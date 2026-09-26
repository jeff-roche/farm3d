//! P6 Task 9 (spec D3, D5, D8, D9, "Commands", "Events"): the executor,
//! the fail-safe reconciler, the Start and control rules, abandon, and the
//! `hostOperations` stream, driven through the Tauri IPC path against
//! `FakeMoonraker`.
//!
//! A "restart" opens a new `Storage` over the same roots, runs startup
//! recovery exactly as `build_runtime_services` does, and boots a new app
//! (the old one's executor was stopped at its fault point, as a crash
//! would). Time never passes for real: the settle period is crossed by
//! moving an injected clock, and automatic retries are pushed an hour out.

mod common;

use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fake_moonraker::{FakeJob, FakeMoonraker, Fault, Route, StartTrace};
use farm3d_lib::connections::adapters::descriptor;
use farm3d_lib::connections::capabilities::{
    capabilities_for, ArtifactStaging, CapabilityEvidence, CapabilityKey, CapabilityMap,
    CapabilityState, EvidenceTier, HostFacts, HostOperationFailureCode, HostStateQuery,
    InconclusiveReason, PrintControl, PrinterCapabilities, UnsupportedReason,
};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::status_repository::{PrinterTelemetry, ToolTemperature};
use farm3d_lib::connections::supervisor::{PrinterSetupFacts, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionObservation, PrinterConnection, MOONRAKER_KIND, OCTOPRINT_KIND,
};
use farm3d_lib::host_ops::repository::{self as host_ops_repo, NewHostOperation, Outcome};
use farm3d_lib::host_ops::{
    self, CapabilityFactory, Clock, FaultAction, FaultPoint, HostOperation, HostOperationEndpoint,
    HostOperationKind, HostOperationResolution, HostOperationServices, HostOperationState,
    HostOpsTimings, MarkSentFault, StartEvidenceSource,
};
use farm3d_lib::library::content::ContentStore;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::operational::HostActivity;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::spools::operations::OperationKind;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;

const PRINTER: &str = "printer-a";
const OCTO_PRINTER: &str = "printer-octo";
const SLR: &str = "slr-a";
const HOST_PATH: &str = "farm3d/slr-a.gcode";
const CREDENTIAL_REF: &str = "farm3d/printer/printer-a/apikey";
const SECRET: &str = "SEEDED-API-KEY-7f3c91";
const NOW: &str = "2026-01-01T00:00:00Z";

// --- rig -----------------------------------------------------------------------

/// Real time plus an offset the test moves forward.
#[derive(Default)]
struct OffsetClock(Mutex<chrono::Duration>);

impl OffsetClock {
    fn advance(&self, by: Duration) {
        *self.0.lock().unwrap() += chrono::Duration::from_std(by).unwrap();
    }
}

impl Clock for OffsetClock {
    fn now(&self) -> chrono::DateTime<chrono::Utc> {
        chrono::Utc::now() + *self.0.lock().unwrap()
    }
}

/// Builds Moonraker capabilities with short timings, and treats every
/// Moonraker capability the registry calls "not verified" as supported
/// (Task 12 adds the real evidence rows) unless the test switched it off.
#[derive(Default)]
struct TestFactory {
    unsupported: Mutex<BTreeSet<CapabilityKey>>,
}

impl TestFactory {
    fn switch_off(&self, key: CapabilityKey) {
        self.unsupported.lock().unwrap().insert(key);
    }

    fn adapter(
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<MoonrakerCapabilities> {
        (config.kind == MOONRAKER_KIND)
            .then(|| MoonrakerCapabilities::new(config, key, short_moonraker_timings()))
    }
}

const NOT_VERIFIED: &str = "Not verified for this Connection type yet.";

impl CapabilityFactory for TestFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        let mut capabilities = capabilities_for(printer, host_facts);
        let is_moonraker = printer
            .connection
            .as_ref()
            .is_some_and(|connection| connection.kind == MOONRAKER_KIND);
        if !is_moonraker {
            return capabilities;
        }
        let unsupported = self.unsupported.lock().unwrap().clone();
        let current = capabilities.capabilities.clone();
        capabilities.capabilities = CapabilityMap::complete(|key| {
            if unsupported.contains(&key) {
                return CapabilityState::Unsupported {
                    reason: UnsupportedReason::NotVerified,
                    detail: NOT_VERIFIED.to_string(),
                };
            }
            match &current[key] {
                CapabilityState::Unsupported {
                    reason: UnsupportedReason::NotVerified,
                    detail,
                } if detail == NOT_VERIFIED => CapabilityState::Supported {
                    evidence: CapabilityEvidence {
                        source: "tests/p6_host_ops.rs".to_string(),
                        tier: EvidenceTier::Sim,
                        verified_host_versions: Vec::new(),
                    },
                },
                other => other.clone(),
            }
        });
        capabilities
    }

    fn staging(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn ArtifactStaging>> {
        Self::adapter(config, key).map(|adapter| Box::new(adapter) as Box<dyn ArtifactStaging>)
    }

    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        Self::adapter(config, key).map(|adapter| Box::new(adapter) as Box<dyn PrintControl>)
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        Self::adapter(config, key).map(|adapter| Box::new(adapter) as Box<dyn HostStateQuery>)
    }
}

fn short_moonraker_timings() -> MoonrakerTimings {
    MoonrakerTimings {
        connect: Duration::from_millis(500),
        query: Duration::from_millis(1500),
        control: Duration::from_millis(1500),
        transfer_base: Duration::from_millis(1500),
        transfer_per_started_mib: Duration::from_millis(10),
        ..MoonrakerTimings::default()
    }
}

/// Production values except: a one-second verification window, and
/// automatic retries an hour out so they never race a test's own
/// `reconcile_host_operation`.
fn test_timings() -> HostOpsTimings {
    HostOpsTimings {
        verify_window: Duration::from_millis(800),
        verify_poll_interval: Duration::from_millis(50),
        backoff: |_| Duration::from_secs(3600),
        ..HostOpsTimings::default()
    }
}

fn gcode() -> Vec<u8> {
    let mut bytes = b"; farm3d P6 host-ops artifact\n".to_vec();
    for line in 0..300 {
        bytes.extend_from_slice(format!("G1 X{line} Y{line}\n").as_bytes());
    }
    bytes
}

struct Rig {
    _roots: tempfile::TempDir,
    paths: StoragePaths,
    lease: MetadataRootLease,
    credentials: tempfile::TempDir,
    fake: FakeMoonraker,
    clock: Arc<OffsetClock>,
    factory: Arc<TestFactory>,
    timings: HostOpsTimings,
}

struct Running {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    manager: Arc<farm3d_lib::connections::supervisor::ConnectionManager<MockRuntime>>,
    services: Arc<farm3d_lib::RuntimeServices<MockRuntime>>,
    storage: Arc<Storage>,
    events: Arc<Mutex<Vec<String>>>,
}

fn unused_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

impl Rig {
    fn new() -> Self {
        Self::with_timings(test_timings())
    }

    fn with_timings(timings: HostOpsTimings) -> Self {
        let roots = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(roots.path().join("metadata"), roots.path().join("data")).unwrap();
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
        let printers = PrinterRepository::new(Arc::clone(&storage));
        printers
            .create(StoredPrinter {
                connection: Some(config),
                ..common::a_stored_printer(PRINTER)
            })
            .unwrap();
        printers
            .create(StoredPrinter {
                connection: Some(ConnectionConfig {
                    kind: OCTOPRINT_KIND.to_string(),
                    host: "192.0.2.10".to_string(),
                    port: 80,
                    use_tls: false,
                    credential_ref: None,
                }),
                ..common::a_stored_printer(OCTO_PRINTER)
            })
            .unwrap();
        seed_slice_revision(&storage, SLR, &gcode());

        Self {
            _roots: roots,
            paths,
            lease,
            credentials,
            fake,
            clock: Arc::new(OffsetClock::default()),
            factory: Arc::new(TestFactory::default()),
            timings,
        }
    }

    /// Opens the roots and boots an app, running startup recovery first as
    /// `build_runtime_services` does. Every boot after the first is a
    /// restart.
    fn boot(&self) -> Running {
        let storage = Arc::new(Storage::open(self.paths.clone(), &self.lease).unwrap());
        host_ops::recover_after_restart(&storage, self.clock.now()).unwrap();
        let factory: Arc<dyn CapabilityFactory> = self.factory.clone();
        let clock: Arc<dyn Clock> = self.clock.clone();
        let timings = self.timings;
        let (app, webview, manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::host_ops::commands::list_host_operations,
                farm3d_lib::host_ops::commands::stage_slice_revision,
                farm3d_lib::host_ops::commands::start_staged_artifact,
                farm3d_lib::host_ops::commands::pause_host_print,
                farm3d_lib::host_ops::commands::resume_host_print,
                farm3d_lib::host_ops::commands::cancel_host_print,
                farm3d_lib::host_ops::commands::reconcile_host_operation,
                farm3d_lib::host_ops::commands::abandon_host_operation,
                farm3d_lib::printers::commands::archive_printer,
            ],
            Arc::clone(&storage),
            Arc::new(common::a_catalog()),
            self.credentials.path().to_path_buf(),
            unused_factory,
            move |services| {
                services.host_ops = Arc::new(HostOperationServices::new(
                    Arc::clone(&services.storage),
                    Arc::clone(&services.library.content),
                    Arc::clone(&services.manager),
                    factory,
                    clock,
                    timings,
                ));
            },
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        app.listen(STATUS_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_string());
        });
        farm3d_lib::start_host_ops_runtime(&services, app.handle());
        Running {
            _app: app,
            webview,
            manager,
            services,
            storage,
            events,
        }
    }
}

/// Seeds an external Slice Revision whose G-code blob is in the content
/// store, the way a real import leaves it.
fn seed_slice_revision(storage: &Arc<Storage>, id: &str, bytes: &[u8]) {
    let content = ContentStore::open(storage.paths().content_root()).unwrap();
    let staged = content
        .stage_bytes(bytes, &format!("seed-{id}"), "part.gcode")
        .unwrap();
    let sha256 = staged.sha256.clone();
    let size = bytes.len();
    content
        .place_and_commit(storage, &[&staged], |tx| {
            tx.execute_batch(&format!(
                "INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
                   VALUES ('mdl-{id}', 1, 'Model', 'gcode', 'managed', '{NOW}', '{NOW}');
                 INSERT INTO model_source_revisions(
                   id, model_id, sequence, content_sha256, size_bytes, format, origin,
                   source_file_name, source_path, captured_at, inspector_version, inspection_json
                 ) VALUES ('msr-{id}', 'mdl-{id}', 1, '{sha256}', {size}, 'gcode', 'import',
                           'part.gcode', '/src/part.gcode', '{NOW}', 1, '{{}}');
                 INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
                   gcode_size, target_json, facts_json, requires_manual_printer_selection,
                   estimates_json, created_at)
                 VALUES ('{id}', 'external', 'mdl-{id}', 'msr-{id}', '{sha256}', {size}, '{{}}',
                         '{{}}', 1, '{{}}', '{NOW}');"
            ))
            .map_err(RepositoryError::from)
        })
        .unwrap();
}

// --- driving the app ------------------------------------------------------------

impl Running {
    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    fn stage(&self, operation_id: &str) -> Result<Value, Value> {
        self.call(
            "stage_slice_revision",
            json!({"operationId": operation_id, "printerId": PRINTER, "sliceRevisionId": SLR}),
        )
    }

    fn start(&self, operation_id: &str, upload: &str, prior: &str) -> Result<Value, Value> {
        self.call(
            "start_staged_artifact",
            json!({
                "operationId": operation_id, "printerId": PRINTER,
                "hostOperationId": upload, "priorState": prior,
            }),
        )
    }

    fn control(&self, verb: &str, operation_id: &str) -> Result<Value, Value> {
        self.call(
            &format!("{verb}_host_print"),
            json!({"operationId": operation_id, "printerId": PRINTER}),
        )
    }

    fn reconcile(&self, id: &str) -> Result<Value, Value> {
        self.call("reconcile_host_operation", json!({"hostOperationId": id}))
    }

    fn abandon(&self, operation_id: &str, id: &str) -> Result<Value, Value> {
        self.call(
            "abandon_host_operation",
            json!({
                "operationId": operation_id, "hostOperationId": id,
                "acknowledgement": "hostStateUnknown", "note": "  printer was scrapped  ",
            }),
        )
    }

    fn row(&self, id: &str) -> HostOperation {
        self.storage
            .read(|connection| Ok(host_ops_repo::load(connection, id)))
            .unwrap()
            .unwrap()
            .expect("row exists")
    }

    fn row_count(&self) -> i64 {
        self.storage
            .read(|connection| {
                connection.query_row("SELECT COUNT(*) FROM host_operations", [], |row| row.get(0))
            })
            .unwrap()
    }

    /// Waits (up to 10 s) until row `id` satisfies `done`.
    fn wait_for(&self, id: &str, done: impl Fn(&HostOperation) -> bool) -> HostOperation {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let row = self.row(id);
            if done(&row) {
                return row;
            }
            assert!(Instant::now() < deadline, "timed out waiting on {row:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn wait_settled(&self, id: &str) -> HostOperation {
        self.wait_for(id, |row| row.state != HostOperationState::Dispatching)
    }

    /// A live telemetry frame: Online and fresh, with `activity`. The first
    /// one brings the Printer Online, whose hook refreshes the host facts
    /// and then reconciles; this waits for the facts so that hook's attempt
    /// can't land in the middle of the test's own steps.
    fn observe(&self, activity: HostActivity) {
        self.manager.apply_observation(
            PRINTER,
            ConnectionObservation::Telemetry(telemetry(activity)),
            PrinterSetupFacts::complete(),
        );
        let printer = PrinterRepository::new(Arc::clone(&self.storage))
            .get(PRINTER)
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(5);
        while self
            .services
            .host_ops
            .capabilities(&printer)
            .host_facts
            .is_none()
        {
            assert!(
                Instant::now() < deadline,
                "the Online hook read no host facts"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    fn events(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|payload| serde_json::from_str(payload).unwrap())
            .filter(|event: &Value| {
                event["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("hostOperations."))
            })
            .collect()
    }
}

fn telemetry(activity: HostActivity) -> PrinterTelemetry {
    PrinterTelemetry {
        host_activity: activity,
        host_activity_name: None,
        job_name: None,
        progress: None,
        nozzle_temp_c: None,
        nozzle_target_c: None,
        bed_temp_c: None,
        bed_target_c: None,
        print_duration_s: None,
        tools: (0..4)
            .map(|index| ToolTemperature {
                index,
                temp_c: Some(25.0),
                target_c: Some(0.0),
            })
            .collect(),
    }
}

fn id_of(row: &Value) -> String {
    row["id"].as_str().expect("a row id").to_string()
}

fn code(error: &Value) -> &str {
    error["code"].as_str().unwrap_or_default()
}

fn requests_to(fake: &FakeMoonraker, method: &str, path: &str) -> usize {
    fake.requests()
        .iter()
        .filter(|request| request.method == method && request.path() == path)
        .count()
}

fn uploads(fake: &FakeMoonraker) -> usize {
    requests_to(fake, "POST", "/server/files/upload")
}

fn starts(fake: &FakeMoonraker) -> usize {
    requests_to(fake, "POST", "/printer/print/start")
}

fn failure_code(row: &HostOperation) -> Option<HostOperationFailureCode> {
    row.failure.as_ref().map(|failure| failure.code)
}

fn reason(row: &HostOperation) -> Option<InconclusiveReason> {
    row.last_attempt.as_ref().map(|attempt| attempt.reason)
}

fn wait_fired(fired: Receiver<()>) {
    fired
        .recv_timeout(Duration::from_secs(10))
        .expect("the fault point was reached");
    // The executor stops right after it signals; give it a beat to unwind.
    std::thread::sleep(Duration::from_millis(100));
}

/// Stages SLR on a Ready Printer and waits for it to succeed.
fn staged_upload(running: &Running, operation_id: &str) -> String {
    running.observe(HostActivity::Idle);
    let id = id_of(&running.stage(operation_id).expect("stage"));
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
    id
}

/// Starts the staged upload with `trace` scripted on the host, and waits
/// for the executor's commit.
fn start_with(
    running: &Running,
    fake: &FakeMoonraker,
    upload: &str,
    fault: Option<Fault>,
) -> HostOperation {
    if let Some(fault) = fault {
        fake.fault(Route::Start, fault);
    }
    let id = id_of(&running.start("op-start", upload, "ready").expect("start"));
    running.wait_settled(&id)
}

// --- restart matrix: uploads ------------------------------------------------------

#[test]
fn crash_after_write_ahead_before_mark_sent_is_never_sent_after_restart() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    let fired = running
        .services
        .host_ops
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Crash);
    let id = id_of(&running.stage("op-stage").unwrap());
    wait_fired(fired);
    let row = running.row(&id);
    assert_eq!(row.state, HostOperationState::Dispatching);
    assert!(row.dispatched_at.is_none());

    let restarted = rig.boot();
    let row = restarted.row(&id);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert_eq!(uploads(&rig.fake), 0, "nothing was sent");
}

#[test]
fn crash_after_write_ahead_before_mark_sent_never_sends_a_start() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let fired = running
        .services
        .host_ops
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Crash);
    let id = id_of(&running.start("op-start", &upload, "ready").unwrap());
    wait_fired(fired);

    let restarted = rig.boot();
    let row = restarted.row(&id);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert_eq!(starts(&rig.fake), 0, "no start was sent");
}

#[test]
fn crash_after_mark_sent_before_send_settles_to_not_applied_after_restart() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    let fired = running
        .services
        .host_ops
        .inject_fault(FaultPoint::AfterMarkSentBeforeSend, FaultAction::Crash);
    let id = id_of(&running.stage("op-stage").unwrap());
    wait_fired(fired);
    assert!(running.row(&id).dispatched_at.is_some());

    let restarted = rig.boot();
    // Startup recovery made it uncertain; the startup pass then made one
    // attempt, which (inside the settle period) proves nothing.
    let row = restarted.wait_for(&id, |row| row.attempts >= 1);
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::UploadSettling));

    let row: HostOperation = serde_json::from_value(restarted.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::UploadSettling));

    rig.clock.advance(Duration::from_secs(61));
    let row: HostOperation = serde_json::from_value(restarted.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NotApplied)
    );
    assert_eq!(uploads(&rig.fake), 0, "no upload was ever sent");
}

#[test]
fn a_crash_after_send_or_before_commit_is_uncertain_and_reconciles_to_staged() {
    for point in [FaultPoint::AfterSend, FaultPoint::AfterResponseBeforeCommit] {
        let rig = Rig::new();
        let running = rig.boot();
        running.observe(HostActivity::Idle);
        let fired = running
            .services
            .host_ops
            .inject_fault(point, FaultAction::Crash);
        let id = id_of(&running.stage("op-stage").unwrap());
        wait_fired(fired);
        assert_eq!(running.row(&id).state, HostOperationState::Dispatching);

        let restarted = rig.boot();
        let row = restarted.wait_for(&id, |row| row.state == HostOperationState::Succeeded);
        assert_eq!(
            row.resolution,
            Some(HostOperationResolution::ArtifactVerified { reconciled: true }),
            "{point:?}"
        );
        assert_eq!(uploads(&rig.fake), 1, "{point:?}: exactly one upload");
    }
}

#[test]
fn a_lost_upload_response_after_the_store_reconciles_to_staged_with_one_upload() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(Route::Upload, Fault::StoreThenDropResponse);
    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::ResponseLost));
    assert!(row.uncertain_since.is_some());

    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Succeeded);
    assert_eq!(
        row.resolution,
        Some(HostOperationResolution::ArtifactVerified { reconciled: true })
    );
    assert_eq!(uploads(&rig.fake), 1, "exactly one upload");
}

#[test]
fn a_body_cut_midway_settles_then_fails_not_applied() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(Route::Upload, Fault::DropMidBody);
    let id = id_of(&running.stage("op-stage").unwrap());
    assert_eq!(
        running.wait_settled(&id).state,
        HostOperationState::Uncertain
    );

    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::UploadSettling));
    assert_eq!(row.attempts, 1);

    rig.clock.advance(Duration::from_secs(61));
    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NotApplied)
    );
    assert_eq!(uploads(&rig.fake), 1, "never re-uploaded");
}

#[test]
fn a_file_delivered_late_inside_the_settle_period_reconciles_to_staged() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(
        Route::Upload,
        Fault::DeliverLate(Duration::from_millis(400)),
    );
    let id = id_of(&running.stage("op-stage").unwrap());
    assert_eq!(
        running.wait_settled(&id).state,
        HostOperationState::Uncertain
    );

    std::thread::sleep(Duration::from_millis(500));
    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Succeeded);
    assert_eq!(uploads(&rig.fake), 1);
}

#[test]
fn a_different_file_at_the_path_fails_host_file_differs_after_settling_and_is_kept() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    let partial = b"; somebody else's partial file\n".to_vec();
    rig.fake
        .fault(Route::Upload, Fault::StoreDifferentBytes(partial.clone()));
    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::UploadSettling));

    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);

    rig.clock.advance(Duration::from_secs(61));
    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::HostFileDiffers)
    );
    assert_eq!(rig.fake.file(HOST_PATH), Some(partial), "never deleted");
    assert_eq!(uploads(&rig.fake), 1);
}

#[test]
fn a_201_whose_locate_fails_is_identity_check_failed_then_staged() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(
        Route::Download,
        Fault::respond(500, "Internal Server Error"),
    );
    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::IdentityCheckFailed));

    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Succeeded);
    assert_eq!(uploads(&rig.fake), 1);
}

#[test]
fn a_clean_stage_is_verified_and_sends_no_print_field() {
    let rig = Rig::new();
    let running = rig.boot();
    let id = staged_upload(&running, "op-stage");
    let row = running.row(&id);
    assert_eq!(
        row.resolution,
        Some(HostOperationResolution::ArtifactVerified { reconciled: false })
    );
    assert_eq!(row.host_path, HOST_PATH);
    assert_eq!(rig.fake.file(HOST_PATH), Some(gcode()));
    assert_eq!(rig.fake.take_print_field_uploads(), 0);
}

// --- restart matrix: starts ------------------------------------------------------------

#[test]
fn a_start_applied_with_its_response_lost_reconciles_from_print_stats() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::PrintingOnly)),
    );
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::ResponseLost));
    assert!(row.history_mark.is_some());

    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Succeeded);
    assert_eq!(
        row.resolution,
        Some(HostOperationResolution::StartObserved {
            source: StartEvidenceSource::PrintStats,
            history_job_id: None,
            interrupted: false,
        })
    );
    assert_eq!(starts(&rig.fake), 1, "exactly one start");
}

#[test]
fn a_start_applied_with_only_a_history_job_left_reconciles_from_history() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(
            StartTrace::PrintingWithHistoryJob,
        )),
    );
    assert_eq!(row.state, HostOperationState::Uncertain);
    // Klipper restarts: the live state is cleared and the job closes as
    // `klippy_disconnect`.
    rig.fake.restart();

    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Succeeded);
    match row.resolution {
        Some(HostOperationResolution::StartObserved {
            source: StartEvidenceSource::History,
            history_job_id: Some(job_id),
            interrupted: true,
        }) => assert_eq!(job_id, rig.fake.history().last().unwrap().job_id),
        other => panic!("expected startObserved from history, got {other:?}"),
    }
    assert_eq!(starts(&rig.fake), 1);
}

#[test]
fn only_complete_with_no_job_is_never_proof_of_a_start() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::CompleteOnly)),
    );
    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::NoStartEvidence));
    assert_eq!(starts(&rig.fake), 1, "never re-started");
}

#[test]
fn an_old_job_for_the_same_file_never_proves_a_start() {
    // A job at or below the high-water mark.
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    rig.fake
        .with_state(|state| state.push_job(HOST_PATH, "completed"));
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::CompleteOnly)),
    );
    assert_eq!(row.history_mark, Some(1));
    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::NoStartEvidence));

    // A job above the mark that started long before dispatch.
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::CompleteOnly)),
    );
    rig.fake.with_state(|state| {
        state.history.push(FakeJob {
            job_id: "0000FF".to_string(),
            filename: HOST_PATH.to_string(),
            status: "completed".to_string(),
            start_time: chrono::Utc::now().timestamp() as f64 - 120.0,
        })
    });
    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::NoStartEvidence));
}

#[test]
fn a_definitively_rejected_start_fails_with_its_code_and_is_not_reconciled() {
    for (fault, expected) in [
        (
            Fault::respond(400, "Bad things"),
            HostOperationFailureCode::HostRejected,
        ),
        (
            Fault::klippy_host_not_connected(),
            HostOperationFailureCode::HostNotReady,
        ),
        (
            Fault::unauthorized(),
            HostOperationFailureCode::AuthRejected,
        ),
    ] {
        let rig = Rig::new();
        let running = rig.boot();
        let upload = staged_upload(&running, "op-stage");
        let row = start_with(&running, &rig.fake, &upload, Some(fault));
        assert_eq!(row.state, HostOperationState::Failed);
        assert_eq!(failure_code(&row), Some(expected));
        assert_eq!(row.attempts, 0);
        assert_eq!(
            running.reconcile(&row.id).unwrap(),
            serde_json::to_value(&row).unwrap(),
            "a failed row is returned unchanged"
        );
    }
}

#[test]
fn klippy_disconnected_is_uncertain_and_no_longer_pending_never_failed() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::klippy_disconnected()),
    );
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert!(row.no_longer_pending);
    assert_eq!(reason(&row), Some(InconclusiveReason::KlipperRestarted));

    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert!(row.no_longer_pending);
    assert_eq!(starts(&rig.fake), 1);
}

#[test]
fn a_host_printing_a_different_file_is_different_file_on_host() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::PrintingOnly)),
    );
    rig.fake
        .with_state(|state| state.print_filename = "other/benchy.gcode".to_string());
    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::DifferentFileOnHost));
    assert_eq!(starts(&rig.fake), 1, "no start sent by reconciliation");
}

#[test]
fn a_reconcile_read_that_finds_klipper_not_ready_sets_no_longer_pending() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let row = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::CompleteOnly)),
    );
    assert!(!row.no_longer_pending);
    rig.fake
        .with_state(|state| state.klippy_state = "shutdown".to_string());
    let row: HostOperation = serde_json::from_value(running.reconcile(&row.id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert!(row.no_longer_pending);
    assert_eq!(reason(&row), Some(InconclusiveReason::HostNotReady));
}

// --- control ---------------------------------------------------------------------------

/// A started print on the fake, with the Printer seen Printing.
fn printing(rig: &Rig, running: &Running) {
    let upload = staged_upload(running, "op-stage");
    let row = start_with(running, &rig.fake, &upload, None);
    assert_eq!(row.state, HostOperationState::Succeeded);
    assert_eq!(row.resolution, Some(HostOperationResolution::StartAccepted));
    running.observe(HostActivity::Printing);
}

#[test]
fn pause_resume_and_cancel_succeed_once_their_effect_is_observed() {
    let rig = Rig::new();
    let running = rig.boot();
    printing(&rig, &running);

    let pause = running.wait_settled(&id_of(&running.control("pause", "op-pause").unwrap()));
    assert_eq!(pause.state, HostOperationState::Succeeded, "{pause:?}");
    assert_eq!(pause.host_path, HOST_PATH);
    running.observe(HostActivity::Paused);

    let resume = running.wait_settled(&id_of(&running.control("resume", "op-resume").unwrap()));
    assert_eq!(resume.state, HostOperationState::Succeeded, "{resume:?}");
    running.observe(HostActivity::Printing);

    let cancel = running.wait_settled(&id_of(&running.control("cancel", "op-cancel").unwrap()));
    assert_eq!(cancel.state, HostOperationState::Succeeded, "{cancel:?}");
    assert_eq!(rig.fake.print_state().0, "cancelled");
}

#[test]
fn a_control_ok_with_no_state_change_is_effect_not_observed() {
    let rig = Rig::new();
    let running = rig.boot();
    printing(&rig, &running);
    rig.fake.fault(Route::Pause, Fault::OkWithoutEffect);
    let row = running.wait_settled(&id_of(&running.control("pause", "op-pause").unwrap()));
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::EffectNotObserved));
}

// --- unreachable, abandon, capability gate ------------------------------------------

/// An `uncertain` row of `kind` whose recorded endpoint has nothing
/// listening.
fn seed_unreachable_uncertain(storage: &Storage, kind: HostOperationKind) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let upload_like = matches!(kind, HostOperationKind::Upload | HostOperationKind::Start);
    let operation_kind = match kind {
        HostOperationKind::Upload => OperationKind::StageSliceRevision,
        HostOperationKind::Start => OperationKind::StartStagedArtifact,
        HostOperationKind::Pause => OperationKind::PauseHostPrint,
        HostOperationKind::Resume => OperationKind::ResumeHostPrint,
        HostOperationKind::Cancel => OperationKind::CancelHostPrint,
    };
    let new = NewHostOperation {
        operation_id: format!("seed-{kind:?}"),
        operation_kind,
        request_digest: "seed".to_string(),
        printer_id: PRINTER.to_string(),
        kind,
        slice_revision_id: None,
        source_host_operation_id: None,
        gcode_sha256: upload_like.then(|| "a".repeat(64)),
        gcode_size: upload_like.then_some(100),
        host_path: HOST_PATH.to_string(),
        history_mark: (kind == HostOperationKind::Start).then_some(0),
        endpoint: HostOperationEndpoint {
            kind: MOONRAKER_KIND.to_string(),
            host: "127.0.0.1".to_string(),
            port,
        },
    };
    storage
        .write_repo(|tx| {
            let row = host_ops_repo::insert_dispatching(tx, &new)?;
            host_ops_repo::mark_sent(tx, &row.id)?;
            host_ops_repo::transition(
                tx,
                &row.id,
                Outcome::Uncertain {
                    reason: InconclusiveReason::ResponseLost,
                    no_longer_pending: false,
                },
            )
        })
        .unwrap()
        .id
}

#[test]
fn a_host_unreachable_throughout_stays_uncertain_and_abandon_lifts_the_guards() {
    for kind in [
        HostOperationKind::Upload,
        HostOperationKind::Start,
        HostOperationKind::Pause,
        HostOperationKind::Resume,
        HostOperationKind::Cancel,
    ] {
        let rig = Rig::new();
        let seed = Arc::new(Storage::open(rig.paths.clone(), &rig.lease).unwrap());
        let id = seed_unreachable_uncertain(&seed, kind);
        let running = rig.boot();
        // The startup pass makes one attempt; wait for it.
        let row = running.wait_for(&id, |row| row.attempts >= 1);
        assert_eq!(row.state, HostOperationState::Uncertain, "{kind:?}");
        assert_eq!(
            reason(&row),
            Some(InconclusiveReason::HostUnreachable),
            "{kind:?}"
        );

        let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
        assert_eq!(row.state, HostOperationState::Uncertain);
        assert_eq!(row.attempts, 2);

        running.observe(HostActivity::Idle);
        let blocked = running.stage("op-blocked").unwrap_err();
        assert_eq!(code(&blocked), "HOST_OPERATION_PENDING", "{blocked}");

        let abandoned: HostOperation =
            serde_json::from_value(running.abandon("op-abandon", &id).unwrap()).unwrap();
        assert_eq!(abandoned.state, HostOperationState::Abandoned);
        assert_eq!(
            abandoned.abandon_note.as_deref(),
            Some("printer was scrapped")
        );
        assert!(abandoned.abandoned_at.is_some() && abandoned.resolved_at.is_some());

        let staged = running
            .stage("op-after")
            .expect("guards lift after abandon");
        running.wait_settled(&id_of(&staged));
    }
}

#[test]
fn abandon_is_refused_before_any_attempt_and_outside_uncertain() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(Route::Upload, Fault::DropMidBody);
    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(
        (row.state, row.attempts),
        (HostOperationState::Uncertain, 0)
    );

    let error = running.abandon("op-abandon", &id).unwrap_err();
    assert_eq!(code(&error), "HOST_OPERATION_NOT_ABANDONABLE", "{error}");
    assert_eq!(error["details"]["attempts"], 0);
    assert_eq!(error["details"]["state"], "uncertain");
    assert_eq!(error["details"]["hostOperationId"], id.as_str());

    // One inconclusive attempt makes it abandonable; the refusal above did
    // not burn the operation id.
    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.attempts, 1);
    let row: HostOperation =
        serde_json::from_value(running.abandon("op-abandon", &id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Abandoned);

    let staged = staged_upload(&running, "op-stage-2");
    let error = running.abandon("op-abandon-2", &staged).unwrap_err();
    assert_eq!(code(&error), "HOST_OPERATION_NOT_ABANDONABLE");
    assert_eq!(error["details"]["state"], "succeeded");
}

#[test]
fn abandon_validates_its_acknowledgement_and_note() {
    let rig = Rig::new();
    let seed = Arc::new(Storage::open(rig.paths.clone(), &rig.lease).unwrap());
    let id = seed_unreachable_uncertain(&seed, HostOperationKind::Upload);
    let running = rig.boot();
    running.wait_for(&id, |row| row.attempts >= 1);

    let error = running
        .call(
            "abandon_host_operation",
            json!({"operationId": "op-a", "hostOperationId": id, "acknowledgement": "yes"}),
        )
        .unwrap_err();
    assert_eq!(code(&error), "VALIDATION");
    assert_eq!(error["details"]["fieldPath"], "acknowledgement");

    let error = running
        .call(
            "abandon_host_operation",
            json!({
                "operationId": "op-a", "hostOperationId": id,
                "acknowledgement": "hostStateUnknown", "note": "x".repeat(501),
            }),
        )
        .unwrap_err();
    assert_eq!(code(&error), "VALIDATION");
    assert_eq!(error["details"]["fieldPath"], "note");
    assert_eq!(running.row(&id).state, HostOperationState::Uncertain);
}

#[test]
fn a_reconcile_capability_now_unsupported_makes_no_attempt_and_allows_abandon() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(Route::Upload, Fault::StoreThenDropResponse);
    let id = id_of(&running.stage("op-stage").unwrap());
    assert_eq!(
        running.wait_settled(&id).state,
        HostOperationState::Uncertain
    );
    let downloads_before = rig
        .fake
        .requests()
        .iter()
        .filter(|request| request.path().starts_with("/server/files/gcodes/"))
        .count();

    rig.factory.switch_off(CapabilityKey::ArtifactIdentity);
    let error = running.reconcile(&id).unwrap_err();
    assert_eq!(code(&error), "CAPABILITY_UNSUPPORTED", "{error}");
    assert_eq!(error["details"]["capability"], "artifactIdentity");
    assert_eq!(error["details"]["printerId"], PRINTER);
    assert_eq!(error["details"]["reason"], "notVerified");

    // A restart's startup pass skips it too.
    let restarted = rig.boot();
    std::thread::sleep(Duration::from_millis(300));
    let row = restarted.row(&id);
    assert_eq!(row.attempts, 0);
    assert_eq!(row.state, HostOperationState::Uncertain);
    let downloads_after = rig
        .fake
        .requests()
        .iter()
        .filter(|request| request.path().starts_with("/server/files/gcodes/"))
        .count();
    assert_eq!(
        downloads_after, downloads_before,
        "no attempt read the host"
    );

    let row: HostOperation =
        serde_json::from_value(restarted.abandon("op-abandon", &id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Abandoned);
    assert_eq!(row.attempts, 0);
}

// --- mark_sent and panics -----------------------------------------------------------

#[test]
fn a_failing_mark_sent_sends_nothing_and_fails_never_sent() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    running
        .services
        .host_ops
        .inject_mark_sent_fault(MarkSentFault::Fails);
    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert!(row.dispatched_at.is_none());
    assert_eq!(uploads(&rig.fake), 0);

    // A start, too.
    let upload = staged_upload(&running, "op-stage-2");
    running
        .services
        .host_ops
        .inject_mark_sent_fault(MarkSentFault::Fails);
    let row = start_with(&running, &rig.fake, &upload, None);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert_eq!(starts(&rig.fake), 0);
}

#[test]
fn a_failing_mark_sent_whose_failure_commit_also_fails_is_never_sent_after_restart() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    let fired = running
        .services
        .host_ops
        .inject_mark_sent_fault(MarkSentFault::FailsAndFailureCommitFails);
    let id = id_of(&running.stage("op-stage").unwrap());
    wait_fired(fired);
    let row = running.row(&id);
    assert_eq!(row.state, HostOperationState::Dispatching);
    assert!(row.dispatched_at.is_none());

    let restarted = rig.boot();
    let row = restarted.row(&id);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert_eq!(uploads(&rig.fake), 0);
}

#[test]
fn a_panic_before_mark_sent_fails_never_sent_and_after_it_is_uncertain() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    running
        .services
        .host_ops
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Panic);
    let id = id_of(&running.stage("op-stage-1").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );

    running
        .services
        .host_ops
        .inject_fault(FaultPoint::AfterSend, FaultAction::Panic);
    let id = id_of(&running.stage("op-stage-2").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::UnexpectedResponse));
    assert!(row.uncertain_since.is_some());
}

#[test]
fn a_missing_local_blob_fails_never_sent_before_mark_sent() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    let sha256: String = running
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT gcode_sha256 FROM slice_revisions WHERE id = ?1",
                [SLR],
                |row| row.get(0),
            )
        })
        .unwrap();
    let blob = running
        .storage
        .paths()
        .content_root()
        .join("blobs/sha256")
        .join(&sha256[..2])
        .join(&sha256);
    std::fs::remove_file(&blob).unwrap();

    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert!(row.dispatched_at.is_none());
    assert_eq!(uploads(&rig.fake), 0);
}

#[test]
fn a_local_blob_whose_size_differs_fails_never_sent_before_mark_sent() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    let sha256: String = running
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT gcode_sha256 FROM slice_revisions WHERE id = ?1",
                [SLR],
                |row| row.get(0),
            )
        })
        .unwrap();
    let blob = running
        .storage
        .paths()
        .content_root()
        .join("blobs/sha256")
        .join(&sha256[..2])
        .join(&sha256);
    let mut bytes = std::fs::read(&blob).unwrap();
    bytes.extend_from_slice(b"; trailing corruption\n");
    // Blobs are stored read-only; corruption doesn't care.
    let mut permissions = std::fs::metadata(&blob).unwrap().permissions();
    #[allow(clippy::permissions_set_readonly_false)]
    permissions.set_readonly(false);
    std::fs::set_permissions(&blob, permissions).unwrap();
    std::fs::write(&blob, &bytes).unwrap();

    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert!(row.dispatched_at.is_none());
    assert_eq!(uploads(&rig.fake), 0);
}

/// Makes every write of `dispatched_at` fail in SQLite itself, so the real
/// `mark_sent` error path runs (no injected fault).
fn refuse_mark_sent(rig: &Rig) {
    rusqlite::Connection::open(rig.paths.database())
        .unwrap()
        .execute_batch(
            "CREATE TRIGGER refuse_mark_sent BEFORE UPDATE OF dispatched_at ON host_operations \
             WHEN NEW.dispatched_at IS NOT NULL \
             BEGIN SELECT RAISE(ABORT, 'mark_sent refused by the test'); END;",
        )
        .unwrap();
}

#[test]
fn a_real_mark_sent_error_sends_nothing_and_fails_never_sent() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    refuse_mark_sent(&rig);
    let id = id_of(&running.stage("op-stage").unwrap());
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NeverSent)
    );
    assert!(row.dispatched_at.is_none());
    assert_eq!(uploads(&rig.fake), 0);
}

// --- the scheduler ------------------------------------------------------------------

#[test]
fn an_upload_settling_while_online_is_retried_at_the_end_of_the_settle_period() {
    // One quick first retry (it finds the file absent: `uploadSettling`),
    // then none: only the settle-period timer can conclude.
    let rig = Rig::with_timings(HostOpsTimings {
        settle_period: Duration::from_millis(600),
        backoff: |step| {
            if step == 0 {
                Duration::from_millis(100)
            } else {
                Duration::from_secs(3600)
            }
        },
        ..test_timings()
    });
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(Route::Upload, Fault::DropMidBody);
    let id = id_of(&running.stage("op-stage").unwrap());
    // Nothing reaches the host, so after the (short) settle period the
    // scheduled attempt proves it was not applied, with no manual check.
    let row = running.wait_for(&id, |row| row.state == HostOperationState::Failed);
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NotApplied)
    );
}

#[test]
fn going_online_triggers_an_attempt() {
    let rig = Rig::new();
    let seed = Arc::new(Storage::open(rig.paths.clone(), &rig.lease).unwrap());
    // An uncertain upload whose bytes are on the host already.
    rig.fake.put_file(HOST_PATH, &gcode());
    let id = seed_uncertain_upload_on_fake(&seed, &rig);
    rig.factory.switch_off(CapabilityKey::ArtifactIdentity); // skip the startup pass
    let running = rig.boot();
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(running.row(&id).state, HostOperationState::Uncertain);
    rig.factory.unsupported.lock().unwrap().clear();

    running.observe(HostActivity::Idle);
    let row = running.wait_for(&id, |row| row.state == HostOperationState::Succeeded);
    assert_eq!(
        row.resolution,
        Some(HostOperationResolution::ArtifactVerified { reconciled: true })
    );
}

#[test]
fn repeated_online_frames_do_not_fire_the_online_hook_again() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    let before = rig.fake.requests().len();
    running.observe(HostActivity::Idle);
    running.observe(HostActivity::Idle);
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        rig.fake.requests().len(),
        before,
        "a frame that keeps the Printer Online read the host again"
    );
}

#[test]
fn nothing_is_scheduled_while_the_printer_is_not_online() {
    let rig = Rig::with_timings(HostOpsTimings {
        backoff: |_| Duration::from_millis(50),
        ..test_timings()
    });
    let seed = Arc::new(Storage::open(rig.paths.clone(), &rig.lease).unwrap());
    // Absent on the host and inside the settle period: `uploadSettling`,
    // which would schedule both a backoff and a settle-deadline timer.
    let id = seed_uncertain_upload_on_fake(&seed, &rig);
    let running = rig.boot(); // never observed: not Online
    running.wait_for(&id, |row| row.attempts >= 1); // the startup pass
    let row: HostOperation = serde_json::from_value(running.reconcile(&id).unwrap()).unwrap();
    assert_eq!(row.state, HostOperationState::Uncertain);
    assert_eq!(reason(&row), Some(InconclusiveReason::UploadSettling));
    let attempts = row.attempts;
    std::thread::sleep(Duration::from_millis(400));
    assert_eq!(running.services.host_ops.retry_timers_scheduled(), 0);
    assert_eq!(running.services.host_ops.retry_attempts_run(), 0);
    assert_eq!(running.row(&id).attempts, attempts);
}

#[test]
fn abandon_cancels_the_pending_retry_timers() {
    let rig = Rig::with_timings(HostOpsTimings {
        backoff: |step| {
            if step == 0 {
                Duration::from_millis(1000)
            } else {
                Duration::from_secs(3600)
            }
        },
        ..test_timings()
    });
    let seed = Arc::new(Storage::open(rig.paths.clone(), &rig.lease).unwrap());
    let id = seed_uncertain_upload_on_fake(&seed, &rig);
    let running = rig.boot();
    running.observe(HostActivity::Idle); // Online: the hook's attempt schedules
    running.reconcile(&id).unwrap(); // resets the backoff: step 0 again
    assert!(running.services.host_ops.retry_timers_scheduled() > 0);
    running.abandon("op-abandon", &id).unwrap();
    assert_eq!(running.row(&id).state, HostOperationState::Abandoned);
    let attempts = running.row(&id).attempts;
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(running.services.host_ops.retry_attempts_run(), 0);
    assert_eq!(running.row(&id).attempts, attempts);
}

fn seed_uncertain_upload_on_fake(storage: &Storage, rig: &Rig) -> String {
    let sha256: String = storage
        .read(|connection| {
            connection.query_row(
                "SELECT gcode_sha256 FROM slice_revisions WHERE id = ?1",
                [SLR],
                |row| row.get(0),
            )
        })
        .unwrap();
    let config = rig.fake.config();
    let new = NewHostOperation {
        operation_id: "seed-upload".to_string(),
        operation_kind: OperationKind::StageSliceRevision,
        request_digest: "seed".to_string(),
        printer_id: PRINTER.to_string(),
        kind: HostOperationKind::Upload,
        slice_revision_id: Some(SLR.to_string()),
        source_host_operation_id: None,
        gcode_sha256: Some(sha256),
        gcode_size: Some(gcode().len() as i64),
        host_path: HOST_PATH.to_string(),
        history_mark: None,
        endpoint: HostOperationEndpoint {
            kind: MOONRAKER_KIND.to_string(),
            host: config.host,
            port: config.port,
        },
    };
    storage
        .write_repo(|tx| {
            let row = host_ops_repo::insert_dispatching(tx, &new)?;
            host_ops_repo::mark_sent(tx, &row.id)?;
            host_ops_repo::transition(
                tx,
                &row.id,
                Outcome::Uncertain {
                    reason: InconclusiveReason::ResponseLost,
                    no_longer_pending: false,
                },
            )
        })
        .unwrap()
        .id
}

// --- command pre-checks -------------------------------------------------------------

#[test]
fn start_is_refused_with_no_row_when_the_host_reread_shows_paused() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    rig.fake.with_state(|state| {
        state.print_state = "paused".to_string();
        state.print_filename = "other.gcode".to_string();
    });
    let before = running.row_count();
    let error = running.start("op-start", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "START_NOT_ALLOWED", "{error}");
    assert_eq!(error["details"]["observedState"], "paused");
    assert_eq!(error["details"]["freshness"], "fresh");
    assert_eq!(running.row_count(), before);
    assert_eq!(starts(&rig.fake), 0);
}

#[test]
fn start_is_refused_with_no_row_when_the_host_reread_shows_the_last_print_failed() {
    // Ruling R21: `print_stats` "error" is `Failed`, which never allows a
    // Start, even while the live status still says Idle.
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    rig.fake
        .with_state(|state| state.print_state = "error".to_string());
    let before = running.row_count();
    let error = running.start("op-start", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "START_NOT_ALLOWED", "{error}");
    assert_eq!(error["details"]["observedState"], "failed");
    assert_eq!(running.row_count(), before);
    assert_eq!(starts(&rig.fake), 0);
}

#[test]
fn start_is_capability_unsupported_with_no_row_when_start_is_switched_off() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    rig.factory.switch_off(CapabilityKey::Start);
    let before = running.row_count();
    let error = running.start("op-start", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "CAPABILITY_UNSUPPORTED", "{error}");
    assert_eq!(error["details"]["capability"], "start");
    assert_eq!(error["details"]["printerId"], PRINTER);
    assert_eq!(running.row_count(), before);
    assert_eq!(starts(&rig.fake), 0);
}

#[test]
fn start_is_refused_with_no_row_when_the_staged_file_is_absent_or_changed() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let before = running.row_count();

    rig.fake.put_file(HOST_PATH, b"changed");
    let error = running.start("op-start-1", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "STAGED_ARTIFACT_INVALID", "{error}");
    assert_eq!(error["details"]["reason"], "differs");
    assert_eq!(error["details"]["hostOperationId"], upload.as_str());

    rig.fake.with_state(|state| {
        state.files.remove(HOST_PATH);
    });
    let error = running.start("op-start-2", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "STAGED_ARTIFACT_INVALID");
    assert_eq!(error["details"]["reason"], "absent");
    assert_eq!(running.row_count(), before);
    assert_eq!(starts(&rig.fake), 0);
}

#[test]
fn start_is_refused_with_a_network_error_when_the_history_is_unreadable() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let before = running.row_count();
    rig.fake
        .fault(Route::History, Fault::DelayResponse(Duration::from_secs(3)));
    let error = running.start("op-start", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "TIMEOUT", "{error}");
    assert_eq!(running.row_count(), before);
    assert_eq!(starts(&rig.fake), 0);
}

#[test]
fn start_follows_the_start_table_and_is_allowed_again_once_failed_clears() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let before = running.row_count();

    running.observe(HostActivity::Failed);
    let error = running.start("op-1", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "START_NOT_ALLOWED", "{error}");
    assert_eq!(error["details"]["observedState"], "failed");

    running.observe(HostActivity::Finished);
    let error = running.start("op-2", &upload, "ready").unwrap_err();
    assert_eq!(code(&error), "START_PRECONDITION_CHANGED", "{error}");
    assert_eq!(error["details"]["priorState"], "ready");
    assert_eq!(error["details"]["observedState"], "finished");
    assert_eq!(running.row_count(), before, "no row on any rejection");

    running.observe(HostActivity::Idle);
    let row = running.wait_settled(&id_of(&running.start("op-3", &upload, "ready").unwrap()));
    assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
}

#[test]
fn a_second_start_from_finished_needs_prior_state_finished() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    rig.fake.with_state(|state| {
        state.print_state = "complete".to_string();
        state.print_filename = HOST_PATH.to_string();
    });
    running.observe(HostActivity::Finished);
    let row = running.wait_settled(&id_of(
        &running.start("op-start", &upload, "finished").unwrap(),
    ));
    assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
}

#[test]
fn pause_is_refused_with_no_row_while_the_host_is_idle() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Printing);
    let before = running.row_count();
    let error = running.control("pause", "op-pause").unwrap_err();
    assert_eq!(code(&error), "CONTROL_NOT_ALLOWED", "{error}");
    assert_eq!(error["details"]["verb"], "pause");
    assert_eq!(error["details"]["observedState"], "ready");
    assert_eq!(running.row_count(), before);

    running.observe(HostActivity::Idle);
    let error = running.control("resume", "op-resume").unwrap_err();
    assert_eq!(code(&error), "CONTROL_NOT_ALLOWED");
    assert_eq!(error["details"]["verb"], "resume");
    assert_eq!(running.row_count(), before);
    assert_eq!(requests_to(&rig.fake, "POST", "/printer/print/pause"), 0);
}

#[test]
fn a_second_write_while_one_is_unresolved_is_host_operation_pending() {
    let rig = Rig::new();
    let running = rig.boot();
    running.observe(HostActivity::Idle);
    rig.fake.fault(Route::Upload, Fault::StoreThenDropResponse);
    let id = id_of(&running.stage("op-stage").unwrap());
    running.wait_settled(&id);
    let before = running.row_count();
    let error = running.stage("op-stage-2").unwrap_err();
    assert_eq!(code(&error), "HOST_OPERATION_PENDING", "{error}");
    assert_eq!(error["details"]["hostOperationIds"], json!([id]));
    assert_eq!(error["details"]["printerIds"], json!([PRINTER]));
    assert_eq!(running.row_count(), before);
}

#[test]
fn an_octoprint_printer_is_capability_unsupported_with_no_row() {
    let rig = Rig::new();
    let running = rig.boot();
    let before = running.row_count();
    let error = running
        .call(
            "stage_slice_revision",
            json!({"operationId": "op-o", "printerId": OCTO_PRINTER, "sliceRevisionId": SLR}),
        )
        .unwrap_err();
    assert_eq!(code(&error), "CAPABILITY_UNSUPPORTED", "{error}");
    assert_eq!(error["details"]["capability"], "upload");
    assert_eq!(error["details"]["printerId"], OCTO_PRINTER);
    for verb in ["pause", "resume", "cancel"] {
        let error = running
            .call(
                &format!("{verb}_host_print"),
                json!({"operationId": format!("op-{verb}"), "printerId": OCTO_PRINTER}),
            )
            .unwrap_err();
        assert_eq!(code(&error), "CAPABILITY_UNSUPPORTED", "{verb}");
        assert_eq!(error["details"]["capability"], verb);
    }
    assert_eq!(running.row_count(), before);
}

#[test]
fn the_production_registry_reports_moonraker_writes_unsupported_until_evidence_lands() {
    // `capabilities_for` over the registry: Task 12 adds the evidence rows.
    let moonraker = descriptor(MOONRAKER_KIND).unwrap();
    assert!(moonraker.evidence.is_empty());
    let printer = StoredPrinter {
        connection: Some(FakeMoonraker::start().config()),
        ..common::a_stored_printer(PRINTER)
    };
    let factory = host_ops::RegistryCapabilityFactory;
    let capabilities = factory.capabilities(&printer, None);
    assert!(matches!(
        capabilities.capabilities[CapabilityKey::Upload],
        CapabilityState::Unsupported { .. }
    ));
}

#[test]
fn an_archived_printer_is_rejected_before_any_row_is_written() {
    let rig = Rig::new();
    let running = rig.boot();
    let revision = PrinterRepository::new(Arc::clone(&running.storage))
        .get(PRINTER)
        .unwrap()
        .unwrap()
        .revision;
    running
        .call(
            "archive_printer",
            json!({"id": PRINTER, "expectedRevision": revision, "operationId": "op-arch", "spoolDispositions": []}),
        )
        .expect("archive");
    let before = running.row_count();
    let error = running.stage("op-stage").unwrap_err();
    assert_eq!(code(&error), "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "printerId");
    assert_eq!(error["message"], "Unarchive this Printer first.");

    let error = running
        .call(
            "stage_slice_revision",
            json!({"operationId": "op-x", "printerId": "no-such-printer", "sliceRevisionId": SLR}),
        )
        .unwrap_err();
    assert_eq!(code(&error), "NOT_FOUND");
    assert_eq!(running.row_count(), before);
    assert_eq!(uploads(&rig.fake), 0);
}

#[test]
fn staging_is_refused_with_no_row_while_the_printer_is_not_online() {
    let rig = Rig::new();
    let running = rig.boot();
    let error = running.stage("op-stage").unwrap_err();
    assert_eq!(code(&error), "PRINTER_UNREACHABLE", "{error}");
    assert_eq!(running.row_count(), 0);

    running.observe(HostActivity::Idle);
    let error = running
        .call(
            "stage_slice_revision",
            json!({"operationId": "op-s", "printerId": PRINTER, "sliceRevisionId": "slr-missing"}),
        )
        .unwrap_err();
    assert_eq!(code(&error), "NOT_FOUND");
    assert_eq!(running.row_count(), 0);
}

// --- replay ---------------------------------------------------------------------------

#[test]
fn every_write_command_replays_its_operation_id_with_no_new_row_or_request() {
    let rig = Rig::new();
    let running = rig.boot();

    // stage
    running.observe(HostActivity::Idle);
    let first = running.stage("op-stage").unwrap();
    let upload = id_of(&first);
    running.wait_settled(&upload);
    let replay = running.stage("op-stage").unwrap();
    assert_eq!(id_of(&replay), upload);
    assert_eq!(replay["state"], "succeeded", "the current row");
    assert_eq!(uploads(&rig.fake), 1);

    // start
    let start = id_of(&running.start("op-start", &upload, "ready").unwrap());
    running.wait_settled(&start);
    let requests = rig.fake.requests().len();
    assert_eq!(
        id_of(&running.start("op-start", &upload, "ready").unwrap()),
        start
    );
    assert_eq!(
        rig.fake.requests().len(),
        requests,
        "no pre-check, no host call"
    );

    // pause, resume, cancel
    running.observe(HostActivity::Printing);
    let pause = id_of(&running.control("pause", "op-pause").unwrap());
    running.wait_settled(&pause);
    let requests = rig.fake.requests().len();
    assert_eq!(id_of(&running.control("pause", "op-pause").unwrap()), pause);
    assert_eq!(rig.fake.requests().len(), requests);
    running.observe(HostActivity::Paused);
    let resume = id_of(&running.control("resume", "op-resume").unwrap());
    running.wait_settled(&resume);
    assert_eq!(
        id_of(&running.control("resume", "op-resume").unwrap()),
        resume
    );
    running.observe(HostActivity::Printing);
    let cancel = id_of(&running.control("cancel", "op-cancel").unwrap());
    running.wait_settled(&cancel);
    assert_eq!(
        id_of(&running.control("cancel", "op-cancel").unwrap()),
        cancel
    );

    // A reused id for a different request.
    let error = running.control("pause", "op-cancel").unwrap_err();
    assert_eq!(code(&error), "VALIDATION");
    assert_eq!(error["details"]["fieldPath"], "operationId");

    // abandon
    let seed_rig = Rig::new();
    let seed = Arc::new(Storage::open(seed_rig.paths.clone(), &seed_rig.lease).unwrap());
    let stuck = seed_unreachable_uncertain(&seed, HostOperationKind::Upload);
    let other = seed_rig.boot();
    other.wait_for(&stuck, |row| row.attempts >= 1);
    let abandoned = other.abandon("op-abandon", &stuck).unwrap();
    let events = other.events().len();
    let replayed = other.abandon("op-abandon", &stuck).unwrap();
    assert_eq!(replayed, abandoned);
    assert_eq!(other.events().len(), events, "a replay emits nothing");
    assert_eq!(running.row_count(), 5);
}

// --- events and backfill ------------------------------------------------------------

#[test]
fn every_committed_change_emits_one_operation_changed_event_in_sequence() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let events = running.events();
    // write-ahead, mark_sent, succeeded.
    let states: Vec<&str> = events
        .iter()
        .filter(|event| event["subject"]["id"] == upload.as_str())
        .map(|event| event["payload"]["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["dispatching", "dispatching", "succeeded"]);
    for event in &events {
        assert_eq!(event["type"], "hostOperations.operation.changed");
        assert_eq!(event["subject"]["kind"], "hostOperation");
    }
    let sequences: Vec<u64> = events
        .iter()
        .map(|event| event["sequence"].as_u64().unwrap())
        .collect();
    assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1));
    let marked = events
        .iter()
        .find(|event| {
            event["subject"]["id"] == upload.as_str() && !event["payload"]["dispatchedAt"].is_null()
        })
        .expect("mark_sent emitted");
    assert_eq!(marked["payload"]["state"], "dispatching");

    // An uncertain start and its reconcile: `reconciling` is committed and
    // emitted before the outcome.
    let start = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::PrintingOnly)),
    );
    running.reconcile(&start.id).unwrap();
    let events = running.events();
    let states: Vec<&str> = events
        .iter()
        .filter(|event| event["subject"]["id"] == start.id.as_str())
        .map(|event| event["payload"]["state"].as_str().unwrap())
        .collect();
    assert_eq!(
        states,
        [
            "dispatching",
            "dispatching",
            "uncertain",
            "reconciling",
            "succeeded"
        ]
    );
    let sequences: Vec<u64> = events
        .iter()
        .map(|event| event["sequence"].as_u64().unwrap())
        .collect();
    assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1));
}

#[test]
fn the_backfill_sequence_is_read_before_its_rows() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    let snapshot = running.call("list_host_operations", json!({})).unwrap();
    let last_event = running
        .events()
        .last()
        .map(|event| event["sequence"].as_u64().unwrap())
        .unwrap();
    assert_eq!(snapshot["snapshotSequence"].as_u64().unwrap(), last_event);
    assert_eq!(snapshot["streamId"], running.events()[0]["streamId"]);
    let ids: Vec<String> = snapshot["operations"]
        .as_array()
        .unwrap()
        .iter()
        .map(id_of)
        .collect();
    assert_eq!(ids, std::slice::from_ref(&upload));

    let scoped = running
        .call("list_host_operations", json!({"printerId": OCTO_PRINTER}))
        .unwrap();
    assert_eq!(scoped["operations"], json!([]));

    // Every event after the snapshot has a larger sequence.
    running.observe(HostActivity::Idle);
    let start = id_of(&running.start("op-start", &upload, "ready").unwrap());
    running.wait_settled(&start);
    assert!(running
        .events()
        .iter()
        .filter(|event| event["subject"]["id"] == start.as_str())
        .all(|event| event["sequence"].as_u64().unwrap()
            > snapshot["snapshotSequence"].as_u64().unwrap()));
}

// --- secrets ------------------------------------------------------------------------

#[test]
fn the_seeded_api_key_is_in_no_event_error_or_row() {
    let rig = Rig::new();
    let running = rig.boot();
    let upload = staged_upload(&running, "op-stage");
    // An uncertain start, a reconcile, a rejected command, and an abandon.
    let start = start_with(
        &running,
        &rig.fake,
        &upload,
        Some(Fault::ApplyStartThenDrop(StartTrace::CompleteOnly)),
    );
    let reconciled = running.reconcile(&start.id).unwrap();
    let errors = vec![
        running.stage("op-again").unwrap_err(),
        running.control("pause", "op-pause").unwrap_err(),
        running.abandon("op-bad", &upload).unwrap_err(),
    ];
    running.abandon("op-abandon", &start.id).unwrap();
    rig.fake
        .with_state(|state| state.api_key = Some("a-different-key".to_string()));
    let unauthorized = running.stage("op-auth");
    let snapshot = running.call("list_host_operations", json!({})).unwrap();

    let mut haystack = format!("{reconciled}{snapshot}{unauthorized:?}");
    for error in &errors {
        haystack.push_str(&error.to_string());
    }
    for event in running.events() {
        haystack.push_str(&event.to_string());
    }
    let rows = running
        .storage
        .read(|connection| Ok(host_ops_repo::snapshot(connection, None)))
        .unwrap()
        .unwrap();
    for row in rows {
        haystack.push_str(&format!("{row:?}"));
    }
    assert!(!haystack.contains(SECRET), "the API key leaked");
    // The main file and its WAL and shared-memory files: uncheckpointed
    // writes live only in the `-wal` file.
    for suffix in ["", "-wal", "-shm"] {
        let mut path = rig.paths.database().as_os_str().to_owned();
        path.push(suffix);
        let Ok(bytes) = std::fs::read(&path) else {
            assert_ne!(suffix, "", "the database file is readable");
            continue;
        };
        assert!(
            !bytes
                .windows(SECRET.len())
                .any(|window| window == SECRET.as_bytes()),
            "the API key is in database{suffix}"
        );
    }
    // The key was used: the fake only answers requests that carry it.
    assert!(rig.fake.requests().iter().any(|request| request
        .headers
        .get("x-api-key")
        .map(String::as_str)
        == Some(SECRET)));
}
