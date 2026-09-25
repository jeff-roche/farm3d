//! The ElegooLink fake: an in-process SDCP server built only from recorded
//! real-hardware captures (ADR-0012).
//!
//! Elegoo publishes no simulator, so ElegooLink gets this fake instead of a
//! container. It runs inside the test process, so tests that use it are
//! hermetic and belong in the normal `cargo test` suite.
//!
//! **Rule: the fake does only what a capture shows a real printer doing.**
//! Nothing comes from desk research, public write-ups, or guesses. Every
//! behavior below is derived from #8's redacted captures at load time, not
//! hard-coded:
//!
//! - **Replies.** A request is answered only if a capture holds the same
//!   request (same `Cmd` *and* same `Data`). The reply is the recorded ack,
//!   with the caller's `RequestID` put in (captured acks echo the request's
//!   `RequestID`), followed by the frames the printer sent after that ack
//!   (for example `Attributes` after Cmd 1, `Status` after Cmd 0). Recorded
//!   `Status` frames are replayed in rotation, byte for byte, so quirks such
//!   as the hex-encoded extrusion keys survive.
//! - **No push.** The captures show `Status` only in reply to Cmd 0, so the
//!   fake never sends anything unprompted.
//! - **No pong.** Recorded text `ping` frames got no reply, so the fake
//!   ignores them (they still count as client activity).
//! - **Silent drop.** A capture in which the client went quiet ends with the
//!   printer closing the socket without a close frame (code 1006). The idle
//!   time before that drop is measured from the captures (about 61 s) and
//!   the fake drops a silent client the same way. Tests may shorten it.
//! - **Discovery.** A UDP datagram matching a recorded discovery request
//!   gets the recorded reply.
//!
//! Anything else (every unrecorded command) gets no answer and is listed by
//! [`FakeSdcp::unmodelled`], so a test can assert that it stayed inside
//! what the captures cover.
//!
//! **Command behavior is pending.** Upload, start, pause, resume, cancel,
//! and camera control may be modelled only from #8's passive captures of
//! other clients sending them (runbook scenarios S7, S7b, and S10p). Until
//! those are committed, every command request is unmodelled. By owner
//! decision the fake is the evidence source for farm3d's ElegooLink
//! commands (a Centauri Carbon never receives writes from automated tests),
//! so it must not grow behavior the captures do not show. It is never
//! evidence for #8's own monitoring gates, which need real hardware.
//!
//! Captures are read from [`CAPTURE_DIR`] (where #8 commits them), or from
//! `FARM3D_SIM_ELEGOOLINK_CAPTURES` if set. With none, [`FakeSdcp::start`]
//! returns [`Pending`] and tests say so instead of running.

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use futures_util::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::net::{TcpListener, UdpSocket};
use tokio_tungstenite::tungstenite::Message;

/// Where #8 commits its redacted captures, relative to the repository root.
pub const CAPTURE_DIR: &str = "docs/superpowers/baselines/evidence/a0-3-elegoolink";

/// Why the fake cannot run yet. Unlike a missing simulator (`Skip`), this is
/// not an environment problem, so `FARM3D_SIM_REQUIRED=1` does not turn it
/// into a failure.
#[derive(Debug)]
pub struct Pending(pub String);

pub fn capture_dir() -> PathBuf {
    match std::env::var("FARM3D_SIM_ELEGOOLINK_CAPTURES") {
        Ok(dir) if !dir.trim().is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("..")
            .join(CAPTURE_DIR),
    }
}

/// The capture files (`*.jsonl`), sorted by name.
pub fn capture_files(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "jsonl"))
        .collect();
    files.sort();
    files
}

/// Everything the fake knows, derived from the captures.
#[derive(Debug, Default)]
pub struct Model {
    /// Recorded replies, keyed by the request's `Cmd` and canonical `Data`.
    replies: HashMap<(i64, String), Reply>,
    /// Recorded frames the printer sent in answer to a text `ping`. Empty
    /// when the captures show pings going unanswered.
    pub ping_replies: Vec<String>,
    /// Whether any capture sent a `ping` at all (so "no pong" is observed,
    /// not assumed).
    pub ping_observed: bool,
    /// The shortest client silence after which a capture shows the printer
    /// dropping the socket without a close frame.
    pub idle_drop_after: Option<Duration>,
    /// Recorded UDP discovery exchanges: request text to reply text.
    discovery: HashMap<String, String>,
    /// The capture files the model was built from.
    pub sources: Vec<String>,
}

#[derive(Debug, Default)]
struct Reply {
    ack: Value,
    /// Each recorded run of follow-up frames; replayed in rotation.
    follow_ups: Vec<Vec<String>>,
}

impl Model {
    pub fn load(dir: &Path) -> Result<Self, Pending> {
        let files = capture_files(dir);
        if files.is_empty() {
            return Err(Pending(format!(
                "the ElegooLink fake is pending #8's captures: it may only be \
                 built from recorded real-hardware captures, and {} has none \
                 (set FARM3D_SIM_ELEGOOLINK_CAPTURES to use a local copy)",
                dir.display()
            )));
        }
        let mut model = Model::default();
        for file in &files {
            let text = std::fs::read_to_string(file)
                .unwrap_or_else(|error| panic!("read {}: {error}", file.display()));
            let records: Vec<Value> = text
                .lines()
                .filter(|line| !line.trim().is_empty())
                .map(|line| {
                    serde_json::from_str(line).unwrap_or_else(|error| {
                        panic!("{}: bad JSONL line: {error}", file.display())
                    })
                })
                .collect();
            model.learn(&records);
            model
                .sources
                .push(file.file_name().unwrap().to_string_lossy().into_owned());
        }
        Ok(model)
    }

    fn learn(&mut self, records: &[Value]) {
        // Requests seen so far on this capture's socket, by RequestID.
        let mut pending: HashMap<String, (i64, String)> = HashMap::new();
        // The request key whose ack came last; later non-ack frames follow it.
        let mut following: Option<(i64, String)> = None;
        let mut run: Vec<String> = Vec::new();
        let mut last_client_ms: Option<i64> = None;
        let mut awaiting_ping_reply = false;

        for record in records {
            let channel = record["channel"].as_str().unwrap_or_default();
            let direction = record["direction"].as_str().unwrap_or_default();
            let kind = record["kind"].as_str().unwrap_or_default();
            let at = record["elapsedMs"].as_i64().unwrap_or_default();
            let text = record["text"].as_str().unwrap_or_default();
            match (channel, direction, kind) {
                ("udp", "out", "discoveryRequest") => {
                    self.discovery.entry(text.to_string()).or_default();
                }
                ("udp", "in", "discoveryReply") => {
                    if let Some(request) = self
                        .discovery
                        .iter_mut()
                        .find(|(_, reply)| reply.is_empty())
                    {
                        *request.1 = text.to_string();
                    }
                }
                ("ws", "out", "ping") => {
                    self.ping_observed = true;
                    awaiting_ping_reply = true;
                    last_client_ms = Some(at);
                }
                ("ws", "out", "request") => {
                    awaiting_ping_reply = false;
                    last_client_ms = Some(at);
                    let Ok(frame) = serde_json::from_str::<Value>(text) else {
                        continue;
                    };
                    if let (Some(cmd), Some(id)) = (
                        frame["Data"]["Cmd"].as_i64(),
                        frame["Data"]["RequestID"].as_str(),
                    ) {
                        pending.insert(id.to_string(), (cmd, canonical(&frame["Data"]["Data"])));
                    }
                }
                ("ws", "in", "text") => {
                    if awaiting_ping_reply {
                        self.ping_replies.push(text.to_string());
                    }
                    let frame: Value = serde_json::from_str(text).unwrap_or(Value::Null);
                    let answered = frame["Data"]["RequestID"]
                        .as_str()
                        .and_then(|id| pending.remove(id));
                    if let Some(key) = answered {
                        flush(self, &following, &mut run);
                        self.replies.entry(key.clone()).or_insert_with(|| Reply {
                            ack: frame.clone(),
                            follow_ups: Vec::new(),
                        });
                        following = Some(key);
                    } else if following.is_some() {
                        run.push(text.to_string());
                    }
                }
                ("ws", "event", "close") => {
                    flush(self, &following, &mut run);
                    following = None;
                    if record["code"].as_i64() == Some(1006) {
                        if let Some(last) = last_client_ms {
                            let silence = Duration::from_millis((at - last).max(0) as u64);
                            self.idle_drop_after = Some(match self.idle_drop_after {
                                Some(shortest) => shortest.min(silence),
                                None => silence,
                            });
                        }
                    }
                }
                _ => {}
            }
        }
        flush(self, &following, &mut run);
    }

    /// The `Cmd` values the captures can answer.
    pub fn modelled_commands(&self) -> Vec<i64> {
        let mut commands: Vec<i64> = self.replies.keys().map(|(cmd, _)| *cmd).collect();
        commands.sort_unstable();
        commands.dedup();
        commands
    }
}

/// Files the frames collected since the last ack under that ack's request.
fn flush(model: &mut Model, key: &Option<(i64, String)>, run: &mut Vec<String>) {
    if let Some(key) = key {
        if let Some(reply) = model.replies.get_mut(key) {
            reply.follow_ups.push(std::mem::take(run));
        }
    }
    run.clear();
}

/// `Data` as a stable string, so `{"a":1,"b":2}` and `{"b":2,"a":1}` match.
fn canonical(value: &Value) -> String {
    match value {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            let inner: Vec<String> = keys
                .into_iter()
                .map(|key| format!("{}:{}", Value::String(key.clone()), canonical(&map[key])))
                .collect();
            format!("{{{}}}", inner.join(","))
        }
        Value::Array(items) => format!(
            "[{}]",
            items.iter().map(canonical).collect::<Vec<_>>().join(",")
        ),
        other => other.to_string(),
    }
}

/// A running fake: a WebSocket server and a UDP discovery responder, both on
/// 127.0.0.1 with OS-assigned ports.
pub struct FakeSdcp {
    pub ws_addr: SocketAddr,
    pub discovery_addr: SocketAddr,
    pub model: Arc<Model>,
    unmodelled: Arc<Mutex<Vec<String>>>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}

impl FakeSdcp {
    /// Starts the fake from the captures, with the idle drop the captures
    /// show. Must be called inside a Tokio runtime.
    pub async fn start() -> Result<Self, Pending> {
        let model = Model::load(&capture_dir())?;
        let idle = model.idle_drop_after;
        Ok(Self::start_with(model, idle).await)
    }

    /// As [`FakeSdcp::start`], but drops a silent client after `idle`
    /// instead (`None`: never). Only the duration changes; the behavior is
    /// still the recorded one.
    pub async fn start_with_idle(idle: Option<Duration>) -> Result<Self, Pending> {
        let model = Model::load(&capture_dir())?;
        Ok(Self::start_with(model, idle).await)
    }

    async fn start_with(model: Model, idle: Option<Duration>) -> Self {
        let model = Arc::new(model);
        let unmodelled = Arc::new(Mutex::new(Vec::new()));
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind the fake SDCP socket");
        let ws_addr = listener.local_addr().unwrap();
        let udp = UdpSocket::bind("127.0.0.1:0")
            .await
            .expect("bind the fake discovery socket");
        let discovery_addr = udp.local_addr().unwrap();

        let ws_task = {
            let model = Arc::clone(&model);
            let unmodelled = Arc::clone(&unmodelled);
            tokio::spawn(async move {
                let rotation = Arc::new(Mutex::new(HashMap::<(i64, String), usize>::new()));
                while let Ok((stream, _)) = listener.accept().await {
                    let model = Arc::clone(&model);
                    let unmodelled = Arc::clone(&unmodelled);
                    let rotation = Arc::clone(&rotation);
                    tokio::spawn(async move {
                        let Ok(socket) = tokio_tungstenite::accept_async(stream).await else {
                            return;
                        };
                        serve(socket, &model, idle, &unmodelled, &rotation).await;
                    });
                }
            })
        };
        let udp_task = {
            let model = Arc::clone(&model);
            tokio::spawn(async move {
                let mut buffer = [0u8; 2048];
                while let Ok((length, from)) = udp.recv_from(&mut buffer).await {
                    let request = String::from_utf8_lossy(&buffer[..length]).to_string();
                    if let Some(reply) = model
                        .discovery
                        .get(&request)
                        .filter(|reply| !reply.is_empty())
                    {
                        let _ = udp.send_to(reply.as_bytes(), from).await;
                    }
                }
            })
        };
        Self {
            ws_addr,
            discovery_addr,
            model,
            unmodelled,
            tasks: vec![ws_task, udp_task],
        }
    }

    pub fn ws_url(&self) -> String {
        format!("ws://{}/websocket", self.ws_addr)
    }

    /// Requests the captures could not answer, as received.
    pub fn unmodelled(&self) -> Vec<String> {
        self.unmodelled.lock().unwrap().clone()
    }
}

impl Drop for FakeSdcp {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

async fn serve(
    mut socket: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    model: &Model,
    idle: Option<Duration>,
    unmodelled: &Mutex<Vec<String>>,
    rotation: &Mutex<HashMap<(i64, String), usize>>,
) {
    loop {
        let next = match idle {
            Some(idle) => match tokio::time::timeout(idle, socket.next()).await {
                Ok(next) => next,
                // The recorded silent drop: close the TCP stream without a
                // WebSocket close frame (a client sees code 1006).
                Err(_) => return,
            },
            None => socket.next().await,
        };
        let text = match next {
            Some(Ok(Message::Text(text))) => text.to_string(),
            Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return,
            Some(Ok(_)) => continue,
        };
        if text == "ping" {
            for reply in &model.ping_replies {
                let _ = socket.send(Message::Text(reply.clone().into())).await;
            }
            continue;
        }
        let Ok(request) = serde_json::from_str::<Value>(&text) else {
            unmodelled.lock().unwrap().push(text);
            continue;
        };
        let key = (
            request["Data"]["Cmd"].as_i64().unwrap_or(-1),
            canonical(&request["Data"]["Data"]),
        );
        let Some(reply) = model.replies.get(&key) else {
            unmodelled.lock().unwrap().push(text);
            continue;
        };
        let mut ack = reply.ack.clone();
        ack["Data"]["RequestID"] = request["Data"]["RequestID"].clone();
        if socket
            .send(Message::Text(ack.to_string().into()))
            .await
            .is_err()
        {
            return;
        }
        if reply.follow_ups.is_empty() {
            continue;
        }
        let index = {
            let mut rotation = rotation.lock().unwrap();
            let next = rotation.entry(key.clone()).or_insert(0);
            let index = *next % reply.follow_ups.len();
            *next += 1;
            index
        };
        for frame in &reply.follow_ups[index] {
            if socket
                .send(Message::Text(frame.clone().into()))
                .await
                .is_err()
            {
                return;
            }
        }
    }
}
