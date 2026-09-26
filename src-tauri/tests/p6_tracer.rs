//! P6 Task 13: the end-to-end reconciliation tracer (spec acceptance,
//! global constraint 8).
//!
//! One core function, [`run_reconciliation_tracer`], runs from two
//! entry points:
//!
//! - `reconciliation_tracer_runs_against_fake_moonraker` (CI): against
//!   [`FakeMoonraker`], using `tests/fixtures/library/plain.gcode`.
//! - `p6_reconciliation_tracer_runs_against_the_simulator` (`#[ignore]`,
//!   the evidence run `just test-sim` drives): against the Moonraker
//!   simulator, using Task 12's no-motion fixture (comments and `M117`
//!   only), since the simulator actually executes the G-code it is sent.
//!
//! Both runs:
//!
//! 1. Import a G-code file and create an external Slice Revision from it.
//! 2. `stage_slice_revision` with the response cut after the host stores
//!    the file (the fake's drop-after-store fault, or `cut_after`).
//! 3. Assert archive, delete, Connection clear, and revision delete are
//!    all blocked while the upload is unresolved.
//! 4. Restart: rebuild `RuntimeServices` over the same roots.
//! 5. Reconcile. Assert `succeeded`, exactly one file at
//!    `farm3d/<slr-id>.gcode` with a matching hash, no second upload, and
//!    no start (the host is still standby; no new history job).
//! 6. Assert the guards have lifted.
//! 7. Repeat on a fresh Printer at the same endpoint (now free), with the
//!    host unreachable throughout instead: reconciliation can never prove
//!    anything, so the row stays `uncertain`, is abandoned, and the guards
//!    lift with the row kept `abandoned`.

mod common;
mod sim;

use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fake_moonraker::{FakeMoonraker, Fault, Route};
use farm3d_lib::connections::capabilities::{
    capabilities_for, ArtifactStaging, CapabilityEvidence, CapabilityMap, CapabilityState,
    CommandFailure, EvidenceTier, HostFacts, HostStateQuery, InconclusiveReason, LocateOutcome,
    PrintControl, PrinterCapabilities, StagedArtifact, UnsupportedReason,
};
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::status_repository::{PrinterTelemetry, ToolTemperature};
use farm3d_lib::connections::supervisor::{ConnectionManager, PrinterSetupFacts};
use farm3d_lib::connections::{ConnectionConfig, ConnectionObservation, PrinterConnection};
use farm3d_lib::host_ops::{
    self, CapabilityFactory, Clock, HostOperation, HostOperationResolution, HostOperationServices,
    HostOperationState, HostOpsTimings, SystemClock,
};
use farm3d_lib::library::selection::SelectionPurpose;
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::operational::HostActivity;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use sim::moonraker::{no_motion_gcode, MoonrakerSim};
use sim::toxiproxy::{Proxy, Toxiproxy};
use tauri::test::MockRuntime;

/// How long any one host operation may take to settle in these tests.
const WAIT: Duration = Duration::from_secs(90);

fn fixtures() -> std::path::PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(future)
}

fn no_observation_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

fn id_of(row: &Value) -> String {
    row["id"].as_str().expect("a row id").to_string()
}

fn reason(row: &HostOperation) -> Option<InconclusiveReason> {
    row.last_attempt.as_ref().map(|attempt| attempt.reason)
}

// ---------------------------------------------------------------------------
// Backend: what differs between the fake and the simulator.
// ---------------------------------------------------------------------------

/// Everything the tracer's core needs from whichever host it is run
/// against, so [`run_reconciliation_tracer`] is written once.
trait Backend {
    fn connection(&self) -> ConnectionConfig;
    fn factory(&self) -> Arc<dyn CapabilityFactory>;
    fn timings(&self) -> HostOpsTimings;
    /// Arms the response-lost-after-store fault for the very next upload.
    fn arm_drop_after_store(&self);
    /// `false` makes the host unreachable throughout; `true` restores it.
    fn set_reachable(&self, reachable: bool);
    fn print_stats_standby(&self) -> bool;
    /// How many history jobs the host reports right now.
    fn history_len(&self) -> usize;
    /// Fails unless the host holds exactly one file at `host_path`.
    fn assert_single_upload(&self, host_path: &str);
    /// The production identity check, run directly against the host.
    fn locate(&self, host_path: &str, sha256: &str, size: u64) -> LocateOutcome;
}

/// D6's write capabilities are the only ones ever marked "not verified" by
/// `capabilities_for`; promoting them to `Supported` here (evidence tier
/// `Sim`, as `p6_host_ops.rs` and `sim_moonraker.rs` both do for their own
/// fixed-outcome hosts) means the tracer's stage/reconcile calls never
/// depend on the registry's own detection.
fn promote_not_verified(capabilities: PrinterCapabilities, source: &str) -> PrinterCapabilities {
    let mut capabilities = capabilities;
    let current = capabilities.capabilities.clone();
    capabilities.capabilities = CapabilityMap::complete(|key| match &current[key] {
        CapabilityState::Unsupported {
            reason: UnsupportedReason::NotVerified,
            ..
        } => CapabilityState::Supported {
            evidence: CapabilityEvidence {
                source: source.to_string(),
                tier: EvidenceTier::Sim,
                verified_host_versions: Vec::new(),
            },
        },
        other => other.clone(),
    });
    capabilities
}

// --- the fake (CI) -----------------------------------------------------------------

struct FakeFactory;

impl CapabilityFactory for FakeFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        promote_not_verified(capabilities_for(printer, host_facts), "tests/p6_tracer.rs")
    }

    fn staging(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn ArtifactStaging>> {
        Some(Box::new(MoonrakerCapabilities::new(
            config,
            key,
            fake_moonraker_timings(),
        )))
    }

    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        Some(Box::new(MoonrakerCapabilities::new(
            config,
            key,
            fake_moonraker_timings(),
        )))
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        Some(Box::new(MoonrakerCapabilities::new(
            config,
            key,
            fake_moonraker_timings(),
        )))
    }
}

fn fake_moonraker_timings() -> MoonrakerTimings {
    MoonrakerTimings {
        connect: Duration::from_millis(500),
        query: Duration::from_millis(1500),
        control: Duration::from_millis(1500),
        transfer_base: Duration::from_millis(1500),
        transfer_per_started_mib: Duration::from_millis(10),
        ..MoonrakerTimings::default()
    }
}

/// A one-second verification window (nothing here waits on it), and
/// automatic retries an hour out so they never race the test's own
/// `reconcile_host_operation` calls.
fn fake_tracer_timings() -> HostOpsTimings {
    HostOpsTimings {
        verify_window: Duration::from_millis(800),
        verify_poll_interval: Duration::from_millis(50),
        backoff: |_| Duration::from_secs(3600),
        ..HostOpsTimings::default()
    }
}

struct FakeBackend {
    fake: FakeMoonraker,
    factory: Arc<FakeFactory>,
}

impl FakeBackend {
    fn new(fake: FakeMoonraker) -> Self {
        Self {
            fake,
            factory: Arc::new(FakeFactory),
        }
    }
}

impl Backend for FakeBackend {
    fn connection(&self) -> ConnectionConfig {
        self.fake.config()
    }

    fn factory(&self) -> Arc<dyn CapabilityFactory> {
        Arc::clone(&self.factory) as Arc<dyn CapabilityFactory>
    }

    fn timings(&self) -> HostOpsTimings {
        fake_tracer_timings()
    }

    fn arm_drop_after_store(&self) {
        self.fake.fault(Route::Upload, Fault::StoreThenDropResponse);
    }

    fn set_reachable(&self, reachable: bool) {
        self.fake.set_reachable(reachable);
    }

    fn print_stats_standby(&self) -> bool {
        self.fake.print_state().0 == "standby"
    }

    fn history_len(&self) -> usize {
        self.fake.history().len()
    }

    fn assert_single_upload(&self, host_path: &str) {
        let uploads = self
            .fake
            .requests()
            .iter()
            .filter(|request| request.method == "POST" && request.path() == "/server/files/upload")
            .count();
        assert_eq!(uploads, 1, "exactly one upload to {host_path}");
    }

    fn locate(&self, host_path: &str, sha256: &str, size: u64) -> LocateOutcome {
        let adapter = MoonrakerCapabilities::new(&self.connection(), None, fake_moonraker_timings());
        block_on(adapter.locate(&StagedArtifact {
            host_path: host_path.to_string(),
            sha256: sha256.to_string(),
            size,
        }))
        .unwrap_or_else(|error| panic!("locate: {error:?}"))
    }
}

// --- the simulator -----------------------------------------------------------------

type Hook = Box<dyn FnOnce() + Send>;

/// A fault applied around exactly one upload: `before` right before the
/// request, `after` as soon as the client has its answer (so the reads the
/// executor makes first, and reconciliation's own reads, are never cut;
/// see `sim_moonraker.rs`'s P6 section).
struct Armed {
    before: Hook,
    after: Hook,
}

#[derive(Clone, Default)]
struct Trigger(Arc<Mutex<Option<Armed>>>);

impl Trigger {
    fn arm(&self, before: impl FnOnce() + Send + 'static) {
        self.arm_with(before, || Toxiproxy::discover().unwrap().reset());
    }

    fn arm_with(&self, before: impl FnOnce() + Send + 'static, after: impl FnOnce() + Send + 'static) {
        *self.0.lock().unwrap() = Some(Armed {
            before: Box::new(before),
            after: Box::new(after),
        });
    }

    fn take(&self) -> Option<(Hook, Hook)> {
        self.0
            .lock()
            .unwrap()
            .take()
            .map(|armed| (armed.before, armed.after))
    }
}

async fn run_blocking(hook: Hook) {
    tokio::task::spawn_blocking(hook)
        .await
        .expect("a fault hook panicked");
}

struct ArmedStaging {
    inner: MoonrakerCapabilities,
    trigger: Trigger,
}

#[async_trait::async_trait]
impl ArtifactStaging for ArmedStaging {
    async fn upload(
        &self,
        artifact: &StagedArtifact,
        body: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    ) -> Result<(), CommandFailure> {
        let Some((before, after)) = self.trigger.take() else {
            return self.inner.upload(artifact, body).await;
        };
        run_blocking(before).await;
        let result = self.inner.upload(artifact, body).await;
        run_blocking(after).await;
        result
    }

    async fn locate(
        &self,
        artifact: &StagedArtifact,
    ) -> Result<LocateOutcome, farm3d_lib::connections::ConnectionError> {
        self.inner.locate(artifact).await
    }
}

struct SimFactory {
    timings: MoonrakerTimings,
    trigger: Trigger,
}

impl SimFactory {
    fn new() -> Self {
        Self {
            timings: sim_moonraker_timings(),
            trigger: Trigger::default(),
        }
    }
}

impl CapabilityFactory for SimFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        promote_not_verified(capabilities_for(printer, host_facts), "tests/p6_tracer.rs")
    }

    fn staging(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn ArtifactStaging>> {
        Some(Box::new(ArmedStaging {
            inner: MoonrakerCapabilities::new(config, key, self.timings),
            trigger: self.trigger.clone(),
        }))
    }

    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        Some(Box::new(MoonrakerCapabilities::new(
            config,
            key,
            self.timings,
        )))
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        Some(Box::new(MoonrakerCapabilities::new(
            config,
            key,
            self.timings,
        )))
    }
}

/// Simulavr is slow: a long connect/query/control/transfer allowance.
fn sim_moonraker_timings() -> MoonrakerTimings {
    MoonrakerTimings {
        connect: Duration::from_secs(2),
        query: Duration::from_secs(10),
        control: Duration::from_secs(20),
        transfer_base: Duration::from_secs(30),
        transfer_per_started_mib: Duration::from_millis(250),
        ..MoonrakerTimings::default()
    }
}

/// A short settle period, a long verification window, and automatic
/// retries an hour out so they never race the test's own reconcile calls.
fn sim_tracer_timings() -> HostOpsTimings {
    HostOpsTimings {
        settle_period: Duration::from_secs(3),
        verify_window: Duration::from_secs(15),
        verify_poll_interval: Duration::from_millis(250),
        backoff: |_| Duration::from_secs(3600),
        ..HostOpsTimings::default()
    }
}

struct SimBackend<'s> {
    sim: &'s MoonrakerSim,
    factory: Arc<SimFactory>,
}

impl<'s> SimBackend<'s> {
    fn new(sim: &'s MoonrakerSim) -> Self {
        Self {
            sim,
            factory: Arc::new(SimFactory::new()),
        }
    }
}

impl Backend for SimBackend<'_> {
    fn connection(&self) -> ConnectionConfig {
        self.sim.config()
    }

    fn factory(&self) -> Arc<dyn CapabilityFactory> {
        Arc::clone(&self.factory) as Arc<dyn CapabilityFactory>
    }

    fn timings(&self) -> HostOpsTimings {
        sim_tracer_timings()
    }

    fn arm_drop_after_store(&self) {
        self.factory
            .trigger
            .arm(|| Toxiproxy::discover().unwrap().cut_after(Proxy::Moonraker, 0));
    }

    fn set_reachable(&self, reachable: bool) {
        Toxiproxy::discover()
            .unwrap()
            .set_enabled(Proxy::Moonraker, reachable);
    }

    fn print_stats_standby(&self) -> bool {
        self.sim.print_stats().0 == "standby"
    }

    fn history_len(&self) -> usize {
        self.sim.history("limit=200")["result"]["jobs"]
            .as_array()
            .map(Vec::len)
            .unwrap_or(0)
    }

    fn assert_single_upload(&self, host_path: &str) {
        assert_eq!(
            self.sim.gcode_files(host_path),
            vec![host_path.to_string()],
            "exactly one file at {host_path}"
        );
    }

    fn locate(&self, host_path: &str, sha256: &str, size: u64) -> LocateOutcome {
        let adapter = MoonrakerCapabilities::new(&self.connection(), None, sim_moonraker_timings());
        block_on(adapter.locate(&StagedArtifact {
            host_path: host_path.to_string(),
            sha256: sha256.to_string(),
            size,
        }))
        .unwrap_or_else(|error| panic!("locate: {error:?}"))
    }
}

// ---------------------------------------------------------------------------
// The shared harness: identical regardless of which `Backend` is behind it.
// ---------------------------------------------------------------------------

fn tracer_telemetry(activity: HostActivity) -> PrinterTelemetry {
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
        tools: vec![ToolTemperature {
            index: 0,
            temp_c: Some(25.0),
            target_c: Some(0.0),
        }],
    }
}

fn create_printer(paths: &StoragePaths, lease: &MetadataRootLease, id: &str, connection: &ConnectionConfig) {
    let storage = Storage::open(paths.clone(), lease).unwrap();
    PrinterRepository::new(Arc::new(storage))
        .create(StoredPrinter {
            connection: Some(connection.clone()),
            ..common::a_stored_printer(id)
        })
        .unwrap();
}

struct Running {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    manager: Arc<ConnectionManager<MockRuntime>>,
    services: Arc<RuntimeServices<MockRuntime>>,
    storage: Arc<Storage>,
}

/// Opens the roots and boots an app, running startup recovery first as
/// `build_runtime_services` does. Every boot after the first is a restart.
fn boot(
    paths: &StoragePaths,
    lease: &MetadataRootLease,
    credentials: &Path,
    factory: Arc<dyn CapabilityFactory>,
    timings: HostOpsTimings,
) -> Running {
    let storage = Arc::new(Storage::open(paths.clone(), lease).unwrap());
    host_ops::recover_after_restart(&storage, SystemClock.now()).unwrap();
    let (app, webview, manager, services) = common::runtime_with(
        tauri::generate_handler![
            farm3d_lib::library::commands::inspect_import_selection,
            farm3d_lib::library::commands::import_models,
            farm3d_lib::slicing::commands::create_external_slice_revision,
            farm3d_lib::slicing::commands::delete_slice_revision,
            farm3d_lib::host_ops::commands::stage_slice_revision,
            farm3d_lib::host_ops::commands::reconcile_host_operation,
            farm3d_lib::host_ops::commands::abandon_host_operation,
            farm3d_lib::printers::commands::archive_printer,
            farm3d_lib::printers::commands::delete_printer,
            farm3d_lib::connections::commands::clear_printer_connection,
        ],
        Arc::clone(&storage),
        Arc::new(common::a_catalog()),
        credentials.to_path_buf(),
        no_observation_factory,
        move |services| {
            services.host_ops = Arc::new(HostOperationServices::new(
                Arc::clone(&services.storage),
                Arc::clone(&services.library.content),
                Arc::clone(&services.manager),
                factory,
                Arc::new(SystemClock),
                timings,
            ));
        },
    );
    farm3d_lib::start_host_ops_runtime(&services, app.handle());
    Running {
        _app: app,
        webview,
        manager,
        services,
        storage,
    }
}

impl Running {
    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    /// Imports `path` (managed) directly through the Library's selection
    /// registry, bypassing the native file picker, and returns its Model
    /// record.
    fn import(&self, path: &Path) -> Value {
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
                    "storageMode": "managed",
                    "acknowledgeUnsupported": true,
                }],
            }),
        );
        assert_eq!(imported["items"][0]["outcome"], "imported", "{imported}");
        imported["items"][0]["model"].clone()
    }

    /// D16: an external Slice Revision from `source_revision_id`, the
    /// nozzle confirmed and everything else absent.
    fn create_external(&self, operation_id: &str, source_revision_id: &str) -> String {
        let revision = self.ok(
            "create_external_slice_revision",
            json!({
                "operationId": operation_id,
                "sourceRevisionId": source_revision_id,
                "facts": {
                    "printerProfile": { "kind": "absent" },
                    "nozzleDiameterMm": { "kind": "confirmed", "value": 0.4 },
                    "materialFamily": { "kind": "absent" },
                    "filamentDiameterMm": { "kind": "absent" },
                },
            }),
        );
        assert_eq!(revision["kind"], "external", "{revision}");
        revision["id"].as_str().unwrap().to_string()
    }

    /// The `gcode_sha256`/`gcode_size` a Slice Revision was stored with.
    fn gcode_meta(&self, slice_revision_id: &str) -> (String, u64) {
        self.storage
            .read(|connection| {
                connection.query_row(
                    "SELECT gcode_sha256, gcode_size FROM slice_revisions WHERE id = ?1",
                    [slice_revision_id],
                    |row| Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)? as u64)),
                )
            })
            .unwrap()
    }

    fn printer_revision(&self, printer_id: &str) -> i64 {
        PrinterRepository::new(Arc::clone(&self.storage))
            .get(printer_id)
            .unwrap()
            .unwrap()
            .revision
    }

    /// A live, fresh telemetry frame with `activity`. The first one brings
    /// the Printer Online, whose hook reads the host facts and then
    /// reconciles; this waits for the facts so that hook's attempt cannot
    /// land in the middle of the test's own steps.
    fn observe(&self, printer_id: &str, activity: HostActivity) {
        self.manager.apply_observation(
            printer_id,
            ConnectionObservation::Telemetry(tracer_telemetry(activity)),
            PrinterSetupFacts::complete(),
        );
        let printer = PrinterRepository::new(Arc::clone(&self.storage))
            .get(printer_id)
            .unwrap()
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(20);
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
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    fn stage(&self, printer_id: &str, operation_id: &str, slice_revision_id: &str) -> String {
        id_of(&self.ok(
            "stage_slice_revision",
            json!({
                "operationId": operation_id, "printerId": printer_id,
                "sliceRevisionId": slice_revision_id,
            }),
        ))
    }

    fn row(&self, id: &str) -> HostOperation {
        self.storage
            .read(|connection| Ok(host_ops::repository::load(connection, id)))
            .unwrap()
            .unwrap()
            .expect("row exists")
    }

    /// Waits (up to 90 s) until row `id` satisfies `done`.
    fn wait_for(&self, id: &str, what: &str, done: impl Fn(&HostOperation) -> bool) -> HostOperation {
        let deadline = Instant::now() + WAIT;
        loop {
            let row = self.row(id);
            if done(&row) {
                return row;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {what}: {row:?}"
            );
            std::thread::sleep(Duration::from_millis(100));
        }
    }

    fn wait_settled(&self, id: &str) -> HostOperation {
        self.wait_for(id, "the executor's commit", |row| {
            row.state != HostOperationState::Dispatching
        })
    }

    fn reconcile(&self, id: &str) -> HostOperation {
        serde_json::from_value(self.ok("reconcile_host_operation", json!({ "hostOperationId": id })))
            .unwrap()
    }

    fn abandon(&self, operation_id: &str, id: &str) -> HostOperation {
        serde_json::from_value(self.ok(
            "abandon_host_operation",
            json!({
                "operationId": operation_id, "hostOperationId": id,
                "acknowledgement": "hostStateUnknown",
            }),
        ))
        .unwrap()
    }

    fn archive(&self, printer_id: &str) -> Result<Value, Value> {
        let revision = self.printer_revision(printer_id);
        self.call(
            "archive_printer",
            json!({
                "id": printer_id, "expectedRevision": revision,
                "operationId": format!("op-archive-{}", uuid::Uuid::new_v4()),
                "spoolDispositions": [],
            }),
        )
    }

    fn delete_printer(&self, printer_id: &str) -> Result<Value, Value> {
        let revision = self.printer_revision(printer_id);
        self.call(
            "delete_printer",
            json!({ "id": printer_id, "expectedRevision": revision }),
        )
    }

    fn clear_connection(&self, printer_id: &str) -> Result<Value, Value> {
        let revision = self.printer_revision(printer_id);
        self.call(
            "clear_printer_connection",
            json!({ "id": printer_id, "expectedRevision": revision }),
        )
    }

    fn delete_revision(&self, slice_revision_id: &str) -> Result<Value, Value> {
        self.call(
            "delete_slice_revision",
            json!({ "sliceRevisionId": slice_revision_id }),
        )
    }
}

/// Every action D7 blocks while `printer_id` has an unresolved upload:
/// archive, delete, clearing the Connection, and deleting the staged Slice
/// Revision.
fn assert_guards_blocked(running: &Running, printer_id: &str, slice_revision_id: &str, upload_id: &str) {
    let error = running.archive(printer_id).unwrap_err();
    assert_blocked_by_unresolved_op(&error, "archive");

    let error = running.delete_printer(printer_id).unwrap_err();
    assert_blocked_by_unresolved_op(&error, "delete");

    let error = running.clear_connection(printer_id).unwrap_err();
    assert_eq!(error["code"], "CONNECTION_IN_USE", "{error}");
    assert_eq!(
        error["details"],
        json!({ "printerId": printer_id, "hostOperationId": upload_id })
    );

    let error = running.delete_revision(slice_revision_id).unwrap_err();
    assert_blocked_by_unresolved_op(&error, "delete");
}

fn assert_blocked_by_unresolved_op(error: &Value, action: &str) {
    assert_eq!(error["code"], "LIFECYCLE_BLOCKED", "{error}");
    let blockers = error["details"]["blockers"].as_array().unwrap();
    assert!(
        blockers.iter().any(|blocker| {
            blocker["action"] == action && blocker["code"] == "HOST_OPERATION_UNRESOLVED"
        }),
        "{error}"
    );
}

/// Once the row is terminal, every D7 guard lifts: this actually performs
/// each action (clearing the Connection, deleting the Slice Revision,
/// archiving, then deleting the Printer), which also frees the Printer's
/// (kind, host, port) for a fresh Printer at the same endpoint.
fn assert_guards_lifted(running: &Running, printer_id: &str, slice_revision_id: &str) {
    running
        .clear_connection(printer_id)
        .unwrap_or_else(|error| panic!("clear_printer_connection: {error}"));
    running
        .delete_revision(slice_revision_id)
        .unwrap_or_else(|error| panic!("delete_slice_revision: {error}"));
    running
        .archive(printer_id)
        .unwrap_or_else(|error| panic!("archive_printer: {error}"));
    running
        .delete_printer(printer_id)
        .unwrap_or_else(|error| panic!("delete_printer: {error}"));
}

// ---------------------------------------------------------------------------
// The tracer's core.
// ---------------------------------------------------------------------------

fn run_reconciliation_tracer<B: Backend>(backend: &B, gcode_path: &Path) {
    let roots = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(roots.path().join("metadata"), roots.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let credentials = tempfile::tempdir().unwrap();
    let connection = backend.connection();
    let history_before = backend.history_len();

    // --- Scenario A: the response is lost after the host stores the
    //     file; a restart proves it landed. --------------------------------

    const PRINTER_A: &str = "printer-tracer-a";
    create_printer(&paths, &lease, PRINTER_A, &connection);
    let running = boot(
        &paths,
        &lease,
        credentials.path(),
        backend.factory(),
        backend.timings(),
    );

    let imported = running.import(gcode_path);
    let source_revision_id = imported["currentRevision"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let slr_a = running.create_external("tracer-external-a", &source_revision_id);
    let (sha256, size) = running.gcode_meta(&slr_a);
    let host_path_a = format!("farm3d/{slr_a}.gcode");

    // 2. Stage with the response cut after the host stores the file.
    running.observe(PRINTER_A, HostActivity::Idle);
    backend.arm_drop_after_store();
    let upload_a = running.stage(PRINTER_A, "op-stage-a", &slr_a);
    let row = running.wait_settled(&upload_a);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::ResponseLost));

    // 3. Archive, delete, Connection clear, and revision delete are all
    //    blocked.
    assert_guards_blocked(&running, PRINTER_A, &slr_a, &upload_a);

    // 4. Restart: rebuild `RuntimeServices` over the same roots.
    drop(running);
    let restarted = boot(
        &paths,
        &lease,
        credentials.path(),
        backend.factory(),
        backend.timings(),
    );

    // 5. Reconcile: succeeded, one file, matching hash, no second upload,
    //    no start.
    let row = restarted.reconcile(&upload_a);
    let row = if row.state == HostOperationState::Succeeded {
        row
    } else {
        restarted.wait_for(&upload_a, "the reconciled upload", |row| {
            row.state == HostOperationState::Succeeded
        })
    };
    assert_eq!(
        row.resolution,
        Some(HostOperationResolution::ArtifactVerified { reconciled: true }),
        "{row:?}"
    );
    assert_eq!(
        backend.locate(&host_path_a, &sha256, size),
        LocateOutcome::Matches,
        "the file at {host_path_a} matches what farm3d staged"
    );
    backend.assert_single_upload(&host_path_a);
    assert!(
        backend.print_stats_standby(),
        "no start: the host is still standby"
    );
    assert_eq!(
        backend.history_len(),
        history_before,
        "no start: no new history job"
    );

    // 6. The guards have lifted.
    assert_guards_lifted(&restarted, PRINTER_A, &slr_a);

    // --- Scenario B: the host is unreachable throughout; abandon is the
    //     only way out. Printer A is gone, so its (kind, host, port) is
    //     free for a fresh Printer at the same endpoint. -------------------

    const PRINTER_B: &str = "printer-tracer-b";
    create_printer(&paths, &lease, PRINTER_B, &connection);
    let slr_b = restarted.create_external("tracer-external-b", &source_revision_id);

    restarted.observe(PRINTER_B, HostActivity::Idle);
    backend.arm_drop_after_store();
    let upload_b = restarted.stage(PRINTER_B, "op-stage-b", &slr_b);
    let row = restarted.wait_settled(&upload_b);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::ResponseLost));
    assert_guards_blocked(&restarted, PRINTER_B, &slr_b, &upload_b);

    backend.set_reachable(false);
    drop(restarted);
    let offline = boot(
        &paths,
        &lease,
        credentials.path(),
        backend.factory(),
        backend.timings(),
    );
    // The startup pass makes one attempt; wait for it, then reconcile
    // covers the case where it has not run yet.
    let row = offline.wait_for(&upload_b, "the startup pass's attempt", |row| row.attempts >= 1);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::HostUnreachable));
    let row = offline.reconcile(&upload_b);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::HostUnreachable));

    let abandoned = offline.abandon("op-abandon-b", &upload_b);
    assert_eq!(abandoned.state, HostOperationState::Abandoned, "{abandoned:?}");

    backend.set_reachable(true);
    assert_guards_lifted(&offline, PRINTER_B, &slr_b);
}

// ---------------------------------------------------------------------------
// The two entry points.
// ---------------------------------------------------------------------------

#[test]
fn reconciliation_tracer_runs_against_fake_moonraker() {
    let backend = FakeBackend::new(FakeMoonraker::start());
    let gcode_path = fixtures().join("library/plain.gcode");
    run_reconciliation_tracer(&backend, &gcode_path);
}

#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_reconciliation_tracer_runs_against_the_simulator() {
    let discovered = MoonrakerSim::discover();
    let sim = require_sim!(discovered);
    let _guard = sim::exclusive();
    sim.reset();

    // The tracer never starts a print, so the smallest fixture with at
    // least one command line is enough (`megabytes: 0` has none, which the
    // Library rejects as `INVALID_CONTENT`).
    let bytes = no_motion_gcode("tracer", 1);
    let source = tempfile::tempdir().unwrap();
    let gcode_path = source.path().join("tracer-no-motion.gcode");
    std::fs::write(&gcode_path, &bytes).unwrap();

    let backend = SimBackend::new(&sim);
    run_reconciliation_tracer(&backend, &gcode_path);

    sim.reset();
}
