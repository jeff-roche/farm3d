//! Connection configuration, live status, and the adapter trait every
//! protocol implements.
//!
//! Two adapters ship: Moonraker (push, over WebSocket) and OctoPrint
//! (status-only, polled over REST). ElegooLink is still gated on its protocol
//! spike. The trait is deliberately two methods wide and `ConnectionState` is
//! farm3d's own small vocabulary rather than any one protocol's, so a push
//! adapter and a polling adapter fit the same shape.

pub mod commands;
pub mod credentials;
pub mod discovery;
pub mod moonraker;
pub mod octoprint;
pub mod status_repository;
pub mod supervisor;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::connections::status_repository::PrinterTelemetry;
use crate::printers::operational::{
    HostActivity, OperationalState, PrinterReadiness, ReadinessReason, ReadinessState,
    TelemetryFreshness,
};

pub const MOONRAKER_KIND: &str = "moonraker";
pub const DEFAULT_MOONRAKER_PORT: u16 = 7125;
pub const OCTOPRINT_KIND: &str = "octoprint";
/// OctoPrint's default when served directly (not behind a reverse proxy).
/// Matches the frontend's `DEFAULT_PORTS` entry.
pub const DEFAULT_OCTOPRINT_PORT: u16 = 80;

/// Every Connection kind this build can construct, probe, and supervise.
/// The one place the setup, batch, and connection-edit paths ask "can this
/// build speak `kind`?" — `supervisor::build` is the matching constructor,
/// and a test there keeps the two in step.
pub const SUPPORTED_KINDS: &[&str] = &[MOONRAKER_KIND, OCTOPRINT_KIND];

pub fn is_supported_kind(kind: &str) -> bool {
    SUPPORTED_KINDS.contains(&kind)
}

/// No adapter speaks TLS yet: the WebSocket client is built without a TLS
/// backend (A0.1, #9, decision B4). Every entry point that accepts a
/// Connection rejects `useTls` with this message, so a user sees why rather
/// than a Connection that can only ever be "unreachable".
pub const TLS_UNSUPPORTED_MESSAGE: &str = "TLS connections are not supported yet.";

/// Rejects a TLS Connection as a validation error at `useTls`.
pub fn reject_tls(use_tls: bool) -> Result<(), crate::contracts::command::CommandError> {
    if use_tls {
        Err(crate::contracts::command::CommandError::validation_at(
            "useTls",
            TLS_UNSUPPORTED_MESSAGE,
        ))
    } else {
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ConnectionConfig.ts")]
pub struct ConnectionConfig {
    /// `"moonraker"` or `"octoprint"` today; `"elegoolink"` is pending its
    /// protocol spike.
    /// A free string rather than an enum so an unknown kind written by a
    /// newer farm3d round-trips through an older one instead of failing the
    /// whole `printers.json` load.
    pub kind: String,
    pub host: String,
    pub port: u16,
    #[serde(default)]
    pub use_tls: bool,
    /// A key into the credential store — never the credential value. See the
    /// module docs on `credentials`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub credential_ref: Option<String>,
}

/// farm3d's connection vocabulary, deliberately orthogonal to the print-job
/// state the card already shows. A live socket to a shut-down Klipper is
/// `Offline`, not `Online`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ConnectionState.ts")]
pub enum ConnectionState {
    Connecting,
    Online,
    Offline,
    Error,
}

/// A recoverable current condition in the telemetry cache, not a Printer
/// connection failure.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/StatusCacheWarningOperation.ts"
)]
pub enum StatusCacheWarningOperation {
    Hydrate,
    Save,
    Delete,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StatusCacheWarning.ts")]
pub struct StatusCacheWarning {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub printer_id: Option<String>,
    pub operation: StatusCacheWarningOperation,
}

/// Canonical runtime status for a durable Printer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterStatus.ts")]
pub struct PrinterStatus {
    pub connection_state: ConnectionState,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    pub telemetry: PrinterTelemetry,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_observed_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub fresh_until: Option<String>,
    pub operational_state: OperationalState,
    pub readiness: PrinterReadiness,
    pub freshness: TelemetryFreshness,
    pub cache_warnings: Vec<StatusCacheWarning>,
    pub updated_at: String,
}

impl PrinterStatus {
    pub fn new(connection_state: ConnectionState) -> Self {
        Self {
            connection_state,
            error: None,
            telemetry: PrinterTelemetry {
                host_activity: HostActivity::Unknown,
                host_activity_name: None,
                job_name: None,
                progress: None,
                nozzle_temp_c: None,
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            },
            last_observed_at: None,
            fresh_until: None,
            operational_state: OperationalState::Unknown,
            readiness: PrinterReadiness {
                state: ReadinessState::NotReady,
                reason: Some(ReadinessReason::UnknownState),
            },
            freshness: TelemetryFreshness::Unavailable,
            cache_warnings: Vec::new(),
            updated_at: crate::printers::now_rfc3339(),
        }
    }

    pub fn errored(message: impl Into<String>) -> Self {
        let mut status = Self::new(ConnectionState::Error);
        status.error = Some(message.into());
        status
    }
}

/// Normalized data sent from an adapter to the protocol-neutral supervisor.
#[derive(Clone, PartialEq, Debug)]
pub enum ConnectionObservation {
    Telemetry(PrinterTelemetry),
    Health {
        state: ConnectionState,
        observed_at: String,
    },
}

/// What "Test connection" renders. Its job is to let a user confirm they
/// reached the machine they meant to reach.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ProbeResult.ts")]
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
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ReportedCapabilities.ts")]
pub struct ReportedCapabilities {
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bed_width_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bed_depth_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
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
        tx: tokio::sync::mpsc::Sender<ConnectionObservation>,
    ) -> Result<(), ConnectionError>;
}

/// Sends a liveness observation. Returns true when supervision has dropped
/// the receiver, which is an adapter's cue to end `subscribe` cleanly.
pub(crate) async fn send_health(
    tx: &tokio::sync::mpsc::Sender<ConnectionObservation>,
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

    #[test]
    fn octoprint_kind_and_default_port_are_defined() {
        assert_eq!(OCTOPRINT_KIND, "octoprint");
        assert_eq!(DEFAULT_OCTOPRINT_PORT, 80);
    }

    #[test]
    fn supported_kinds_are_moonraker_and_octoprint_only() {
        assert!(is_supported_kind("moonraker"));
        assert!(is_supported_kind("octoprint"));
        assert!(!is_supported_kind("elegoolink"));
        assert!(!is_supported_kind("prusalink"));
        assert!(!is_supported_kind(""));
    }

    #[test]
    fn connection_config_round_trips_for_octoprint() {
        let config = ConnectionConfig {
            kind: OCTOPRINT_KIND.to_string(),
            host: "octoprint.local".to_string(),
            port: DEFAULT_OCTOPRINT_PORT,
            use_tls: false,
            credential_ref: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        assert_eq!(
            serde_json::from_str::<ConnectionConfig>(&json).unwrap(),
            config
        );
    }

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
        assert_eq!(
            serde_json::from_str::<ConnectionConfig>(&json).unwrap(),
            config
        );
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
