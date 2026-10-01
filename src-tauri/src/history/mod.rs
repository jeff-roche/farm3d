//! P9 D11: the Job history read model. Two read-only commands over the P7
//! and P8 tables: `list_job_history` (a filtered, keyset-paged search of
//! settled Jobs) and `get_job_timeline` (every record linked to one Job,
//! in a deterministic order). It writes nothing.
//!
//! [`repository`] holds the query builder, the cursor codec, and the
//! timeline joins; [`commands`] the two Tauri commands.

pub mod commands;
pub mod repository;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::attention::AttentionEvent;
use crate::cameras::CameraSnapshot;
use crate::host_ops::HostOperation;
use crate::incidents::{Incident, IncidentEntry};
use crate::jobs::{
    CancelReason, Job, JobEvent, PrinterSnapshot, ReconciliationRequirement, Settlement,
};
use crate::queue::QueueEntry;
use crate::slicing::{
    SliceEstimates, SliceFacts, SlicePlateRef, SliceRevisionKind, SliceRevisionTarget,
    SliceRuntimeInfo,
};
use crate::spools::ledger::AmountEvent;
use crate::spools::reservations::Reservation;

/// The states history shows: the settled Job states, and `outcomeUnknown`
/// (shown only when asked for).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobHistoryState.ts")]
pub enum JobHistoryState {
    Completed,
    Failed,
    Cancelled,
    OutcomeUnknown,
}

impl JobHistoryState {
    pub const ALL: [JobHistoryState; 4] = [
        JobHistoryState::Completed,
        JobHistoryState::Failed,
        JobHistoryState::Cancelled,
        JobHistoryState::OutcomeUnknown,
    ];

    /// What `list_job_history` shows when `states` is absent.
    pub const DEFAULT: [JobHistoryState; 3] = [
        JobHistoryState::Completed,
        JobHistoryState::Failed,
        JobHistoryState::Cancelled,
    ];

    /// The `jobs.state` text.
    pub fn as_sql(self) -> &'static str {
        match self {
            JobHistoryState::Completed => "completed",
            JobHistoryState::Failed => "failed",
            JobHistoryState::Cancelled => "cancelled",
            JobHistoryState::OutcomeUnknown => "outcomeUnknown",
        }
    }
}

/// Whether the Job's Printer is archived now.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/PrinterLifecycleFilter.ts"
)]
pub enum PrinterLifecycleFilter {
    #[default]
    Any,
    Active,
    Archived,
}

/// `list_job_history`'s filters (D11). Every field is optional.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobHistoryQuery.ts")]
pub struct JobHistoryQuery {
    /// 1 to 4 distinct states; default completed, failed, cancelled.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub states: Option<Vec<JobHistoryState>>,
    /// Defaults to no Printer-lifecycle filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub printer_lifecycle: Option<PrinterLifecycleFilter>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub printer_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub spool_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model_id: Option<String>,
    /// RFC 3339, inclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ended_after: Option<String>,
    /// RFC 3339, exclusive.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub ended_before: Option<String>,
    /// Trimmed, at most 200 characters; blank means absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub text: Option<String>,
    /// The opaque `nextCursor` of the previous page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub after: Option<String>,
    /// 1 to 200, default 50.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub limit: Option<i64>,
}

/// One history row: the Job's settled facts, its Printer snapshot name,
/// and the current links (Printer archived flag, Model name).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobHistoryRow.ts")]
pub struct JobHistoryRow {
    pub job_id: String,
    pub state: JobHistoryState,
    pub cancel_reason: Option<CancelReason>,
    /// `COALESCE(endedAt, createdAt)`, the paging key.
    pub history_at: String,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    pub printer_id: String,
    /// The Printer's name when the Job was assigned.
    pub printer_snapshot_name: String,
    /// Current.
    pub printer_archived: bool,
    pub spool_id: String,
    #[ts(type = "number")]
    pub spool_number: i64,
    pub model_id: String,
    /// Current.
    pub model_name: String,
    pub slice_revision_id: String,
    pub plate_name: Option<String>,
    pub settlement: Settlement,
    pub incident_id: Option<String>,
    /// Snapshots carrying this Job's id, pruned included.
    #[ts(type = "number")]
    pub snapshot_count: i64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobHistoryPage.ts")]
pub struct JobHistoryPage {
    pub rows: Vec<JobHistoryRow>,
    /// Present exactly when another row follows this page.
    pub next_cursor: Option<String>,
}

/// The Slice Revision's immutable facts, without its current-label fields.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/JobTimelineSliceRevision.ts"
)]
pub struct JobTimelineSliceRevision {
    pub id: String,
    pub kind: SliceRevisionKind,
    pub model_id: String,
    pub source_revision_id: String,
    pub plate: Option<SlicePlateRef>,
    pub target: Option<SliceRevisionTarget>,
    pub runtime: Option<SliceRuntimeInfo>,
    pub estimates: Option<SliceEstimates>,
    pub facts: SliceFacts,
    pub created_at: String,
}

/// One timeline entry, tagged by its source. `at` is the record's own
/// time: an event's `at`, a Host Operation's or Reservation's `createdAt`,
/// an amount event's `occurredAt`, a requirement's `openedAt`, an
/// Attention Event's `firstObservedAt`, an Incident entry's `at`, a
/// snapshot's `capturedAt`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/JobTimelineItem.ts"
)]
pub enum JobTimelineItem {
    Job {
        at: String,
        event: JobEvent,
    },
    HostOperation {
        at: String,
        host_operation: HostOperation,
    },
    Reservation {
        at: String,
        reservation: Reservation,
    },
    AmountEvent {
        at: String,
        amount_event: AmountEvent,
        is_correction: bool,
    },
    Requirement {
        at: String,
        requirement: ReconciliationRequirement,
    },
    Attention {
        at: String,
        event: AttentionEvent,
    },
    Incident {
        at: String,
        entry: IncidentEntry,
    },
    Snapshot {
        at: String,
        snapshot: CameraSnapshot,
    },
}

impl JobTimelineItem {
    pub fn at(&self) -> &str {
        match self {
            JobTimelineItem::Job { at, .. }
            | JobTimelineItem::HostOperation { at, .. }
            | JobTimelineItem::Reservation { at, .. }
            | JobTimelineItem::AmountEvent { at, .. }
            | JobTimelineItem::Requirement { at, .. }
            | JobTimelineItem::Attention { at, .. }
            | JobTimelineItem::Incident { at, .. }
            | JobTimelineItem::Snapshot { at, .. } => at,
        }
    }
}

/// `get_job_timeline`: an immutable view of a settled Job. It reads no
/// current mutable state other than a snapshot's pruned fields.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/JobTimeline.ts")]
pub struct JobTimeline {
    pub job: Job,
    pub entry: QueueEntry,
    pub lineage: Vec<QueueEntry>,
    pub printer_snapshot: PrinterSnapshot,
    pub slice_revision: JobTimelineSliceRevision,
    pub spool_id: String,
    #[ts(type = "number")]
    pub spool_number: i64,
    pub incident: Option<Incident>,
    pub items: Vec<JobTimelineItem>,
}
