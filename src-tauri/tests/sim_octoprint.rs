//! OctoPrint simulator tests: real OctoPrint with its Virtual Printer plugin,
//! behind the fault proxy (ADR-0012, `sim/compose.yaml`).
//!
//! The first two tests check the harness itself: that the simulator is
//! reachable through the proxy, that its fixed API key works and a wrong
//! one is refused, and that a proxy fault really cuts it off. The rest
//! drive the production `OctoPrintConnection`, the way `sim_moonraker.rs`
//! drives `MoonrakerConnection`.
//!
//! `#[ignore]`d; run with `just sim-up && just test-sim`.

#[macro_use]
mod sim;

use std::time::Duration;

use farm3d_lib::connections::octoprint::OctoPrintConnection;
use farm3d_lib::connections::{
    ConnectionError, ConnectionObservation, ConnectionState, PrinterConnection,
};
use sim::octoprint::OctoPrintSim;
use sim::toxiproxy::Proxy;
use tokio::sync::mpsc;

#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn the_simulator_answers_through_the_proxy_with_its_api_key() {
    let sim = require_sim!(OctoPrintSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let version = sim
        .get_target("/api/version", Some(sim.api_key()))
        .expect("GET /api/version through the proxy");
    assert_eq!(version.status, 200);
    assert_eq!(version.json().unwrap()["server"], "1.11.8");

    let printer = sim
        .get_target("/api/printer", Some(sim.api_key()))
        .expect("GET /api/printer through the proxy");
    assert_eq!(printer.status, 200);
    assert_eq!(
        printer.json().unwrap()["state"]["flags"]["operational"],
        true
    );

    let refused = sim
        .get_target("/api/printer", Some("farm3d-sim-wrong-key"))
        .expect("GET /api/printer with a wrong key");
    assert_eq!(refused.status, 403);
}

#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn a_disabled_proxy_cuts_the_simulator_off() {
    let sim = require_sim!(OctoPrintSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    sim.faults.set_enabled(Proxy::OctoPrint, false);
    let result = sim.get_target("/api/version", Some(sim.api_key()));
    assert!(result.is_err(), "expected no connection, got {result:?}");

    sim.reset();
    assert_eq!(
        sim.get_target("/api/version", Some(sim.api_key()))
            .unwrap()
            .status,
        200
    );
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn probe_reports_the_simulated_octoprint() {
    let sim = require_sim!(OctoPrintSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let probe = sim.connection().probe().await.expect("probe the simulator");

    assert_eq!(probe.kind, "octoprint");
    assert_eq!(probe.host_software, "1.11.8");
    assert_eq!(probe.state, "Operational");
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn subscribe_yields_telemetry_and_an_online_health() {
    let sim = require_sim!(OctoPrintSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let (tx, mut rx) = mpsc::channel(16);
    let connection = sim.connection();
    let task = tokio::spawn(async move { connection.subscribe(tx).await });

    let (mut saw_telemetry, mut saw_online) = (false, false);
    let deadline = Duration::from_secs(15);
    while !(saw_telemetry && saw_online) {
        match tokio::time::timeout(deadline, rx.recv())
            .await
            .expect("an observation within the deadline")
        {
            Some(ConnectionObservation::Telemetry(_)) => saw_telemetry = true,
            Some(ConnectionObservation::Health { state, .. })
                if state == ConnectionState::Online =>
            {
                saw_online = true;
            }
            Some(_) => {}
            None => panic!("the stream ended before telemetry and an online health arrived"),
        }
    }

    task.abort();
    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_disabled_proxy_ends_subscribe_with_an_error() {
    let sim = require_sim!(OctoPrintSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    sim.faults.set_enabled(Proxy::OctoPrint, false);
    let (tx, _rx) = mpsc::channel(16);
    let outcome = tokio::time::timeout(Duration::from_secs(20), sim.connection().subscribe(tx))
        .await
        .expect("subscribe returns once the proxy is down");
    assert!(outcome.is_err(), "{outcome:?}");

    sim.faults.set_enabled(Proxy::OctoPrint, true);
    sim.reset();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
async fn a_wrong_key_is_an_auth_error() {
    let sim = require_sim!(OctoPrintSim::discover());
    let _guard = sim::exclusive();
    sim.reset();

    let wrong = OctoPrintConnection::new(sim.config(), Some("farm3d-sim-wrong-key".to_string()));
    let probe = wrong.probe().await;
    assert!(matches!(probe, Err(ConnectionError::Auth(_))), "{probe:?}");
}
