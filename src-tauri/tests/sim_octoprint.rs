//! OctoPrint simulator tests: real OctoPrint with its Virtual Printer plugin,
//! behind the fault proxy (ADR-0012, `sim/compose.yaml`).
//!
//! **The adapter test is pending #10.** The OctoPrint adapter is not on
//! `main` yet, so these tests check the harness itself: that the simulator
//! is reachable through the proxy, that its fixed API key works and a wrong
//! one is refused, and that a proxy fault really cuts it off. When #10
//! merges, replace `octoprint_adapter_against_the_simulator_is_pending_10`
//! with probe and status-poll tests that drive the production adapter, the
//! way `sim_moonraker.rs` does.
//!
//! `#[ignore]`d; run with `just sim-up && just test-sim`.

#[macro_use]
mod sim;

use sim::octoprint::OctoPrintSim;
use sim::toxiproxy::Proxy;

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

#[test]
#[ignore = "needs the simulators: just sim-up && just test-sim"]
fn octoprint_adapter_against_the_simulator_is_pending_10() {
    eprintln!(
        "PENDING (#10): the OctoPrint adapter is not on main yet. The harness \
         (sim::octoprint) is ready; add adapter probe and status-poll tests here \
         when #10 merges."
    );
}
