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
    ArtifactStaging, CapabilityEvidence, CapabilityKey, CapabilityMap, CapabilityState, EvidenceTier, HostFacts,
    HostStateQuery, PrintControl, PrinterCapabilities, UnsupportedReason,
};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::supervisor::{ConnectionManager, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND,
};
use chrono::{DateTime, Utc};
use farm3d_lib::host_ops::repository as host_ops_repo;
use farm3d_lib::jobs::{JobServices, JobTimings};
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
use rusqlite::OptionalExtension;
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
/// How often a wait on a Job re-reads it. Each read is a `get_job_history`
/// (several fresh SQLite connections, each parsing the schema), so a
/// tighter loop across parallel tests only adds contention.
const POLL: Duration = Duration::from_millis(50);

// --- capabilities: every capability supported with sim evidence ---------------

struct SimFactory {
    upload_unsupported: Arc<std::sync::atomic::AtomicBool>,
    tier: Arc<Mutex<EvidenceTier>>,
}

/// The adapter's timeouts against the in-process fake. No test here needs
/// one to fire: an unreachable fake drops the connection at once, and no
/// fault stalls past them. So they only have to be longer than a loaded
/// machine can take to answer on loopback (sub-2 s values turned a slow but
/// healthy fake into `TIMEOUT` refusals and `uncertain` uploads and starts
/// when many test binaries ran at once), and shorter than [`WAIT`], so a
/// request that really hangs still fails the wait around it.
fn rig_moonraker_timings() -> MoonrakerTimings {
    MoonrakerTimings {
        connect: Duration::from_secs(5),
        query: Duration::from_secs(10),
        control: Duration::from_secs(10),
        transfer_base: Duration::from_secs(10),
        transfer_per_started_mib: Duration::from_millis(10),
    }
}

fn adapter(
    config: &ConnectionConfig,
    key: Option<zeroize::Zeroizing<String>>,
) -> Option<MoonrakerCapabilities> {
    (config.kind == MOONRAKER_KIND)
        .then(|| MoonrakerCapabilities::new(config, key, rig_moonraker_timings()))
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
            capabilities: CapabilityMap::complete(|key| {
                if key == CapabilityKey::Upload
                    && self.upload_unsupported.load(std::sync::atomic::Ordering::SeqCst)
                {
                    return CapabilityState::Unsupported {
                        reason: UnsupportedReason::NotVerified,
                        detail: "Uploads are switched off in this test.".to_string(),
                    };
                }
                CapabilityState::Supported {
                    evidence: CapabilityEvidence {
                        source: "tests/p7_dispatch_rig".to_string(),
                        tier: *self.tier.lock().unwrap(),
                        verified_host_versions: Vec::new(),
                    },
                }
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
    /// When set, the Printer's `upload` capability is unsupported.
    pub upload_unsupported: Arc<std::sync::atomic::AtomicBool>,
    /// The evidence tier every supported capability carries (`sim` by
    /// default; Task 10 sets `readOnlyHardware` for an unproven adapter).
    pub evidence_tier: Arc<Mutex<EvidenceTier>>,
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
            upload_unsupported: Arc::default(),
            evidence_tier: Arc::new(Mutex::new(EvidenceTier::Sim)),
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

// --- time ------------------------------------------------------------------------

/// A clock a test moves by hand (the tracker's `host_unreachable_since` and
/// `declareOutcome`), starting at the real time.
pub struct ManualClock(Mutex<DateTime<Utc>>);

impl ManualClock {
    pub fn new() -> Arc<Self> {
        Arc::new(Self(Mutex::new(Utc::now())))
    }

    pub fn advance(&self, by: Duration) {
        let mut now = self.0.lock().unwrap();
        *now += chrono::Duration::from_std(by).unwrap();
    }
}

impl Clock for ManualClock {
    fn now(&self) -> DateTime<Utc> {
        *self.0.lock().unwrap()
    }
}

/// Short tracker timings: a 200 ms history poll, the production limit of
/// three inconclusive polls, and the production 30 minutes before a
/// declare (tests move a [`ManualClock`] instead of waiting).
pub fn fast() -> JobTimings {
    JobTimings {
        history_poll: Duration::from_millis(200),
        ..JobTimings::default()
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

/// Dropping a running app stops its Job runtime. `RuntimeServices` holds
/// the `AppHandle` whose state holds `RuntimeServices`, so dropping the app
/// never frees them, and without a stop the driver (and the evaluator)
/// would keep running to the end of the test binary. With `fast()` timings
/// each one polls its fake's history every 200 ms, and the leaked runtimes
/// of earlier tests starve later ones on a loaded machine.
impl Drop for Running {
    fn drop(&mut self) {
        self.services.jobs.stop();
    }
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
    boot_with(roots, driver, status_of(OperationalState::Ready))
}

/// [`boot`], with the Printer's status `initial` when the runtimes start
/// (production starts them while the Printer is still `Connecting`).
pub fn boot_with(roots: &Roots, driver: Driver, initial: PrinterStatus) -> Running {
    boot_tuned(roots, driver, initial, JobTimings::default(), None)
}

/// [`boot_with`], with the Job runtime's timings and (optionally) its
/// clock injected.
pub fn boot_tuned(
    roots: &Roots,
    driver: Driver,
    initial: PrinterStatus,
    timings: JobTimings,
    job_clock: Option<Arc<dyn Clock>>,
) -> Running {
    boot_prepared(roots, driver, initial, timings, job_clock, |_| {})
}

/// [`boot_tuned`], with `prepare` run over the built services before any
/// runtime starts (Task 10: to install the evaluator's test hooks before
/// its first run).
pub fn boot_prepared(
    roots: &Roots,
    driver: Driver,
    initial: PrinterStatus,
    timings: JobTimings,
    job_clock: Option<Arc<dyn Clock>>,
    prepare: impl FnOnce(&Arc<RuntimeServices<MockRuntime>>),
) -> Running {
    let storage = Arc::new(Storage::open(roots.paths.clone(), &roots.lease).unwrap());
    host_ops::recover_after_restart(&storage, SystemClock.now()).unwrap();
    let recovered = farm3d_lib::jobs::recover_after_restart(&storage, SystemClock.now()).unwrap();
    let factory: Arc<dyn CapabilityFactory> = Arc::new(SimFactory {
        upload_unsupported: Arc::clone(&roots.upload_unsupported),
        tier: Arc::clone(&roots.evidence_tier),
    });
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let (app, webview, manager, services) = common::runtime_with(
        tauri::generate_handler![
            farm3d_lib::queue::commands::list_queue,
            farm3d_lib::queue::commands::add_to_queue,
            farm3d_lib::queue::commands::update_queue_entry,
            farm3d_lib::queue::commands::move_queue_entry,
            farm3d_lib::queue::commands::remove_queue_entry,
            farm3d_lib::jobs::commands::assign_queue_entry,
            farm3d_lib::jobs::commands::stage_job,
            farm3d_lib::jobs::commands::start_job,
            farm3d_lib::jobs::commands::pause_job,
            farm3d_lib::jobs::commands::resume_job,
            farm3d_lib::jobs::commands::cancel_job,
            farm3d_lib::jobs::commands::release_job,
            farm3d_lib::jobs::commands::get_job_history,
            farm3d_lib::jobs::commands::declare_job_outcome,
            farm3d_lib::jobs::commands::settle_job_material,
            farm3d_lib::jobs::commands::correct_job_material,
            farm3d_lib::queue::commands::explain_queue_entry,
            farm3d_lib::host_ops::commands::reconcile_host_operation,
            farm3d_lib::host_ops::commands::abandon_host_operation,
            farm3d_lib::spools::commands::move_spool,
            farm3d_lib::spools::commands::spool_history,
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
            services.jobs = Arc::new(match job_clock {
                Some(clock) => JobServices::with_clock(timings, clock),
                None => JobServices::new(timings),
            });
        },
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&events);
    app.listen(STATUS_EVENT, move |event| {
        sink.lock().unwrap().push(event.payload().to_string());
    });
    manager.seed(PRINTER, initial);
    prepare(&services);
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

/// What `fake` reports, as a live status: Online and fresh, with its
/// print state, file, and progress.
pub fn status_from(fake: &FakeMoonraker) -> PrinterStatus {
    let (print_state, filename, _) = fake.print_state();
    let progress = fake.with_state(|state| state.progress);
    let mut status = status_of(match print_state.as_str() {
        "printing" => OperationalState::Printing,
        "paused" => OperationalState::Paused,
        "complete" => OperationalState::Finished,
        "cancelled" => OperationalState::Cancelled,
        "error" => OperationalState::Failed,
        _ => OperationalState::Ready,
    });
    status.telemetry.job_name = (!filename.is_empty()).then_some(filename);
    status.telemetry.progress = Some(progress);
    status
}

/// The status a restored Connection has before its first contact.
pub fn connecting() -> PrinterStatus {
    PrinterStatus::new(ConnectionState::Connecting)
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

    /// Waits until the driver's first pass (its first resync) has run.
    pub fn wait_first_pass(&self) {
        let deadline = Instant::now() + WAIT;
        while self.services.jobs.resyncs() == 0 {
            assert!(Instant::now() < deadline, "the driver's first pass never ran");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Sets the Printer's live status to `status`.
    pub fn seed(&self, status: PrinterStatus) {
        self.manager.seed(PRINTER, status);
    }

    /// Sets the Printer's live status to what `fake` reports: Online and
    /// fresh, its print state, file, and progress.
    pub fn mirror(&self, fake: &FakeMoonraker) {
        self.seed(status_from(fake));
    }

    /// Waits until the driver has run `count` whole passes that started
    /// after this call (each pass polls every `printing`/`paused` Job's
    /// history). One pass may already be running, hence the extra one.
    pub fn wait_passes(&self, count: u64) {
        let target = self.services.jobs.resyncs() + count + 1;
        let deadline = Instant::now() + WAIT;
        while self.services.jobs.resyncs() < target {
            assert!(Instant::now() < deadline, "the driver never ran {count} more passes");
            std::thread::sleep(Duration::from_millis(10));
        }
    }

    /// `declare_job_outcome` with the right acknowledgement.
    pub fn declare(&self, operation_id: &str, job_id: &str, outcome: &str) -> Result<Value, Value> {
        self.call(
            "declare_job_outcome",
            json!({
                "operationId": operation_id,
                "jobId": job_id,
                "outcome": outcome,
                "acknowledgement": "hostStateUnknown",
            }),
        )
    }

    /// How many timeline events of `kind` the Job has (one SQL read).
    pub fn count_events(&self, job_id: &str, kind: &str) -> usize {
        self.storage
            .read(|connection| {
                connection.query_row(
                    "SELECT COUNT(*) FROM job_events WHERE job_id = ?1 AND kind = ?2",
                    [job_id, kind],
                    |row| row.get::<_, i64>(0),
                )
            })
            .unwrap() as usize
    }

    /// Waits until `done` holds, re-checking every [`POLL`]. For conditions
    /// on stored columns, which don't need the presented Job.
    pub fn wait_until(&self, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + WAIT;
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting until {what}");
            std::thread::sleep(POLL);
        }
    }

    /// A backend-only `jobs` column, as SQLite has it.
    pub fn job_column(&self, job_id: &str, column: &str) -> Option<i64> {
        self.storage
            .read(|connection| {
                connection.query_row(
                    &format!("SELECT {column} FROM jobs WHERE id = ?1"),
                    [job_id],
                    |row| row.get(0),
                )
            })
            .unwrap()
    }

    /// A Spool in storage, PLA 1.75, 1 kg on it.
    pub fn spool(&self) -> String {
        self.spool_sized(1_000_000)
    }

    /// A Spool in storage, PLA 1.75, `nominal_mg` on it (Task 9: sized
    /// tightly against the rig's fixed 12.5 g estimate for the
    /// over-reservation tests).
    pub fn spool_sized(&self, nominal_mg: i64) -> String {
        let fields = SpoolFields {
            manufacturer: "Polymaker".to_string(),
            product: None,
            material_family: MaterialFamily::Pla,
            material_other: None,
            color_name: "Black".to_string(),
            color_hex: None,
            diameter: FilamentDiameter::D175,
            nominal_mg,
            low_threshold_mg: (nominal_mg / 10).max(1),
            tare_id: None,
            notes: None,
        };
        let entry = AmountEntry::Net {
            net_mg: nominal_mg,
            confidence: AmountConfidence::Estimated,
        };
        self.storage
            .write_repo(|tx| spools_repository::insert_spool(tx, &fields, &entry, None))
            .unwrap()
            .id
    }

    /// Loads `spool_id` into the Printer's one slot through `move_spool`.
    pub fn load(&self, spool_id: &str) {
        let (revision, slot_id, occupant): (i64, String, Option<String>) = self
            .storage
            .read(|connection| {
                let slot_id: String = connection.query_row(
                    "SELECT id FROM material_slots WHERE printer_id = ?1",
                    [PRINTER],
                    |row| row.get(0),
                )?;
                Ok((
                    connection.query_row(
                        "SELECT revision FROM spools WHERE id = ?1",
                        [spool_id],
                        |row| row.get(0),
                    )?,
                    slot_id.clone(),
                    connection.query_row(
                        "SELECT id FROM spools WHERE slot_id = ?1",
                        [&slot_id],
                        |row| row.get(0),
                    ).optional()?,
                ))
            })
            .unwrap();
        self.ok(
            "move_spool",
            json!({
                "operationId": format!("load-{}", uuid::Uuid::new_v4()),
                "spoolId": spool_id,
                "expectedSpoolRevision": revision,
                "destination": {"kind": "slot", "slotId": slot_id, "expectedOccupantSpoolId": occupant},
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
        self.wait_job_within(job_id, state, WAIT)
    }

    /// [`Running::wait_job`] with its own deadline: shorter than the
    /// driver's 10 s poll, it proves a wake (not the poll) moved the Job.
    pub fn wait_job_within(&self, job_id: &str, state: &str, within: Duration) -> Value {
        let deadline = Instant::now() + within;
        loop {
            // One SQL read per round; the presented Job only at the end.
            let stored = self.text(&format!("SELECT state FROM jobs WHERE id = '{job_id}'"));
            if stored.as_deref() == Some(state) {
                return self.job(job_id);
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for {state}: {}\nits Host Operations: {:?}",
                self.job(job_id),
                self.ops(job_id)
            );
            std::thread::sleep(POLL);
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
            assert!(
                Instant::now() < deadline,
                "timed out waiting on {job}\nits Host Operations: {:?}",
                self.ops(job_id)
            );
            std::thread::sleep(POLL);
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
            std::thread::sleep(POLL);
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
            std::thread::sleep(POLL);
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
        self.awaiting_start_with(&spool)
    }

    /// [`Running::awaiting_start`] against an existing Spool (Task 9: the
    /// settlement tests size their own Spools).
    pub fn awaiting_start_with(&self, spool_id: &str) -> String {
        self.load(spool_id);
        let job = self.assign(spool_id);
        self.wait_job(&job, "awaitingStart");
        job
    }

    /// A Job printing (started by the operator), its history job pinned
    /// by the tracker's check on entering `printing`: the Job id.
    pub fn printing(&self) -> String {
        let spool = self.spool();
        self.printing_with(&spool)
    }

    /// [`Running::printing`] against an existing Spool.
    pub fn printing_with(&self, spool_id: &str) -> String {
        let job = self.awaiting_start_with(spool_id);
        self.start(&format!("start-{job}"), &job, "ready")
            .expect("start_job");
        self.wait_job(&job, "printing");
        self.wait_until("pinned", || self.count_events(&job, "hostJobPinned") == 1);
        self.status(OperationalState::Printing);
        job
    }

    /// `settle_job_material`.
    pub fn settle(&self, operation_id: &str, job_id: &str, choice: Value) -> Result<Value, Value> {
        self.call(
            "settle_job_material",
            json!({"operationId": operation_id, "jobId": job_id, "choice": choice}),
        )
    }

    /// `correct_job_material`.
    pub fn correct(&self, operation_id: &str, job_id: &str, entry: Value) -> Result<Value, Value> {
        self.call(
            "correct_job_material",
            json!({"operationId": operation_id, "jobId": job_id, "entry": entry}),
        )
    }

    /// A reservation's stored `state` (Task 9: not on the Job's wire type).
    pub fn reservation_state(&self, reservation_id: &str) -> String {
        self.text(&format!(
            "SELECT state FROM spool_reservations WHERE id = '{reservation_id}'"
        ))
        .unwrap()
    }

    /// A Spool's stored `current_mg`.
    pub fn spool_current_mg(&self, spool_id: &str) -> i64 {
        self.scalar(&format!("SELECT current_mg FROM spools WHERE id = '{spool_id}'"))
    }

    /// `spool_history`'s ledger rows (Task 9: to check `isCorrection`).
    pub fn amount_events(&self, spool_id: &str) -> Vec<Value> {
        self.ok("spool_history", json!({"spoolId": spool_id}))["amountEvents"]
            .as_array()
            .unwrap()
            .clone()
    }

    /// How many captured stream events of `event_type` name `subject_id`
    /// (Task 9: counting `queue.job.changed`/`queue.requirement.changed`/
    /// `spool.changed` for the exactly-once publish tests).
    pub fn count_stream_events(&self, event_type: &str, subject_id: &str) -> usize {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str::<Value>(text).unwrap())
            .filter(|event| event["type"] == event_type && event["subject"]["id"] == subject_id)
            .count()
    }
}
