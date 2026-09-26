//! Spec "Events": the `hostOperations` stream.
//!
//! One type, `hostOperations.operation.changed`, with subject
//! `{ kind: "hostOperation", id }` and the whole `HostOperation` as its
//! payload. It goes out on the shared `farm3d-event-v1` channel in the usual
//! `EventEnvelope`, on its own stream id and sequence (as
//! `slicing/events.rs` does), after every committed change to a row: the
//! write-ahead insert, `mark_sent`, every transition, every recorded
//! attempt, and `no_longer_pending` becoming true. Never inside a
//! transaction, and never for a replay. Rows carry no credential (D2), so
//! neither does any event.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::connections::supervisor::STATUS_EVENT;
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::contracts::ContractVersion;

use super::HostOperation;

/// The stream's one event type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "domain/HostOperationsEventType.ts")]
pub enum HostOperationsEventType {
    #[serde(rename = "hostOperations.operation.changed")]
    #[ts(rename = "hostOperations.operation.changed")]
    OperationChanged,
}

/// The exact envelope emitted on [`STATUS_EVENT`] for this stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/HostOperationsEvent.ts")]
pub struct HostOperationsEvent(pub EventEnvelope<HostOperationsEventType, HostOperation>);

/// The subject kind every event carries.
pub const SUBJECT_KIND: &str = "hostOperation";

/// The stream's identity and sequence. One per process.
pub struct HostOperationsStream {
    stream_id: String,
    sequence: AtomicU64,
    /// Held while rows are numbered and emitted, so events reach the
    /// channel in sequence order.
    emit: Mutex<()>,
}

impl Default for HostOperationsStream {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
        }
    }
}

impl HostOperationsStream {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The last sequence handed out. `list_host_operations` reads this
    /// before it reads rows, so a change the snapshot misses always carries
    /// a larger sequence (listen before backfill).
    pub fn snapshot_sequence(&self) -> JsSafeInteger {
        let _ordered = self.lock();
        JsSafeInteger::try_from(self.sequence.load(Ordering::SeqCst))
            .expect("hostOperations sequence is JS-safe")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Emits one `hostOperations.operation.changed` per row, in order.
    /// Call only after the change each row describes has committed.
    pub fn publish<R: tauri::Runtime>(&self, app: &AppHandle<R>, rows: &[HostOperation]) {
        if rows.is_empty() {
            return;
        }
        let _ordered = self.lock();
        for row in rows {
            let _ = app.emit(STATUS_EVENT, self.envelope(row));
        }
    }

    fn envelope(&self, row: &HostOperation) -> HostOperationsEvent {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        HostOperationsEvent(EventEnvelope {
            contract_version: ContractVersion::V1,
            stream_id: self.stream_id.clone(),
            sequence: JsSafeInteger::try_from(sequence)
                .expect("hostOperations sequence is JS-safe"),
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            event_type: HostOperationsEventType::OperationChanged,
            subject: EventSubject {
                kind: SUBJECT_KIND.to_string(),
                id: row.id.clone(),
            },
            payload: row.clone(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_event_type_uses_its_dotted_wire_name() {
        assert_eq!(
            serde_json::to_value(HostOperationsEventType::OperationChanged).unwrap(),
            serde_json::json!("hostOperations.operation.changed")
        );
    }

    #[test]
    fn the_snapshot_sequence_starts_at_zero() {
        let stream = HostOperationsStream::default();
        assert_eq!(serde_json::to_value(stream.snapshot_sequence()).unwrap(), 0);
        assert!(!stream.stream_id().is_empty());
    }
}
