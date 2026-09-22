use serde::{Deserialize, Serialize};
use ts_rs::TS;

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

/// Compatibility aliases for the canonical runtime status family.
pub type PrinterStatus = crate::connections::PrinterStatus;
pub type PrinterStatusRow = crate::connections::supervisor::PrinterStatusRow;
pub type PrinterStatusBackfill = crate::connections::supervisor::PrinterStatusBackfill;
