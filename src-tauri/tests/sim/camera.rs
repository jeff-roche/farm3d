//! The static snapshot camera (P8, `sim/camera/`): nginx serving the
//! committed synthetic test pattern `sim/camera/snapshot.jpg`. The Moonraker
//! simulator's `[webcam farm3d-sim]` names it through the fault proxy, so a
//! host-webcam camera source on the simulator resolves to it.
//!
//! farm3d fetches through `FARM3D_SIM_CAMERA` (the proxy); the harness
//! checks readiness on `FARM3D_SIM_CAMERA_CONTROL`. The camera logs every
//! snapshot `GET`, and [`CameraSim::requests`] counts them, so a test can
//! prove that no frame was fetched without a trigger. The count only ever
//! grows while the container runs: compare it before and after, never
//! against zero.

use super::http::{self, Url};
use super::toxiproxy::Toxiproxy;
use super::{sim_url, simctl_capture, Skip};

/// The committed test pattern, byte for byte what the camera serves.
pub const TEST_PATTERN: &[u8] = include_bytes!("../../../sim/camera/snapshot.jpg");

/// The webcam the Moonraker simulator lists (`sim/moonraker/moonraker.conf`).
pub const WEBCAM_NAME: &str = "farm3d-sim";
pub const WEBCAM_SERVICE: &str = "mjpegstreamer-adaptive";

pub struct CameraSim {
    /// What farm3d fetches from (through the fault proxy).
    pub target: Url,
    pub faults: Toxiproxy,
}

impl CameraSim {
    /// Finds the camera, or explains why the test is skipped.
    pub fn discover() -> Result<Self, Skip> {
        let target = sim_url("FARM3D_SIM_CAMERA")?;
        let control = sim_url("FARM3D_SIM_CAMERA_CONTROL")?;
        let faults = Toxiproxy::discover()?;
        let ready = http::request("GET", &control, "/healthz", &[], None).map_err(|error| {
            Skip(format!(
                "the camera at {} is not answering: {error}",
                control.authority()
            ))
        })?;
        if ready.status != 204 {
            return Err(Skip(format!(
                "the camera at {} answered HTTP {} for /healthz",
                control.authority(),
                ready.status
            )));
        }
        Ok(Self { target, faults })
    }

    /// The snapshot URL through the proxy, as the Moonraker simulator lists
    /// it.
    pub fn snapshot_url(&self) -> String {
        format!("http://{}/snapshot.jpg", self.target.authority())
    }

    /// How many snapshot `GET`s the camera has answered since it started
    /// (`sim/simctl camera-requests`).
    pub fn requests(&self) -> usize {
        let output = simctl_capture(&["camera-requests"])
            .unwrap_or_else(|error| panic!("simctl camera-requests: {error}"));
        output
            .lines()
            .rev()
            .find_map(|line| line.trim().parse().ok())
            .unwrap_or_else(|| panic!("simctl camera-requests printed no count: {output:?}"))
    }
}
