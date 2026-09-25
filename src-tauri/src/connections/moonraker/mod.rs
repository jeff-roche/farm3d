//! The Moonraker adapter: JSON-RPC 2.0 over `ws://<host>:<port>/websocket`.
//!
//! Deliberately thin. All framing and all status accumulation live in
//! `protocol`, which has no I/O and therefore real test coverage. What is
//! left here is connect, send, read, dispatch.
//!
//! This adapter implements NO retry logic. The supervisor owns
//! reconnect-with-backoff; `subscribe` returning at all is the supervisor's
//! cue to reconnect.

use crate::connections::{
    ConnectionConfig, ConnectionError, ConnectionObservation, ConnectionState, PrinterConnection,
    ProbeResult,
};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    is_auth_error, parse_frame, probe_result_from, rpc_request, subscribe_params, Frame,
    StatusSnapshot, Step, SubscriptionState, ID_PRINTER_INFO, ID_SERVER_INFO, ID_SUBSCRIBE,
};
use std::time::Duration;
use tokio::sync::mpsc::Sender;
use tokio_tungstenite::tungstenite::client::IntoClientRequest;
use tokio_tungstenite::tungstenite::handshake::client::Request;
use tokio_tungstenite::tungstenite::http::HeaderValue;
use tokio_tungstenite::tungstenite::Message;

pub mod protocol;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const PROBE_TIMEOUT: Duration = Duration::from_secs(10);
const LIVENESS_INTERVAL: Duration = Duration::from_secs(10);
/// How long a subscription may hear nothing before it counts as dead.
/// Moonraker pushes `notify_proc_stat_update` every second and answers the
/// Ping that each liveness tick sends, so this much silence means the network
/// path is gone. A pulled cable or a Wi-Fi drop sends no FIN or RST. Without
/// this check the socket waits forever while the liveness tick keeps
/// reporting the printer online (seen in A0.1, #9).
const SILENCE_LIMIT: Duration = Duration::from_secs(25);

pub struct MoonrakerConnection {
    config: ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
    liveness_interval: Duration,
    silence_limit: Duration,
}

impl MoonrakerConnection {
    pub fn new(config: ConnectionConfig, api_key: Option<String>) -> Self {
        Self::with_zeroizing_secret(config, api_key.map(zeroize::Zeroizing::new))
    }

    pub fn with_zeroizing_secret(
        config: ConnectionConfig,
        api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Self {
        Self {
            config,
            api_key,
            liveness_interval: LIVENESS_INTERVAL,
            silence_limit: SILENCE_LIMIT,
        }
    }

    /// Shorter timings so a socket-level test does not wait 25 s of real
    /// time. Real sockets and a paused tokio clock do not mix.
    #[cfg(test)]
    fn with_timings(mut self, liveness_interval: Duration, silence_limit: Duration) -> Self {
        self.liveness_interval = liveness_interval;
        self.silence_limit = silence_limit;
        self
    }
}

pub fn websocket_url(config: &ConnectionConfig) -> String {
    let scheme = if config.use_tls { "wss" } else { "ws" };
    format!("{scheme}://{}:{}/websocket", config.host, config.port)
}

/// Moonraker accepts the API key as a header on the WebSocket upgrade, which
/// avoids the `/access/oneshot_token` round trip entirely — and with it, any
/// need for an HTTP client in this phase.
pub fn upgrade_request(
    config: &ConnectionConfig,
    api_key: Option<&str>,
) -> Result<Request, ConnectionError> {
    let mut request = websocket_url(config)
        .into_client_request()
        .map_err(|e| ConnectionError::Unreachable(e.to_string()))?;
    if let Some(key) = api_key.filter(|k| !k.is_empty()) {
        let value = HeaderValue::from_str(key)
            .map_err(|_| ConnectionError::Auth("the API key contains invalid characters".into()))?;
        request.headers_mut().insert("X-Api-Key", value);
    }
    Ok(request)
}

type Socket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn connect(
    config: &ConnectionConfig,
    api_key: Option<&str>,
) -> Result<Socket, ConnectionError> {
    let request = upgrade_request(config, api_key)?;
    let connecting = tokio_tungstenite::connect_async(request);
    match tokio::time::timeout(CONNECT_TIMEOUT, connecting).await {
        Err(_) => Err(ConnectionError::Timeout),
        Ok(Err(e)) => Err(classify(e)),
        Ok(Ok((socket, _response))) => Ok(socket),
    }
}

/// Separating auth failure from unreachability is what lets the Connection
/// tab say "check the API key" instead of "check the address".
fn classify(error: tokio_tungstenite::tungstenite::Error) -> ConnectionError {
    use tokio_tungstenite::tungstenite::Error as WsError;
    match &error {
        WsError::Http(response)
            if response.status().as_u16() == 401 || response.status().as_u16() == 403 =>
        {
            ConnectionError::Auth(format!("HTTP {}", response.status()))
        }
        _ => ConnectionError::Unreachable(error.to_string()),
    }
}

async fn send(socket: &mut Socket, frame: impl Into<Message>) -> Result<(), ConnectionError> {
    socket
        .send(frame.into())
        .await
        .map_err(|e| ConnectionError::Unreachable(e.to_string()))
}

/// Reads until a frame we care about arrives, discarding the rest. Returns
/// `None` when the socket closes.
async fn next_frame(socket: &mut Socket) -> Option<Frame> {
    while let Some(message) = socket.next().await {
        match message {
            Ok(Message::Text(text)) => return Some(parse_frame(&text)),
            // Ping/Pong are answered by tungstenite; Binary and Close are not
            // part of Moonraker's JSON-RPC surface.
            Ok(Message::Close(_)) | Err(_) => return None,
            Ok(_) => continue,
        }
    }
    None
}

#[async_trait::async_trait]
impl PrinterConnection for MoonrakerConnection {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        let mut socket = connect(
            &self.config,
            self.api_key.as_ref().map(|value| value.as_str()),
        )
        .await?;
        send(
            &mut socket,
            rpc_request(ID_SERVER_INFO, "server.info", None),
        )
        .await?;
        send(
            &mut socket,
            rpc_request(ID_PRINTER_INFO, "printer.info", None),
        )
        .await?;
        send(
            &mut socket,
            rpc_request(
                ID_SUBSCRIBE,
                "printer.objects.query",
                Some(subscribe_params()),
            ),
        )
        .await?;

        let mut server_info = serde_json::Value::Null;
        let mut printer_info = serde_json::Value::Null;
        let mut snapshot = StatusSnapshot::default();
        let mut outstanding = 3;

        let collect = async {
            while outstanding > 0 {
                match next_frame(&mut socket).await {
                    None => {
                        return Err(ConnectionError::Protocol(
                            "the printer closed the connection before answering".into(),
                        ))
                    }
                    Some(Frame::Response { id, result }) => {
                        match id {
                            ID_SERVER_INFO => server_info = result,
                            ID_PRINTER_INFO => printer_info = result,
                            // `printer.objects.query` wraps its payload in a
                            // `status` key; the subscription notifications do
                            // not, which is why only this branch unwraps.
                            ID_SUBSCRIBE => snapshot.merge(&result["status"]),
                            _ => continue,
                        }
                        outstanding -= 1;
                    }
                    // `printer.info` fails outright when Klipper is down, but
                    // the probe is still a success: it proves we reached the
                    // right host, and `server.info.klippy_state` explains the
                    // rest. Reporting it as a connection failure would send a
                    // user hunting for a network problem they do not have.
                    Some(Frame::Error { id, message, code }) => {
                        if is_auth_error(code, &message) {
                            return Err(ConnectionError::Auth(message));
                        }
                        if id == ID_PRINTER_INFO || id == ID_SUBSCRIBE {
                            outstanding -= 1;
                            continue;
                        }
                        return Err(ConnectionError::Protocol(message));
                    }
                    Some(_) => continue,
                }
            }
            Ok(())
        };

        match tokio::time::timeout(PROBE_TIMEOUT, collect).await {
            Err(_) => return Err(ConnectionError::Timeout),
            Ok(result) => result?,
        }

        let _ = socket.close(None).await;
        Ok(probe_result_from(&server_info, &printer_info, &snapshot))
    }

    async fn subscribe(&self, tx: Sender<ConnectionObservation>) -> Result<(), ConnectionError> {
        let mut socket = connect(
            &self.config,
            self.api_key.as_ref().map(|value| value.as_str()),
        )
        .await?;
        // Klipper can be down behind a perfectly healthy Moonraker socket, so
        // the connection state comes from Moonraker, never from the socket
        // being open. `server.info` gives the state at connect time; the
        // lifecycle notifications give every change after it.
        send(
            &mut socket,
            rpc_request(ID_SERVER_INFO, "server.info", None),
        )
        .await?;
        send(&mut socket, subscribe_request()).await?;

        let mut state = SubscriptionState::default();
        let mut last_inbound = tokio::time::Instant::now();
        let mut liveness = liveness_interval(self.liveness_interval);
        loop {
            tokio::select! {
                _ = liveness.tick() => {
                    if last_inbound.elapsed() >= self.silence_limit {
                        return Err(ConnectionError::Timeout);
                    }
                    if let Some(health) = state.health() {
                        if send_health(&tx, health).await { return Ok(()); }
                    }
                    send(&mut socket, Message::Ping(Vec::new().into())).await?;
                }
                message = socket.next() => {
                    let frame = match message {
                        Some(Ok(Message::Text(text))) => parse_frame(&text),
                        // Binary and Close are not part of Moonraker's
                        // JSON-RPC surface. tungstenite answers Pings itself;
                        // a Pong still proves the path is alive.
                        Some(Ok(Message::Close(_))) | Some(Err(_)) | None => return Ok(()),
                        Some(Ok(_)) => {
                            last_inbound = tokio::time::Instant::now();
                            continue;
                        }
                    };
                    last_inbound = tokio::time::Instant::now();
                    for step in state.on_frame(frame) {
                        let closed = match step {
                            Step::Telemetry => tx
                                .send(ConnectionObservation::Telemetry(state.telemetry()))
                                .await
                                .is_err(),
                            Step::Health(health) => send_health(&tx, health).await,
                            // Moonraker drops subscriptions when Klippy
                            // disconnects; readings stop unless we ask again.
                            Step::Resubscribe => {
                                send(&mut socket, subscribe_request()).await?;
                                false
                            }
                            Step::AuthFailed(message) => {
                                return Err(ConnectionError::Auth(message));
                            }
                        };
                        if closed {
                            return Ok(());
                        }
                    }
                }
            }
        }
    }
}

fn subscribe_request() -> String {
    rpc_request(
        ID_SUBSCRIBE,
        "printer.objects.subscribe",
        Some(subscribe_params()),
    )
}

fn liveness_interval(period: Duration) -> tokio::time::Interval {
    tokio::time::interval_at(tokio::time::Instant::now() + period, period)
}

/// Returns true when supervision has dropped the receiver.
pub(crate) async fn send_health(
    tx: &Sender<ConnectionObservation>,
    state: ConnectionState,
) -> bool {
    tx.send(ConnectionObservation::Health {
        state,
        observed_at: chrono::Utc::now().to_rfc3339(),
    })
    .await
    .is_err()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND};

    fn config(use_tls: bool) -> ConnectionConfig {
        ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: DEFAULT_MOONRAKER_PORT,
            use_tls,
            credential_ref: None,
        }
    }

    #[test]
    fn builds_a_plain_websocket_url() {
        assert_eq!(
            websocket_url(&config(false)),
            "ws://voron.local:7125/websocket"
        );
    }

    #[test]
    fn builds_a_tls_websocket_url() {
        assert_eq!(
            websocket_url(&config(true)),
            "wss://voron.local:7125/websocket"
        );
    }

    #[test]
    fn an_api_key_becomes_an_upgrade_request_header() {
        let request = upgrade_request(&config(false), Some("abc123")).unwrap();
        assert_eq!(request.headers()["X-Api-Key"], "abc123");
    }

    #[test]
    fn no_api_key_means_no_header() {
        // Moonraker instances on a trusted LAN need no key at all; sending an
        // empty one would be rejected where sending none is accepted.
        let request = upgrade_request(&config(false), None).unwrap();
        assert!(request.headers().get("X-Api-Key").is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn liveness_scheduler_waits_ten_seconds_before_emitting_health() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);
        let mut liveness = liveness_interval(LIVENESS_INTERVAL);
        let sender = tokio::spawn(async move {
            liveness.tick().await;
            send_health(&tx, ConnectionState::Online).await
        });

        tokio::task::yield_now().await;
        assert!(rx.try_recv().is_err());
        tokio::time::advance(LIVENESS_INTERVAL).await;
        assert!(!sender.await.unwrap());
        assert!(matches!(
            rx.recv().await,
            Some(ConnectionObservation::Health {
                state: ConnectionState::Online,
                ..
            })
        ));
    }

    // --- Socket-level regressions for A0.1 (#9) ------------------------------

    type ServerSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

    /// A one-connection fake Moonraker on an ephemeral local port. `script`
    /// plays the server side of the conversation.
    async fn fake_moonraker<F, Fut>(script: F) -> (ConnectionConfig, tokio::task::JoinHandle<()>)
    where
        F: FnOnce(ServerSocket) -> Fut + Send + 'static,
        Fut: std::future::Future<Output = ()> + Send,
    {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        let server = tokio::spawn(async move {
            let (stream, _) = listener.accept().await.unwrap();
            script(tokio_tungstenite::accept_async(stream).await.unwrap()).await;
        });
        let config = ConnectionConfig {
            host: "127.0.0.1".to_string(),
            port,
            ..config(false)
        };
        (config, server)
    }

    /// Reads requests until one names `method`, and returns its id.
    async fn expect_request(socket: &mut ServerSocket, method: &str) -> u64 {
        while let Some(Ok(message)) = socket.next().await {
            if let Message::Text(text) = message {
                let request: serde_json::Value = serde_json::from_str(&text).unwrap();
                if request["method"] == method {
                    return request["id"].as_u64().unwrap();
                }
            }
        }
        panic!("the client never sent {method}");
    }

    async fn reply(socket: &mut ServerSocket, frame: serde_json::Value) {
        socket
            .send(Message::Text(frame.to_string().into()))
            .await
            .unwrap();
    }

    fn subscribed(id: u64, nozzle: f64) -> serde_json::Value {
        serde_json::json!({"jsonrpc": "2.0", "id": id, "result": {"eventtime": 1.0, "status": {
            "extruder": {"temperature": nozzle, "target": 0.0},
            "print_stats": {"state": "standby", "filename": ""}
        }}})
    }

    async fn next_telemetry(
        rx: &mut tokio::sync::mpsc::Receiver<ConnectionObservation>,
    ) -> crate::connections::status_repository::PrinterTelemetry {
        loop {
            match tokio::time::timeout(Duration::from_secs(5), rx.recv()).await {
                Ok(Some(ConnectionObservation::Telemetry(telemetry))) => return telemetry,
                Ok(Some(_)) => continue,
                other => panic!("no telemetry arrived: {other:?}"),
            }
        }
    }

    #[tokio::test]
    async fn the_subscription_resubscribes_when_klipper_becomes_ready_again() {
        let (config, server) = fake_moonraker(|mut socket| async move {
            let info = expect_request(&mut socket, "server.info").await;
            reply(
                &mut socket,
                serde_json::json!({"jsonrpc": "2.0", "id": info,
                "result": {"klippy_state": "ready"}}),
            )
            .await;
            let first = expect_request(&mut socket, "printer.objects.subscribe").await;
            reply(&mut socket, subscribed(first, 24.0)).await;
            // A FIRMWARE_RESTART: Moonraker forgets every subscription.
            reply(
                &mut socket,
                serde_json::json!({"jsonrpc": "2.0", "method": "notify_klippy_disconnected"}),
            )
            .await;
            reply(
                &mut socket,
                serde_json::json!({"jsonrpc": "2.0", "method": "notify_klippy_ready"}),
            )
            .await;
            let second = expect_request(&mut socket, "printer.objects.subscribe").await;
            reply(&mut socket, subscribed(second, 31.0)).await;
            std::future::pending::<()>().await;
        })
        .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let connection = MoonrakerConnection::new(config, None);
        let client = tokio::spawn(async move { connection.subscribe(tx).await });

        assert_eq!(next_telemetry(&mut rx).await.nozzle_temp_c, Some(24.0));
        assert_eq!(
            next_telemetry(&mut rx).await.nozzle_temp_c,
            Some(31.0),
            "no readings after Klipper came back"
        );
        client.abort();
        server.abort();
    }

    #[tokio::test]
    async fn a_silent_connection_ends_the_subscription_with_a_timeout() {
        let (config, server) = fake_moonraker(|mut socket| async move {
            let info = expect_request(&mut socket, "server.info").await;
            reply(
                &mut socket,
                serde_json::json!({"jsonrpc": "2.0", "id": info,
                "result": {"klippy_state": "ready"}}),
            )
            .await;
            // A pulled cable: the socket stays open, nothing more arrives,
            // and Pings go unanswered because nobody reads them.
            std::future::pending::<()>().await;
        })
        .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let connection = MoonrakerConnection::new(config, None)
            .with_timings(Duration::from_millis(50), Duration::from_millis(200));

        let outcome = tokio::time::timeout(Duration::from_secs(5), connection.subscribe(tx))
            .await
            .expect("the subscription never noticed the silence");
        assert_eq!(outcome, Err(ConnectionError::Timeout));
        drain.abort();
        server.abort();
    }

    #[tokio::test]
    async fn a_quiet_but_answering_connection_stays_up() {
        // The server says nothing on its own but reads (and so answers
        // Pings). That is a healthy idle socket, not a dead one.
        let (config, server) = fake_moonraker(|mut socket| async move {
            let info = expect_request(&mut socket, "server.info").await;
            reply(
                &mut socket,
                serde_json::json!({"jsonrpc": "2.0", "id": info,
                "result": {"klippy_state": "ready"}}),
            )
            .await;
            while let Some(Ok(_)) = socket.next().await {}
        })
        .await;
        let (tx, mut rx) = tokio::sync::mpsc::channel(32);
        let drain = tokio::spawn(async move { while rx.recv().await.is_some() {} });
        let connection = MoonrakerConnection::new(config, None)
            .with_timings(Duration::from_millis(50), Duration::from_millis(200));

        let outcome =
            tokio::time::timeout(Duration::from_millis(800), connection.subscribe(tx)).await;
        assert!(
            outcome.is_err(),
            "a Pong-answering socket was dropped: {outcome:?}"
        );
        drain.abort();
        server.abort();
    }
}
