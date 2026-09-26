//! P6 Task 12: the read-only real-hardware tier for the Moonraker capability
//! adapter.
//!
//! Real printers are read-only (owner answer 8). These tests may only probe,
//! subscribe, and query a real Moonraker, never upload, start, pause,
//! resume, cancel, send G-code, or restart anything. Two layers enforce it:
//!
//! - **The wrapper** ([`ReadOnlyMoonraker`], [`ReadOnlyFactory`]) exposes
//!   only probe, subscribe, `HostStateQuery`, `ArtifactStaging::locate`, and
//!   `CameraDiscovery`. The factory builds no `PrintControl` at all, and its
//!   staging object refuses `upload` without sending a byte.
//! - **The gate** ([`Gate`]) is a loopback proxy every request goes through.
//!   It records every HTTP request and every WebSocket JSON-RPC call, and
//!   forwards only `GET`s of a fixed list of read endpoints and the
//!   read-only RPC methods. Anything else (any `POST` or `DELETE`, any
//!   `printer.print.*` or `printer.gcode.*` call) is refused, recorded, and
//!   never reaches the host. Every test asserts that nothing was refused.
//!
//! The tests that touch a real host are `#[ignore]`d; `just p6-readonly`
//! runs them. The unit tests at the top run in the normal suite and prove
//! the gate's refusals against a local stand-in host.
//!
//! Environment (as `a0_moonraker_live.rs`):
//!
//! | Variable | Meaning |
//! | --- | --- |
//! | `FARM3D_MOONRAKER_HOST` | Required, with no default. The Moonraker host. It is never printed or written. |
//! | `FARM3D_MOONRAKER_PORT` | Optional. Defaults to 7125. |
//! | `FARM3D_MOONRAKER_API_KEY` | Optional. Sent as `X-Api-Key`. Never printed or written. |
//! | `FARM3D_MOONRAKER_API_KEY_FILE` | Optional. A file holding the key. |
//!
//! "Blocking the port on the farm3d machine" (ruling R3) is the gate's own
//! [`Gate::block`]: the local listener keeps accepting and drops every
//! connection unanswered. No firewall rule, and nothing reaches the host.

mod common;

use std::collections::HashMap;
use std::io::{Read, Write};
use std::net::{Shutdown, SocketAddr, TcpListener, TcpStream, ToSocketAddrs};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use farm3d_lib::connections::capabilities::{
    capabilities_for, ArtifactStaging, CameraDiscovery, CameraInfo, CapabilityEvidence,
    CapabilityKey, CapabilityMap, CapabilityState, CommandFailure, EvidenceTier, HistoryJob,
    HistoryQuery, HostFacts, HostJobState, HostOperationFailureCode, HostStateQuery,
    InconclusiveReason, KlippyState, LocateOutcome, PrintControl, PrinterCapabilities,
    StagedArtifact, UnsupportedReason,
};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::moonraker::control::{MoonrakerCapabilities, MoonrakerTimings};
use farm3d_lib::connections::moonraker::MoonrakerConnection;
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionError, ConnectionObservation, PrinterConnection, ProbeResult,
    DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND,
};
use farm3d_lib::host_ops::repository::{self as host_ops_repo, NewHostOperation, Outcome};
use farm3d_lib::host_ops::{
    CapabilityFactory, HostOperation, HostOperationEndpoint, HostOperationKind,
    HostOperationServices, HostOperationState, HostOpsTimings, SystemClock,
};
use farm3d_lib::persistence::Storage;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::spools::operations::OperationKind;
use serde_json::{json, Value};
use tauri::test::MockRuntime;

// ---------------------------------------------------------------------------
// The gate: a recording, write-refusing loopback proxy
// ---------------------------------------------------------------------------

/// Every `GET` path the adapters read (D10), without the query string.
const HTTP_READS: &[&str] = &[
    "/server/info",
    "/printer/info",
    "/printer/objects/list",
    "/printer/objects/query",
    "/server/history/list",
    "/server/webcams/list",
];

/// `locate`'s download: `GET /server/files/gcodes/<host_path>`.
const DOWNLOAD_PREFIX: &str = "/server/files/gcodes/";

/// The WebSocket upgrade the observation adapter opens.
const WEBSOCKET_PATH: &str = "/websocket";

/// Every JSON-RPC method `probe` and `subscribe` may call.
const RPC_READS: &[&str] = &[
    "server.info",
    "printer.info",
    "printer.objects.list",
    "printer.objects.query",
    "printer.objects.subscribe",
    "server.history.list",
    "server.webcams.list",
    "server.connection.identify",
];

/// The largest client frame the gate will inspect.
const MAX_FRAME: u64 = 1 << 20;

/// One request the gate or a wrapper saw.
#[derive(Clone, Debug, PartialEq)]
enum Seen {
    Http {
        method: String,
        target: String,
        forwarded: bool,
    },
    Rpc {
        method: String,
        forwarded: bool,
    },
    /// A write a wrapper refused before anything was sent.
    Refused {
        what: String,
    },
}

impl Seen {
    fn is_write(&self) -> bool {
        match self {
            Seen::Http { forwarded, .. } | Seen::Rpc { forwarded, .. } => !forwarded,
            Seen::Refused { .. } => true,
        }
    }
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<Seen>>>);

impl Log {
    fn push(&self, seen: Seen) {
        self.0.lock().unwrap().push(seen);
    }

    fn all(&self) -> Vec<Seen> {
        self.0.lock().unwrap().clone()
    }

    fn writes(&self) -> Vec<Seen> {
        self.all().into_iter().filter(Seen::is_write).collect()
    }
}

/// Where the gate forwards to. Deliberately not `Debug`: the real host's
/// address must never reach a log or a panic message.
struct Upstream {
    addresses: Vec<SocketAddr>,
}

impl Upstream {
    fn resolve(host: &str, port: u16) -> Self {
        let addresses: Vec<SocketAddr> = (host, port)
            .to_socket_addrs()
            .map(Iterator::collect)
            .unwrap_or_default();
        assert!(
            !addresses.is_empty(),
            "FARM3D_MOONRAKER_HOST does not resolve (the value is not printed)"
        );
        Self { addresses }
    }

    fn connect(&self) -> Option<TcpStream> {
        self.addresses
            .iter()
            .find_map(|address| TcpStream::connect_timeout(address, Duration::from_secs(5)).ok())
    }
}

/// A loopback proxy in front of one Moonraker. See the module docs.
struct Gate {
    local: SocketAddr,
    log: Log,
    blocked: Arc<AtomicBool>,
    blocked_connections: Arc<AtomicUsize>,
    stop: Arc<AtomicBool>,
    accept: Option<JoinHandle<()>>,
}

impl Gate {
    fn start(upstream: Upstream) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind the gate");
        listener.set_nonblocking(true).unwrap();
        let local = listener.local_addr().unwrap();
        let log = Log::default();
        let blocked = Arc::new(AtomicBool::new(false));
        let blocked_connections = Arc::new(AtomicUsize::new(0));
        let stop = Arc::new(AtomicBool::new(false));
        let upstream = Arc::new(upstream);
        let accept = {
            let (log, blocked, blocked_connections, stop) = (
                log.clone(),
                Arc::clone(&blocked),
                Arc::clone(&blocked_connections),
                Arc::clone(&stop),
            );
            std::thread::spawn(move || {
                while !stop.load(Ordering::SeqCst) {
                    match listener.accept() {
                        Ok((client, _)) => {
                            if blocked.load(Ordering::SeqCst) {
                                blocked_connections.fetch_add(1, Ordering::SeqCst);
                                let _ = client.shutdown(Shutdown::Both);
                                continue;
                            }
                            let (log, upstream) = (log.clone(), Arc::clone(&upstream));
                            std::thread::spawn(move || handle(client, &upstream, &log));
                        }
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        Err(_) => std::thread::sleep(Duration::from_millis(5)),
                    }
                }
            })
        };
        Self {
            local,
            log,
            blocked,
            blocked_connections,
            stop,
            accept: Some(accept),
        }
    }

    /// Blocks the port on this machine: the listener stays, but every new
    /// connection is dropped unanswered and counted. Nothing is forwarded.
    fn block(&self) {
        self.blocked.store(true, Ordering::SeqCst);
    }

    fn blocked_connections(&self) -> usize {
        self.blocked_connections.load(Ordering::SeqCst)
    }

    /// A Connection that reaches the host only through this gate.
    fn config(&self) -> ConnectionConfig {
        ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "127.0.0.1".to_string(),
            port: self.local.port(),
            use_tls: false,
            credential_ref: None,
        }
    }
}

impl Drop for Gate {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(accept) = self.accept.take() {
            let _ = accept.join();
        }
    }
}

struct Head {
    method: String,
    target: String,
    headers: HashMap<String, String>,
    raw: Vec<u8>,
    /// Bytes the client sent after the head.
    rest: Vec<u8>,
}

fn read_head(client: &mut TcpStream) -> Option<Head> {
    let mut buffer = Vec::new();
    let mut chunk = [0u8; 4096];
    let end = loop {
        if let Some(end) = buffer.windows(4).position(|window| window == b"\r\n\r\n") {
            break end + 4;
        }
        if buffer.len() > 64 * 1024 {
            return None;
        }
        let read = client.read(&mut chunk).ok()?;
        if read == 0 {
            return None;
        }
        buffer.extend_from_slice(&chunk[..read]);
    };
    let text = String::from_utf8_lossy(&buffer[..end]).into_owned();
    let mut lines = text.split("\r\n");
    let mut request_line = lines.next()?.split_whitespace();
    let method = request_line.next()?.to_string();
    let target = request_line.next()?.to_string();
    let headers = lines
        .filter_map(|line| line.split_once(':'))
        .map(|(name, value)| (name.trim().to_ascii_lowercase(), value.trim().to_string()))
        .collect();
    Some(Head {
        method,
        target,
        headers,
        raw: buffer[..end].to_vec(),
        rest: buffer[end..].to_vec(),
    })
}

fn is_upgrade(head: &Head) -> bool {
    head.headers
        .get("upgrade")
        .is_some_and(|value| value.eq_ignore_ascii_case("websocket"))
}

/// Whether the gate forwards this request: a body-less `GET` of a read
/// endpoint, or the WebSocket upgrade.
fn is_allowed_http(head: &Head) -> bool {
    let path = head.target.split('?').next().unwrap_or_default();
    let has_body = head.headers.contains_key("transfer-encoding")
        || head
            .headers
            .get("content-length")
            .is_some_and(|length| length.trim() != "0");
    if head.method != "GET" || has_body {
        return false;
    }
    // Path-exact: no dot segments and no encoded dots or slashes that could
    // step out of a listed prefix.
    let lowered = path.to_ascii_lowercase();
    if ["..", "%2e", "%2f", "%5c", "\\"]
        .iter()
        .any(|bad| lowered.contains(bad))
    {
        return false;
    }
    if is_upgrade(head) {
        path == WEBSOCKET_PATH
    } else {
        HTTP_READS.contains(&path) || path.starts_with(DOWNLOAD_PREFIX)
    }
}

fn refuse(client: &mut TcpStream) {
    let _ = client.write_all(
        b"HTTP/1.1 403 Forbidden\r\nContent-Type: application/json\r\nContent-Length: 58\r\nConnection: close\r\n\r\n{\"error\":{\"code\":403,\"message\":\"read-only gate refused\"}}",
    );
    let _ = client.shutdown(Shutdown::Both);
}

/// The request head with any `Connection` header replaced by `close`, so
/// the host answers once and closes: no second request can follow on the
/// same connection without passing the gate.
fn closing_head(head: &Head) -> Vec<u8> {
    let text = String::from_utf8_lossy(&head.raw);
    let mut out = String::new();
    for line in text.split("\r\n").filter(|line| !line.is_empty()) {
        if line.to_ascii_lowercase().starts_with("connection:") {
            continue;
        }
        out.push_str(line);
        out.push_str("\r\n");
    }
    out.push_str("Connection: close\r\n\r\n");
    out.into_bytes()
}

fn handle(mut client: TcpStream, upstream: &Upstream, log: &Log) {
    let _ = client.set_nonblocking(false);
    let _ = client.set_read_timeout(Some(Duration::from_secs(60)));
    let Some(head) = read_head(&mut client) else {
        return;
    };
    let forwarded = is_allowed_http(&head);
    log.push(Seen::Http {
        method: head.method.clone(),
        target: head.target.clone(),
        forwarded,
    });
    if !forwarded {
        refuse(&mut client);
        return;
    }
    let Some(mut host) = upstream.connect() else {
        let _ = client.shutdown(Shutdown::Both);
        return;
    };
    let _ = host.set_read_timeout(None);
    if is_upgrade(&head) {
        if host.write_all(&head.raw).is_err() {
            return;
        }
        let (Ok(mut from_host), Ok(mut to_client)) = (host.try_clone(), client.try_clone()) else {
            return;
        };
        let down = std::thread::spawn(move || {
            let _ = std::io::copy(&mut from_host, &mut to_client);
            let _ = to_client.shutdown(Shutdown::Both);
        });
        let _ = client.set_read_timeout(None);
        let mut from_client = std::io::Cursor::new(head.rest).chain(client.try_clone().unwrap());
        inspect_client_frames(&mut from_client, &mut host, log);
        let _ = host.shutdown(Shutdown::Both);
        let _ = client.shutdown(Shutdown::Both);
        let _ = down.join();
    } else {
        if host.write_all(&closing_head(&head)).is_err() {
            return;
        }
        let _ = std::io::copy(&mut host, &mut client);
        let _ = client.shutdown(Shutdown::Both);
    }
}

/// One WebSocket frame from the client, as read off the wire.
struct Frame {
    raw: Vec<u8>,
    fin: bool,
    opcode: u8,
    payload: Vec<u8>,
}

fn read_frame(reader: &mut impl Read) -> std::io::Result<Frame> {
    let mut raw = vec![0u8; 2];
    reader.read_exact(&mut raw)?;
    let fin = raw[0] & 0x80 != 0;
    let opcode = raw[0] & 0x0f;
    let masked = raw[1] & 0x80 != 0;
    let mut length = u64::from(raw[1] & 0x7f);
    if length == 126 {
        let mut extended = [0u8; 2];
        reader.read_exact(&mut extended)?;
        raw.extend_from_slice(&extended);
        length = u64::from(u16::from_be_bytes(extended));
    } else if length == 127 {
        let mut extended = [0u8; 8];
        reader.read_exact(&mut extended)?;
        raw.extend_from_slice(&extended);
        length = u64::from_be_bytes(extended);
    }
    if length > MAX_FRAME {
        return Err(std::io::Error::other("frame too large"));
    }
    let mut mask = [0u8; 4];
    if masked {
        reader.read_exact(&mut mask)?;
        raw.extend_from_slice(&mask);
    }
    let mut payload = vec![0u8; length as usize];
    reader.read_exact(&mut payload)?;
    raw.extend_from_slice(&payload);
    if masked {
        for (index, byte) in payload.iter_mut().enumerate() {
            *byte ^= mask[index % 4];
        }
    }
    Ok(Frame {
        raw,
        fin,
        opcode,
        payload,
    })
}

/// What the gate does with one client frame: `Ok(Some(method))` forwards an
/// allowed RPC, `Ok(None)` forwards a control frame, and `Err(what)` refuses
/// and closes.
fn frame_verdict(frame: &Frame) -> Result<Option<String>, String> {
    match frame.opcode {
        0x8..=0xA => Ok(None),
        0x1 if frame.fin => {
            let method = serde_json::from_slice::<Value>(&frame.payload)
                .ok()
                .and_then(|value| value["method"].as_str().map(str::to_string))
                .unwrap_or_else(|| "<no method>".to_string());
            if RPC_READS.contains(&method.as_str()) {
                Ok(Some(method))
            } else {
                Err(method)
            }
        }
        _ => Err("<a fragmented or binary frame>".to_string()),
    }
}

fn inspect_client_frames(from_client: &mut impl Read, host: &mut TcpStream, log: &Log) {
    while let Ok(frame) = read_frame(from_client) {
        match frame_verdict(&frame) {
            Ok(method) => {
                if let Some(method) = method {
                    log.push(Seen::Rpc {
                        method,
                        forwarded: true,
                    });
                }
                if host.write_all(&frame.raw).is_err() {
                    return;
                }
            }
            Err(method) => {
                log.push(Seen::Rpc {
                    method,
                    forwarded: false,
                });
                return;
            }
        }
    }
}

// ---------------------------------------------------------------------------
// The read-only wrapper
// ---------------------------------------------------------------------------

/// The only way these tests reach a host: probe, subscribe, and the three
/// read capabilities, all through a [`Gate`]. It has no write method.
struct ReadOnlyMoonraker {
    gate: Gate,
    api_key: Option<String>,
    /// The read capabilities, held only behind their read traits (and a
    /// staging object whose `upload` refuses), so no write is callable.
    host_state: Box<dyn HostStateQuery>,
    camera: Box<dyn CameraDiscovery>,
    staging: ReadOnlyStaging,
}

fn read_timings() -> MoonrakerTimings {
    MoonrakerTimings {
        connect: Duration::from_secs(5),
        query: Duration::from_secs(10),
        ..MoonrakerTimings::default()
    }
}

impl ReadOnlyMoonraker {
    fn new(upstream: Upstream, api_key: Option<String>) -> Self {
        let gate = Gate::start(upstream);
        let adapter = || {
            MoonrakerCapabilities::new(
                &gate.config(),
                api_key.clone().map(zeroize::Zeroizing::new),
                read_timings(),
            )
        };
        let host_state: Box<dyn HostStateQuery> = Box::new(adapter());
        let camera: Box<dyn CameraDiscovery> = Box::new(adapter());
        let staging = ReadOnlyStaging {
            inner: adapter(),
            log: gate.log.clone(),
        };
        Self {
            gate,
            api_key,
            host_state,
            camera,
            staging,
        }
    }

    fn observer(&self) -> MoonrakerConnection {
        MoonrakerConnection::new(self.gate.config(), self.api_key.clone())
    }

    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        self.observer().probe().await
    }

    /// Subscribes for `duration` and returns what arrived.
    async fn subscribe_for(&self, duration: Duration) -> Vec<ConnectionObservation> {
        let (tx, mut rx) = tokio::sync::mpsc::channel(256);
        let observer = self.observer();
        let task = tokio::spawn(async move { observer.subscribe(tx).await });
        let mut seen = Vec::new();
        let _ = tokio::time::timeout(duration, async {
            while let Some(observation) = rx.recv().await {
                seen.push(observation);
            }
        })
        .await;
        task.abort();
        seen
    }

    async fn host_facts(&self) -> Result<HostFacts, ConnectionError> {
        self.host_state.host_facts().await
    }

    async fn host_job_state(&self) -> Result<HostJobState, ConnectionError> {
        self.host_state.host_job_state().await
    }

    async fn job_history(&self, query: HistoryQuery) -> Result<Vec<HistoryJob>, ConnectionError> {
        self.host_state.job_history(query).await
    }

    async fn locate(&self, artifact: &StagedArtifact) -> Result<LocateOutcome, ConnectionError> {
        self.staging.locate(artifact).await
    }

    async fn cameras(&self) -> Result<Vec<CameraInfo>, ConnectionError> {
        self.camera.cameras().await
    }

    fn seen(&self) -> Vec<Seen> {
        self.gate.log.all()
    }

    /// Everything refused: any write that was attempted.
    fn writes(&self) -> Vec<Seen> {
        self.gate.log.writes()
    }

    /// Fails unless every request was a forwarded read.
    fn assert_no_writes(&self) {
        let writes = self.writes();
        assert!(writes.is_empty(), "writes were attempted: {writes:?}");
        for seen in self.seen() {
            if let Seen::Http { method, .. } = &seen {
                assert_eq!(method, "GET", "{seen:?}");
            }
        }
    }
}

/// `locate` through the gate; `upload` refused before anything is sent.
struct ReadOnlyStaging {
    inner: MoonrakerCapabilities,
    log: Log,
}

#[async_trait::async_trait]
impl ArtifactStaging for ReadOnlyStaging {
    async fn upload(
        &self,
        artifact: &StagedArtifact,
        _body: Box<dyn tokio::io::AsyncRead + Send + Unpin>,
    ) -> Result<(), CommandFailure> {
        self.log.push(Seen::Refused {
            what: format!("upload {}", artifact.host_path),
        });
        Err(CommandFailure::Definitive(
            HostOperationFailureCode::HostRejected,
        ))
    }

    async fn locate(&self, artifact: &StagedArtifact) -> Result<LocateOutcome, ConnectionError> {
        self.inner.locate(artifact).await
    }
}

/// The capability factory `host_ops` gets in these tests: the three read
/// capabilities (at `readOnlyHardware` tier), every write unsupported, and
/// no `PrintControl` object at all. It only ever builds adapters aimed at a
/// loopback address (the gate).
struct ReadOnlyFactory {
    log: Log,
}

const NEVER_WRITES: &str = "The read-only suite never writes.";

/// D6's write capabilities.
fn is_write(key: CapabilityKey) -> bool {
    matches!(
        key,
        CapabilityKey::Upload
            | CapabilityKey::Start
            | CapabilityKey::Pause
            | CapabilityKey::Resume
            | CapabilityKey::Cancel
    )
}

fn assert_gated(config: &ConnectionConfig) {
    assert_eq!(
        config.host, "127.0.0.1",
        "a read-only adapter must go through the gate"
    );
}

impl CapabilityFactory for ReadOnlyFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        let mut capabilities = capabilities_for(printer, host_facts);
        capabilities.capabilities = CapabilityMap::complete(|key| {
            if is_write(key) {
                CapabilityState::Unsupported {
                    reason: UnsupportedReason::NotVerified,
                    detail: NEVER_WRITES.to_string(),
                }
            } else {
                CapabilityState::Supported {
                    evidence: CapabilityEvidence {
                        source: "tests/p6_moonraker_readonly.rs".to_string(),
                        tier: EvidenceTier::ReadOnlyHardware,
                        verified_host_versions: Vec::new(),
                    },
                }
            }
        });
        capabilities
    }

    fn staging(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn ArtifactStaging>> {
        assert_gated(config);
        Some(Box::new(ReadOnlyStaging {
            inner: MoonrakerCapabilities::new(config, key, read_timings()),
            log: self.log.clone(),
        }))
    }

    fn control(
        &self,
        _config: &ConnectionConfig,
        _key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        self.log.push(Seen::Refused {
            what: "a print-control object".to_string(),
        });
        None
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        assert_gated(config);
        Some(Box::new(MoonrakerCapabilities::new(
            config,
            key,
            read_timings(),
        )))
    }
}

// ---------------------------------------------------------------------------
// The gate's refusals, against a local stand-in host (runs in CI)
// ---------------------------------------------------------------------------

/// A stand-in host that answers `200 {}` to anything and records each
/// request line it receives, so a test can show a refused request never
/// arrived.
fn stand_in_host() -> (SocketAddr, Arc<Mutex<Vec<String>>>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let address = listener.local_addr().unwrap();
    let received = Arc::new(Mutex::new(Vec::new()));
    let sink = Arc::clone(&received);
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { continue };
            let sink = Arc::clone(&sink);
            std::thread::spawn(move || {
                if let Some(head) = read_head(&mut stream) {
                    sink.lock()
                        .unwrap()
                        .push(format!("{} {}", head.method, head.target));
                    let _ = stream.write_all(
                        b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 2\r\nConnection: close\r\n\r\n{}",
                    );
                }
            });
        }
    });
    (address, received)
}

fn raw_request(gate: &Gate, method: &str, target: &str) -> Option<u16> {
    let mut stream = TcpStream::connect(gate.local).unwrap();
    stream
        .set_read_timeout(Some(Duration::from_secs(5)))
        .unwrap();
    let request = format!(
        "{method} {target} HTTP/1.1\r\nHost: {}\r\nContent-Length: 0\r\n\r\n",
        gate.local
    );
    stream.write_all(request.as_bytes()).ok()?;
    let mut response = Vec::new();
    let _ = stream.read_to_end(&mut response);
    String::from_utf8_lossy(&response)
        .split_whitespace()
        .nth(1)
        .and_then(|code| code.parse().ok())
}

fn stand_in_gate() -> (Gate, Arc<Mutex<Vec<String>>>) {
    let (address, received) = stand_in_host();
    let gate = Gate::start(Upstream {
        addresses: vec![address],
    });
    (gate, received)
}

#[test]
fn the_gate_forwards_reads_and_refuses_every_write_without_forwarding_it() {
    let (gate, received) = stand_in_gate();

    assert_eq!(raw_request(&gate, "GET", "/server/info"), Some(200));
    assert_eq!(
        raw_request(&gate, "GET", "/server/files/gcodes/farm3d/x.gcode"),
        Some(200)
    );
    for (method, target) in [
        ("POST", "/printer/print/start?filename=farm3d%2Fx.gcode"),
        ("POST", "/printer/print/pause"),
        ("POST", "/server/files/upload"),
        ("POST", "/printer/gcode/script"),
        ("DELETE", "/server/files/gcodes/farm3d/x.gcode"),
        ("PUT", "/server/info"),
        // A GET that is not on the read list is refused too.
        ("GET", "/printer/gcode/script?script=M112"),
        ("GET", "/machine/reboot"),
        // Nothing may step out of the download prefix.
        ("GET", "/server/files/gcodes/../../machine/reboot"),
        ("GET", "/server/files/gcodes/%2e%2e/%2E%2E/machine/reboot"),
        ("GET", "/server/files/gcodes/farm3d%2F..%2Fx.gcode"),
    ] {
        assert_eq!(
            raw_request(&gate, method, target),
            Some(403),
            "{method} {target}"
        );
    }

    let received = received.lock().unwrap().clone();
    assert_eq!(
        received,
        vec![
            "GET /server/info".to_string(),
            "GET /server/files/gcodes/farm3d/x.gcode".to_string()
        ],
        "only the two reads reached the host"
    );
    let writes = gate.log.writes();
    assert_eq!(writes.len(), 11, "{writes:?}");
    assert!(writes.iter().all(|seen| matches!(
        seen,
        Seen::Http {
            forwarded: false,
            ..
        }
    )));
}

#[test]
fn the_gate_stops_the_production_adapters_writes() {
    let (gate, received) = stand_in_gate();
    let adapter = MoonrakerCapabilities::new(&gate.config(), None, read_timings());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();

    runtime.block_on(async {
        assert!(adapter.start("farm3d/x.gcode").await.is_err());
        assert!(adapter.pause().await.is_err());
        assert!(adapter.resume().await.is_err());
        assert!(adapter.cancel().await.is_err());
        let artifact = StagedArtifact {
            host_path: "farm3d/x.gcode".to_string(),
            sha256: "0".repeat(64),
            size: 3,
        };
        assert!(adapter
            .upload(&artifact, Box::new(std::io::Cursor::new(b"abc".to_vec())))
            .await
            .is_err());
    });

    assert!(
        received.lock().unwrap().is_empty(),
        "no write reached the host"
    );
    let methods: Vec<String> = gate
        .log
        .writes()
        .into_iter()
        .map(|seen| match seen {
            Seen::Http { method, target, .. } => format!("{method} {target}"),
            other => format!("{other:?}"),
        })
        .collect();
    assert_eq!(
        methods,
        vec![
            "POST /printer/print/start?filename=farm3d%2Fx.gcode",
            "POST /printer/print/pause",
            "POST /printer/print/resume",
            "POST /printer/print/cancel",
            "POST /server/files/upload",
        ]
    );
}

#[test]
fn a_blocked_gate_drops_connections_unanswered_and_counts_them() {
    let (gate, received) = stand_in_gate();
    gate.block();
    assert_eq!(raw_request(&gate, "GET", "/server/info"), None);
    assert_eq!(gate.blocked_connections(), 1);
    assert!(received.lock().unwrap().is_empty());
    assert!(gate.log.all().is_empty());
}

fn masked_text_frame(text: &str) -> Vec<u8> {
    let payload = text.as_bytes();
    let mask = [0x12u8, 0x34, 0x56, 0x78];
    let mut frame = vec![0x81u8];
    if payload.len() < 126 {
        frame.push(0x80 | payload.len() as u8);
    } else {
        frame.push(0x80 | 126);
        frame.extend_from_slice(&(payload.len() as u16).to_be_bytes());
    }
    frame.extend_from_slice(&mask);
    frame.extend(
        payload
            .iter()
            .enumerate()
            .map(|(index, byte)| byte ^ mask[index % 4]),
    );
    frame
}

#[test]
fn the_gate_refuses_every_rpc_that_is_not_a_read() {
    let verdict = |text: &str| {
        let bytes = masked_text_frame(text);
        let frame = read_frame(&mut std::io::Cursor::new(bytes.clone())).unwrap();
        assert_eq!(frame.raw, bytes, "the raw frame is forwarded unchanged");
        frame_verdict(&frame)
    };
    for method in [
        "server.info",
        "printer.objects.subscribe",
        "printer.objects.query",
    ] {
        assert_eq!(
            verdict(&json!({"jsonrpc": "2.0", "method": method, "id": 1}).to_string()),
            Ok(Some(method.to_string()))
        );
    }
    for method in [
        "printer.print.start",
        "printer.print.pause",
        "printer.print.resume",
        "printer.print.cancel",
        "printer.gcode.script",
        "printer.emergency_stop",
        "printer.firmware_restart",
        "server.files.delete_file",
        "machine.reboot",
    ] {
        assert_eq!(
            verdict(
                &json!({"jsonrpc": "2.0", "method": method, "params": {"filename": "x"}, "id": 1})
                    .to_string()
            ),
            Err(method.to_string()),
            "{method}"
        );
    }
    assert!(verdict("not json").is_err());
    // A fragmented text frame can't be inspected whole, so it's refused.
    let mut fragment = masked_text_frame(r#"{"method":"server.info"}"#);
    fragment[0] = 0x01;
    let frame = read_frame(&mut std::io::Cursor::new(fragment)).unwrap();
    assert!(frame_verdict(&frame).is_err());
}

#[test]
fn the_read_only_factory_builds_no_control_and_refuses_uploads_unsent() {
    let (gate, received) = stand_in_gate();
    let factory = ReadOnlyFactory {
        log: gate.log.clone(),
    };
    assert!(factory.control(&gate.config(), None).is_none());
    let staging = factory.staging(&gate.config(), None).unwrap();
    let artifact = StagedArtifact {
        host_path: "farm3d/x.gcode".to_string(),
        sha256: "0".repeat(64),
        size: 3,
    };
    let result = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(staging.upload(&artifact, Box::new(std::io::Cursor::new(b"abc".to_vec()))));
    assert!(result.is_err());
    assert!(received.lock().unwrap().is_empty(), "nothing was sent");
    assert_eq!(gate.log.writes().len(), 2, "{:?}", gate.log.writes());

    let printer = StoredPrinter {
        connection: Some(gate.config()),
        ..common::a_stored_printer("printer-ro")
    };
    let capabilities = factory.capabilities(&printer, None);
    for key in CapabilityKey::ALL {
        let supported = matches!(
            capabilities.capabilities[key],
            CapabilityState::Supported { .. }
        );
        assert_eq!(supported, !is_write(key), "{key:?}");
    }
}

// ---------------------------------------------------------------------------
// The real host (ignored; `just p6-readonly`)
// ---------------------------------------------------------------------------

/// The real host's settings. Not `Debug`, and never printed.
struct Live {
    host: String,
    port: u16,
    api_key: Option<String>,
}

impl Live {
    fn from_env() -> Self {
        let host = std::env::var("FARM3D_MOONRAKER_HOST").unwrap_or_default();
        assert!(
            !host.trim().is_empty(),
            "set FARM3D_MOONRAKER_HOST to the Moonraker host (it has no default)"
        );
        let port = std::env::var("FARM3D_MOONRAKER_PORT")
            .ok()
            .map(|value| value.parse().expect("FARM3D_MOONRAKER_PORT is not a port"))
            .unwrap_or(DEFAULT_MOONRAKER_PORT);
        let api_key = std::env::var("FARM3D_MOONRAKER_API_KEY")
            .ok()
            .or_else(|| {
                std::env::var("FARM3D_MOONRAKER_API_KEY_FILE")
                    .ok()
                    .map(|path| {
                        std::fs::read_to_string(path)
                            .expect("read FARM3D_MOONRAKER_API_KEY_FILE (the path is not printed)")
                    })
            })
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        Self {
            host: host.trim().to_string(),
            port,
            api_key,
        }
    }

    fn read_only(&self) -> ReadOnlyMoonraker {
        ReadOnlyMoonraker::new(
            Upstream::resolve(&self.host, self.port),
            self.api_key.clone(),
        )
    }
}

fn never_sent_path() -> String {
    format!("farm3d/{}.gcode", uuid::Uuid::new_v4())
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real hardware, read-only: FARM3D_MOONRAKER_HOST=<host> just p6-readonly"]
async fn readonly_probe_subscribe_and_capability_detection() {
    let live = Live::from_env();
    let host = live.read_only();

    let probe = host.probe().await.expect("probe");
    assert_eq!(probe.kind, MOONRAKER_KIND);
    let observations = host.subscribe_for(Duration::from_secs(5)).await;
    assert!(
        observations
            .iter()
            .any(|o| matches!(o, ConnectionObservation::Telemetry(_))),
        "no telemetry within 5 s"
    );

    let facts = host.host_facts().await.expect("host facts");
    let cameras = host.cameras().await.expect("cameras");
    assert!(facts.tool_count >= 1, "{facts:?}");
    assert_eq!(facts.camera_count as usize, cameras.len());
    let history = host
        .job_history(HistoryQuery {
            since_epoch_s: None,
            limit: 50,
        })
        .await
        .expect("history");
    println!(
        "P6 read-only: {} API {}; klippy {}; tools {}; bed {}; virtual_sdcard {}; \
         pause_resume {}; history {} ({} jobs on the first page); cameras {}",
        facts.host_software,
        facts.api_version,
        probe.state,
        facts.tool_count,
        facts.has_heater_bed,
        facts.has_virtual_sdcard,
        facts.has_pause_resume,
        facts.has_history,
        history.len(),
        cameras.len(),
    );
    host.assert_no_writes();
    println!("P6 read-only: {} reads, 0 writes", host.seen().len());
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real hardware, read-only: FARM3D_MOONRAKER_HOST=<host> just p6-readonly"]
async fn readonly_host_job_state_reports_every_tool() {
    let live = Live::from_env();
    let host = live.read_only();

    let facts = host.host_facts().await.expect("host facts");
    let state = host.host_job_state().await.expect("host job state");
    if state.klippy_state == KlippyState::Ready {
        let indices: Vec<u32> = state.tools.iter().map(|tool| tool.index).collect();
        assert_eq!(indices, (0..facts.tool_count).collect::<Vec<_>>());
        assert_eq!(state.bed.is_some(), facts.has_heater_bed);
        assert!(state.print.is_some());
    }
    println!(
        "P6 read-only: klippy {:?}, print {:?}, tools {}",
        state.klippy_state,
        state.print.as_ref().map(|print| &print.state),
        state.tools.len()
    );
    host.assert_no_writes();
}

#[tokio::test(flavor = "multi_thread")]
#[ignore = "real hardware, read-only: FARM3D_MOONRAKER_HOST=<host> just p6-readonly"]
async fn readonly_locate_of_a_never_sent_file_is_absent() {
    let live = Live::from_env();
    let host = live.read_only();

    let outcome = host
        .locate(&StagedArtifact {
            host_path: never_sent_path(),
            sha256: "0".repeat(64),
            size: 1,
        })
        .await
        .expect("locate");
    assert_eq!(outcome, LocateOutcome::Absent);
    host.assert_no_writes();
}

// --- host_ops against the real host, read-only --------------------------------------

struct ReadOnlyApp {
    _storage_dir: tempfile::TempDir,
    _lease: farm3d_lib::persistence::MetadataRootLease,
    _credentials: tempfile::TempDir,
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    storage: Arc<Storage>,
}

const CREDENTIAL_REF: &str = "farm3d/printer/readonly/apikey";

fn no_observation_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

impl ReadOnlyApp {
    /// A mock app whose `host_ops` builds adapters only through
    /// [`ReadOnlyFactory`], with the production timings (a 60 s settle
    /// period), and Printers `printer_ids` whose Connection is the gate.
    fn boot(host: &ReadOnlyMoonraker, printer_ids: &[&str]) -> Self {
        let (storage_dir, lease, storage, _database) = common::storage();
        let credentials = tempfile::tempdir().unwrap();
        let mut config = host.gate.config();
        if let Some(key) = &host.api_key {
            CredentialStore::file_backed(credentials.path().to_path_buf())
                .set(CREDENTIAL_REF, key)
                .unwrap();
            config.credential_ref = Some(CREDENTIAL_REF.to_string());
        }
        let printers = PrinterRepository::new(Arc::clone(&storage));
        for id in printer_ids {
            printers
                .create(StoredPrinter {
                    connection: Some(config.clone()),
                    ..common::a_stored_printer(id)
                })
                .unwrap();
        }
        let factory: Arc<dyn CapabilityFactory> = Arc::new(ReadOnlyFactory {
            log: host.gate.log.clone(),
        });
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::host_ops::commands::reconcile_host_operation,
                farm3d_lib::host_ops::commands::abandon_host_operation,
            ],
            Arc::clone(&storage),
            Arc::new(common::a_catalog()),
            credentials.path().to_path_buf(),
            no_observation_factory,
            move |services| {
                services.host_ops = Arc::new(HostOperationServices::new(
                    Arc::clone(&services.storage),
                    Arc::clone(&services.library.content),
                    Arc::clone(&services.manager),
                    factory,
                    Arc::new(SystemClock),
                    HostOpsTimings::default(),
                ));
            },
        );
        farm3d_lib::start_host_ops_runtime(&services, app.handle());
        Self {
            _storage_dir: storage_dir,
            _lease: lease,
            _credentials: credentials,
            _app: app,
            webview,
            storage,
        }
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    fn reconcile(&self, id: &str) -> HostOperation {
        serde_json::from_value(
            self.call("reconcile_host_operation", json!({"hostOperationId": id}))
                .unwrap_or_else(|error| panic!("reconcile: {error}")),
        )
        .unwrap()
    }

    fn abandon(&self, id: &str) -> HostOperation {
        serde_json::from_value(
            self.call(
                "abandon_host_operation",
                json!({
                    "operationId": format!("op-abandon-{id}"), "hostOperationId": id,
                    "acknowledgement": "hostStateUnknown",
                }),
            )
            .unwrap_or_else(|error| panic!("abandon: {error}")),
        )
        .unwrap()
    }

    /// A local `uncertain` upload row for a file farm3d never sent, aimed
    /// at the gate, whose `uncertain_since` is `age` ago. Nothing is sent to
    /// create it.
    fn seed_uncertain_upload(&self, printer_id: &str, gate: &Gate, age: Duration) -> String {
        let new = NewHostOperation {
            operation_id: format!("seed-{}", uuid::Uuid::new_v4()),
            operation_kind: OperationKind::StageSliceRevision,
            request_digest: "seed".to_string(),
            printer_id: printer_id.to_string(),
            kind: HostOperationKind::Upload,
            slice_revision_id: None,
            source_host_operation_id: None,
            gcode_sha256: Some("0".repeat(64)),
            gcode_size: Some(100),
            host_path: never_sent_path(),
            history_mark: None,
            endpoint: HostOperationEndpoint {
                kind: MOONRAKER_KIND.to_string(),
                host: "127.0.0.1".to_string(),
                port: gate.local.port(),
            },
        };
        let since = (chrono::Utc::now() - chrono::Duration::from_std(age).unwrap())
            .format("%Y-%m-%dT%H:%M:%SZ")
            .to_string();
        self.storage
            .write_repo(|tx| {
                let row = host_ops_repo::insert_dispatching(tx, &new)?;
                host_ops_repo::mark_sent(tx, &row.id)?;
                let row = host_ops_repo::transition(
                    tx,
                    &row.id,
                    Outcome::Uncertain {
                        reason: InconclusiveReason::ResponseLost,
                        no_longer_pending: false,
                    },
                )?;
                tx.execute(
                    "UPDATE host_operations SET uncertain_since = ?1 WHERE id = ?2",
                    [&since, &row.id],
                )
                .map_err(farm3d_lib::persistence::RepositoryError::from)?;
                Ok(row.id)
            })
            .unwrap()
    }
}

fn reason(row: &HostOperation) -> Option<InconclusiveReason> {
    row.last_attempt.as_ref().map(|attempt| attempt.reason)
}

#[test]
#[ignore = "real hardware, read-only: FARM3D_MOONRAKER_HOST=<host> just p6-readonly"]
fn readonly_an_old_uncertain_upload_fails_not_applied_and_a_new_one_stays_uncertain() {
    let live = Live::from_env();
    let host = live.read_only();
    // One Printer (a Connection's endpoint is unique), so one unresolved
    // row at a time: the old row first, then the new one.
    let app = ReadOnlyApp::boot(&host, &["printer-readonly"]);
    let old = app.seed_uncertain_upload("printer-readonly", &host.gate, Duration::from_secs(120));
    let row = app.reconcile(&old);
    assert_eq!(row.state, HostOperationState::Failed, "{row:?}");
    assert_eq!(
        row.failure.as_ref().map(|failure| failure.code),
        Some(HostOperationFailureCode::NotApplied)
    );

    let new = app.seed_uncertain_upload("printer-readonly", &host.gate, Duration::ZERO);
    let row = app.reconcile(&new);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::UploadSettling));
    assert_eq!(row.attempts, 1);

    host.assert_no_writes();
    println!(
        "P6 read-only: the old row failed notApplied, the new row stayed uncertain; {} reads, 0 writes",
        host.seen().len()
    );
}

#[test]
#[ignore = "real hardware, read-only: FARM3D_MOONRAKER_HOST=<host> just p6-readonly"]
fn readonly_a_blocked_port_keeps_the_row_uncertain_and_abandon_is_local_only() {
    let live = Live::from_env();
    let host = live.read_only();
    let app = ReadOnlyApp::boot(&host, &["printer-blocked"]);
    let id = app.seed_uncertain_upload("printer-blocked", &host.gate, Duration::from_secs(120));

    host.gate.block();
    let row = app.reconcile(&id);
    assert_eq!(row.state, HostOperationState::Uncertain, "{row:?}");
    assert_eq!(reason(&row), Some(InconclusiveReason::HostUnreachable));
    assert_eq!(row.attempts, 1);
    let tried = host.gate.blocked_connections();
    assert!(tried >= 1, "the check tried the blocked port");

    let row = app.abandon(&id);
    assert_eq!(row.state, HostOperationState::Abandoned, "{row:?}");
    assert_eq!(
        host.gate.blocked_connections(),
        tried,
        "abandon opened no connection"
    );
    host.assert_no_writes();
    assert!(host.seen().is_empty(), "nothing reached the host at all");
}
