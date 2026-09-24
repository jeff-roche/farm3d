//! D5/D6: the only code that changes where a Spool is. Load, unload, swap,
//! storage relocation, and a direct move between Printers are all one
//! [`apply_move`] call, which writes the Spool's new location, its
//! `spool_movements` row(s), and every revision bump inside the caller's
//! transaction (global constraint: "no other code writes `spools.slot_id`
//! or `spool_movements`").
//!
//! [`move_spool`] runs D6 steps 1-6: step 1 (replay) is its claim in the
//! operations ledger (`spools::operations`), steps 2-6 are [`apply_move`].
//! Step 7 (commit, then emit events) is the caller's: `Storage::write_repo`
//! commits on `Ok` and rolls back on `Err`, so every early return below
//! leaves the database untouched.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::{take_transaction_failure, FailurePoint, RepositoryError, StorageError};
use crate::printers::now_rfc3339;

use super::operations::{self, Claim, OperationKind};
use super::repository::{check_and_bump_revision, normalize_storage_label};
use super::{decode_enum, encode_enum, SpoolLifecycle};

/// D6: where a Spool is going.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    export_to = "domain/MoveDestination.ts"
)]
pub enum MoveDestination {
    /// Load into `slot_id`. `expected_occupant_spool_id` is the concurrency
    /// guard (D6 step 3); a current occupant is displaced to storage under
    /// `displaced_storage_label` (D6 step 4).
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Slot {
        slot_id: String,
        expected_occupant_spool_id: Option<String>,
        #[serde(default)]
        #[ts(optional = nullable)]
        displaced_storage_label: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Storage {
        #[serde(default)]
        #[ts(optional = nullable)]
        storage_label: Option<String>,
    },
}

/// D6: why a movement row was written. Stored as the camelCase string the
/// migration's `reason` CHECK lists.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/MovementReason.ts")]
pub enum MovementReason {
    Load,
    Unload,
    Displaced,
    Relocate,
    Consumed,
    PrinterArchived,
}

/// One end of a movement row. A slot end carries `slotId` and its
/// `printerId` (resolved through `material_slots` at read time); a storage
/// end carries only the optional `storageLabel`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/SpoolLocationSnapshot.ts"
)]
pub struct SpoolLocationSnapshot {
    pub slot_id: Option<String>,
    pub printer_id: Option<String>,
    pub storage_label: Option<String>,
}

/// One `spool_movements` row (D6).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SpoolMovement.ts")]
pub struct SpoolMovement {
    pub id: String,
    pub operation_id: String,
    pub spool_id: String,
    pub reason: MovementReason,
    pub from: SpoolLocationSnapshot,
    pub to: SpoolLocationSnapshot,
    pub occurred_at: String,
}

/// What one operation touched, for the caller to reload and broadcast
/// after commit. `spool_ids` lists the moved Spool, then the displaced one
/// if any; `printer_ids` lists each Printer whose occupancy changed.
/// `movements` is in write order (a `Displaced` row comes first).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MoveOutcome {
    pub spool_ids: Vec<String>,
    pub printer_ids: Vec<String>,
    pub movements: Vec<SpoolMovement>,
    pub replayed: bool,
}

impl MoveOutcome {
    /// No Spool, Printer, or movement to report — two "nothing found under
    /// this operationId" fallbacks that differ only in `replayed`:
    /// `archive_with_outcome`'s replay branch (`printers/repository.rs`,
    /// `replayed: true` — the archive's dispositions wrote no movement, but
    /// the claim itself was still a replay), and [`apply_moves`]'s
    /// defensive "no rows found for a batch it just wrote" fallback
    /// (`replayed: false`, immediately confirmed by its caller regardless).
    /// [`recorded_or_unmoved`] needs its own literal instead — its
    /// `spool_ids` isn't empty (it still names the Spool the caller asked
    /// about).
    pub(crate) fn empty(replayed: bool) -> Self {
        Self {
            spool_ids: Vec::new(),
            printer_ids: Vec::new(),
            movements: Vec::new(),
            replayed,
        }
    }
}

/// One move of [`apply_moves`]'s batch — the same arguments [`apply_move`]
/// takes for a single move, packaged so several can share one
/// `operation_id`.
#[derive(Clone, Debug)]
pub struct MoveRequest {
    pub spool_id: String,
    pub expected_spool_revision: i64,
    pub destination: MoveDestination,
    pub reason_override: Option<MovementReason>,
}

/// The `move_spool` operation (D6): claims `operation_id` in the operations
/// ledger, then applies the move. A retry of the same request is a replay:
/// it returns the outcome [`find_operation`] reconstructs, with `replayed =
/// true`, and writes nothing, before any revision or occupancy check. The
/// same id for a different request is
/// [`RepositoryError::OperationIdReused`].
pub fn move_spool(
    tx: &Transaction<'_>,
    operation_id: &str,
    spool_id: &str,
    expected_spool_revision: i64,
    dest: &MoveDestination,
) -> Result<MoveOutcome, RepositoryError> {
    let digest = operations::digest(&MoveSpoolRequest {
        spool_id,
        expected_spool_revision,
        destination: dest,
    });
    match operations::claim(tx, operation_id, OperationKind::MoveSpool, &digest)? {
        Claim::Replay => Ok(recorded_or_unmoved(tx, operation_id, spool_id)?),
        Claim::Fresh => apply_move(
            tx,
            operation_id,
            spool_id,
            expected_spool_revision,
            dest,
            None,
        ),
    }
}

/// The request fields that define a `move_spool` operation, in a fixed
/// order for [`operations::digest`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MoveSpoolRequest<'a> {
    spool_id: &'a str,
    expected_spool_revision: i64,
    destination: &'a MoveDestination,
}

/// A replay's outcome: the movements recorded under `operation_id`, or —
/// when the operation wrote none (or Printer deletion has since cascaded
/// them away) — just `spool_id`, with nothing moved.
pub(crate) fn recorded_or_unmoved(
    tx: &Transaction<'_>,
    operation_id: &str,
    spool_id: &str,
) -> Result<MoveOutcome, StorageError> {
    Ok(
        find_operation(tx, operation_id)?.unwrap_or_else(|| MoveOutcome {
            spool_ids: vec![spool_id.to_string()],
            printer_ids: Vec::new(),
            movements: Vec::new(),
            replayed: true,
        }),
    )
}

/// D6 steps 2-6 for a single move under `operation_id`. Does no replay
/// check: that is the operations ledger's job, done once by the
/// operation's caller ([`move_spool`], `lifecycle::apply_lifecycle`). A
/// thin wrapper over [`apply_moves`] — see that function for callers that
/// need several moves (e.g. several initial loads) to share one
/// `operation_id`.
pub fn apply_move(
    tx: &Transaction<'_>,
    operation_id: &str,
    spool_id: &str,
    expected_spool_revision: i64,
    dest: &MoveDestination,
    reason_override: Option<MovementReason>,
) -> Result<MoveOutcome, RepositoryError> {
    apply_moves(
        tx,
        operation_id,
        &[MoveRequest {
            spool_id: spool_id.to_string(),
            expected_spool_revision,
            destination: dest.clone(),
            reason_override,
        }],
    )
}

/// D6 steps 2-6 for a batch of moves sharing one `operation_id`. Moves
/// apply in order, each under its own revision/occupancy checks; if any
/// move fails, the whole batch's writes roll back with it (the caller's
/// transaction). The returned outcome combines every row written under
/// `operation_id` — `spool_ids`/`printer_ids` deduped, `movements` in
/// write order — via the same [`find_operation`] query a replay uses.
///
/// This function performs no replay check itself: the caller claims
/// `operation_id` in the operations ledger first (`spools/operations.rs`'s
/// `claim`, e.g. `archive_printer`'s dispositions), or uses a fresh
/// server-generated id (`printers/repository.rs`'s `create_with_layout`,
/// for `initialLoads`, D12).
pub fn apply_moves(
    tx: &Transaction<'_>,
    operation_id: &str,
    moves: &[MoveRequest],
) -> Result<MoveOutcome, RepositoryError> {
    if operation_id.trim().is_empty() {
        return Err(RepositoryError::Validation {
            field_path: "operationId",
        });
    }
    for request in moves {
        apply_one_move(
            tx,
            operation_id,
            &request.spool_id,
            request.expected_spool_revision,
            &request.destination,
            request.reason_override,
        )?;
    }
    let mut outcome =
        find_operation(tx, operation_id)?.unwrap_or_else(|| MoveOutcome::empty(false));
    outcome.replayed = false;
    Ok(outcome)
}

/// One move's steps 2-6, without the final combined-outcome query (that's
/// [`apply_moves`]'s, once per batch — this function only writes the
/// rows).
fn apply_one_move(
    tx: &Transaction<'_>,
    operation_id: &str,
    spool_id: &str,
    expected_spool_revision: i64,
    dest: &MoveDestination,
    reason_override: Option<MovementReason>,
) -> Result<(), RepositoryError> {
    // Step 2 (and half of step 6): the returned row still holds the
    // Spool's pre-move location.
    let spool = check_and_bump_revision(tx, spool_id, expected_spool_revision)?;
    let occurred_at = now_rfc3339();
    let from = Location {
        slot_id: spool.slot_id.clone(),
        storage_label: spool.storage_label.clone(),
    };
    let mut touched_printers = Vec::new();
    if let Some(from_slot) = &from.slot_id {
        touched_printers.push(printer_of_slot(tx, from_slot)?);
    }

    let (to, derived_reason) = match dest {
        MoveDestination::Slot {
            slot_id,
            expected_occupant_spool_id,
            displaced_storage_label,
        } => {
            if spool.lifecycle == SpoolLifecycle::Archived {
                return Err(RepositoryError::Validation {
                    field_path: "spoolId",
                });
            }
            let printer_id = loadable_slot_printer(tx, slot_id)?;
            if from.slot_id.as_deref() == Some(slot_id.as_str()) {
                return Err(RepositoryError::Validation {
                    field_path: "destination.slotId",
                });
            }
            let displaced_label = normalize_storage_label(displaced_storage_label.as_deref())
                .map_err(|_| RepositoryError::Validation {
                    field_path: "destination.displacedStorageLabel",
                })?;

            // Step 3: the concurrent-movement guard.
            let occupant = current_occupant(tx, slot_id)?;
            if occupant != *expected_occupant_spool_id {
                return Err(RepositoryError::OccupancyConflict {
                    slot_id: slot_id.clone(),
                    current_occupant_spool_id: occupant,
                });
            }

            // Step 4: displace the occupant first, so the slot is free when
            // the moved Spool takes it (`spools_slot_occupancy`).
            if let Some(occupant) = occupant {
                let to_storage = Location {
                    slot_id: None,
                    storage_label: displaced_label,
                };
                set_location(tx, &occupant, &to_storage, true, &occurred_at)?;
                insert_row(
                    tx,
                    operation_id,
                    &occupant,
                    MovementReason::Displaced,
                    &Location {
                        slot_id: Some(slot_id.clone()),
                        storage_label: None,
                    },
                    &to_storage,
                    &occurred_at,
                )?;
            }
            if take_transaction_failure(tx, FailurePoint::AfterDisplacement) {
                return Err(RepositoryError::Storage(StorageError::OperationFailed));
            }

            touched_printers.push(printer_id);
            let to = Location {
                slot_id: Some(slot_id.clone()),
                storage_label: None,
            };
            (to, MovementReason::Load)
        }
        MoveDestination::Storage { storage_label } => {
            let storage_label =
                normalize_storage_label(storage_label.as_deref()).map_err(|_| {
                    RepositoryError::Validation {
                        field_path: "destination.storageLabel",
                    }
                })?;
            let to = Location {
                slot_id: None,
                storage_label,
            };
            // Mirrors the "into its current slot" rule: a no-op move is
            // rejected rather than recorded.
            if to == from {
                return Err(RepositoryError::Validation {
                    field_path: "destination.storageLabel",
                });
            }
            let reason = if from.slot_id.is_some() {
                MovementReason::Unload
            } else {
                MovementReason::Relocate
            };
            (to, reason)
        }
    };

    // Step 5. The Spool's own revision was bumped by
    // `check_and_bump_revision` above.
    set_location(tx, spool_id, &to, false, &occurred_at)?;
    insert_row(
        tx,
        operation_id,
        spool_id,
        reason_override.unwrap_or(derived_reason),
        &from,
        &to,
        &occurred_at,
    )?;

    // Step 6: every Printer whose occupancy changed.
    touched_printers.dedup();
    for printer_id in &touched_printers {
        tx.execute(
            "UPDATE printers SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
            params![printer_id, occurred_at],
        )?;
    }

    Ok(())
}

/// The outcome recorded under `operation_id`, marked `replayed = true`, or
/// `None` if no movement used that id. A replay's result (D6 step 1) and
/// every applied batch's combined outcome both come from here.
pub fn find_operation(
    tx: &Transaction<'_>,
    operation_id: &str,
) -> Result<Option<MoveOutcome>, StorageError> {
    let movements = query_movements(tx, "m.operation_id = ?1", operation_id)?;
    if movements.is_empty() {
        return Ok(None);
    }

    // The moved Spool first, then the displaced one; printers in the order
    // those rows touch them.
    let (displaced, moved): (Vec<&SpoolMovement>, Vec<&SpoolMovement>) = movements
        .iter()
        .partition(|row| row.reason == MovementReason::Displaced);
    let mut spool_ids = Vec::new();
    let mut printer_ids = Vec::new();
    for row in moved.into_iter().chain(displaced) {
        push_unique(&mut spool_ids, &row.spool_id);
        for printer_id in [&row.from.printer_id, &row.to.printer_id]
            .into_iter()
            .flatten()
        {
            push_unique(&mut printer_ids, printer_id);
        }
    }
    Ok(Some(MoveOutcome {
        spool_ids,
        printer_ids,
        movements,
        replayed: true,
    }))
}

/// A Spool's movement history, oldest first.
pub fn history(tx: &Transaction<'_>, spool_id: &str) -> Result<Vec<SpoolMovement>, StorageError> {
    query_movements(tx, "m.spool_id = ?1", spool_id)
}

/// A Spool's location as stored: a slot, or storage with an optional label
/// (never both — the table's CHECK).
#[derive(PartialEq, Eq)]
struct Location {
    slot_id: Option<String>,
    storage_label: Option<String>,
}

fn push_unique(values: &mut Vec<String>, value: &str) {
    if !values.iter().any(|existing| existing == value) {
        values.push(value.to_string());
    }
}

fn printer_of_slot(tx: &Transaction<'_>, slot_id: &str) -> Result<String, StorageError> {
    tx.query_row(
        "SELECT printer_id FROM material_slots WHERE id = ?1",
        [slot_id],
        |row| row.get(0),
    )
    .map_err(StorageError::from)
}

/// D5: a Spool can load only into a live slot on an active Printer.
/// Returns that Printer's id.
fn loadable_slot_printer(tx: &Transaction<'_>, slot_id: &str) -> Result<String, RepositoryError> {
    let slot: Option<(String, bool)> = tx
        .query_row(
            "SELECT ms.printer_id, ms.removed_at IS NULL AND p.archived_at IS NULL
             FROM material_slots ms JOIN printers p ON p.id = ms.printer_id
             WHERE ms.id = ?1",
            [slot_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?;
    match slot {
        Some((printer_id, true)) => Ok(printer_id),
        _ => Err(RepositoryError::Validation {
            field_path: "destination.slotId",
        }),
    }
}

fn current_occupant(tx: &Transaction<'_>, slot_id: &str) -> Result<Option<String>, StorageError> {
    tx.query_row(
        "SELECT id FROM spools WHERE slot_id = ?1",
        [slot_id],
        |row| row.get(0),
    )
    .optional()
    .map_err(StorageError::from)
}

/// The single write of `spools.slot_id`/`storage_label`. `bump_revision`
/// is for the displaced Spool, whose revision nobody checked; the moved
/// Spool's was already bumped by `check_and_bump_revision`.
fn set_location(
    tx: &Transaction<'_>,
    spool_id: &str,
    location: &Location,
    bump_revision: bool,
    now: &str,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE spools SET slot_id = ?2, storage_label = ?3,
             revision = revision + ?4, updated_at = ?5
         WHERE id = ?1",
        params![
            spool_id,
            location.slot_id,
            location.storage_label,
            i64::from(bump_revision),
            now
        ],
    )?;
    Ok(())
}

fn insert_row(
    tx: &Transaction<'_>,
    operation_id: &str,
    spool_id: &str,
    reason: MovementReason,
    from: &Location,
    to: &Location,
    occurred_at: &str,
) -> Result<(), StorageError> {
    tx.execute(
        "INSERT INTO spool_movements(
            id, operation_id, spool_id, reason,
            from_slot_id, from_storage_label, to_slot_id, to_storage_label, occurred_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            format!("mov-{}", uuid::Uuid::new_v4()),
            operation_id,
            spool_id,
            encode_enum(reason),
            from.slot_id,
            from.storage_label,
            to.slot_id,
            to.storage_label,
            occurred_at
        ],
    )?;
    Ok(())
}

/// Movement rows matching `filter` (one `?1` parameter), in write order.
/// `rowid` breaks ties between rows of one operation, which share
/// `occurred_at`.
fn query_movements(
    tx: &Transaction<'_>,
    filter: &str,
    value: &str,
) -> Result<Vec<SpoolMovement>, StorageError> {
    let query = format!(
        "SELECT m.id, m.operation_id, m.spool_id, m.reason,
                m.from_slot_id, from_slot.printer_id, m.from_storage_label,
                m.to_slot_id, to_slot.printer_id, m.to_storage_label,
                m.occurred_at
         FROM spool_movements m
         LEFT JOIN material_slots from_slot ON from_slot.id = m.from_slot_id
         LEFT JOIN material_slots to_slot ON to_slot.id = m.to_slot_id
         WHERE {filter}
         ORDER BY m.occurred_at, m.rowid"
    );
    let mut statement = tx.prepare(&query)?;
    let rows = statement
        .query_map([value], |row| {
            let reason_text: String = row.get(3)?;
            Ok(SpoolMovement {
                id: row.get(0)?,
                operation_id: row.get(1)?,
                spool_id: row.get(2)?,
                reason: decode_enum(&reason_text).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(
                        3,
                        rusqlite::types::Type::Text,
                        Box::new(error),
                    )
                })?,
                from: SpoolLocationSnapshot {
                    slot_id: row.get(4)?,
                    printer_id: row.get(5)?,
                    storage_label: row.get(6)?,
                },
                to: SpoolLocationSnapshot {
                    slot_id: row.get(7)?,
                    printer_id: row.get(8)?,
                    storage_label: row.get(9)?,
                },
                occurred_at: row.get(10)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
