use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::command::JsonValue;
use super::ContractVersion;

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/NoArgsRequest.ts")]
pub struct NoArgsRequest {
    #[ts(type = "1")]
    pub contract_version: ContractVersion,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/PrinterRevisionPrecondition.ts"
)]
pub struct PrinterRevisionPrecondition {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ConnectionConfig.ts")]
pub struct ConnectionConfig {
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub use_tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credential_ref: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ConnectionSubmission.ts")]
pub struct ConnectionSubmission {
    pub kind: String,
    pub host: String,
    pub port: u16,
    pub use_tls: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub credential: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterStatus.ts")]
pub struct PrinterStatus {
    pub connection_state: String,
    pub updated_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub telemetry: Option<JsonValue>,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterStatusRow.ts")]
pub struct PrinterStatusRow {
    pub printer_id: String,
    pub status: PrinterStatus,
}

#[derive(Clone, Debug, Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/PrinterStatusBackfill.ts"
)]
pub struct PrinterStatusBackfill {
    pub stream_id: String,
    #[ts(type = "number")]
    pub snapshot_sequence: u64,
    pub statuses: Vec<PrinterStatusRow>,
}
