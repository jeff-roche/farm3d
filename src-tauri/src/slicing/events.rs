//! D17: the `slicing` event stream.
//!
//! Slicing events go out on the shared `farm3d-event-v1` channel
//! ([`STATUS_EVENT`]) in the usual [`EventEnvelope`], on their own stream:
//! [`SlicingStream`] owns a per-process `streamId` and a sequence that
//! `list_slicing` reports as `snapshotSequence`. Every type starts with
//! `slicing.`, so the Library and Printer listeners ignore them.
//!
//! Every event consumes a sequence number, as the Library stream's do,
//! the ephemeral `slicing.operation.progress` included: it is never part of
//! a snapshot, but gap detection still works. Progress is throttled to one
//! event per operation per [`PROGRESS_INTERVAL`] ([`ProgressThrottle`]).
//! Record-bearing events go out after commit only, never inside a
//! transaction and never for a replay. Payloads carry ids and basenames,
//! never a full path, except `SlicerRuntimeStatus`'s paths, which only the
//! Slicer settings show (D2).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::connections::supervisor::STATUS_EVENT;
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::contracts::ContractVersion;

use super::process::SliceProgress;
use super::{PreparationRecord, SliceOperationRecord, SliceRevisionSummary, SlicerRuntimeStatus};

/// D9: at most one `slicing.operation.progress` per operation this often.
pub const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// D17's seven slicing event types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "domain/SlicingEventType.ts")]
pub enum SlicingEventType {
    #[serde(rename = "slicing.runtime.changed")]
    #[ts(rename = "slicing.runtime.changed")]
    RuntimeChanged,
    #[serde(rename = "slicing.preparation.changed")]
    #[ts(rename = "slicing.preparation.changed")]
    PreparationChanged,
    #[serde(rename = "slicing.preparation.removed")]
    #[ts(rename = "slicing.preparation.removed")]
    PreparationRemoved,
    #[serde(rename = "slicing.operation.changed")]
    #[ts(rename = "slicing.operation.changed")]
    OperationChanged,
    #[serde(rename = "slicing.operation.progress")]
    #[ts(rename = "slicing.operation.progress")]
    OperationProgress,
    #[serde(rename = "slicing.revision.created")]
    #[ts(rename = "slicing.revision.created")]
    RevisionCreated,
    #[serde(rename = "slicing.revision.removed")]
    #[ts(rename = "slicing.revision.removed")]
    RevisionRemoved,
}

/// D17: `slicing.operation.progress`'s payload. `message` is always
/// present, since the percentage sits at 1% for most of the load phase
/// (Gate E); off Linux it is "Slicing…" with no percentages.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename = "SliceProgress",
    rename_all = "camelCase",
    export_to = "domain/SliceProgress.ts"
)]
pub struct SliceProgressPayload {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub total_percent: Option<u8>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plate_percent: Option<u8>,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub warning: Option<String>,
}

impl From<&SliceProgress> for SliceProgressPayload {
    fn from(update: &SliceProgress) -> Self {
        Self {
            total_percent: update.total_percent,
            plate_percent: update.plate_percent,
            message: update.message.clone(),
            warning: update.warning.clone(),
        }
    }
}

/// A slicing event's payload. Untagged: the envelope's `type` says which
/// shape it is. Variant order matters when deserializing: each record's
/// required fields rule out the ones before it, and the empty `Removed {}`
/// matches every payload, so it stays last.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(untagged, export_to = "domain/SlicingEventPayload.ts")]
pub enum SlicingEventPayload {
    RuntimeChanged(Box<SlicerRuntimeStatus>),
    OperationChanged(Box<SliceOperationRecord>),
    PreparationChanged(Box<PreparationRecord>),
    RevisionCreated(Box<SliceRevisionSummary>),
    OperationProgress(SliceProgressPayload),
    /// `slicing.preparation.removed` and `slicing.revision.removed`: `{}`.
    Removed {},
}

/// The exact envelope emitted on [`STATUS_EVENT`] for the slicing stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/SlicingEvent.ts")]
pub struct SlicingEvent(pub EventEnvelope<SlicingEventType, SlicingEventPayload>);

/// One event to publish: its type, subject, and payload.
pub type SlicingEventSpec = (SlicingEventType, EventSubject, SlicingEventPayload);

/// The slicing stream's identity and sequence. One per process, held in
/// `SlicingServices`.
pub struct SlicingStream {
    stream_id: String,
    sequence: AtomicU64,
    /// Held while a batch is numbered and emitted, so events reach the
    /// channel in sequence order and one change's events stay together.
    emit: Mutex<()>,
}

impl Default for SlicingStream {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
        }
    }
}

impl SlicingStream {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The last sequence handed out. `list_slicing` reads this before it
    /// reads rows, so a change the snapshot misses always carries a larger
    /// sequence.
    pub fn snapshot_sequence(&self) -> JsSafeInteger {
        JsSafeInteger::try_from(self.sequence.load(Ordering::SeqCst))
            .expect("slicing sequence is JS-safe")
    }

    /// Numbers and emits `events` in order, as one uninterrupted batch.
    /// Call only after the write they describe has committed.
    pub fn publish<R: tauri::Runtime>(&self, app: &AppHandle<R>, events: Vec<SlicingEventSpec>) {
        if events.is_empty() {
            return;
        }
        let _ordered = self
            .emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for (event_type, subject, payload) in events {
            let _ = app.emit(STATUS_EVENT, self.envelope(event_type, subject, payload));
        }
    }

    fn envelope(
        &self,
        event_type: SlicingEventType,
        subject: EventSubject,
        payload: SlicingEventPayload,
    ) -> SlicingEvent {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        SlicingEvent(EventEnvelope {
            contract_version: ContractVersion::V1,
            stream_id: self.stream_id.clone(),
            sequence: JsSafeInteger::try_from(sequence).expect("slicing sequence is JS-safe"),
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            event_type,
            subject,
            payload,
        })
    }
}

fn subject(kind: &str, id: &str) -> EventSubject {
    EventSubject {
        kind: kind.to_string(),
        id: id.to_string(),
    }
}

/// The runtime's subject: there is one runtime per machine.
pub const RUNTIME_SUBJECT_ID: &str = "local";

/// `slicing.runtime.changed`.
pub fn runtime_changed(status: &SlicerRuntimeStatus) -> SlicingEventSpec {
    (
        SlicingEventType::RuntimeChanged,
        subject("runtime", RUNTIME_SUBJECT_ID),
        SlicingEventPayload::RuntimeChanged(Box::new(status.clone())),
    )
}

/// `slicing.preparation.changed`.
pub fn preparation_changed(preparation: &PreparationRecord) -> SlicingEventSpec {
    (
        SlicingEventType::PreparationChanged,
        subject("preparation", &preparation.id),
        SlicingEventPayload::PreparationChanged(Box::new(preparation.clone())),
    )
}

/// `slicing.preparation.removed`.
pub fn preparation_removed(id: &str) -> SlicingEventSpec {
    (
        SlicingEventType::PreparationRemoved,
        subject("preparation", id),
        SlicingEventPayload::Removed {},
    )
}

/// `slicing.operation.changed`.
pub fn operation_changed(operation: &SliceOperationRecord) -> SlicingEventSpec {
    (
        SlicingEventType::OperationChanged,
        subject("sliceOperation", &operation.id),
        SlicingEventPayload::OperationChanged(Box::new(operation.clone())),
    )
}

/// `slicing.operation.progress`.
pub fn operation_progress(operation_id: &str, progress: SliceProgressPayload) -> SlicingEventSpec {
    (
        SlicingEventType::OperationProgress,
        subject("sliceOperation", operation_id),
        SlicingEventPayload::OperationProgress(progress),
    )
}

/// `slicing.revision.created`.
pub fn revision_created(revision: &SliceRevisionSummary) -> SlicingEventSpec {
    (
        SlicingEventType::RevisionCreated,
        subject("sliceRevision", &revision.id),
        SlicingEventPayload::RevisionCreated(Box::new(revision.clone())),
    )
}

/// `slicing.revision.removed`.
pub fn revision_removed(id: &str) -> SlicingEventSpec {
    (
        SlicingEventType::RevisionRemoved,
        subject("sliceRevision", id),
        SlicingEventPayload::Removed {},
    )
}

/// D9: holds one operation's progress to at most one update per
/// `interval`. An update inside the window is kept, replacing any older
/// held one, and comes out at the next [`Self::due`] after the window
/// passes, or at [`Self::finish`], so the last update is never lost.
#[derive(Debug)]
pub struct ProgressThrottle {
    interval: Duration,
    last_sent: Option<Instant>,
    held: Option<SliceProgressPayload>,
}

impl ProgressThrottle {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last_sent: None,
            held: None,
        }
    }

    /// Offers `update` at `now`. Returns it when it may go out now;
    /// otherwise holds it.
    pub fn offer(
        &mut self,
        update: SliceProgressPayload,
        now: Instant,
    ) -> Option<SliceProgressPayload> {
        self.held = Some(update);
        self.due(now)
    }

    /// The held update, once the window since the last one sent has
    /// passed.
    pub fn due(&mut self, now: Instant) -> Option<SliceProgressPayload> {
        let open = self
            .last_sent
            .is_none_or(|sent| now.duration_since(sent) >= self.interval);
        if !open {
            return None;
        }
        let update = self.held.take()?;
        self.last_sent = Some(now);
        Some(update)
    }

    /// The held update, if any, whatever the window: the run is over.
    pub fn finish(&mut self) -> Option<SliceProgressPayload> {
        self.held.take()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn update(percent: u8) -> SliceProgressPayload {
        SliceProgressPayload {
            total_percent: Some(percent),
            plate_percent: None,
            message: format!("{percent}%"),
            warning: None,
        }
    }

    #[test]
    fn event_types_use_the_slicing_prefix_spellings() {
        let spellings = serde_json::to_value([
            SlicingEventType::RuntimeChanged,
            SlicingEventType::PreparationChanged,
            SlicingEventType::PreparationRemoved,
            SlicingEventType::OperationChanged,
            SlicingEventType::OperationProgress,
            SlicingEventType::RevisionCreated,
            SlicingEventType::RevisionRemoved,
        ])
        .unwrap();
        assert_eq!(
            spellings,
            serde_json::json!([
                "slicing.runtime.changed",
                "slicing.preparation.changed",
                "slicing.preparation.removed",
                "slicing.operation.changed",
                "slicing.operation.progress",
                "slicing.revision.created",
                "slicing.revision.removed",
            ])
        );
    }

    #[test]
    fn progress_payload_serializes_as_d17_names_it() {
        assert_eq!(
            serde_json::to_value(SlicingEventPayload::OperationProgress(update(40))).unwrap(),
            serde_json::json!({ "totalPercent": 40, "message": "40%" })
        );
        assert_eq!(
            serde_json::to_value(SlicingEventPayload::Removed {}).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn every_event_takes_the_next_sequence_on_one_stream() {
        let stream = SlicingStream::default();
        assert_eq!(stream.snapshot_sequence().get(), 0);
        let first = stream.envelope(
            SlicingEventType::RevisionRemoved,
            subject("sliceRevision", "slr-1"),
            SlicingEventPayload::Removed {},
        );
        let second = stream.envelope(
            SlicingEventType::OperationProgress,
            subject("sliceOperation", "sop-1"),
            SlicingEventPayload::OperationProgress(update(1)),
        );
        assert_eq!(first.0.sequence.get(), 1);
        assert_eq!(second.0.sequence.get(), 2);
        assert_eq!(stream.snapshot_sequence().get(), 2);
    }

    #[test]
    fn progress_is_throttled_to_one_per_interval_and_keeps_the_last() {
        let start = Instant::now();
        let at = |ms: u64| start + Duration::from_millis(ms);
        let mut throttle = ProgressThrottle::new(PROGRESS_INTERVAL);
        assert_eq!(throttle.offer(update(1), at(0)), Some(update(1)));
        assert_eq!(throttle.offer(update(2), at(50)), None);
        assert_eq!(throttle.offer(update(3), at(100)), None);
        assert_eq!(throttle.due(at(200)), None, "still inside the window");
        assert_eq!(
            throttle.due(at(250)),
            Some(update(3)),
            "the newest held update goes out once the window passes"
        );
        assert_eq!(throttle.due(at(600)), None, "nothing held");
        assert_eq!(throttle.offer(update(4), at(600)), Some(update(4)));
        assert_eq!(throttle.offer(update(5), at(610)), None);
        assert_eq!(throttle.finish(), Some(update(5)), "the last is never lost");
        assert_eq!(throttle.finish(), None);
    }
}
