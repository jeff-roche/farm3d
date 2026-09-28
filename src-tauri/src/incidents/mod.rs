//! P8 D1/D2: the Incident domain's pure wire types — the durable record
//! that groups a `printer.hostFailed` or a Job's fatal/cancelled Events
//! and evidence under one timeline. See the P8 design spec's D1
//! ("Vocabulary and ownership") and "Backend model" module layout table.
//!
//! This module (Task 3) is wire types only: the repository (open, link,
//! reopen, `append_entry`, `close_if_settled`), the lifecycle-blocker and
//! import guards, and commands are later tasks (see the module layout
//! table in the design spec).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::attention::lifecycle::AckBy;
use crate::attention::{AttentionEvent, AttentionResolution};
use crate::cameras::{CameraErrorKind, CameraSnapshot, EvidenceSkipReason, PruneReason, SnapshotTrigger};
use crate::jobs::{JobEvent, PrinterSnapshot};

/// D1: an Incident's own open/closed lifecycle, independent of its linked
/// Events'.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/IncidentState.ts")]
pub enum IncidentState {
    Open,
    Closed,
}

impl IncidentState {
    pub const ALL: [IncidentState; 2] = [IncidentState::Open, IncidentState::Closed];
}

/// D2 "Incident rule": the four Conditions that can open (or, for
/// `printer.hostFailed`, never link to) an Incident.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[ts(export_to = "domain/IncidentKind.ts")]
pub enum IncidentKind {
    #[serde(rename = "printer.hostFailed")]
    #[ts(rename = "printer.hostFailed")]
    PrinterHostFailed,
    #[serde(rename = "job.failed")]
    #[ts(rename = "job.failed")]
    JobFailed,
    #[serde(rename = "job.hostCancelled")]
    #[ts(rename = "job.hostCancelled")]
    JobHostCancelled,
    #[serde(rename = "requirement.jobOutcomeUnknown")]
    #[ts(rename = "requirement.jobOutcomeUnknown")]
    RequirementJobOutcomeUnknown,
}

impl IncidentKind {
    pub const ALL: [IncidentKind; 4] = [
        IncidentKind::PrinterHostFailed,
        IncidentKind::JobFailed,
        IncidentKind::JobHostCancelled,
        IncidentKind::RequirementJobOutcomeUnknown,
    ];
}

/// `incidents` in full (spec "Backend model" wire types). `job_id` is
/// `None` exactly for `printer.hostFailed` (the 0009 migration's CHECK).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Incident.ts")]
pub struct Incident {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub kind: IncidentKind,
    pub state: IncidentState,
    pub printer_id: String,
    pub job_id: Option<String>,
    /// What the Printer looked like when the Incident opened (P7's
    /// `PrinterSnapshot`).
    pub printer_snapshot: PrinterSnapshot,
    pub opened_at: String,
    pub closed_at: Option<String>,
    pub linked_event_ids: Vec<String>,
    #[ts(type = "number")]
    pub open_linked_event_count: i64,
    #[ts(type = "number")]
    pub snapshot_count: i64,
}

/// D9 decision 9: the Incident timeline's own kinds. `job_events` rows
/// merge in at read time ([`IncidentTimelineItem`]) — there is no
/// `operatorAction` kind here.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/IncidentEntryKind.ts")]
pub enum IncidentEntryKind {
    Opened,
    EventLinked,
    Reopened,
    EventAcknowledged,
    EventResolved,
    EvidenceCaptured,
    EvidenceSkipped,
    EvidencePruned,
    EvidencePinned,
    EvidenceUnpinned,
    NoteAdded,
    Closed,
}

impl IncidentEntryKind {
    pub const ALL: [IncidentEntryKind; 12] = [
        IncidentEntryKind::Opened,
        IncidentEntryKind::EventLinked,
        IncidentEntryKind::Reopened,
        IncidentEntryKind::EventAcknowledged,
        IncidentEntryKind::EventResolved,
        IncidentEntryKind::EvidenceCaptured,
        IncidentEntryKind::EvidenceSkipped,
        IncidentEntryKind::EvidencePruned,
        IncidentEntryKind::EvidencePinned,
        IncidentEntryKind::EvidenceUnpinned,
        IncidentEntryKind::NoteAdded,
        IncidentEntryKind::Closed,
    ];
}

/// `incident_events.detail_json`, tagged by [`IncidentEntryKind`].
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/IncidentEntryDetail.ts"
)]
pub enum IncidentEntryDetail {
    Opened {
        event_id: String,
    },
    EventLinked {
        event_id: String,
    },
    Reopened {
        event_id: String,
    },
    EventAcknowledged {
        event_id: String,
        by: AckBy,
    },
    EventResolved {
        event_id: String,
        resolution: AttentionResolution,
    },
    EvidenceCaptured {
        snapshot_id: String,
        trigger: SnapshotTrigger,
    },
    EvidenceSkipped {
        reason: EvidenceSkipReason,
        error_kind: Option<CameraErrorKind>,
    },
    EvidencePruned {
        snapshot_id: String,
        reason: PruneReason,
    },
    EvidencePinned {
        snapshot_id: String,
    },
    EvidenceUnpinned {
        snapshot_id: String,
    },
    NoteAdded {
        text: String,
    },
    Closed,
}

/// `incident_events` in full.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/IncidentEntry.ts")]
pub struct IncidentEntry {
    pub id: String,
    pub incident_id: String,
    #[ts(type = "number")]
    pub sequence: i64,
    pub kind: IncidentEntryKind,
    pub detail: IncidentEntryDetail,
    pub operation_id: Option<String>,
    pub at: String,
}

/// D9 decision 9: the Incident timeline merges `incident_events` with
/// the Job's own `job_events` at read time.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "source", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "source",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/IncidentTimelineItem.ts"
)]
pub enum IncidentTimelineItem {
    Incident { entry: IncidentEntry },
    Job { event: JobEvent },
}

/// `get_incident`/`add_incident_note`'s result.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/IncidentDetail.ts")]
pub struct IncidentDetail {
    pub incident: Incident,
    pub timeline: Vec<IncidentTimelineItem>,
    pub events: Vec<AttentionEvent>,
    pub snapshots: Vec<CameraSnapshot>,
}

/// `list_incidents`' result, `openedAt` descending then id.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/IncidentPage.ts")]
pub struct IncidentPage {
    pub incidents: Vec<Incident>,
    pub next_cursor: Option<String>,
}
