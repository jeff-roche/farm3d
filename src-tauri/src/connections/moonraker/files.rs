//! P6 D4/D5/D10, pure: the Moonraker capability adapter's request shapes
//! (including the upload's multipart form), response classification, and
//! response parsers. No I/O lives here — `control` owns the HTTP client and
//! calls into this module.
//!
//! Raw host bodies never leave this module. Moonraker's error bodies carry
//! Python tracebacks with host file paths (spike 3), so a body is parsed for
//! the one field a decision needs (`error.message`) and then dropped. No
//! error string built here quotes a body.

use reqwest::multipart::{Form, Part};
use serde_json::Value;

use super::protocol::tool_objects;
use crate::connections::capabilities::{
    derive_host_facts, BedTemperature, CameraInfo, CommandFailure, DiffersReason, HistoryJob,
    HistoryQuery, HostFacts, HostJobState, HostOperationFailureCode, InconclusiveReason,
    KlippyState, LocateOutcome, PrintSnapshot, PrintStatsState, StagedArtifact,
};
use crate::connections::status_repository::ToolTemperature;
use crate::connections::ConnectionError;

/// Moonraker's file root that `host_path` is relative to.
pub const UPLOAD_ROOT: &str = "gcodes";
/// The multipart part that carries the G-code body.
const UPLOAD_FILE_FIELD: &str = "file";
const UPLOAD_FILE_CONTENT_TYPE: &str = "application/octet-stream";

// --- D4: the upload form --------------------------------------------------

/// The upload's multipart form, in wire order: `root`, `path`, `checksum`,
/// then the `file` part. Its only constructor takes a `StagedArtifact`,
/// nothing can add a part to it, and [`UploadForm::into_multipart`] is the
/// only way to a `reqwest` form — so no code path can send Moonraker's
/// `print` field (which would start the print on upload).
#[derive(Clone, PartialEq, Debug)]
pub struct UploadForm {
    text_fields: [(&'static str, String); 3],
    file_name: String,
}

impl UploadForm {
    pub fn for_artifact(artifact: &StagedArtifact) -> Self {
        let (directory, file_name) = match artifact.host_path.rsplit_once('/') {
            Some((directory, file_name)) => (directory, file_name),
            None => ("", artifact.host_path.as_str()),
        };
        Self {
            text_fields: [
                ("root", UPLOAD_ROOT.to_string()),
                ("path", directory.to_string()),
                ("checksum", artifact.sha256.to_ascii_lowercase()),
            ],
            file_name: file_name.to_string(),
        }
    }

    /// The request body: the text parts in order, then `body` (`size`
    /// bytes) as the `file` part named `<slr-id>.gcode`.
    pub fn into_multipart(self, body: reqwest::Body, size: u64) -> Form {
        let mut form = Form::new();
        for (name, value) in self.text_fields {
            form = form.text(name, value);
        }
        let file = Part::stream_with_length(body, size)
            .file_name(self.file_name)
            .mime_str(UPLOAD_FILE_CONTENT_TYPE)
            .expect("a constant, valid MIME type");
        form.part(UPLOAD_FILE_FIELD, file)
    }
}

// --- D10: request paths ---------------------------------------------------

/// Percent-encodes everything except RFC 3986's unreserved characters, so
/// the result is safe as one path segment or one query value.
fn encode_component(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

/// `GET /server/files/gcodes/<host_path>`, each path segment encoded.
pub fn locate_path(host_path: &str) -> String {
    let segments: Vec<String> = host_path.split('/').map(encode_component).collect();
    format!("/server/files/{UPLOAD_ROOT}/{}", segments.join("/"))
}

/// `POST /printer/print/start?filename=<percent-encoded host_path>`.
pub fn start_path(host_path: &str) -> String {
    format!(
        "/printer/print/start?filename={}",
        encode_component(host_path)
    )
}

/// `GET /printer/objects/query?webhooks&print_stats&pause_resume&heater_bed&<each extruder object>`.
/// `objects` is `printer.objects.list`'s answer; the tools are the objects
/// matching `^extruder\d*$`, in index order.
pub fn job_state_query_path(objects: &[String]) -> String {
    let mut path =
        "/printer/objects/query?webhooks&print_stats&pause_resume&heater_bed".to_string();
    for (_index, name) in tool_objects_of(objects) {
        path.push('&');
        path.push_str(&encode_component(&name));
    }
    path
}

/// `GET /server/history/list?limit=<n>&order=desc[&since=<epoch s>]`.
pub fn history_list_path(query: &HistoryQuery) -> String {
    let mut path = format!("/server/history/list?limit={}&order=desc", query.limit);
    if let Some(since) = query.since_epoch_s.filter(|since| since.is_finite()) {
        path.push_str(&format!("&since={since}"));
    }
    path
}

// --- D5: response classification -------------------------------------------

/// Which D5 dispatch table a write's response is read against.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum WriteKind {
    Upload,
    /// Start, pause, resume, and cancel.
    Control,
}

fn definitive(code: HostOperationFailureCode) -> CommandFailure {
    CommandFailure::Definitive(code)
}

fn indeterminate(reason: InconclusiveReason) -> CommandFailure {
    CommandFailure::Indeterminate {
        reason,
        no_longer_pending: false,
    }
}

/// Moonraker's error body is `{"error":{"code":..,"message":..,"traceback":..}}`.
/// Only `message` is read.
fn error_message(body: &[u8]) -> Option<String> {
    let value: Value = serde_json::from_slice(body).ok()?;
    value
        .get("error")?
        .get("message")?
        .as_str()
        .map(str::to_string)
}

fn is_result_ok(body: &[u8]) -> bool {
    serde_json::from_slice::<Value>(body)
        .ok()
        .and_then(|value| {
            value
                .get("result")
                .and_then(Value::as_str)
                .map(|r| r == "ok")
        })
        .unwrap_or(false)
}

/// D5's dispatch table for a response that arrived. Any status or body the
/// table does not name is `Indeterminate(unexpectedResponse)`.
pub fn classify_write_response(
    kind: WriteKind,
    status: u16,
    body: &[u8],
) -> Result<(), CommandFailure> {
    match (kind, status) {
        (WriteKind::Upload, 201) => Ok(()),
        (WriteKind::Control, 200) if is_result_ok(body) => Ok(()),
        (_, 401) => Err(definitive(HostOperationFailureCode::AuthRejected)),
        (WriteKind::Upload, 403) => Err(definitive(HostOperationFailureCode::FileLoaded)),
        (WriteKind::Upload, 422) => Err(definitive(HostOperationFailureCode::ChecksumRejected)),
        (WriteKind::Upload, 400) => Err(definitive(HostOperationFailureCode::HostRejected)),
        (WriteKind::Control, 400) => {
            let message = error_message(body).unwrap_or_default();
            Err(definitive(if message.contains("SD busy") {
                HostOperationFailureCode::HostBusy
            } else if message.contains("Unable to open file") {
                HostOperationFailureCode::FileMissing
            } else {
                HostOperationFailureCode::HostRejected
            }))
        }
        (WriteKind::Control, 503) => match error_message(body).as_deref().map(str::trim) {
            Some("Klippy Host not connected") => {
                Err(definitive(HostOperationFailureCode::HostNotReady))
            }
            Some("Klippy Disconnected") => Err(CommandFailure::Indeterminate {
                reason: InconclusiveReason::KlipperRestarted,
                no_longer_pending: true,
            }),
            _ => Err(indeterminate(InconclusiveReason::UnexpectedResponse)),
        },
        _ => Err(indeterminate(InconclusiveReason::UnexpectedResponse)),
    }
}

/// A write whose response never arrived. A connect failure happens before
/// any request byte is sent, so it is definitive; anything else (a timeout,
/// a reset, a closed connection) may have been applied.
pub fn classify_write_transport_failure(is_connect: bool) -> CommandFailure {
    if is_connect {
        definitive(HostOperationFailureCode::HostUnreachable)
    } else {
        indeterminate(InconclusiveReason::ResponseLost)
    }
}

/// A read's HTTP status (and, for a 503, the error `message`). A read never
/// becomes `Absent` or a default value on an error status: it is an `Err`,
/// and the caller stays inconclusive. A 503 from Klippy is `HostNotReady`,
/// which D5 treats as "Klipper went away"; any other 503 says nothing about
/// Klipper and stays a protocol error.
pub fn classify_read_response(status: u16, body: &[u8]) -> Result<(), ConnectionError> {
    match status {
        200..=299 => Ok(()),
        401 | 403 => Err(ConnectionError::Auth(format!("HTTP {status}"))),
        503 if matches!(
            error_message(body).as_deref().map(str::trim),
            Some("Klippy Host not connected" | "Klippy Disconnected")
        ) =>
        {
            Err(ConnectionError::HostNotReady)
        }
        _ => Err(ConnectionError::Protocol(format!(
            "unexpected HTTP {status}"
        ))),
    }
}

// --- D4: locate -------------------------------------------------------------

/// What `locate` does after the download's status line and headers.
#[derive(Clone, PartialEq, Debug)]
pub enum LocateStep {
    /// Decided without reading the body.
    Decided(LocateOutcome),
    /// Stream the body through SHA-256 and call [`compare_download`].
    HashBody,
}

pub fn locate_step(
    status: u16,
    content_length: Option<u64>,
    expected_size: u64,
) -> Result<LocateStep, ConnectionError> {
    if status == 404 {
        return Ok(LocateStep::Decided(LocateOutcome::Absent));
    }
    if status != 200 {
        classify_read_response(status, b"")?;
        return Err(ConnectionError::Protocol(format!(
            "unexpected HTTP {status}"
        )));
    }
    match content_length {
        Some(actual) if actual != expected_size => {
            Ok(LocateStep::Decided(LocateOutcome::Differs {
                reason: DiffersReason::Size { actual },
            }))
        }
        _ => Ok(LocateStep::HashBody),
    }
}

/// While streaming: once more bytes than the artifact's size have arrived,
/// the answer is `Differs { Size }` (with the bytes read so far, a lower
/// bound on the real size), and the rest of the body is not read.
pub fn overran_size(artifact: &StagedArtifact, byte_count: u64) -> Option<LocateOutcome> {
    (byte_count > artifact.size).then_some(LocateOutcome::Differs {
        reason: DiffersReason::Size { actual: byte_count },
    })
}

/// The streamed body's byte count and lower-case SHA-256 against the
/// artifact. Size alone never proves identity.
pub fn compare_download(
    artifact: &StagedArtifact,
    byte_count: u64,
    sha256_hex: &str,
) -> LocateOutcome {
    if byte_count != artifact.size {
        LocateOutcome::Differs {
            reason: DiffersReason::Size { actual: byte_count },
        }
    } else if !sha256_hex.eq_ignore_ascii_case(&artifact.sha256) {
        LocateOutcome::Differs {
            reason: DiffersReason::Hash,
        }
    } else {
        LocateOutcome::Matches
    }
}

// --- D10: response parsers ----------------------------------------------------

fn malformed(what: &str) -> ConnectionError {
    ConnectionError::Protocol(format!("the {what} response was not understood"))
}

fn result<'a>(body: &'a Value, what: &str) -> Result<&'a Value, ConnectionError> {
    body.get("result").ok_or_else(|| malformed(what))
}

fn strings(value: Option<&Value>) -> Vec<String> {
    value
        .and_then(Value::as_array)
        .map(|items| {
            items
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// `server.info`'s fields that P6 reads.
#[derive(Clone, PartialEq, Debug)]
pub struct ServerInfo {
    pub klippy_state: KlippyState,
    pub components: Vec<String>,
    pub moonraker_version: String,
    pub api_version: String,
}

fn parse_klippy_state(value: &str) -> Option<KlippyState> {
    match value {
        "ready" => Some(KlippyState::Ready),
        "startup" => Some(KlippyState::Startup),
        "shutdown" => Some(KlippyState::Shutdown),
        "error" => Some(KlippyState::Error),
        "disconnected" => Some(KlippyState::Disconnected),
        _ => None,
    }
}

pub fn parse_server_info(body: &Value) -> Result<ServerInfo, ConnectionError> {
    let result = result(body, "server info")?;
    let klippy_state = result
        .get("klippy_state")
        .and_then(Value::as_str)
        .and_then(parse_klippy_state)
        .ok_or_else(|| malformed("server info"))?;
    Ok(ServerInfo {
        klippy_state,
        components: strings(result.get("components")),
        moonraker_version: result
            .get("moonraker_version")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
        api_version: result
            .get("api_version_string")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string(),
    })
}

pub fn parse_objects_list(body: &Value) -> Result<Vec<String>, ConnectionError> {
    let objects = result(body, "objects list")?
        .get("objects")
        .filter(|objects| objects.is_array())
        .ok_or_else(|| malformed("objects list"))?;
    Ok(strings(Some(objects)))
}

/// D6 "Host facts" from `server.info`, `printer.objects.list`, and the
/// camera count.
pub fn host_facts_from(info: &ServerInfo, objects: &[String], camera_count: usize) -> HostFacts {
    derive_host_facts(
        format!("Moonraker {}", info.moonraker_version),
        info.api_version.clone(),
        info.components.clone(),
        objects,
        camera_count,
    )
}

fn tool_objects_of(objects: &[String]) -> Vec<(u32, String)> {
    let values: Vec<Value> = objects.iter().cloned().map(Value::String).collect();
    tool_objects(&values)
}

/// A `HostJobState` for a Klipper that is not ready: no print snapshot and
/// no temperatures (Moonraker cannot query objects then).
pub fn not_ready_job_state(klippy_state: KlippyState) -> HostJobState {
    HostJobState {
        klippy_state,
        print: None,
        tools: Vec::new(),
        bed: None,
    }
}

fn parse_print_stats_state(value: &str) -> PrintStatsState {
    match value {
        "standby" => PrintStatsState::Standby,
        "printing" => PrintStatsState::Printing,
        "paused" => PrintStatsState::Paused,
        "complete" => PrintStatsState::Complete,
        "cancelled" => PrintStatsState::Cancelled,
        "error" => PrintStatsState::Error,
        other => PrintStatsState::Other(other.to_string()),
    }
}

/// Parses the [`job_state_query_path`] answer, asked while `server.info`
/// reported Klipper ready. `webhooks.state` is Klipper's own, fresher view:
/// if it is no longer `ready`, no print snapshot is reported. A body without
/// it is malformed, never assumed ready, because a ready snapshot can prove
/// a start or a control verb applied (D5).
pub fn parse_host_job_state(
    body: &Value,
    objects: &[String],
) -> Result<HostJobState, ConnectionError> {
    let status = result(body, "objects query")?
        .get("status")
        .ok_or_else(|| malformed("objects query"))?;
    let klippy_state = status
        .get("webhooks")
        .and_then(|webhooks| webhooks.get("state"))
        .and_then(Value::as_str)
        .and_then(parse_klippy_state)
        .ok_or_else(|| malformed("objects query"))?;
    let number = |object: &str, field: &str| {
        status
            .get(object)
            .and_then(|object| object.get(field))
            .and_then(Value::as_f64)
    };
    let tools = tool_objects_of(objects)
        .into_iter()
        .map(|(index, name)| ToolTemperature {
            index,
            temp_c: number(&name, "temperature"),
            target_c: number(&name, "target"),
        })
        .collect();
    let bed = match (
        number("heater_bed", "temperature"),
        number("heater_bed", "target"),
    ) {
        (Some(temp_c), Some(target_c)) => Some(BedTemperature { temp_c, target_c }),
        _ => None,
    };
    let print = if klippy_state == KlippyState::Ready {
        let print_stats = status
            .get("print_stats")
            .ok_or_else(|| malformed("objects query"))?;
        let state = print_stats
            .get("state")
            .and_then(Value::as_str)
            .ok_or_else(|| malformed("objects query"))?;
        Some(PrintSnapshot {
            state: parse_print_stats_state(state),
            filename: print_stats
                .get("filename")
                .and_then(Value::as_str)
                .filter(|filename| !filename.is_empty())
                .map(str::to_string),
            is_paused: status
                .get("pause_resume")
                .and_then(|pause_resume| pause_resume.get("is_paused"))
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    } else {
        None
    };
    Ok(HostJobState {
        klippy_state,
        print,
        tools,
        bed,
    })
}

/// `server.history.list`'s jobs, in the order the host sent them. A job
/// whose hex `job_id` does not parse, or that lacks a field, is dropped.
pub fn parse_history_list(body: &Value) -> Result<Vec<HistoryJob>, ConnectionError> {
    let jobs = result(body, "history")?
        .get("jobs")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("history"))?;
    Ok(jobs
        .iter()
        .filter_map(|job| {
            Some(HistoryJob {
                job_id: u64::from_str_radix(job.get("job_id")?.as_str()?, 16).ok()?,
                filename: job.get("filename")?.as_str()?.to_string(),
                status: job.get("status")?.as_str()?.to_string(),
                start_time_epoch_s: job.get("start_time")?.as_f64()?,
            })
        })
        .collect())
}

/// `server.webcams.list`: name and service only. Stream and snapshot URLs
/// are discarded, since a real host's URLs embed its LAN address.
pub fn parse_webcams_list(body: &Value) -> Result<Vec<CameraInfo>, ConnectionError> {
    let webcams = result(body, "webcams")?
        .get("webcams")
        .and_then(Value::as_array)
        .ok_or_else(|| malformed("webcams"))?;
    Ok(webcams
        .iter()
        .filter_map(|webcam| {
            Some(CameraInfo {
                name: webcam.get("name")?.as_str()?.to_string(),
                service: webcam.get("service")?.as_str()?.to_string(),
            })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Loopback-simulator captures (Moonraker v0.11.0-1, API 1.5.0), taken
    /// for this task with tracebacks trimmed. No real-host capture.
    macro_rules! fixture {
        ($name:literal) => {
            serde_json::from_str::<Value>(include_str!(concat!(
                "../../../tests/fixtures/moonraker/",
                $name
            )))
            .expect("fixture is JSON")
        };
    }

    macro_rules! fixture_bytes {
        ($name:literal) => {
            include_bytes!(concat!("../../../tests/fixtures/moonraker/", $name)).as_slice()
        };
    }

    const SHA: &str = "7707ba5c81dd03b640cbf52b212ecdbae825bfc7a1e3b683f32f5bf72a03da53";

    fn artifact() -> StagedArtifact {
        StagedArtifact {
            host_path: "farm3d/slr-1.gcode".to_string(),
            sha256: SHA.to_string(),
            size: 5517,
        }
    }

    fn objects(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn definitive_code(result: Result<(), CommandFailure>) -> HostOperationFailureCode {
        match result {
            Err(CommandFailure::Definitive(code)) => code,
            other => panic!("expected a definitive failure, got {other:?}"),
        }
    }

    fn indeterminate_reason(result: Result<(), CommandFailure>) -> (InconclusiveReason, bool) {
        match result {
            Err(CommandFailure::Indeterminate {
                reason,
                no_longer_pending,
            }) => (reason, no_longer_pending),
            other => panic!("expected an indeterminate failure, got {other:?}"),
        }
    }

    // --- the upload form ---------------------------------------------------

    /// The multipart body `into_multipart` actually puts on the wire.
    async fn rendered(artifact: &StagedArtifact, body: &'static [u8]) -> String {
        use futures_util::TryStreamExt;
        let chunks: Vec<_> = UploadForm::for_artifact(artifact)
            .into_multipart(reqwest::Body::from(body), body.len() as u64)
            .into_stream()
            .try_collect()
            .await
            .unwrap();
        String::from_utf8(chunks.concat()).unwrap()
    }

    /// Each part's `name="..."`, in wire order.
    fn part_names(rendered: &str) -> Vec<&str> {
        rendered
            .split("form-data; name=\"")
            .skip(1)
            .map(|rest| rest.split('"').next().unwrap())
            .collect()
    }

    #[tokio::test]
    async fn the_upload_form_sends_root_path_checksum_then_file_in_order() {
        let wire = rendered(&artifact(), b"G28\n").await;
        assert_eq!(part_names(&wire), ["root", "path", "checksum", "file"]);
        assert!(wire.contains("name=\"root\"\r\n\r\ngcodes\r\n"));
        assert!(wire.contains("name=\"path\"\r\n\r\nfarm3d\r\n"));
        assert!(wire.contains(&format!("name=\"checksum\"\r\n\r\n{SHA}\r\n")));
        assert!(wire.contains("name=\"file\"; filename=\"slr-1.gcode\""));
        assert!(wire.contains("Content-Type: application/octet-stream\r\n\r\nG28\n\r\n"));
    }

    #[tokio::test]
    async fn the_built_form_has_no_print_field() {
        let wire = rendered(&artifact(), b"G28\n").await;
        assert!(!part_names(&wire).contains(&"print"));
        assert!(!wire.contains("print"));
    }

    #[tokio::test]
    async fn the_checksum_is_always_sent_lower_case() {
        let mut upper = artifact();
        upper.sha256 = SHA.to_ascii_uppercase();
        let wire = rendered(&upper, b"G28\n").await;
        assert!(wire.contains(&format!("name=\"checksum\"\r\n\r\n{SHA}\r\n")));
    }

    // --- request paths -------------------------------------------------------

    #[test]
    fn locate_and_start_percent_encode_the_host_path() {
        assert_eq!(
            locate_path("farm3d/slr 1#x.gcode"),
            "/server/files/gcodes/farm3d/slr%201%23x.gcode"
        );
        assert_eq!(
            start_path("farm3d/slr-1.gcode"),
            "/printer/print/start?filename=farm3d%2Fslr-1.gcode"
        );
    }

    #[test]
    fn the_job_state_query_asks_for_every_tool_in_index_order() {
        let path = job_state_query_path(&objects(&[
            "extruder2",
            "heater_bed",
            "extruder",
            "extruder_offset_calibration",
            "extruder1",
            "extruder3",
        ]));
        assert_eq!(
            path,
            "/printer/objects/query?webhooks&print_stats&pause_resume&heater_bed\
             &extruder&extruder1&extruder2&extruder3"
        );
    }

    #[test]
    fn the_history_query_sends_limit_order_desc_and_since_when_given() {
        assert_eq!(
            history_list_path(&HistoryQuery {
                since_epoch_s: None,
                limit: 50
            }),
            "/server/history/list?limit=50&order=desc"
        );
        assert_eq!(
            history_list_path(&HistoryQuery {
                since_epoch_s: Some(1790381406.5),
                limit: 50
            }),
            "/server/history/list?limit=50&order=desc&since=1790381406.5"
        );
    }

    // --- response classification: upload -----------------------------------

    #[test]
    fn an_upload_201_is_success() {
        let body = br#"{"action":"create_file","item":{"path":"farm3d/slr-1.gcode","root":"gcodes"},"print_started":false,"print_queued":false}"#;
        assert_eq!(
            classify_write_response(WriteKind::Upload, 201, body),
            Ok(())
        );
    }

    #[test]
    fn upload_definitive_failures_follow_the_d5_table() {
        let unauthorized = br#"{"error":{"code":401,"message":"Unauthorized","traceback":"x"}}"#;
        assert_eq!(
            definitive_code(classify_write_response(
                WriteKind::Upload,
                401,
                unauthorized
            )),
            HostOperationFailureCode::AuthRejected
        );
        assert_eq!(
            definitive_code(classify_write_response(
                WriteKind::Upload,
                403,
                fixture_bytes!("error_403_upload_403.json")
            )),
            HostOperationFailureCode::FileLoaded
        );
        assert_eq!(
            definitive_code(classify_write_response(
                WriteKind::Upload,
                422,
                fixture_bytes!("error_422_upload_422.json")
            )),
            HostOperationFailureCode::ChecksumRejected
        );
        assert_eq!(
            definitive_code(classify_write_response(WriteKind::Upload, 400, b"{}")),
            HostOperationFailureCode::HostRejected
        );
    }

    #[test]
    fn any_other_upload_status_is_an_unexpected_response() {
        for status in [200, 500, 503, 302, 404] {
            assert_eq!(
                indeterminate_reason(classify_write_response(WriteKind::Upload, status, b"")),
                (InconclusiveReason::UnexpectedResponse, false),
                "status {status}"
            );
        }
    }

    // --- response classification: control -----------------------------------

    #[test]
    fn a_control_200_ok_is_success_and_any_other_200_is_unexpected() {
        assert_eq!(
            classify_write_response(WriteKind::Control, 200, br#"{"result":"ok"}"#),
            Ok(())
        );
        for body in [&br#"{"result":"nope"}"#[..], b"", b"not json"] {
            assert_eq!(
                indeterminate_reason(classify_write_response(WriteKind::Control, 200, body)),
                (InconclusiveReason::UnexpectedResponse, false)
            );
        }
    }

    #[test]
    fn control_400s_are_classified_by_message() {
        assert_eq!(
            definitive_code(classify_write_response(
                WriteKind::Control,
                400,
                fixture_bytes!("error_400_start_busy.json")
            )),
            HostOperationFailureCode::HostBusy
        );
        assert_eq!(
            definitive_code(classify_write_response(
                WriteKind::Control,
                400,
                fixture_bytes!("error_400_start_missing.json")
            )),
            HostOperationFailureCode::FileMissing
        );
        // Klippy shutdown/startup: HTTP carries only "Unknown" (Gate E).
        assert_eq!(
            definitive_code(classify_write_response(
                WriteKind::Control,
                400,
                fixture_bytes!("error_400_start_shutdown.json")
            )),
            HostOperationFailureCode::HostRejected
        );
        assert_eq!(
            definitive_code(classify_write_response(WriteKind::Control, 400, b"garbage")),
            HostOperationFailureCode::HostRejected
        );
    }

    #[test]
    fn control_503s_are_classified_by_message() {
        assert_eq!(
            definitive_code(classify_write_response(
                WriteKind::Control,
                503,
                fixture_bytes!("error_503_start_503.json")
            )),
            HostOperationFailureCode::HostNotReady
        );
        let disconnected =
            br#"{"error":{"code":503,"message":"Klippy Disconnected","traceback":"x"}}"#;
        assert_eq!(
            indeterminate_reason(classify_write_response(
                WriteKind::Control,
                503,
                disconnected
            )),
            (InconclusiveReason::KlipperRestarted, true)
        );
        let other = br#"{"error":{"code":503,"message":"Service Unavailable"}}"#;
        assert_eq!(
            indeterminate_reason(classify_write_response(WriteKind::Control, 503, other)),
            (InconclusiveReason::UnexpectedResponse, false)
        );
    }

    #[test]
    fn control_401_is_auth_rejected_and_other_statuses_are_unexpected() {
        assert_eq!(
            definitive_code(classify_write_response(WriteKind::Control, 401, b"")),
            HostOperationFailureCode::AuthRejected
        );
        for status in [201, 403, 404, 422, 500, 502] {
            assert_eq!(
                indeterminate_reason(classify_write_response(WriteKind::Control, status, b"")),
                (InconclusiveReason::UnexpectedResponse, false),
                "status {status}"
            );
        }
    }

    #[test]
    fn a_connect_failure_is_definitive_and_any_other_transport_failure_is_lost() {
        assert_eq!(
            classify_write_transport_failure(true),
            CommandFailure::Definitive(HostOperationFailureCode::HostUnreachable)
        );
        assert_eq!(
            classify_write_transport_failure(false),
            CommandFailure::Indeterminate {
                reason: InconclusiveReason::ResponseLost,
                no_longer_pending: false
            }
        );
    }

    #[test]
    fn read_statuses_never_quote_a_body() {
        assert_eq!(classify_read_response(200, b""), Ok(()));
        assert_eq!(
            classify_read_response(401, b"{\"error\":{\"message\":\"Unauthorized\"}}"),
            Err(ConnectionError::Auth("HTTP 401".to_string()))
        );
        assert_eq!(
            classify_read_response(500, fixture_bytes!("error_400_start_busy.json")),
            Err(ConnectionError::Protocol("unexpected HTTP 500".to_string()))
        );
    }

    #[test]
    fn a_read_503_from_klippy_is_host_not_ready_and_any_other_503_is_not() {
        assert_eq!(
            classify_read_response(503, fixture_bytes!("error_503_objects_list_503.json")),
            Err(ConnectionError::HostNotReady)
        );
        let disconnected = br#"{"error":{"code":503,"message":"Klippy Disconnected"}}"#;
        assert_eq!(
            classify_read_response(503, disconnected),
            Err(ConnectionError::HostNotReady)
        );
        for body in [
            &br#"{"error":{"code":503,"message":"Service Unavailable"}}"#[..],
            b"",
        ] {
            assert_eq!(
                classify_read_response(503, body),
                Err(ConnectionError::Protocol("unexpected HTTP 503".to_string()))
            );
        }
    }

    // --- locate -----------------------------------------------------------------

    #[test]
    fn locate_404_is_absent_and_a_content_length_mismatch_differs_by_size() {
        assert_eq!(
            locate_step(404, None, 5517),
            Ok(LocateStep::Decided(LocateOutcome::Absent))
        );
        assert_eq!(
            locate_step(200, Some(10), 5517),
            Ok(LocateStep::Decided(LocateOutcome::Differs {
                reason: DiffersReason::Size { actual: 10 }
            }))
        );
        assert_eq!(locate_step(200, Some(5517), 5517), Ok(LocateStep::HashBody));
        assert_eq!(locate_step(200, None, 5517), Ok(LocateStep::HashBody));
    }

    #[test]
    fn locate_errors_are_never_absent() {
        assert_eq!(
            locate_step(401, None, 5517),
            Err(ConnectionError::Auth("HTTP 401".to_string()))
        );
        for status in [500, 503, 302, 403] {
            assert!(locate_step(status, None, 5517).is_err(), "status {status}");
        }
    }

    #[test]
    fn overran_size_stops_only_past_the_artifact_size() {
        assert_eq!(overran_size(&artifact(), 5517), None);
        assert_eq!(
            overran_size(&artifact(), 5518),
            Some(LocateOutcome::Differs {
                reason: DiffersReason::Size { actual: 5518 }
            })
        );
    }

    #[test]
    fn compare_download_checks_the_byte_count_then_the_hash() {
        assert_eq!(
            compare_download(&artifact(), 5517, SHA),
            LocateOutcome::Matches
        );
        assert_eq!(
            compare_download(&artifact(), 5517, &SHA.to_ascii_uppercase()),
            LocateOutcome::Matches
        );
        assert_eq!(
            compare_download(&artifact(), 5516, SHA),
            LocateOutcome::Differs {
                reason: DiffersReason::Size { actual: 5516 }
            }
        );
        assert_eq!(
            compare_download(&artifact(), 5517, &"0".repeat(64)),
            LocateOutcome::Differs {
                reason: DiffersReason::Hash
            }
        );
    }

    // --- parsers --------------------------------------------------------------------

    #[test]
    fn server_info_parses_the_simulator_capture() {
        let info = parse_server_info(&fixture!("server_info.json")).unwrap();
        assert_eq!(info.klippy_state, KlippyState::Ready);
        assert!(info.components.contains(&"history".to_string()));
        assert_eq!(info.moonraker_version, "v0.11.0-1-g1cfb0c4-prind");
        assert_eq!(info.api_version, "1.5.0");

        let disconnected = parse_server_info(&fixture!("server_info_disconnected.json")).unwrap();
        assert_eq!(disconnected.klippy_state, KlippyState::Disconnected);
    }

    #[test]
    fn server_info_with_an_unknown_klippy_state_is_an_error() {
        let body = json!({"result": {"klippy_state": "sideways", "components": []}});
        assert!(parse_server_info(&body).is_err());
    }

    #[test]
    fn host_facts_from_the_simulator_captures() {
        let info = parse_server_info(&fixture!("server_info.json")).unwrap();
        let single = parse_objects_list(&fixture!("objects_list.json")).unwrap();
        let facts = host_facts_from(&info, &single, 0);
        assert_eq!(facts.host_software, "Moonraker v0.11.0-1-g1cfb0c4-prind");
        assert_eq!(facts.api_version, "1.5.0");
        assert!(facts.has_virtual_sdcard);
        assert!(facts.has_pause_resume);
        assert!(facts.has_history);
        assert!(facts.has_heater_bed);
        assert_eq!(facts.tool_count, 1);
        assert_eq!(facts.camera_count, 0);

        let multi = parse_objects_list(&fixture!("objects_list_multi.json")).unwrap();
        assert_eq!(host_facts_from(&info, &multi, 2).tool_count, 4);
    }

    #[test]
    fn job_state_parses_one_tool_and_the_bed() {
        let objects = parse_objects_list(&fixture!("objects_list.json")).unwrap();
        let state =
            parse_host_job_state(&fixture!("objects_query_printing.json"), &objects).unwrap();
        assert_eq!(state.klippy_state, KlippyState::Ready);
        assert_eq!(
            state.print,
            Some(PrintSnapshot {
                state: PrintStatsState::Printing,
                filename: Some("farm3d/task8-long2.gcode".to_string()),
                is_paused: false,
            })
        );
        assert_eq!(
            state.tools,
            vec![ToolTemperature {
                index: 0,
                temp_c: Some(103.26),
                target_c: Some(0.0)
            }]
        );
        assert_eq!(
            state.bed,
            Some(BedTemperature {
                temp_c: 103.26,
                target_c: 0.0
            })
        );
    }

    #[test]
    fn job_state_parses_paused_and_complete_captures() {
        let objects = parse_objects_list(&fixture!("objects_list.json")).unwrap();
        let paused =
            parse_host_job_state(&fixture!("objects_query_paused.json"), &objects).unwrap();
        let print = paused.print.unwrap();
        assert_eq!(print.state, PrintStatsState::Paused);
        assert!(print.is_paused);

        let complete =
            parse_host_job_state(&fixture!("objects_query_complete.json"), &objects).unwrap();
        assert_eq!(complete.print.unwrap().state, PrintStatsState::Complete);
    }

    #[test]
    fn job_state_parses_four_tools_in_index_order() {
        let objects = parse_objects_list(&fixture!("objects_list_multi.json")).unwrap();
        let state = parse_host_job_state(&fixture!("objects_query_multi.json"), &objects).unwrap();
        assert_eq!(
            state
                .tools
                .iter()
                .map(|tool| tool.index)
                .collect::<Vec<_>>(),
            [0, 1, 2, 3]
        );
        assert!(state
            .tools
            .iter()
            .all(|tool| tool.temp_c == Some(103.26) && tool.target_c == Some(0.0)));
        // Standby with an empty filename maps to `None`.
        let print = state.print.unwrap();
        assert_eq!(print.state, PrintStatsState::Standby);
        assert_eq!(print.filename, None);
    }

    #[test]
    fn job_state_without_webhooks_is_malformed_never_ready() {
        // A body missing Klipper's own state must never read as Ready:
        // a Ready snapshot can become reconciliation proof.
        let body = json!({"result": {"status": {
            "print_stats": {"state": "printing", "filename": "farm3d/x.gcode"},
            "extruder": {"temperature": 21.0, "target": 0.0}
        }}});
        assert!(matches!(
            parse_host_job_state(&body, &objects(&["extruder"])),
            Err(ConnectionError::Protocol(_))
        ));
        let no_state = json!({"result": {"status": {
            "webhooks": {"state_message": "Printer is ready"},
            "print_stats": {"state": "printing", "filename": "farm3d/x.gcode"}
        }}});
        assert!(parse_host_job_state(&no_state, &[]).is_err());
    }

    #[test]
    fn job_state_on_a_no_bed_printer_has_no_bed_rather_than_zero() {
        let body = json!({"result": {"status": {
            "webhooks": {"state": "ready"},
            "print_stats": {"state": "standby", "filename": ""},
            "pause_resume": {"is_paused": false},
            "extruder": {"temperature": 21.0, "target": 0.0}
        }}});
        let state = parse_host_job_state(&body, &objects(&["extruder"])).unwrap();
        assert_eq!(state.bed, None);
        assert_eq!(state.tools.len(), 1);
    }

    #[test]
    fn job_state_reports_no_print_when_webhooks_says_klipper_is_not_ready() {
        let body = json!({"result": {"status": {
            "webhooks": {"state": "shutdown"},
            "print_stats": {"state": "complete", "filename": "farm3d/x.gcode"},
            "extruder": {"temperature": 21.0, "target": 0.0}
        }}});
        let state = parse_host_job_state(&body, &objects(&["extruder"])).unwrap();
        assert_eq!(state.klippy_state, KlippyState::Shutdown);
        assert_eq!(state.print, None);
    }

    #[test]
    fn not_ready_job_state_has_nothing_but_the_klippy_state() {
        let state = not_ready_job_state(KlippyState::Disconnected);
        assert_eq!(state.klippy_state, KlippyState::Disconnected);
        assert_eq!(state.print, None);
        assert!(state.tools.is_empty());
        assert_eq!(state.bed, None);
    }

    fn job_ids(body: &Value) -> Vec<u64> {
        parse_history_list(body)
            .unwrap()
            .iter()
            .map(|job| job.job_id)
            .collect()
    }

    #[test]
    fn history_parses_the_simulator_capture_newest_first() {
        let jobs = parse_history_list(&fixture!("history_list_desc.json")).unwrap();
        assert_eq!(
            jobs.iter().map(|job| job.job_id).collect::<Vec<_>>(),
            [4, 3, 2, 1]
        );
        assert_eq!(jobs[0].filename, "farm3d/task8-job4.gcode");
        assert_eq!(jobs[0].status, "completed");
        assert_eq!(jobs[0].start_time_epoch_s, 1790383031.2507887);
    }

    // D5 "Confirming the parameters": the same four jobs, captured from the
    // v0.11.0 simulator with each query. Moonraker honours `order` and
    // `since` (on `start_time`), and a `limit` keeps the newest under desc.

    #[test]
    fn the_simulator_honours_order_asc() {
        assert_eq!(job_ids(&fixture!("history_list_asc.json")), [1, 2, 3, 4]);
    }

    #[test]
    fn the_simulator_keeps_the_newest_jobs_under_a_limit_with_order_desc() {
        assert_eq!(job_ids(&fixture!("history_list_desc_limit2.json")), [4, 3]);
    }

    #[test]
    fn the_simulator_honours_since_on_start_time() {
        // Captured with `since=1790383010`, between job 2 (…009.20) and
        // job 3 (…020.22).
        let jobs = parse_history_list(&fixture!("history_list_desc_since.json")).unwrap();
        assert_eq!(
            jobs.iter().map(|job| job.job_id).collect::<Vec<_>>(),
            [4, 3]
        );
        assert!(jobs.iter().all(|job| job.start_time_epoch_s > 1790383010.0));
    }

    #[test]
    fn history_job_ids_are_hex_and_an_unparseable_id_is_dropped() {
        let body = json!({"result": {"count": 3, "jobs": [
            {"job_id": "0000A1", "filename": "farm3d/a.gcode", "status": "in_progress", "start_time": 10.5},
            {"job_id": "zz", "filename": "farm3d/b.gcode", "status": "completed", "start_time": 9.0},
            {"job_id": 7, "filename": "farm3d/c.gcode", "status": "completed", "start_time": 8.0}
        ]}});
        let jobs = parse_history_list(&body).unwrap();
        assert_eq!(
            jobs,
            vec![HistoryJob {
                job_id: 0xA1,
                filename: "farm3d/a.gcode".to_string(),
                status: "in_progress".to_string(),
                start_time_epoch_s: 10.5,
            }]
        );
    }

    #[test]
    fn webcams_keep_name_and_service_and_discard_urls() {
        // Synthetic (the simulator has no webcam): Moonraker's webcam entry
        // shape, with an RFC 5737 address standing in for a LAN one.
        let body = json!({"result": {"webcams": [
            {"name": "front", "service": "webrtc-camerastreamer", "enabled": true,
             "stream_url": "http://192.0.2.10/webcam/webrtc",
             "snapshot_url": "http://192.0.2.10/webcam/snapshot"},
            {"name": "top", "service": "mjpegstreamer-adaptive",
             "stream_url": "/webcam2/?action=stream"}
        ]}});
        let cameras = parse_webcams_list(&body).unwrap();
        assert_eq!(
            cameras,
            vec![
                CameraInfo {
                    name: "front".to_string(),
                    service: "webrtc-camerastreamer".to_string()
                },
                CameraInfo {
                    name: "top".to_string(),
                    service: "mjpegstreamer-adaptive".to_string()
                },
            ]
        );
        assert!(parse_webcams_list(&fixture!("webcams_empty.json"))
            .unwrap()
            .is_empty());
    }

    #[test]
    fn parser_errors_never_quote_the_body() {
        let body = json!({"error": {"message": "secret-ish", "traceback": "/opt/klipper/x.py"}});
        for error in [
            parse_server_info(&body).unwrap_err(),
            parse_objects_list(&body).unwrap_err(),
            parse_history_list(&body).unwrap_err(),
            parse_webcams_list(&body).unwrap_err(),
            parse_host_job_state(&body, &[]).unwrap_err(),
        ] {
            let text = format!("{error} {error:?}");
            assert!(
                !text.contains("secret-ish") && !text.contains("/opt/klipper"),
                "{text}"
            );
        }
    }
}
