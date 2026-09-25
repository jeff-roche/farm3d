//! OctoPrint's REST JSON shapes — PURE. No HTTP client, no async, no Tauri.
//! Mirrors `moonraker::protocol`: the risky mapping logic lives here, where
//! it is testable without a printer or a network client.
//!
//! Every function reads `serde_json::Value`s rather than typed response
//! structs so a field OctoPrint omits, nulls, or renames degrades to "no
//! reading" instead of failing the whole poll.

use crate::connections::status_repository::PrinterTelemetry;
use crate::connections::{ConnectionState, ProbeResult, ReportedCapabilities};
use crate::printers::operational::HostActivity;
use serde_json::Value;

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// A string that is present AND non-empty. OctoPrint reports "no job" as
/// `null` on current versions, but an empty string must not become a label
/// either.
fn non_empty(value: &Value) -> Option<String> {
    value
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_string)
}

/// `GET /api/connection`'s `current.printerProfile` names the active
/// profile's id in `GET /api/printerprofiles`'s `profiles` map. Looking it up
/// this way — rather than calling the single-profile endpoint
/// (`GET /api/printerprofiles/<id>`) — relies only on a response shape
/// OctoPrint's docs show a concrete worked example for.
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
        // `current.state` is already self-describing (e.g. "Offline after
        // error"), so there is no separate message field the way Moonraker's
        // `printer.info.state_message` supplies one.
        state_message: String::new(),
        reported: ReportedCapabilities {
            bed_width_mm: positive(&profile["volume"]["width"]),
            bed_depth_mm: positive(&profile["volume"]["depth"]),
            printable_height_mm: positive(&profile["volume"]["height"]),
        },
    }
}

/// A profile dimension of zero is OctoPrint's "unset", not a real volume;
/// reporting it would make the build-volume cross-check cry wolf.
fn positive(value: &Value) -> Option<f64> {
    value.as_f64().filter(|mm| *mm > 0.0)
}

/// OctoPrint has no clean state enum the way Moonraker's `klippy_state`
/// does; this maps `GET /api/job`'s free-text `state` (the strings
/// `octoprint.util.comm.MachineCom.getStateString` produces) onto farm3d's
/// connection vocabulary.
///
/// The HTTP request succeeding already proves OctoPrint itself is up, so
/// these states describe OctoPrint's serial link to the printer — the same
/// layer Moonraker's "Klippy down behind a healthy socket" describes.
pub fn connection_state_from_job_state(state: &str) -> ConnectionState {
    if state.starts_with("Offline after error") || state.starts_with("Error") {
        ConnectionState::Error
    } else if state.starts_with("Offline") || state.starts_with("Closed") {
        ConnectionState::Offline
    } else if state.starts_with("Opening serial")
        || state.starts_with("Detecting")
        || state.starts_with("Connecting")
    {
        ConnectionState::Connecting
    } else {
        ConnectionState::Online
    }
}

/// Converts OctoPrint job-state strings before they cross the adapter
/// boundary, the counterpart of `moonraker::protocol::normalize_host_activity`.
/// Anything not recognized is `Unknown` — never guessed to be `Idle`, which
/// would claim the printer is ready for work.
pub fn normalize_host_activity(state: &str) -> HostActivity {
    match state {
        "Operational" => HostActivity::Idle,
        "Paused" => HostActivity::Paused,
        // "Printing" and "Printing from SD".
        s if s.starts_with("Printing") => HostActivity::Printing,
        // Transitional states: the printer is occupied but neither printing
        // nor settled into a pause.
        "Pausing" | "Resuming" | "Cancelling" | "Finishing" => HostActivity::Busy,
        s if s.starts_with("Starting")
            || s.starts_with("Sending file")
            || s.starts_with("Transferring file") =>
        {
            HostActivity::Busy
        }
        _ => HostActivity::Unknown,
    }
}

/// The dashboard telemetry for one poll tick.
///
/// `printer` is `None` when `GET /api/printer` answered 409 (OctoPrint is up
/// but no printer is connected to it) — a normal state, not an error, so
/// this still produces telemetry with every temperature `None` rather than
/// refusing to build one.
pub fn telemetry_from(job: &Value, printer: Option<&Value>) -> PrinterTelemetry {
    let host_activity_name = non_empty(&job["state"]);
    let temperature = printer.map(|printer| &printer["temperature"]);
    let reading = |heater: &str, field: &str| temperature.and_then(|t| t[heater][field].as_f64());
    PrinterTelemetry {
        host_activity: host_activity_name
            .as_deref()
            .map(normalize_host_activity)
            .unwrap_or(HostActivity::Unknown),
        host_activity_name,
        job_name: non_empty(&job["job"]["file"]["display"])
            .or_else(|| non_empty(&job["job"]["file"]["name"])),
        // `progress.completion` is a 0-100 PERCENTAGE; farm3d's `progress`
        // is 0.0..=1.0. Skipping the /100.0 here would render every progress
        // bar 100x too full. Clamped because OctoPrint computes it from the
        // file position and can overshoot by a rounding hair at the end.
        progress: job["progress"]["completion"]
            .as_f64()
            .map(|percent| (percent / 100.0).clamp(0.0, 1.0)),
        nozzle_temp_c: reading("tool0", "actual"),
        nozzle_target_c: reading("tool0", "target"),
        // `bed` is absent entirely on a printer profile with no heated bed —
        // a normal state, not a missing reading to warn about.
        bed_temp_c: reading("bed", "actual"),
        bed_target_c: reading("bed", "target"),
        // Multi-tool OctoPrint (tool1..toolN) is a follow-up; tool0 is
        // reported through the nozzle fields above.
        tools: Vec::new(),
        print_duration_s: job["progress"]["printTime"].as_f64(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // Fixtures follow the response examples in OctoPrint's REST API docs
    // (docs.octoprint.org/en/master/api/), with values changed to be
    // distinguishable from one another.

    fn version() -> Value {
        serde_json::json!({"api": "0.1", "server": "1.10.3", "text": "OctoPrint 1.10.3"})
    }

    fn connection(state: &str) -> Value {
        serde_json::json!({
            "current": {
                "state": state, "port": "/dev/ttyACM0", "baudrate": 250000,
                "printerProfile": "_default"
            },
            "options": {"ports": ["/dev/ttyACM0", "VIRTUAL"], "baudrates": [250000]}
        })
    }

    fn profiles() -> Value {
        serde_json::json!({"profiles": {"_default": {
            "id": "_default", "name": "Voron 2.4", "model": "Generic",
            "default": true, "current": true, "heatedBed": true,
            "volume": {
                "formFactor": "rectangular", "origin": "lowerleft",
                "width": 250.0, "depth": 210.0, "height": 220.0
            }
        }}})
    }

    fn job(state: &str, name: Value, completion: Value, print_time: Value) -> Value {
        serde_json::json!({
            "job": {
                "file": {"name": name, "origin": "local", "size": 1468987, "date": 1378847754},
                "estimatedPrintTime": 8811,
                "filament": {"tool0": {"length": 810, "volume": 5.36}}
            },
            "progress": {
                "completion": completion, "filepos": 337942,
                "printTime": print_time, "printTimeLeft": 912
            },
            "state": state
        })
    }

    fn printing(completion: f64) -> Value {
        job(
            "Printing",
            serde_json::json!("benchy.gcode"),
            serde_json::json!(completion),
            serde_json::json!(300),
        )
    }

    fn idle() -> Value {
        job("Operational", Value::Null, Value::Null, Value::Null)
    }

    fn printer_with_both_heaters() -> Value {
        serde_json::json!({
            "temperature": {
                "tool0": {"actual": 210.1, "target": 215.0, "offset": 0},
                "bed": {"actual": 60.5, "target": 60.0, "offset": 0}
            },
            "state": {"text": "Printing", "flags": {"operational": true, "printing": true}}
        })
    }

    #[test]
    fn probe_reads_version_state_and_active_profile_volume() {
        let probe = probe_result_from(&version(), &connection("Operational"), &profiles());
        assert_eq!(probe.kind, "octoprint");
        assert_eq!(probe.host_software, "1.10.3");
        assert_eq!(probe.firmware, "");
        assert_eq!(probe.reported_name, "Voron 2.4");
        assert_eq!(probe.state, "Operational");
        assert_eq!(probe.state_message, "");
        assert_eq!(probe.reported.bed_width_mm, Some(250.0));
        assert_eq!(probe.reported.bed_depth_mm, Some(210.0));
        assert_eq!(probe.reported.printable_height_mm, Some(220.0));
    }

    #[test]
    fn probe_tolerates_an_unknown_active_profile_id_rather_than_panicking() {
        let probe = probe_result_from(
            &version(),
            &connection("Closed"),
            &serde_json::json!({"profiles": {}}),
        );
        assert_eq!(probe.reported, ReportedCapabilities::default());
        assert_eq!(probe.reported_name, "");
        assert_eq!(probe.state, "Closed");
    }

    #[test]
    fn probe_treats_a_zero_dimension_as_unreported() {
        let mut profiles = profiles();
        profiles["profiles"]["_default"]["volume"]["height"] = serde_json::json!(0);
        let probe = probe_result_from(&version(), &connection("Operational"), &profiles);
        assert_eq!(probe.reported.printable_height_mm, None);
        assert_eq!(probe.reported.bed_width_mm, Some(250.0));
    }

    #[test]
    fn serial_link_states_map_to_connection_states() {
        for (state, expected) in [
            ("Operational", ConnectionState::Online),
            ("Printing", ConnectionState::Online),
            ("Printing from SD", ConnectionState::Online),
            ("Paused", ConnectionState::Online),
            ("Offline", ConnectionState::Offline),
            ("Closed", ConnectionState::Offline),
            ("Offline after error", ConnectionState::Error),
            ("Error", ConnectionState::Error),
            (
                "Error: Too many consecutive timeouts",
                ConnectionState::Error,
            ),
            ("Opening serial connection", ConnectionState::Connecting),
            ("Detecting serial connection", ConnectionState::Connecting),
            ("Connecting", ConnectionState::Connecting),
        ] {
            assert_eq!(connection_state_from_job_state(state), expected, "{state}");
        }
    }

    #[test]
    fn job_states_normalize_to_host_activity_inside_the_adapter_boundary() {
        for (state, expected) in [
            ("Operational", HostActivity::Idle),
            ("Printing", HostActivity::Printing),
            ("Printing from SD", HostActivity::Printing),
            ("Paused", HostActivity::Paused),
            ("Pausing", HostActivity::Busy),
            ("Resuming", HostActivity::Busy),
            ("Cancelling", HostActivity::Busy),
            ("Finishing", HostActivity::Busy),
            ("Starting print from SD", HostActivity::Busy),
            ("Sending file to SD", HostActivity::Busy),
            ("Offline", HostActivity::Unknown),
            ("Error", HostActivity::Unknown),
            ("Some future OctoPrint state", HostActivity::Unknown),
        ] {
            assert_eq!(normalize_host_activity(state), expected, "{state}");
        }
    }

    #[test]
    fn completion_percentage_is_converted_to_a_zero_to_one_fraction() {
        // THE bug this module exists to prevent. OctoPrint's `completion` is
        // 0-100; farm3d's `progress` is 0.0..=1.0.
        let telemetry = telemetry_from(&printing(42.5), None);
        assert_eq!(telemetry.progress, Some(0.425));
        assert_eq!(telemetry.job_name, Some("benchy.gcode".to_string()));
        assert_eq!(telemetry.print_duration_s, Some(300.0));
        assert_eq!(telemetry.host_activity, HostActivity::Printing);
        assert_eq!(telemetry.host_activity_name, Some("Printing".to_string()));
    }

    #[test]
    fn a_finished_job_reads_as_exactly_complete() {
        assert_eq!(telemetry_from(&printing(100.0), None).progress, Some(1.0));
        assert_eq!(
            telemetry_from(&printing(100.0000001), None).progress,
            Some(1.0)
        );
    }

    #[test]
    fn an_idle_printer_with_null_job_fields_reports_no_job() {
        // OctoPrint answers `/api/job` with nulls, not absent keys, when no
        // file is selected.
        let telemetry = telemetry_from(&idle(), Some(&printer_with_both_heaters()));
        assert_eq!(telemetry.job_name, None);
        assert_eq!(telemetry.progress, None);
        assert_eq!(telemetry.print_duration_s, None);
        assert_eq!(telemetry.host_activity, HostActivity::Idle);
    }

    #[test]
    fn a_409_from_api_printer_reports_no_temperatures_rather_than_erroring() {
        let offline = job("Offline", Value::Null, Value::Null, Value::Null);
        let telemetry = telemetry_from(&offline, None);
        assert_eq!(telemetry.nozzle_temp_c, None);
        assert_eq!(telemetry.nozzle_target_c, None);
        assert_eq!(telemetry.bed_temp_c, None);
        assert_eq!(telemetry.bed_target_c, None);
        assert_eq!(telemetry.host_activity_name, Some("Offline".to_string()));
    }

    #[test]
    fn temperatures_are_read_when_the_printer_answers() {
        let telemetry = telemetry_from(&printing(10.0), Some(&printer_with_both_heaters()));
        assert_eq!(telemetry.nozzle_temp_c, Some(210.1));
        assert_eq!(telemetry.nozzle_target_c, Some(215.0));
        assert_eq!(telemetry.bed_temp_c, Some(60.5));
        assert_eq!(telemetry.bed_target_c, Some(60.0));
    }

    #[test]
    fn a_profile_with_no_heated_bed_reports_no_bed_temperature() {
        let printer =
            serde_json::json!({"temperature": {"tool0": {"actual": 200.0, "target": 200.0}}});
        let telemetry = telemetry_from(&printing(5.0), Some(&printer));
        assert_eq!(telemetry.nozzle_temp_c, Some(200.0));
        assert_eq!(telemetry.bed_temp_c, None);
        assert_eq!(telemetry.bed_target_c, None);
    }

    #[test]
    fn an_empty_job_state_is_treated_as_absent_not_an_empty_label() {
        let telemetry = telemetry_from(&job("", Value::Null, Value::Null, Value::Null), None);
        assert_eq!(telemetry.host_activity_name, None);
        assert_eq!(telemetry.host_activity, HostActivity::Unknown);
    }

    #[test]
    fn the_display_name_is_preferred_over_the_on_disk_file_name() {
        // OctoPrint 1.3.10+ reports a `display` name for files uploaded with
        // characters its storage layer normalizes away.
        let mut job = printing(1.0);
        job["job"]["file"]["display"] = serde_json::json!("Benchy (fast).gcode");
        job["job"]["file"]["name"] = serde_json::json!("Benchy_fast.gcode");
        assert_eq!(
            telemetry_from(&job, None).job_name,
            Some("Benchy (fast).gcode".to_string())
        );
    }
}
