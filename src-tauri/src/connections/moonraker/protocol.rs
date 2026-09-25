//! Moonraker's JSON-RPC 2.0 framing and status accumulation — PURE. No
//! sockets, no async, no Tauri. Everything here is a function of its input,
//! which is what makes the merge semantics below testable without a printer.

use crate::connections::status_repository::PrinterTelemetry;
use crate::connections::{ConnectionState, ProbeResult, ReportedCapabilities};
use crate::printers::operational::HostActivity;
use serde_json::{json, Value};

/// One decoded inbound frame. Anything farm3d does not act on decodes to
/// `Ignored` rather than an error: Moonraker emits many notifications
/// (`notify_gcode_response`, `notify_proc_stat_update`, …) that are not this
/// phase's business, and an adapter that errors on them is an adapter that
/// disconnects constantly.
#[derive(Debug)]
pub enum Frame {
    Response {
        id: u64,
        result: Value,
    },
    Error {
        id: u64,
        message: String,
        code: Option<i64>,
    },
    StatusUpdate(Value),
    KlippyReady,
    KlippyDown,
    Ignored,
}

pub fn rpc_request(id: u64, method: &str, params: Option<Value>) -> String {
    let mut req = json!({"jsonrpc": "2.0", "method": method, "id": id});
    if let Some(params) = params {
        req["params"] = params;
    }
    req.to_string()
}

/// Exactly the objects and attributes the dashboard renders. Requesting
/// named attributes rather than `null` (which means "every attribute") keeps
/// the notification volume proportional to what is actually displayed.
pub fn subscribe_params() -> Value {
    json!({"objects": {
        "extruder": ["temperature", "target"],
        "heater_bed": ["temperature", "target"],
        "print_stats": ["filename", "state", "print_duration", "message"],
        "display_status": ["progress"],
        "toolhead": ["axis_minimum", "axis_maximum"]
    }})
}

pub fn parse_frame(raw: &str) -> Frame {
    let Ok(value) = serde_json::from_str::<Value>(raw) else {
        return Frame::Ignored;
    };

    if let Some(method) = value.get("method").and_then(Value::as_str) {
        return match method {
            // Params are POSITIONAL: [status_object, eventtime]. Moonraker
            // documents that notification params are always an array.
            "notify_status_update" => value
                .get("params")
                .and_then(Value::as_array)
                .and_then(|p| p.first())
                .cloned()
                .map(Frame::StatusUpdate)
                .unwrap_or(Frame::Ignored),
            "notify_klippy_ready" => Frame::KlippyReady,
            // Both mean "the firmware is not usable right now" even though
            // our socket to Moonraker is perfectly healthy.
            "notify_klippy_shutdown" | "notify_klippy_disconnected" => Frame::KlippyDown,
            _ => Frame::Ignored,
        };
    }

    let Some(id) = value.get("id").and_then(Value::as_u64) else {
        return Frame::Ignored;
    };
    if let Some(error) = value.get("error") {
        return Frame::Error {
            id,
            message: error
                .get("message")
                .and_then(Value::as_str)
                .unwrap_or("unknown error")
                .to_string(),
            code: error.get("code").and_then(Value::as_i64),
        };
    }
    match value.get("result") {
        Some(result) => Frame::Response {
            id,
            result: result.clone(),
        },
        None => Frame::Ignored,
    }
}

/// The running merge of every status object Moonraker has reported.
///
/// Moonraker's `notify_status_update` carries ONLY the fields that changed.
/// This type is the reason that is safe: each notification is folded into the
/// accumulated tree, per-object and per-attribute, and `to_status` reads the
/// accumulated result. Replacing the tree per notification is the bug.
#[derive(Default, Debug, Clone)]
pub struct StatusSnapshot {
    objects: serde_json::Map<String, Value>,
}

impl StatusSnapshot {
    pub fn merge(&mut self, update: &Value) {
        let Some(update) = update.as_object() else {
            return;
        };
        for (object_name, attributes) in update {
            let Some(attributes) = attributes.as_object() else {
                continue;
            };
            let entry = self
                .objects
                .entry(object_name.clone())
                .or_insert_with(|| Value::Object(serde_json::Map::new()));
            let Some(entry) = entry.as_object_mut() else {
                continue;
            };
            for (attribute, value) in attributes {
                entry.insert(attribute.clone(), value.clone());
            }
        }
    }

    fn is_empty(&self) -> bool {
        self.objects.is_empty()
    }

    fn number(&self, object: &str, attribute: &str) -> Option<f64> {
        self.objects.get(object)?.get(attribute)?.as_f64()
    }

    /// An empty string is Klipper's "nothing here" (an idle printer reports
    /// `filename: ""`), so it reads as absent.
    fn string(&self, object: &str, attribute: &str) -> Option<String> {
        let value = self.objects.get(object)?.get(attribute)?.as_str()?;
        (!value.is_empty()).then(|| value.to_string())
    }

    /// Reads one axis extent. The arrays are `[x, y, z, e]` where the 4th
    /// element is always zero and formally deprecated — index by position,
    /// never by length.
    fn axis_span(&self, index: usize) -> Option<f64> {
        let toolhead = self.objects.get("toolhead")?;
        let min = toolhead
            .get("axis_minimum")?
            .as_array()?
            .get(index)?
            .as_f64()?;
        let max = toolhead
            .get("axis_maximum")?
            .as_array()?
            .get(index)?
            .as_f64()?;
        Some(max - min)
    }

    pub fn reported_capabilities(&self) -> ReportedCapabilities {
        ReportedCapabilities {
            bed_width_mm: self.axis_span(0),
            bed_depth_mm: self.axis_span(1),
            printable_height_mm: self.axis_span(2),
        }
    }

    pub fn to_telemetry(&self) -> PrinterTelemetry {
        let host_activity_name = self.string("print_stats", "state");
        PrinterTelemetry {
            host_activity: host_activity_name
                .as_deref()
                .map(normalize_host_activity)
                .unwrap_or(HostActivity::Unknown),
            host_activity_name,
            job_name: self.string("print_stats", "filename"),
            progress: self.number("display_status", "progress"),
            nozzle_temp_c: self.number("extruder", "temperature"),
            nozzle_target_c: self.number("extruder", "target"),
            bed_temp_c: self.number("heater_bed", "temperature"),
            bed_target_c: self.number("heater_bed", "target"),
            print_duration_s: self.number("print_stats", "print_duration"),
        }
    }
}

/// Whether a JSON-RPC error means Moonraker rejected the credential.
///
/// Moonraker accepts every WebSocket upgrade and rejects requests one by one.
/// Its JSON-RPC layer rewrites HTTP 401 to -32602 ("Invalid params") but
/// keeps the message "Unauthorized" (seen in A0.1, #9), so the message is
/// what tells a rejected key from a genuinely bad parameter.
pub fn is_auth_error(code: Option<i64>, message: &str) -> bool {
    matches!(code, Some(401) | Some(403))
        || (code == Some(-32602) && message.trim() == "Unauthorized")
}

/// Request ids. Each connection sends at most one of each at a time, so fixed
/// ids are enough to route the responses.
pub const ID_SERVER_INFO: u64 = 1;
pub const ID_PRINTER_INFO: u64 = 2;
pub const ID_SUBSCRIBE: u64 = 3;

/// One thing the subscription loop must do after a frame.
#[derive(Debug, Clone, PartialEq)]
pub enum Step {
    /// Publish the accumulated telemetry.
    Telemetry,
    /// Publish the connection state.
    Health(ConnectionState),
    /// Send `printer.objects.subscribe` again.
    Resubscribe,
    /// Moonraker rejected the credential; end the subscription.
    AuthFailed(String),
}

/// The subscription's view of Klipper behind a Moonraker socket. PURE: it
/// maps each decoded frame to the `Step`s the I/O loop performs.
///
/// Live validation (A0.1, #9) found three facts this type encodes:
///
/// - Klipper can already be shut down when the socket opens. Then no
///   lifecycle notification ever arrives, so the starting state has to come
///   from `server.info`'s `klippy_state`. Assuming "online" reported a
///   shut-down printer as Ready.
/// - Moonraker drops every subscription when Klippy disconnects (a
///   FIRMWARE_RESTART, a host restart, a crash). A client has to subscribe
///   again on `notify_klippy_ready`, or its readings silently stop.
/// - Readings merge into the snapshot in every state, but they are published
///   only while Klipper is ready. The supervisor treats telemetry as proof of
///   an online printer, so publishing it during a shutdown would flip the
///   state back to online.
#[derive(Debug, Default)]
pub struct SubscriptionState {
    snapshot: StatusSnapshot,
    klippy: Option<ConnectionState>,
}

impl SubscriptionState {
    /// The state the liveness tick reports. `None` until Moonraker has said
    /// whether Klipper is ready.
    pub fn health(&self) -> Option<ConnectionState> {
        self.klippy
    }

    pub fn telemetry(&self) -> PrinterTelemetry {
        self.snapshot.to_telemetry()
    }

    fn ready(&self) -> bool {
        self.klippy == Some(ConnectionState::Online)
    }

    fn publish(&self) -> Vec<Step> {
        if self.ready() {
            vec![Step::Telemetry, Step::Health(ConnectionState::Online)]
        } else {
            Vec::new()
        }
    }

    pub fn on_frame(&mut self, frame: Frame) -> Vec<Step> {
        match frame {
            Frame::Response { id, result } if id == ID_SERVER_INFO => {
                let state = if result.get("klippy_state").and_then(Value::as_str) == Some("ready") {
                    ConnectionState::Online
                } else {
                    ConnectionState::Offline
                };
                self.klippy = Some(state);
                let mut steps = if self.snapshot.is_empty() {
                    Vec::new()
                } else {
                    self.publish()
                };
                if steps.is_empty() {
                    steps.push(Step::Health(state));
                }
                steps
            }
            Frame::Response { id, result } if id == ID_SUBSCRIBE => {
                // `printer.objects.subscribe` wraps its payload in a `status`
                // key; the notifications do not.
                self.snapshot.merge(&result["status"]);
                self.publish()
            }
            Frame::StatusUpdate(update) => {
                self.snapshot.merge(&update);
                self.publish()
            }
            Frame::KlippyReady => {
                self.klippy = Some(ConnectionState::Online);
                vec![Step::Resubscribe, Step::Health(ConnectionState::Online)]
            }
            Frame::KlippyDown => {
                self.klippy = Some(ConnectionState::Offline);
                vec![Step::Health(ConnectionState::Offline)]
            }
            Frame::Error { message, code, .. } if is_auth_error(code, &message) => {
                vec![Step::AuthFailed(message)]
            }
            // A subscribe sent while Klippy is disconnected fails with 503.
            // `notify_klippy_ready` triggers the next attempt.
            _ => Vec::new(),
        }
    }
}

/// Converts Moonraker activity strings before they cross the adapter boundary.
pub fn normalize_host_activity(value: &str) -> HostActivity {
    match value {
        "standby" | "ready" | "idle" => HostActivity::Idle,
        "printing" => HostActivity::Printing,
        "paused" => HostActivity::Paused,
        "busy" => HostActivity::Busy,
        "complete" => HostActivity::Finished,
        "cancelled" => HostActivity::Cancelled,
        "error" => HostActivity::Failed,
        _ => HostActivity::Unknown,
    }
}

fn str_field(value: &Value, key: &str) -> String {
    value
        .get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

pub fn probe_result_from(
    server_info: &Value,
    printer_info: &Value,
    snapshot: &StatusSnapshot,
) -> ProbeResult {
    ProbeResult {
        kind: crate::connections::MOONRAKER_KIND.to_string(),
        host_software: str_field(server_info, "moonraker_version"),
        firmware: str_field(printer_info, "software_version"),
        reported_name: str_field(printer_info, "hostname"),
        state: str_field(server_info, "klippy_state"),
        state_message: str_field(printer_info, "state_message"),
        reported: snapshot.reported_capabilities(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn snapshot_with_both_heaters() -> StatusSnapshot {
        let mut snapshot = StatusSnapshot::default();
        snapshot.merge(&serde_json::json!({
            "extruder": {"temperature": 200.5, "target": 210.0},
            "heater_bed": {"temperature": 60.1, "target": 60.0},
            "print_stats": {"filename": "benchy.gcode", "state": "printing", "print_duration": 42.0},
            "display_status": {"progress": 0.25}
        }));
        snapshot
    }

    #[test]
    fn a_partial_update_does_not_blank_the_other_fields() {
        // THE bug this module exists to prevent. Moonraker sends only what
        // changed; treating a notification as a whole snapshot wipes every
        // reading it omits.
        let mut snapshot = snapshot_with_both_heaters();
        snapshot.merge(&serde_json::json!({"extruder": {"temperature": 201.4}}));

        let telemetry = snapshot.to_telemetry();
        assert_eq!(telemetry.nozzle_temp_c, Some(201.4));
        assert_eq!(
            telemetry.bed_temp_c,
            Some(60.1),
            "bed temperature was blanked by a nozzle-only update"
        );
        assert_eq!(
            telemetry.nozzle_target_c,
            Some(210.0),
            "nozzle target was blanked by a temperature-only update"
        );
        assert_eq!(telemetry.job_name, Some("benchy.gcode".to_string()));
        assert_eq!(telemetry.progress, Some(0.25));
    }

    #[test]
    fn merging_an_unknown_object_is_ignored_rather_than_fatal() {
        // Moonraker instances expose different printer objects; an adapter
        // that errors on an unexpected key is an adapter that breaks on the
        // next Klipper release.
        let mut snapshot = snapshot_with_both_heaters();
        snapshot.merge(&serde_json::json!({"fan": {"speed": 1.0}, "mcu": {"last_stats": {}}}));
        assert_eq!(snapshot.to_telemetry().bed_temp_c, Some(60.1));
    }

    #[test]
    fn an_empty_snapshot_reports_no_readings_rather_than_zeroes() {
        let telemetry = StatusSnapshot::default().to_telemetry();
        assert_eq!(telemetry.nozzle_temp_c, None);
        assert_eq!(telemetry.bed_temp_c, None);
        assert_eq!(telemetry.host_activity, HostActivity::Unknown);
    }

    fn server_info(klippy_state: &str) -> Frame {
        Frame::Response {
            id: ID_SERVER_INFO,
            result: serde_json::json!({"klippy_state": klippy_state}),
        }
    }

    fn subscribe_response(nozzle: f64) -> Frame {
        Frame::Response {
            id: ID_SUBSCRIBE,
            result: serde_json::json!({"eventtime": 1.0, "status": {
                "extruder": {"temperature": nozzle, "target": 0.0},
                "print_stats": {"state": "standby", "filename": ""}
            }}),
        }
    }

    fn ready_subscription() -> SubscriptionState {
        let mut state = SubscriptionState::default();
        state.on_frame(server_info("ready"));
        state.on_frame(subscribe_response(24.0));
        state
    }

    #[test]
    fn a_ready_klipper_publishes_the_subscription_snapshot() {
        let mut state = SubscriptionState::default();
        assert_eq!(
            state.on_frame(server_info("ready")),
            vec![Step::Health(ConnectionState::Online)]
        );
        assert_eq!(
            state.on_frame(subscribe_response(24.0)),
            vec![Step::Telemetry, Step::Health(ConnectionState::Online)]
        );
        assert_eq!(state.telemetry().nozzle_temp_c, Some(24.0));
    }

    #[test]
    fn subscribing_while_klipper_is_shut_down_reports_offline_and_publishes_no_readings() {
        // Seen live (A0.1, #9, simulator): supervision started after an M112
        // reported the printer online and Ready, because no lifecycle
        // notification arrives for a shutdown that already happened.
        let mut state = SubscriptionState::default();
        assert_eq!(
            state.on_frame(server_info("shutdown")),
            vec![Step::Health(ConnectionState::Offline)]
        );
        assert_eq!(state.on_frame(subscribe_response(24.0)), Vec::new());
        assert_eq!(
            state.on_frame(Frame::StatusUpdate(
                serde_json::json!({"extruder": {"temperature": 23.0}})
            )),
            Vec::new()
        );
        assert_eq!(state.health(), Some(ConnectionState::Offline));
        // Readings still accumulate for when Klipper comes back.
        assert_eq!(state.telemetry().nozzle_temp_c, Some(23.0));
    }

    #[test]
    fn a_subscription_answer_that_beats_server_info_waits_for_the_klippy_state() {
        let mut state = SubscriptionState::default();
        assert_eq!(state.health(), None);
        assert_eq!(state.on_frame(subscribe_response(24.0)), Vec::new());
        assert_eq!(
            state.on_frame(server_info("ready")),
            vec![Step::Telemetry, Step::Health(ConnectionState::Online)]
        );
    }

    #[test]
    fn klipper_becoming_ready_again_resubscribes() {
        // Seen live (A0.1, #9, simulator): after a Klipper restart the
        // readings stopped for good, because Moonraker drops every
        // subscription when Klippy disconnects.
        let mut state = ready_subscription();
        assert_eq!(
            state.on_frame(Frame::KlippyDown),
            vec![Step::Health(ConnectionState::Offline)]
        );
        assert_eq!(
            state.on_frame(Frame::KlippyReady),
            vec![Step::Resubscribe, Step::Health(ConnectionState::Online)]
        );
        assert_eq!(
            state.on_frame(subscribe_response(30.0)),
            vec![Step::Telemetry, Step::Health(ConnectionState::Online)]
        );
        assert_eq!(state.telemetry().nozzle_temp_c, Some(30.0));
    }

    #[test]
    fn readings_during_a_shutdown_do_not_publish_until_klipper_is_ready() {
        let mut state = ready_subscription();
        state.on_frame(Frame::KlippyDown);
        assert_eq!(
            state.on_frame(Frame::StatusUpdate(
                serde_json::json!({"extruder": {"temperature": 20.0}})
            )),
            Vec::new(),
            "telemetry during a shutdown would flip the supervisor back to online"
        );
    }

    #[test]
    fn a_failed_subscribe_waits_for_klipper_rather_than_failing() {
        let mut state = SubscriptionState::default();
        state.on_frame(server_info("startup"));
        assert_eq!(
            state.on_frame(Frame::Error {
                id: ID_SUBSCRIBE,
                message: "Klippy Host not connected".into(),
                code: Some(503),
            }),
            Vec::new()
        );
        assert_eq!(state.health(), Some(ConnectionState::Offline));
    }

    #[test]
    fn a_rejected_credential_ends_the_subscription() {
        let mut state = SubscriptionState::default();
        assert_eq!(
            state.on_frame(Frame::Error {
                id: ID_SUBSCRIBE,
                message: "Unauthorized".into(),
                code: Some(401),
            }),
            vec![Step::AuthFailed("Unauthorized".into())]
        );
    }

    #[test]
    fn moonraker_reports_a_rejected_credential_as_invalid_params() {
        // Seen live (A0.1, #9, simulator in API-key mode): Moonraker's
        // JSON-RPC layer rewrites HTTP 401 to -32602 ("Invalid params") and
        // keeps the "Unauthorized" message. Checking for 401 alone told the
        // user "unexpected response" instead of "check the API key".
        assert!(is_auth_error(Some(-32602), "Unauthorized"));
        assert!(is_auth_error(Some(401), "Unauthorized"));
        assert!(is_auth_error(Some(403), "Forbidden"));
        assert!(!is_auth_error(
            Some(-32602),
            "Invalid params:\nmissing required argument"
        ));
        assert!(!is_auth_error(Some(503), "Klippy Host not connected"));

        let mut state = SubscriptionState::default();
        assert_eq!(
            state.on_frame(parse_frame(
                r#"{"jsonrpc":"2.0","error":{"code":-32602,"message":"Unauthorized"},"id":3}"#
            )),
            vec![Step::AuthFailed("Unauthorized".into())]
        );
    }

    #[test]
    fn an_object_the_printer_lacks_reads_as_absent_not_zero() {
        // Seen live (A0.1, #9, simulator without a heated bed): Moonraker
        // answers for a missing object with every attribute set to null.
        let mut snapshot = StatusSnapshot::default();
        snapshot.merge(&serde_json::json!({
            "extruder": {"temperature": 24.0, "target": 0.0},
            "heater_bed": {"temperature": null, "target": null}
        }));

        let telemetry = snapshot.to_telemetry();
        assert_eq!(telemetry.nozzle_temp_c, Some(24.0));
        assert_eq!(telemetry.bed_temp_c, None);
        assert_eq!(telemetry.bed_target_c, None);
    }

    #[test]
    fn an_idle_printer_with_no_job_reports_no_job_name() {
        // Seen live (A0.1, #9): an idle Klipper reports `filename: ""` and
        // `message: ""`. An empty job name is not a job, and the telemetry
        // cache rejects empty strings, so passing it through failed every
        // cache write while the printer sat idle.
        let mut snapshot = StatusSnapshot::default();
        snapshot.merge(&serde_json::json!({
            "print_stats": {"filename": "", "state": "standby", "print_duration": 0.0}
        }));

        let telemetry = snapshot.to_telemetry();
        assert_eq!(telemetry.job_name, None);
        assert_eq!(telemetry.host_activity_name, Some("standby".to_string()));
    }

    #[test]
    fn normalizes_moonraker_activity_inside_the_adapter_boundary() {
        use crate::printers::operational::HostActivity;

        assert_eq!(normalize_host_activity("standby"), HostActivity::Idle);
        assert_eq!(normalize_host_activity("printing"), HostActivity::Printing);
        assert_eq!(normalize_host_activity("paused"), HostActivity::Paused);
        assert_eq!(normalize_host_activity("busy"), HostActivity::Busy);
        // Seen live on a Snapmaker U1 (A0.1, #9): a finished print leaves
        // `complete` until the next job starts. Decision B1: never Idle.
        assert_eq!(normalize_host_activity("complete"), HostActivity::Finished);
        assert_eq!(
            normalize_host_activity("cancelled"),
            HostActivity::Cancelled
        );
        assert_eq!(normalize_host_activity("error"), HostActivity::Failed);
        assert_eq!(
            normalize_host_activity("moonraker-future-state"),
            HostActivity::Unknown
        );
    }

    #[test]
    fn parses_a_status_notification_with_positional_params() {
        // Moonraker ALWAYS sends notification params as an array — first the
        // status object, then a Klipper-uptime float. Parsing it as a named
        // object silently matches nothing.
        let frame = parse_frame(
            r#"{"jsonrpc":"2.0","method":"notify_status_update",
                "params":[{"extruder":{"temperature":201.4}},578243.578]}"#,
        );
        match frame {
            Frame::StatusUpdate(status) => {
                assert_eq!(status["extruder"]["temperature"], 201.4);
            }
            other => panic!("expected a StatusUpdate, got {other:?}"),
        }
    }

    #[test]
    fn parses_klippy_lifecycle_notifications_that_carry_no_params() {
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notify_klippy_ready"}"#),
            Frame::KlippyReady
        ));
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notify_klippy_shutdown"}"#),
            Frame::KlippyDown
        ));
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notify_klippy_disconnected"}"#),
            Frame::KlippyDown
        ));
    }

    #[test]
    fn parses_a_response_and_an_error_by_id() {
        match parse_frame(r#"{"jsonrpc":"2.0","result":{"klippy_state":"ready"},"id":1}"#) {
            Frame::Response { id, result } => {
                assert_eq!(id, 1);
                assert_eq!(result["klippy_state"], "ready");
            }
            other => panic!("expected a Response, got {other:?}"),
        }
        match parse_frame(
            r#"{"jsonrpc":"2.0","error":{"code":401,"message":"Unauthorized"},"id":2}"#,
        ) {
            Frame::Error { id, message, code } => {
                assert_eq!(id, 2);
                assert_eq!(code, Some(401));
                assert!(message.contains("Unauthorized"));
            }
            other => panic!("expected an Error, got {other:?}"),
        }
    }

    #[test]
    fn unrecognized_frames_are_ignored_not_errors() {
        assert!(matches!(
            parse_frame(r#"{"jsonrpc":"2.0","method":"notify_gcode_response","params":["ok"]}"#),
            Frame::Ignored
        ));
        assert!(matches!(parse_frame("this is not json"), Frame::Ignored));
    }

    #[test]
    fn subscribe_params_request_only_the_fields_we_render() {
        let params = subscribe_params();
        let objects = &params["objects"];
        assert!(objects.get("extruder").is_some());
        assert!(objects.get("heater_bed").is_some());
        assert!(objects.get("print_stats").is_some());
        assert!(objects.get("display_status").is_some());
        assert!(objects.get("toolhead").is_some());
        // Narrow subscriptions keep the notification volume down; `null`
        // here would subscribe to every attribute of every object.
        assert!(objects["extruder"].is_array());
    }

    #[test]
    fn rpc_request_is_well_formed_json_rpc() {
        let raw = rpc_request(7, "printer.info", None);
        let parsed: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(parsed["jsonrpc"], "2.0");
        assert_eq!(parsed["method"], "printer.info");
        assert_eq!(parsed["id"], 7);
        assert!(
            parsed.get("params").is_none(),
            "a params-less call must omit params entirely"
        );
    }

    #[test]
    fn probe_reads_the_build_volume_from_axis_limits() {
        let server_info =
            serde_json::json!({"klippy_state": "ready", "moonraker_version": "v0.9.3"});
        let printer_info = serde_json::json!({
            "hostname": "voron", "software_version": "v0.12.0-85-gd785b396",
            "state": "ready", "state_message": "Printer is ready"
        });
        let mut snapshot = StatusSnapshot::default();
        // 4-element [x, y, z, e] arrays; the 4th is always zero and formally
        // deprecated, so only indices 0-2 may be read.
        snapshot.merge(&serde_json::json!({"toolhead": {
            "axis_minimum": [0.0, -4.0, -2.0, 0.0],
            "axis_maximum": [250.0, 210.0, 220.0, 0.0]
        }}));

        let probe = probe_result_from(&server_info, &printer_info, &snapshot);
        assert_eq!(probe.kind, "moonraker");
        assert_eq!(probe.host_software, "v0.9.3");
        assert_eq!(probe.firmware, "v0.12.0-85-gd785b396");
        assert_eq!(probe.reported_name, "voron");
        assert_eq!(probe.state, "ready");
        assert_eq!(probe.reported.bed_width_mm, Some(250.0));
        assert_eq!(probe.reported.bed_depth_mm, Some(214.0)); // 210 - (-4)
        assert_eq!(probe.reported.printable_height_mm, Some(222.0)); // 220 - (-2)
    }

    #[test]
    fn probe_tolerates_a_printer_that_reports_no_axis_limits() {
        // A shut-down or unhomed Klipper reports no toolhead limits. That is
        // a normal state, not a probe failure.
        let probe = probe_result_from(
            &serde_json::json!({"klippy_state": "shutdown"}),
            &serde_json::json!({"state_message": "Klipper is shut down"}),
            &StatusSnapshot::default(),
        );
        assert_eq!(probe.reported, ReportedCapabilities::default());
        assert_eq!(probe.state, "shutdown");
        assert_eq!(probe.reported_name, "");
    }
}
