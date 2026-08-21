//! Connection configuration, live status, and the adapter trait every
//! protocol implements.
//!
//! Phase 2 ships exactly one adapter (Moonraker). Phase 3 adds OctoPrint and
//! ElegooLink against this same trait — and is expected to reshape it, which
//! is why the trait is deliberately two methods wide and why `ConnectionState`
//! is farm3d's own small vocabulary rather than any one protocol's.

pub mod credentials;

use serde::{Deserialize, Serialize};

pub const MOONRAKER_KIND: &str = "moonraker";
pub const DEFAULT_MOONRAKER_PORT: u16 = 7125;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ConnectionConfig {
    /// `"moonraker"` today; phase 3 adds `"octoprint"` and `"elegoolink"`.
    /// A free string rather than an enum so an unknown kind written by a
    /// newer farm3d round-trips through an older one instead of failing the
    /// whole `printers.json` load.
    pub kind: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub use_tls: bool,
    /// A key INTO the credential store — never the secret itself. See the
    /// module docs on `credentials`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

/// farm3d's connection vocabulary, deliberately orthogonal to the print-job
/// state the card already shows. A live socket to a shut-down Klipper is
/// `Offline`, not `Online`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum ConnectionState {
    Connecting,
    Online,
    Offline,
    Error,
}

/// Everything the dashboard renders live.
///
/// Every reading is `Option` for two independent reasons: Moonraker sends
/// PARTIAL updates (see `moonraker::protocol`), and a printer can be online
/// with no job loaded and no heaters configured. A `None` means "not
/// reported", which must render as an em dash — never as `0`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrinterStatus {
    pub connection_state: ConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_state: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub job_name: Option<String>,
    /// `0.0..=1.0`, from `display_status.progress`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nozzle_target_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_temp_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_target_c: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub print_duration_s: Option<f64>,
    /// farm3d's own clock, deliberately NOT Moonraker's `eventtime` — that
    /// value is a Klipper-uptime float, meaningless to a user and
    /// incomparable across printers.
    pub updated_at: String,
}

impl PrinterStatus {
    pub fn new(connection_state: ConnectionState) -> Self {
        Self {
            connection_state,
            error: None,
            job_state: None,
            job_name: None,
            progress: None,
            nozzle_temp_c: None,
            nozzle_target_c: None,
            bed_temp_c: None,
            bed_target_c: None,
            print_duration_s: None,
            updated_at: crate::printers::now_rfc3339(),
        }
    }

    pub fn errored(message: impl Into<String>) -> Self {
        let mut status = Self::new(ConnectionState::Error);
        status.error = Some(message.into());
        status
    }
}

/// What "Test connection" renders. Its job is to let a user confirm they
/// reached the machine they meant to reach.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProbeResult {
    pub kind: String,
    /// The host software, e.g. Moonraker's own version.
    pub host_software: String,
    /// The firmware behind it, e.g. Klipper's `software_version`.
    pub firmware: String,
    pub reported_name: String,
    pub state: String,
    pub state_message: String,
    pub reported: ReportedCapabilities,
}

/// The build volume the HOST reports, for cross-checking against the catalog
/// Profile. All `Option` — an unhomed or shut-down Klipper reports no axis
/// limits at all, which is a normal state and not an error.
///
/// The comparison itself happens in the frontend, which already holds the
/// `ResolvedPrinter` carrying the catalog's numbers; doing it here would mean
/// a second catalog lookup for no gain.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct ReportedCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_width_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_depth_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printable_height_mm: Option<f64>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum ConnectionError {
    /// Could not open a socket at all — wrong host, wrong port, host down.
    Unreachable(String),
    /// Reached the host; it rejected our credentials.
    Auth(String),
    /// Reached the host; it spoke something we could not parse.
    Protocol(String),
    Timeout,
}

impl std::fmt::Display for ConnectionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ConnectionError::Unreachable(m) => write!(f, "Could not reach the printer: {m}"),
            ConnectionError::Auth(m) => write!(f, "The printer rejected the credentials: {m}"),
            ConnectionError::Protocol(m) => write!(f, "Unexpected response from the printer: {m}"),
            ConnectionError::Timeout => write!(f, "The printer did not respond in time"),
        }
    }
}

impl std::error::Error for ConnectionError {}

/// One protocol adapter.
///
/// `subscribe` owns its transport and pushes into a channel rather than
/// exposing a `poll()` the supervisor ticks: Moonraker and ElegooLink push
/// over WebSocket while OctoPrint will drive its own interval internally in
/// phase 3, and neither shape should leak into the supervisor.
///
/// An adapter never implements retry. The supervisor owns
/// reconnect-with-backoff, so `subscribe` returning `Ok(())` means "the
/// stream ended cleanly" and returning `Err` means "it failed"; both are the
/// supervisor's cue to reconnect.
#[async_trait::async_trait]
pub trait PrinterConnection: Send + Sync {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError>;

    async fn subscribe(
        &self,
        tx: tokio::sync::mpsc::Sender<PrinterStatus>,
    ) -> Result<(), ConnectionError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn connection_config_uses_camel_case_and_omits_absent_credential() {
        let config = ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: DEFAULT_MOONRAKER_PORT,
            use_tls: false,
            credential_ref: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            json,
            r#"{"kind":"moonraker","host":"voron.local","port":7125,"useTls":false}"#
        );
    }

    #[test]
    fn connection_config_round_trips_with_a_credential_ref() {
        let config = ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "10.0.0.5".to_string(),
            port: 7125,
            use_tls: true,
            credential_ref: Some("farm3d/printer/prn-1/apikey".to_string()),
        };
        let json = serde_json::to_string(&config).unwrap();
        assert!(json.contains(r#""credentialRef":"farm3d/printer/prn-1/apikey""#));
        assert_eq!(serde_json::from_str::<ConnectionConfig>(&json).unwrap(), config);
    }

    #[test]
    fn connection_state_serializes_camel_case() {
        assert_eq!(
            serde_json::to_string(&ConnectionState::Connecting).unwrap(),
            r#""connecting""#
        );
    }

    #[test]
    fn printer_status_omits_every_absent_field() {
        let status = PrinterStatus::new(ConnectionState::Offline);
        let json = serde_json::to_string(&status).unwrap();
        assert!(json.contains(r#""connectionState":"offline""#));
        // A status with no readings must not claim zero temperatures.
        assert!(!json.contains("nozzleTempC"));
        assert!(!json.contains("progress"));
    }
}
