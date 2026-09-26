//! The production Moonraker adapter against the Moonraker simulator: real
//! Klipper on simulavr behind real Moonraker, with a fault proxy in between
//! (ADR-0012, `sim/compose.yaml`).
//!
//! Every test is `#[ignore]`d and skips cleanly when the simulator is not
//! running. Run them with `just sim-up && just test-sim`.
//!
//! These tests write to the simulator (G-code, M112, Klipper restarts,
//! network faults). The harness refuses any non-loopback target, so they
//! can never reach a real printer.

// `sim::exclusive()` is a std mutex held for a whole test on purpose: it
// serializes tests that run on different threads, each on its own runtime,
// so holding it across an await cannot deadlock.
#![allow(clippy::await_holding_lock)]

#[macro_use]
mod sim;

use std::time::Duration;

use farm3d_lib::connections::moonraker::MoonrakerConnection;
use farm3d_lib::connections::{
    ConnectionError, ConnectionObservation, ConnectionState, PrinterConnection, MOONRAKER_KIND,
};
use sim::moonraker::{Mode, MoonrakerSim, Variant};
use sim::toxiproxy::Proxy;
use tokio::sync::mpsc;

/// A running `subscribe()` and the observations it has sent.
struct Stream {
    rx: mpsc::Receiver<ConnectionObservation>,
    task: tokio::task::JoinHandle<Result<(), ConnectionError>>,
}

impl Stream {
    fn open(sim: &MoonrakerSim) -> Self {
        let (tx, rx) = mpsc::channel(256);
        let connection = sim.connection();
        let task = tokio::spawn(async move { connection.subscribe(tx).await });
        Self { rx, task }
    }

    /// Waits for an observation that satisfies `matches`, discarding others.
    async fn wait_for(
        &mut self,
        what: &str,
        timeout: Duration,
        mut matches: impl FnMut(&ConnectionObservation) -> bool,
    ) -> ConnectionObservation {
        let found = tokio::time::timeout(timeout, async {
            while let Some(observation) = self.rx.recv().await {
                if matches(&observation) {
                    return Some(observation);
                }
            }
            None
        })
        .await;
        match found {
            Ok(Some(observation)) => observation,
            Ok(None) => panic!("the stream ended before {what}"),
            Err(_) => panic!("no {what} within {timeout:?}"),
        }
    }

    async fn first_telemetry(&mut self) {
        self.wait_for("first telemetry", Duration::from_secs(15), |o| {
            matches!(o, ConnectionObservation::Telemetry(_))
        })
        .await;
    }

    /// Waits for `subscribe()` itself to return.
    async fn ended(self, timeout: Duration) -> Result<(), ConnectionError> {
        tokio::time::timeout(timeout, self.task)
            .await
            .unwrap_or_else(|_| panic!("subscribe() was still running after {timeout:?}"))
            .expect("the subscribe task panicked")
    }
}

fn health(observation: &ConnectionObservation) -> Option<ConnectionState> {
    match observation {
        ConnectionObservation::Health { state, .. } => Some(*state),
        ConnectionObservation::Telemetry(_) => None,
    }
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn probe_reports_the_simulated_klipper() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let probe = sim.connection().probe().await.expect("probe the simulator");

    assert_eq!(probe.kind, MOONRAKER_KIND);
    assert_eq!(probe.state, "ready");
    assert!(probe.host_software.starts_with("v0.11"), "{probe:?}");
    // The pinned Klipper build (sim/simctl: klipper_commit).
    assert!(
        probe.firmware.starts_with("v0.13.0-770-gce7002bed"),
        "{probe:?}"
    );
    // sim/moonraker/printer.cfg: X and Y run -0.25..200, Z runs 0.1..200.
    assert_eq!(probe.reported.bed_width_mm, Some(200.25), "{probe:?}");
    assert_eq!(probe.reported.bed_depth_mm, Some(200.25), "{probe:?}");
    assert_eq!(probe.reported.printable_height_mm, Some(199.9), "{probe:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn the_four_toolhead_simulator_probes_and_streams() {
    let sim = require_sim!(MoonrakerSim::discover_variant(Variant::MultiTool));
    let _guard = sim::exclusive();
    sim.reset();

    let objects = sim.objects();
    for extruder in ["extruder", "extruder1", "extruder2", "extruder3"] {
        assert!(
            objects.iter().any(|o| o == extruder),
            "{extruder} missing from {objects:?}"
        );
    }
    // Each toolhead's heater is independently controllable.
    sim.gcode("SET_HEATER_TEMPERATURE HEATER=extruder2 TARGET=1");
    let status = sim.query("extruder&extruder2");
    assert_eq!(status["result"]["status"]["extruder2"]["target"], 1.0);
    assert_eq!(status["result"]["status"]["extruder"]["target"], 0.0);

    let probe = sim
        .connection()
        .probe()
        .await
        .expect("probe the four-toolhead simulator");
    assert_eq!(probe.state, "ready");
    let mut stream = Stream::open(&sim);
    stream.first_telemetry().await;
    // Per-toolhead telemetry is #9's multi-extruder work. When it lands,
    // assert here that extruder1..3 reach the adapter's telemetry.

    stream.task.abort();
    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn subscribe_streams_telemetry_and_follows_a_target_change() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let mut stream = Stream::open(&sim);
    stream.first_telemetry().await;
    stream
        .wait_for("an online health report", Duration::from_secs(15), |o| {
            health(o) == Some(ConnectionState::Online)
        })
        .await;

    // A one-field change arrives as a partial update and must not blank the
    // readings it omits.
    sim.gcode("SET_HEATER_TEMPERATURE HEATER=heater_bed TARGET=1");
    let observation = stream
        .wait_for(
            "bed target 1 in telemetry",
            Duration::from_secs(15),
            |o| matches!(o, ConnectionObservation::Telemetry(t) if t.bed_target_c == Some(1.0)),
        )
        .await;
    let ConnectionObservation::Telemetry(telemetry) = observation else {
        unreachable!()
    };
    assert!(telemetry.nozzle_temp_c.is_some(), "{telemetry:?}");
    assert!(telemetry.bed_temp_c.is_some(), "{telemetry:?}");
    assert_eq!(telemetry.nozzle_target_c, Some(0.0), "{telemetry:?}");

    stream.task.abort();
    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_klipper_shutdown_turns_the_stream_offline() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let mut stream = Stream::open(&sim);
    stream.first_telemetry().await;

    sim.emergency_stop();
    stream
        .wait_for("an offline health report", Duration::from_secs(20), |o| {
            health(o) == Some(ConnectionState::Offline)
        })
        .await;
    // The socket to Moonraker is still healthy; only Klipper is down.
    assert!(
        !stream.task.is_finished(),
        "subscribe() ended on a Klipper shutdown"
    );

    stream.task.abort();
    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_host_that_goes_away_ends_the_stream_and_fails_the_probe() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let mut stream = Stream::open(&sim);
    stream.first_telemetry().await;

    sim.faults.set_enabled(Proxy::Moonraker, false);
    // The adapter never retries; returning at all is the supervisor's cue
    // to reconnect with backoff.
    let _ = stream.ended(Duration::from_secs(15)).await;
    let probe = sim.connection().probe().await;
    assert!(
        matches!(probe, Err(ConnectionError::Unreachable(_))),
        "probe against a host that went away: {probe:?}"
    );

    sim.faults.set_enabled(Proxy::Moonraker, true);
    sim.connection()
        .probe()
        .await
        .expect("probe once the host is back");
    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_slow_host_still_probes() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    // Well inside the adapter's 10 s timeouts. (Latency past a timeout is
    // covered by the silent-host test: Toxiproxy cannot drop a latency toxic
    // quickly while it is still holding data back.)
    sim.faults.slow(Proxy::Moonraker, 1_500);
    let started = std::time::Instant::now();
    sim.connection()
        .probe()
        .await
        .expect("a slow but responsive host probes successfully");
    assert!(
        started.elapsed() >= Duration::from_millis(1_500),
        "{:?}",
        started.elapsed()
    );

    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_silent_host_times_out_the_probe() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    // The connection opens but nothing comes back, like a pulled cable.
    sim.faults.hang(Proxy::Moonraker);
    let started = std::time::Instant::now();
    let probe = sim.connection().probe().await;
    assert_eq!(probe, Err(ConnectionError::Timeout));
    assert!(
        started.elapsed() < Duration::from_secs(20),
        "{:?}",
        started.elapsed()
    );

    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_response_cut_mid_stream_fails_the_probe_without_hanging() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    // Enough for the WebSocket upgrade, not for the probe's three answers.
    sim.faults.cut_after(Proxy::Moonraker, 400);
    let started = std::time::Instant::now();
    let probe = sim.connection().probe().await;
    assert!(
        matches!(
            probe,
            Err(ConnectionError::Unreachable(_) | ConnectionError::Protocol(_))
        ),
        "probe with the response cut: {probe:?}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );

    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_no_bed_printer_reports_the_bed_as_absent_not_zero() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    sim.set_mode(Mode::NoBed);

    let objects = sim.objects();
    assert!(
        !objects.iter().any(|o| o == "heater_bed"),
        "heater_bed should be gone from {objects:?}"
    );

    let mut stream = Stream::open(&sim);
    let observation = stream
        .wait_for(
            "telemetry from the no-bed printer",
            Duration::from_secs(15),
            |o| matches!(o, ConnectionObservation::Telemetry(_)),
        )
        .await;
    let ConnectionObservation::Telemetry(telemetry) = observation else {
        unreachable!()
    };
    // Absent, not a zeroed reading: a real bed at 0 deg C would also show
    // `Some(0.0)`, so only `None` proves Moonraker never reported the object.
    assert_eq!(telemetry.bed_temp_c, None, "{telemetry:?}");
    assert_eq!(telemetry.bed_target_c, None, "{telemetry:?}");
    assert!(telemetry.nozzle_temp_c.is_some(), "{telemetry:?}");

    stream.task.abort();
    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn apikey_mode_refuses_no_key_and_accepts_the_right_one() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    sim.set_mode(Mode::ApiKey);

    let no_key = MoonrakerConnection::new(sim.config(), None);
    let probe = no_key.probe().await;
    assert!(
        matches!(probe, Err(ConnectionError::Auth(_))),
        "probe with no key in apikey mode: {probe:?}"
    );

    let probe = sim
        .connection()
        .probe()
        .await
        .expect("probe with the simulator's own API key");
    assert_eq!(probe.state, "ready");

    sim.reset();
}

// ===========================================================================
// P6 (Task 12): Host Operations against the simulator.
//
// Each test drives `host_ops` through a `RuntimeServices` whose Printer
// points at `sim.config()` (the fault proxy), with the production Moonraker
// capability adapter. The Printer's live status is fed by hand from what the
// simulator really reports (`observe_host`), so the supervisor's Online hook
// runs once, at a known moment, instead of racing the test's own steps.
//
// Every start first passes `assert_safe_to_start`: no heater has a target,
// and the host is not printing or paused. The staged G-code is comments and
// `M117` only: no motion, no heating, no tool change.
//
// Faults are armed on the adapter's write itself (`Armed`): the toxic goes
// in just before the upload or start request and comes out as soon as the
// client has its answer, so the reads the commands make first (the start's
// host re-read, history mark, and identity check) and the reconciliation
// reads after it are never cut.
// ===========================================================================

mod common;

use std::sync::{Arc, Mutex};
use std::time::Instant;

use farm3d_lib::connections::capabilities::{
    capabilities_for, ArtifactStaging, CapabilityEvidence, CapabilityMap, CapabilityState,
    CommandFailure, EvidenceTier, HistoryQuery, HostFacts, HostOperationFailureCode,
    HostStateQuery, InconclusiveReason, LocateOutcome, PrintControl, PrinterCapabilities,
    StagedArtifact, UnsupportedReason,
};
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::status_repository::{PrinterTelemetry, ToolTemperature};
use farm3d_lib::connections::supervisor::PrinterSetupFacts;
use farm3d_lib::connections::ConnectionConfig;
use farm3d_lib::host_ops::repository as host_ops_repo;
use farm3d_lib::host_ops::{
    self, CapabilityFactory, Clock, HostOperation, HostOperationResolution, HostOperationServices,
    HostOperationState, HostOpsTimings, StartEvidenceSource, SystemClock,
};
use farm3d_lib::library::content::ContentStore;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::operational::HostActivity;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sim::toxiproxy::Toxiproxy;
use tauri::test::MockRuntime;

const P6_PRINTER: &str = "printer-sim";
const P6_NOW: &str = "2026-01-01T00:00:00Z";
/// How long any one host operation may take to settle in these tests.
const P6_WAIT: Duration = Duration::from_secs(90);

// --- fixtures -----------------------------------------------------------------

use sim::moonraker::no_motion_gcode;

/// A couple of seconds on the simulator: long enough to leave a history job.
fn quick_gcode(tag: &str) -> Vec<u8> {
    no_motion_gcode(tag, 8)
}

/// About 10 s on the simulator: long enough to pause, resume, and cancel.
fn long_gcode(tag: &str) -> Vec<u8> {
    no_motion_gcode(tag, 120)
}

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

// --- the simulator's safety precondition ------------------------------------------

/// Refuses to start a print unless every heater's target is 0 and the host
/// is neither printing nor paused. Returns the `priorState` the host's state
/// calls for: `ready` from standby, `finished` from complete, `cancelled`
/// from cancelled.
fn assert_safe_to_start(sim: &MoonrakerSim) -> (&'static str, HostActivity) {
    let targets = sim.heater_targets();
    assert!(
        targets
            .iter()
            .any(|(heater, _)| heater.starts_with("extruder")),
        "no extruder among the heaters: {targets:?}"
    );
    for (heater, target) in &targets {
        assert!(
            *target == 0.0,
            "refusing to start: {heater} has target {target}"
        );
    }
    let (state, _) = sim.print_stats();
    match state.as_str() {
        "standby" => ("ready", HostActivity::Idle),
        "complete" => ("finished", HostActivity::Finished),
        "cancelled" => ("cancelled", HostActivity::Cancelled),
        other => panic!("refusing to start: the host is {other}"),
    }
}

/// The live status the simulator's `print_stats` calls for.
fn host_activity(sim: &MoonrakerSim) -> HostActivity {
    match sim.print_stats().0.as_str() {
        "standby" => HostActivity::Idle,
        "printing" => HostActivity::Printing,
        "paused" => HostActivity::Paused,
        "complete" => HostActivity::Finished,
        "cancelled" => HostActivity::Cancelled,
        "error" => HostActivity::Failed,
        _ => HostActivity::Unknown,
    }
}

// --- armed faults ----------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Write {
    Upload,
    Start,
}

type Hook = Box<dyn FnOnce() + Send>;

/// A fault applied around exactly one write: `before` right before the
/// request, `after` as soon as the client has its answer.
struct Armed {
    write: Write,
    before: Hook,
    after: Hook,
}

#[derive(Clone, Default)]
struct Trigger(Arc<Mutex<Option<Armed>>>);

impl Trigger {
    fn arm(&self, write: Write, before: impl FnOnce() + Send + 'static) {
        self.arm_with(write, before, || Toxiproxy::discover().unwrap().reset());
    }

    fn arm_with(
        &self,
        write: Write,
        before: impl FnOnce() + Send + 'static,
        after: impl FnOnce() + Send + 'static,
    ) {
        *self.0.lock().unwrap() = Some(Armed {
            write,
            before: Box::new(before),
            after: Box::new(after),
        });
    }

    fn take(&self, write: Write) -> Option<(Hook, Hook)> {
        let mut armed = self.0.lock().unwrap();
        if armed.as_ref().is_some_and(|armed| armed.write == write) {
            armed.take().map(|armed| (armed.before, armed.after))
        } else {
            None
        }
    }

    fn is_armed(&self) -> bool {
        self.0.lock().unwrap().is_some()
    }
}

async fn run_blocking(hook: Hook) {
    tokio::task::spawn_blocking(hook)
        .await
        .expect("a fault hook panicked");
}

/// The production adapter, with the armed fault (if any) around its write.
struct ArmedAdapter {
    inner: MoonrakerCapabilities,
    trigger: Trigger,
}

#[async_trait::async_trait]
impl ArtifactStaging for ArmedAdapter {
    async fn upload(
        &self,
        artifact: &StagedArtifact,
        body: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    ) -> Result<(), CommandFailure> {
        let Some((before, after)) = self.trigger.take(Write::Upload) else {
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

#[async_trait::async_trait]
impl PrintControl for ArmedAdapter {
    async fn start(&self, host_path: &str) -> Result<(), CommandFailure> {
        let Some((before, after)) = self.trigger.take(Write::Start) else {
            return self.inner.start(host_path).await;
        };
        run_blocking(before).await;
        let result = self.inner.start(host_path).await;
        run_blocking(after).await;
        result
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

/// The production Moonraker adapter with test timings, plus [`Trigger`].
/// Every Moonraker capability the registry calls "not verified" is treated
/// as supported here, so these tests never depend on the evidence rows they
/// produce.
struct SimFactory {
    timings: Mutex<MoonrakerTimings>,
    trigger: Trigger,
}

impl SimFactory {
    fn new() -> Self {
        Self {
            timings: Mutex::new(MoonrakerTimings {
                connect: Duration::from_secs(2),
                query: Duration::from_secs(10),
                control: Duration::from_secs(20),
                transfer_base: Duration::from_secs(30),
                transfer_per_started_mib: Duration::from_millis(250),
                ..MoonrakerTimings::default()
            }),
            trigger: Trigger::default(),
        }
    }

    fn set_control_timeout(&self, timeout: Duration) {
        self.timings.lock().unwrap().control = timeout;
    }

    fn adapter(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> ArmedAdapter {
        ArmedAdapter {
            inner: MoonrakerCapabilities::new(config, key, *self.timings.lock().unwrap()),
            trigger: self.trigger.clone(),
        }
    }
}

impl CapabilityFactory for SimFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        let mut capabilities = capabilities_for(printer, host_facts);
        let current = capabilities.capabilities.clone();
        capabilities.capabilities = CapabilityMap::complete(|key| match &current[key] {
            CapabilityState::Unsupported {
                reason: UnsupportedReason::NotVerified,
                ..
            } => CapabilityState::Supported {
                evidence: CapabilityEvidence {
                    source: "tests/sim_moonraker.rs".to_string(),
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
        Some(Box::new(self.adapter(config, key)))
    }

    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        Some(Box::new(self.adapter(config, key)))
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        Some(Box::new(MoonrakerCapabilities::new(
            config,
            key,
            *self.timings.lock().unwrap(),
        )))
    }
}

/// Short settle period, a long verification window (simulavr is slow), and
/// automatic retries an hour out unless a test asks for them.
fn sim_timings() -> HostOpsTimings {
    HostOpsTimings {
        settle_period: Duration::from_secs(3),
        verify_window: Duration::from_secs(15),
        verify_poll_interval: Duration::from_millis(250),
        backoff: |_| Duration::from_secs(3600),
        ..HostOpsTimings::default()
    }
}

// --- the rig ----------------------------------------------------------------------

struct SimRig {
    _roots: tempfile::TempDir,
    paths: StoragePaths,
    lease: MetadataRootLease,
    credentials: tempfile::TempDir,
    factory: Arc<SimFactory>,
    timings: HostOpsTimings,
}

struct SimRunning {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    manager: Arc<farm3d_lib::connections::supervisor::ConnectionManager<MockRuntime>>,
    services: Arc<farm3d_lib::RuntimeServices<MockRuntime>>,
    storage: Arc<Storage>,
}

fn no_observation_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

impl SimRig {
    fn new(sim: &MoonrakerSim, timings: HostOpsTimings) -> Self {
        let roots = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(roots.path().join("metadata"), roots.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let storage = Storage::open(paths.clone(), &lease).unwrap();
        PrinterRepository::new(Arc::new(storage))
            .create(StoredPrinter {
                connection: Some(sim.config()),
                ..common::a_stored_printer(P6_PRINTER)
            })
            .unwrap();
        Self {
            _roots: roots,
            paths,
            lease,
            credentials: tempfile::tempdir().unwrap(),
            factory: Arc::new(SimFactory::new()),
            timings,
        }
    }

    /// Seeds a Slice Revision with a fresh id whose G-code is `bytes`, and
    /// returns its id. Its staged path is `farm3d/<id>.gcode`.
    fn slice_revision(&self, bytes: &[u8]) -> String {
        let id = farm3d_lib::library::new_id("slr");
        let storage = Storage::open(self.paths.clone(), &self.lease).unwrap();
        seed_slice_revision(&storage, &id, bytes);
        id
    }

    /// Opens the roots and boots an app, running startup recovery first as
    /// `build_runtime_services` does. Every boot after the first is a
    /// restart.
    fn boot(&self) -> SimRunning {
        let storage = Arc::new(Storage::open(self.paths.clone(), &self.lease).unwrap());
        host_ops::recover_after_restart(&storage, SystemClock.now()).unwrap();
        let factory: Arc<dyn CapabilityFactory> = self.factory.clone();
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
            ],
            Arc::clone(&storage),
            Arc::new(common::a_catalog()),
            self.credentials.path().to_path_buf(),
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
        SimRunning {
            _app: app,
            webview,
            manager,
            services,
            storage,
        }
    }
}

/// Seeds an external Slice Revision whose G-code blob is in the content
/// store, the way a real import leaves it.
fn seed_slice_revision(storage: &Storage, id: &str, bytes: &[u8]) {
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
                   VALUES ('mdl-{id}', 1, 'Model', 'gcode', 'managed', '{P6_NOW}', '{P6_NOW}');
                 INSERT INTO model_source_revisions(
                   id, model_id, sequence, content_sha256, size_bytes, format, origin,
                   source_file_name, source_path, captured_at, inspector_version, inspection_json
                 ) VALUES ('msr-{id}', 'mdl-{id}', 1, '{sha256}', {size}, 'gcode', 'import',
                           'part.gcode', '/src/part.gcode', '{P6_NOW}', 1, '{{}}');
                 INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
                   gcode_size, target_json, facts_json, requires_manual_printer_selection,
                   estimates_json, created_at)
                 VALUES ('{id}', 'external', 'mdl-{id}', 'msr-{id}', '{sha256}', {size}, '{{}}',
                         '{{}}', 1, '{{}}', '{P6_NOW}');"
            ))
            .map_err(RepositoryError::from)
        })
        .unwrap();
}

fn host_path_of(slr: &str) -> String {
    format!("farm3d/{slr}.gcode")
}

impl SimRunning {
    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    fn stage(&self, operation_id: &str, slr: &str) -> String {
        let row = self
            .call(
                "stage_slice_revision",
                json!({"operationId": operation_id, "printerId": P6_PRINTER, "sliceRevisionId": slr}),
            )
            .unwrap_or_else(|error| panic!("stage: {error}"));
        id_of(&row)
    }

    fn try_start(&self, operation_id: &str, upload: &str, prior: &str) -> Result<Value, Value> {
        self.call(
            "start_staged_artifact",
            json!({
                "operationId": operation_id, "printerId": P6_PRINTER,
                "hostOperationId": upload, "priorState": prior,
            }),
        )
    }

    /// Checks the simulator is safe to start, brings the live status in
    /// line with it, and starts the staged upload with the matching
    /// `priorState`.
    fn start(&self, sim: &MoonrakerSim, operation_id: &str, upload: &str) -> String {
        let (prior, activity) = assert_safe_to_start(sim);
        self.observe(activity);
        let row = self
            .try_start(operation_id, upload, prior)
            .unwrap_or_else(|error| panic!("start from {prior}: {error}"));
        id_of(&row)
    }

    fn control(&self, verb: &str, operation_id: &str) -> String {
        let row = self
            .call(
                &format!("{verb}_host_print"),
                json!({"operationId": operation_id, "printerId": P6_PRINTER}),
            )
            .unwrap_or_else(|error| panic!("{verb}: {error}"));
        id_of(&row)
    }

    fn reconcile(&self, id: &str) -> HostOperation {
        let row = self
            .call("reconcile_host_operation", json!({"hostOperationId": id}))
            .unwrap_or_else(|error| panic!("reconcile: {error}"));
        serde_json::from_value(row).unwrap()
    }

    fn abandon(&self, operation_id: &str, id: &str) -> Result<Value, Value> {
        self.call(
            "abandon_host_operation",
            json!({
                "operationId": operation_id, "hostOperationId": id,
                "acknowledgement": "hostStateUnknown",
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

    fn wait_for(
        &self,
        id: &str,
        what: &str,
        done: impl Fn(&HostOperation) -> bool,
    ) -> HostOperation {
        let deadline = Instant::now() + P6_WAIT;
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

    /// A live, fresh telemetry frame with `activity`. The first one brings
    /// the Printer Online, whose hook reads the host facts from the
    /// simulator and then reconciles; this waits for the facts so that
    /// hook's attempt cannot land in the middle of the test's own steps.
    fn observe(&self, activity: HostActivity) {
        self.manager.apply_observation(
            P6_PRINTER,
            ConnectionObservation::Telemetry(p6_telemetry(activity)),
            PrinterSetupFacts::complete(),
        );
        let printer = PrinterRepository::new(Arc::clone(&self.storage))
            .get(P6_PRINTER)
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
                "the Online hook read no host facts from the simulator"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        std::thread::sleep(Duration::from_millis(100));
    }

    /// [`SimRunning::observe`] with whatever the simulator reports now.
    fn observe_host(&self, sim: &MoonrakerSim) {
        self.observe(host_activity(sim));
    }

    /// Stages `slr` cleanly and waits for it to be verified.
    fn staged(&self, sim: &MoonrakerSim, operation_id: &str, slr: &str) -> String {
        self.observe_host(sim);
        let id = self.stage(operation_id, slr);
        let row = self.wait_settled(&id);
        assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
        id
    }
}

fn p6_telemetry(activity: HostActivity) -> PrinterTelemetry {
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

fn id_of(row: &Value) -> String {
    row["id"].as_str().expect("a row id").to_string()
}

fn reason(row: &HostOperation) -> Option<InconclusiveReason> {
    row.last_attempt.as_ref().map(|attempt| attempt.reason)
}

fn failure_code(row: &HostOperation) -> Option<HostOperationFailureCode> {
    row.failure.as_ref().map(|failure| failure.code)
}

fn wait_for_print_state(sim: &MoonrakerSim, states: &[&str]) -> String {
    sim::wait_until(&format!("print_stats in {states:?}"), P6_WAIT, || {
        let (state, _) = sim.print_stats();
        states.contains(&state.as_str()).then_some(state)
    })
    .unwrap_or_else(|error| panic!("{error}"))
}

/// The simulator's Moonraker, straight from `server.info`, for the log.
fn moonraker_version(sim: &MoonrakerSim) -> String {
    let facts = block_on(sim_capabilities(sim, None).host_facts()).expect("host facts");
    format!("{} API {}", facts.host_software, facts.api_version)
}

fn sim_capabilities(sim: &MoonrakerSim, key: Option<String>) -> MoonrakerCapabilities {
    MoonrakerCapabilities::new(
        &sim.config(),
        key.map(zeroize::Zeroizing::new),
        MoonrakerTimings::default(),
    )
}

fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
    tauri::async_runtime::block_on(future)
}

// --- D5: `since` and `order` on /server/history/list ---------------------------------

/// Spec D5 "Confirming the parameters": Moonraker v0.11.0 must honour
/// `order=desc` and `since` on `/server/history/list`. farm3d re-checks
/// every job anyway, but a page that ignored `order` could leave the newest
/// jobs out of a 50-job page.
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_history_list_honours_order_desc_and_since() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let adapter = sim_capabilities(&sim, None);

    // Three new jobs, each a separate file so each leaves its own job.
    for index in 0..3 {
        assert_safe_to_start(&sim);
        let bytes = quick_gcode(&format!("history-{index}"));
        let artifact = StagedArtifact {
            host_path: host_path_of(&farm3d_lib::library::new_id("slr")),
            sha256: sha256_hex(&bytes),
            size: bytes.len() as u64,
        };
        block_on(adapter.upload(&artifact, Box::new(std::io::Cursor::new(bytes))))
            .expect("upload a history fixture");
        block_on(adapter.start(&artifact.host_path)).expect("start a history fixture");
        wait_for_print_state(&sim, &["complete"]);
    }

    let ids = |value: &Value| -> Vec<u64> {
        value["result"]["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|job| u64::from_str_radix(job["job_id"].as_str().unwrap(), 16).unwrap())
            .collect()
    };
    let starts = |value: &Value| -> Vec<f64> {
        value["result"]["jobs"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|job| job["start_time"].as_f64().unwrap())
            .collect()
    };
    let desc = sim.history("limit=50&order=desc");
    let asc = sim.history("limit=50&order=asc");
    let desc_ids = ids(&desc);
    let asc_ids = ids(&asc);
    assert!(desc_ids.len() >= 3, "{desc}");
    assert!(
        desc_ids.windows(2).all(|pair| pair[0] > pair[1]),
        "order=desc is not newest first: {desc_ids:?}"
    );
    assert!(
        asc_ids.windows(2).all(|pair| pair[0] < pair[1]),
        "order=asc is not oldest first: {asc_ids:?}"
    );
    // A short page holds the newest jobs, not the oldest.
    let page = ids(&sim.history("limit=2&order=desc"));
    assert_eq!(page, desc_ids[..2].to_vec());

    // `since` drops every job that started before it.
    let second_newest_start = starts(&desc)[1];
    let since = second_newest_start - 0.001;
    let filtered = sim.history(&format!("limit=50&order=desc&since={since}"));
    assert_eq!(ids(&filtered), desc_ids[..2].to_vec(), "{filtered}");
    assert!(starts(&filtered).iter().all(|start| *start >= since));

    // The adapter's own query (D10) sends both and gets the same jobs back.
    let jobs = block_on(adapter.job_history(HistoryQuery {
        since_epoch_s: Some(since),
        limit: 50,
    }))
    .expect("job_history");
    assert_eq!(
        jobs.iter().map(|job| job.job_id).collect::<Vec<_>>(),
        desc_ids[..2].to_vec()
    );
    eprintln!(
        "P6 D5: {} honours order=desc (newest {:06X} first) and since",
        moonraker_version(&sim),
        desc_ids[0]
    );
    sim.reset();
}

// --- uploads ------------------------------------------------------------------------

/// Stage with the response lost after the host stored the file
/// (`cut_after(Moonraker, 0)`), then a restart: reconciliation proves the
/// upload applied, and the host holds exactly one copy.
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_a_lost_upload_response_reconciles_after_a_restart_to_one_file() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let rig = SimRig::new(&sim, sim_timings());
    let slr = rig.slice_revision(&quick_gcode("lost-response"));
    let running = rig.boot();
    running.observe_host(&sim);

    rig.factory.trigger.arm(Write::Upload, || {
        Toxiproxy::discover()
            .unwrap()
            .cut_after(Proxy::Moonraker, 0)
    });
    let id = running.stage("op-stage", &slr);
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::ResponseLost));
    assert!(!rig.factory.trigger.is_armed());
    drop(running);

    let restarted = rig.boot();
    // The startup pass makes one attempt; `reconcile` covers the case where
    // it has not run yet.
    let row = restarted.reconcile(&id);
    let row = if row.state == HostOperationState::Succeeded {
        row
    } else {
        restarted.wait_for(&id, "the reconciled upload", |row| {
            row.state == HostOperationState::Succeeded
        })
    };
    assert_eq!(
        row.resolution,
        Some(HostOperationResolution::ArtifactVerified { reconciled: true })
    );
    assert_eq!(
        sim.gcode_files(&host_path_of(&slr)),
        vec![host_path_of(&slr)],
        "exactly one file"
    );
    sim.reset();
}

/// An upload cut mid-body (`cut_request_after`) never reaches `host_path`.
/// Reconciliation says `uploadSettling` until the settle period after
/// `uncertain_since` has passed, then `failed { notApplied }`.
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_an_upload_cut_mid_body_settles_then_fails_not_applied() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let timings = HostOpsTimings {
        settle_period: Duration::from_secs(5),
        ..sim_timings()
    };
    let rig = SimRig::new(&sim, timings);
    let bytes = quick_gcode("mid-body");
    let half = bytes.len() as u64 / 2;
    let slr = rig.slice_revision(&bytes);
    let running = rig.boot();
    running.observe_host(&sim);

    rig.factory.trigger.arm(Write::Upload, move || {
        Toxiproxy::discover()
            .unwrap()
            .cut_request_after(Proxy::Moonraker, half)
    });
    let id = running.stage("op-stage", &slr);
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    let since = chrono::DateTime::parse_from_rfc3339(row.uncertain_since.as_deref().unwrap())
        .unwrap()
        .with_timezone(&chrono::Utc);
    let settled_at = since
        + chrono::Duration::seconds(1)
        + chrono::Duration::from_std(timings.settle_period).unwrap();

    let row = running.reconcile(&id);
    assert!(
        chrono::Utc::now() < settled_at,
        "the check ran too late to test settling"
    );
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::UploadSettling));
    assert!(
        sim.gcode_files(&host_path_of(&slr)).is_empty(),
        "no partial file"
    );

    while chrono::Utc::now() < settled_at {
        std::thread::sleep(Duration::from_millis(100));
    }
    // The automatic retry scheduled for the settle deadline may be holding
    // the row in `reconciling` right now, in which case `reconcile`
    // returns it unchanged; wait for whichever attempt commits.
    running.reconcile(&id);
    let row = running.wait_for(&id, "the settled upload to fail", |row| {
        row.state == HostOperationState::Failed
    });
    assert_eq!(
        failure_code(&row),
        Some(HostOperationFailureCode::NotApplied)
    );
    assert!(sim.gcode_files(&host_path_of(&slr)).is_empty());
    sim.reset();
}

// --- starts -------------------------------------------------------------------------

/// A start whose response is cut after dispatch is `uncertain`, and
/// reconciliation proves it ran (from `print_stats`, or from a history job
/// above the high-water mark).
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_a_start_whose_response_is_cut_reconciles_to_succeeded() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let rig = SimRig::new(&sim, sim_timings());
    let slr = rig.slice_revision(&quick_gcode("start-cut"));
    let running = rig.boot();
    let upload = running.staged(&sim, "op-stage", &slr);

    rig.factory.trigger.arm(Write::Start, || {
        Toxiproxy::discover()
            .unwrap()
            .cut_after(Proxy::Moonraker, 0)
    });
    let id = running.start(&sim, "op-start", &upload);
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::ResponseLost));

    let row = running.reconcile(&id);
    assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
    match &row.resolution {
        Some(HostOperationResolution::StartObserved {
            source,
            interrupted,
            ..
        }) => {
            assert!(!interrupted);
            eprintln!("P6: the cut start was proved from {source:?}");
            assert!(matches!(
                source,
                StartEvidenceSource::PrintStats | StartEvidenceSource::History
            ));
        }
        other => panic!("expected startObserved, got {other:?}"),
    }
    wait_for_print_state(&sim, &["complete"]);
    sim.reset();
}

/// Answer 13: after a print finishes, a second start needs `priorState:
/// finished`; `ready` is refused with no row.
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_a_second_start_from_finished_needs_prior_state_finished() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let rig = SimRig::new(&sim, sim_timings());
    let slr = rig.slice_revision(&quick_gcode("second-start"));
    let running = rig.boot();
    let upload = running.staged(&sim, "op-stage", &slr);

    let first = running.start(&sim, "op-start-1", &upload);
    let row = running.wait_settled(&first);
    assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
    assert_eq!(row.resolution, Some(HostOperationResolution::StartAccepted));
    wait_for_print_state(&sim, &["complete"]);

    let (prior, activity) = assert_safe_to_start(&sim);
    assert_eq!(prior, "finished");
    running.observe(activity);
    let before = running.row_count();
    let refused = running
        .try_start("op-start-ready", &upload, "ready")
        .expect_err("start from Finished with priorState ready");
    assert_eq!(refused["code"], "START_PRECONDITION_CHANGED", "{refused}");
    assert_eq!(running.row_count(), before, "no row for a refused start");

    let second = running.start(&sim, "op-start-2", &upload);
    let row = running.wait_settled(&second);
    assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
    assert_eq!(row.resolution, Some(HostOperationResolution::StartAccepted));
    wait_for_print_state(&sim, &["complete"]);
    sim.reset();
}

/// A start queued behind a `G4` dwell, with a client timeout shorter than
/// the dwell, stays `uncertain` while it waits (no evidence yet, and never
/// "not applied"), then reconciles to `succeeded` once it runs.
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_a_queued_start_stays_uncertain_until_it_runs() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let rig = SimRig::new(&sim, sim_timings());
    let slr = rig.slice_revision(&quick_gcode("queued-start"));
    let running = rig.boot();
    let upload = running.staged(&sim, "op-stage", &slr);

    let (prior, activity) = assert_safe_to_start(&sim);
    running.observe(activity);
    rig.factory.set_control_timeout(Duration::from_secs(2));
    let dwell = sim.gcode_in_background("G4 P8000");
    std::thread::sleep(Duration::from_millis(500));
    let id = id_of(
        &running
            .try_start("op-start", &upload, prior)
            .expect("start"),
    );
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::ResponseLost));

    let row = running.reconcile(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::NoStartEvidence));

    let deadline = Instant::now() + P6_WAIT;
    let row = loop {
        let row = running.reconcile(&id);
        if row.state != HostOperationState::Uncertain {
            break row;
        }
        assert!(
            Instant::now() < deadline,
            "the queued start never showed: {row:?}"
        );
        std::thread::sleep(Duration::from_secs(1));
    };
    assert_eq!(row.state, HostOperationState::Succeeded, "{row:?}");
    assert!(
        matches!(
            row.resolution,
            Some(HostOperationResolution::StartObserved { .. })
        ),
        "{row:?}"
    );
    eprintln!(
        "P6: the queued start reconciled after {} attempts",
        row.attempts
    );
    dwell.join().unwrap();
    wait_for_print_state(&sim, &["complete"]);
    sim.reset();
}

// --- pause, resume, cancel ----------------------------------------------------------

#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_pause_resume_and_cancel_a_running_print() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let rig = SimRig::new(&sim, sim_timings());
    let slr = rig.slice_revision(&long_gcode("control"));
    let running = rig.boot();
    let upload = running.staged(&sim, "op-stage", &slr);
    let host_path = host_path_of(&slr);

    let start = running.start(&sim, "op-start", &upload);
    assert_eq!(
        running.wait_settled(&start).state,
        HostOperationState::Succeeded
    );
    wait_for_print_state(&sim, &["printing"]);

    for (verb, expect_state, observed) in [
        (
            "pause",
            "paused",
            host_ops::HostOperationObservedState::Paused,
        ),
        (
            "resume",
            "printing",
            host_ops::HostOperationObservedState::Printing,
        ),
        (
            "cancel",
            "cancelled",
            host_ops::HostOperationObservedState::Cancelled,
        ),
    ] {
        running.observe_host(&sim);
        let id = running.control(verb, &format!("op-{verb}"));
        let row = running.wait_settled(&id);
        assert_eq!(row.state, HostOperationState::Succeeded, "{verb}: {row:?}");
        assert_eq!(row.host_path, host_path, "{verb}");
        assert_eq!(
            row.resolution,
            Some(HostOperationResolution::StateObserved {
                observed_state: observed,
                reconciled: false,
            }),
            "{verb}"
        );
        let (state, filename) = sim.print_stats();
        assert_eq!(state, expect_state, "{verb}");
        assert_eq!(filename.as_deref(), Some(host_path.as_str()), "{verb}");
    }
    sim.reset();
}

// --- a host that stays away -----------------------------------------------------------

/// With the host gone for longer than the backoff, automatic retries keep
/// failing and the row stays `uncertain` (never `failed`). Abandon then ends
/// it locally, and nothing is ever uploaded twice.
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_a_host_that_stays_away_keeps_the_row_uncertain_until_abandoned() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let timings = HostOpsTimings {
        backoff: |step| Duration::from_millis(250 * 2u64.pow(step.min(2))),
        ..sim_timings()
    };
    let rig = SimRig::new(&sim, timings);
    let slr = rig.slice_revision(&quick_gcode("host-away"));
    let running = rig.boot();
    running.observe_host(&sim);

    // The response is lost, and the host is gone before the executor can
    // commit `uncertain` (and so before any retry can read it).
    rig.factory.trigger.arm_with(
        Write::Upload,
        || {
            Toxiproxy::discover()
                .unwrap()
                .cut_after(Proxy::Moonraker, 0)
        },
        || {
            let faults = Toxiproxy::discover().unwrap();
            faults.reset();
            faults.set_enabled(Proxy::Moonraker, false);
        },
    );
    let id = running.stage("op-stage", &slr);
    assert_eq!(
        running.wait_settled(&id).state,
        HostOperationState::Uncertain
    );

    // Longer than the backoff: several automatic attempts, all unreachable.
    let row = running.wait_for(&id, "three failed automatic attempts", |row| {
        row.attempts >= 3
    });
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::HostUnreachable));
    assert!(
        running.services.host_ops.retry_attempts_run() >= 3,
        "the retries ran on their own"
    );

    let abandoned = running.abandon("op-abandon", &id).expect("abandon");
    assert_eq!(abandoned["state"], "abandoned");
    let attempts = running.row(&id).attempts;
    std::thread::sleep(Duration::from_secs(2));
    assert_eq!(
        running.row(&id).attempts,
        attempts,
        "no attempt after abandon"
    );

    sim.faults.set_enabled(Proxy::Moonraker, true);
    assert_eq!(
        sim.gcode_files(&host_path_of(&slr)),
        vec![host_path_of(&slr)],
        "uploaded once, never again"
    );
    sim.reset();
}

// --- Klipper restarts -------------------------------------------------------------------

/// A Klipper restart mid-print leaves a started row `succeeded` (terminal
/// rows never change). A start still queued when Klipper restarts is
/// answered `503 Klippy Disconnected`: `uncertain`, `noLongerPending`, and
/// it stays `uncertain` (never `failed`) once Klipper is back.
#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn p6_a_klipper_restart_keeps_a_started_row_and_an_uncertain_start_no_longer_pending() {
    let sim = require_sim!(MoonrakerSim::discover());
    let _guard = sim::exclusive();
    sim.reset();
    let rig = SimRig::new(&sim, sim_timings());
    let long = rig.slice_revision(&long_gcode("restart-mid-print"));
    let quick = rig.slice_revision(&quick_gcode("restart-queued"));
    let running = rig.boot();

    // A started row stays succeeded.
    let upload = running.staged(&sim, "op-stage-long", &long);
    let started = running.start(&sim, "op-start-long", &upload);
    let before = running.wait_settled(&started);
    assert_eq!(before.state, HostOperationState::Succeeded, "{before:?}");
    wait_for_print_state(&sim, &["printing"]);
    sim.restart_klipper();
    sim.wait_ready();
    assert_eq!(
        sim.print_stats().0,
        "standby",
        "a Klipper restart ends the print"
    );
    assert_eq!(
        running.row(&started),
        before,
        "the succeeded row never changes"
    );

    // An uncertain start gets noLongerPending and stays uncertain.
    let upload = running.staged(&sim, "op-stage-quick", &quick);
    let (prior, activity) = assert_safe_to_start(&sim);
    running.observe(activity);
    rig.factory.set_control_timeout(Duration::from_secs(60));
    let _dwell = sim.gcode_in_background("G4 P20000");
    std::thread::sleep(Duration::from_millis(500));
    let id = id_of(
        &running
            .try_start("op-start-quick", &upload, prior)
            .expect("start"),
    );
    std::thread::sleep(Duration::from_millis(1500));
    assert_eq!(
        running.row(&id).state,
        HostOperationState::Dispatching,
        "the start should still be queued behind the dwell"
    );
    sim.restart_klipper();
    let row = running.wait_settled(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert!(row.no_longer_pending, "{row:?}");
    sim.wait_ready();

    let row = running.reconcile(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert!(row.no_longer_pending, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::NoStartEvidence));
    assert_eq!(
        sim.print_stats().0,
        "standby",
        "the queued start was dropped"
    );
    sim.reset();
}

// --- capability detection ----------------------------------------------------------------

fn assert_facts(facts: &HostFacts, tools: u32, bed: bool) {
    assert_eq!(facts.tool_count, tools, "{facts:?}");
    assert_eq!(facts.has_heater_bed, bed, "{facts:?}");
    assert!(facts.has_virtual_sdcard, "{facts:?}");
    assert!(facts.has_pause_resume, "{facts:?}");
    assert!(facts.has_history, "{facts:?}");
    // The simulator has no webcam (spike Gate H): the query answers, empty.
    assert_eq!(facts.camera_count, 0, "{facts:?}");
    assert!(facts.host_software.contains("v0.11"), "{facts:?}");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn p6_capability_detection_on_every_simulator_variant() {
    use farm3d_lib::connections::capabilities::CameraDiscovery;
    use farm3d_lib::connections::capabilities::KlippyState;

    let sim = require_sim!(MoonrakerSim::discover());
    let multi = require_sim!(MoonrakerSim::discover_variant(Variant::MultiTool));
    let _guard = sim::exclusive();
    sim.reset();
    multi.reset();

    // moonraker: one tool and a bed.
    let adapter = sim_capabilities(&sim, None);
    assert_facts(&adapter.host_facts().await.expect("facts"), 1, true);
    let state = adapter.host_job_state().await.expect("job state");
    assert_eq!(state.klippy_state, KlippyState::Ready);
    assert_eq!(state.tools.len(), 1, "{state:?}");
    assert!(state.bed.is_some(), "{state:?}");
    assert!(adapter.cameras().await.expect("cameras").is_empty());

    // moonraker-multi: four tools, every one reported.
    let adapter = sim_capabilities(&multi, None);
    assert_facts(&adapter.host_facts().await.expect("facts"), 4, true);
    let state = adapter.host_job_state().await.expect("job state");
    assert_eq!(
        state
            .tools
            .iter()
            .map(|tool| tool.index)
            .collect::<Vec<_>>(),
        vec![0, 1, 2, 3],
        "{state:?}"
    );

    // variant no-bed: the bed is absent, never zero.
    sim.set_mode(Mode::NoBed);
    let adapter = sim_capabilities(&sim, None);
    assert_facts(&adapter.host_facts().await.expect("facts"), 1, false);
    let state = adapter.host_job_state().await.expect("job state");
    assert_eq!(state.bed, None, "{state:?}");

    // variant apikey: refused without the key, answered with it.
    sim.set_mode(Mode::ApiKey);
    let no_key = sim_capabilities(&sim, None);
    assert!(
        matches!(no_key.host_facts().await, Err(ConnectionError::Auth(_))),
        "host facts with no key in apikey mode"
    );
    let keyed = sim_capabilities(&sim, sim.api_key());
    assert_facts(
        &keyed.host_facts().await.expect("facts with the key"),
        1,
        true,
    );
    assert!(keyed
        .cameras()
        .await
        .expect("cameras with the key")
        .is_empty());

    sim.reset();
}
