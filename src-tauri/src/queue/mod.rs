//! P7 D1/D2: the Queue Entry domain — order, lineage, and the pure state
//! machine ([`state`]). See the P7 design spec's D1 (vocabulary and
//! ownership) and D2 (the Queue Entry state machine) for the rules this
//! module's types encode.
//!
//! This task (Task 2) only lands the pure wire enums the D2 state machine
//! needs and the transition function itself. The wire types the rest of
//! `QueueEntry` needs (`QueueEntryDisplay`, `QueueSnapshot`, and so on),
//! the repository, eligibility, the evaluator, events, and commands are
//! later tasks (see the module layout table in the design spec).

pub mod state;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// D2: a Queue Entry's state. A closed entry always has a [`CloseReason`]
/// (stored alongside it, not carried on this variant — see the entry's
/// `closeReason` field in the wire type).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/QueueEntryState.ts")]
pub enum QueueEntryState {
    Queued,
    Assigned,
    Closed,
}

impl QueueEntryState {
    pub const ALL: [QueueEntryState; 3] = [
        QueueEntryState::Queued,
        QueueEntryState::Assigned,
        QueueEntryState::Closed,
    ];
}

/// D2: why a closed Queue Entry closed.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/CloseReason.ts")]
pub enum CloseReason {
    Completed,
    Failed,
    Cancelled,
    Released,
    Removed,
}

/// D1/D2: a Queue Entry's rule for becoming a Job.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DispatchPolicy.ts")]
pub enum DispatchPolicy {
    Manual,
    Recommended,
    Automatic,
}

/// D1: how Automatic and Recommended rank Printers.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/DispatchPreference.ts")]
pub enum DispatchPreference {
    LoadedFirst,
    LeastRecentlyUsed,
}

/// A Queue Entry's fixed material estimate's provenance.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/EstimateSource.ts")]
pub enum EstimateSource {
    SliceEstimate,
    FileClaimConfirmed,
    OperatorEntered,
}

/// A Queue Entry's fixed material estimate.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/MaterialEstimate.ts")]
pub struct MaterialEstimate {
    #[ts(type = "number")]
    pub amount_mg: i64,
    pub source: EstimateSource,
}

/// D1: how a retried or released entry relates to its origin.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/OriginKind.ts")]
pub enum OriginKind {
    Retry,
    Release,
}
