//! P8 D1/D2: the Attention domain's pure wire types — an Attention
//! Event's vocabulary (Conditions, severities, resolutions) and the
//! `attention_events` record itself. See the P8 design spec's D1
//! ("Vocabulary and ownership") and "Backend model" module layout table.
//!
//! It holds the wire types, the Rust-only [`Condition`] and the D2
//! catalogue ([`ConditionKind::spec`]), plus the Attention lifecycle
//! rules ([`lifecycle`]), the pure observer ([`observe`]), and the pure
//! planner ([`plan`]). The repository, the projector, deep links,
//! services, the event stream, and commands are later tasks (see the
//! module layout table in the design spec).

pub mod lifecycle;
pub mod observe;
pub mod plan;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::cameras::{CameraErrorKind, CameraHealth, EvidenceSkipReason};
use crate::incidents::Incident;

/// D2's Condition catalogue: exactly ten Conditions. Severity, action
/// requirement, resolution mode, recurrence, Incident behavior, and
/// notification class are fixed per Condition ([`ConditionKind::spec`]).
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

impl ConditionKind {
    /// The wire string (`"printer.offline"`, …), the dedup key's first
    /// segment.
    pub fn as_str(self) -> &'static str {
        match self {
            ConditionKind::PrinterOffline => "printer.offline",
            ConditionKind::PrinterConnectionError => "printer.connectionError",
            ConditionKind::PrinterHostFailed => "printer.hostFailed",
            ConditionKind::JobStartConfirmation => "job.startConfirmation",
            ConditionKind::JobFailed => "job.failed",
            ConditionKind::JobHostCancelled => "job.hostCancelled",
            ConditionKind::RequirementMaterialReconciliation => {
                "requirement.materialReconciliation"
            }
            ConditionKind::RequirementJobOutcomeUnknown => "requirement.jobOutcomeUnknown",
            ConditionKind::SpoolLow => "spool.low",
            ConditionKind::JobCompleted => "job.completed",
        }
    }

    /// D2's Condition catalogue: every fixed property of this Condition.
    /// No Condition changes severity while open.
    pub fn spec(self) -> ConditionSpec {
        use AttentionSeverity::{Fatal, Info, Warning};
        use AttentionSourceKind as S;
        use IncidentRule as I;
        use NotificationClass as N;
        use ResolutionMode::{Action, Auto, Manual};
        let (source_kind, severity, requires_action, resolution_mode, recurs, incident, class) =
            match self {
                ConditionKind::PrinterOffline => (
                    S::Printer,
                    Warning,
                    true,
                    Auto,
                    true,
                    I::None,
                    N::Connectivity,
                ),
                ConditionKind::PrinterConnectionError => (
                    S::Printer,
                    Warning,
                    true,
                    Auto,
                    true,
                    I::None,
                    N::Connectivity,
                ),
                ConditionKind::PrinterHostFailed => {
                    (S::Printer, Fatal, true, Auto, true, I::OpensNew, N::Fatal)
                }
                ConditionKind::JobStartConfirmation => (
                    S::Job,
                    Info,
                    true,
                    Action,
                    true,
                    I::LinksJob,
                    N::Confirmation,
                ),
                ConditionKind::JobFailed => (
                    S::Job,
                    Fatal,
                    true,
                    Manual,
                    false,
                    I::OpensOrLinksJob,
                    N::Fatal,
                ),
                ConditionKind::JobHostCancelled => (
                    S::Job,
                    Warning,
                    true,
                    Manual,
                    false,
                    I::OpensOrLinksJob,
                    N::Connectivity,
                ),
                ConditionKind::RequirementMaterialReconciliation => (
                    S::ReconciliationRequirement,
                    Warning,
                    true,
                    Action,
                    false,
                    I::LinksJob,
                    N::Reconciliation,
                ),
                ConditionKind::RequirementJobOutcomeUnknown => (
                    S::ReconciliationRequirement,
                    Fatal,
                    true,
                    Action,
                    false,
                    I::OpensOrLinksJob,
                    N::Fatal,
                ),
                ConditionKind::SpoolLow => {
                    (S::Spool, Warning, false, Auto, true, I::None, N::Inventory)
                }
                ConditionKind::JobCompleted => {
                    (S::Job, Info, false, Auto, false, I::None, N::Completion)
                }
            };
        ConditionSpec {
            source_kind,
            severity,
            requires_action,
            resolution_mode,
            recurs,
            incident,
            notification_class: class,
        }
    }
}

impl AttentionSourceKind {
    /// The wire string (`"printer"`, …), the dedup key's second segment.
    pub fn as_str(self) -> &'static str {
        match self {
            AttentionSourceKind::Printer => "printer",
            AttentionSourceKind::Job => "job",
            AttentionSourceKind::ReconciliationRequirement => "reconciliationRequirement",
            AttentionSourceKind::Spool => "spool",
        }
    }
}

impl AttentionDetail {
    /// The one Condition whose `detail` this variant is (the D2 detail
    /// table is one variant per Condition).
    pub fn condition_kind(&self) -> ConditionKind {
        match self {
            AttentionDetail::PrinterOffline { .. } => ConditionKind::PrinterOffline,
            AttentionDetail::PrinterConnectionError { .. } => ConditionKind::PrinterConnectionError,
            AttentionDetail::PrinterHostFailed => ConditionKind::PrinterHostFailed,
            AttentionDetail::JobStartConfirmation { .. } => ConditionKind::JobStartConfirmation,
            AttentionDetail::JobFailed { .. } => ConditionKind::JobFailed,
            AttentionDetail::JobHostCancelled { .. } => ConditionKind::JobHostCancelled,
            AttentionDetail::RequirementMaterialReconciliation { .. } => {
                ConditionKind::RequirementMaterialReconciliation
            }
            AttentionDetail::RequirementJobOutcomeUnknown => {
                ConditionKind::RequirementJobOutcomeUnknown
            }
            AttentionDetail::SpoolLow { .. } => ConditionKind::SpoolLow,
            AttentionDetail::JobCompleted { .. } => ConditionKind::JobCompleted,
        }
    }
}

/// D2 "Dedup keys": `"<ConditionKind>:<AttentionSourceKind>:<sourceId>"`.
/// Neither the condition nor the source kind contains `:`, and the id is
/// last, so a key is unique without escaping. Keys are compared, never
/// parsed.
pub fn dedup_key(kind: ConditionKind, source_id: &str) -> String {
    format!(
        "{}:{}:{}",
        kind.as_str(),
        kind.spec().source_kind.as_str(),
        source_id
    )
}

/// D2 catalogue "Incident" column: what an inserted Event of this
/// Condition does to Incidents (applied by the projector, D2 "Apply").
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum IncidentRule {
    /// Never opens or links an Incident.
    None,
    /// Opens a new Incident with no Job (`printer.hostFailed`).
    OpensNew,
    /// Opens the Job's Incident, or links to it if it exists.
    OpensOrLinksJob,
    /// Links to the Job's Incident, if any.
    LinksJob,
}

/// One row of D2's Condition catalogue.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ConditionSpec {
    pub source_kind: AttentionSourceKind,
    pub severity: AttentionSeverity,
    pub requires_action: bool,
    pub resolution_mode: ResolutionMode,
    /// A recurring Condition that returns after its Event resolved inserts
    /// a new Event with `recurrenceOf`; a once-per-source one never gets a
    /// second Event for the same key.
    pub recurs: bool,
    pub incident: IncidentRule,
    pub notification_class: NotificationClass,
}

/// D2: a current fact that needs the operator's attention (Rust-only,
/// never stored). `observe` builds it from the `FarmView`; the planner
/// inserts it as an Attention Event or amends the open one.
#[derive(Clone, PartialEq, Debug)]
pub struct Condition {
    pub kind: ConditionKind,
    pub source_id: String,
    /// The source for `printer.*`; the Job's Printer for `job.*` and
    /// `requirement.*`; `None` for `spool.low` (and when the Job isn't in
    /// the view).
    pub printer_id: Option<String>,
    /// The source for `job.*`; the requirement's Job for `requirement.*`.
    pub job_id: Option<String>,
    /// The source for `spool.low`; the requirement's Spool for
    /// `requirement.materialReconciliation`; `None` otherwise.
    pub spool_id: Option<String>,
    /// The source for `requirement.*`.
    pub requirement_id: Option<String>,
    /// Fixed at insert; never rewritten by `Amend`.
    pub subject: AttentionSubject,
    /// Planner-owned: a pure function of the `FarmView`.
    pub detail: AttentionDetail,
    /// D2 planner rule: insert (or acknowledge) the Event as acknowledged
    /// by the system — a `deferred` material requirement.
    pub acknowledge: bool,
}

impl Condition {
    pub fn spec(&self) -> ConditionSpec {
        self.kind.spec()
    }

    pub fn dedup_key(&self) -> String {
        dedup_key(self.kind, &self.source_id)
    }

    pub fn source(&self) -> AttentionSource {
        AttentionSource {
            kind: self.spec().source_kind,
            id: self.source_id.clone(),
        }
    }

    /// The Event's one-sentence summary for this Condition's own subject
    /// and detail (see [`summary`]).
    pub fn summary(&self) -> String {
        summary(self.kind, &self.subject, &self.detail)
    }
}

/// The longest `attention_events.summary` the schema accepts.
pub const SUMMARY_MAX_CHARS: usize = 500;

/// D2 "Condition detail and subject": the Rust-built, one-sentence
/// summary shown in the center and used as the notification body. A pure
/// function of the Condition kind, the Event's subject (fixed at insert),
/// and its detail, so an `Amend` that changes the detail re-derives it
/// (a `spool.low` "80 g left" never goes stale). Never empty; at most
/// [`SUMMARY_MAX_CHARS`] characters. Never holds a host, URL, or
/// credential: the subject can't.
pub fn summary(
    kind: ConditionKind,
    subject: &AttentionSubject,
    detail: &AttentionDetail,
) -> String {
    let printer_name = subject.printer_name.as_deref().unwrap_or("A Printer");
    let printer = match (&subject.printer_name, &subject.printer_location) {
        (Some(name), Some(location)) => format!("{name} ({location})"),
        _ => printer_name.to_string(),
    };
    let job = subject.job_label.as_deref().unwrap_or("A Job");
    let on = subject
        .printer_name
        .as_deref()
        .map(|name| format!(" on {name}"))
        .unwrap_or_default();
    let spool = subject
        .spool_number
        .map(|number| format!("Spool #{number}"))
        .unwrap_or_else(|| "A Spool".to_string());
    let text = match (kind, detail) {
        (ConditionKind::PrinterOffline, _) => format!("{printer} is offline."),
        (
            ConditionKind::PrinterConnectionError,
            AttentionDetail::PrinterConnectionError {
                cause: PrinterConnectionErrorCause::Auth,
            },
        ) => format!("{printer} rejected its connection credentials."),
        (ConditionKind::PrinterConnectionError, _) => {
            format!("{printer} answered in a way farm3d couldn't read.")
        }
        (ConditionKind::PrinterHostFailed, _) => format!("{printer} reported a failed print."),
        (
            ConditionKind::JobStartConfirmation,
            AttentionDetail::JobStartConfirmation {
                awaiting_material: true,
            },
        ) => format!("{job} is waiting to start{on}, but its Spool isn't loaded."),
        (ConditionKind::JobStartConfirmation, _) => format!("{job} is waiting to start{on}."),
        (ConditionKind::JobFailed, _) => format!("{job} failed{on}."),
        (ConditionKind::JobHostCancelled, _) => format!("{job} was cancelled by the host{on}."),
        (
            ConditionKind::RequirementMaterialReconciliation,
            AttentionDetail::RequirementMaterialReconciliation {
                requirement_status: MaterialReconciliationStatus::Deferred,
                ..
            },
        ) => format!("{job} has deferred material to settle."),
        (ConditionKind::RequirementMaterialReconciliation, _) => {
            format!("{job} needs its material settled.")
        }
        (ConditionKind::RequirementJobOutcomeUnknown, _) => {
            format!("{job} has an unknown outcome{on}.")
        }
        (ConditionKind::SpoolLow, AttentionDetail::SpoolLow { current_mg, .. }) => {
            format!("{spool} is low ({} left).", grams(*current_mg))
        }
        (ConditionKind::SpoolLow, _) => format!("{spool} is low."),
        (ConditionKind::JobCompleted, _) => format!("{job} finished{on}."),
    };
    truncate_chars(text, SUMMARY_MAX_CHARS)
}

/// Milligrams as whole grams, rounded half up ("80 g").
fn grams(mg: i64) -> String {
    let g = if mg >= 0 {
        (mg + 500) / 1000
    } else {
        -((-mg + 500) / 1000)
    };
    format!("{g} g")
}

fn truncate_chars(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        return text;
    }
    let mut out: String = text.chars().take(max - 1).collect();
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wire(value: impl Serialize) -> String {
        serde_json::to_value(value)
            .unwrap()
            .as_str()
            .unwrap()
            .to_string()
    }

    #[test]
    fn as_str_matches_the_wire_names() {
        for kind in ConditionKind::ALL {
            assert_eq!(kind.as_str(), wire(kind));
        }
        for kind in AttentionSourceKind::ALL {
            assert_eq!(kind.as_str(), wire(kind));
        }
    }

    /// D2's Condition catalogue, copied row by row.
    #[test]
    fn catalogue_matches_the_spec_table() {
        use AttentionSeverity::*;
        use AttentionSourceKind as S;
        use IncidentRule as I;
        use NotificationClass as N;
        use ResolutionMode::*;
        let table = [
            (
                "printer.offline",
                S::Printer,
                Warning,
                true,
                Auto,
                true,
                I::None,
                N::Connectivity,
            ),
            (
                "printer.connectionError",
                S::Printer,
                Warning,
                true,
                Auto,
                true,
                I::None,
                N::Connectivity,
            ),
            (
                "printer.hostFailed",
                S::Printer,
                Fatal,
                true,
                Auto,
                true,
                I::OpensNew,
                N::Fatal,
            ),
            (
                "job.startConfirmation",
                S::Job,
                Info,
                true,
                Action,
                true,
                I::LinksJob,
                N::Confirmation,
            ),
            (
                "job.failed",
                S::Job,
                Fatal,
                true,
                Manual,
                false,
                I::OpensOrLinksJob,
                N::Fatal,
            ),
            (
                "job.hostCancelled",
                S::Job,
                Warning,
                true,
                Manual,
                false,
                I::OpensOrLinksJob,
                N::Connectivity,
            ),
            (
                "requirement.materialReconciliation",
                S::ReconciliationRequirement,
                Warning,
                true,
                Action,
                false,
                I::LinksJob,
                N::Reconciliation,
            ),
            (
                "requirement.jobOutcomeUnknown",
                S::ReconciliationRequirement,
                Fatal,
                true,
                Action,
                false,
                I::OpensOrLinksJob,
                N::Fatal,
            ),
            (
                "spool.low",
                S::Spool,
                Warning,
                false,
                Auto,
                true,
                I::None,
                N::Inventory,
            ),
            (
                "job.completed",
                S::Job,
                Info,
                false,
                Auto,
                false,
                I::None,
                N::Completion,
            ),
        ];
        assert_eq!(table.len(), ConditionKind::ALL.len());
        for (kind, row) in ConditionKind::ALL.into_iter().zip(table) {
            let spec = kind.spec();
            assert_eq!(kind.as_str(), row.0);
            assert_eq!(
                (
                    spec.source_kind,
                    spec.severity,
                    spec.requires_action,
                    spec.resolution_mode,
                    spec.recurs,
                    spec.incident,
                    spec.notification_class
                ),
                (row.1, row.2, row.3, row.4, row.5, row.6, row.7),
                "{}",
                row.0
            );
        }
    }

    #[test]
    fn every_detail_variant_names_a_distinct_condition() {
        let details = [
            AttentionDetail::PrinterOffline {
                unreachable_since: "2026-09-27T11:50:00Z".into(),
            },
            AttentionDetail::PrinterConnectionError {
                cause: PrinterConnectionErrorCause::Auth,
            },
            AttentionDetail::PrinterHostFailed,
            AttentionDetail::JobStartConfirmation {
                awaiting_material: false,
            },
            AttentionDetail::JobFailed {
                ended_at: "2026-09-27T11:30:00Z".into(),
            },
            AttentionDetail::JobHostCancelled {
                ended_at: "2026-09-27T11:30:00Z".into(),
            },
            AttentionDetail::RequirementMaterialReconciliation {
                requirement_status: MaterialReconciliationStatus::Pending,
                spool_id: "spl-1".into(),
            },
            AttentionDetail::RequirementJobOutcomeUnknown,
            AttentionDetail::SpoolLow {
                current_mg: 1,
                low_threshold_mg: 2,
            },
            AttentionDetail::JobCompleted {
                ended_at: "2026-09-27T11:30:00Z".into(),
            },
        ];
        let kinds: Vec<_> = details
            .iter()
            .map(AttentionDetail::condition_kind)
            .collect();
        assert_eq!(kinds, ConditionKind::ALL);
    }

    #[test]
    fn dedup_key_is_condition_source_kind_and_id() {
        assert_eq!(
            dedup_key(ConditionKind::PrinterOffline, "prn-7f3c"),
            "printer.offline:printer:prn-7f3c"
        );
        assert_eq!(
            dedup_key(ConditionKind::RequirementJobOutcomeUnknown, "rrq-1"),
            "requirement.jobOutcomeUnknown:reconciliationRequirement:rrq-1"
        );
    }

    fn subject() -> AttentionSubject {
        AttentionSubject {
            printer_name: Some("Voron".into()),
            printer_location: Some("Bay A".into()),
            job_label: Some("Cube — Plate 1".into()),
            spool_number: Some(12),
            spool_label: Some("Polymaker PLA".into()),
        }
    }

    #[test]
    fn summaries_follow_the_spec_examples() {
        assert_eq!(
            summary(
                ConditionKind::PrinterOffline,
                &subject(),
                &AttentionDetail::PrinterOffline {
                    unreachable_since: "2026-09-27T11:50:00Z".into()
                }
            ),
            "Voron (Bay A) is offline."
        );
        assert_eq!(
            summary(
                ConditionKind::JobFailed,
                &subject(),
                &AttentionDetail::JobFailed {
                    ended_at: "2026-09-27T11:30:00Z".into()
                }
            ),
            "Cube — Plate 1 failed on Voron."
        );
        assert_eq!(
            summary(
                ConditionKind::SpoolLow,
                &subject(),
                &AttentionDetail::SpoolLow {
                    current_mg: 80_000,
                    low_threshold_mg: 100_000
                }
            ),
            "Spool #12 is low (80 g left)."
        );
    }

    #[test]
    fn summary_follows_the_detail_so_an_amend_can_rederive_it() {
        let low = |mg| {
            summary(
                ConditionKind::SpoolLow,
                &subject(),
                &AttentionDetail::SpoolLow {
                    current_mg: mg,
                    low_threshold_mg: 100_000,
                },
            )
        };
        assert_eq!(low(90_000), "Spool #12 is low (90 g left).");
        assert_eq!(low(79_600), "Spool #12 is low (80 g left).");
        assert_eq!(low(0), "Spool #12 is low (0 g left).");
    }

    #[test]
    fn summary_is_never_empty_and_never_over_the_schema_limit() {
        let empty = AttentionSubject {
            printer_name: None,
            printer_location: None,
            job_label: None,
            spool_number: None,
            spool_label: None,
        };
        assert_eq!(
            summary(
                ConditionKind::PrinterHostFailed,
                &empty,
                &AttentionDetail::PrinterHostFailed
            ),
            "A Printer reported a failed print."
        );
        let long = AttentionSubject {
            job_label: Some("x".repeat(2000)),
            ..empty
        };
        let text = summary(
            ConditionKind::JobCompleted,
            &long,
            &AttentionDetail::JobCompleted {
                ended_at: "2026-09-27T11:30:00Z".into(),
            },
        );
        assert_eq!(text.chars().count(), SUMMARY_MAX_CHARS);
    }
}
