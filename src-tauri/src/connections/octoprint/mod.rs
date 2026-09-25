//! The OctoPrint adapter: status-only monitoring over plain REST with an
//! `X-Api-Key` header.
//!
//! Deliberately thin, like `moonraker::mod`. All response parsing lives in
//! `protocol`, which has no I/O. What is left here is the HTTP client, the
//! polling loop, and HTTP-status classification.
//!
//! Unlike Moonraker, OctoPrint does not push: `subscribe()` drives its own
//! interval and feeds the same observation channel a WebSocket adapter
//! would. Each tick sends a `Telemetry` observation followed by a `Health`
//! observation carrying the serial-link state, exactly the order Moonraker
//! uses, so the supervisor needs no OctoPrint-specific logic.
//!
//! Scope: monitoring only. Nothing here uploads, starts, pauses, resumes,
//! or cancels a job, or reads a camera; those are P6 work and need their
//! own command research.
//!
//! This adapter implements NO retry logic. The supervisor owns
//! reconnect-with-backoff; a single failed poll ends `subscribe()` with an
//! `Err`, which is the supervisor's cue to reconnect.
//!
//! Credential handling: the API key is held in a `Zeroizing` buffer, sent
//! only as a header marked sensitive (so `Debug` output redacts it), never in
//! a URL, and never included in an error message. Redirects are not
//! followed, because reqwest only strips its own well-known auth headers on
//! a cross-host redirect, not a custom `X-Api-Key`. The system proxy is
//! bypassed for the same reason: a LAN printer's key has no business
//! transiting a proxy.

use crate::connections::{
    send_health, ConnectionConfig, ConnectionError, ConnectionObservation, PrinterConnection,
    ProbeResult,
};
use protocol::{connection_state_from_job_state, probe_result_from, telemetry_from};
use reqwest::header::HeaderValue;
use reqwest::{Client, Response, StatusCode};
use serde_json::Value;
use std::time::Duration;
use tokio::sync::mpsc::Sender;

pub mod protocol;

const API_KEY_HEADER: &str = "X-Api-Key";
const REQUEST_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
/// Fast enough to read as live on the dashboard and far inside the
/// supervisor's 30-second telemetry-freshness window; comfortably below
/// anything that would trouble OctoPrint's Tornado server. OctoPrint
/// documents no rate limit to size against instead.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

pub struct OctoPrintConnection {
    config: ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
    poll_interval: Duration,
}

impl OctoPrintConnection {
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
            poll_interval: POLL_INTERVAL,
        }
    }

    #[cfg(test)]
    fn with_poll_interval(mut self, poll_interval: Duration) -> Self {
        self.poll_interval = poll_interval;
        self
    }

    fn request(
        &self,
        client: &Client,
        path: &str,
    ) -> Result<reqwest::RequestBuilder, ConnectionError> {
        let mut request = client.get(format!("{}{path}", base_url(&self.config)));
        if let Some(key) = api_key_header(self.api_key.as_ref().map(|key| key.as_str()))? {
            request = request.header(API_KEY_HEADER, key);
        }
        Ok(request)
    }

    async fn get_json(&self, client: &Client, path: &str) -> Result<Value, ConnectionError> {
        let response = self
            .request(client, path)?
            .send()
            .await
            .map_err(classify_transport)?;
        parse_json_response(response).await
    }

    /// `/api/printer` answers 409 when no printer is connected to OctoPrint —
    /// a normal, expected state, not a poll failure. `Ok(None)` is that case.
    async fn get_json_tolerating_409(
        &self,
        client: &Client,
        path: &str,
    ) -> Result<Option<Value>, ConnectionError> {
        let response = self
            .request(client, path)?
            .send()
            .await
            .map_err(classify_transport)?;
        if response.status() == StatusCode::CONFLICT {
            return Ok(None);
        }
        parse_json_response(response).await.map(Some)
    }

    fn client(&self) -> Result<Client, ConnectionError> {
        // This build compiles no TLS backend (Moonraker's `wss://` has the
        // same limit), so refuse clearly rather than letting reqwest fail
        // with a generic "invalid URL scheme".
        if self.config.use_tls {
            return Err(ConnectionError::Unreachable(
                "HTTPS connections are not supported by this build".into(),
            ));
        }
        Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .connect_timeout(CONNECT_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .no_proxy()
            .build()
            .map_err(|error| ConnectionError::Unreachable(error.to_string()))
    }
}

pub fn base_url(config: &ConnectionConfig) -> String {
    let scheme = if config.use_tls { "https" } else { "http" };
    format!("{scheme}://{}:{}", config.host, config.port)
}

/// An empty key means "no key" — access-control-disabled OctoPrint instances
/// need none, and sending an empty header would be rejected where sending
/// none is accepted.
fn api_key_header(api_key: Option<&str>) -> Result<Option<HeaderValue>, ConnectionError> {
    let Some(key) = api_key.filter(|key| !key.is_empty()) else {
        return Ok(None);
    };
    let mut value = HeaderValue::from_str(key)
        .map_err(|_| ConnectionError::Auth("the API key contains invalid characters".into()))?;
    value.set_sensitive(true);
    Ok(Some(value))
}

fn classify_transport(error: reqwest::Error) -> ConnectionError {
    if error.is_timeout() {
        ConnectionError::Timeout
    } else {
        // `without_url` keeps the message about the failure, not the address
        // (which the user already knows and which may carry a host they
        // consider private in a diagnostic).
        ConnectionError::Unreachable(error.without_url().to_string())
    }
}

/// Separating auth failure from unreachability is what lets the Connection
/// tab say "check the API key" instead of "check the address". OctoPrint
/// answers 403 for a missing, invalid, or insufficiently-permissioned key;
/// 401 is classified the same way because a reverse proxy in front of
/// OctoPrint may use it instead.
async fn parse_json_response(response: Response) -> Result<Value, ConnectionError> {
    let status = response.status();
    if status == StatusCode::UNAUTHORIZED || status == StatusCode::FORBIDDEN {
        return Err(ConnectionError::Auth(format!("HTTP {}", status.as_u16())));
    }
    if !status.is_success() {
        return Err(ConnectionError::Protocol(format!(
            "unexpected HTTP {}",
            status.as_u16()
        )));
    }
    let body = response.bytes().await.map_err(classify_transport)?;
    serde_json::from_slice::<Value>(&body)
        .map_err(|_| ConnectionError::Protocol("the response was not JSON".into()))
}

#[async_trait::async_trait]
impl PrinterConnection for OctoPrintConnection {
    /// Three calls that all answer whether or not a printer is attached to
    /// OctoPrint — unlike `/api/printer`, which 409s — so a probe proves the
    /// address and key are right even while the printer is switched off.
    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        let client = self.client()?;
        let version = self.get_json(&client, "/api/version").await?;
        let connection = self.get_json(&client, "/api/connection").await?;
        let profiles = self.get_json(&client, "/api/printerprofiles").await?;
        Ok(probe_result_from(&version, &connection, &profiles))
    }

    async fn subscribe(&self, tx: Sender<ConnectionObservation>) -> Result<(), ConnectionError> {
        // Built once per subscription: a `Client` pools its connections, so a
        // fresh one per tick would reopen a socket every two seconds.
        let client = self.client()?;
        let mut ticker = tokio::time::interval(self.poll_interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            let job = self.get_json(&client, "/api/job").await?;
            let printer = self
                .get_json_tolerating_409(&client, "/api/printer?exclude=sd,state")
                .await?;
            let telemetry = telemetry_from(&job, printer.as_ref());
            let state = connection_state_from_job_state(
                telemetry.host_activity_name.as_deref().unwrap_or_default(),
            );
            // A closed receiver means the supervisor dropped this printer.
            if tx
                .send(ConnectionObservation::Telemetry(telemetry))
                .await
                .is_err()
            {
                return Ok(());
            }
            if send_health(&tx, state).await {
                return Ok(());
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::{ConnectionState, DEFAULT_OCTOPRINT_PORT, OCTOPRINT_KIND};
    use crate::printers::operational::HostActivity;
    use std::collections::HashMap;
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    const KEY: &str = "0123456789ABCDEF0123456789ABCDEF";

    fn config(host: &str, port: u16, use_tls: bool) -> ConnectionConfig {
        ConnectionConfig {
            kind: OCTOPRINT_KIND.to_string(),
            host: host.to_string(),
            port,
            use_tls,
            credential_ref: None,
        }
    }

    /// One recorded request: its path and lower-cased header map.
    #[derive(Debug, Clone)]
    struct Seen {
        path: String,
        headers: HashMap<String, String>,
    }

    /// A canned HTTP/1.1 responder standing in for OctoPrint. Each route is
    /// `(path, status, body)`; an unknown path answers 404. It exists to
    /// test what `protocol` cannot: status classification, the 409
    /// tolerance, and which headers go on the wire. The JSON bodies are the
    /// same documented shapes `protocol`'s tests use.
    fn serve(routes: Vec<(&'static str, u16, String)>) -> (u16, Arc<Mutex<Vec<Seen>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("bind stub");
        let port = listener.local_addr().expect("stub addr").port();
        let seen = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&seen);
        std::thread::spawn(move || {
            for stream in listener.incoming() {
                let Ok(mut stream) = stream else { return };
                let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));
                let mut request_line = String::new();
                if reader.read_line(&mut request_line).is_err() {
                    continue;
                }
                let path = request_line
                    .split_whitespace()
                    .nth(1)
                    .unwrap_or_default()
                    .to_string();
                let mut headers = HashMap::new();
                loop {
                    let mut line = String::new();
                    if reader.read_line(&mut line).is_err() || line.trim().is_empty() {
                        break;
                    }
                    if let Some((name, value)) = line.split_once(':') {
                        headers.insert(name.trim().to_ascii_lowercase(), value.trim().to_string());
                    }
                }
                recorded.lock().unwrap().push(Seen {
                    path: path.clone(),
                    headers,
                });
                let (status, body) = routes
                    .iter()
                    .find(|(route, _, _)| *route == path)
                    .map(|(_, status, body)| (*status, body.clone()))
                    .unwrap_or((404, String::new()));
                let extra = if status == 302 {
                    "Location: http://elsewhere.invalid/api/version\r\n"
                } else {
                    ""
                };
                let _ = write!(
                    stream,
                    "HTTP/1.1 {status} X\r\nContent-Type: application/json\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
            }
        });
        (port, seen)
    }

    fn json(value: serde_json::Value) -> String {
        value.to_string()
    }

    fn probe_routes() -> Vec<(&'static str, u16, String)> {
        vec![
            (
                "/api/version",
                200,
                json(
                    serde_json::json!({"api": "0.1", "server": "1.10.3", "text": "OctoPrint 1.10.3"}),
                ),
            ),
            (
                "/api/connection",
                200,
                json(serde_json::json!({"current": {
                    "state": "Operational", "port": "VIRTUAL", "baudrate": 115200,
                    "printerProfile": "_default"
                }})),
            ),
            (
                "/api/printerprofiles",
                200,
                json(serde_json::json!({"profiles": {"_default": {
                    "id": "_default", "name": "Voron 2.4",
                    "volume": {"width": 250.0, "depth": 210.0, "height": 220.0}
                }}})),
            ),
        ]
    }

    fn job_body(state: &str, completion: serde_json::Value) -> String {
        json(serde_json::json!({
            "job": {"file": {"name": "benchy.gcode"}},
            "progress": {"completion": completion, "printTime": 60},
            "state": state
        }))
    }

    const PRINTER_PATH: &str = "/api/printer?exclude=sd,state";

    fn printer_body() -> String {
        json(serde_json::json!({"temperature": {
            "tool0": {"actual": 210.0, "target": 215.0},
            "bed": {"actual": 60.0, "target": 60.0}
        }}))
    }

    #[test]
    fn builds_a_plain_http_base_url() {
        assert_eq!(
            base_url(&config("octoprint.local", DEFAULT_OCTOPRINT_PORT, false)),
            "http://octoprint.local:80"
        );
    }

    #[test]
    fn an_api_key_becomes_a_sensitive_header() {
        let value = api_key_header(Some(KEY)).unwrap().unwrap();
        assert_eq!(value, KEY);
        // Sensitive header values print as `Sensitive` under `Debug`, so a
        // logged request cannot leak the key.
        assert!(value.is_sensitive());
        assert!(!format!("{value:?}").contains(KEY));
    }

    #[test]
    fn no_or_empty_api_key_means_no_header() {
        assert!(api_key_header(None).unwrap().is_none());
        assert!(api_key_header(Some("")).unwrap().is_none());
    }

    #[test]
    fn an_unencodable_api_key_is_a_credential_error_that_does_not_echo_it() {
        let error = api_key_header(Some("bad\nkey")).unwrap_err();
        assert!(matches!(error, ConnectionError::Auth(_)));
        assert!(!error.to_string().contains("bad"));
    }

    #[tokio::test]
    async fn probe_reads_the_three_status_free_endpoints_with_the_key_header() {
        let (port, seen) = serve(probe_routes());
        let connection =
            OctoPrintConnection::new(config("127.0.0.1", port, false), Some(KEY.to_string()));

        let probe = connection.probe().await.expect("probe");

        assert_eq!(probe.kind, "octoprint");
        assert_eq!(probe.host_software, "1.10.3");
        assert_eq!(probe.reported_name, "Voron 2.4");
        assert_eq!(probe.state, "Operational");
        assert_eq!(probe.reported.bed_width_mm, Some(250.0));
        let seen = seen.lock().unwrap().clone();
        let paths: Vec<_> = seen.iter().map(|s| s.path.as_str()).collect();
        assert_eq!(
            paths,
            ["/api/version", "/api/connection", "/api/printerprofiles"]
        );
        for request in &seen {
            assert_eq!(
                request.headers.get("x-api-key").map(String::as_str),
                Some(KEY)
            );
            // The key travels only as a header, never in the URL.
            assert!(!request.path.contains(KEY));
        }
    }

    #[tokio::test]
    async fn probe_without_a_key_sends_no_key_header() {
        let (port, seen) = serve(probe_routes());
        OctoPrintConnection::new(config("127.0.0.1", port, false), None)
            .probe()
            .await
            .expect("probe");
        assert!(seen
            .lock()
            .unwrap()
            .iter()
            .all(|request| !request.headers.contains_key("x-api-key")));
    }

    #[tokio::test]
    async fn a_403_or_401_is_a_credential_error_not_a_reachability_one() {
        for status in [403, 401] {
            let (port, _) = serve(vec![("/api/version", status, String::new())]);
            let error =
                OctoPrintConnection::new(config("127.0.0.1", port, false), Some(KEY.to_string()))
                    .probe()
                    .await
                    .unwrap_err();
            assert_eq!(error, ConnectionError::Auth(format!("HTTP {status}")));
            assert!(!error.to_string().contains(KEY));
        }
    }

    #[tokio::test]
    async fn a_non_json_answer_is_a_protocol_error_not_a_success() {
        let (port, _) = serve(vec![("/api/version", 200, "<html>nginx</html>".into())]);
        let error = OctoPrintConnection::new(config("127.0.0.1", port, false), None)
            .probe()
            .await
            .unwrap_err();
        assert!(matches!(error, ConnectionError::Protocol(_)), "{error:?}");
    }

    #[tokio::test]
    async fn a_redirect_is_not_followed_so_the_key_cannot_leave_the_host() {
        let (port, seen) = serve(vec![("/api/version", 302, String::new())]);
        let error =
            OctoPrintConnection::new(config("127.0.0.1", port, false), Some(KEY.to_string()))
                .probe()
                .await
                .unwrap_err();
        assert_eq!(
            error,
            ConnectionError::Protocol("unexpected HTTP 302".into())
        );
        assert_eq!(seen.lock().unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_closed_port_is_unreachable() {
        // Bind then drop, so the port is very likely closed.
        let port = TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
            .port();
        let error = OctoPrintConnection::new(config("127.0.0.1", port, false), None)
            .probe()
            .await
            .unwrap_err();
        assert!(
            matches!(error, ConnectionError::Unreachable(_)),
            "{error:?}"
        );
    }

    #[tokio::test]
    async fn tls_is_refused_clearly_without_touching_the_network() {
        let error = OctoPrintConnection::new(config("octoprint.invalid", 443, true), None)
            .probe()
            .await
            .unwrap_err();
        assert_eq!(
            error,
            ConnectionError::Unreachable(
                "HTTPS connections are not supported by this build".into()
            )
        );
    }

    async fn first_two(
        connection: OctoPrintConnection,
    ) -> (Vec<ConnectionObservation>, Result<(), ConnectionError>) {
        let (tx, mut rx) = tokio::sync::mpsc::channel(4);
        let task = tokio::spawn(async move { connection.subscribe(tx).await });
        let mut observations = Vec::new();
        for _ in 0..2 {
            observations.push(rx.recv().await.expect("observation"));
        }
        drop(rx);
        let outcome = tokio::time::timeout(Duration::from_secs(5), task)
            .await
            .expect("subscribe ends once the receiver is dropped")
            .expect("task");
        (observations, outcome)
    }

    #[tokio::test]
    async fn a_poll_sends_converted_telemetry_then_health() {
        let (port, seen) = serve(vec![
            (
                "/api/job",
                200,
                job_body("Printing", serde_json::json!(42.5)),
            ),
            (PRINTER_PATH, 200, printer_body()),
        ]);
        let connection =
            OctoPrintConnection::new(config("127.0.0.1", port, false), Some(KEY.to_string()))
                .with_poll_interval(Duration::from_millis(10));

        let (observations, outcome) = first_two(connection).await;

        assert_eq!(outcome, Ok(()));
        let ConnectionObservation::Telemetry(telemetry) = &observations[0] else {
            panic!("expected telemetry first, got {:?}", observations[0]);
        };
        assert_eq!(telemetry.progress, Some(0.425));
        assert_eq!(telemetry.nozzle_temp_c, Some(210.0));
        assert_eq!(telemetry.host_activity, HostActivity::Printing);
        assert!(matches!(
            observations[1],
            ConnectionObservation::Health {
                state: ConnectionState::Online,
                ..
            }
        ));
        assert!(seen.lock().unwrap().iter().all(|request| request
            .headers
            .get("x-api-key")
            .map(String::as_str)
            == Some(KEY)));
    }

    #[tokio::test]
    async fn a_409_from_api_printer_keeps_polling_with_no_temperatures() {
        let (port, _) = serve(vec![
            (
                "/api/job",
                200,
                job_body("Offline", serde_json::Value::Null),
            ),
            (PRINTER_PATH, 409, "Printer is not operational".into()),
        ]);
        let connection = OctoPrintConnection::new(config("127.0.0.1", port, false), None)
            .with_poll_interval(Duration::from_millis(10));

        let (observations, outcome) = first_two(connection).await;

        assert_eq!(outcome, Ok(()), "a 409 must not end the stream");
        let ConnectionObservation::Telemetry(telemetry) = &observations[0] else {
            panic!("expected telemetry first");
        };
        assert_eq!(telemetry.nozzle_temp_c, None);
        assert_eq!(telemetry.bed_temp_c, None);
        assert!(matches!(
            observations[1],
            ConnectionObservation::Health {
                state: ConnectionState::Offline,
                ..
            }
        ));
    }

    #[tokio::test]
    async fn a_failed_poll_ends_the_stream_with_its_classified_error() {
        for (status, expected) in [
            (403, ConnectionError::Auth("HTTP 403".into())),
            (500, ConnectionError::Protocol("unexpected HTTP 500".into())),
        ] {
            let (port, _) = serve(vec![("/api/job", status, String::new())]);
            let connection = OctoPrintConnection::new(config("127.0.0.1", port, false), None)
                .with_poll_interval(Duration::from_millis(10));
            let (tx, _rx) = tokio::sync::mpsc::channel(4);
            let outcome = tokio::time::timeout(Duration::from_secs(5), connection.subscribe(tx))
                .await
                .expect("subscribe returns");
            assert_eq!(outcome, Err(expected));
        }
    }
}
