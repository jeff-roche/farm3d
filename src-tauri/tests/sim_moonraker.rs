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

use farm3d_lib::connections::{
    ConnectionError, ConnectionObservation, ConnectionState, PrinterConnection, MOONRAKER_KIND,
};
use sim::moonraker::{MoonrakerSim, Variant};
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
