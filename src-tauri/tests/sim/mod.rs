//! Shared harness for the simulator-backed adapter tests (ADR-0012).
//!
//! The simulators run outside the test process (`sim/compose.yaml`, driven
//! by `sim/simctl`). A test finds them through `FARM3D_SIM_*` environment
//! variables, which `just test-sim` sets from `sim/simctl env`:
//!
//! | Variable | Meaning |
//! | --- | --- |
//! | `FARM3D_SIM_MOONRAKER` | Moonraker through the fault proxy, e.g. `http://127.0.0.1:27125` |
//! | `FARM3D_SIM_MOONRAKER_CONTROL` | Moonraker direct; the harness drives and resets the sim here |
//! | `FARM3D_SIM_MOONRAKER_MULTI` | The four-toolhead Moonraker, through the fault proxy |
//! | `FARM3D_SIM_MOONRAKER_MULTI_CONTROL` | The four-toolhead Moonraker, direct |
//! | `FARM3D_SIM_OCTOPRINT` | OctoPrint through the fault proxy |
//! | `FARM3D_SIM_OCTOPRINT_CONTROL` | OctoPrint direct |
//! | `FARM3D_SIM_OCTOPRINT_API_KEY` | The sim's fixed API key (a fixture, not a secret) |
//! | `FARM3D_SIM_TOXIPROXY` | The fault proxy's API |
//! | `FARM3D_SIM_CTL` | Path to `sim/simctl`, for faults that need the container engine |
//! | `FARM3D_SIM_REQUIRED` | `1` turns "simulator missing" from a skip into a failure (CI) |
//!
//! Two rules hold for every helper here:
//!
//! - **Loopback only.** These tests write: they send G-code, trigger M112,
//!   restart Klipper, and cut connections. Every `FARM3D_SIM_*` URL must name
//!   a loopback host, so a real printer's address can never end up behind
//!   them. Real printers are reachable only through the read-only live tier.
//! - **Skip, don't fail, when the simulators are absent** (unless
//!   `FARM3D_SIM_REQUIRED=1`). The tests are also `#[ignore]`d, so
//!   `cargo test` never runs them by accident.
//!
//! Tests that share a simulator must not overlap. Each test holds
//! [`exclusive`] for its whole body and calls the adapter's `reset` first.
#![allow(dead_code)]

pub mod elegoolink;
pub mod http;
pub mod moonraker;
pub mod octoprint;
pub mod toxiproxy;

use std::sync::{Mutex, MutexGuard};
use std::time::{Duration, Instant};

/// Why a simulator-backed test did not run. Printed, then the test returns.
#[derive(Debug)]
pub struct Skip(pub String);

/// Unwraps a harness handle, or skips the test with a message.
///
/// ```ignore
/// let sim = require_sim!(sim::moonraker::MoonrakerSim::discover());
/// ```
#[macro_export]
macro_rules! require_sim {
    ($discover:expr) => {
        match $discover {
            Ok(handle) => handle,
            Err(skip) => {
                $crate::sim::skip(skip);
                return;
            }
        }
    };
}

/// Reports a skip. With `FARM3D_SIM_REQUIRED=1` it fails instead, so CI
/// cannot pass by skipping everything.
pub fn skip(skip: Skip) {
    if std::env::var("FARM3D_SIM_REQUIRED").as_deref() == Ok("1") {
        panic!(
            "FARM3D_SIM_REQUIRED=1 but the simulator is unavailable: {}",
            skip.0
        );
    }
    eprintln!("SKIPPED (simulator unavailable): {}", skip.0);
    eprintln!("  start the simulators with `just sim-up`, then run `just test-sim`");
}

static EXCLUSIVE: Mutex<()> = Mutex::new(());

/// Serializes simulator tests within one test binary. A panicking test
/// poisons the lock; the next test takes it anyway and resets the sim.
pub fn exclusive() -> MutexGuard<'static, ()> {
    EXCLUSIVE
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reads a `FARM3D_SIM_*` URL and checks that it names a loopback host.
pub fn sim_url(var: &str) -> Result<http::Url, Skip> {
    let raw = std::env::var(var)
        .ok()
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| Skip(format!("{var} is not set")))?;
    let url = http::Url::parse(raw.trim())
        .unwrap_or_else(|error| panic!("{var}={raw:?} is not an http URL: {error}"));
    assert!(
        is_loopback(&url.host),
        "{var} names {:?}, which is not a loopback address. Simulator tests \
         write to their target, so they only ever talk to a local simulator; \
         real printers are reachable only through the read-only live tier \
         (ADR-0012).",
        url.host
    );
    Ok(url)
}

pub fn is_loopback(host: &str) -> bool {
    let host = host.trim_start_matches('[').trim_end_matches(']');
    if host.eq_ignore_ascii_case("localhost") {
        return true;
    }
    host.parse::<std::net::IpAddr>()
        .map(|address| address.is_loopback())
        .unwrap_or(false)
}

/// Polls `check` until it returns `Some`, or fails after `timeout`.
pub fn wait_until<T>(
    what: &str,
    timeout: Duration,
    mut check: impl FnMut() -> Option<T>,
) -> Result<T, String> {
    let deadline = Instant::now() + timeout;
    loop {
        if let Some(value) = check() {
            return Ok(value);
        }
        if Instant::now() >= deadline {
            return Err(format!("{what} did not happen within {timeout:?}"));
        }
        std::thread::sleep(Duration::from_millis(250));
    }
}

/// Runs `sim/simctl <args>` for faults that need the container engine
/// (restarting Klipper). The path comes from `FARM3D_SIM_CTL`.
pub fn simctl(args: &[&str]) -> Result<(), String> {
    let ctl = std::env::var("FARM3D_SIM_CTL")
        .map_err(|_| "FARM3D_SIM_CTL is not set; run through `just test-sim`".to_string())?;
    let output = std::process::Command::new(&ctl)
        .args(args)
        .output()
        .map_err(|error| format!("run {ctl}: {error}"))?;
    if output.status.success() {
        Ok(())
    } else {
        Err(format!(
            "{ctl} {} failed ({}): {}",
            args.join(" "),
            output.status,
            String::from_utf8_lossy(&output.stderr)
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::is_loopback;

    #[test]
    fn only_loopback_hosts_count_as_simulators() {
        for host in ["127.0.0.1", "127.3.2.1", "localhost", "::1", "[::1]"] {
            assert!(is_loopback(host), "{host}");
        }
        for host in ["192.0.2.10", "printer.local", "10.0.0.5", "0.0.0.0", ""] {
            assert!(!is_loopback(host), "{host}");
        }
    }
}
