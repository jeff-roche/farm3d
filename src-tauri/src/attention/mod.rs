//! P8 D1/D2: the Attention domain's pure wire types — an Attention
//! Event's vocabulary (Conditions, severities, resolutions) and the
//! `attention_events` record itself. See the P8 design spec's D1
//! ("Vocabulary and ownership") and "Backend model" module layout table.
//!
//! This module (Task 3) is wire types only, plus the Attention lifecycle
//! rules ([`lifecycle`]). The Condition catalogue and observation
//! (`observe.rs`), the planner (`plan.rs`), the repository, the
//! projector, deep links, services, the event stream, and commands are
//! later tasks (see the module layout table in the design spec).

pub mod lifecycle;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::cameras::{CameraErrorKind, CameraHealth, EvidenceSkipReason};
use crate::incidents::Incident;

/// D2's Condition catalogue: exactly ten Conditions. Severity, action
/// requirement, resolution mode, recurrence, Incident behavior, and
/// notification class are fixed per Condition (the catalogue itself,
/// `ConditionKind::spec()`, is a later task's Rust-only addition).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[ts(export_to = "domain/ConditionKind.ts")]
pub enum ConditionKind {
    #[serde(rename = "printer.offline")]
    #[ts(rename = "printer.offline")]
    PrinterOffline,
    #[serde(rename = "printer.connectionError")]
    #[ts(rename = "printer.connectionError")]
    PrinterConnectionError,
    #[serde(rename = "printer.hostFailed")]
    #[ts(rename = "printer.hostFailed")]
    PrinterHostFailed,
    #[serde(rename = "job.startConfirmation")]
    #[ts(rename = "job.startConfirmation")]
    JobStartConfirmation,
    #[serde(rename = "job.failed")]
    #[ts(rename = "job.failed")]
    JobFailed,
    #[serde(rename = "job.hostCancelled")]
    #[ts(rename = "job.hostCancelled")]
    JobHostCancelled,
    #[serde(rename = "requirement.materialReconciliation")]
    #[ts(rename = "requirement.materialReconciliation")]
    RequirementMaterialReconciliation,
    #[serde(rename = "requirement.jobOutcomeUnknown")]
    #[ts(rename = "requirement.jobOutcomeUnknown")]
    RequirementJobOutcomeUnknown,
    #[serde(rename = "spool.low")]
    #[ts(rename = "spool.low")]
    SpoolLow,
    #[serde(rename = "job.completed")]
    #[ts(rename = "job.completed")]
    JobCompleted,
}

impl ConditionKind {
    pub const ALL: [ConditionKind; 10] = [
        ConditionKind::PrinterOffline,
        ConditionKind::PrinterConnectionError,
        ConditionKind::PrinterHostFailed,
        ConditionKind::JobStartConfirmation,
        ConditionKind::JobFailed,
        ConditionKind::JobHostCancelled,
        ConditionKind::RequirementMaterialReconciliation,
        ConditionKind::RequirementJobOutcomeUnknown,
        ConditionKind::SpoolLow,
        ConditionKind::JobCompleted,
    ];
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionSeverity.ts")]
pub enum AttentionSeverity {
    Fatal,
    Warning,
    Info,
}

impl AttentionSeverity {
    pub const ALL: [AttentionSeverity; 3] = [
        AttentionSeverity::Fatal,
        AttentionSeverity::Warning,
        AttentionSeverity::Info,
    ];
}

/// D1: how an open Attention Event's `resolved_at` may be set. `manual`
/// Events resolve only through `resolve_attention_event`
/// ([`lifecycle::apply`]'s `NotManual` rejects every other mode there).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ResolutionMode.ts")]
pub enum ResolutionMode {
    Auto,
    Action,
    Manual,
}

impl ResolutionMode {
    pub const ALL: [ResolutionMode; 3] =
        [ResolutionMode::Auto, ResolutionMode::Action, ResolutionMode::Manual];
}

/// D1: why an Attention Event resolved. `operatorResolved` only ever
/// applies to a `manual` Event; the other three are system resolutions,
/// which also set `read_at` (the umbrella's "resolving implies read").
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionResolution.ts")]
pub enum AttentionResolution {
    ConditionCleared,
    ActionCompleted,
    OperatorResolved,
    SourceRemoved,
}

impl AttentionResolution {
    pub const ALL: [AttentionResolution; 4] = [
        AttentionResolution::ConditionCleared,
        AttentionResolution::ActionCompleted,
        AttentionResolution::OperatorResolved,
        AttentionResolution::SourceRemoved,
    ];
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionOrigin.ts")]
pub enum AttentionOrigin {
    Live,
    Backfill,
}

impl AttentionOrigin {
    pub const ALL: [AttentionOrigin; 2] = [AttentionOrigin::Live, AttentionOrigin::Backfill];
}

/// D2's dedup key's second segment: `"<ConditionKind>:<AttentionSourceKind>:<sourceId>"`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionSourceKind.ts")]
pub enum AttentionSourceKind {
    Printer,
    Job,
    ReconciliationRequirement,
    Spool,
}

impl AttentionSourceKind {
    pub const ALL: [AttentionSourceKind; 4] = [
        AttentionSourceKind::Printer,
        AttentionSourceKind::Job,
        AttentionSourceKind::ReconciliationRequirement,
        AttentionSourceKind::Spool,
    ];
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionSource.ts")]
pub struct AttentionSource {
    pub kind: AttentionSourceKind,
    pub id: String,
}

/// D6: which settings toggle (`notify_*`) and Printer alert default a
/// Condition's notifications follow.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/NotificationClass.ts")]
pub enum NotificationClass {
    Fatal,
    Confirmation,
    Completion,
    Reconciliation,
    Connectivity,
    Inventory,
}

impl NotificationClass {
    pub const ALL: [NotificationClass; 6] = [
        NotificationClass::Fatal,
        NotificationClass::Confirmation,
        NotificationClass::Completion,
        NotificationClass::Reconciliation,
        NotificationClass::Connectivity,
        NotificationClass::Inventory,
    ];
}

/// D2 "Condition detail and subject": what an Attention Event's source
/// looked like at insert, fixed thereafter. Never a host, port, URL, or
/// credential reference (global constraint 3).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionSubject.ts")]
pub struct AttentionSubject {
    pub printer_name: Option<String>,
    pub printer_location: Option<String>,
    pub job_label: Option<String>,
    #[ts(type = "number | null")]
    pub spool_number: Option<i64>,
    pub spool_label: Option<String>,
}

/// D2: `printer.connectionError`'s narrower `Reach::Misconfigured` cause
/// (auth/protocol only — `unreachable`/`timeout` never reach this
/// Condition; see D2 "Reachability").
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/PrinterConnectionErrorCause.ts"
)]
pub enum PrinterConnectionErrorCause {
    Auth,
    Protocol,
}

impl PrinterConnectionErrorCause {
    pub const ALL: [PrinterConnectionErrorCause; 2] = [
        PrinterConnectionErrorCause::Auth,
        PrinterConnectionErrorCause::Protocol,
    ];
}

/// D2: `requirement.materialReconciliation`'s narrower status (never
/// `resolved` — the Condition is `Absent` once the requirement resolves).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/MaterialReconciliationStatus.ts"
)]
pub enum MaterialReconciliationStatus {
    Pending,
    Deferred,
}

impl MaterialReconciliationStatus {
    pub const ALL: [MaterialReconciliationStatus; 2] = [
        MaterialReconciliationStatus::Pending,
        MaterialReconciliationStatus::Deferred,
    ];
}

/// D2 "Condition detail and subject": planner-owned, a pure function of
/// the `FarmView` (only `Insert`/`Amend` ever write `detail_json`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/AttentionDetail.ts"
)]
pub enum AttentionDetail {
    PrinterOffline {
        unreachable_since: String,
    },
    PrinterConnectionError {
        cause: PrinterConnectionErrorCause,
    },
    PrinterHostFailed,
    JobStartConfirmation {
        awaiting_material: bool,
    },
    JobFailed {
        ended_at: String,
    },
    JobHostCancelled {
        ended_at: String,
    },
    RequirementMaterialReconciliation {
        requirement_status: MaterialReconciliationStatus,
        spool_id: String,
    },
    RequirementJobOutcomeUnknown,
    SpoolLow {
        #[ts(type = "number")]
        current_mg: i64,
        #[ts(type = "number")]
        low_threshold_mg: i64,
    },
    JobCompleted {
        ended_at: String,
    },
}

/// D2 "Evidence is not detail": the `job.completed` completion capture's
/// outcome, on its own column (`attention_events.evidence_json`), never
/// planner-owned. Written once by `attention::repository::record_evidence`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "status", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/EvidenceOutcome.ts"
)]
pub enum EvidenceOutcome {
    Captured {
        snapshot_id: String,
    },
    Skipped {
        reason: EvidenceSkipReason,
        error_kind: Option<CameraErrorKind>,
    },
}

/// D1: what the frontend may offer for an Attention Event — Rust-computed
/// (`markRead` if unread; `acknowledge` if open and unacknowledged;
/// `resolve` if open and `manual`).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionAction.ts")]
pub enum AttentionAction {
    MarkRead,
    Acknowledge,
    Resolve,
}

impl AttentionAction {
    pub const ALL: [AttentionAction; 3] = [
        AttentionAction::MarkRead,
        AttentionAction::Acknowledge,
        AttentionAction::Resolve,
    ];
}

/// `attention_events` in full (spec "Backend model" wire types).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionEvent.ts")]
pub struct AttentionEvent {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub dedup_key: String,
    pub condition: ConditionKind,
    pub severity: AttentionSeverity,
    pub requires_action: bool,
    pub resolution_mode: ResolutionMode,
    pub notification_class: NotificationClass,
    pub source: AttentionSource,
    pub printer_id: Option<String>,
    pub job_id: Option<String>,
    pub spool_id: Option<String>,
    pub requirement_id: Option<String>,
    pub incident_id: Option<String>,
    pub subject: AttentionSubject,
    pub detail: AttentionDetail,
    pub summary: String,
    pub origin: AttentionOrigin,
    pub first_observed_at: String,
    pub last_observed_at: String,
    #[ts(type = "number")]
    pub observation_count: i64,
    pub recurrence_of: Option<String>,
    pub read_at: Option<String>,
    pub acknowledged_at: Option<String>,
    pub resolved_at: Option<String>,
    pub resolution: Option<AttentionResolution>,
    pub notified_at: Option<String>,
    /// `job.completed` only; written by `record_evidence`, never by `Amend`.
    pub evidence: Option<EvidenceOutcome>,
    /// Rust-computed: markRead if unread; acknowledge if open and
    /// unacknowledged; resolve if open and manual.
    pub allowed_actions: Vec<AttentionAction>,
}

/// An opaque `list_attention`/backfill pagination cursor, encoding
/// `(resolvedAt, id)`. Never parsed by TypeScript — round-tripped only.
#[derive(Clone, PartialEq, Eq, Debug, TS)]
#[ts(type = "string", export_to = "domain/AttentionCursor.ts")]
pub struct AttentionCursor(pub String);

impl Serialize for AttentionCursor {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for AttentionCursor {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self)
    }
}

/// `list_attention`'s no-`resolvedBefore` result and the startup
/// snapshot the `attention` stream backfills from (D9 rename:
/// `AttentionSnapshot` -> `AttentionBackfill`).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionBackfill.ts")]
pub struct AttentionBackfill {
    pub stream_id: String,
    #[ts(type = "number")]
    pub snapshot_sequence: i64,
    /// Every open Event: severity (fatal, warning, info), then
    /// `firstObservedAt` descending, then id.
    pub open: Vec<AttentionEvent>,
    /// Up to `limit` (default 200), `resolvedAt` descending, then id
    /// descending.
    pub resolved: Vec<AttentionEvent>,
    /// `null` when no older resolved Event exists.
    pub resolved_cursor: Option<AttentionCursor>,
    pub open_incidents: Vec<Incident>,
    /// One per Printer with a camera source.
    pub camera_health: Vec<CameraHealth>,
}

/// What every mutating Attention/Incident command publishes after
/// commit: the Events and Incidents that changed.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AttentionChange.ts")]
pub struct AttentionChange {
    pub events: Vec<AttentionEvent>,
    pub incidents: Vec<Incident>,
}
