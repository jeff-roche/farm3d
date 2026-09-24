//! D11: inventory events, the `list_spools` backfill stream, and the
//! availability signal for P7.
//!
//! Inventory events go out on the shared `farm3d-event-v1` channel
//! ([`STATUS_EVENT`]) in the usual [`EventEnvelope`], on their own stream:
//! [`InventoryStream`] owns a per-process `streamId` and a sequence that
//! `list_spools` reports as `snapshotSequence`. The frontend listens before
//! it backfills and drops events at or below that sequence.
//!
//! [`publish`] and [`publish_ids`] are the only emitters. Commands call one
//! after their write returns `Ok`, never inside the transaction, and never
//! for an idempotent replay (D11: events and the broadcast fire after
//! commit only).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;

use chrono::{SecondsFormat, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use crate::connections::supervisor::STATUS_EVENT;
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::contracts::ContractVersion;
use crate::persistence::StorageError;
use crate::printers::StoredPrinter;

use super::{repository, slots, Availability, MaterialSlot, SpoolRecord};

/// Capacity of `RuntimeServices::inventory_changes`. A subscriber that
/// falls further behind sees `RecvError::Lagged` and should reload.
pub const INVENTORY_CHANGE_CAPACITY: usize = 64;

/// Every entity id one committed operation touched, for a reload. Sent on
/// `RuntimeServices::inventory_changes` (D11's availability signal for P7).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InventoryChange {
    pub spool_ids: Vec<String>,
    pub printer_ids: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "domain/InventoryEventType.ts")]
pub enum InventoryEventType {
    #[serde(rename = "spool.changed")]
    #[ts(rename = "spool.changed")]
    SpoolChanged,
    #[serde(rename = "printer.slots.changed")]
    #[ts(rename = "printer.slots.changed")]
    PrinterSlotsChanged,
    #[serde(rename = "spool.availability.changed")]
    #[ts(rename = "spool.availability.changed")]
    SpoolAvailabilityChanged,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    export_to = "domain/InventoryEventPayload.ts"
)]
pub enum InventoryEventPayload {
    SpoolChanged {
        spool: Box<SpoolRecord>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    PrinterSlotsChanged {
        printer_id: String,
        #[ts(type = "number")]
        revision: i64,
        material_slots: Vec<MaterialSlot>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    SpoolAvailabilityChanged {
        spool_id: String,
        availability: Availability,
    },
}

/// The exact envelope emitted on [`STATUS_EVENT`] for the inventory stream.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/InventoryEvent.ts")]
pub struct InventoryEvent(pub EventEnvelope<InventoryEventType, InventoryEventPayload>);

/// The inventory stream's identity and sequence. One per process, held in
/// `RuntimeServices`.
pub struct InventoryStream {
    stream_id: String,
    sequence: AtomicU64,
    /// Held while a batch is numbered and emitted, so events reach the
    /// channel in sequence order and one operation's events stay together.
    emit: Mutex<()>,
}

impl Default for InventoryStream {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            sequence: AtomicU64::new(0),
            emit: Mutex::new(()),
        }
    }
}

impl InventoryStream {
    pub fn stream_id(&self) -> &str {
        &self.stream_id
    }

    /// The last sequence handed out. `list_spools` reads this before it
    /// reads the Spools, so a change the snapshot misses always carries a
    /// larger sequence. A change the snapshot already includes may also
    /// arrive as a later event; the frontend's revision check absorbs it.
    pub fn snapshot_sequence(&self) -> JsSafeInteger {
        JsSafeInteger::try_from(self.sequence.load(Ordering::SeqCst))
            .expect("inventory sequence is JS-safe")
    }

    fn envelope(
        &self,
        event_type: InventoryEventType,
        subject: EventSubject,
        payload: InventoryEventPayload,
    ) -> InventoryEvent {
        let sequence = self.sequence.fetch_add(1, Ordering::SeqCst) + 1;
        InventoryEvent(EventEnvelope {
            contract_version: ContractVersion::V1,
            stream_id: self.stream_id.clone(),
            sequence: JsSafeInteger::try_from(sequence).expect("inventory sequence is JS-safe"),
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
            event_type,
            subject,
            payload,
        })
    }
}

/// The source of one `printer.slots.changed` payload.
#[derive(Clone, Debug)]
pub struct PrinterSlots {
    pub printer_id: String,
    pub revision: i64,
    pub material_slots: Vec<MaterialSlot>,
}

impl From<&StoredPrinter> for PrinterSlots {
    fn from(printer: &StoredPrinter) -> Self {
        Self {
            printer_id: printer.id.clone(),
            revision: printer.revision,
            material_slots: printer.material_slots.clone(),
        }
    }
}

/// Emits the committed state of every changed Spool and Printer, then
/// broadcasts one [`InventoryChange`]. In order:
///
/// 1. `spool.changed` for each Spool.
/// 2. `printer.slots.changed` for each Printer.
/// 3. `spool.availability.changed` for each Spool. Every change that
///    reaches here (create, edit, amount, move, lifecycle, reservation) can
///    change what P7 may use that Spool for, so each Spool gets one.
///
/// Sends nothing when both lists are empty. Call only after commit, and
/// never for a replay.
pub fn publish<R: tauri::Runtime>(
    app: &AppHandle<R>,
    services: &crate::RuntimeServices<R>,
    spools: &[SpoolRecord],
    printers: &[PrinterSlots],
) {
    if spools.is_empty() && printers.is_empty() {
        return;
    }
    emit_records(app, services, spools, printers);
    broadcast(
        services,
        spools.iter().map(|spool| spool.id.clone()).collect(),
        printers
            .iter()
            .map(|printer| printer.printer_id.clone())
            .collect(),
    );
}

/// The record-bearing events of [`publish`], in its order, with no
/// broadcast.
fn emit_records<R: tauri::Runtime>(
    app: &AppHandle<R>,
    services: &crate::RuntimeServices<R>,
    spools: &[SpoolRecord],
    printers: &[PrinterSlots],
) {
    let stream = &services.inventory_stream;
    {
        let _ordered = stream
            .emit
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        for spool in spools {
            let event = stream.envelope(
                InventoryEventType::SpoolChanged,
                subject("spool", &spool.id),
                InventoryEventPayload::SpoolChanged {
                    spool: Box::new(spool.clone()),
                },
            );
            let _ = app.emit(STATUS_EVENT, event);
        }
        for printer in printers {
            let event = stream.envelope(
                InventoryEventType::PrinterSlotsChanged,
                subject("printer", &printer.printer_id),
                InventoryEventPayload::PrinterSlotsChanged {
                    printer_id: printer.printer_id.clone(),
                    revision: printer.revision,
                    material_slots: printer.material_slots.clone(),
                },
            );
            let _ = app.emit(STATUS_EVENT, event);
        }
        for spool in spools {
            let event = stream.envelope(
                InventoryEventType::SpoolAvailabilityChanged,
                subject("spool", &spool.id),
                InventoryEventPayload::SpoolAvailabilityChanged {
                    spool_id: spool.id.clone(),
                    availability: spool.availability,
                },
            );
            let _ = app.emit(STATUS_EVENT, event);
        }
    }
}

/// Sends one [`InventoryChange`] (D11's availability signal for P7).
fn broadcast<R: tauri::Runtime>(
    services: &crate::RuntimeServices<R>,
    spool_ids: Vec<String>,
    printer_ids: Vec<String>,
) {
    // Having no subscriber is not an error: P7's evaluator doesn't exist yet.
    let _ = services.inventory_changes.send(InventoryChange {
        spool_ids,
        printer_ids,
    });
}

/// [`publish`] for callers that know only ids: the Printer commands, whose
/// transaction lives in `PrinterRepository`. Reads the committed records
/// first. If that read fails, the record-bearing events are skipped (the
/// write has already committed, so the command still succeeds, and the
/// frontend's next backfill recovers the state), but the
/// [`InventoryChange`] broadcast still goes out from the ids, so P7's
/// evaluator never misses a committed change.
pub fn publish_ids<R: tauri::Runtime>(
    app: &AppHandle<R>,
    services: &crate::RuntimeServices<R>,
    spool_ids: &[String],
    printer_ids: &[String],
) {
    if spool_ids.is_empty() && printer_ids.is_empty() {
        return;
    }
    let read = services
        .storage
        .read(|connection| Ok(read_committed(connection, spool_ids, printer_ids)))
        .and_then(|inner| inner);
    if let Ok((spools, printers)) = read {
        emit_records(app, services, &spools, &printers);
    }
    broadcast(services, spool_ids.to_vec(), printer_ids.to_vec());
}

fn read_committed(
    connection: &Connection,
    spool_ids: &[String],
    printer_ids: &[String],
) -> Result<(Vec<SpoolRecord>, Vec<PrinterSlots>), StorageError> {
    let spools = spool_ids
        .iter()
        .map(|id| repository::load_record(connection, id)?.ok_or(StorageError::OperationFailed))
        .collect::<Result<Vec<_>, _>>()?;
    let printers = printer_ids
        .iter()
        .map(|id| printer_slots(connection, id))
        .collect::<Result<Vec<_>, _>>()?;
    Ok((spools, printers))
}

/// `printer_id`'s current revision and live slots, read through whatever
/// connection or transaction the caller holds.
fn printer_slots(connection: &Connection, printer_id: &str) -> Result<PrinterSlots, StorageError> {
    let revision: i64 = connection.query_row(
        "SELECT revision FROM printers WHERE id = ?1",
        [printer_id],
        |row| row.get(0),
    )?;
    Ok(PrinterSlots {
        printer_id: printer_id.to_string(),
        revision,
        material_slots: slots::live_slots(connection, printer_id)?,
    })
}

fn subject(kind: &str, id: &str) -> EventSubject {
    EventSubject {
        kind: kind.to_string(),
        id: id.to_string(),
    }
}
