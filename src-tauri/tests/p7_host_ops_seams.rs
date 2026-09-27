//! P7 Task 7 (spec D4, D7, D9, "Wire types", "Error codes"): the host-ops
//! seams P7's dispatch driver builds on, against `FakeMoonraker`:
//!
//! - `host_ops::api::{stage, start, control}` with a Job link that runs
//!   inside P6's write-ahead transaction, commits with the row, and rolls
//!   the whole write-ahead back (nothing sent) when it fails;
//! - `HostOperationServices::subscribe_changes` (every published row, in
//!   commit order) and `ConnectionManager::subscribe_status`;
//! - `HostOperation.jobId` on the wire;
//! - the raw P6 writes refusing with `JOB_ACTIVE` while the Printer has an
//!   active Job, with reconcile and abandon left unguarded (spec D3/D9).

mod common;

use std::collections::BTreeSet;
use std::sync::mpsc::Receiver;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fake_moonraker::{FakeMoonraker, Fault, Route};
use farm3d_lib::catalog::PrinterProfile;
use farm3d_lib::connections::capabilities::{
    capabilities_for, ArtifactStaging, CapabilityEvidence, CapabilityKey, CapabilityMap,
    CapabilityState, EvidenceTier, HostFacts, HostStateQuery, PrintControl, PrinterCapabilities,
    UnsupportedReason,
};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::status_repository::{PrinterTelemetry, ToolTemperature};
use farm3d_lib::connections::supervisor::{ConnectionManager, PrinterSetupFacts, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionObservation, PrinterConnection, MOONRAKER_KIND,
};
use farm3d_lib::host_ops::api::{self, LinkInTx};
use farm3d_lib::host_ops::repository as host_ops_repo;
use farm3d_lib::host_ops::start_rule::ControlVerb;
use farm3d_lib::host_ops::{
    self, CapabilityFactory, Clock, FaultAction, FaultPoint, HostOperation, HostOperationServices,
    HostOperationState, HostOpsTimings, PriorState, SystemClock,
};
use farm3d_lib::jobs::PrinterSnapshot;
use farm3d_lib::library::content::ContentStore;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::operational::HostActivity;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;

const PRINTER: &str = "printer-a";
const SLR: &str = "slr-a";
const CREDENTIAL_REF: &str = "farm3d/printer/printer-a/apikey";
const SECRET: &str = "SEEDED-API-KEY-p7-seams-4b21";
const NOW: &str = "2026-09-27T00:00:00Z";
const JOB: &str = "job-seams-a";

// --- rig -----------------------------------------------------------------------

const NOT_VERIFIED: &str = "Not verified for this Connection type yet.";

/// Moonraker capabilities with short timings; every capability the
/// registry calls "not verified" is treated as supported (as in
/// `p6_host_ops.rs`), so these tests don't depend on the evidence rows.
struct TestFactory;

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

impl CapabilityFactory for TestFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        let mut capabilities = capabilities_for(printer, host_facts);
        let current = capabilities.capabilities.clone();
        capabilities.capabilities =
            CapabilityMap::complete(|key: CapabilityKey| match &current[key] {
                CapabilityState::Unsupported {
                    reason: UnsupportedReason::NotVerified,
                    detail,
                } if detail == NOT_VERIFIED => CapabilityState::Supported {
                    evidence: CapabilityEvidence {
                        source: "tests/p7_host_ops_seams.rs".to_string(),
                        tier: EvidenceTier::Sim,
                        verified_host_versions: Vec::new(),
                    },
                },
                other => other.clone(),
            });
        capabilities
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

/// Production values, except a short verification window and automatic
/// retries an hour out, so none races the test's own steps.
fn test_timings() -> HostOpsTimings {
    HostOpsTimings {
        verify_window: Duration::from_millis(800),
        verify_poll_interval: Duration::from_millis(50),
        backoff: |_| Duration::from_secs(3600),
        ..HostOpsTimings::default()
    }
}

fn gcode() -> Vec<u8> {
    let mut bytes = b"; farm3d P7 host-ops seams artifact\n".to_vec();
    for line in 0..300 {
        bytes.extend_from_slice(format!("G1 X{line} Y{line}\n").as_bytes());
    }
    bytes
}

fn unused_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

struct Rig {
    _roots: tempfile::TempDir,
    paths: StoragePaths,
    _lease: MetadataRootLease,
    _credentials: tempfile::TempDir,
    fake: FakeMoonraker,
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    manager: Arc<ConnectionManager<MockRuntime>>,
    services: Arc<farm3d_lib::RuntimeServices<MockRuntime>>,
    storage: Arc<Storage>,
    events: Arc<Mutex<Vec<String>>>,
}

impl Rig {
    fn new() -> Self {
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
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                connection: Some(config),
                ..common::a_stored_printer(PRINTER)
            })
            .unwrap();
        seed_slice_revision(&storage, SLR, &gcode());
        seed_marker_table(&storage);

        let factory: Arc<dyn CapabilityFactory> = Arc::new(TestFactory);
        let clock: Arc<dyn Clock> = Arc::new(SystemClock);
        let (app, webview, manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::host_ops::commands::stage_slice_revision,
                farm3d_lib::host_ops::commands::start_staged_artifact,
                farm3d_lib::host_ops::commands::pause_host_print,
                farm3d_lib::host_ops::commands::resume_host_print,
                farm3d_lib::host_ops::commands::cancel_host_print,
                farm3d_lib::host_ops::commands::reconcile_host_operation,
                farm3d_lib::host_ops::commands::abandon_host_operation,
            ],
            Arc::clone(&storage),
            Arc::new(common::a_catalog()),
            credentials.path().to_path_buf(),
            unused_factory,
            move |services| {
                services.host_ops = Arc::new(HostOperationServices::new(
                    Arc::clone(&services.storage),
                    Arc::clone(&services.library.content),
                    Arc::clone(&services.manager),
                    factory,
                    clock,
                    test_timings(),
                ));
            },
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        app.listen(STATUS_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_string());
        });
        farm3d_lib::start_host_ops_runtime(&services, app.handle());
        Self {
            _roots: roots,
            paths,
            _lease: lease,
            _credentials: credentials,
            fake,
            _app: app,
            webview,
            manager,
            services,
            storage,
            events,
        }
    }

    fn host_ops(&self) -> &Arc<HostOperationServices<MockRuntime>> {
        &self.services.host_ops
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    fn raw_stage(&self, operation_id: &str) -> Result<Value, Value> {
        self.call(
            "stage_slice_revision",
            json!({"operationId": operation_id, "printerId": PRINTER, "sliceRevisionId": SLR}),
        )
    }

    fn linked_stage(
        &self,
        operation_id: &str,
        link: Option<(String, LinkInTx<'_>)>,
    ) -> Result<HostOperation, farm3d_lib::contracts::command::CommandError> {
        tauri::async_runtime::block_on(api::stage(
            self.host_ops(),
            operation_id.to_string(),
            PRINTER.to_string(),
            SLR.to_string(),
            link,
        ))
    }

    fn row(&self, id: &str) -> HostOperation {
        self.storage
            .read(|connection| Ok(host_ops_repo::load(connection, id)))
            .unwrap()
            .unwrap()
            .expect("row exists")
    }

    fn scalar(&self, sql: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

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

    /// A live telemetry frame: Online and fresh. Waits for the Online
    /// hook's host-facts read, so its attempt can't land mid-test.
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
        while self.host_ops().capabilities(&printer).host_facts.is_none() {
            assert!(
                Instant::now() < deadline,
                "the Online hook read no host facts"
            );
            std::thread::sleep(Duration::from_millis(10));
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    /// The `hostOperations` events' payload rows, in emission order.
    fn host_operation_events(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|payload| serde_json::from_str::<Value>(payload).unwrap())
            .filter(|event| {
                event["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("hostOperations."))
            })
            .map(|event| event["payload"].clone())
            .collect()
    }

    fn uploads(&self) -> usize {
        self.fake
            .requests()
            .iter()
            .filter(|request| request.method == "POST" && request.path() == "/server/files/upload")
            .count()
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

/// An external Slice Revision whose G-code blob is in the content store.
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

/// The table a test link writes its marker to: proof the link's write
/// committed (or rolled back) with the write-ahead.
fn seed_marker_table(storage: &Arc<Storage>) {
    storage
        .write_repo(|tx| {
            tx.execute_batch(
                "CREATE TABLE test_link_marker(host_operation_id TEXT NOT NULL, job_id TEXT NOT NULL);",
            )
            .map_err(RepositoryError::from)
        })
        .unwrap();
}

/// An `assigned` (so active) Job on PRINTER over SLR, with the Spool,
/// reservation, and Queue Entry its foreign keys need.
fn seed_active_job(storage: &Arc<Storage>) {
    let snapshot = serde_json::to_string(&PrinterSnapshot {
        name: "Test Printer".to_string(),
        location: None,
        catalog_ref: None,
        adapter_kind: Some(MOONRAKER_KIND.to_string()),
        profile: PrinterProfile::from(&common::a_catalog().models[0].variants[0]),
    })
    .unwrap();
    storage
        .write_repo(|tx| {
            tx.execute_batch(&format!(
                "INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
                   color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
                   created_at, updated_at)
                 VALUES ('spl-seams', 1, 1, 'Acme', 'PLA', 'Black', '1.75', 1000000,
                         1000000, 'measured', 'active', '{NOW}', '{NOW}');
                 INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg,
                   state, operation_id, created_at)
                 VALUES ('rsv-seams', 'spl-seams', 'job', '{JOB}', 12500, 'active',
                         'rsv-seams-op', '{NOW}');
                 INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                   state, position, policy, preference, estimate_mg, estimate_source,
                   created_at, updated_at)
                 VALUES ('qen-seams', 1, '{SLR}', 'qln-seams', 1, 'queued', 1,
                         'manual', 'loadedFirst', 12500, 'operatorEntered', '{NOW}', '{NOW}');
                 INSERT INTO jobs(
                   id, revision, queue_entry_id, slice_revision_id, printer_id,
                   printer_snapshot_json, spool_id, reservation_id, estimate_mg, state,
                   settlement, assigned_by, created_at, updated_at
                 ) VALUES ('{JOB}', 1, 'qen-seams', '{SLR}', '{PRINTER}', '{snapshot}', 'spl-seams',
                           'rsv-seams', 12500, 'assigned', 'open', 'operator', '{NOW}', '{NOW}');
                 UPDATE queue_entries SET state = 'assigned', job_id = '{JOB}' WHERE id = 'qen-seams';"
            ))
            .map_err(RepositoryError::from)
        })
        .unwrap();
}

/// A link that asserts it runs inside the write-ahead (its row and ledger
/// claim are visible, carrying the Job's id) and writes a marker row.
fn marker_link(seen: Arc<Mutex<Option<String>>>) -> (String, LinkInTx<'static>) {
    (
        JOB.to_string(),
        Box::new(move |tx, operation| {
            let in_tx: Option<String> = tx.query_row(
                "SELECT job_id FROM host_operations WHERE id = ?1",
                [&operation.id],
                |row| row.get(0),
            )?;
            assert_eq!(in_tx.as_deref(), Some(JOB), "the row is written, linked");
            assert_eq!(operation.job_id.as_deref(), Some(JOB));
            assert_eq!(operation.state, HostOperationState::Dispatching);
            tx.execute(
                "INSERT INTO test_link_marker(host_operation_id, job_id) VALUES (?1, ?2)",
                [&operation.id, JOB],
            )?;
            *seen.lock().unwrap() = Some(operation.id.clone());
            Ok(())
        }),
    )
}

fn code(error: &Value) -> &str {
    error["code"].as_str().unwrap_or_default()
}

fn wait_fired(fired: Receiver<()>) {
    fired
        .recv_timeout(Duration::from_secs(10))
        .expect("the fault point was reached");
    std::thread::sleep(Duration::from_millis(100));
}

// --- the Job link --------------------------------------------------------------

#[test]
fn link_runs_in_the_write_ahead_transaction_and_commits_with_the_row() {
    let rig = Rig::new();
    seed_active_job(&rig.storage);
    rig.observe(HostActivity::Idle);
    let seen = Arc::new(Mutex::new(None));

    let row = rig
        .linked_stage("op-1#hostOperation", Some(marker_link(Arc::clone(&seen))))
        .expect("a linked stage");

    assert_eq!(row.job_id.as_deref(), Some(JOB));
    assert_eq!(seen.lock().unwrap().as_deref(), Some(row.id.as_str()));
    assert_eq!(
        rig.scalar(&format!(
            "SELECT COUNT(*) FROM test_link_marker WHERE host_operation_id = '{}' AND job_id = '{JOB}'",
            row.id
        )),
        1,
        "the link's write committed with the row"
    );
    assert_eq!(
        rig.scalar("SELECT COUNT(*) FROM operations WHERE id = 'op-1#hostOperation' AND kind = 'stageSliceRevision'"),
        1
    );
    let settled = rig.wait_settled(&row.id);
    assert_eq!(settled.state, HostOperationState::Succeeded, "{settled:?}");
    assert_eq!(
        settled.job_id.as_deref(),
        Some(JOB),
        "job_id survives every transition"
    );
    assert_eq!(rig.uploads(), 1);
}

#[test]
fn link_error_rolls_back_the_write_ahead_and_sends_nothing() {
    let rig = Rig::new();
    seed_active_job(&rig.storage);
    rig.observe(HostActivity::Idle);
    let ran = Arc::new(Mutex::new(false));
    let flag = Arc::clone(&ran);
    let link: LinkInTx<'static> = Box::new(move |tx, operation| {
        tx.execute(
            "INSERT INTO test_link_marker(host_operation_id, job_id) VALUES (?1, ?2)",
            [&operation.id, JOB],
        )?;
        *flag.lock().unwrap() = true;
        Err(RepositoryError::Validation {
            field_path: "jobId",
        })
    });

    let error = rig
        .linked_stage("op-2#hostOperation", Some((JOB.to_string(), link)))
        .expect_err("the link refused");

    assert!(*ran.lock().unwrap(), "the link ran");
    let error = serde_json::to_value(&error).unwrap();
    assert_eq!(code(&error), "VALIDATION", "{error}");
    assert_eq!(rig.scalar("SELECT COUNT(*) FROM host_operations"), 0);
    assert_eq!(rig.scalar("SELECT COUNT(*) FROM test_link_marker"), 0);
    assert_eq!(
        rig.scalar("SELECT COUNT(*) FROM operations WHERE id = 'op-2#hostOperation'"),
        0,
        "a rolled-back write-ahead never burns its id"
    );
    assert!(rig.host_operation_events().is_empty(), "nothing published");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(rig.uploads(), 0, "nothing was sent");
}

#[test]
fn crash_between_write_ahead_and_send_leaves_a_linked_dispatching_row() {
    let rig = Rig::new();
    seed_active_job(&rig.storage);
    rig.observe(HostActivity::Idle);
    let fired = rig
        .host_ops()
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Crash);
    let seen = Arc::new(Mutex::new(None));

    let row = rig
        .linked_stage("op-3#hostOperation", Some(marker_link(seen)))
        .expect("a linked stage");
    wait_fired(fired);

    let left = rig.row(&row.id);
    assert_eq!(left.state, HostOperationState::Dispatching);
    assert!(left.dispatched_at.is_none());
    assert_eq!(left.job_id.as_deref(), Some(JOB));
    assert_eq!(rig.scalar("SELECT COUNT(*) FROM test_link_marker"), 1);
    assert_eq!(rig.uploads(), 0);

    // A restart's recovery settles it never-sent, still linked.
    let reopened = Storage::open(rig.paths.clone(), &rig._lease).unwrap();
    host_ops::recover_after_restart(&reopened, chrono::Utc::now()).unwrap();
    let recovered = rig.row(&row.id);
    assert_eq!(recovered.state, HostOperationState::Failed, "{recovered:?}");
    assert_eq!(recovered.job_id.as_deref(), Some(JOB));
}

/// A link that fails the test if it ever runs (a replay must not run it).
fn never_link(job_id: &str) -> (String, LinkInTx<'static>) {
    (
        job_id.to_string(),
        Box::new(|_, _| panic!("a replayed operation id re-ran its link")),
    )
}

fn marker_count(rig: &Rig) -> i64 {
    rig.scalar("SELECT COUNT(*) FROM test_link_marker")
}

/// Task 7 review: a replay returns the stored row without running the
/// link, and a replay whose stored row belongs to another Job (or to no
/// Job) is a reused operation id (`VALIDATION` on `operationId`).
#[test]
fn a_replayed_linked_operation_never_reruns_its_link_and_must_name_the_same_job() {
    let rig = Rig::new();
    seed_active_job(&rig.storage);
    rig.observe(HostActivity::Idle);
    let seen = Arc::new(Mutex::new(None));
    let row = rig
        .linked_stage("op-r#hostOperation", Some(marker_link(seen)))
        .expect("a linked stage");
    rig.wait_settled(&row.id);
    assert_eq!(marker_count(&rig), 1);

    let replayed = rig
        .linked_stage("op-r#hostOperation", Some(never_link(JOB)))
        .expect("a replay");
    assert_eq!(replayed.id, row.id);
    assert_eq!(marker_count(&rig), 1, "the link did not run again");

    for link in [Some(never_link("job-someone-else")), None] {
        let error = serde_json::to_value(
            rig.linked_stage("op-r#hostOperation", link)
                .expect_err("another Job's operation id"),
        )
        .unwrap();
        assert_eq!(code(&error), "VALIDATION", "{error}");
        assert_eq!(error["details"]["fieldPath"], "operationId", "{error}");
    }
    assert_eq!(rig.scalar("SELECT COUNT(*) FROM host_operations"), 1);
    assert_eq!(rig.uploads(), 1);
}

/// Task 7 review: linked `start` and `control` run their links inside the
/// write-ahead and carry the Job, as `stage` does.
#[test]
fn linked_start_and_control_run_their_links_and_carry_the_job() {
    let rig = Rig::new();
    seed_active_job(&rig.storage);
    rig.observe(HostActivity::Idle);
    let upload = rig
        .linked_stage("op-u#hostOperation", Some(marker_link(Arc::new(Mutex::new(None)))))
        .expect("a linked stage");
    assert_eq!(rig.wait_settled(&upload.id).state, HostOperationState::Succeeded);

    let seen = Arc::new(Mutex::new(None));
    let start = tauri::async_runtime::block_on(api::start(
        rig.host_ops(),
        "op-s#hostOperation".to_string(),
        PRINTER.to_string(),
        upload.id.clone(),
        PriorState::Ready,
        false,
        Some(marker_link(Arc::clone(&seen))),
    ))
    .expect("a linked start");
    assert_eq!(start.job_id.as_deref(), Some(JOB));
    assert_eq!(seen.lock().unwrap().as_deref(), Some(start.id.as_str()));
    assert_eq!(marker_count(&rig), 2);
    assert_eq!(rig.wait_settled(&start.id).state, HostOperationState::Succeeded);

    rig.manager.apply_observation(
        PRINTER,
        ConnectionObservation::Telemetry(telemetry(HostActivity::Printing)),
        PrinterSetupFacts::complete(),
    );
    let seen = Arc::new(Mutex::new(None));
    let pause = tauri::async_runtime::block_on(api::control(
        rig.host_ops(),
        "op-p#hostOperation".to_string(),
        PRINTER.to_string(),
        ControlVerb::Pause,
        Some(marker_link(Arc::clone(&seen))),
    ))
    .expect("a linked pause");
    assert_eq!(pause.job_id.as_deref(), Some(JOB));
    assert_eq!(pause.host_path, upload.host_path, "the host reports the staged file");
    assert_eq!(seen.lock().unwrap().as_deref(), Some(pause.id.as_str()));
    assert_eq!(marker_count(&rig), 3);
    assert_eq!(rig.wait_settled(&pause.id).state, HostOperationState::Succeeded);
}

// --- broadcasts -----------------------------------------------------------------

#[test]
fn subscribe_changes_sees_every_published_row_in_commit_order() {
    let rig = Rig::new();
    rig.observe(HostActivity::Idle);
    let mut changes = rig.host_ops().subscribe_changes();

    let staged = rig.raw_stage("op-raw").expect("stage");
    let id = staged["id"].as_str().unwrap().to_string();
    let settled = rig.wait_settled(&id);
    assert_eq!(settled.state, HostOperationState::Succeeded);
    std::thread::sleep(Duration::from_millis(100));

    let mut seen = Vec::new();
    while let Ok(row) = changes.try_recv() {
        seen.push(row);
    }
    let published = rig.host_operation_events();
    assert!(seen.len() >= 3, "write-ahead, mark-sent, outcome: {seen:?}");
    assert_eq!(
        seen.iter()
            .map(|row| serde_json::to_value(row).unwrap())
            .collect::<Vec<_>>(),
        published,
        "the broadcast carries exactly the published rows, in order"
    );
    assert_eq!(seen.first().unwrap().state, HostOperationState::Dispatching);
    assert!(seen.first().unwrap().dispatched_at.is_none());
    assert_eq!(seen.last().unwrap(), &settled);
    assert!(seen.iter().all(|row| row.id == id));
}

#[test]
fn subscribe_status_fires_on_apply_observation() {
    let rig = Rig::new();
    let mut statuses = rig.manager.subscribe_status();

    rig.manager.apply_observation(
        PRINTER,
        ConnectionObservation::Telemetry(telemetry(HostActivity::Idle)),
        PrinterSetupFacts::complete(),
    );

    assert_eq!(statuses.try_recv().expect("a status broadcast"), PRINTER);
}

// --- the raw-write guard (D9) ---------------------------------------------------

#[test]
fn raw_stage_on_a_printer_with_an_active_job_is_job_active() {
    let rig = Rig::new();
    seed_active_job(&rig.storage);
    rig.observe(HostActivity::Idle);

    let error = rig.raw_stage("op-raw").unwrap_err();
    assert_eq!(code(&error), "JOB_ACTIVE", "{error}");
    assert_eq!(error["details"]["printerId"], PRINTER);
    assert_eq!(error["details"]["jobId"], JOB);
    assert_eq!(error["recovery"], json!(["OPEN_JOB"]));
    assert_eq!(rig.scalar("SELECT COUNT(*) FROM host_operations"), 0);
    assert_eq!(
        rig.scalar("SELECT COUNT(*) FROM operations WHERE id = 'op-raw'"),
        0,
        "a refusal never burns the id"
    );
    assert!(rig.host_operation_events().is_empty());
    assert_eq!(rig.uploads(), 0);

    // The raw controls step aside too.
    rig.fake.with_state(|state| {
        state.print_state = "printing".to_string();
        state.print_filename = "farm3d/slr-a.gcode".to_string();
    });
    rig.observe(HostActivity::Printing);
    for verb in ["pause", "cancel"] {
        let error = rig
            .call(
                &format!("{verb}_host_print"),
                json!({"operationId": format!("op-{verb}"), "printerId": PRINTER}),
            )
            .unwrap_err();
        assert_eq!(code(&error), "JOB_ACTIVE", "{verb}: {error}");
    }
    assert_eq!(rig.scalar("SELECT COUNT(*) FROM host_operations"), 0);
}

#[test]
fn reconcile_and_abandon_stay_unguarded_with_an_active_job() {
    let rig = Rig::new();
    rig.observe(HostActivity::Idle);
    rig.fake.fault(Route::Upload, Fault::DropMidBody);
    let staged = rig.raw_stage("op-stage").unwrap();
    let id = staged["id"].as_str().unwrap().to_string();
    let row = rig.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");

    seed_active_job(&rig.storage);

    let checked: HostOperation = serde_json::from_value(
        rig.call("reconcile_host_operation", json!({"hostOperationId": id}))
            .expect("reconcile is not guarded"),
    )
    .unwrap();
    assert!(checked.attempts >= 1, "{checked:?}");
    assert_eq!(checked.state, HostOperationState::Uncertain);

    let abandoned: HostOperation = serde_json::from_value(
        rig.call(
            "abandon_host_operation",
            json!({
                "operationId": "op-abandon", "hostOperationId": id,
                "acknowledgement": "hostStateUnknown",
            }),
        )
        .expect("abandon is not guarded"),
    )
    .unwrap();
    assert_eq!(abandoned.state, HostOperationState::Abandoned);
}

// --- the wire -------------------------------------------------------------------

#[test]
fn host_operation_job_id_serializes_and_is_null_for_raw_writes() {
    let rig = Rig::new();
    rig.observe(HostActivity::Idle);

    let raw = rig.raw_stage("op-raw").expect("stage");
    assert!(raw.as_object().unwrap().contains_key("jobId"), "{raw}");
    assert_eq!(raw["jobId"], Value::Null);
    let raw_id = raw["id"].as_str().unwrap().to_string();
    assert_eq!(rig.wait_settled(&raw_id).job_id, None);

    seed_active_job(&rig.storage);
    let linked = rig
        .linked_stage(
            "op-linked#hostOperation",
            Some(marker_link(Arc::new(Mutex::new(None)))),
        )
        .expect("a linked stage");
    let value = serde_json::to_value(&linked).unwrap();
    assert_eq!(value["jobId"], JOB);
    let keys: BTreeSet<&str> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    assert!(
        !keys.contains("job_id") && !keys.contains("historyMark"),
        "{keys:?}"
    );
    let settled = rig.wait_settled(&linked.id);
    assert_eq!(settled.job_id.as_deref(), Some(JOB));
    // No credential ever reaches the row.
    assert!(!value.to_string().contains(SECRET));
}
