use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::ContractVersion;

/// A top-level farm3d destination.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "navigation/NavigationDestination.ts"
)]
pub enum NavigationDestination {
    Monitor,
    Queue,
    Library,
    Spools,
    Settings,
}

/// The stable object categories supported by navigation identity.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "navigation/NavigationSelectionKind.ts"
)]
pub enum NavigationSelectionKind {
    Printer,
    Job,
    Model,
    Project,
    Spool,
    Incident,
    Attention,
}

/// A selected object's kind and opaque stable ID.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "navigation/NavigationSelection.ts"
)]
pub struct NavigationSelection {
    pub kind: NavigationSelectionKind,
    pub id: String,
}

/// A versioned destination with an optional selected object.
#[derive(Clone, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "navigation/NavigationTarget.ts")]
pub struct NavigationTarget {
    #[ts(type = "1")]
    pub version: ContractVersion,
    pub destination: NavigationDestination,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub selection: Option<NavigationSelection>,
}
