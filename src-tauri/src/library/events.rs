//! D17: the Library event stream.
//!
//! Library events go out on the shared `farm3d-event-v1` channel
//! ([`STATUS_EVENT`]) in the usual [`EventEnvelope`], on their own stream:
//! [`LibraryStream`] owns a per-process `streamId` and a sequence that
//! `list_library` reports as `snapshotSequence`. The frontend's Library
//! store keeps only `library.*` types.
//!
//! Every event consumes a sequence number, the ephemeral ones
//! (`library.selection.dropped`, `library.import.progress`) included, so
//! gap detection still works. Record-bearing events go out after commit
//! only, never inside a transaction and never for a replay. Payloads carry
//! ids and basenames, never a full path (D6); `ModelRecord.link.path` in
//! `library.model.changed` is the one exception.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::connections::supervisor::STATUS_EVENT;
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::contracts::ContractVersion;

use super::selection::{ImportProgress, ImportSelectionSummary};
use super::{ModelRecord, ModelSourceRevisionSummary, ProjectRecord};

/// D17's seven Library event types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "domain/LibraryEventType.ts")]
pub enum LibraryEventType {
    #[serde(rename = "library.project.changed")]
    #[ts(rename = "library.project.changed")]
    ProjectChanged,
    #[serde(rename = "library.project.removed")]
    #[ts(rename = "library.project.removed")]
    ProjectRemoved,
    #[serde(rename = "library.model.changed")]
    #[ts(rename = "library.model.changed")]
    ModelChanged,
    #[serde(rename = "library.model.removed")]
    #[ts(rename = "library.model.removed")]
    ModelRemoved,
    #[serde(rename = "library.revision.created")]
    #[ts(rename = "library.revision.created")]
    RevisionCreated,
    #[serde(rename = "library.selection.dropped")]
    #[ts(rename = "library.selection.dropped")]
    SelectionDropped,
    #[serde(rename = "library.import.progress")]
    #[ts(rename = "library.import.progress")]
    ImportProgress,
}

/// A Library event's payload. Untagged: the envelope's `type` says which
/// shape it is, and the payload is exactly the record D17 names. Variant
/// order matters when deserializing: each record's required fields rule out
/// the ones before it, and the empty `Removed {}` matches every payload, so
/// it stays last.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(untagged)]
#[ts(untagged, export_to = "domain/LibraryEventPayload.ts")]
pub enum LibraryEventPayload {
    ProjectChanged(ProjectRecord),
    ModelChanged(Box<ModelRecord>),
    RevisionCreated(Box<ModelSourceRevisionSummary>),
    SelectionDropped(ImportSelectionSummary),
    ImportProgress(ImportProgress),
    /// `library.project.removed` and `library.model.removed`: `{}`.
    Removed {},
}

/// The exact envelope emitted on [`STATUS_EVENT`] for the Library stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/LibraryEvent.ts")]
pub struct LibraryEvent(pub EventEnvelope<LibraryEventType, LibraryEventPayload>);

/// The Library stream's identity and sequence. One per process, held in
/// `LibraryServices`.
pub struct LibraryStream {
    stream_id: String,
    sequence: AtomicU64,
    /// Held while a batch is numbered and emitted, so events reach the
    /// channel in sequence order and one operation's events stay together.
    emit: Mutex<()>,
}

impl Default for LibraryStream {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
        }
    }
}

impl LibraryStream {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The last sequence handed out. `list_library` reads this before it
    /// reads rows, so a change the snapshot misses always carries a larger
    /// sequence.
    pub fn snapshot_sequence(&self) -> JsSafeInteger {
        JsSafeInteger::try_from(self.sequence.load(Ordering::SeqCst))
            .expect("library sequence is JS-safe")
    }

    /// Numbers and emits `events` in order, as one uninterrupted batch.
    /// Call only after the write they describe has committed.
    pub fn publish<R: tauri::Runtime>(
        &self,
        app: &AppHandle<R>,
        events: Vec<(LibraryEventType, EventSubject, LibraryEventPayload)>,
    ) {
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
        event_type: LibraryEventType,
        subject: EventSubject,
        payload: LibraryEventPayload,
    ) -> LibraryEvent {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        LibraryEvent(EventEnvelope {
            contract_version: ContractVersion::V1,
            stream_id: self.stream_id.clone(),
            sequence: JsSafeInteger::try_from(sequence).expect("library sequence is JS-safe"),
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            event_type,
            subject,
            payload,
        })
    }
}

/// An event subject such as `selection/<id>` or `model/<id>`.
pub fn subject(kind: &str, id: &str) -> EventSubject {
    EventSubject {
        kind: kind.to_string(),
        id: id.to_string(),
    }
}

/// `library.selection.dropped`: a window drop registered a selection.
pub fn publish_selection_dropped<R: tauri::Runtime>(
    app: &AppHandle<R>,
    stream: &LibraryStream,
    summary: &ImportSelectionSummary,
) {
    stream.publish(
        app,
        vec![(
            LibraryEventType::SelectionDropped,
            subject("selection", &summary.selection_id),
            LibraryEventPayload::SelectionDropped(summary.clone()),
        )],
    );
}

/// `library.import.progress` for one file of a selection being staged.
pub fn publish_import_progress<R: tauri::Runtime>(
    app: &AppHandle<R>,
    stream: &LibraryStream,
    selection_id: &str,
    progress: ImportProgress,
) {
    stream.publish(
        app,
        vec![(
            LibraryEventType::ImportProgress,
            subject("selection", selection_id),
            LibraryEventPayload::ImportProgress(progress),
        )],
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_use_the_library_prefix_spellings() {
        let spellings = serde_json::to_value([
            LibraryEventType::ProjectChanged,
            LibraryEventType::ProjectRemoved,
            LibraryEventType::ModelChanged,
            LibraryEventType::ModelRemoved,
            LibraryEventType::RevisionCreated,
            LibraryEventType::SelectionDropped,
            LibraryEventType::ImportProgress,
        ])
        .unwrap();
        assert_eq!(
            spellings,
            serde_json::json!([
                "library.project.changed",
                "library.project.removed",
                "library.model.changed",
                "library.model.removed",
                "library.revision.created",
                "library.selection.dropped",
                "library.import.progress",
            ])
        );
    }

    #[test]
    fn payloads_serialize_as_the_bare_record() {
        let progress = LibraryEventPayload::ImportProgress(ImportProgress {
            file_index: 1,
            bytes_done: 2,
            bytes_total: 3,
        });
        assert_eq!(
            serde_json::to_value(progress).unwrap(),
            serde_json::json!({ "fileIndex": 1, "bytesDone": 2, "bytesTotal": 3 })
        );
        assert_eq!(
            serde_json::to_value(LibraryEventPayload::Removed {}).unwrap(),
            serde_json::json!({})
        );
    }

    #[test]
    fn every_event_takes_the_next_sequence_on_one_stream() {
        let stream = LibraryStream::default();
        assert_eq!(stream.snapshot_sequence().get(), 0);
        let first = stream.envelope(
            LibraryEventType::ProjectRemoved,
            subject("project", "prj-1"),
            LibraryEventPayload::Removed {},
        );
        let second = stream.envelope(
            LibraryEventType::ModelRemoved,
            subject("model", "mdl-1"),
            LibraryEventPayload::Removed {},
        );
        assert_eq!(first.0.sequence.get(), 1);
        assert_eq!(second.0.sequence.get(), 2);
        assert_eq!(first.0.stream_id, stream.stream_id());
        assert_eq!(stream.snapshot_sequence().get(), 2);
    }
}
