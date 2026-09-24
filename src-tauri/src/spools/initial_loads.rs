//! D12: applies a brand-new Printer's `initialLoads` against its
//! just-created Material Slot layout, inside the same create transaction as
//! `spools::slots::insert_layout` — moved out of
//! `printers::repository::create_with_layout` so the Printer repository
//! never needs to know `apply_moves`'s revision-bump bookkeeping.

use rusqlite::Transaction;

use crate::persistence::RepositoryError;

use super::movement::{self, MoveDestination, MoveRequest};
use super::slots::InitialLoad;
use super::MaterialSlot;

/// Loads every `initial_loads` entry into `created_slots` (indexed by
/// `slot_index`) under one generated `operationId` (D12: server-generated,
/// so no client can retry it — not claimed in the operations ledger). An
/// out-of-range `slot_index` is `VALIDATION` on `initialLoads`.
///
/// Returns the Printer's `(revision, updatedAt)` after the loads, or `None`
/// when `initial_loads` is empty (a no-op: the caller keeps its
/// just-inserted revision 1). `apply_moves` bumps the Printer's row once per
/// occupied slot, so a non-empty batch always leaves it past revision 1 —
/// the caller must re-read it rather than reuse what it inserted.
pub fn apply_initial_loads(
    tx: &Transaction<'_>,
    printer_id: &str,
    created_slots: &[MaterialSlot],
    initial_loads: &[InitialLoad],
) -> Result<Option<(i64, String)>, RepositoryError> {
    if initial_loads.is_empty() {
        return Ok(None);
    }
    let operation_id = format!("op-{}", uuid::Uuid::new_v4());
    let mut moves = Vec::with_capacity(initial_loads.len());
    for load in initial_loads {
        let slot = created_slots
            .get(load.slot_index)
            .ok_or(RepositoryError::Validation {
                field_path: "initialLoads",
            })?;
        moves.push(MoveRequest {
            spool_id: load.spool_id.clone(),
            expected_spool_revision: load.expected_spool_revision,
            destination: MoveDestination::Slot {
                slot_id: slot.id.clone(),
                expected_occupant_spool_id: None,
                displaced_storage_label: None,
            },
            reason_override: None,
        });
    }
    movement::apply_moves(tx, &operation_id, &moves)?;
    let (revision, updated_at): (i64, String) = tx.query_row(
        "SELECT revision, updated_at FROM printers WHERE id = ?1",
        [printer_id],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )?;
    Ok(Some((revision, updated_at)))
}
