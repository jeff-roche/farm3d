//! Spec "Events": the `queue` stream.
//!
//! Four event types on the shared `farm3d-event-v1` channel, in the usual
//! `EventEnvelope`, on this stream's own id and sequence (as
//! `host_ops/events.rs` does): `queue.entry.changed`, `queue.job.changed`,
//! `queue.requirement.changed`, and `queue.eligibility.changed` (the
//! evaluator's, a later task). The payload is a tagged union, as
//! `InventoryEventPayload` is.
//!
//! [`QueueStream::publish`] emits one event per changed row — entries
//! first, then Jobs, then requirements — only after the change committed,
//! and never for a replay. Rows carry no credential (no Queue or Job
//! column holds one, and `printer_snapshot_json` never holds an endpoint
//! or a credential reference), so neither does any event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::connections::supervisor::STATUS_EVENT;
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::contracts::ContractVersion;
use crate::jobs::{Job, ReconciliationRequirement};

use super::{EligibilitySummary, NextAutomaticAction, QueueChange, QueueEntry};

/// The stream's event types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "domain/QueueEventType.ts")]
pub enum QueueEventType {
    #[serde(rename = "queue.entry.changed")]
    #[ts(rename = "queue.entry.changed")]
    EntryChanged,
    #[serde(rename = "queue.job.changed")]
    #[ts(rename = "queue.job.changed")]
    JobChanged,
    #[serde(rename = "queue.requirement.changed")]
    #[ts(rename = "queue.requirement.changed")]
    RequirementChanged,
    #[serde(rename = "queue.eligibility.changed")]
    #[ts(rename = "queue.eligibility.changed")]
    EligibilityChanged,
}

/// The stream's payloads, tagged by `type`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/QueueEventPayload.ts"
)]
pub enum QueueEventPayload {
    EntryChanged {
        entry: Box<QueueEntry>,
    },
    JobChanged {
        job: Box<Job>,
    },
    RequirementChanged {
        requirement: ReconciliationRequirement,
    },
    /// The full set of summaries, not a diff.
    EligibilityChanged {
        summaries: Vec<EligibilitySummary>,
        next_automatic_action: NextAutomaticAction,
    },
}

/// The exact envelope emitted on [`STATUS_EVENT`] for this stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/QueueEvent.ts")]
pub struct QueueEvent(pub EventEnvelope<QueueEventType, QueueEventPayload>);

/// The stream's identity and sequence. One per process, held in
/// `RuntimeServices`.
pub struct QueueStream {
    stream_id: String,
    sequence: AtomicU64,
    /// Held while rows are numbered and emitted, so events reach the
    /// channel in sequence order and one change's events stay together.
    emit: Mutex<()>,
}

impl Default for QueueStream {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
        }
    }
}

impl QueueStream {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The last sequence handed out. `list_queue` reads this before it
    /// reads rows, so a change the snapshot misses always carries a larger
    /// sequence (listen before backfill).
    pub fn snapshot_sequence(&self) -> JsSafeInteger {
        let _ordered = self.lock();
        JsSafeInteger::try_from(self.sequence.load(Ordering::SeqCst))
            .expect("queue sequence is JS-safe")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Emits one event per changed row: entries, then Jobs, then
    /// requirements. Call only after the change committed, and never for
    /// a replay.
    pub fn publish<R: tauri::Runtime>(&self, app: &AppHandle<R>, change: &QueueChange) {
        let _ordered = self.lock();
        for entry in &change.entries {
            let _ = app.emit(
                STATUS_EVENT,
                self.envelope(
                    QueueEventType::EntryChanged,
                    "queueEntry",
                    &entry.id,
                    QueueEventPayload::EntryChanged {
                        entry: Box::new(entry.clone()),
                    },
                ),
            );
        }
        for job in &change.jobs {
            let _ = app.emit(
                STATUS_EVENT,
                self.envelope(
                    QueueEventType::JobChanged,
                    "job",
                    &job.id,
                    QueueEventPayload::JobChanged {
                        job: Box::new(job.clone()),
                    },
                ),
            );
        }
        for requirement in &change.requirements {
            let _ = app.emit(
                STATUS_EVENT,
                self.envelope(
                    QueueEventType::RequirementChanged,
                    "reconciliationRequirement",
                    &requirement.id,
                    QueueEventPayload::RequirementChanged {
                        requirement: requirement.clone(),
                    },
                ),
            );
        }
    }

    fn envelope(
        &self,
        event_type: QueueEventType,
        subject_kind: &str,
        subject_id: &str,
        payload: QueueEventPayload,
    ) -> QueueEvent {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        QueueEvent(EventEnvelope {
            contract_version: ContractVersion::V1,
            stream_id: self.stream_id.clone(),
            sequence: JsSafeInteger::try_from(sequence).expect("queue sequence is JS-safe"),
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            event_type,
            subject: EventSubject {
                kind: subject_kind.to_string(),
                id: subject_id.to_string(),
            },
            payload,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_use_their_dotted_wire_names() {
        assert_eq!(
            serde_json::to_value([
                QueueEventType::EntryChanged,
                QueueEventType::JobChanged,
                QueueEventType::RequirementChanged,
                QueueEventType::EligibilityChanged,
            ])
            .unwrap(),
            serde_json::json!([
                "queue.entry.changed",
                "queue.job.changed",
                "queue.requirement.changed",
                "queue.eligibility.changed"
            ])
        );
    }

    #[test]
    fn the_snapshot_sequence_starts_at_zero() {
        let stream = QueueStream::default();
        assert_eq!(serde_json::to_value(stream.snapshot_sequence()).unwrap(), 0);
        assert!(!stream.stream_id().is_empty());
    }

    #[test]
    fn the_eligibility_payload_is_tagged_and_camel_cased() {
        let payload = QueueEventPayload::EligibilityChanged {
            summaries: Vec::new(),
            next_automatic_action: NextAutomaticAction::EvaluatorNotRunning,
        };
        assert_eq!(
            serde_json::to_value(payload).unwrap(),
            serde_json::json!({
                "type": "eligibilityChanged",
                "summaries": [],
                "nextAutomaticAction": { "kind": "evaluatorNotRunning" }
            })
        );
    }
}
