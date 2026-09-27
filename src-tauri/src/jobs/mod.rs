//! P7 D1/D3: the Job domain — assignment, the dispatch driver, the
//! tracker, settlement, and the pure state machine ([`state`]). See the
//! P7 design spec's D1 (vocabulary and ownership) and D3 (the Job state
//! machine) for the rules this module's types encode.
//!
//! This task (Task 2) only lands the pure wire enums the D3 state machine
//! needs and the transition/settlement functions themselves. The rest of
//! `Job`'s wire type, the repository, assignment, dispatch, the tracker,
//! settlement, guards, and commands are later tasks (see the module
//! layout table in the design spec).

pub mod state;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

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
