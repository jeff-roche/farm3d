//! The Moonraker simulators: real Klipper on simulavr behind real Moonraker
//! (ADR-0012). Only the MCU and its sensors are emulated. Two printers run
//! side by side; [`Variant`] picks one.
//!
//! The adapter under test connects to `FARM3D_SIM_MOONRAKER[_MULTI]`,
//! which goes through the fault proxy. The harness drives and resets the
//! simulator on `..._CONTROL`, which does not, so a fault never blocks the
//! reset that clears it.
//!
//! [`Variant::Single`] also has a switchable [`Mode`] (`sim/simctl variant
//! moonraker default|no-bed|apikey`): a no-bed printer, or a Moonraker that
//! no longer trusts loopback, so a request needs an API key. [`reset`]
//! always restores [`Mode::Default`].
//!
//! [`reset`]: MoonrakerSim::reset

use std::cell::{Cell, RefCell};
use std::time::Duration;

use farm3d_lib::connections::moonraker::MoonrakerConnection;
use farm3d_lib::connections::{ConnectionConfig, MOONRAKER_KIND};
use serde_json::{json, Value};

use super::http::{self, Url};
use super::toxiproxy::{Proxy, Toxiproxy};
use super::{sim_url, simctl, simctl_capture, wait_until, Skip};

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

/// The single-extruder simulator's mode (`sim/simctl variant moonraker
/// ...`). Only [`Variant::Single`] supports switching; the four-toolhead
/// sim always stays [`Mode::Default`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Mode {
    /// The full printer, loopback trusted.
    Default,
    /// `[heater_bed]` dropped from `printer.cfg`: Moonraker reports it as a
    /// missing object, not a zeroed one.
    NoBed,
    /// Loopback no longer trusted; every request needs the API key.
    ApiKey,
}

impl Mode {
    fn simctl_name(self) -> &'static str {
        match self {
            Mode::Default => "default",
            Mode::NoBed => "no-bed",
            Mode::ApiKey => "apikey",
        }
    }
}

pub struct MoonrakerSim {
    pub variant: Variant,
    /// What the adapter connects to (through the fault proxy).
    pub target: Url,
    control: Url,
    pub faults: Toxiproxy,
    mode: Cell<Mode>,
    /// Set once [`Mode::ApiKey`] is selected; Moonraker's key never changes
    /// across restarts, so it stays cached rather than re-read every call.
    api_key: RefCell<Option<String>>,
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
            mode: Cell::new(Mode::Default),
            api_key: RefCell::new(None),
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

    /// The production adapter, aimed at the simulator. Carries the API key
    /// in [`Mode::ApiKey`]; the sim trusts loopback clients otherwise, so
    /// there is none.
    pub fn connection(&self) -> MoonrakerConnection {
        MoonrakerConnection::new(self.config(), self.api_key.borrow().clone())
    }

    /// Switches [`Variant::Single`] to `mode` (`sim/simctl variant
    /// moonraker ...`) and waits for Moonraker to report Klipper ready
    /// again. Only the single-extruder simulator supports this.
    pub fn set_mode(&self, mode: Mode) {
        assert_eq!(
            self.variant,
            Variant::Single,
            "only the single-extruder simulator has switchable modes"
        );
        simctl(&["variant", "moonraker", mode.simctl_name()])
            .unwrap_or_else(|error| panic!("{error}"));
        if mode == Mode::ApiKey {
            let env =
                simctl_capture(&["env"]).unwrap_or_else(|error| panic!("simctl env: {error}"));
            let key = env
                .lines()
                .find_map(|line| line.strip_prefix("export FARM3D_SIM_MOONRAKER_API_KEY="))
                .unwrap_or_else(|| {
                    panic!("simctl env did not export FARM3D_SIM_MOONRAKER_API_KEY in apikey mode")
                })
                .to_string();
            *self.api_key.borrow_mut() = Some(key);
        } else {
            *self.api_key.borrow_mut() = None;
        }
        // Set before checking readiness: klippy_state() needs the key once
        // mode is ApiKey, and needs to stop sending one once mode is
        // anything else.
        self.mode.set(mode);
        // `simctl variant` is idempotent: it skips the restart when the
        // simulator is already in `mode`. That means it does *not* recover
        // Klipper from an unrelated non-ready state (e.g. a prior test's
        // emergency_stop left it "shutdown"), so do that here too, the same
        // way `reset` does.
        if self.klippy_state().as_deref() != Some("ready") {
            self.restart_klipper();
        }
        self.wait_ready();
    }

    /// Returns the simulator to its baseline: default mode, no faults,
    /// Klipper ready, no print running or paused, every heater target at 0.
    /// Klipper's FIRMWARE_RESTART
    /// cannot reset simulavr's emulated MCU, so a Klipper that is not ready
    /// is recovered by restarting its container.
    pub fn reset(&self) {
        if self.variant == Variant::Single {
            self.set_mode(Mode::Default);
        }
        self.faults.reset();
        if self.klippy_state().as_deref() != Some("ready") {
            self.restart_klipper();
        }
        self.wait_ready();
        self.cancel_any_print();
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
        let key = self.api_key.borrow();
        let headers: Vec<(&str, &str)> = match (self.mode.get(), key.as_deref()) {
            (Mode::ApiKey, Some(key)) => vec![("X-Api-Key", key)],
            _ => Vec::new(),
        };
        http::get_json(&self.control, "/server/info", &headers)
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

    /// The API key the adapter needs in [`Mode::ApiKey`]; `None` otherwise.
    pub fn api_key(&self) -> Option<String> {
        self.api_key.borrow().clone()
    }

    /// `print_stats.state` and `print_stats.filename` (empty maps to `None`).
    pub fn print_stats(&self) -> (String, Option<String>) {
        let status = self.query("print_stats");
        let stats = &status["result"]["status"]["print_stats"];
        let state = stats["state"].as_str().unwrap_or_default().to_string();
        let filename = stats["filename"]
            .as_str()
            .filter(|name| !name.is_empty())
            .map(str::to_string);
        (state, filename)
    }

    /// Every heater's target, e.g. `[("extruder", 0.0), ("heater_bed", 0.0)]`.
    pub fn heater_targets(&self) -> Vec<(String, f64)> {
        let heaters = self.heaters();
        if heaters.is_empty() {
            return Vec::new();
        }
        let status = self.query(&heaters.join("&"));
        heaters
            .into_iter()
            .map(|heater| {
                let target = status["result"]["status"][heater.as_str()]["target"]
                    .as_f64()
                    .unwrap_or_else(|| panic!("{heater} reports no target: {status}"));
                (heater, target)
            })
            .collect()
    }

    /// `GET /server/history/list?<query>`, straight from Moonraker.
    pub fn history(&self, query: &str) -> Value {
        http::get_json(&self.control, &format!("/server/history/list?{query}"), &[])
            .unwrap_or_else(|error| panic!("history/list?{query}: {error}"))
    }

    /// Paths (relative to the `gcodes` root) of every file whose path starts
    /// with `prefix`.
    pub fn gcode_files(&self, prefix: &str) -> Vec<String> {
        http::get_json(&self.control, "/server/files/list?root=gcodes", &[])
            .unwrap_or_else(|error| panic!("files/list: {error}"))["result"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|file| file["path"].as_str())
            .filter(|path| path.starts_with(prefix))
            .map(str::to_string)
            .collect()
    }

    /// Cancels whatever print is running or paused (through the control
    /// path) and waits until `print_stats` leaves `printing`/`paused`, so a
    /// test never inherits another test's print.
    pub fn cancel_any_print(&self) {
        let busy = |state: &str| matches!(state, "printing" | "paused");
        if !busy(&self.print_stats().0) {
            return;
        }
        let response = http::post_json(&self.control, "/printer/print/cancel", &[], &json!({}))
            .unwrap_or_else(|error| panic!("cancel: {error}"));
        assert_eq!(
            response.status, 200,
            "cancel answered HTTP {}",
            response.status
        );
        wait_until("the print to stop", Duration::from_secs(60), || {
            (!busy(&self.print_stats().0)).then_some(())
        })
        .unwrap_or_else(|error| panic!("{error}"));
    }

    /// Sends G-code without waiting for it, for a script that holds the
    /// G-code queue (a `G4` dwell). The answer, or its timeout, is ignored.
    pub fn gcode_in_background(&self, script: &str) -> std::thread::JoinHandle<()> {
        let control = self.control.clone();
        let body = json!({"script": script});
        std::thread::spawn(move || {
            let _ = http::post_json(&control, "/printer/gcode/script", &[], &body);
        })
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

/// A no-motion G-code file of about `megabytes` MB: comments and `M117`
/// only. The simulator runs roughly 10 MB of it per second (spike Gate E),
/// and a file that ends within one Moonraker status batch leaves no history
/// job, so even a "quick" file needs a few MB. `megabytes: 0` yields
/// comments only, no `M117` lines, which the Library's import inspection
/// rejects as `INVALID_CONTENT` (a G-code file with no commands) — callers
/// that only stage or identity-check the file, and never run it, still
/// need at least `1`.
pub fn no_motion_gcode(tag: &str, megabytes: usize) -> Vec<u8> {
    let mut bytes =
        format!("; farm3d P6 simulator fixture {tag}: comments and M117 only\n").into_bytes();
    let mut line = 0usize;
    while bytes.len() < megabytes * 1024 * 1024 {
        bytes.extend_from_slice(format!("M117 farm3d {tag} {line}\n").as_bytes());
        line += 1;
    }
    bytes.extend_from_slice(b"; end\n");
    // Nothing but comments and M117: no M104/M109/M140/M190, no T<n>, no
    // motion.
    for line in bytes.split(|byte| *byte == b'\n') {
        assert!(
            line.is_empty() || line.starts_with(b";") || line.starts_with(b"M117 "),
            "the fixture must hold only comments and M117: {}",
            String::from_utf8_lossy(line)
        );
    }
    bytes
}
