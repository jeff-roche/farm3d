//! Spec "Events": the `queue` stream.
//!
//! Four event types on the shared `farm3d-event-v1` channel, in the usual
//! `EventEnvelope`, on this stream's own id and sequence (as
//! `host_ops/events.rs` does): `queue.entry.changed`, `queue.job.changed`,
//! `queue.requirement.changed`, and `queue.eligibility.changed` (the
//! automatic evaluator's, `queue::evaluator`). The payload is a tagged union, as
//! `InventoryEventPayload` is.
//!
//! [`QueueStream::publish`] emits one event per changed row — entries
//! first, then Jobs, then requirements — only after the change committed,
//! and never for a replay, then sends the changed ids in-process
//! ([`QueueStream::subscribe_changes`], P8's Attention wake). Rows carry no credential (no Queue or Job
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

/// P8 D2 "Wakes": the ids one committed [`QueueStream::publish`] changed,
/// sent in-process after the Tauri emits so the Attention projector can
/// run a pass. Never serialized.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct QueueChangeIds {
    pub entry_ids: Vec<String>,
    pub job_ids: Vec<String>,
    pub requirement_ids: Vec<String>,
}

/// The in-process queue broadcast's capacity (P8 D2 "Wakes"). A
/// subscriber that falls further behind sees `Lagged`, which means "run a
/// full pass".
pub const QUEUE_CHANGE_CAPACITY: usize = 256;

/// The stream's identity and sequence. One per process, held in
/// `RuntimeServices`.
pub struct QueueStream {
    stream_id: String,
    sequence: AtomicU64,
    /// Held while rows are numbered and emitted, so events reach the
    /// channel in sequence order and one change's events stay together.
    emit: Mutex<()>,
    /// P8 D2: every publish's ids, in-process. All three P7 publish paths
    /// (`queue::commands::publish_rows`, `jobs::services::publish`, and
    /// the evaluator's assignments through `publish_rows`) go through
    /// [`QueueStream::publish`], so this covers them all.
    changes: tokio::sync::broadcast::Sender<QueueChangeIds>,
}

impl Default for QueueStream {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
            changes: tokio::sync::broadcast::channel(QUEUE_CHANGE_CAPACITY).0,
        }
    }
}

impl QueueStream {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// P8 D2 "Wakes": a receiver of the ids of every change published from
    /// now on (sent after its Tauri emits).
    pub fn subscribe_changes(&self) -> tokio::sync::broadcast::Receiver<QueueChangeIds> {
        self.changes.subscribe()
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
        let ids = QueueChangeIds {
            entry_ids: change
                .entries
                .iter()
                .map(|entry| entry.id.clone())
                .collect(),
            job_ids: change.jobs.iter().map(|job| job.id.clone()).collect(),
            requirement_ids: change
                .requirements
                .iter()
                .map(|requirement| requirement.id.clone())
                .collect(),
        };
        if !(ids.entry_ids.is_empty() && ids.job_ids.is_empty() && ids.requirement_ids.is_empty()) {
            // An error only means nobody is subscribed.
            let _ = self.changes.send(ids);
        }
    }

    /// D6: the evaluator's `queue.eligibility.changed`, with the full set
    /// of summaries and the run's `nextAutomaticAction`. The evaluator
    /// calls it after a run whose conclusion changed.
    pub fn publish_eligibility<R: tauri::Runtime>(
        &self,
        app: &AppHandle<R>,
        summaries: &[EligibilitySummary],
        next_automatic_action: &NextAutomaticAction,
    ) {
        let _ordered = self.lock();
        let _ = app.emit(
            STATUS_EVENT,
            self.envelope(
                QueueEventType::EligibilityChanged,
                "queue",
                "eligibility",
                QueueEventPayload::EligibilityChanged {
                    summaries: summaries.to_vec(),
                    next_automatic_action: next_automatic_action.clone(),
                },
            ),
        );
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

    fn a_requirement(id: &str) -> ReconciliationRequirement {
        ReconciliationRequirement {
            id: id.to_string(),
            job_id: "job-1".to_string(),
            kind: crate::jobs::RequirementKind::MaterialReconciliation,
            status: crate::jobs::RequirementStatus::Pending,
            spool_id: None,
            reservation_id: None,
            opened_at: "2026-09-28T00:00:00Z".to_string(),
            deferred_at: None,
            resolved_at: None,
            resolution: None,
        }
    }

    /// P8 D2 "Wakes": every publish also goes out in-process, as the ids
    /// it changed, to every subscriber (the Attention projector).
    #[test]
    fn publish_broadcasts_the_changed_ids_in_process() {
        let app = tauri::test::mock_app();
        let stream = QueueStream::default();
        let mut changes = stream.subscribe_changes();
        stream.publish(
            app.handle(),
            &QueueChange {
                requirements: vec![a_requirement("rrq-1"), a_requirement("rrq-2")],
                ..QueueChange::default()
            },
        );
        assert_eq!(
            changes.try_recv().unwrap(),
            QueueChangeIds {
                entry_ids: vec![],
                job_ids: vec![],
                requirement_ids: vec!["rrq-1".to_string(), "rrq-2".to_string()],
            }
        );
        // An empty change changed nothing: no wake.
        stream.publish(app.handle(), &QueueChange::default());
        assert!(changes.try_recv().is_err());
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
