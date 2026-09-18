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
    ConnectionConfig, ConnectionError, ConnectionState, PrinterConnection, PrinterStatus,
    ProbeResult,
};
use futures_util::{SinkExt, StreamExt};
use protocol::{
    parse_frame, probe_result_from, rpc_request, subscribe_params, Frame, StatusSnapshot,
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

const ID_SERVER_INFO: u64 = 1;
const ID_PRINTER_INFO: u64 = 2;
const ID_SUBSCRIBE: u64 = 3;

pub struct MoonrakerConnection {
    config: ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
}

impl MoonrakerConnection {
    pub fn new(config: ConnectionConfig, api_key: Option<String>) -> Self {
        Self {
            config,
            api_key: api_key.map(zeroize::Zeroizing::new),
        }
    }

    pub fn with_zeroizing_secret(
        config: ConnectionConfig,
        api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Self {
        Self { config, api_key }
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

async fn send(socket: &mut Socket, frame: String) -> Result<(), ConnectionError> {
    socket
        .send(Message::Text(frame.into()))
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
                        if code == Some(401) || code == Some(403) {
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

    async fn subscribe(&self, tx: Sender<PrinterStatus>) -> Result<(), ConnectionError> {
        let mut socket = connect(
            &self.config,
            self.api_key.as_ref().map(|value| value.as_str()),
        )
        .await?;
        send(
            &mut socket,
            rpc_request(
                ID_SUBSCRIBE,
                "printer.objects.subscribe",
                Some(subscribe_params()),
            ),
        )
        .await?;

        let mut snapshot = StatusSnapshot::default();
        // Klipper can be down behind a perfectly healthy Moonraker socket, so
        // connection state is tracked from the lifecycle notifications rather
        // than assumed from the socket being open.
        let mut state = ConnectionState::Online;

        while let Some(frame) = next_frame(&mut socket).await {
            match frame {
                Frame::Response { id, result } if id == ID_SUBSCRIBE => {
                    snapshot.merge(&result["status"]);
                }
                Frame::StatusUpdate(update) => snapshot.merge(&update),
                Frame::KlippyReady => state = ConnectionState::Online,
                Frame::KlippyDown => state = ConnectionState::Offline,
                Frame::Error { message, code, .. } if code == Some(401) || code == Some(403) => {
                    return Err(ConnectionError::Auth(message));
                }
                _ => continue,
            }
            // A closed receiver means the supervisor dropped this printer.
            // Ending cleanly beats logging into the void.
            if tx.send(snapshot.to_status(state)).await.is_err() {
                return Ok(());
            }
        }
        Ok(())
    }
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
}
