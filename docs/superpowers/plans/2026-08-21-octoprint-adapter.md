# OctoPrint Adapter Implementation Plan

> **Status (2026-09-25, issue #10):** Implemented on
> `feature/a0-2-octoprint-monitoring`, adapted to the P1–P5 trait shape
> (`ConnectionObservation`, `PrinterTelemetry`, `HostActivity`) and to the
> five kind gates that exist by now. The task-by-task code below is kept
> as the original design record. For the differences and the live
> evidence, see `docs/verification/2026-09-25-a0-2-octoprint.md`.

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Make OctoPrint the second real `PrinterConnection` — configure an OctoPrint printer, test the connection, and stream its live temperatures, job state, and progress onto the dashboard, exactly as phase 2 did for Moonraker.

**Architecture:** A second implementation of the existing `PrinterConnection` trait (`probe`/`subscribe`), added without changing the trait, `ProbeResult`, `PrinterStatus`, or `ReportedCapabilities` — all of which already generalize cleanly to OctoPrint's REST shapes. Unlike Moonraker's push-based WebSocket, `subscribe()` drives its own 2-second polling interval over plain HTTP with an `X-Api-Key` header, feeding the same `mpsc::Sender<PrinterStatus>` a push adapter would; the supervisor, credential store, and mDNS discovery added in phase 2 are all protocol-agnostic already and need no changes. All response parsing lives in a pure, I/O-free module, mirroring `moonraker::protocol`.

**Tech Stack:** Rust (Tauri 2, `reqwest` new to this phase, `tokio`, `async-trait` already present), TypeScript (SolidJS, the repo's design system), `cargo test` + Vitest.

**Spec:** `docs/superpowers/specs/2026-08-20-printer-adapters-design.md` (OctoPrint section; ElegooLink is out of scope for this plan — see "Notes for the ElegooLink spike" at the end).

## Global Constraints

Every task's requirements implicitly include this section.

- **The `PrinterConnection` trait, `ProbeResult`, `PrinterStatus`, `ReportedCapabilities`, and `ConnectionError` are NOT modified by this plan.** They already generalize to OctoPrint (see the spec's "OctoPrint wire contract"). If implementation reveals a real need to change one, stop and treat it the way the spec's "Expected trait reshaping is not a phase-2 failure" section describes — record it in this plan's self-review, don't silently widen a shared type.
- **Exactly two registration points for a new `kind`:** `src-tauri/src/connections/supervisor.rs`'s `build()` and `src-tauri/src/connections/commands.rs`'s `adapter()`. Both are a `match config.kind.as_str()`. Nothing else gates a connection kind — confirmed by grepping the whole `src-tauri/src` tree for `MOONRAKER_KIND` before writing this plan.
- **`GET /api/printer` answers 409 Conflict when no printer is attached to OctoPrint.** This is normal, not an error — the direct equivalent of Moonraker's "Klippy down behind a healthy socket." Treat it as "no temperature reading this tick," never as a `ConnectionError`.
- **`progress.completion` is a 0–100 percentage; `PrinterStatus.progress` is `0.0..=1.0`.** Divide by 100. This is this phase's single highest-risk detail — the direct equivalent of Moonraker's partial-update merge — and gets the same dedicated, prove-it-catches-the-bug test treatment.
- **No adapter implements retry.** A single failed poll tick ends `subscribe()` (returning `Err`), and the supervisor's existing reconnect-with-backoff handles it — exactly Moonraker's contract. Do not add internal retry/backoff to the OctoPrint adapter; that would contradict the trait's stated invariant.
- **`cargo`/`rustc` are not on `PATH` in non-interactive shells.** Prefix every Rust command with `source "$HOME/.cargo/env" && `.
- **Verification gate:** `just build`, `just test`, and `just test-rust` must all pass before any task is considered done.
- **Test idiom (Rust):** follow `moonraker/protocol.rs` and `moonraker/mod.rs` exactly — pure `serde_json::Value` navigation (no typed response structs), plain `#[test]` functions with no async runtime for anything that doesn't need one. The repo has **no dev-dependencies** and adds none here — confirmed by compiling `reqwest 0.12` (`features = ["json"]`) in a scratch crate with a plain, non-async `#[test]` building a `Request` and inspecting its headers, exactly like `moonraker::upgrade_request`'s tests.
- **Never hardcode colors, font sizes, or radii in component CSS.** Not touched by this plan (no new CSS), but stated per repo convention since a component file is modified.
- **`ConnectionState` stays `connecting | online | offline | error`.** OctoPrint's free-text `state`/job `state` strings map onto it; they do not extend it.

---

## File Structure

**Create (Rust):**

| File | Responsibility |
|---|---|
| `src-tauri/src/connections/octoprint/protocol.rs` | **Pure** REST response parsing + status/probe mapping. No HTTP client. |
| `src-tauri/src/connections/octoprint/mod.rs` | The HTTP-facing adapter: client, polling loop, status classification. Thin — framing lives next door. |

**Modify:** `src-tauri/Cargo.toml`, `src-tauri/src/connections/mod.rs` (constants + module declaration), `src-tauri/src/connections/supervisor.rs`, `src-tauri/src/connections/commands.rs`, `src/screens/PrinterConnectionPanel.tsx` (+ `.test.tsx`).

**Why `protocol.rs` is split from `octoprint/mod.rs`:** the percentage-to-fraction conversion and the 409-tolerant temperature merge are the highest-risk logic in this phase, and keeping them in a module with no HTTP client means their tests are plain `#[test]` functions over JSON literals — no network, no mock server, no fixtures. This is the same split phase 2 made for Moonraker, for the same reason.

---

## Ordering and risk

Tasks 1–2 are pure Rust with no I/O — highest test value, lowest integration friction, so they come first. Task 3 is the first code that makes an HTTP request. Task 5 is the frontend, developable against `just web` without an OctoPrint instance (it only touches a static option list).

**The single highest-risk detail in this phase** is `progress.completion` being a 0–100 percentage against farm3d's 0.0–1.0 contract. Task 2 exists to get it right in isolation, with a step that deliberately breaks the conversion first to prove the test actually catches it — the same discipline Task 3 of the phase-2 plan applied to Moonraker's partial-update merge.

---

### Task 1: The `reqwest` dependency and OctoPrint connection constants

**Files:**
- Modify: `src-tauri/Cargo.toml`
- Modify: `src-tauri/src/connections/mod.rs`

**Interfaces:**
- Produces: `OCTOPRINT_KIND: &str`, `DEFAULT_OCTOPRINT_PORT: u16`. Every later task consumes these.

- [ ] **Step 1: Add the dependency**

`reqwest 0.12` with `features = ["json"]` was compile-verified in a scratch crate against this repo's exact existing `tokio` feature set (`sync`, `time`, `rt` — no `rt-multi-thread`, no `macros`) while writing this plan: a `Client` was built and a request sent under `#[tokio::main(flavor = "current_thread")]`, confirming it needs nothing farm3d's `tokio` dependency doesn't already provide via Tauri's runtime. Its default TLS backend (pulling in `hyper-tls`/`native-tls`) also compiled clean, which is what lets this adapter honor `ConnectionConfig.use_tls` the same way Moonraker's `wss://` does.

In `src-tauri/Cargo.toml`, under `[dependencies]`, after `mdns-sd = "0.21"`:

```toml
reqwest = { version = "0.12", features = ["json"] }
```

- [ ] **Step 2: Verify the dependency tree resolves**

Run: `source "$HOME/.cargo/env" && cargo fetch --manifest-path src-tauri/Cargo.toml`
Expected: completes without a version-resolution error.

- [ ] **Step 3: Write the failing tests**

Append to the existing `#[cfg(test)] mod tests` block in `src-tauri/src/connections/mod.rs`:

```rust
    #[test]
    fn octoprint_kind_and_default_port_are_defined() {
        assert_eq!(OCTOPRINT_KIND, "octoprint");
        assert_eq!(DEFAULT_OCTOPRINT_PORT, 80);
    }

    #[test]
    fn connection_config_round_trips_for_octoprint() {
        let config = ConnectionConfig {
            kind: OCTOPRINT_KIND.to_string(),
            host: "octoprint.local".to_string(),
            port: DEFAULT_OCTOPRINT_PORT,
            use_tls: false,
            credential_ref: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(serde_json::from_str::<ConnectionConfig>(&json).unwrap(), config);
    }
```

- [ ] **Step 4: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml connections::tests`
Expected: FAIL — `cannot find value OCTOPRINT_KIND in this scope`.

- [ ] **Step 5: Add the constants and update the two doc comments that named only Moonraker**

In `src-tauri/src/connections/mod.rs`, change:

```rust
pub const MOONRAKER_KIND: &str = "moonraker";
pub const DEFAULT_MOONRAKER_PORT: u16 = 7125;
```

to:

```rust
pub const MOONRAKER_KIND: &str = "moonraker";
pub const DEFAULT_MOONRAKER_PORT: u16 = 7125;
pub const OCTOPRINT_KIND: &str = "octoprint";
/// OctoPrint's own default when served directly (not behind a reverse
/// proxy). Matches the frontend's existing `DEFAULT_PORTS` entry.
pub const DEFAULT_OCTOPRINT_PORT: u16 = 80;
```

And change the `kind` field's doc comment on `ConnectionConfig` from:

```rust
    /// `"moonraker"` today; phase 3 adds `"octoprint"` and `"elegoolink"`.
    /// A free string rather than an enum so an unknown kind written by a
    /// newer farm3d round-trips through an older one instead of failing the
    /// whole `printers.json` load.
    pub kind: String,
```

to:

```rust
    /// `"moonraker"` and `"octoprint"` today; `"elegoolink"` is still
    /// pending its protocol spike (see
    /// `docs/superpowers/specs/2026-08-20-printer-adapters-design.md`).
    /// A free string rather than an enum so an unknown kind written by a
    /// newer farm3d round-trips through an older one instead of failing the
    /// whole `printers.json` load.
    pub kind: String,
```

- [ ] **Step 6: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml connections::tests`
Expected: 6 passed (4 existing + 2 here).

- [ ] **Step 7: Commit**

```bash
git add src-tauri/Cargo.toml src-tauri/src/connections/mod.rs
git commit -m "feat: add the reqwest dependency and OctoPrint connection constants"
```

---

### Task 2: OctoPrint response parsing and status mapping (pure, no HTTP client)

**This is the highest-risk task in the phase.** `progress.completion` is a 0–100 percentage; farm3d's `PrinterStatus.progress` is `0.0..=1.0`. Getting it right here — in a module with no I/O — means it is testable with plain `#[test]` functions over JSON literals, and this task includes a step that proves the test would actually catch the bug.

**Files:**
- Create: `src-tauri/src/connections/octoprint/protocol.rs`
- Create: `src-tauri/src/connections/octoprint/mod.rs` (declaration only this task; Task 3 fills it)
- Modify: `src-tauri/src/connections/mod.rs` (add `pub mod octoprint;`)

**Interfaces:**
- Consumes: `ConnectionState`, `PrinterStatus`, `ProbeResult`, `ReportedCapabilities`, `OCTOPRINT_KIND` (Task 1).
- Produces: `str_field(&Value, &str) -> String`, `probe_result_from(version: &Value, connection: &Value, profiles: &Value) -> ProbeResult`, `connection_state_from_job_state(&str) -> ConnectionState`, `status_from(job: &Value, printer: Option<&Value>) -> PrinterStatus`.

- [ ] **Step 1: Write the failing tests**

Create `src-tauri/src/connections/octoprint/protocol.rs` containing only this test module for now:

```rust
#[cfg(test)]
mod tests {
    use super::*;

    fn version() -> Value {
        serde_json::json!({"api": "0.1", "server": "1.9.3", "text": "OctoPrint 1.9.3"})
    }

    fn connection(state: &str) -> Value {
        serde_json::json!({"current": {
            "state": state, "port": "/dev/ttyACM0", "baudrate": 250000,
            "printerProfile": "_default"
        }})
    }

    fn profiles() -> Value {
        serde_json::json!({"profiles": {"_default": {
            "id": "_default", "name": "Voron 2.4",
            "volume": {
                "formFactor": "rectangular", "origin": "lowerleft",
                "width": 250.0, "depth": 210.0, "height": 220.0
            }
        }}})
    }

    #[test]
    fn probe_reads_version_state_and_active_profile_volume() {
        let probe = probe_result_from(&version(), &connection("Operational"), &profiles());
        assert_eq!(probe.kind, "octoprint");
        assert_eq!(probe.host_software, "1.9.3");
        assert_eq!(probe.firmware, "");
        assert_eq!(probe.reported_name, "Voron 2.4");
        assert_eq!(probe.state, "Operational");
        assert_eq!(probe.reported.bed_width_mm, Some(250.0));
        assert_eq!(probe.reported.bed_depth_mm, Some(210.0));
        assert_eq!(probe.reported.printable_height_mm, Some(220.0));
    }

    #[test]
    fn probe_tolerates_an_unknown_active_profile_id_rather_than_panicking() {
        // Should never happen against a real OctoPrint, but must degrade to
        // absent capabilities rather than panic — the same tolerance
        // Moonraker's probe has for a shut-down Klipper reporting no axis
        // limits.
        let probe = probe_result_from(
            &version(),
            &connection("Closed"),
            &serde_json::json!({"profiles": {}}),
        );
        assert_eq!(probe.reported, ReportedCapabilities::default());
        assert_eq!(probe.reported_name, "");
    }

    #[test]
    fn job_states_map_to_connection_states_by_prefix() {
        assert_eq!(connection_state_from_job_state("Operational"), ConnectionState::Online);
        assert_eq!(connection_state_from_job_state("Printing"), ConnectionState::Online);
        assert_eq!(connection_state_from_job_state("Offline"), ConnectionState::Offline);
        assert_eq!(
            connection_state_from_job_state("Offline after error"),
            ConnectionState::Offline
        );
        assert_eq!(
            connection_state_from_job_state("Error: something bad"),
            ConnectionState::Error
        );
    }

    fn job(state: &str, name: &str, completion: f64, print_time: f64) -> Value {
        serde_json::json!({
            "job": {"file": {"name": name}},
            "progress": {"completion": completion, "printTime": print_time},
            "state": state
        })
    }

    #[test]
    fn completion_percentage_is_converted_to_a_zero_to_one_fraction() {
        // THE bug this module exists to prevent. OctoPrint's `completion` is
        // 0-100; farm3d's `progress` is 0.0..=1.0.
        let status = status_from(&job("Printing", "benchy.gcode", 42.5, 300.0), None);
        assert_eq!(status.progress, Some(0.425));
        assert_eq!(status.job_name, Some("benchy.gcode".to_string()));
        assert_eq!(status.print_duration_s, Some(300.0));
    }

    #[test]
    fn a_409_from_api_printer_reports_no_temperatures_rather_than_erroring() {
        let status = status_from(&job("Offline", "", 0.0, 0.0), None);
        assert_eq!(status.connection_state, ConnectionState::Offline);
        assert_eq!(status.nozzle_temp_c, None);
        assert_eq!(status.bed_temp_c, None);
    }

    #[test]
    fn temperatures_are_read_when_the_printer_answers() {
        let printer = serde_json::json!({"temperature": {
            "tool0": {"actual": 210.1, "target": 215.0},
            "bed": {"actual": 60.0, "target": 60.0}
        }});
        let status = status_from(&job("Printing", "x.gcode", 10.0, 60.0), Some(&printer));
        assert_eq!(status.nozzle_temp_c, Some(210.1));
        assert_eq!(status.nozzle_target_c, Some(215.0));
        assert_eq!(status.bed_temp_c, Some(60.0));
        assert_eq!(status.bed_target_c, Some(60.0));
    }

    #[test]
    fn a_profile_with_no_heated_bed_reports_no_bed_temperature() {
        let printer =
            serde_json::json!({"temperature": {"tool0": {"actual": 200.0, "target": 200.0}}});
        let status = status_from(&job("Printing", "x.gcode", 5.0, 10.0), Some(&printer));
        assert_eq!(status.bed_temp_c, None);
        assert_eq!(status.bed_target_c, None);
    }

    #[test]
    fn an_empty_job_state_is_treated_as_absent_not_an_empty_string_label() {
        let status = status_from(&job("", "", 0.0, 0.0), None);
        assert_eq!(status.job_state, None);
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml octoprint`
Expected: FAIL — `cannot find function probe_result_from in this scope` (plus a "file not found for module octoprint" error until Step 4 wires `connections/mod.rs`).

- [ ] **Step 3: Write the implementation**

Prepend to `src-tauri/src/connections/octoprint/protocol.rs`, above the test module:

```rust
//! OctoPrint's REST JSON shapes — PURE. No HTTP client, no async, no Tauri.
//! Mirrors `moonraker::protocol`: the risky mapping logic lives here, where
//! it is testable without a printer or a network client.

use crate::connections::{ConnectionState, PrinterStatus, ProbeResult, ReportedCapabilities};
use serde_json::Value;

fn str_field(value: &Value, key: &str) -> String {
    value.get(key).and_then(Value::as_str).unwrap_or_default().to_string()
}

/// `GET /api/connection`'s `current.printerProfile` names the active
/// profile's id in `GET /api/printerprofiles`'s `profiles` map. Looking it up
/// this way — rather than calling the single-profile endpoint
/// (`GET /api/printerprofiles/<id>`) — relies only on a response shape
/// OctoPrint's docs show a concrete worked example for; the single-profile
/// endpoint's shape is undocumented beyond prose.
pub fn probe_result_from(version: &Value, connection: &Value, profiles: &Value) -> ProbeResult {
    let current = &connection["current"];
    let active_id = current["printerProfile"].as_str().unwrap_or_default();
    let profile = &profiles["profiles"][active_id];
    ProbeResult {
        kind: crate::connections::OCTOPRINT_KIND.to_string(),
        host_software: str_field(version, "server"),
        // OctoPrint's core REST API exposes no field for the underlying
        // printer's firmware version, unlike Moonraker's Klipper
        // `software_version`. Left empty rather than guessed at.
        firmware: String::new(),
        reported_name: str_field(profile, "name"),
        state: str_field(current, "state"),
        // `current.state` already folds error detail in (e.g. "Error:
        // ..."), so there is no separate message field the way Moonraker's
        // `printer.info.state_message` supplies one.
        state_message: String::new(),
        reported: ReportedCapabilities {
            bed_width_mm: profile["volume"]["width"].as_f64(),
            bed_depth_mm: profile["volume"]["depth"].as_f64(),
            printable_height_mm: profile["volume"]["height"].as_f64(),
        },
    }
}

/// OctoPrint has no clean state enum the way Moonraker's `klippy_state`
/// does; this is a prefix match over `GET /api/job`'s free-text `state`.
pub fn connection_state_from_job_state(state: &str) -> ConnectionState {
    if state.starts_with("Offline") {
        ConnectionState::Offline
    } else if state.starts_with("Error") {
        ConnectionState::Error
    } else {
        ConnectionState::Online
    }
}

/// `printer` is `None` when `GET /api/printer` answered 409 (no printer
/// connected to OctoPrint) — a normal state, not an error, so this must
/// still produce a `PrinterStatus` with every temperature field `None`
/// rather than refusing to build one at all.
pub fn status_from(job: &Value, printer: Option<&Value>) -> PrinterStatus {
    let job_state = str_field(job, "state");
    let mut status = PrinterStatus::new(connection_state_from_job_state(&job_state));
    if !job_state.is_empty() {
        status.job_state = Some(job_state);
    }
    status.job_name = job["job"]["file"]["name"].as_str().map(str::to_string);
    // `progress.completion` is a 0-100 PERCENTAGE (OctoPrint's data-model
    // page, not the endpoint page's example value, which reads as ambiguous
    // alone); farm3d's `progress` is 0.0..=1.0. Skipping the /100.0 here
    // would render every progress bar 100x too full.
    status.progress = job["progress"]["completion"].as_f64().map(|pct| pct / 100.0);
    status.print_duration_s = job["progress"]["printTime"].as_f64();
    if let Some(printer) = printer {
        status.nozzle_temp_c = printer["temperature"]["tool0"]["actual"].as_f64();
        status.nozzle_target_c = printer["temperature"]["tool0"]["target"].as_f64();
        // `bed` is absent entirely on a printer profile with no heated bed —
        // a normal state, not a missing reading to warn about.
        status.bed_temp_c = printer["temperature"]["bed"]["actual"].as_f64();
        status.bed_target_c = printer["temperature"]["bed"]["target"].as_f64();
    }
    status
}
```

- [ ] **Step 4: Declare the module**

Create `src-tauri/src/connections/octoprint/mod.rs` with just:

```rust
pub mod protocol;
```

And add to `src-tauri/src/connections/mod.rs`, beside `pub mod moonraker;`:

```rust
pub mod octoprint;
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml octoprint::protocol`
Expected: 7 passed.

- [ ] **Step 6: Prove the percentage-conversion test actually catches the bug**

Temporarily replace the `progress` line in `status_from` with the naive version:

```rust
    status.progress = job["progress"]["completion"].as_f64(); // WRONG ON PURPOSE — no /100.0
```

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml octoprint::protocol`
Expected: FAIL on `completion_percentage_is_converted_to_a_zero_to_one_fraction` — `left: Some(42.5), right: Some(0.425)`.

Then restore the real implementation (`.map(|pct| pct / 100.0)`) and re-run — expected: 7 passed.

- [ ] **Step 7: Commit**

```bash
git add src-tauri/src/connections/octoprint/ src-tauri/src/connections/mod.rs
git commit -m "feat: add OctoPrint response parsing and status mapping"
```

---

### Task 3: The OctoPrint REST adapter

The first code in this repo that issues an HTTP request. It stays thin because Task 2 owns the parsing: this task is the client, the polling loop, and HTTP-status classification.

**Files:**
- Modify: `src-tauri/src/connections/octoprint/mod.rs`

**Interfaces:**
- Consumes: `PrinterConnection`, `ConnectionConfig`, `ConnectionError`, `PrinterStatus`, `ProbeResult` (existing, Task 1 phase 2); `probe_result_from`, `status_from` (Task 2).
- Produces: `OctoPrintConnection::new(config: ConnectionConfig, api_key: Option<String>) -> Self`, `base_url(&ConnectionConfig) -> String`, and the `impl PrinterConnection for OctoPrintConnection`.

- [ ] **Step 1: Write the failing tests**

Only the pure helpers are unit-tested. `probe()` and `subscribe()` make real HTTP requests; testing them needs a live OctoPrint, which the end-to-end verification at the bottom of this plan covers. Do **not** add a mock HTTP server — it would test the mock, and Task 2 already covers every decision that isn't "did the bytes move".

Append to `src-tauri/src/connections/octoprint/mod.rs`:

```rust
#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{DEFAULT_OCTOPRINT_PORT, OCTOPRINT_KIND};

    fn config(use_tls: bool) -> ConnectionConfig {
        ConnectionConfig {
            kind: OCTOPRINT_KIND.to_string(),
            host: "octoprint.local".to_string(),
            port: DEFAULT_OCTOPRINT_PORT,
            use_tls,
            credential_ref: None,
        }
    }

    #[test]
    fn builds_a_plain_http_base_url() {
        assert_eq!(base_url(&config(false)), "http://octoprint.local:80");
    }

    #[test]
    fn builds_an_https_base_url_under_tls() {
        assert_eq!(base_url(&config(true)), "https://octoprint.local:80");
    }

    #[test]
    fn an_api_key_becomes_a_request_header() {
        let connection = OctoPrintConnection::new(config(false), Some("abc123".to_string()));
        let request = connection.get("/api/version").build().unwrap();
        assert_eq!(request.headers()["X-Api-Key"], "abc123");
    }

    #[test]
    fn no_api_key_means_no_header() {
        // Access-control-disabled OctoPrint instances need no key at all;
        // sending an empty one would be rejected where sending none is
        // accepted (mirrors Moonraker's `no_api_key_means_no_header`).
        let connection = OctoPrintConnection::new(config(false), None);
        let request = connection.get("/api/version").build().unwrap();
        assert!(request.headers().get("X-Api-Key").is_none());
    }
}
```

- [ ] **Step 2: Run them to verify they fail**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml octoprint::tests`
Expected: FAIL — `cannot find function base_url in this scope`.

- [ ] **Step 3: Implement the adapter**

Prepend to `src-tauri/src/connections/octoprint/mod.rs`, above `pub mod protocol;`:

```rust
//! The OctoPrint adapter: plain REST over HTTP(S) with an `X-Api-Key`
//! header.
//!
//! Deliberately thin, like `moonraker::mod`. All response parsing lives in
//! `protocol`, which has no I/O and therefore real test coverage. What is
//! left here is the HTTP client, the polling loop, and status
//! classification.
//!
//! Unlike Moonraker, OctoPrint does not push — `subscribe()` drives its own
//! interval and feeds the same `mpsc::Sender<PrinterStatus>` a WebSocket
//! adapter would. This is the case the phase-3 spec called out as most
//! likely to stress the `PrinterConnection` trait; it did not need to
//! change (see this plan's self-review).
//!
//! This adapter implements NO retry logic. The supervisor owns
//! reconnect-with-backoff; `subscribe` returning at all — `Ok` or `Err` — is
//! its cue to reconnect. A single failed poll tick therefore ends the whole
//! stream rather than being retried in place; the supervisor resets its
//! backoff to 1s on every clean `subscribe()` exit, so a genuinely
//! transient blip costs one brief "Connecting" flicker, not a long outage.

use crate::connections::{
    ConnectionConfig, ConnectionError, PrinterConnection, PrinterStatus, ProbeResult,
};
use reqwest::{Client, Response};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

pub mod protocol;

use protocol::{probe_result_from, status_from};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
/// Fast enough to read as live on the dashboard, an order of magnitude below
/// Moonraker's push cadence, and comfortably below anything that would
/// trouble OctoPrint's Flask-based server. No documented rate limit exists
/// to size against instead.
const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct OctoPrintConnection {
    config: ConnectionConfig,
    api_key: Option<String>,
    /// Built once and reused: a `reqwest::Client` holds its own connection
    /// pool, so building a fresh one per request (as a stateless adapter
    /// would need to) would throw that pooling away on every poll tick.
    client: Client,
}

impl OctoPrintConnection {
    pub fn new(config: ConnectionConfig, api_key: Option<String>) -> Self {
        let client = Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .expect("a Client with only a timeout configured should never fail to build");
        Self { config, api_key, client }
    }

    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        let mut request = self.client.get(format!("{}{path}", base_url(&self.config)));
        if let Some(key) = self.api_key.as_deref().filter(|k| !k.is_empty()) {
            request = request.header("X-Api-Key", key);
        }
        request
    }

    async fn get_json(&self, path: &str) -> Result<Value, ConnectionError> {
        let response = self.get(path).send().await.map_err(classify_transport)?;
        parse_json_response(response).await
    }

    /// `/api/printer` answers 409 when no printer is connected to OctoPrint —
    /// a normal, expected state, not a poll failure. `Ok(None)` is that case.
    async fn get_json_tolerating_409(&self, path: &str) -> Result<Option<Value>, ConnectionError> {
        let response = self.get(path).send().await.map_err(classify_transport)?;
        if response.status().as_u16() == 409 {
            return Ok(None);
        }
        parse_json_response(response).await.map(Some)
    }
}

pub fn base_url(config: &ConnectionConfig) -> String {
    let scheme = if config.use_tls { "https" } else { "http" };
    format!("{scheme}://{}:{}", config.host, config.port)
}

fn classify_transport(error: reqwest::Error) -> ConnectionError {
    if error.is_timeout() {
        ConnectionError::Timeout
    } else {
        ConnectionError::Unreachable(error.to_string())
    }
}

/// Separating auth failure from unreachability is what lets the Connection
/// tab say "check the API key" instead of "check the address" — the same
/// distinction `moonraker::classify` draws. OctoPrint's docs specify 403 for
/// a missing/invalid key; 401 is classified the same way defensively, since
/// a reverse proxy in front of OctoPrint may use it instead.
async fn parse_json_response(response: Response) -> Result<Value, ConnectionError> {
    let status = response.status();
    if status.as_u16() == 401 || status.as_u16() == 403 {
        return Err(ConnectionError::Auth(format!("HTTP {status}")));
    }
    if !status.is_success() {
        return Err(ConnectionError::Protocol(format!("unexpected HTTP {status}")));
    }
    response.json::<Value>().await.map_err(|e| ConnectionError::Protocol(e.to_string()))
}

#[async_trait::async_trait]
impl PrinterConnection for OctoPrintConnection {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        let version = self.get_json("/api/version").await?;
        let connection = self.get_json("/api/connection").await?;
        let profiles = self.get_json("/api/printerprofiles").await?;
        Ok(probe_result_from(&version, &connection, &profiles))
    }

    async fn subscribe(&self, tx: Sender<PrinterStatus>) -> Result<(), ConnectionError> {
        let mut ticker = tokio::time::interval(POLL_INTERVAL);
        loop {
            ticker.tick().await;
            let job = self.get_json("/api/job").await?;
            let printer = self.get_json_tolerating_409("/api/printer?exclude=sd").await?;
            let status = status_from(&job, printer.as_ref());
            // A closed receiver means the supervisor dropped this printer.
            // Ending cleanly beats logging into the void.
            if tx.send(status).await.is_err() {
                return Ok(());
            }
        }
    }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml octoprint`
Expected: 11 passed (7 from Task 2 + 4 here).

- [ ] **Step 5: Commit**

```bash
git add src-tauri/src/connections/octoprint/mod.rs
git commit -m "feat: add the OctoPrint REST adapter"
```

---

### Task 4: Register the adapter with the supervisor and commands

The only two places a connection `kind` is dispatched. Both are a one-arm addition to an existing `match`.

**Files:**
- Modify: `src-tauri/src/connections/supervisor.rs`
- Modify: `src-tauri/src/connections/commands.rs`

**Interfaces:**
- Consumes: `OctoPrintConnection::new` (Task 3), `OCTOPRINT_KIND` (Task 1).
- Produces: nothing new — this task only wires existing pieces together. Every command, the reconnect-on-launch path in `lib.rs`, and discovery already dispatch generically over `config.kind` and need no changes.

- [ ] **Step 1: Write the failing test**

Add to the `#[cfg(test)] mod tests` block in `src-tauri/src/connections/supervisor.rs`. `build` is private, so this test lives in the same module rather than needing anything public:

```rust
    #[test]
    fn build_recognizes_octoprint() {
        let config = ConnectionConfig {
            kind: crate::connections::OCTOPRINT_KIND.to_string(),
            host: "octoprint.local".to_string(),
            port: crate::connections::DEFAULT_OCTOPRINT_PORT,
            use_tls: false,
            credential_ref: None,
        };
        assert!(build(&config, None).is_some());
    }
```

- [ ] **Step 2: Run it to verify it fails**

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml supervisor::tests::build_recognizes_octoprint`
Expected: FAIL — every `kind` falls through to `_ => None`, so `build(&config, None)` is `None`.

- [ ] **Step 3: Register OctoPrint in the supervisor**

In `src-tauri/src/connections/supervisor.rs`, change:

```rust
use super::moonraker::MoonrakerConnection;
use super::{ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND};
```

to:

```rust
use super::moonraker::MoonrakerConnection;
use super::octoprint::OctoPrintConnection;
use super::{
    ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND,
    OCTOPRINT_KIND,
};
```

And change:

```rust
fn build(config: &ConnectionConfig, api_key: Option<String>) -> Option<Box<dyn PrinterConnection>> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Some(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        _ => None,
    }
}
```

to:

```rust
fn build(config: &ConnectionConfig, api_key: Option<String>) -> Option<Box<dyn PrinterConnection>> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Some(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        OCTOPRINT_KIND => Some(Box::new(OctoPrintConnection::new(config.clone(), api_key))),
        _ => None,
    }
}
```

- [ ] **Step 4: Register OctoPrint in the commands module**

Add the identical test shape to `commands.rs`'s test module first. Using the fully-qualified constant path here (rather than importing `OCTOPRINT_KIND` yet, which the `use` change below adds) keeps the failure in this step specifically about dispatch, not name resolution:

```rust
    #[test]
    fn adapter_recognizes_octoprint() {
        let config = ConnectionConfig {
            kind: crate::connections::OCTOPRINT_KIND.to_string(),
            host: "octoprint.local".to_string(),
            port: 80,
            use_tls: false,
            credential_ref: None,
        };
        assert!(adapter(&config, None).is_ok());
    }
```

Run: `source "$HOME/.cargo/env" && cargo test --manifest-path src-tauri/Cargo.toml commands::tests::adapter_recognizes_octoprint`
Expected: FAIL — `"octoprint"` falls into `other => Err(...)`.

Then change:

```rust
use super::moonraker::MoonrakerConnection;
```

to:

```rust
use super::moonraker::MoonrakerConnection;
use super::octoprint::OctoPrintConnection;
```

and:

```rust
use super::{ConnectionConfig, PrinterConnection, PrinterStatus, ProbeResult, MOONRAKER_KIND};
```

to:

```rust
use super::{
    ConnectionConfig, PrinterConnection, PrinterStatus, ProbeResult, MOONRAKER_KIND,
    OCTOPRINT_KIND,
};
```

and:

```rust
fn adapter(
    config: &ConnectionConfig,
    api_key: Option<String>,
) -> Result<Box<dyn PrinterConnection>, String> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Ok(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        other => Err(format!("This build cannot speak `{other}` connections")),
    }
}
```

to:

```rust
fn adapter(
    config: &ConnectionConfig,
    api_key: Option<String>,
) -> Result<Box<dyn PrinterConnection>, String> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Ok(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        OCTOPRINT_KIND => Ok(Box::new(OctoPrintConnection::new(config.clone(), api_key))),
        other => Err(format!("This build cannot speak `{other}` connections")),
    }
}
```

- [ ] **Step 5: Run the full Rust suite**

Run: `just test-rust`
Expected: all tests pass, including both new `*_recognizes_octoprint` tests.

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/connections/supervisor.rs src-tauri/src/connections/commands.rs
git commit -m "feat: register the OctoPrint adapter with the supervisor and commands"
```

---

### Task 5: Offer OctoPrint as a Connection kind in the UI

The Connection tab, discovery, credential store, and the TypeScript `ProbeResult`/`PrinterStatus`/`ConnectionSubmission` types are all already generic over `kind: string` — confirmed by reading `PrinterConnectionPanel.tsx`, `printer-store.ts`, and `types.ts` before writing this plan. The only frontend change is the static list of kinds the Select offers; `DEFAULT_PORTS` already has an `octoprint: 80` entry from phase 2.

**Files:**
- Modify: `src/screens/PrinterConnectionPanel.tsx`
- Modify: `src/screens/PrinterConnectionPanel.test.tsx`

**Interfaces:** none new — reuses `ConnectionSubmission`, `ProbeResult`, `ReportedCapabilities` (existing).

- [ ] **Step 1: Write the failing test**

Add to `describe("PrinterConnectionPanel", ...)` in `src/screens/PrinterConnectionPanel.test.tsx`, beside the existing `"defaults the kind from the catalog's suggestedHostType"` test:

```tsx
  it("offers OctoPrint as a selectable kind", async () => {
    const suggested = {
      ...printer,
      profile: { ...PROFILE, suggestedHostType: "octoprint" },
    } as unknown as ResolvedPrinter;
    render(() => <PrinterConnectionPanel printer={suggested} />);
    expect(await screen.findByRole("button", { name: /OctoPrint/ })).toBeInTheDocument();
  });
```

- [ ] **Step 2: Run it to verify it fails**

Run: `just test` (or `npx vitest run src/screens/PrinterConnectionPanel.test.tsx`)
Expected: FAIL — the Select has no option matching `kind() === "octoprint"`, so no button renders with that name.

- [ ] **Step 3: Add OctoPrint to the kind list**

In `src/screens/PrinterConnectionPanel.tsx`, change:

```tsx
/** Only what this build can actually speak. Phase 3 appends OctoPrint and
 *  ElegooLink here as each adapter lands — listing them now as disabled
 *  entries would need a prop `Select` does not have (verified: `SelectProps`
 *  exposes no `optionDisabled`), and offering a kind that errors on save is
 *  worse than not offering it. */
const KINDS = [{ value: "moonraker", label: "Moonraker (Klipper)" }];
```

to:

```tsx
/** Only what this build can actually speak. ElegooLink appends here once
 *  its protocol spike (see
 *  docs/superpowers/specs/2026-08-20-printer-adapters-design.md) is
 *  complete — listing it now as a disabled entry would need a prop `Select`
 *  does not have (verified: `SelectProps` exposes no `optionDisabled`), and
 *  offering a kind that errors on save is worse than not offering it. */
const KINDS = [
  { value: "moonraker", label: "Moonraker (Klipper)" },
  { value: "octoprint", label: "OctoPrint" },
];
```

- [ ] **Step 4: Run the frontend suite**

Run: `just test`
Expected: all tests pass, including the new one.

- [ ] **Step 5: Run the full build**

Run: `just build`
Expected: tsc + vite build succeed with no errors.

- [ ] **Step 6: Commit**

```bash
git add src/screens/PrinterConnectionPanel.tsx src/screens/PrinterConnectionPanel.test.tsx
git commit -m "feat: offer OctoPrint as a selectable Connection kind"
```

---

## End-to-end verification

Automated tests cannot cover the parts that need a real printer. Per `AGENTS.md`, a passing suite does not mean the UI renders correctly — run this before considering the phase done.

**Against a real OctoPrint instance** (a Raspberry Pi running OctoPi, or `pip install octoprint` pointed at a virtual/simulated printer works for everything except the temperature and job checks):

1. `just dev`. Add a printer whose catalog variant has `suggestedHostType: "octoprint"` (or any printer — the kind selector now offers OctoPrint regardless), select it, open **Connection**.
2. Confirm **Discovered on this network** lists the instance — OctoPrint's bundled Discovery plugin advertises `_octoprint._tcp.local.` by default. If it does not, continue with manual entry, exactly as the phase-2 checklist requires for Moonraker.
3. Generate an API key in OctoPrint's Settings → API page. Enter host and port, paste the key, click **Test connection**. Confirm the probe renders OctoPrint's own version, the active printer profile's name, and the connection state (e.g. `"Operational"`).
4. Confirm the build-volume cross-check: silent for a correctly-matched printer. Deliberately add a second printer bound to a *different* catalog variant, point it at the same host, confirm the mismatch list appears.
5. **Save.** Confirm the card badge goes `connecting` → `online` and that temperatures appear and tick roughly every 2 seconds.
6. **The percentage-conversion check — the one thing worth doing by hand.** Start a print (or a simulated one). Confirm the progress bar's fraction matches what OctoPrint's own web UI reports as a percentage divided by 100 — not the raw percentage value itself.
7. **The 409 check.** In OctoPrint's own UI, disconnect the printer (Connection panel → Disconnect) without stopping OctoPrint itself. Confirm farm3d's temperatures blank to an em dash rather than the connection showing an error, and that `jobState` reflects an `"Offline"`-prefixed string.
8. Stop the OctoPrint service (or block its port). Confirm the badge goes to `error`/`offline` and that reconnect attempts back off rather than hammering; restart it and confirm recovery without restarting farm3d.
9. Restart farm3d. Confirm the printer reconnects on launch without opening its Connection tab — this path is already generic over `kind` (`lib.rs`), so this mainly confirms nothing here broke it.
10. Enter a wrong API key against an access-control-enabled instance; confirm the error is described as a credentials problem, not a reachability one.

**Without a printer** (still worth doing): point a connection at a closed port and confirm the error says *reachability*, not credentials; then at a host running some other HTTP service (e.g. a plain web server) and confirm the error is distinguishable — a non-JSON response should surface as a protocol error, not silently succeed.

---

## Notes for the ElegooLink spike

Record these against `docs/superpowers/specs/2026-08-20-printer-adapters-design.md` when that spike runs:

- The `PrinterConnection` trait, `ProbeResult`, `PrinterStatus`, and `ReportedCapabilities` needed **zero changes** for OctoPrint, despite the spec calling this phase "most likely to stress-test" the trait (its polling shape fit the same `mpsc::Sender` `subscribe()` a push adapter uses). That is one data point, not a guarantee ElegooLink needs none either — SDCP is a different transport (WebSocket) and, per the spec, potentially a thin wrapper over genuine Klipper firmware, which is a different kind of pressure on the trait than REST polling was.
- If the spike confirms Elegoo's Centauri Carbon line runs real Klipper behind SDCP, re-read `moonraker::protocol`'s `StatusSnapshot` before assuming ElegooLink needs its own from-scratch status accumulator — the partial-update merge problem may recur verbatim.
- `commands::adapter` and `supervisor::build` are still the only two registration points; confirm that remains true (a grep, not an assumption) before writing the ElegooLink plan.

---

## Self-review

Checked against the spec after writing:

**Spec coverage.** OctoPrint's three settled requirements from
`2026-08-20-printer-adapters-design.md` are each covered: REST with
`X-Api-Key` implementing the existing trait (Tasks 1–3), registration at
exactly the two dispatch points the phase-2 notes named (Task 4), and the UI
offering it as a kind (Task 5). ElegooLink is explicitly left out, per the
spec's spike gate.

**No trait revision needed.** Unlike phase 2 (which revised its own spec
three times while implementing), this plan needed no correction against the
now-resolved OctoPrint wire contract — because that contract was resolved
*in the spec itself*, against verified documentation, before this plan was
written, rather than discovered during implementation. The one deliberate
choice worth restating: `probe()` reads the active printer profile via the
list endpoint (`GET /api/printerprofiles`) rather than the single-profile
endpoint, specifically because the latter's response shape is not shown in
OctoPrint's docs. This is a decision already recorded in the spec, not a
plan-vs-spec discrepancy.

**Type consistency.** `ProbeResult` and `PrinterStatus` field names and
types are unchanged from phase 2 and used identically by
`octoprint::protocol` and `moonraker::protocol`. `OCTOPRINT_KIND`/
`DEFAULT_OCTOPRINT_PORT` (Task 1) are the exact names Tasks 2–4 import.
`base_url` mirrors `websocket_url`'s signature (`&ConnectionConfig ->
String`); `OctoPrintConnection::new` mirrors `MoonrakerConnection::new`'s
`(ConnectionConfig, Option<String>) -> Self`.

**One risk this plan does not eliminate.** As with phase 2, nothing here has
spoken to a live OctoPrint instance — the wire contract is
documentation-derived. The detail most worth confirming in the end-to-end
checklist's step 6 is the `progress.completion` percentage-vs-fraction
question: it was resolved from the data-model page's field description
rather than the (ambiguous, possibly stale) example value on the endpoint
page itself, and is exactly the kind of thing that reads correctly in a code
review and wrong against a real printer.
