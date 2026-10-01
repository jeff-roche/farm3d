//! The P7 dispatch rig shared by `p7_jobs.rs` (Task 8a's handoff tests) and
//! `p7_restart_matrix.rs`: durable roots (a leased Storage, a credential
//! directory, and a `FakeMoonraker` Printer with one Material Slot), a farm3d
//! Slice Revision whose G-code is in the content store and whose facts match
//! the Printer, and [`boot`], which builds `RuntimeServices` over those roots
//! the way `build_runtime_services` does: host-ops recovery, then Job
//! recovery, then the runtimes. Every boot after the first is a restart
//! (`p6_tracer.rs`'s pattern).
//!
//! `p7_tracer.rs` also points the rig's Printer at the Moonraker simulator
//! instead of the fake ([`Roots::on_host`]), with its own G-code and
//! timings ([`RigTimings`]); [`WriteCounts`] counts the uploads and starts
//! farm3d sends, whichever host is behind the adapter.
//!
//! Each test crate uses a different subset, hence the `dead_code` allow.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use farm3d_lib::attention::projector::AppliedChanges;
use farm3d_lib::attention::services::{AttentionServices, AttentionTimings};
use farm3d_lib::connections::capabilities::{
    ArtifactStaging, CapabilityEvidence, CapabilityKey, CapabilityMap, CapabilityState,
    CommandFailure, EvidenceTier, HostFacts, HostStateQuery, LocateOutcome, PrintControl,
    PrinterCapabilities, StagedArtifact, UnsupportedReason,
};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::supervisor::{build_connection, ConnectionManager, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND,
};
use farm3d_lib::host_ops::repository as host_ops_repo;
use farm3d_lib::host_ops::{
    self, CapabilityFactory, Clock, HostOperation, HostOperationServices, HostOperationState,
    HostOpsTimings, SystemClock,
};
use farm3d_lib::jobs::{JobServices, JobTimings};
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
    moonraker: MoonrakerTimings,
    writes: Arc<WriteCounts>,
}

/// How many uploads and starts farm3d has sent through the adapter, counted
/// just before each request goes out. For a host that keeps no request log
/// (the simulator), this is the only count of uploads there is.
#[derive(Default, Debug)]
pub struct WriteCounts {
    uploads: AtomicUsize,
    starts: AtomicUsize,
}

impl WriteCounts {
    pub fn uploads(&self) -> usize {
        self.uploads.load(Ordering::SeqCst)
    }

    pub fn starts(&self) -> usize {
        self.starts.load(Ordering::SeqCst)
    }
}

/// The production adapter, with every upload and start counted.
struct Counted {
    inner: MoonrakerCapabilities,
    writes: Arc<WriteCounts>,
}

#[async_trait::async_trait]
impl ArtifactStaging for Counted {
    async fn upload(
        &self,
        artifact: &StagedArtifact,
        body: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    ) -> Result<(), CommandFailure> {
        self.writes.uploads.fetch_add(1, Ordering::SeqCst);
        self.inner.upload(artifact, body).await
    }

    async fn locate(
        &self,
        artifact: &StagedArtifact,
    ) -> Result<LocateOutcome, farm3d_lib::connections::ConnectionError> {
        self.inner.locate(artifact).await
    }
}

#[async_trait::async_trait]
impl PrintControl for Counted {
    async fn start(&self, host_path: &str) -> Result<(), CommandFailure> {
        self.writes.starts.fetch_add(1, Ordering::SeqCst);
        self.inner.start(host_path).await
    }

    async fn pause(&self) -> Result<(), CommandFailure> {
        self.inner.pause().await
    }

    async fn resume(&self) -> Result<(), CommandFailure> {
        self.inner.resume().await
    }

    async fn cancel(&self) -> Result<(), CommandFailure> {
        self.inner.cancel().await
    }
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

impl SimFactory {
    fn adapter(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Counted> {
        (config.kind == MOONRAKER_KIND).then(|| Counted {
            inner: MoonrakerCapabilities::new(config, key, self.moonraker),
            writes: Arc::clone(&self.writes),
        })
    }
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
                    && self
                        .upload_unsupported
                        .load(std::sync::atomic::Ordering::SeqCst)
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
        self.adapter(config, key)
            .map(|adapter| Box::new(adapter) as Box<dyn ArtifactStaging>)
    }

    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        self.adapter(config, key)
            .map(|adapter| Box::new(adapter) as Box<dyn PrintControl>)
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        (config.kind == MOONRAKER_KIND).then(|| {
            Box::new(MoonrakerCapabilities::new(config, key, self.moonraker))
                as Box<dyn HostStateQuery>
        })
    }
}

/// A short verification window, and automatic retries an hour out so none
/// races a test's own steps.
fn rig_host_ops_timings() -> HostOpsTimings {
    HostOpsTimings {
        verify_window: Duration::from_millis(800),
        verify_poll_interval: Duration::from_millis(50),
        backoff: |_| Duration::from_secs(3600),
        ..HostOpsTimings::default()
    }
}

/// The rig's default adapter factory: no adapter at all, so a rig user
/// that starts the supervisor by mistake sees it fail at once instead of
/// reaching a host. Only [`AttentionBoot::real_adapters`] opts out.
fn unused_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

type ConnectionFactory =
    fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Option<Box<dyn PrinterConnection>>;

/// The adapter's and host operations' timings, and how long any one wait
/// on the running app may take. [`RigTimings::default`] suits the
/// in-process fake; a slower host (the simulator) needs longer ones.
#[derive(Clone, Copy)]
pub struct RigTimings {
    pub moonraker: MoonrakerTimings,
    pub host_ops: HostOpsTimings,
    pub wait: Duration,
}

impl Default for RigTimings {
    fn default() -> Self {
        Self {
            moonraker: rig_moonraker_timings(),
            host_ops: rig_host_ops_timings(),
            wait: WAIT,
        }
    }
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

/// A farm3d Slice Revision (PLA 1.75, 0.4 mm, 12.5 g) whose G-code,
/// `bytes`, is in the content store, so an upload has real bytes to send.
fn seed_slice_revision(storage: &Arc<Storage>, bytes: &[u8]) {
    let content = ContentStore::open(storage.paths().content_root()).unwrap();
    let staged = content
        .stage_bytes(bytes, "seed-dispatch", "part.gcode")
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
    /// The in-process host. Idle when the Printer points elsewhere
    /// ([`Roots::on_host`]).
    pub fake: FakeMoonraker,
    /// The uploads and starts farm3d has sent, whichever host is behind
    /// the Printer.
    pub writes: Arc<WriteCounts>,
    pub timings: RigTimings,
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
        Self::on_host(start_safety, None, &gcode(), RigTimings::default())
    }

    /// [`Roots::new`], with the Printer's Connection `target` (the fake,
    /// with its API key, when `None`), the Slice Revision's G-code
    /// `bytes`, and `timings`.
    pub fn on_host(
        start_safety: StartSafety,
        target: Option<ConnectionConfig>,
        bytes: &[u8],
        timings: RigTimings,
    ) -> Self {
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
        let config = target.unwrap_or_else(|| ConnectionConfig {
            credential_ref: Some(CREDENTIAL_REF.to_string()),
            ..fake.config()
        });
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
        seed_slice_revision(&storage, bytes);
        Self {
            _temp: temp,
            paths,
            lease,
            credentials,
            fake,
            writes: Arc::default(),
            timings,
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

/// A history poll (the driver's resync) that never comes during a test:
/// after the first pass, only a wake can move a Job, so a test that ends
/// with `resyncs() == 1` proves the wake did it.
pub fn no_poll() -> JobTimings {
    JobTimings {
        history_poll: Duration::from_secs(3600),
        ..JobTimings::default()
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
    /// How long any one wait may take ([`RigTimings::wait`]).
    pub wait: Duration,
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
        self.services.attention.stop();
        self.services.cameras.stop();
        self.services.notifications.stop();
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
    boot_inner(roots, driver, initial, timings, job_clock, prepare, None).0
}

/// P8: how [`boot_with_attention`] starts the Attention runtime.
pub struct AttentionBoot {
    pub timings: AttentionTimings,
    /// The projector's clock (the offline grace); the real time if `None`.
    pub clock: Option<Arc<dyn Clock>>,
    /// P8 Task 8: `Some` starts the camera capture runtime (and the
    /// `MediaJanitor`) with these timings, before the projector, as
    /// `build_runtime_services` does; `None` leaves captures off.
    pub cameras: Option<farm3d_lib::cameras::services::CameraTimings>,
    /// P8 Task 16: `Some` starts the notification runtime over this sink
    /// and window control, before the projector, as
    /// `build_runtime_services` does; `None` leaves it off.
    pub notifications: Option<NotificationBoot>,
    /// `true` gives the supervisor the production adapters
    /// (`build_connection`), for a test that really starts it (P8's
    /// tracer, around its offline cuts). `false` keeps the rig's
    /// no-adapter factory, as every other rig user seeds the live status.
    pub real_adapters: bool,
}

/// P8 Task 16: what the notification runtime shows through and raises.
pub struct NotificationBoot {
    pub sink: Arc<dyn farm3d_lib::notifications::NotificationSink>,
    pub control: Arc<dyn farm3d_lib::notifications::activation::WindowControl>,
}

/// [`boot_tuned`] with the Attention projector as `build_runtime_services`
/// runs it: the startup backfill right after Job recovery, and
/// `start_attention_runtime` after `start_jobs_runtime`. Returns the
/// backfill's changes beside the running app.
pub fn boot_with_attention(
    roots: &Roots,
    initial: PrinterStatus,
    timings: JobTimings,
    attention: AttentionBoot,
) -> (Running, AppliedChanges) {
    let (running, backfilled) = boot_inner(
        roots,
        Driver::Started,
        initial,
        timings,
        None,
        |_| {},
        Some(attention),
    );
    (running, backfilled.expect("the backfill ran"))
}

fn boot_inner(
    roots: &Roots,
    driver: Driver,
    initial: PrinterStatus,
    timings: JobTimings,
    job_clock: Option<Arc<dyn Clock>>,
    prepare: impl FnOnce(&Arc<RuntimeServices<MockRuntime>>),
    attention: Option<AttentionBoot>,
) -> (Running, Option<AppliedChanges>) {
    let storage = Arc::new(Storage::open(roots.paths.clone(), &roots.lease).unwrap());
    host_ops::recover_after_restart(&storage, SystemClock.now()).unwrap();
    let recovered = farm3d_lib::jobs::recover_after_restart(&storage, SystemClock.now()).unwrap();
    // P8 D2 "Startup backfill": right after Job recovery, before any
    // command is served.
    let backfilled = attention
        .as_ref()
        .map(|_| farm3d_lib::attention::projector::backfill(&storage, SystemClock.now()).unwrap());
    // P8 D5 "Startup sweep", when captures are on: before any command is
    // served, published once the camera runtime starts.
    let swept = attention
        .as_ref()
        .and_then(|boot| boot.cameras)
        .map(|_| farm3d_lib::cameras::media::startup_sweep(&storage, SystemClock.now()));
    let factory: Arc<dyn CapabilityFactory> = Arc::new(SimFactory {
        upload_unsupported: Arc::clone(&roots.upload_unsupported),
        tier: Arc::clone(&roots.evidence_tier),
        moonraker: roots.timings.moonraker,
        writes: Arc::clone(&roots.writes),
    });
    let host_ops_timings = roots.timings.host_ops;
    let clock: Arc<dyn Clock> = Arc::new(SystemClock);
    let factory_for_manager: ConnectionFactory =
        if attention.as_ref().is_some_and(|boot| boot.real_adapters) {
            build_connection
        } else {
            unused_factory
        };
    let camera_timings = attention.as_ref().and_then(|boot| boot.cameras);
    let notification_boot = attention.as_ref().and_then(|boot| {
        boot.notifications.as_ref().map(|notifications| {
            (
                Arc::clone(&notifications.sink),
                Arc::clone(&notifications.control),
            )
        })
    });
    let attention_services = attention.as_ref().map(|boot| {
        Arc::new(AttentionServices::with_clock(
            boot.timings,
            boot.clock.clone().unwrap_or_else(|| Arc::new(SystemClock)),
        ))
    });
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
            farm3d_lib::jobs::commands::retry_job,
            farm3d_lib::jobs::commands::get_job_history,
            farm3d_lib::jobs::commands::declare_job_outcome,
            farm3d_lib::jobs::commands::settle_job_material,
            farm3d_lib::jobs::commands::correct_job_material,
            farm3d_lib::queue::commands::explain_queue_entry,
            farm3d_lib::host_ops::commands::reconcile_host_operation,
            farm3d_lib::host_ops::commands::abandon_host_operation,
            farm3d_lib::spools::commands::move_spool,
            farm3d_lib::spools::commands::spool_history,
            farm3d_lib::printers::commands::archive_printer,
            farm3d_lib::printers::commands::delete_printer,
            farm3d_lib::attention::commands::list_attention,
            farm3d_lib::attention::commands::mark_attention_read,
            farm3d_lib::attention::commands::acknowledge_attention_event,
            farm3d_lib::attention::commands::resolve_attention_event,
            farm3d_lib::incidents::commands::list_incidents,
            farm3d_lib::incidents::commands::get_incident,
            farm3d_lib::incidents::commands::add_incident_note,
            farm3d_lib::cameras::commands::set_printer_camera,
            farm3d_lib::cameras::commands::capture_snapshot,
            farm3d_lib::cameras::commands::list_snapshots,
            farm3d_lib::cameras::commands::snapshot_image,
            farm3d_lib::cameras::commands::set_snapshot_pinned,
            farm3d_lib::cameras::commands::media_usage,
            farm3d_lib::connections::commands::printer_statuses,
            farm3d_lib::cameras::commands::get_printer_camera,
            farm3d_lib::printers::alerts::get_printer_alert_defaults,
            farm3d_lib::printers::alerts::set_printer_alert_defaults,
            farm3d_lib::settings::commands::load_settings,
            farm3d_lib::settings::commands::save_settings,
        ],
        Arc::clone(&storage),
        Arc::new(common::a_catalog()),
        roots.credentials.path().to_path_buf(),
        factory_for_manager,
        move |services| {
            services.host_ops = Arc::new(HostOperationServices::new(
                Arc::clone(&services.storage),
                Arc::clone(&services.library.content),
                Arc::clone(&services.manager),
                factory,
                clock,
                host_ops_timings,
            ));
            services.jobs = Arc::new(match job_clock {
                Some(clock) => JobServices::with_clock(timings, clock),
                None => JobServices::new(timings),
            });
            if let Some(attention) = attention_services {
                services.attention = attention;
            }
            if let Some(timings) = camera_timings {
                services.cameras =
                    Arc::new(farm3d_lib::cameras::services::CameraServices::new(timings));
            }
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
    if let Some(changes) = &backfilled {
        services.attention.set_backfilled(changes.clone());
        if let Some(swept) = swept {
            services.cameras.apply_startup_sweep(swept);
            // Before the projector, so its first pass's captures are heard.
            farm3d_lib::start_camera_runtime(&services, app.handle());
        }
        if let Some((sink, control)) = notification_boot {
            // Before `start`, so neither the platform sink nor the main
            // window is ever reached; before the projector, so its first
            // live pass is heard.
            services.notifications.set_sink(sink);
            services.notifications.set_window_control(control);
            farm3d_lib::start_notification_runtime(&services, app.handle());
        }
        farm3d_lib::start_attention_runtime(&services, app.handle());
    }
    (
        Running {
            app,
            webview,
            manager,
            services,
            storage,
            events,
            wait: roots.timings.wait,
        },
        backfilled,
    )
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
    status_from_host(&print_state, filename, progress)
}

/// A Moonraker host's `print_stats.state`, file, and progress as a live
/// status: Online and fresh.
pub fn status_from_host(print_state: &str, filename: String, progress: f64) -> PrinterStatus {
    let mut status = status_of(match print_state {
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

    pub fn job_command(
        &self,
        command: &str,
        operation_id: &str,
        job_id: &str,
    ) -> Result<Value, Value> {
        self.call(
            command,
            json!({"operationId": operation_id, "jobId": job_id}),
        )
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
        let deadline = Instant::now() + self.wait;
        while self.services.jobs.resyncs() == 0 {
            assert!(
                Instant::now() < deadline,
                "the driver's first pass never ran"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// Stops the Job runtime, as a crash would, and waits until its tasks
    /// (the driver, and the evaluator's task and pumps) have all ended, so
    /// nothing of the stopped runtime writes after this returns.
    pub fn stop_runtime(&self) {
        self.services.jobs.stop();
        let deadline = Instant::now() + self.wait;
        while self.services.jobs.running_tasks() > 0 {
            assert!(
                Instant::now() < deadline,
                "the stopped runtime still has {} tasks",
                self.services.jobs.running_tasks()
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Waits until the driver has handled everything sent to it so far
    /// (see `JobServices::barrier`).
    fn driver_barrier(&self) {
        let mut done = self.services.jobs.barrier();
        let deadline = Instant::now() + self.wait;
        loop {
            match done.try_recv() {
                Ok(()) => return,
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("the driver stopped before the barrier")
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
            assert!(
                Instant::now() < deadline,
                "the driver never reached the barrier"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// P8: waits until the Attention projector has finished a whole pass
    /// that started after this call (`AttentionServices::pass_barrier`).
    pub fn attention_pass(&self) {
        let mut done = self.services.attention.pass_barrier();
        let deadline = Instant::now() + self.wait;
        loop {
            match done.try_recv() {
                Ok(()) => return,
                Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
                    panic!("the projector stopped before the barrier")
                }
                Err(tokio::sync::oneshot::error::TryRecvError::Empty) => {}
            }
            assert!(
                Instant::now() < deadline,
                "the projector never finished a pass"
            );
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// P8: the stored `attention_events` rows as `list_attention`'s
    /// decoder reads them, open ones and resolved ones alike, oldest first.
    pub fn attention_rows(&self) -> Vec<farm3d_lib::attention::AttentionEvent> {
        self.storage
            .read(|connection| {
                let mut statement = connection
                    .prepare("SELECT id FROM attention_events ORDER BY first_observed_at, rowid")?;
                let ids = statement
                    .query_map([], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(ids)
            })
            .unwrap()
            .iter()
            .map(|id| {
                self.storage
                    .read(|connection| {
                        Ok(farm3d_lib::attention::repository::load_event(
                            connection, id,
                        ))
                    })
                    .unwrap()
                    .unwrap()
                    .expect("row exists")
            })
            .collect()
    }

    /// P8: the captured `attention` stream events of `event_type` whose
    /// subject is `subject_id`.
    pub fn attention_stream(&self, event_type: &str, subject_id: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str::<Value>(text).unwrap())
            .filter(|event| event["type"] == event_type && event["subject"]["id"] == subject_id)
            .collect()
    }

    /// Waits until everything already set in motion has run: the driver has
    /// handled every change sent to it, no Host Operation is still being
    /// dispatched or reconciled, and the evaluator is idle. A negative
    /// check after this ("no second upload") can't be beaten by work still
    /// on its way. With the driver off, only the Host Operations are waited
    /// for.
    pub fn quiesce(&self) {
        let running = self.services.jobs.running_tasks() > 0;
        if running {
            self.driver_barrier();
        }
        self.wait_until("no Host Operation in flight", || {
            self.scalar(
                "SELECT COUNT(*) FROM host_operations WHERE state IN ('dispatching', 'reconciling')",
            ) == 0
        });
        if running {
            // An op resolving wakes the driver again, and whatever it did
            // may have poked the evaluator.
            self.driver_barrier();
            self.wait_until("the evaluator is idle", || {
                self.services.evaluator.is_idle()
            });
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
        let deadline = Instant::now() + self.wait;
        while self.services.jobs.resyncs() < target {
            assert!(
                Instant::now() < deadline,
                "the driver never ran {count} more passes"
            );
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
        let deadline = Instant::now() + self.wait;
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
                    connection
                        .query_row(
                            "SELECT id FROM spools WHERE slot_id = ?1",
                            [&slot_id],
                            |row| row.get(0),
                        )
                        .optional()?,
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

    /// Waits until the Job's stored state is `state`. To prove a wake (not
    /// the driver's poll) moved the Job, boot with [`no_poll`] and check
    /// `resyncs()` afterwards: a deadline shorter than the poll proves
    /// nothing once a slow machine can reach the poll first.
    pub fn wait_job(&self, job_id: &str, state: &str) -> Value {
        let deadline = Instant::now() + self.wait;
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
        let deadline = Instant::now() + self.wait;
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
        let deadline = Instant::now() + self.wait;
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
        let deadline = Instant::now() + self.wait;
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
            .filter(|event| {
                event["type"] == "queue.job.changed" && event["subject"]["id"] == job_id
            })
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
        self.scalar(&format!(
            "SELECT current_mg FROM spools WHERE id = '{spool_id}'"
        ))
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
