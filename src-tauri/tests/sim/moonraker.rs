//! The Moonraker simulators: real Klipper on simulavr behind real Moonraker
//! (ADR-0012). Only the MCU and its sensors are emulated. Two printers run
//! side by side; [`Variant`] picks one.
//!
//! The adapter under test connects to `FARM3D_SIM_MOONRAKER[_MULTI]`,
//! which goes through the fault proxy. The harness drives and resets the
//! simulator on `..._CONTROL`, which does not, so a fault never blocks the
//! reset that clears it.

use std::time::Duration;

use farm3d_lib::connections::moonraker::MoonrakerConnection;
use farm3d_lib::connections::{ConnectionConfig, MOONRAKER_KIND};
use serde_json::{json, Value};

use super::http::{self, Url};
use super::toxiproxy::{Proxy, Toxiproxy};
use super::{sim_url, simctl, wait_until, Skip};

/// Klipper's first "ready" after a container restart takes a few seconds on
/// simulavr; allow for a slow CI runner.
const READY_TIMEOUT: Duration = Duration::from_secs(90);

/// Which simulated printer. Both run the same Klipper and Moonraker builds.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Variant {
    /// One extruder and a heated bed (`sim/moonraker/printer.cfg`).
    Single,
    /// Four toolheads, `extruder` to `extruder3`, shaped like a Snapmaker
    /// U1 (`sim/moonraker/printer-multi.cfg`).
    MultiTool,
}

impl Variant {
    fn env_prefix(self) -> &'static str {
        match self {
            Variant::Single => "FARM3D_SIM_MOONRAKER",
            Variant::MultiTool => "FARM3D_SIM_MOONRAKER_MULTI",
        }
    }

    /// The name `sim/simctl` uses for it.
    fn simctl_name(self) -> &'static str {
        match self {
            Variant::Single => "moonraker",
            Variant::MultiTool => "moonraker-multi",
        }
    }

    pub fn proxy(self) -> Proxy {
        match self {
            Variant::Single => Proxy::Moonraker,
            Variant::MultiTool => Proxy::MoonrakerMulti,
        }
    }
}

pub struct MoonrakerSim {
    pub variant: Variant,
    /// What the adapter connects to (through the fault proxy).
    pub target: Url,
    control: Url,
    pub faults: Toxiproxy,
}

impl MoonrakerSim {
    /// The single-extruder simulator. See [`MoonrakerSim::discover_variant`].
    pub fn discover() -> Result<Self, Skip> {
        Self::discover_variant(Variant::Single)
    }

    /// Finds a simulator, or explains why the test is skipped. It does not
    /// reset anything; call [`MoonrakerSim::reset`] inside the test's
    /// `exclusive()` section.
    pub fn discover_variant(variant: Variant) -> Result<Self, Skip> {
        let prefix = variant.env_prefix();
        let target = sim_url(prefix)?;
        let control = sim_url(&format!("{prefix}_CONTROL"))?;
        let faults = Toxiproxy::discover()?;
        http::get_json(&control, "/server/info", &[]).map_err(|error| {
            Skip(format!(
                "Moonraker at {} is not answering: {error}",
                control.authority()
            ))
        })?;
        Ok(Self {
            variant,
            target,
            control,
            faults,
        })
    }

    /// The Connection a Printer pointed at this simulator would have.
    pub fn config(&self) -> ConnectionConfig {
        ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: self.target.host.clone(),
            port: self.target.port,
            use_tls: false,
            credential_ref: None,
        }
    }

    /// The production adapter, aimed at the simulator. The sim trusts
    /// loopback clients, so there is no API key.
    pub fn connection(&self) -> MoonrakerConnection {
        MoonrakerConnection::new(self.config(), None)
    }

    /// Returns the simulator to its baseline: no faults, Klipper ready, every
    /// heater target at 0. Klipper's FIRMWARE_RESTART cannot reset
    /// simulavr's emulated MCU, so a Klipper that is not ready is recovered
    /// by restarting its container.
    pub fn reset(&self) {
        self.faults.reset();
        if self.klippy_state().as_deref() != Some("ready") {
            self.restart_klipper();
        }
        self.wait_ready();
        for heater in self.heaters() {
            self.gcode(&format!("SET_HEATER_TEMPERATURE HEATER={heater} TARGET=0"));
        }
    }

    /// Every heater Klipper reports, e.g. `extruder`, `extruder1`, `heater_bed`.
    pub fn heaters(&self) -> Vec<String> {
        self.query("heaters")["result"]["status"]["heaters"]["available_heaters"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    }

    pub fn klippy_state(&self) -> Option<String> {
        http::get_json(&self.control, "/server/info", &[])
            .ok()
            .and_then(|info| info["result"]["klippy_state"].as_str().map(str::to_string))
    }

    pub fn wait_ready(&self) {
        wait_until("Klipper reporting ready", READY_TIMEOUT, || {
            (self.klippy_state().as_deref() == Some("ready")).then_some(())
        })
        .unwrap_or_else(|error| {
            panic!(
                "{error}; see `sim/simctl logs {}`",
                self.variant.simctl_name()
            )
        });
    }

    /// Runs G-code through Moonraker's control path and fails on an error.
    pub fn gcode(&self, script: &str) {
        let response = http::post_json(
            &self.control,
            "/printer/gcode/script",
            &[],
            &json!({"script": script}),
        )
        .unwrap_or_else(|error| panic!("G-code {script:?}: {error}"));
        assert_eq!(
            response.status,
            200,
            "G-code {script:?} answered HTTP {}: {}",
            response.status,
            String::from_utf8_lossy(&response.body)
        );
    }

    /// M112. Klipper goes to "shutdown" and stays there until [`reset`].
    ///
    /// [`reset`]: MoonrakerSim::reset
    pub fn emergency_stop(&self) {
        let response = http::post_json(&self.control, "/printer/emergency_stop", &[], &json!({}))
            .unwrap_or_else(|error| panic!("emergency_stop: {error}"));
        assert_eq!(
            response.status, 200,
            "emergency_stop answered HTTP {}",
            response.status
        );
        // Moonraker learns of the shutdown asynchronously; return only once
        // it reports it, so a following `reset` never sees a stale "ready".
        wait_until(
            "Klipper reporting shutdown",
            Duration::from_secs(10),
            || (self.klippy_state().as_deref() == Some("shutdown")).then_some(()),
        )
        .unwrap_or_else(|error| panic!("{error}"));
    }

    /// Restarts the Klipper container: a fresh MCU and a fresh klippy.
    /// Moonraker sees Klippy disconnect and reconnect.
    pub fn restart_klipper(&self) {
        simctl(&["fault", "klipper-restart", self.variant.simctl_name()])
            .unwrap_or_else(|error| panic!("{error}"));
    }

    /// Queries Klipper objects through the control path, e.g. `"heaters"` or
    /// `"extruder&extruder1"`.
    pub fn query(&self, objects: &str) -> Value {
        http::get_json(
            &self.control,
            &format!("/printer/objects/query?{objects}"),
            &[],
        )
        .unwrap_or_else(|error| panic!("query {objects}: {error}"))
    }

    /// The object names Klipper reports (`printer.objects.list`).
    pub fn objects(&self) -> Vec<String> {
        http::get_json(&self.control, "/printer/objects/list", &[])
            .unwrap_or_else(|error| panic!("objects/list: {error}"))["result"]["objects"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_string)
            .collect()
    }
}
