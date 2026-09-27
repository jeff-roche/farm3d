//! P7 D1/D2: the Queue Entry domain — order, lineage, and the pure state
//! machine ([`state`]). See the P7 design spec's D1 (vocabulary and
//! ownership) and D2 (the Queue Entry state machine) for the rules this
//! module's types encode.
//!
//! Task 2 landed the pure wire enums the D2 state machine needs and the
//! transition function itself. Task 3 (this module's [`repository`]) adds
//! the rest of `QueueEntry`'s wire shape (`QueueEntryDisplay`,
//! `QueueEntryAction`) and the SQL that creates, orders, and closes rows.
//! `QueueSnapshot`, the eligibility types, events, and commands are later
//! tasks (see the module layout table in the design spec).

pub mod eligibility;
pub mod repository;
pub mod state;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contracts::command::RecoveryCode;
use crate::spools::MaterialFamily;

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

/// What a Queue Entry's linked Slice Revision looks like in a list, joined
/// in from `slicing::repository::load_revision` and `library::repository
/// ::load_model` (Task 3; spec "Backend model" wire types).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/QueueEntryDisplay.ts")]
pub struct QueueEntryDisplay {
    pub model_id: String,
    pub model_name: String,
    pub plate_label: Option<String>,
    pub target_label: String,
    pub material_family: Option<MaterialFamily>,
    pub material_other: Option<String>,
    #[ts(type = "number | null")]
    pub print_seconds: Option<u64>,
}

/// D2: what a command may do to an open Queue Entry, computed from its
/// state alone ([`allowed_actions`]) — `queued` allows all four,
/// `assigned` only `move` (its Job's controls own release/cancel
/// instead), `closed` allows none.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/QueueEntryAction.ts")]
pub enum QueueEntryAction {
    Assign,
    Update,
    Move,
    Remove,
}

/// D2's `allowedActions` for a Queue Entry in `state`, per the design
/// spec: `update_queue_entry` (policy, preference) is legal only on
/// `queued`; `move_queue_entry` is legal on any open entry; `assign` and
/// `remove` are only ever offered on `queued` (an `assigned` entry's Job
/// owns release/cancel).
pub fn allowed_actions(state: QueueEntryState) -> Vec<QueueEntryAction> {
    match state {
        QueueEntryState::Queued => vec![
            QueueEntryAction::Assign,
            QueueEntryAction::Update,
            QueueEntryAction::Move,
            QueueEntryAction::Remove,
        ],
        QueueEntryState::Assigned => vec![QueueEntryAction::Move],
        QueueEntryState::Closed => vec![],
    }
}

/// A Queue Entry as the wire shares it (spec "Backend model" wire types).
/// Assembled by [`repository`] from the `queue_entries` row plus its
/// linked Slice Revision's display data and lineage's `copyCount`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/QueueEntry.ts")]
pub struct QueueEntry {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub slice_revision_id: String,
    pub lineage_id: String,
    #[ts(type = "number")]
    pub copy_index: i64,
    #[ts(type = "number")]
    pub copy_count: i64,
    pub origin_entry_id: Option<String>,
    pub origin_kind: Option<OriginKind>,
    pub state: QueueEntryState,
    pub close_reason: Option<CloseReason>,
    #[ts(type = "number | null")]
    pub position: Option<i64>,
    pub policy: DispatchPolicy,
    pub preference: DispatchPreference,
    pub estimate: MaterialEstimate,
    pub manual_printer_id: Option<String>,
    pub job_id: Option<String>,
    pub requires_manual_printer_selection: bool,
    pub allowed_actions: Vec<QueueEntryAction>,
    pub display: QueueEntryDisplay,
    pub created_at: String,
    pub updated_at: String,
    pub closed_at: Option<String>,
}

/// D5: why a Printer failed a gate for a Queue Entry, or (start blockers
/// only, D7/D8) why an `awaitingStart` Job's Printer can't start yet.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(rename_all = "SCREAMING_SNAKE_CASE", export_to = "domain/BlockerCode.ts")]
pub enum BlockerCode {
    PrinterArchived,
    SetupIncomplete,
    ConnectionError,
    PrinterOffline,
    JobActive,
    HostOperationPending,
    PrinterBusyExternal,
    /// Automatic only (D5 gate 1).
    PrinterNotIdle,
    PinnedToOtherPrinter,
    ProfileMismatch,
    NeedsManualPrinter,
    CapabilityUnsupported,
    /// Automatic only (D5 gate 3).
    AdapterNotProven,
    NoCompatibleSpool,
    InsufficientMaterial,
    /// Automatic only (D5 gate 4).
    SpoolNotLoaded,
    /// Start blockers only (D7/D8), never produced by `eligibility::evaluate`
    /// or `eligibility::check_assignment`.
    PrinterNotReady,
}

/// D5: a gate failure blocking one or more Printers for a Queue Entry.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Blocker.ts")]
pub struct Blocker {
    pub code: BlockerCode,
    pub message: String,
    pub detail: Option<String>,
    pub recovery: Option<RecoveryCode>,
    pub printer_ids: Vec<String>,
}

/// D5: one Spool an operator (or the ranking) may choose for a Candidate
/// Printer.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SpoolOption.ts")]
pub struct SpoolOption {
    pub spool_id: String,
    #[ts(type = "number")]
    pub spool_number: i64,
    pub loaded_on_printer: bool,
    #[ts(type = "number")]
    pub available_mg: i64,
}

/// D5: a Printer that passed every gate for a Queue Entry, ranked.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Candidate.ts")]
pub struct Candidate {
    pub printer_id: String,
    pub printer_name: String,
    /// 1-based.
    #[ts(type = "number")]
    pub rank: i64,
    pub spool: SpoolOption,
    pub spool_options: Vec<SpoolOption>,
    pub loaded_match: bool,
    pub last_used_at: Option<String>,
    pub manual_facts_acknowledgement_required: bool,
}

/// D5: one Printer's eligibility for a Queue Entry — the gate it failed,
/// when it failed one.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterEligibility.ts")]
pub struct PrinterEligibility {
    pub printer_id: String,
    pub printer_name: String,
    pub eligible: bool,
    pub blockers: Vec<Blocker>,
}

/// D5: an open `queued` entry's overall standing. `assigned` entries have
/// no verdict.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/EligibilityVerdict.ts")]
pub enum EligibilityVerdict {
    Blocked,
    AwaitingOperator,
    Ready,
}

/// D5: `eligibility::evaluate`'s full result for one Queue Entry —
/// ranked candidates, every considered Printer's own standing, and the
/// aggregated blockers.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/QueueEntryEligibility.ts")]
pub struct QueueEntryEligibility {
    pub entry_id: String,
    pub verdict: EligibilityVerdict,
    pub candidates: Vec<Candidate>,
    pub printers: Vec<PrinterEligibility>,
    pub blockers: Vec<Blocker>,
    pub evaluated_at: String,
}

/// Ruling R3: the small per-entry projection `list_queue` and
/// `QueueSnapshot` carry instead of the full [`QueueEntryEligibility`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/EligibilitySummary.ts")]
pub struct EligibilitySummary {
    pub entry_id: String,
    pub verdict: EligibilityVerdict,
    #[ts(type = "number")]
    pub eligible_count: i64,
    pub top_blocker: Option<Blocker>,
    pub candidate_printer_ids: Vec<String>,
}
