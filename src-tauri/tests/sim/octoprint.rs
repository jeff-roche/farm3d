//! The OctoPrint simulator: real OctoPrint 1.11.8 with its bundled Virtual
//! Printer plugin attached as the serial device (ADR-0012).
//!
//! The adapter under test connects to `FARM3D_SIM_OCTOPRINT`, through the
//! fault proxy. The harness uses `FARM3D_SIM_OCTOPRINT_CONTROL` directly.
//!
//! [`OctoPrintSim::connection`] builds the production `OctoPrintConnection`
//! from [`OctoPrintSim::config`] and [`OctoPrintSim::api_key`], the same
//! way `MoonrakerSim::connection` does.

use std::time::Duration;

use farm3d_lib::connections::octoprint::OctoPrintConnection;
use farm3d_lib::connections::ConnectionConfig;
use serde_json::{json, Value};

use super::http::{self, Url};
use super::toxiproxy::Toxiproxy;
use super::{sim_url, wait_until, Skip};

const READY_TIMEOUT: Duration = Duration::from_secs(60);

pub struct OctoPrintSim {
    pub target: Url,
    control: Url,
    api_key: String,
    pub faults: Toxiproxy,
}

impl OctoPrintSim {
    pub fn discover() -> Result<Self, Skip> {
        let target = sim_url("FARM3D_SIM_OCTOPRINT")?;
        let control = sim_url("FARM3D_SIM_OCTOPRINT_CONTROL")?;
        let api_key = std::env::var("FARM3D_SIM_OCTOPRINT_API_KEY")
            .map_err(|_| Skip("FARM3D_SIM_OCTOPRINT_API_KEY is not set".into()))?;
        let faults = Toxiproxy::discover()?;
        let sim = Self {
            target,
            control,
            api_key,
            faults,
        };
        http::get_json(&sim.control, "/api/version", &sim.key_header()).map_err(|error| {
            Skip(format!(
                "OctoPrint at {} is not answering: {error}",
                sim.control.authority()
            ))
        })?;
        Ok(sim)
    }

    /// The Connection a Printer pointed at this simulator would have. The
    /// kind string matches the one #10 introduces.
    pub fn config(&self) -> ConnectionConfig {
        ConnectionConfig {
            kind: "octoprint".to_string(),
            host: self.target.host.clone(),
            port: self.target.port,
            use_tls: false,
            credential_ref: None,
        }
    }

    /// The simulator's fixed API key: a published fixture, not a secret.
    pub fn api_key(&self) -> &str {
        &self.api_key
    }

    /// The production adapter, aimed at the simulator through the fault
    /// proxy, with the simulator's fixed API key.
    pub fn connection(&self) -> OctoPrintConnection {
        OctoPrintConnection::new(self.config(), Some(self.api_key.clone()))
    }

    fn key_header(&self) -> [(&str, &str); 1] {
        [("X-Api-Key", self.api_key.as_str())]
    }

    /// No faults, and the Virtual Printer attached ("Operational").
    pub fn reset(&self) {
        self.faults.reset();
        if self.connection_state().as_deref() != Some("Operational") {
            let response = http::post_json(
                &self.control,
                "/api/connection",
                &self.key_header(),
                &json!({"command": "connect", "port": "VIRTUAL", "autoconnect": true}),
            )
            .unwrap_or_else(|error| panic!("connect the Virtual Printer: {error}"));
            assert_eq!(
                response.status, 204,
                "connect answered HTTP {}",
                response.status
            );
        }
        wait_until("OctoPrint reporting Operational", READY_TIMEOUT, || {
            (self.connection_state().as_deref() == Some("Operational")).then_some(())
        })
        .unwrap_or_else(|error| panic!("{error}; see `sim/simctl logs octoprint`"));
    }

    pub fn connection_state(&self) -> Option<String> {
        self.get_control("/api/connection")
            .ok()
            .and_then(|value| value["current"]["state"].as_str().map(str::to_string))
    }

    /// GET through the fault proxy, as an adapter would see it.
    pub fn get_target(&self, path: &str, api_key: Option<&str>) -> Result<http::Response, String> {
        let headers: Vec<(&str, &str)> =
            api_key.map(|key| ("X-Api-Key", key)).into_iter().collect();
        http::request("GET", &self.target, path, &headers, None)
    }

    pub fn get_control(&self, path: &str) -> Result<Value, String> {
        http::get_json(&self.control, path, &self.key_header())
    }
}
