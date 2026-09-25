//! P6: the durable Host Operation record — one row per write-ahead commit
//! of an upload, start, pause, resume, or cancel command (D2), its pure
//! state machine ([`state`], D3), and its SQL ([`repository`], D2/D3/D5).
//!
//! This module holds the ts-rs domain types the wire shares with the
//! frontend (`HostOperation` and friends); the executor, reconciler, and
//! guards a later task adds build on top of [`repository`]'s functions.

pub mod repository;
pub mod state;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::connections::capabilities::{HostOperationFailureCode, InconclusiveReason};
use crate::contracts::event::JsSafeInteger;

/// The `hop-<uuid v4>` id prefix (D2).
pub const HOST_OPERATION_ID_PREFIX: &str = "hop";

pub fn new_host_operation_id() -> String {
    crate::library::new_id(HOST_OPERATION_ID_PREFIX)
}

/// D2/D3: what a Host Operation row is for.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperationKind.ts")]
pub enum HostOperationKind {
    Upload,
    Start,
    Pause,
    Resume,
    Cancel,
}

/// D3: a Host Operation's state. `succeeded`, `failed`, and `abandoned`
/// are terminal (the migration's `BEFORE UPDATE` trigger enforces it).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperationState.ts")]
pub enum HostOperationState {
    Dispatching,
    Uncertain,
    Reconciling,
    Succeeded,
    Failed,
    Abandoned,
}

/// D9: the Printer state `start_staged_artifact` was offered from, so the
/// bed-clear confirmation can name it.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PriorState.ts")]
pub enum PriorState {
    Ready,
    Finished,
    Cancelled,
}

/// A Host Operation's `endpoint_json`: the Connection farm3d dispatched
/// to, frozen at write-ahead time. Never a credential or `credentialRef`
/// (D2).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationEndpoint.ts"
)]
pub struct HostOperationEndpoint {
    pub kind: String,
    pub host: String,
    pub port: u16,
}

/// D11: a failed row's `failure_json`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperationFailure.ts")]
pub struct HostOperationFailure {
    pub code: HostOperationFailureCode,
    pub message: String,
}

impl HostOperationFailure {
    /// D11's fixed message for `code` (the table's "Message" column,
    /// reproduced verbatim). The one place that text lives, so
    /// `repository::recover_after_restart` and a later task's executor
    /// raise the exact same wording for the same code.
    pub fn for_code(code: HostOperationFailureCode) -> Self {
        use HostOperationFailureCode as Code;
        let message = match code {
            Code::NeverSent => "farm3d closed before sending this. Nothing reached the printer.",
            Code::HostUnreachable => "farm3d couldn't connect to the printer. Nothing was sent.",
            Code::AuthRejected => "The printer rejected farm3d's API key.",
            Code::ChecksumRejected => "The printer found the upload damaged and discarded it.",
            Code::FileLoaded => {
                "The printer is using a file with this name, so it refused the upload."
            }
            Code::HostBusy => "The printer is busy with another print.",
            Code::FileMissing => "The printer couldn't find the staged file.",
            Code::HostRejected => "The printer refused the request.",
            Code::HostNotReady => "Klipper isn't running on the printer.",
            Code::NotApplied => {
                "The file isn't on the printer, and it didn't appear within a minute."
            }
            Code::HostFileDiffers => {
                "A different file is at farm3d's path on the printer. Staging again replaces it."
            }
        };
        Self {
            code,
            message: message.to_string(),
        }
    }
}

/// D11 `HostOperationResolution.startObserved.source`: where the start
/// evidence came from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StartEvidenceSource.ts")]
pub enum StartEvidenceSource {
    PrintStats,
    History,
}

/// D11 `HostOperationResolution.stateObserved.observedState`: the
/// `print_stats.state` reconciliation read to prove pause/resume/cancel.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationObservedState.ts"
)]
pub enum HostOperationObservedState {
    Printing,
    Paused,
    Complete,
    Cancelled,
}

/// D11: a succeeded row's `resolution_json`, tagged on `kind`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/HostOperationResolution.ts"
)]
pub enum HostOperationResolution {
    ArtifactVerified {
        reconciled: bool,
    },
    /// A 200 `{"result":"ok"}` at dispatch (D5: definitive for `start`).
    StartAccepted,
    StartObserved {
        source: StartEvidenceSource,
        /// Moonraker's hex id string, for display only.
        history_job_id: Option<String>,
        interrupted: bool,
    },
    StateObserved {
        observed_state: HostOperationObservedState,
        reconciled: bool,
    },
}

/// `HostOperation.lastAttempt`: the most recent reconciliation attempt
/// that left the row `uncertain`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationLastAttempt.ts"
)]
pub struct HostOperationLastAttempt {
    pub at: String,
    pub reason: InconclusiveReason,
}

/// A `host_operations` row, on the wire. `history_mark` is backend-only
/// (D2/"Wire types"): present here for the reconciler and executor, never
/// serialized or exported.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperation.ts")]
pub struct HostOperation {
    pub id: String,
    pub printer_id: String,
    pub kind: HostOperationKind,
    pub state: HostOperationState,
    pub slice_revision_id: Option<String>,
    pub source_host_operation_id: Option<String>,
    pub gcode_sha256: Option<String>,
    #[ts(type = "number | null")]
    pub gcode_size: Option<i64>,
    pub host_path: String,
    /// Start only: the newest history `job_id` before dispatch. Backend-
    /// only — not on the wire.
    #[serde(skip)]
    #[ts(skip)]
    pub history_mark: Option<i64>,
    pub endpoint: HostOperationEndpoint,
    pub failure: Option<HostOperationFailure>,
    pub resolution: Option<HostOperationResolution>,
    #[ts(type = "number")]
    pub attempts: i64,
    pub last_attempt: Option<HostOperationLastAttempt>,
    pub no_longer_pending: bool,
    pub abandoned_at: Option<String>,
    pub abandon_note: Option<String>,
    pub created_at: String,
    pub dispatched_at: Option<String>,
    pub uncertain_since: Option<String>,
    pub resolved_at: Option<String>,
}

/// `list_host_operations`' result: the stream identity a later `hostOperations`
/// event carries, plus the backfill (spec "Commands").
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationsSnapshot.ts"
)]
pub struct HostOperationsSnapshot {
    pub stream_id: String,
    pub snapshot_sequence: JsSafeInteger,
    pub operations: Vec<HostOperation>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_host_operation_id_uses_the_hop_prefix() {
        assert!(new_host_operation_id().starts_with("hop-"));
    }

    /// `for_code`'s match has no wildcard arm, so a code the compiler
    /// doesn't cover here fails to build; this spot-checks a few against
    /// D11's table text verbatim.
    #[test]
    fn for_code_reproduces_d11s_message_verbatim() {
        assert_eq!(
            HostOperationFailure::for_code(HostOperationFailureCode::NeverSent).message,
            "farm3d closed before sending this. Nothing reached the printer."
        );
        assert_eq!(
            HostOperationFailure::for_code(HostOperationFailureCode::HostFileDiffers).message,
            "A different file is at farm3d's path on the printer. Staging again replaces it."
        );
        assert_eq!(
            HostOperationFailure::for_code(HostOperationFailureCode::AuthRejected).code,
            HostOperationFailureCode::AuthRejected
        );
    }

    /// The migration's `kind`/`state` `CHECK` lists spell each stored enum
    /// exactly as serde does.
    #[test]
    fn stored_enums_serialize_to_the_d2_check_spellings() {
        let wire = |value: serde_json::Value| value.as_str().unwrap().to_string();
        let kinds = [
            HostOperationKind::Upload,
            HostOperationKind::Start,
            HostOperationKind::Pause,
            HostOperationKind::Resume,
            HostOperationKind::Cancel,
        ]
        .map(|kind| wire(serde_json::to_value(kind).unwrap()));
        assert_eq!(kinds, ["upload", "start", "pause", "resume", "cancel"]);

        let states = [
            HostOperationState::Dispatching,
            HostOperationState::Uncertain,
            HostOperationState::Reconciling,
            HostOperationState::Succeeded,
            HostOperationState::Failed,
            HostOperationState::Abandoned,
        ]
        .map(|state| wire(serde_json::to_value(state).unwrap()));
        assert_eq!(
            states,
            [
                "dispatching",
                "uncertain",
                "reconciling",
                "succeeded",
                "failed",
                "abandoned"
            ]
        );
    }

    /// D11's tagged resolution shapes, verbatim.
    #[test]
    fn resolution_uses_its_d11_tagged_wire_shape() {
        assert_eq!(
            serde_json::to_value(HostOperationResolution::ArtifactVerified { reconciled: true })
                .unwrap(),
            serde_json::json!({ "kind": "artifactVerified", "reconciled": true })
        );
        assert_eq!(
            serde_json::to_value(HostOperationResolution::StartAccepted).unwrap(),
            serde_json::json!({ "kind": "startAccepted" })
        );
        assert_eq!(
            serde_json::to_value(HostOperationResolution::StartObserved {
                source: StartEvidenceSource::History,
                history_job_id: Some("0000A1".to_string()),
                interrupted: true,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "startObserved",
                "source": "history",
                "historyJobId": "0000A1",
                "interrupted": true,
            })
        );
        assert_eq!(
            serde_json::to_value(HostOperationResolution::StateObserved {
                observed_state: HostOperationObservedState::Paused,
                reconciled: false,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "stateObserved",
                "observedState": "paused",
                "reconciled": false,
            })
        );
    }

    /// `history_mark` never reaches the wire, even when it's set.
    #[test]
    fn history_mark_is_never_serialized() {
        let operation = sample_operation();
        let value = serde_json::to_value(&operation).unwrap();
        assert!(value.get("historyMark").is_none());
        assert!(value.get("history_mark").is_none());
    }

    fn sample_operation() -> HostOperation {
        HostOperation {
            id: "hop-a".to_string(),
            printer_id: "prn-a".to_string(),
            kind: HostOperationKind::Start,
            state: HostOperationState::Dispatching,
            slice_revision_id: Some("slr-a".to_string()),
            source_host_operation_id: None,
            gcode_sha256: Some("a".repeat(64)),
            gcode_size: Some(100),
            host_path: "farm3d/slr-a.gcode".to_string(),
            history_mark: Some(3),
            endpoint: HostOperationEndpoint {
                kind: "moonraker".to_string(),
                host: "192.0.2.1".to_string(),
                port: 7125,
            },
            failure: None,
            resolution: None,
            attempts: 0,
            last_attempt: None,
            no_longer_pending: false,
            abandoned_at: None,
            abandon_note: None,
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            dispatched_at: None,
            uncertain_since: None,
            resolved_at: None,
        }
    }
}
