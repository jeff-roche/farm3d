//! P8 D7 "Events": the `attention` stream.
//!
//! Four event types on the shared `farm3d-event-v1` channel, in the usual
//! `EventEnvelope`, on this stream's own id and sequence (as
//! `queue/events.rs` does): `attention.event.changed`,
//! `attention.incident.changed`, `attention.snapshot.changed`, and
//! `camera.health.changed`. The payload is a tagged union. The envelope
//! type is [`AttentionStreamEvent`] (`AttentionEvent` is the record).
//!
//! [`AttentionStream::publish_change`] emits one event per changed row,
//! Events first, then Incidents (then snapshots, when a caller has them),
//! only after the change committed, and never for a replay or a no-op.
//! No payload carries a credential, host, or camera URL: none of these
//! records holds one (global constraint 3).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::cameras::{CameraHealth, CameraSnapshot};
use crate::connections::supervisor::STATUS_EVENT;
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::contracts::ContractVersion;
use crate::incidents::Incident;

use super::AttentionEvent;

/// The stream's event types.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "domain/AttentionStreamEventType.ts")]
pub enum AttentionStreamEventType {
    #[serde(rename = "attention.event.changed")]
    #[ts(rename = "attention.event.changed")]
    EventChanged,
    #[serde(rename = "attention.incident.changed")]
    #[ts(rename = "attention.incident.changed")]
    IncidentChanged,
    #[serde(rename = "attention.snapshot.changed")]
    #[ts(rename = "attention.snapshot.changed")]
    SnapshotChanged,
    #[serde(rename = "camera.health.changed")]
    #[ts(rename = "camera.health.changed")]
    CameraHealthChanged,
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
    export_to = "domain/AttentionStreamPayload.ts"
)]
pub enum AttentionStreamPayload {
    EventChanged { event: Box<AttentionEvent> },
    IncidentChanged { incident: Box<Incident> },
    SnapshotChanged { snapshot: Box<CameraSnapshot> },
    CameraHealthChanged { health: CameraHealth },
}

/// The exact envelope emitted on [`STATUS_EVENT`] for this stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/AttentionStreamEvent.ts")]
pub struct AttentionStreamEvent(
    pub EventEnvelope<AttentionStreamEventType, AttentionStreamPayload>,
);

/// The stream's identity and sequence. One per process, held in
/// `AttentionServices`.
pub struct AttentionStream {
    stream_id: String,
    sequence: AtomicU64,
    /// Held while rows are numbered and emitted, so events reach the
    /// channel in sequence order and one change's events stay together.
    emit: Mutex<()>,
}

impl Default for AttentionStream {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
        }
    }
}

impl AttentionStream {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The last sequence handed out. `list_attention` reads this before
    /// it reads rows (listen before backfill).
    pub fn snapshot_sequence(&self) -> JsSafeInteger {
        let _ordered = self.lock();
        JsSafeInteger::try_from(self.sequence.load(Ordering::SeqCst))
            .expect("attention sequence is JS-safe")
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, ()> {
        self.emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Emits one event per row: `events`, then `incidents`, then
    /// `snapshots`. Call only after the change committed, and never for a
    /// replay or a no-op.
    pub fn publish_change<R: tauri::Runtime>(
        &self,
        app: &AppHandle<R>,
        events: &[AttentionEvent],
        incidents: &[Incident],
        snapshots: &[CameraSnapshot],
    ) {
        let _ordered = self.lock();
        for event in events {
            self.emit(
                app,
                AttentionStreamEventType::EventChanged,
                "attentionEvent",
                &event.id,
                AttentionStreamPayload::EventChanged {
                    event: Box::new(event.clone()),
                },
            );
        }
        for incident in incidents {
            self.emit(
                app,
                AttentionStreamEventType::IncidentChanged,
                "incident",
                &incident.id,
                AttentionStreamPayload::IncidentChanged {
                    incident: Box::new(incident.clone()),
                },
            );
        }
        for snapshot in snapshots {
            self.emit(
                app,
                AttentionStreamEventType::SnapshotChanged,
                "cameraSnapshot",
                &snapshot.id,
                AttentionStreamPayload::SnapshotChanged {
                    snapshot: Box::new(snapshot.clone()),
                },
            );
        }
    }

    /// `camera.health.changed` for one Printer's camera.
    pub fn publish_camera_health<R: tauri::Runtime>(&self, app: &AppHandle<R>, health: &CameraHealth) {
        let _ordered = self.lock();
        self.emit(
            app,
            AttentionStreamEventType::CameraHealthChanged,
            "printer",
            &health.printer_id,
            AttentionStreamPayload::CameraHealthChanged {
                health: health.clone(),
            },
        );
    }

    fn emit<R: tauri::Runtime>(
        &self,
        app: &AppHandle<R>,
        event_type: AttentionStreamEventType,
        subject_kind: &str,
        subject_id: &str,
        payload: AttentionStreamPayload,
    ) {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        let event = AttentionStreamEvent(EventEnvelope {
            contract_version: ContractVersion::V1,
            stream_id: self.stream_id.clone(),
            sequence: JsSafeInteger::try_from(sequence).expect("attention sequence is JS-safe"),
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            event_type,
            subject: EventSubject {
                kind: subject_kind.to_string(),
                id: subject_id.to_string(),
            },
            payload,
        });
        let _ = app.emit(STATUS_EVENT, event);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn event_types_use_their_dotted_wire_names() {
        assert_eq!(
            serde_json::to_value([
                AttentionStreamEventType::EventChanged,
                AttentionStreamEventType::IncidentChanged,
                AttentionStreamEventType::SnapshotChanged,
                AttentionStreamEventType::CameraHealthChanged,
            ])
            .unwrap(),
            serde_json::json!([
                "attention.event.changed",
                "attention.incident.changed",
                "attention.snapshot.changed",
                "camera.health.changed"
            ])
        );
    }

    #[test]
    fn the_snapshot_sequence_starts_at_zero_and_counts_emits() {
        let app = tauri::test::mock_app();
        let stream = AttentionStream::default();
        assert_eq!(serde_json::to_value(stream.snapshot_sequence()).unwrap(), 0);
        stream.publish_camera_health(
            app.handle(),
            &CameraHealth {
                printer_id: "prn-1".to_string(),
                state: crate::cameras::CameraHealthState::NotConfigured,
                source_kind: None,
                last_success_at: None,
                last_failure_at: None,
                last_failure_kind: None,
            },
        );
        assert_eq!(serde_json::to_value(stream.snapshot_sequence()).unwrap(), 1);
    }
}
