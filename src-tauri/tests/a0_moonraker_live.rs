//! A0.1 (#9) live-validation harness for the Moonraker adapter.
//!
//! These tests are `#[ignore]`d. They talk to a REAL Moonraker instance
//! named by environment variables, so the normal suite never runs them.
//! `just moonraker-live` runs them. `docs/verification/a0-1-moonraker-live-validation.md`
//! is the checklist that says when to run each one and what to record.
//!
//! Environment:
//!
//! | Variable | Meaning |
//! | --- | --- |
//! | `FARM3D_MOONRAKER_HOST` | Required. Moonraker host name or IP address. |
//! | `FARM3D_MOONRAKER_PORT` | Optional. Defaults to 7125. |
//! | `FARM3D_MOONRAKER_TLS` | Optional. `1` connects with `wss://`. |
//! | `FARM3D_MOONRAKER_API_KEY` | Optional. Sent as `X-Api-Key`. It is never printed or written. |
//! | `FARM3D_MOONRAKER_API_KEY_FILE` | Optional. A file holding the key, which keeps it out of shell history. |
//! | `FARM3D_MOONRAKER_RESTART_CMD` | Optional. A shell command `live_lifecycle_drive` runs instead of FIRMWARE_RESTART (the simulator's MCU cannot reset). |
//! | `FARM3D_MOONRAKER_OUT` | Optional. Evidence directory. Defaults to `target/a0-moonraker/<UTC time>`. |
//! | `FARM3D_MOONRAKER_WATCH_SECS` | Optional. How long `live_watch` observes. Defaults to 180. |
//! | `FARM3D_MOONRAKER_ALLOW_CONTROL` | Must be `1` for `live_lifecycle_drive`. It sends M112 and FIRMWARE_RESTART. |
//!
//! The tests drive the production adapter (`MoonrakerConnection`) and the
//! production `ConnectionManager` with a mock Tauri runtime. Next to them, a
//! "tap" opens its own raw WebSocket to the same Moonraker and logs every
//! frame it receives. The tap re-subscribes after `notify_klippy_ready`, the
//! way Mainsail and Fluidd do. The raw log is the evidence for what
//! Moonraker actually sent. The supervisor log is the evidence for what
//! farm3d made of it.
//!
//! These tests never assert on printer-specific values. `live_lifecycle_drive`
//! checks the behavior the issue asks about and prints PASS, FAIL, or
//! INCONCLUSIVE for each check. It fails at the end if any check FAILed, after
//! it has written all of its evidence.

mod common;

use std::fs::{File, OpenOptions};
use std::io::Write as _;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::connections::moonraker::protocol::subscribe_params;
use farm3d_lib::connections::moonraker::{upgrade_request, MoonrakerConnection};
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::{ConnectionManager, PrinterSetupFacts, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionError, PrinterConnection, DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND,
};
use farm3d_lib::printers::repository::PrinterRepository;
use futures_util::{SinkExt, StreamExt};
use serde_json::{json, Value};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::Listener;
use tokio_tungstenite::tungstenite::Message;

const PRINTER_ID: &str = "a0-live-printer";

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

struct Live {
    config: ConnectionConfig,
    api_key: Option<String>,
    out: PathBuf,
}

impl Live {
    fn from_env() -> Self {
        let host = std::env::var("FARM3D_MOONRAKER_HOST").unwrap_or_default();
        assert!(
            !host.trim().is_empty(),
            "set FARM3D_MOONRAKER_HOST to the Moonraker host (see docs/verification/a0-1-moonraker-live-validation.md)"
        );
        let port = std::env::var("FARM3D_MOONRAKER_PORT")
            .ok()
            .map(|value| value.parse().expect("FARM3D_MOONRAKER_PORT is not a port"))
            .unwrap_or(DEFAULT_MOONRAKER_PORT);
        let use_tls = std::env::var("FARM3D_MOONRAKER_TLS").as_deref() == Ok("1");
        let api_key = std::env::var("FARM3D_MOONRAKER_API_KEY")
            .ok()
            .or_else(|| {
                std::env::var("FARM3D_MOONRAKER_API_KEY_FILE")
                    .ok()
                    .map(|path| {
                        std::fs::read_to_string(&path).unwrap_or_else(|error| {
                            panic!("read FARM3D_MOONRAKER_API_KEY_FILE: {error}")
                        })
                    })
            })
            .map(|key| key.trim().to_string())
            .filter(|key| !key.is_empty());
        let out = std::env::var("FARM3D_MOONRAKER_OUT")
            .map(PathBuf::from)
            .unwrap_or_else(|_| {
                PathBuf::from(env!("CARGO_MANIFEST_DIR"))
                    .join("target/a0-moonraker")
                    .join(chrono::Utc::now().format("%Y%m%dT%H%M%SZ").to_string())
            });
        std::fs::create_dir_all(&out).expect("create the evidence directory");
        println!("a0: evidence directory {}", out.display());
        Self {
            config: ConnectionConfig {
                kind: MOONRAKER_KIND.to_string(),
                host: host.trim().to_string(),
                port,
                use_tls,
                credential_ref: None,
            },
            api_key,
            out,
        }
    }

    fn key(&self) -> Option<&str> {
        self.api_key.as_deref()
    }

    fn connection(&self, api_key: Option<&str>) -> MoonrakerConnection {
        MoonrakerConnection::new(self.config.clone(), api_key.map(str::to_string))
    }
}

// ---------------------------------------------------------------------------
// Evidence log: one JSON object per line, wall time plus elapsed time
// ---------------------------------------------------------------------------

#[derive(Clone)]
struct Log {
    file: Arc<Mutex<File>>,
    started: Instant,
    echo: bool,
}

impl Log {
    fn create(path: PathBuf, echo: bool) -> Self {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .unwrap_or_else(|error| panic!("open {}: {error}", path.display()));
        Self {
            file: Arc::new(Mutex::new(file)),
            started: Instant::now(),
            echo,
        }
    }

    fn elapsed(&self) -> f64 {
        self.started.elapsed().as_secs_f64()
    }

    fn write(&self, source: &str, body: Value) {
        let line = json!({
            "at": chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            "t": (self.elapsed() * 1000.0).round() / 1000.0,
            "source": source,
            "body": body,
        });
        let mut file = self.file.lock().expect("log lock");
        let _ = writeln!(file, "{line}");
        if self.echo {
            println!(
                "[{:>8.3}s] {source}: {}",
                self.elapsed(),
                summarize(source, &line["body"])
            );
        }
    }
}

/// One line a person can read while the scenario runs. The full frame is in
/// the log file.
fn summarize(source: &str, body: &Value) -> String {
    if source == "supervisor" {
        let status = &body["payload"]["status"];
        if status.is_null() {
            return format!("#{} {}", body["sequence"], body["type"]);
        }
        let telemetry = &status["telemetry"];
        return format!(
            "#{} connection={} operational={} readiness={} freshness={} error={} activity={}/{} nozzle={}/{} bed={}/{} lastObservedAt={}",
            body["sequence"],
            status["connectionState"],
            status["operationalState"],
            status["readiness"]["state"],
            status["freshness"],
            status.get("error").unwrap_or(&Value::Null),
            telemetry["hostActivity"],
            telemetry.get("hostActivityName").unwrap_or(&Value::Null),
            telemetry.get("nozzleTempC").unwrap_or(&Value::Null),
            telemetry.get("nozzleTargetC").unwrap_or(&Value::Null),
            telemetry.get("bedTempC").unwrap_or(&Value::Null),
            telemetry.get("bedTargetC").unwrap_or(&Value::Null),
            status.get("lastObservedAt").unwrap_or(&Value::Null),
        );
    }
    let text = body.to_string();
    if text.len() > 240 {
        format!("{}…", &text[..240])
    } else {
        text
    }
}

fn write_json(live: &Live, name: &str, value: &Value) {
    let path = live.out.join(name);
    std::fs::write(&path, serde_json::to_vec_pretty(value).unwrap())
        .unwrap_or_else(|error| panic!("write {}: {error}", path.display()));
    println!("a0: wrote {}", path.display());
}

// ---------------------------------------------------------------------------
// Raw JSON-RPC helpers (a separate socket from the adapter's)
// ---------------------------------------------------------------------------

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn open(live: &Live, api_key: Option<&str>) -> Result<Socket, String> {
    let request = upgrade_request(&live.config, api_key).map_err(|e| e.to_string())?;
    match tokio::time::timeout(
        Duration::from_secs(10),
        tokio_tungstenite::connect_async(request),
    )
    .await
    {
        Err(_) => Err("timed out opening the WebSocket".to_string()),
        Ok(Err(error)) => Err(error.to_string()),
        Ok(Ok((socket, _))) => Ok(socket),
    }
}

/// Sends one request and waits for the frame with the same id. Returns the
/// whole response frame, including an `error` member if there is one.
async fn call(socket: &mut Socket, id: u64, method: &str, params: Option<Value>) -> Value {
    let mut request = json!({"jsonrpc": "2.0", "method": method, "id": id});
    if let Some(params) = params {
        request["params"] = params;
    }
    if let Err(error) = socket.send(Message::Text(request.to_string().into())).await {
        return json!({"transportError": error.to_string()});
    }
    let wait = async {
        while let Some(message) = socket.next().await {
            match message {
                Ok(Message::Text(text)) => {
                    let Ok(frame) = serde_json::from_str::<Value>(&text) else {
                        continue;
                    };
                    if frame.get("id").and_then(Value::as_u64) == Some(id) {
                        return frame;
                    }
                }
                Ok(Message::Close(_)) => return json!({"transportError": "closed"}),
                Err(error) => return json!({"transportError": error.to_string()}),
                Ok(_) => {}
            }
        }
        json!({"transportError": "closed"})
    };
    tokio::time::timeout(Duration::from_secs(30), wait)
        .await
        .unwrap_or_else(|_| json!({"transportError": "timed out waiting for the response"}))
}

async fn one_call(live: &Live, method: &str, params: Option<Value>) -> Value {
    match open(live, live.key()).await {
        Err(error) => json!({"transportError": error}),
        Ok(mut socket) => {
            let response = call(&mut socket, 1, method, params).await;
            let _ = socket.close(None).await;
            response
        }
    }
}

/// Which requested objects and attributes the query response did not carry.
/// These are the readings farm3d must show as absent, never as zero.
///
/// Moonraker answers a query for an object Klipper does not have with every
/// requested attribute set to `null` (seen on the simulator without a heated
/// bed), so `null` counts as missing, and an object whose attributes are all
/// `null` counts as absent.
fn missing_fields(query_status: &Value) -> Value {
    let mut missing = serde_json::Map::new();
    for (object, attributes) in subscribe_params()["objects"].as_object().unwrap() {
        let reported = query_status.get(object);
        let requested = attributes.as_array().unwrap();
        let absent: Vec<Value> = requested
            .iter()
            .filter(|attribute| {
                reported
                    .and_then(|r| r.get(attribute.as_str().unwrap()))
                    .is_none_or(Value::is_null)
            })
            .cloned()
            .collect();
        if reported.is_none() || absent.len() == requested.len() {
            missing.insert(object.clone(), json!("object absent"));
        } else if !absent.is_empty() {
            missing.insert(object.clone(), Value::Array(absent));
        }
    }
    Value::Object(missing)
}

fn result(frame: &Value) -> &Value {
    &frame["result"]
}

fn outcome(result: &Result<impl serde::Serialize, ConnectionError>) -> Value {
    match result {
        Ok(value) => json!({"ok": value}),
        Err(error) => json!({"error": format!("{error:?}"), "display": error.to_string()}),
    }
}

// ---------------------------------------------------------------------------
// The tap: a raw, re-subscribing observer that logs every frame
// ---------------------------------------------------------------------------

#[derive(Default)]
struct TapState {
    /// Elapsed seconds of each `notify_klippy_ready`.
    ready_at: Vec<f64>,
    /// Elapsed seconds of each `notify_status_update`.
    status_update_at: Vec<f64>,
    /// Elapsed seconds of each socket open and close.
    connected_at: Vec<f64>,
    disconnected_at: Vec<f64>,
}

fn spawn_tap(
    live: Arc<Live>,
    log: Log,
    state: Arc<Mutex<TapState>>,
    mut stop: tokio::sync::watch::Receiver<bool>,
) -> tauri::async_runtime::JoinHandle<()> {
    tauri::async_runtime::spawn(async move {
        let mut next_id = 100;
        let mut proc_stats = 0u64;
        loop {
            if *stop.borrow() {
                return;
            }
            let mut socket = match open(&live, live.key()).await {
                Ok(socket) => socket,
                Err(error) => {
                    log.write("tap", json!({"event": "connectFailed", "error": error}));
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_secs(2)) => continue,
                        _ = stop.changed() => return,
                    }
                }
            };
            state.lock().unwrap().connected_at.push(log.elapsed());
            log.write("tap", json!({"event": "connected"}));
            let subscribe = |id: u64| {
                json!({"jsonrpc": "2.0", "method": "printer.objects.subscribe", "id": id,
                       "params": subscribe_params()})
                .to_string()
            };
            let _ = socket
                .send(Message::Text(
                    json!({"jsonrpc": "2.0", "method": "server.info", "id": next_id})
                        .to_string()
                        .into(),
                ))
                .await;
            next_id += 1;
            let _ = socket.send(Message::Text(subscribe(next_id).into())).await;
            next_id += 1;
            loop {
                let message = tokio::select! {
                    message = socket.next() => message,
                    _ = stop.changed() => {
                        log.write("tap", json!({"event": "stopped", "procStatUpdatesIgnored": proc_stats}));
                        let _ = socket.close(None).await;
                        return;
                    }
                };
                let text = match message {
                    Some(Ok(Message::Text(text))) => text,
                    Some(Ok(Message::Close(frame))) => {
                        log.write(
                            "tap",
                            json!({"event": "closedByServer", "frame": format!("{frame:?}")}),
                        );
                        break;
                    }
                    Some(Err(error)) => {
                        log.write(
                            "tap",
                            json!({"event": "socketError", "error": error.to_string()}),
                        );
                        break;
                    }
                    None => {
                        log.write("tap", json!({"event": "streamEnded"}));
                        break;
                    }
                    Some(Ok(_)) => continue,
                };
                let frame: Value =
                    serde_json::from_str(&text).unwrap_or(json!({"unparsed": text.to_string()}));
                match frame.get("method").and_then(Value::as_str) {
                    // One per second and irrelevant to monitoring.
                    Some("notify_proc_stat_update") => {
                        proc_stats += 1;
                        continue;
                    }
                    Some("notify_klippy_ready") => {
                        state.lock().unwrap().ready_at.push(log.elapsed());
                        log.write("tap", frame);
                        // Moonraker drops every subscription when Klippy
                        // disconnects, so a client must subscribe again.
                        let _ = socket.send(Message::Text(subscribe(next_id).into())).await;
                        next_id += 1;
                        continue;
                    }
                    Some("notify_status_update") => {
                        state.lock().unwrap().status_update_at.push(log.elapsed());
                    }
                    _ => {}
                }
                log.write("tap", frame);
            }
            state.lock().unwrap().disconnected_at.push(log.elapsed());
            tokio::select! {
                _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                _ = stop.changed() => return,
            }
        }
    })
}

// ---------------------------------------------------------------------------
// The production supervisor, observed through its `farm3d-event-v1` emits
// ---------------------------------------------------------------------------

struct Observed {
    t: f64,
    event: Value,
}

struct Supervised {
    _app: tauri::App<tauri::test::MockRuntime>,
    manager: Arc<ConnectionManager<tauri::test::MockRuntime>>,
    events: Arc<Mutex<Vec<Observed>>>,
    _storage: (
        tempfile::TempDir,
        farm3d_lib::persistence::MetadataRootLease,
        Arc<farm3d_lib::persistence::Storage>,
    ),
}

impl Supervised {
    fn new(log: &Log) -> Self {
        let (temp, lease, storage, _database) = common::storage();
        // The telemetry cache row references a durable Printer.
        PrinterRepository::new(Arc::clone(&storage))
            .create(common::a_stored_printer(PRINTER_ID))
            .expect("create the harness Printer");
        let app = mock_builder()
            .build(mock_context(noop_assets()))
            .expect("mock app");
        let events = Arc::new(Mutex::new(Vec::new()));
        {
            let events = Arc::clone(&events);
            let log = log.clone();
            app.listen_any(STATUS_EVENT, move |event| {
                let value: Value = serde_json::from_str(event.payload()).unwrap_or(Value::Null);
                log.write("supervisor", value.clone());
                events.lock().unwrap().push(Observed {
                    t: log.elapsed(),
                    event: value,
                });
            });
        }
        let manager = Arc::new(ConnectionManager::new(
            app.handle().clone(),
            Arc::new(StatusRepository::new(Arc::clone(&storage))),
        ));
        Self {
            _app: app,
            manager,
            events,
            _storage: (temp, lease, storage),
        }
    }

    async fn start(&self, live: &Live) {
        self.manager
            .start(
                PRINTER_ID.to_string(),
                live.config.clone(),
                live.api_key.clone().map(zeroize::Zeroizing::new),
                PrinterSetupFacts::complete(),
            )
            .await;
    }

    /// Statuses published at or after `since` (elapsed seconds).
    fn statuses_since(&self, since: f64) -> Vec<(f64, Value)> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter(|observed| observed.t >= since)
            .filter_map(|observed| {
                let status = observed.event["payload"]["status"].clone();
                (!status.is_null()).then_some((observed.t, status))
            })
            .collect()
    }

    fn latest(&self) -> Option<Value> {
        self.statuses_since(0.0).pop().map(|(_, status)| status)
    }

    async fn wait_for(
        &self,
        since: f64,
        timeout: Duration,
        predicate: impl Fn(&Value) -> bool,
    ) -> Option<(f64, Value)> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(found) = self
                .statuses_since(since)
                .into_iter()
                .find(|(_, status)| predicate(status))
            {
                return Some(found);
            }
            if Instant::now() >= deadline {
                return None;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
}

fn connection_is(state: &'static str) -> impl Fn(&Value) -> bool {
    move |status| status["connectionState"] == state
}

// ---------------------------------------------------------------------------
// Checks
// ---------------------------------------------------------------------------

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Verdict {
    Pass,
    Fail,
    Inconclusive,
}

struct Checks {
    log: Log,
    rows: Vec<Value>,
}

impl Checks {
    fn record(&mut self, id: &str, title: &str, verdict: Verdict, detail: Value) {
        let row =
            json!({"id": id, "title": title, "verdict": format!("{verdict:?}"), "detail": detail});
        self.log.write("check", row.clone());
        println!("a0: CHECK {id} {verdict:?} — {title}");
        self.rows.push(row);
    }

    fn failed(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|row| row["verdict"] == "Fail")
            .map(|row| row["id"].as_str().unwrap().to_string())
            .collect()
    }
}

fn verdict(ok: bool) -> Verdict {
    if ok {
        Verdict::Pass
    } else {
        Verdict::Fail
    }
}

// ---------------------------------------------------------------------------
// Test 1: probe, versions, missing fields, authentication
// ---------------------------------------------------------------------------

/// Records the versions, the objects and fields this printer really reports,
/// and how Moonraker answers each credential case. It asserts only that the
/// configured credential probes successfully.
#[test]
#[ignore = "needs a live Moonraker; run with `just moonraker-live probe`"]
fn live_probe() {
    let live = Live::from_env();
    tauri::async_runtime::block_on(async {
        let adapter_probe = live.connection(live.key()).probe().await;
        println!(
            "a0: adapter probe (configured credential): {}",
            outcome(&adapter_probe)
        );

        // Authentication matrix. Moonraker accepts every WebSocket upgrade
        // and rejects individual requests with JSON-RPC error 401, so each
        // case goes through the adapter's own probe.
        let without_key = live.connection(None).probe().await;
        let wrong_key = live
            .connection(Some("farm3d-a0-deliberately-wrong-key"))
            .probe()
            .await;

        let mut socket = open(&live, live.key())
            .await
            .unwrap_or_else(|error| panic!("open the raw WebSocket: {error}"));
        let server_info = call(&mut socket, 1, "server.info", None).await;
        let printer_info = call(&mut socket, 2, "printer.info", None).await;
        let objects_list = call(&mut socket, 3, "printer.objects.list", None).await;
        let query = call(
            &mut socket,
            4,
            "printer.objects.query",
            Some(subscribe_params()),
        )
        .await;
        let system_info = call(&mut socket, 5, "machine.system_info", None).await;
        let _ = socket.close(None).await;

        // Only the fields the evidence template asks for. The full
        // `machine.system_info` answer also carries network addresses.
        let system = &result(&system_info)["system_info"];
        let host = json!({
            "distribution": system["distribution"],
            "cpu": system["cpu_info"]["model"],
            "cpuDescription": system["cpu_info"]["cpu_desc"],
            "python": system["python"]["version_string"],
            "virtualization": system["virtualization"],
            "error": system_info.get("error"),
        });
        let evidence = json!({
            "target": {"host": live.config.host, "port": live.config.port, "tls": live.config.use_tls,
                        "apiKeyConfigured": live.api_key.is_some()},
            "adapterProbe": outcome(&adapter_probe),
            "authentication": {
                "configuredCredential": outcome(&adapter_probe),
                "noKey": outcome(&without_key),
                "wrongKey": outcome(&wrong_key),
            },
            "versions": {
                "moonraker": result(&server_info)["moonraker_version"],
                "moonrakerApi": result(&server_info)["api_version_string"],
                "klipper": result(&printer_info)["software_version"],
                "klippyState": result(&server_info)["klippy_state"],
                "host": host,
            },
            "serverInfo": server_info,
            "printerInfo": printer_info,
            "objectsList": objects_list,
            "subscribedQuery": query,
            "missingRequestedFields": missing_fields(&result(&query)["status"]),
        });
        write_json(&live, "probe.json", &evidence);
        println!(
            "a0: versions {}",
            serde_json::to_string_pretty(&evidence["versions"]).unwrap()
        );
        println!(
            "a0: authentication {}",
            serde_json::to_string_pretty(&evidence["authentication"]).unwrap()
        );
        println!(
            "a0: requested objects/fields this printer did not report: {}",
            evidence["missingRequestedFields"]
        );
        adapter_probe.expect("the configured credential must probe successfully");
    });
}

// ---------------------------------------------------------------------------
// Test 2: passive watch while a person drives the printer
// ---------------------------------------------------------------------------

/// Runs the production supervisor and the tap side by side for
/// `FARM3D_MOONRAKER_WATCH_SECS`. It sends nothing to the printer. A person
/// performs the manual steps in the checklist (restart Moonraker, pull the
/// network, power-cycle, start a print) while it runs.
#[test]
#[ignore = "needs a live Moonraker; run with `just moonraker-live watch`"]
fn live_watch() {
    let live = Arc::new(Live::from_env());
    let secs: u64 = std::env::var("FARM3D_MOONRAKER_WATCH_SECS")
        .ok()
        .map(|value| {
            value
                .parse()
                .expect("FARM3D_MOONRAKER_WATCH_SECS is not a number")
        })
        .unwrap_or(180);
    let log = Log::create(live.out.join("watch.jsonl"), true);
    let supervised = Supervised::new(&log);
    tauri::async_runtime::block_on(async {
        let tap_state = Arc::new(Mutex::new(TapState::default()));
        let (stop, stop_rx) = tokio::sync::watch::channel(false);
        let tap = spawn_tap(
            Arc::clone(&live),
            log.clone(),
            Arc::clone(&tap_state),
            stop_rx,
        );
        supervised.start(&live).await;
        println!("a0: watching for {secs} s. Note the wall-clock time of every manual action.");
        tokio::time::sleep(Duration::from_secs(secs)).await;
        let backfill = supervised.manager.status_backfill();
        let _ = supervised.manager.stop(PRINTER_ID).await;
        let _ = stop.send(true);
        let _ = tap.await;
        write_json(
            &live,
            "watch-summary.json",
            &json!({
                "backfillAtEnd": backfill,
                "supervisorEvents": supervised.events.lock().unwrap().len(),
                "tap": {
                    "readyAt": tap_state.lock().unwrap().ready_at,
                    "connectedAt": tap_state.lock().unwrap().connected_at,
                    "disconnectedAt": tap_state.lock().unwrap().disconnected_at,
                    "statusUpdates": tap_state.lock().unwrap().status_update_at.len(),
                },
            }),
        );
    });
}

// ---------------------------------------------------------------------------
// Test 3: the scripted lifecycle scenario
// ---------------------------------------------------------------------------

/// Drives Klipper through ready → shutdown → restart → ready and checks what
/// the production supervisor publishes at each step. It sends
/// `printer.emergency_stop` (M112) and `printer.firmware_restart`, and it sets
/// the bed target to 1 °C and back to 0 as a harmless stimulus. It refuses
/// to run unless `FARM3D_MOONRAKER_ALLOW_CONTROL=1` and the printer is idle.
#[test]
#[ignore = "needs a live, idle Moonraker; run with `just moonraker-live drive`"]
fn live_lifecycle_drive() {
    let live = Arc::new(Live::from_env());
    assert_eq!(
        std::env::var("FARM3D_MOONRAKER_ALLOW_CONTROL").as_deref(),
        Ok("1"),
        "this scenario sends M112 and FIRMWARE_RESTART; set FARM3D_MOONRAKER_ALLOW_CONTROL=1 on an idle printer"
    );
    let log = Log::create(live.out.join("drive.jsonl"), true);
    let mut checks = Checks {
        log: log.clone(),
        rows: Vec::new(),
    };
    let supervised = Supervised::new(&log);

    tauri::async_runtime::block_on(async {
        // Refuse to touch a printer that is doing something.
        let query = one_call(&live, "printer.objects.query", Some(subscribe_params())).await;
        let state = result(&query)["status"]["print_stats"]["state"].clone();
        log.write(
            "driver",
            json!({"step": "preflight", "printStatsState": state, "query": query}),
        );
        assert!(
            state != "printing" && state != "paused",
            "the printer reports print_stats.state={state}; run this only on an idle printer"
        );
        let missing = missing_fields(&result(&query)["status"]);
        let heater = if missing.get("heater_bed").is_none() {
            Some(("heater_bed", "bedTargetC"))
        } else if missing.get("extruder").is_none() {
            Some(("extruder", "nozzleTargetC"))
        } else {
            None
        };

        let tap_state = Arc::new(Mutex::new(TapState::default()));
        let (stop, stop_rx) = tokio::sync::watch::channel(false);
        let tap = spawn_tap(
            Arc::clone(&live),
            log.clone(),
            Arc::clone(&tap_state),
            stop_rx,
        );
        supervised.start(&live).await;

        // D1: first live status.
        let first = supervised
            .wait_for(0.0, Duration::from_secs(20), |status| {
                status["connectionState"] == "online" && status.get("lastObservedAt").is_some()
            })
            .await;
        checks.record(
            "D1",
            "the supervisor publishes an online status with live telemetry after start",
            verdict(first.is_some()),
            json!({"status": first.as_ref().map(|(_, s)| s)}),
        );

        // D2: absent readings stay absent.
        let telemetry = supervised
            .latest()
            .map(|status| status["telemetry"].clone())
            .unwrap_or(Value::Null);
        let expectations = [
            ("extruder", "temperature", "nozzleTempC"),
            ("extruder", "target", "nozzleTargetC"),
            ("heater_bed", "temperature", "bedTempC"),
            ("heater_bed", "target", "bedTargetC"),
            ("display_status", "progress", "progress"),
            ("print_stats", "print_duration", "printDurationS"),
            ("print_stats", "filename", "jobName"),
        ];
        let mut violations = Vec::new();
        let mut absent_count = 0;
        for (object, attribute, field) in expectations {
            let absent = match missing.get(object) {
                Some(Value::String(_)) => true,
                Some(Value::Array(attributes)) => attributes.iter().any(|a| a == attribute),
                _ => false,
            };
            if absent {
                absent_count += 1;
                if telemetry.get(field).is_some() {
                    violations.push(json!({"field": field, "value": telemetry[field]}));
                }
            }
        }
        checks.record(
            "D2",
            "readings Moonraker does not report are absent from telemetry, not zero",
            if absent_count == 0 {
                Verdict::Inconclusive
            } else {
                verdict(violations.is_empty())
            },
            json!({"missing": missing, "telemetry": telemetry, "violations": violations,
                   "note": "INCONCLUSIVE means this printer reports every requested field; check the UI with a printer that lacks one"}),
        );

        // D3: a partial update merges instead of blanking other readings.
        if let Some((heater_name, target_field)) = heater {
            let before = supervised.latest().unwrap_or(Value::Null)["telemetry"].clone();
            let since = log.elapsed();
            let response = one_call(
                &live,
                "printer.gcode.script",
                Some(json!({"script": format!("SET_HEATER_TEMPERATURE HEATER={heater_name} TARGET=1")})),
            )
            .await;
            log.write(
                "driver",
                json!({"step": "stimulus target=1", "response": response}),
            );
            let seen = supervised
                .wait_for(since, Duration::from_secs(10), |status| {
                    status["telemetry"][target_field] == json!(1.0)
                })
                .await;
            let kept = seen.as_ref().is_some_and(|(_, status)| {
                ["nozzleTempC", "bedTempC", "hostActivityName"]
                    .iter()
                    .all(|field| {
                        before.get(*field).is_none() || status["telemetry"].get(*field).is_some()
                    })
            });
            checks.record(
                "D3",
                "a one-field update arrives and keeps every other reading",
                if response.get("error").is_some() || response.get("transportError").is_some() {
                    Verdict::Inconclusive
                } else {
                    verdict(seen.is_some() && kept)
                },
                json!({"before": before, "after": seen.map(|(_, s)| s["telemetry"].clone())}),
            );
            let restore = one_call(
                &live,
                "printer.gcode.script",
                Some(json!({"script": format!("SET_HEATER_TEMPERATURE HEATER={heater_name} TARGET=0")})),
            )
            .await;
            log.write(
                "driver",
                json!({"step": "stimulus target=0", "response": restore}),
            );
        } else {
            checks.record(
                "D3",
                "a one-field update arrives and keeps every other reading",
                Verdict::Inconclusive,
                json!({"note": "no extruder or heater_bed to stimulate"}),
            );
        }

        // D4: M112 → offline, and it stays offline while Klipper is shut down.
        let since = log.elapsed();
        let response = one_call(&live, "printer.emergency_stop", None).await;
        log.write(
            "driver",
            json!({"step": "emergency_stop", "response": response}),
        );
        let offline = supervised
            .wait_for(since, Duration::from_secs(15), connection_is("offline"))
            .await;
        checks.record(
            "D4",
            "Klipper shutdown (M112) turns the connection state offline",
            verdict(offline.is_some()),
            json!({"status": offline.as_ref().map(|(_, s)| s)}),
        );
        tokio::time::sleep(Duration::from_secs(15)).await;
        let flicker: Vec<Value> = offline
            .as_ref()
            .map(|(t, _)| {
                supervised
                    .statuses_since(*t)
                    .into_iter()
                    .filter(|(_, s)| s["connectionState"] != "offline")
                    .map(|(t, s)| json!({"t": t, "connectionState": s["connectionState"]}))
                    .collect()
            })
            .unwrap_or_default();
        checks.record(
            "D5",
            "while Klipper stays shut down, no status reports it online again",
            if offline.is_none() {
                Verdict::Inconclusive
            } else {
                verdict(flicker.is_empty())
            },
            json!({"nonOfflineAfterShutdown": flicker}),
        );

        // D6: supervision that STARTS while Klipper is shut down.
        let _ = supervised.manager.stop(PRINTER_ID).await;
        let since = log.elapsed();
        supervised.start(&live).await;
        tokio::time::sleep(Duration::from_secs(12)).await;
        let after_start = supervised.statuses_since(since);
        let claimed_online: Vec<Value> = after_start
            .iter()
            .filter(|(_, s)| s["connectionState"] == "online")
            .map(|(t, s)| json!({"t": t, "status": s}))
            .collect();
        checks.record(
            "D6",
            "supervision started while Klipper is shut down does not report online",
            verdict(!after_start.is_empty() && claimed_online.is_empty()),
            json!({"statuses": after_start.iter().map(|(t, s)| json!({"t": t, "connectionState": s["connectionState"], "operationalState": s["operationalState"]})).collect::<Vec<_>>()}),
        );

        // D7: FIRMWARE_RESTART → ready → online. simulavr cannot reset its
        // emulated MCU, so the simulator restarts klippy's container instead
        // (FARM3D_MOONRAKER_RESTART_CMD). Moonraker sees a Klippy disconnect
        // either way, which is what drops its subscriptions.
        let since = log.elapsed();
        match std::env::var("FARM3D_MOONRAKER_RESTART_CMD") {
            Ok(command) if !command.trim().is_empty() => {
                let shell = command.clone();
                let output = tauri::async_runtime::spawn_blocking(move || {
                    std::process::Command::new("sh")
                        .arg("-c")
                        .arg(shell)
                        .output()
                })
                .await
                .expect("restart command task");
                log.write(
                    "driver",
                    json!({"step": "restart command", "command": command,
                           "status": output.as_ref().map(|o| o.status.to_string()).map_err(|e| e.to_string())}),
                );
            }
            _ => {
                let response = one_call(&live, "printer.firmware_restart", None).await;
                log.write(
                    "driver",
                    json!({"step": "firmware_restart", "response": response}),
                );
            }
        }
        let deadline = Instant::now() + Duration::from_secs(90);
        let ready_at = loop {
            let ready = tap_state
                .lock()
                .unwrap()
                .ready_at
                .iter()
                .copied()
                .find(|t| *t >= since);
            if ready.is_some() || Instant::now() >= deadline {
                break ready;
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        };
        let online = match ready_at {
            Some(ready) => {
                supervised
                    .wait_for(ready, Duration::from_secs(15), connection_is("online"))
                    .await
            }
            None => None,
        };
        checks.record(
            "D7",
            "after FIRMWARE_RESTART and notify_klippy_ready the connection state returns to online",
            if ready_at.is_none() {
                Verdict::Inconclusive
            } else {
                verdict(online.is_some())
            },
            json!({"readyAt": ready_at, "online": online.map(|(t, s)| json!({"t": t, "status": s}))}),
        );

        // D8: telemetry resumes after the Klipper restart.
        if let (Some(ready), Some((heater_name, target_field))) = (ready_at, heater) {
            tokio::time::sleep(Duration::from_secs(3)).await;
            let since = log.elapsed();
            let response = one_call(
                &live,
                "printer.gcode.script",
                Some(json!({"script": format!("SET_HEATER_TEMPERATURE HEATER={heater_name} TARGET=1")})),
            )
            .await;
            log.write(
                "driver",
                json!({"step": "post-restart stimulus target=1", "response": response}),
            );
            let seen = supervised
                .wait_for(since, Duration::from_secs(15), |status| {
                    status["telemetry"][target_field] == json!(1.0)
                })
                .await;
            let tap_saw_updates = tap_state
                .lock()
                .unwrap()
                .status_update_at
                .iter()
                .any(|t| *t >= since);
            let last_observed = supervised
                .latest()
                .and_then(|s| s.get("lastObservedAt").cloned());
            checks.record(
                "D8",
                "after a Klipper restart, new readings reach the supervisor",
                if response.get("error").is_some() || !tap_saw_updates {
                    Verdict::Inconclusive
                } else {
                    verdict(seen.is_some())
                },
                json!({"readyAt": ready, "tapSawStatusUpdates": tap_saw_updates,
                       "supervisorSawTarget": seen.is_some(), "latestLastObservedAt": last_observed,
                       "latestFreshness": supervised.latest().map(|s| s["freshness"].clone())}),
            );
            let restore = one_call(
                &live,
                "printer.gcode.script",
                Some(json!({"script": format!("SET_HEATER_TEMPERATURE HEATER={heater_name} TARGET=0")})),
            )
            .await;
            log.write(
                "driver",
                json!({"step": "post-restart stimulus target=0", "response": restore}),
            );
        } else {
            checks.record(
                "D8",
                "after a Klipper restart, new readings reach the supervisor",
                Verdict::Inconclusive,
                json!({"readyAt": ready_at, "heater": heater.map(|h| h.0)}),
            );
        }

        // D9: the backfill agrees with the last published event.
        tokio::time::sleep(Duration::from_secs(2)).await;
        let backfill = serde_json::to_value(supervised.manager.status_backfill()).unwrap();
        let last_event = supervised
            .events
            .lock()
            .unwrap()
            .last()
            .map(|observed| observed.event.clone())
            .unwrap_or(Value::Null);
        let row = backfill["statuses"]
            .as_array()
            .and_then(|rows| rows.iter().find(|row| row["printerId"] == PRINTER_ID))
            .map(|row| row["status"].clone());
        checks.record(
            "D9",
            "the status backfill matches the last published event",
            verdict(
                backfill["snapshotSequence"] == last_event["sequence"]
                    && backfill["streamId"] == last_event["streamId"]
                    && row.as_ref() == Some(&last_event["payload"]["status"]),
            ),
            json!({"backfill": backfill, "lastEvent": last_event}),
        );

        let _ = supervised.manager.stop(PRINTER_ID).await;
        let _ = stop.send(true);
        let _ = tap.await;
    });

    write_json(
        &live,
        "drive-checks.json",
        &Value::Array(checks.rows.clone()),
    );
    let failed = checks.failed();
    assert!(
        failed.is_empty(),
        "checks failed: {failed:?}; evidence in {}",
        live.out.display()
    );
}
