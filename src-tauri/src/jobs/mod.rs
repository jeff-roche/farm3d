//! P7 D1/D3: the Job domain — assignment, the dispatch driver, the
//! tracker, settlement, and the pure state machine ([`state`]). See the
//! P7 design spec's D1 (vocabulary and ownership) and D3 (the Job state
//! machine) for the rules this module's types encode.
//!
//! Task 2 landed the pure wire enums the D3 state machine needs and the
//! transition/settlement functions themselves. Task 3 (this module's
//! [`repository`]) adds the rest of `Job`'s wire type, the Job timeline
//! (`job_events`), and Reconciliation Requirements. Assignment, dispatch,
//! the tracker, settlement, guards, and commands are later tasks (see the
//! module layout table in the design spec).

pub mod assign;
pub mod commands;
pub mod dispatch;
pub mod recovery;
pub mod repository;
pub mod services;
pub mod settlement;
pub mod state;
pub mod tracker;

pub use recovery::recover_after_restart;
pub use services::{JobServices, JobTimings};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::catalog::PrinterProfile;
use crate::contracts::command::ErrorCode;
use crate::host_ops::HostOperationFailure;
use crate::printers::CatalogRef;
use crate::queue::{Blocker, QueueEntry};
use crate::spools::ledger::AmountEntry;

/// D3: a Job's state. `completed`, `failed`, and `cancelled` are
/// terminal; every other state is active. A partial unique index allows
/// at most one active Job per Printer (the 0008 migration).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobState.ts")]
pub enum JobState {
    Assigned,
    Staging,
    AwaitingStart,
    Starting,
    Printing,
    Paused,
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

impl JobState {
    pub const ALL: [JobState; 10] = [
        JobState::Assigned,
        JobState::Staging,
        JobState::AwaitingStart,
        JobState::Starting,
        JobState::Printing,
        JobState::Paused,
        JobState::Completed,
        JobState::Failed,
        JobState::Cancelled,
        JobState::OutcomeUnknown,
    ];

    /// D3: `completed`, `failed`, and `cancelled` are terminal.
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            JobState::Completed | JobState::Failed | JobState::Cancelled
        )
    }
}

/// D3: set exactly when a Job's state is `cancelled`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CancelReason.ts")]
pub enum CancelReason {
    ReleasedBeforeStart,
    CancelledBeforeStart,
    CancelledByOperator,
    HostCancelled,
    OperatorDeclared,
}

/// D3: how a Job's reserved material becomes a deduction, orthogonal to
/// its state.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Settlement.ts")]
pub enum Settlement {
    Open,
    NotRequired,
    Pending,
    Deferred,
    Settled,
}

/// D3: how a `settled` Job's reservation was consumed.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SettlementMethod.ts")]
pub enum SettlementMethod {
    Estimated,
    Measured,
}

/// Ruling R1: who assigned a Queue Entry to become this Job.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AssignedBy.ts")]
pub enum AssignedBy {
    Operator,
    Automatic,
}

/// Ruling R2: the Printer state `start_job` was offered from, so the
/// bed-clear confirmation can name it (D9).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StartConfirmation.ts")]
pub enum StartConfirmation {
    BedClear,
    Unattended,
}

/// D3: what a Job command asks of a Job. `JOB_ACTION_NOT_ALLOWED` names
/// the refused one in its `details.action`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobAction.ts")]
pub enum JobAction {
    Stage,
    Start,
    Pause,
    Resume,
    Cancel,
    Release,
    Retry,
    DeclareOutcome,
    SettleMaterial,
    CorrectMaterial,
}

impl JobAction {
    /// The user command whose D3 event this is, if any (D3's "Illegal event
    /// raised by" table). `None` for the events only farm3d's own code
    /// raises: an illegal one of those is a bug (`INTERNAL`).
    pub fn for_user_event(event: JobEventKind) -> Option<JobAction> {
        match event {
            JobEventKind::StageHandedOff => Some(JobAction::Stage),
            JobEventKind::StartHandedOff => Some(JobAction::Start),
            JobEventKind::PauseHandedOff => Some(JobAction::Pause),
            JobEventKind::ResumeHandedOff => Some(JobAction::Resume),
            JobEventKind::CancelHandedOff | JobEventKind::CancelledBeforeStart => {
                Some(JobAction::Cancel)
            }
            JobEventKind::Released => Some(JobAction::Release),
            JobEventKind::DeclaredCompleted
            | JobEventKind::DeclaredFailed
            | JobEventKind::DeclaredCancelled => Some(JobAction::DeclareOutcome),
            JobEventKind::MaterialSettled | JobEventKind::MaterialDeferred => {
                Some(JobAction::SettleMaterial)
            }
            JobEventKind::MaterialCorrected => Some(JobAction::CorrectMaterial),
            _ => None,
        }
    }
}

/// D3's `job_events` kinds. `Assigned` is the Job's insert event: it has
/// no `from_state` and is not a transition (`jobs::state::transition`
/// never accepts it). Every other kind names one D3 edge.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobEventKind.ts")]
pub enum JobEventKind {
    Assigned,
    StageHandedOff,
    StageSucceeded,
    StageFailed,
    StartHandedOff,
    StartSucceeded,
    StartFailed,
    StartAbandoned,
    HostJobPinned,
    PauseHandedOff,
    ResumeHandedOff,
    CancelHandedOff,
    ControlFailed,
    Paused,
    Resumed,
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
    DeclaredCompleted,
    DeclaredFailed,
    DeclaredCancelled,
    Released,
    CancelledBeforeStart,
    MaterialSettled,
    MaterialDeferred,
    MaterialCorrected,
}

impl JobEventKind {
    pub const ALL: [JobEventKind; 27] = [
        JobEventKind::Assigned,
        JobEventKind::StageHandedOff,
        JobEventKind::StageSucceeded,
        JobEventKind::StageFailed,
        JobEventKind::StartHandedOff,
        JobEventKind::StartSucceeded,
        JobEventKind::StartFailed,
        JobEventKind::StartAbandoned,
        JobEventKind::HostJobPinned,
        JobEventKind::PauseHandedOff,
        JobEventKind::ResumeHandedOff,
        JobEventKind::CancelHandedOff,
        JobEventKind::ControlFailed,
        JobEventKind::Paused,
        JobEventKind::Resumed,
        JobEventKind::Completed,
        JobEventKind::Failed,
        JobEventKind::Cancelled,
        JobEventKind::OutcomeUnknown,
        JobEventKind::DeclaredCompleted,
        JobEventKind::DeclaredFailed,
        JobEventKind::DeclaredCancelled,
        JobEventKind::Released,
        JobEventKind::CancelledBeforeStart,
        JobEventKind::MaterialSettled,
        JobEventKind::MaterialDeferred,
        JobEventKind::MaterialCorrected,
    ];
}

/// What a Job's Printer looked like at assignment (`printer_snapshot_json`,
/// spec "Backend model"). Never carries an endpoint or a credential
/// reference — see the 0008 migration's notes.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterSnapshot.ts")]
pub struct PrinterSnapshot {
    pub name: String,
    pub location: Option<String>,
    pub catalog_ref: Option<CatalogRef>,
    pub adapter_kind: Option<String>,
    pub profile: PrinterProfile,
}

/// D3/D9: what a Job's own record or a proved Host Operation says went
/// wrong, most recently. `refused` is a farm3d-side rejection (never sent
/// to the host); the other two describe the linked Host Operation's own
/// outcome.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/JobFailure.ts"
)]
pub enum JobFailure {
    HostOperationFailed {
        at: String,
        host_operation_id: String,
        failure: HostOperationFailure,
    },
    HostOperationAbandoned {
        at: String,
        host_operation_id: String,
    },
    Refused {
        at: String,
        code: ErrorCode,
        message: String,
    },
}

/// Ruling R4: `Job.settlementPreview`, present only while `settlement` is
/// `pending` or `deferred` (spec "Material settlement").
/// `estimatedUseMg = ceil(estimateMg × maxProgressPct / 100)`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SettlementPreview.ts")]
pub struct SettlementPreview {
    #[ts(type = "number")]
    pub estimated_use_mg: i64,
}

/// `estimatedUseMg = ceil(estimateMg × maxProgressPct / 100)`, the
/// integer form the spec gives: `(estimateMg * pct + 99) / 100`. `0` when
/// the Job never printed (`maxProgressPct == 0`).
pub fn estimated_use_mg(estimate_mg: i64, max_progress_pct: i64) -> i64 {
    (estimate_mg * max_progress_pct + 99) / 100
}

/// Material settlement's operator choice (spec "Material settlement",
/// `settle_job_material`'s `choice` argument). `Estimated`/`Measured` are
/// allowed while settlement is `pending` or `deferred`; `Defer` only from
/// `pending` (`jobs::settlement::settle`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/SettleChoice.ts"
)]
pub enum SettleChoice {
    Estimated,
    Measured { entry: AmountEntry },
    Defer,
}

/// `JOB_ALREADY_SETTLED`'s `details.reason` (spec "Error codes"): a second
/// `settle_job_material` on an already-`settled` Job, or a second
/// `correct_job_material` on a Job that already has a correction.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum SettleFailureReason {
    Settled,
    Corrected,
}

/// A Job as the wire shares it (spec "Backend model" wire types).
/// Assembled by [`repository`] from the `jobs` row. `allowedActions` is
/// computed there, at read time (D3's table, [`state::allowed_actions`]).
/// `startBlockers` depends on the live Printer status, so the repository
/// leaves it empty and [`dispatch::present_jobs`] fills it for an
/// `awaitingStart` Job before the Job leaves Rust (D7).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Job.ts")]
pub struct Job {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub queue_entry_id: String,
    pub slice_revision_id: String,
    pub printer_id: String,
    pub printer_snapshot: PrinterSnapshot,
    pub spool_id: String,
    pub reservation_id: String,
    #[ts(type = "number")]
    pub estimate_mg: i64,
    pub state: JobState,
    pub cancel_reason: Option<CancelReason>,
    pub settlement: Settlement,
    pub settlement_method: Option<SettlementMethod>,
    pub settlement_preview: Option<SettlementPreview>,
    pub corrected: bool,
    pub assigned_by: AssignedBy,
    pub start_confirmation: Option<StartConfirmation>,
    pub upload_host_operation_id: Option<String>,
    pub active_host_operation_id: Option<String>,
    #[ts(type = "number")]
    pub max_progress_pct: i64,
    pub host_unreachable_since: Option<String>,
    pub host_path: Option<String>,
    pub last_failure: Option<JobFailure>,
    /// D7: non-empty only in `awaitingStart`.
    pub start_blockers: Vec<Blocker>,
    /// D3: what the frontend may offer. Rust-computed.
    pub allowed_actions: Vec<JobAction>,
    pub created_at: String,
    pub updated_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
}

/// `job_events` in full (spec "Backend model" wire types). `Assigned`
/// (the insert event) carries no `fromState`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobEvent.ts")]
pub struct JobEvent {
    pub id: String,
    pub job_id: String,
    #[ts(type = "number")]
    pub sequence: i64,
    pub kind: JobEventKind,
    pub from_state: Option<JobState>,
    pub to_state: JobState,
    pub operation_id: Option<String>,
    pub host_operation_id: Option<String>,
    #[ts(type = "Record<string, unknown> | null")]
    pub detail: Option<serde_json::Value>,
    pub at: String,
}

/// D1: what a Reconciliation Requirement is durably about.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/RequirementKind.ts")]
pub enum RequirementKind {
    MaterialReconciliation,
    JobOutcomeUnknown,
}

/// D1: a Reconciliation Requirement's own lifecycle, independent of the
/// Job's.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/RequirementStatus.ts")]
pub enum RequirementStatus {
    Pending,
    Deferred,
    Resolved,
}

/// D9: the outcome an operator declares for an `outcomeUnknown` Job (or a
/// `printing`/`paused` one the host has been unreachable for, D9).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DeclaredOutcome.ts")]
pub enum DeclaredOutcome {
    Completed,
    Failed,
    Cancelled,
}

/// D1/D4: how a resolved Reconciliation Requirement was closed.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/RequirementResolution.ts"
)]
pub enum RequirementResolution {
    Settled {
        method: SettlementMethod,
        #[ts(type = "number")]
        used_mg: i64,
    },
    Declared {
        outcome: DeclaredOutcome,
    },
}

/// D1: a durable Reconciliation Requirement row, one per `(job, kind)`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ReconciliationRequirement.ts")]
pub struct ReconciliationRequirement {
    pub id: String,
    pub job_id: String,
    pub kind: RequirementKind,
    pub status: RequirementStatus,
    pub spool_id: Option<String>,
    pub reservation_id: Option<String>,
    pub opened_at: String,
    pub deferred_at: Option<String>,
    pub resolved_at: Option<String>,
    pub resolution: Option<RequirementResolution>,
}

/// `get_job_history`'s result (spec "Backend model" wire types): the Job,
/// its Queue Entry, every entry in that entry's lineage, the Job's
/// timeline in sequence order, its reservation(s), its linked Host
/// Operations, and its Reconciliation Requirements.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobHistory.ts")]
pub struct JobHistory {
    pub job: Job,
    pub entry: QueueEntry,
    pub lineage: Vec<QueueEntry>,
    pub events: Vec<JobEvent>,
    pub reservations: Vec<crate::spools::reservations::Reservation>,
    pub host_operations: Vec<crate::host_ops::HostOperation>,
    pub requirements: Vec<ReconciliationRequirement>,
}
