//! `FakeMoonraker`: an in-process Moonraker HTTP server on a loopback port,
//! for the P6 capability adapter (`connections::moonraker::control`) and the
//! executor and reconciler built on it. The adapter speaks HTTP only, so
//! there is no WebSocket here.
//!
//! It keeps uploads in memory and serves them back at
//! `/server/files/gcodes/...`, keeps a job history with hex job ids, tracks
//! Klipper's live print state, and records every request. Its default
//! answers follow the loopback simulator's (spike Gates A, B, E, and the
//! captures in `tests/fixtures/moonraker/`). Scripted [`Fault`]s cover what
//! the simulators can only reach with a proxy or a restart.
//!
//! An upload carrying a `print` field fails the test: the fake answers 400
//! and panics when it is dropped, unless the test took the violation with
//! [`FakeMoonraker::take_print_field_uploads`].

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use farm3d_lib::connections::{ConnectionConfig, MOONRAKER_KIND};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

/// Which endpoint a scripted fault waits for.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub enum Route {
    Upload,
    Download,
    Start,
    Pause,
    Resume,
    Cancel,
    ServerInfo,
    ObjectsList,
    ObjectsQuery,
    History,
    Webcams,
}

/// How a scripted start leaves the host when its response is dropped.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartTrace {
    /// `printing` with the file, and a new `in_progress` history job.
    PrintingWithHistoryJob,
    /// `printing` with the file, but no history job (yet).
    PrintingOnly,
    /// `complete` with the file and no history job: a short file that
    /// finished inside one status batch (spike Gate F case 1).
    CompleteOnly,
}

/// One scripted misbehaviour, used by the next request on its [`Route`].
#[derive(Clone, Debug)]
pub enum Fault {
    /// Upload: store the file, then close without a response.
    StoreThenDropResponse,
    /// Upload: read half the body, then close. Nothing is stored.
    DropMidBody,
    /// Upload: close without a response; the file appears only after the
    /// delay (a buffering hop delivered the body late, spike Gate D).
    DeliverLate(Duration),
    /// Upload: store these bytes instead of the body (a different or
    /// partial file at the path), and answer 201.
    StoreDifferentBytes(Vec<u8>),
    /// Any route: handle the request normally, but wait this long before
    /// answering (past the client's timeout).
    DelayResponse(Duration),
    /// Any route: answer this status with Moonraker's error body (message
    /// plus a traceback naming host file paths), changing nothing.
    Respond { status: u16, message: String },
    /// Start: apply the start as described, then close without a response.
    ApplyStartThenDrop(StartTrace),
    /// Pause, resume, cancel: answer `{"result":"ok"}` and change nothing.
    OkWithoutEffect,
    /// Download: send the file with chunked encoding (no `Content-Length`),
    /// then hold the connection open this long before ending the body.
    ChunkedDownloadThenStall(Duration),
}

impl Fault {
    pub fn respond(status: u16, message: &str) -> Self {
        Fault::Respond {
            status,
            message: message.to_string(),
        }
    }
    /// Spike Gate A's body shape.
    pub fn unauthorized() -> Self {
        Self::respond(401, "Unauthorized")
    }
    pub fn file_loaded() -> Self {
        Self::respond(403, "Forbidden")
    }
    pub fn checksum_mismatch() -> Self {
        Self::respond(422, "Unprocessable Entity")
    }
    pub fn sd_busy() -> Self {
        Self::respond(400, "SD busy")
    }
    pub fn unable_to_open_file() -> Self {
        Self::respond(400, "Unable to open file")
    }
    pub fn klippy_host_not_connected() -> Self {
        Self::respond(503, "Klippy Host not connected")
    }
    pub fn klippy_disconnected() -> Self {
        Self::respond(503, "Klippy Disconnected")
    }
}

/// A traceback fragment every error body carries, so tests can prove no raw
/// body reaches an error string.
pub const TRACEBACK_MARKER: &str = "/opt/moonraker/moonraker/components/application.py";

#[derive(Clone, PartialEq, Debug)]
pub struct FakeJob {
    /// Moonraker's hex string (`"00000A"`); tests may put junk here.
    pub job_id: String,
    pub filename: String,
    pub status: String,
    pub start_time: f64,
}

#[derive(Clone, Debug)]
pub struct SeenRequest {
    pub method: String,
    /// Path plus query string, as sent.
    pub target: String,
    /// Lower-cased names.
    pub headers: HashMap<String, String>,
    /// An upload's multipart part names, in wire order.
    pub form_parts: Vec<String>,
    /// An upload's text parts.
    pub form_values: Vec<(String, String)>,
}

impl SeenRequest {
    pub fn path(&self) -> &str {
        self.target.split('?').next().unwrap_or_default()
    }
    pub fn query(&self) -> Vec<(String, String)> {
        query_pairs(&self.target)
    }
}

/// Everything the fake knows. Fields are public so a test can set up any
/// host state directly through [`FakeMoonraker::with_state`].
#[derive(Debug)]
pub struct FakeState {
    /// Keyed by path under the `gcodes` root (`farm3d/<id>.gcode`).
    pub files: BTreeMap<String, Vec<u8>>,
    /// Newest last.
    pub history: Vec<FakeJob>,
    pub next_job_id: u64,
    /// `ready`, `startup`, `shutdown`, `error`, or `disconnected`.
    pub klippy_state: String,
    pub print_state: String,
    pub print_filename: String,
    pub is_paused: bool,
    /// (temperature, target) for `extruder`, `extruder1`, ...
    pub tools: Vec<(f64, f64)>,
    /// `None` on a no-bed printer.
    pub bed: Option<(f64, f64)>,
    /// Objects listed besides the tools, bed, and the Klipper basics (for
    /// example `extruder_offset_calibration`).
    pub extra_objects: Vec<String>,
    pub components: Vec<String>,
    pub webcams: Vec<Value>,
    /// `Some`: every request needs exactly this `X-Api-Key`, or gets 401.
    pub api_key: Option<String>,
    late_files: Vec<(Instant, String, Vec<u8>)>,
    faults: HashMap<Route, VecDeque<Fault>>,
    requests: Vec<SeenRequest>,
    print_field_uploads: usize,
}

impl FakeState {
    fn new() -> Self {
        Self {
            files: BTreeMap::new(),
            history: Vec::new(),
            next_job_id: 1,
            klippy_state: "ready".to_string(),
            print_state: "standby".to_string(),
            print_filename: String::new(),
            is_paused: false,
            tools: vec![(200.0, 210.0)],
            bed: Some((60.0, 65.0)),
            extra_objects: Vec::new(),
            // The v0.11.0 simulator's list (fixtures/moonraker/server_info.json).
            components: [
                "klippy_connection",
                "application",
                "websockets",
                "database",
                "file_manager",
                "authorization",
                "klippy_apis",
                "machine",
                "data_store",
                "proc_stats",
                "job_state",
                "job_queue",
                "history",
                "http_client",
                "announcements",
                "webcam",
                "extensions",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
            webcams: Vec::new(),
            api_key: None,
            late_files: Vec::new(),
            faults: HashMap::new(),
            requests: Vec::new(),
            print_field_uploads: 0,
        }
    }

    fn promote_late_files(&mut self) {
        let now = Instant::now();
        let (ready, waiting): (Vec<_>, Vec<_>) = std::mem::take(&mut self.late_files)
            .into_iter()
            .partition(|(at, _, _)| *at <= now);
        self.late_files = waiting;
        for (_, path, bytes) in ready {
            self.files.insert(path, bytes);
        }
    }

    fn file(&mut self, path: &str) -> Option<Vec<u8>> {
        self.promote_late_files();
        self.files.get(path).cloned()
    }

    fn is_loaded(&self, path: &str) -> bool {
        self.print_filename == path && matches!(self.print_state.as_str(), "printing" | "paused")
    }

    fn apply_start(&mut self, path: &str, trace: StartTrace) {
        self.print_filename = path.to_string();
        self.is_paused = false;
        match trace {
            StartTrace::PrintingWithHistoryJob => {
                self.print_state = "printing".to_string();
                self.push_job(path, "in_progress");
            }
            StartTrace::PrintingOnly => self.print_state = "printing".to_string(),
            StartTrace::CompleteOnly => self.print_state = "complete".to_string(),
        }
    }

    /// Adds a history job with the next hex id, started now.
    pub fn push_job(&mut self, filename: &str, status: &str) {
        let job_id = format!("{:06X}", self.next_job_id);
        self.next_job_id += 1;
        self.history.push(FakeJob {
            job_id,
            filename: filename.to_string(),
            status: status.to_string(),
            start_time: now_epoch_s(),
        });
    }

    fn close_open_job(&mut self, status: &str) {
        if let Some(job) = self
            .history
            .iter_mut()
            .rev()
            .find(|job| job.status == "in_progress")
        {
            job.status = status.to_string();
        }
    }

    fn objects(&self) -> Vec<String> {
        let mut objects: Vec<String> = [
            "webhooks",
            "configfile",
            "print_stats",
            "virtual_sdcard",
            "pause_resume",
            "toolhead",
            "idle_timeout",
        ]
        .into_iter()
        .map(str::to_string)
        .collect();
        if self.bed.is_some() {
            objects.push("heater_bed".to_string());
        }
        objects.extend(self.tool_names());
        objects.extend(self.extra_objects.iter().cloned());
        objects
    }

    fn tool_names(&self) -> Vec<String> {
        (0..self.tools.len())
            .map(|index| {
                if index == 0 {
                    "extruder".to_string()
                } else {
                    format!("extruder{index}")
                }
            })
            .collect()
    }
}

pub struct FakeMoonraker {
    pub port: u16,
    state: Arc<Mutex<FakeState>>,
    /// `true` once [`FakeMoonraker::set_reachable`]`(false)` has been
    /// called: every new connection is accepted, then dropped unanswered,
    /// the way a host that has gone away looks to a client (the same
    /// technique as `p6_moonraker_readonly.rs`'s `Gate::block`).
    down: Arc<AtomicBool>,
}

impl FakeMoonraker {
    pub fn start() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind FakeMoonraker");
        listener
            .set_nonblocking(true)
            .expect("nonblocking listener");
        let port = listener.local_addr().expect("fake address").port();
        let state = Arc::new(Mutex::new(FakeState::new()));
        let down = Arc::new(AtomicBool::new(false));
        let shared = Arc::clone(&state);
        let down_flag = Arc::clone(&down);
        std::thread::spawn(move || loop {
            match listener.accept() {
                Ok((stream, _)) => {
                    if down_flag.load(Ordering::SeqCst) {
                        let _ = stream.shutdown(Shutdown::Both);
                        continue;
                    }
                    let state = Arc::clone(&shared);
                    std::thread::spawn(move || handle_connection(stream, &state));
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Err(_) => std::thread::sleep(Duration::from_millis(5)),
            }
        });
        Self { port, state, down }
    }

    /// `false` makes the fake unreachable: every new connection is accepted
    /// and immediately dropped, unanswered. `true` (the default) restores
    /// normal service.
    pub fn set_reachable(&self, reachable: bool) {
        self.down.store(!reachable, Ordering::SeqCst);
    }

    /// A Moonraker `ConnectionConfig` pointing at this fake.
    pub fn config(&self) -> ConnectionConfig {
        ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "127.0.0.1".to_string(),
            port: self.port,
            use_tls: false,
            credential_ref: None,
        }
    }

    fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn with_state<T>(&self, change: impl FnOnce(&mut FakeState) -> T) -> T {
        change(&mut self.lock())
    }

    /// Scripts `fault` for the next request on `route` (faults on one route
    /// are used in the order they were scripted).
    pub fn fault(&self, route: Route, fault: Fault) {
        self.lock()
            .faults
            .entry(route)
            .or_default()
            .push_back(fault);
    }

    pub fn requests(&self) -> Vec<SeenRequest> {
        self.lock().requests.clone()
    }

    pub fn file(&self, path: &str) -> Option<Vec<u8>> {
        self.lock().file(path)
    }

    pub fn put_file(&self, path: &str, bytes: &[u8]) {
        self.lock().files.insert(path.to_string(), bytes.to_vec());
    }

    /// The live print state: (`print_stats.state`, filename, `is_paused`).
    pub fn print_state(&self) -> (String, String, bool) {
        let state = self.lock();
        (
            state.print_state.clone(),
            state.print_filename.clone(),
            state.is_paused,
        )
    }

    pub fn history(&self) -> Vec<FakeJob> {
        self.lock().history.clone()
    }

    /// A Klipper restart as the simulator shows it (spike Gate G): the live
    /// state is cleared and an open job closes as `klippy_disconnect`, while
    /// files and history persist.
    pub fn restart(&self) {
        let mut state = self.lock();
        state.close_open_job("klippy_disconnect");
        state.klippy_state = "ready".to_string();
        state.print_state = "standby".to_string();
        state.print_filename = String::new();
        state.is_paused = false;
    }

    /// How many uploads carried a `print` field; resets the count so the
    /// drop check passes.
    pub fn take_print_field_uploads(&self) -> usize {
        std::mem::take(&mut self.lock().print_field_uploads)
    }
}

impl Drop for FakeMoonraker {
    fn drop(&mut self) {
        let violations = self.lock().print_field_uploads;
        if violations > 0 && !std::thread::panicking() {
            panic!("FakeMoonraker: {violations} upload(s) carried a `print` field");
        }
    }
}

// --- HTTP handling ------------------------------------------------------------

fn now_epoch_s() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock after 1970")
        .as_secs_f64()
}

fn percent_decode(value: &str) -> String {
    let bytes = value.as_bytes();
    let mut decoded = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        let escaped = (bytes[index] == b'%')
            .then(|| bytes.get(index + 1..index + 3))
            .flatten()
            .and_then(|hex| u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok());
        match escaped {
            Some(byte) => {
                decoded.push(byte);
                index += 3;
            }
            None => {
                decoded.push(bytes[index]);
                index += 1;
            }
        }
    }
    String::from_utf8_lossy(&decoded).into_owned()
}

fn query_pairs(target: &str) -> Vec<(String, String)> {
    let Some((_, query)) = target.split_once('?') else {
        return Vec::new();
    };
    query
        .split('&')
        .filter(|pair| !pair.is_empty())
        .map(|pair| match pair.split_once('=') {
            Some((name, value)) => (percent_decode(name), percent_decode(value)),
            None => (percent_decode(pair), String::new()),
        })
        .collect()
}

fn route_of(method: &str, path: &str) -> Option<Route> {
    Some(match (method, path) {
        ("POST", "/server/files/upload") => Route::Upload,
        ("GET", path) if path.starts_with("/server/files/gcodes/") => Route::Download,
        ("POST", "/printer/print/start") => Route::Start,
        ("POST", "/printer/print/pause") => Route::Pause,
        ("POST", "/printer/print/resume") => Route::Resume,
        ("POST", "/printer/print/cancel") => Route::Cancel,
        ("GET", "/server/info") => Route::ServerInfo,
        ("GET", "/printer/objects/list") => Route::ObjectsList,
        ("GET", "/printer/objects/query") => Route::ObjectsQuery,
        ("GET", "/server/history/list") => Route::History,
        ("GET", "/server/webcams/list") => Route::Webcams,
        _ => return None,
    })
}

struct Response {
    status: u16,
    body: Vec<u8>,
    content_type: &'static str,
}

impl Response {
    fn json(status: u16, body: Value) -> Self {
        Self {
            status,
            body: body.to_string().into_bytes(),
            content_type: "application/json; charset=UTF-8",
        }
    }

    fn ok_result() -> Self {
        Self::json(200, json!({"result": "ok"}))
    }

    fn result(body: Value) -> Self {
        Self::json(200, json!({ "result": body }))
    }

    /// Moonraker's error shape, with a traceback naming host file paths
    /// (spike Gate A).
    fn error(status: u16, message: &str) -> Self {
        Self::json(
            status,
            json!({"error": {
                "code": status,
                "message": message,
                "traceback": format!(
                    "Traceback (most recent call last):\n  File \"{TRACEBACK_MARKER}\", line 714, in _process_http_request\n...\ntornado.web.HTTPError: HTTP {status}: {message}\n"
                ),
            }}),
        )
    }
}

enum Outcome {
    Respond(Response),
    /// Close the connection without answering.
    Drop,
    /// A chunked 200 carrying these bytes, then a stall before the end.
    ChunkedThenStall(Vec<u8>, Duration),
}

struct RequestHead {
    method: String,
    target: String,
    headers: HashMap<String, String>,
}

fn read_head(reader: &mut BufReader<TcpStream>) -> Option<RequestHead> {
    let mut request_line = String::new();
    reader.read_line(&mut request_line).ok()?;
    let mut parts = request_line.split_whitespace();
    let method = parts.next()?.to_string();
    let target = parts.next()?.to_string();
    let mut headers = HashMap::new();
    loop {
        let mut line = String::new();
        if reader.read_line(&mut line).ok()? == 0 || line.trim().is_empty() {
            break;
        }
        if let Some((name, value)) = line.split_once(':') {
            headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
        }
    }
    Some(RequestHead {
        method,
        target,
        headers,
    })
}

fn read_body(reader: &mut BufReader<TcpStream>, head: &RequestHead) -> Option<Vec<u8>> {
    let chunked = head
        .headers
        .get("transfer-encoding")
        .is_some_and(|value| value.eq_ignore_ascii_case("chunked"));
    if chunked {
        let mut body = Vec::new();
        loop {
            let mut size_line = String::new();
            reader.read_line(&mut size_line).ok()?;
            let size = usize::from_str_radix(size_line.trim().split(';').next()?, 16).ok()?;
            if size == 0 {
                let mut trailer = String::new();
                reader.read_line(&mut trailer).ok()?;
                return Some(body);
            }
            let start = body.len();
            body.resize(start + size, 0);
            reader.read_exact(&mut body[start..]).ok()?;
            let mut crlf = [0u8; 2];
            reader.read_exact(&mut crlf).ok()?;
        }
    }
    let length: usize = head
        .headers
        .get("content-length")
        .and_then(|value| value.parse().ok())
        .unwrap_or(0);
    let mut body = vec![0u8; length];
    reader.read_exact(&mut body).ok()?;
    Some(body)
}

struct FormPart {
    name: String,
    file_name: Option<String>,
    data: Vec<u8>,
}

fn find(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    haystack
        .get(from..)?
        .windows(needle.len())
        .position(|window| window == needle)
        .map(|position| position + from)
}

fn disposition_param(disposition: &str, param: &str) -> Option<String> {
    disposition.split(';').find_map(|piece| {
        let (name, value) = piece.trim().split_once('=')?;
        (name.trim() == param).then(|| value.trim().trim_matches('"').to_string())
    })
}

fn parse_multipart(content_type: &str, body: &[u8]) -> Vec<FormPart> {
    let Some(boundary) = content_type
        .split(';')
        .find_map(|piece| piece.trim().strip_prefix("boundary="))
    else {
        return Vec::new();
    };
    let delimiter = format!("--{}", boundary.trim_matches('"')).into_bytes();
    let mut parts = Vec::new();
    let Some(mut cursor) = find(body, &delimiter, 0) else {
        return parts;
    };
    loop {
        cursor += delimiter.len();
        if body.get(cursor..cursor + 2) == Some(b"--") {
            break;
        }
        cursor += 2; // CRLF after the delimiter
        let Some(headers_end) = find(body, b"\r\n\r\n", cursor) else {
            break;
        };
        let headers = String::from_utf8_lossy(&body[cursor..headers_end]).into_owned();
        let data_start = headers_end + 4;
        let mut closing = b"\r\n".to_vec();
        closing.extend_from_slice(&delimiter);
        let Some(data_end) = find(body, &closing, data_start) else {
            break;
        };
        let disposition = headers
            .lines()
            .find_map(|line| {
                let (name, value) = line.split_once(':')?;
                name.trim()
                    .eq_ignore_ascii_case("content-disposition")
                    .then(|| value.trim().to_string())
            })
            .unwrap_or_default();
        parts.push(FormPart {
            name: disposition_param(&disposition, "name").unwrap_or_default(),
            file_name: disposition_param(&disposition, "filename"),
            data: body[data_start..data_end].to_vec(),
        });
        cursor = data_end + 2;
    }
    parts
}

fn write_response(stream: &mut TcpStream, response: &Response) {
    let head = format!(
        "HTTP/1.1 {} X\r\nContent-Type: {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        response.status,
        response.content_type,
        response.body.len()
    );
    let _ = stream.write_all(head.as_bytes());
    let _ = stream.write_all(&response.body);
    let _ = stream.flush();
}

fn handle_connection(stream: TcpStream, state: &Arc<Mutex<FakeState>>) {
    let Ok(clone) = stream.try_clone() else {
        return;
    };
    let mut reader = BufReader::new(clone);
    let mut stream = stream;
    let Some(head) = read_head(&mut reader) else {
        return;
    };
    let path = head
        .target
        .split('?')
        .next()
        .unwrap_or_default()
        .to_string();
    let route = route_of(&head.method, &path);
    let fault = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        route.and_then(|route| state.faults.get_mut(&route)?.pop_front())
    };

    if let Some(Fault::DropMidBody) = fault {
        let length: usize = head
            .headers
            .get("content-length")
            .and_then(|value| value.parse().ok())
            .unwrap_or(0);
        let mut half = vec![0u8; length / 2];
        let _ = reader.read_exact(&mut half);
        record(state, &head, Vec::new(), Vec::new());
        let _ = stream.shutdown(Shutdown::Both);
        return;
    }

    let Some(body) = read_body(&mut reader, &head) else {
        return;
    };
    let form = match route {
        Some(Route::Upload) => parse_multipart(
            head.headers
                .get("content-type")
                .map(String::as_str)
                .unwrap_or_default(),
            &body,
        ),
        _ => Vec::new(),
    };
    record(
        state,
        &head,
        form.iter().map(|part| part.name.clone()).collect(),
        form.iter()
            .filter(|part| part.file_name.is_none())
            .map(|part| {
                (
                    part.name.clone(),
                    String::from_utf8_lossy(&part.data).into_owned(),
                )
            })
            .collect(),
    );

    let mut delay = None;
    let outcome = {
        let mut state = state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let authorized = match &state.api_key {
            None => true,
            Some(key) => head.headers.get("x-api-key") == Some(key),
        };
        if !authorized {
            Outcome::Respond(Response::error(401, "Unauthorized"))
        } else {
            match fault {
                Some(Fault::Respond { status, message }) => {
                    Outcome::Respond(Response::error(status, &message))
                }
                Some(Fault::DelayResponse(duration)) => {
                    delay = Some(duration);
                    handle(&mut state, route, &head, &form, None)
                }
                other => handle(&mut state, route, &head, &form, other),
            }
        }
    };
    if let Some(delay) = delay {
        std::thread::sleep(delay);
    }
    match outcome {
        Outcome::Respond(response) => write_response(&mut stream, &response),
        Outcome::Drop => {
            let _ = stream.shutdown(Shutdown::Both);
        }
        Outcome::ChunkedThenStall(bytes, stall) => {
            write_chunked_then_stall(&mut stream, &bytes, stall)
        }
    }
}

fn write_chunked_then_stall(stream: &mut TcpStream, bytes: &[u8], stall: Duration) {
    let head = "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n";
    let _ = stream.write_all(head.as_bytes());
    for chunk in bytes.chunks(4096) {
        let _ = write!(stream, "{:x}\r\n", chunk.len());
        let _ = stream.write_all(chunk);
        let _ = stream.write_all(b"\r\n");
    }
    let _ = stream.flush();
    std::thread::sleep(stall);
    let _ = stream.write_all(b"0\r\n\r\n");
}

fn record(
    state: &Arc<Mutex<FakeState>>,
    head: &RequestHead,
    form_parts: Vec<String>,
    form_values: Vec<(String, String)>,
) {
    state
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .requests
        .push(SeenRequest {
            method: head.method.clone(),
            target: head.target.clone(),
            headers: head.headers.clone(),
            form_parts,
            form_values,
        });
}

fn handle(
    state: &mut FakeState,
    route: Option<Route>,
    head: &RequestHead,
    form: &[FormPart],
    fault: Option<Fault>,
) -> Outcome {
    let Some(route) = route else {
        return Outcome::Respond(Response::error(404, "Not Found"));
    };
    let query = query_pairs(&head.target);
    let klipper_answer = || -> Option<Response> {
        match state.klippy_state.as_str() {
            "ready" => None,
            "disconnected" => Some(Response::error(503, "Klippy Host not connected")),
            // Shutdown and startup reach HTTP as "Unknown" (spike Gate E).
            _ => Some(Response::error(400, "Unknown")),
        }
    };
    match route {
        Route::Upload => upload(state, form, fault),
        Route::Download => {
            let path = percent_decode(
                head.target
                    .split('?')
                    .next()
                    .unwrap_or_default()
                    .trim_start_matches("/server/files/gcodes/"),
            );
            if let (Some(Fault::ChunkedDownloadThenStall(stall)), Some(bytes)) =
                (&fault, state.file(&path))
            {
                return Outcome::ChunkedThenStall(bytes, *stall);
            }
            Outcome::Respond(match state.file(&path) {
                Some(bytes) => Response {
                    status: 200,
                    body: bytes,
                    content_type: "application/octet-stream",
                },
                None => Response::error(404, &format!("File does not exist: {path}")),
            })
        }
        Route::Start => {
            if let Some(answer) = klipper_answer() {
                return Outcome::Respond(answer);
            }
            let path = query
                .iter()
                .find(|(name, _)| name == "filename")
                .map(|(_, value)| value.clone())
                .unwrap_or_default();
            if state.print_state == "printing" {
                return Outcome::Respond(Response::error(400, "SD busy"));
            }
            if state.file(&path).is_none() {
                return Outcome::Respond(Response::error(400, "Unable to open file"));
            }
            match fault {
                Some(Fault::ApplyStartThenDrop(trace)) => {
                    state.apply_start(&path, trace);
                    Outcome::Drop
                }
                _ => {
                    state.apply_start(&path, StartTrace::PrintingWithHistoryJob);
                    Outcome::Respond(Response::ok_result())
                }
            }
        }
        Route::Pause | Route::Resume | Route::Cancel => {
            if let Some(answer) = klipper_answer() {
                return Outcome::Respond(answer);
            }
            if !matches!(fault, Some(Fault::OkWithoutEffect)) {
                control(state, route);
            }
            Outcome::Respond(Response::ok_result())
        }
        Route::ServerInfo => Outcome::Respond(Response::result(json!({
            "klippy_connected": state.klippy_state != "disconnected",
            "klippy_state": state.klippy_state,
            "components": state.components,
            "failed_components": [],
            "registered_directories": ["config", "logs", "gcodes"],
            "warnings": [],
            "websocket_count": 0,
            "moonraker_version": "v0.11.0-1-g1cfb0c4-prind",
            "missing_klippy_requirements": [],
            "api_version": [1, 5, 0],
            "api_version_string": "1.5.0",
        }))),
        Route::ObjectsList => Outcome::Respond(match state.klippy_state.as_str() {
            "disconnected" => Response::error(503, "Klippy Host not connected"),
            _ => Response::result(json!({ "objects": state.objects() })),
        }),
        Route::ObjectsQuery => {
            if state.klippy_state == "disconnected" {
                return Outcome::Respond(Response::error(503, "Klippy Host not connected"));
            }
            let mut status = serde_json::Map::new();
            for (name, _) in &query {
                if let Some(value) = object_status(state, name) {
                    status.insert(name.clone(), value);
                }
            }
            Outcome::Respond(Response::result(
                json!({"eventtime": 1234.5, "status": status}),
            ))
        }
        Route::History => Outcome::Respond(Response::result(history(state, &query))),
        Route::Webcams => Outcome::Respond(Response::result(json!({"webcams": state.webcams}))),
    }
}

fn upload(state: &mut FakeState, form: &[FormPart], fault: Option<Fault>) -> Outcome {
    if form.iter().any(|part| part.name == "print") {
        state.print_field_uploads += 1;
        return Outcome::Respond(Response::error(400, "farm3d must never send `print`"));
    }
    let text = |name: &str| {
        form.iter()
            .find(|part| part.name == name && part.file_name.is_none())
            .map(|part| String::from_utf8_lossy(&part.data).into_owned())
    };
    let Some(file) = form.iter().find(|part| part.name == "file") else {
        return Outcome::Respond(Response::error(400, "No file name specifed in upload form"));
    };
    let directory = text("path").unwrap_or_default();
    let file_name = file.file_name.clone().unwrap_or_default();
    let path = if directory.is_empty() {
        file_name
    } else {
        format!("{directory}/{file_name}")
    };
    if let Some(checksum) = text("checksum") {
        let actual = format!("{:x}", Sha256::digest(&file.data));
        if !checksum.eq_ignore_ascii_case(&actual) {
            return Outcome::Respond(Response::error(422, "Unprocessable Entity"));
        }
    }
    if state.is_loaded(&path) {
        return Outcome::Respond(Response::error(403, "Forbidden"));
    }
    match fault {
        Some(Fault::StoreThenDropResponse) => {
            state.files.insert(path, file.data.clone());
            Outcome::Drop
        }
        Some(Fault::DeliverLate(delay)) => {
            state
                .late_files
                .push((Instant::now() + delay, path, file.data.clone()));
            Outcome::Drop
        }
        Some(Fault::StoreDifferentBytes(bytes)) => {
            state.files.insert(path.clone(), bytes);
            Outcome::Respond(upload_created(&path, file.data.len()))
        }
        _ => {
            state.files.insert(path.clone(), file.data.clone());
            Outcome::Respond(upload_created(&path, file.data.len()))
        }
    }
}

/// Spike Gate B's 201 body.
fn upload_created(path: &str, size: usize) -> Response {
    Response::json(
        201,
        json!({
            "action": "create_file",
            "item": {"modified": now_epoch_s(), "size": size, "permissions": "rw",
                     "path": path, "root": "gcodes"},
            "print_started": false,
            "print_queued": false,
        }),
    )
}

/// Spike Gate E's control matrix, for the states the fake models.
fn control(state: &mut FakeState, route: Route) {
    match (route, state.print_state.as_str()) {
        (Route::Pause, "printing") => {
            state.print_state = "paused".to_string();
            state.is_paused = true;
        }
        // Pause from an idle printer only sets `is_paused` (Gate E).
        (Route::Pause, "paused") => {}
        (Route::Pause, _) => state.is_paused = true,
        (Route::Resume, "paused") => {
            state.print_state = "printing".to_string();
            state.is_paused = false;
        }
        (Route::Resume, _) => state.is_paused = false,
        (Route::Cancel, "printing" | "paused") => {
            state.print_state = "cancelled".to_string();
            state.is_paused = false;
            state.close_open_job("cancelled");
        }
        (Route::Cancel, _) => state.is_paused = false,
        _ => {}
    }
}

fn object_status(state: &FakeState, name: &str) -> Option<Value> {
    if let Some(index) = state.tool_names().iter().position(|tool| tool == name) {
        let (temperature, target) = state.tools[index];
        return Some(json!({"temperature": temperature, "target": target, "power": 0.0}));
    }
    Some(match name {
        "webhooks" => json!({"state": state.klippy_state, "state_message": "Printer is ready"}),
        "print_stats" => json!({
            "filename": state.print_filename, "state": state.print_state, "message": "",
            "total_duration": 0.0, "print_duration": 0.0, "filament_used": 0.0,
        }),
        "pause_resume" => json!({"is_paused": state.is_paused}),
        "heater_bed" => {
            let (temperature, target) = state.bed?;
            json!({"temperature": temperature, "target": target, "power": 0.0})
        }
        _ => return None,
    })
}

/// `server.history.list` as the v0.11.0 simulator answers it: `since`
/// filters on `start_time`, `order` defaults to `desc`, then `limit`.
fn history(state: &FakeState, query: &[(String, String)]) -> Value {
    let param = |name: &str| {
        query
            .iter()
            .find(|(key, _)| key == name)
            .map(|(_, value)| value.clone())
    };
    let since: Option<f64> = param("since").and_then(|value| value.parse().ok());
    let limit: usize = param("limit")
        .and_then(|value| value.parse().ok())
        .unwrap_or(50);
    let mut jobs: Vec<&FakeJob> = state
        .history
        .iter()
        .filter(|job| since.is_none_or(|since| job.start_time > since))
        .collect();
    if param("order").as_deref() != Some("asc") {
        jobs.reverse();
    }
    jobs.truncate(limit);
    json!({
        "count": jobs.len(),
        "jobs": jobs.iter().map(|job| json!({
            "job_id": job.job_id, "user": "_TRUSTED_USER_", "filename": job.filename,
            "status": job.status, "start_time": job.start_time, "end_time": null,
            "print_duration": 0.0, "total_duration": 0.0, "filament_used": 0.0,
            "metadata": {}, "auxiliary_data": [], "exists": true,
        })).collect::<Vec<_>>(),
    })
}
