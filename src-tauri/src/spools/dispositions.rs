//! D10: where each loaded Spool goes when its Printer is archived.
//! [`apply_dispositions`] runs inside `PrinterRepository::archive`'s
//! transaction, before eligibility is re-evaluated, so an archive never
//! leaves a Spool loaded and a failing disposition rolls the whole archive
//! back.

use std::collections::HashSet;

use rusqlite::{OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::RepositoryError;
use crate::printers::lifecycle::LifecycleAction;

use super::lifecycle::{ensure_can_mark_empty, record_marked_empty};
use super::movement::{self, MoveDestination, MoveOutcome, MoveRequest, MovementReason};
use super::repository;

/// Where one loaded Spool goes when its Printer is archived (D10).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    export_to = "domain/SpoolDisposition.ts"
)]
pub enum SpoolDisposition {
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Storage {
        #[serde(default)]
        #[ts(optional = nullable)]
        storage_label: Option<String>,
    },
    /// Into a slot on a different, non-archived Printer, with the same
    /// occupant guard and displacement as `move_spool` (D6).
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Slot {
        slot_id: String,
        expected_occupant_spool_id: Option<String>,
        #[serde(default)]
        #[ts(optional = nullable)]
        displaced_storage_label: Option<String>,
    },
    /// D9's `markEmpty`: unload to storage and record the Spool as used up.
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    MarkEmpty {
        #[serde(default)]
        #[ts(optional = nullable)]
        storage_label: Option<String>,
    },
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/SpoolDispositionInput.ts"
)]
pub struct SpoolDispositionInput {
    pub spool_id: String,
    #[ts(type = "number")]
    pub expected_spool_revision: i64,
    pub disposition: SpoolDisposition,
}

/// D10 steps 1-3 for `printer_id`'s archive:
///
/// 1. `inputs` must name every Spool loaded in the Printer exactly once.
/// 2. A `slot` destination must be a live slot on a different,
///    non-archived Printer.
/// 3. A `markEmpty` Spool must pass D9's mark-empty rules; a reserved one
///    is `LIFECYCLE_BLOCKED` (`SPOOL_RESERVED`).
///
/// Rule violations in 1-3 are `VALIDATION` on `spoolDispositions`. Then
/// every move is applied as ONE `apply_moves` operation under
/// `operation_id` (reason `printerArchived`, or `consumed` for
/// `markEmpty`; a displaced occupant gets `displaced`), and each
/// `markEmpty` Spool gets its ledger row and `empty` lifecycle.
///
/// Empty `inputs` writes nothing and returns an empty outcome, so the
/// caller's eligibility check reports any loaded Spool as `SPOOLS_LOADED`.
pub fn apply_dispositions(
    tx: &Transaction<'_>,
    printer_id: &str,
    operation_id: &str,
    inputs: &[SpoolDispositionInput],
) -> Result<MoveOutcome, RepositoryError> {
    let invalid = RepositoryError::Validation {
        field_path: "spoolDispositions",
    };
    if inputs.is_empty() {
        return Ok(MoveOutcome {
            spool_ids: Vec::new(),
            printer_ids: Vec::new(),
            movements: Vec::new(),
            replayed: false,
        });
    }

    let loaded: HashSet<String> = repository::loaded_on_printer(tx, printer_id)?
        .into_iter()
        .map(|spool| spool.id)
        .collect();
    let mut named = HashSet::new();
    if !inputs
        .iter()
        .all(|input| named.insert(input.spool_id.clone()))
        || named != loaded
    {
        return Err(invalid);
    }

    let mut moves = Vec::with_capacity(inputs.len());
    let mut emptied = Vec::new();
    for input in inputs {
        let (destination, reason) = match &input.disposition {
            SpoolDisposition::Storage { storage_label } => (
                MoveDestination::Storage {
                    storage_label: storage_label.clone(),
                },
                MovementReason::PrinterArchived,
            ),
            SpoolDisposition::Slot {
                slot_id,
                expected_occupant_spool_id,
                displaced_storage_label,
            } => {
                if !is_slot_elsewhere(tx, slot_id, printer_id)? {
                    return Err(invalid);
                }
                (
                    MoveDestination::Slot {
                        slot_id: slot_id.clone(),
                        expected_occupant_spool_id: expected_occupant_spool_id.clone(),
                        displaced_storage_label: displaced_storage_label.clone(),
                    },
                    MovementReason::PrinterArchived,
                )
            }
            SpoolDisposition::MarkEmpty { storage_label } => {
                let spool = repository::load_spool(tx, &input.spool_id)?.ok_or_else(|| {
                    RepositoryError::NotFound {
                        entity_id: input.spool_id.clone(),
                    }
                })?;
                ensure_can_mark_empty(tx, &spool, LifecycleAction::Archive, "spoolDispositions")?;
                emptied.push(input.spool_id.as_str());
                (
                    MoveDestination::Storage {
                        storage_label: storage_label.clone(),
                    },
                    MovementReason::Consumed,
                )
            }
        };
        moves.push(MoveRequest {
            spool_id: input.spool_id.clone(),
            expected_spool_revision: input.expected_spool_revision,
            destination,
            reason_override: Some(reason),
        });
    }

    let outcome = movement::apply_moves(tx, operation_id, &moves)?;
    if outcome.replayed {
        return Ok(outcome);
    }
    for spool_id in emptied {
        record_marked_empty(tx, spool_id)?;
    }
    Ok(outcome)
}

/// D10 step 2: `slot_id` is a live slot on a non-archived Printer other
/// than `archiving_printer_id`.
fn is_slot_elsewhere(
    tx: &Transaction<'_>,
    slot_id: &str,
    archiving_printer_id: &str,
) -> Result<bool, RepositoryError> {
    let eligible: Option<bool> = tx
        .query_row(
            "SELECT ms.removed_at IS NULL AND p.archived_at IS NULL AND p.id <> ?2
             FROM material_slots ms JOIN printers p ON p.id = ms.printer_id
             WHERE ms.id = ?1",
            [slot_id, archiving_printer_id],
            |row| row.get(0),
        )
        .optional()?;
    Ok(eligible.unwrap_or(false))
}
