//! P3 Task 3: atomic Spool movement and slot occupancy (D5, D6). Covers
//! every point of the task-3 brief's Step 1: load, swap, the swap's
//! atomicity under an injected mid-transaction failure, the
//! expected-occupant guard, a concurrent race for one empty slot, replay by
//! `operationId`, rejected moves, and a direct move between Printers.

mod common;

use std::sync::{Arc, Barrier};
use std::thread;

use farm3d_lib::persistence::{FailurePoint, RepositoryError, Storage, StorageError};
use farm3d_lib::spools::ledger::AmountEntry;
use farm3d_lib::spools::movement::{self, MoveDestination, MoveOutcome, MoveRequest, MovementReason};
use farm3d_lib::spools::repository::{self, StoredSpool};
use farm3d_lib::spools::{AmountConfidence, FilamentDiameter, MaterialFamily, SpoolFields};

use common::storage;

fn valid_fields() -> SpoolFields {
    SpoolFields {
        manufacturer: "Polymaker".to_string(),
        product: None,
        material_family: MaterialFamily::Pla,
        material_other: None,
        color_name: "Black".to_string(),
        color_hex: None,
        diameter: FilamentDiameter::D175,
        nominal_mg: 1_000_000,
        low_threshold_mg: 100_000,
        tare_id: None,
        notes: None,
    }
}

/// Seeds a Printer and one live `Main` slot with raw SQL — slot layouts
/// have no Rust API until Task 5.
fn seed_printer_with_slot(storage: &Storage, printer_id: &str, slot_id: &str) {
    storage
        .write(|tx| {
            tx.execute(
                "INSERT INTO printers(
                    id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                    catalog_model_id, catalog_printer_variant, notes, overrides_json,
                    created_at, updated_at
                 ) VALUES (?1, 1, 'Printer', '', '', '', '', '', '', '{}',
                           '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                [printer_id],
            )?;
            tx.execute(
                "INSERT INTO material_slots(id, printer_id, position, name, created_at)
                 VALUES (?1, ?2, 0, 'Main', '2026-01-01T00:00:00.000Z')",
                [slot_id, printer_id],
            )?;
            Ok(())
        })
        .unwrap();
}

fn new_spool(storage: &Storage) -> StoredSpool {
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    storage
        .write_repo(|tx| repository::insert_spool(tx, &valid_fields(), &entry, None))
        .unwrap()
}

fn spool(storage: &Storage, id: &str) -> StoredSpool {
    storage
        .write_repo(|tx| Ok(repository::load_spool(tx, id)?.unwrap()))
        .unwrap()
}

fn printer_revision(storage: &Storage, printer_id: &str) -> i64 {
    storage
        .read(|connection| {
            connection.query_row(
                "SELECT revision FROM printers WHERE id = ?1",
                [printer_id],
                |row| row.get(0),
            )
        })
        .unwrap()
}

fn movement_count(storage: &Storage) -> i64 {
    storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM spool_movements", [], |row| row.get(0))
        })
        .unwrap()
}

fn to_slot(
    slot_id: &str,
    expected_occupant: Option<&str>,
    displaced_label: Option<&str>,
) -> MoveDestination {
    MoveDestination::Slot {
        slot_id: slot_id.to_string(),
        expected_occupant_spool_id: expected_occupant.map(str::to_string),
        displaced_storage_label: displaced_label.map(str::to_string),
    }
}

fn apply(
    storage: &Storage,
    operation_id: &str,
    spool: &StoredSpool,
    dest: &MoveDestination,
) -> Result<MoveOutcome, RepositoryError> {
    storage.write_repo(|tx| {
        movement::apply_move(tx, operation_id, &spool.id, spool.revision, dest, None)
    })
}

fn apply_batch(
    storage: &Storage,
    operation_id: &str,
    moves: &[MoveRequest],
) -> Result<MoveOutcome, RepositoryError> {
    storage.write_repo(|tx| movement::apply_moves(tx, operation_id, moves))
}

/// 1. Load: a storage Spool loads into an empty `Main`; one `Load` row; the
///    Spool's and the Printer's revisions are both +1.
#[test]
fn loading_a_storage_spool_into_an_empty_slot_writes_one_load_row_and_bumps_both_revisions() {
    let (_temp, _lease, storage, _db) = storage();
    seed_printer_with_slot(&storage, "prn-x", "slt-x");
    let a = new_spool(&storage);

    let outcome = apply(&storage, "op-load", &a, &to_slot("slt-x", None, None)).unwrap();

    let after = spool(&storage, &a.id);
    assert_eq!(after.slot_id.as_deref(), Some("slt-x"));
    assert_eq!(after.storage_label, None);
    assert_eq!(after.revision, a.revision + 1);
    assert_eq!(printer_revision(&storage, "prn-x"), 2);

    assert!(!outcome.replayed);
    assert_eq!(outcome.spool_ids, vec![a.id.clone()]);
    assert_eq!(outcome.printer_ids, vec!["prn-x".to_string()]);
    assert_eq!(outcome.movements.len(), 1);
    let row = &outcome.movements[0];
    assert!(row.id.starts_with("mov-"));
    assert_eq!(row.operation_id, "op-load");
    assert_eq!(row.reason, MovementReason::Load);
    assert_eq!(row.from.slot_id, None);
    assert_eq!(row.to.slot_id.as_deref(), Some("slt-x"));
    assert_eq!(row.to.printer_id.as_deref(), Some("prn-x"));

    let history = storage.write(|tx| movement::history(tx, &a.id)).unwrap();
    assert_eq!(history, outcome.movements);
}

fn swap_setup(storage: &Storage) -> (StoredSpool, StoredSpool) {
    seed_printer_with_slot(storage, "prn-x", "slt-x");
    let a = new_spool(storage);
    let b = new_spool(storage);
    apply(storage, "op-load-a", &a, &to_slot("slt-x", None, None)).unwrap();
    (spool(storage, &a.id), b)
}

/// 2. Swap: B loads into A's slot with `expected_occupant = A` and
///    `displaced_storage_label = "Shelf"`. Two rows share the operation,
///    `Displaced` first.
#[test]
fn swapping_into_an_occupied_slot_displaces_the_occupant_to_storage_first() {
    let (_temp, _lease, storage, _db) = storage();
    let (a, b) = swap_setup(&storage);
    let printer_before = printer_revision(&storage, "prn-x");

    let outcome = apply(
        &storage,
        "op-swap",
        &b,
        &to_slot("slt-x", Some(&a.id), Some("Shelf")),
    )
    .unwrap();

    let a_after = spool(&storage, &a.id);
    let b_after = spool(&storage, &b.id);
    assert_eq!(a_after.slot_id, None);
    assert_eq!(a_after.storage_label.as_deref(), Some("Shelf"));
    assert_eq!(a_after.revision, a.revision + 1);
    assert_eq!(b_after.slot_id.as_deref(), Some("slt-x"));
    assert_eq!(b_after.revision, b.revision + 1);
    assert_eq!(printer_revision(&storage, "prn-x"), printer_before + 1);

    assert_eq!(outcome.spool_ids, vec![b.id.clone(), a.id.clone()]);
    assert_eq!(outcome.printer_ids, vec!["prn-x".to_string()]);
    assert_eq!(outcome.movements.len(), 2);
    assert!(outcome
        .movements
        .iter()
        .all(|row| row.operation_id == "op-swap"));
    assert_eq!(outcome.movements[0].reason, MovementReason::Displaced);
    assert_eq!(outcome.movements[0].spool_id, a.id);
    assert_eq!(outcome.movements[0].from.slot_id.as_deref(), Some("slt-x"));
    assert_eq!(
        outcome.movements[0].to.storage_label.as_deref(),
        Some("Shelf")
    );
    assert_eq!(outcome.movements[1].reason, MovementReason::Load);
    assert_eq!(outcome.movements[1].spool_id, b.id);
}

/// 3. Atomic swap under failure: a fault between the displaced write and
///    the loaded write leaves both Spools and the movement table unchanged.
#[test]
fn a_failure_after_displacement_rolls_back_the_whole_swap() {
    let (_temp, _lease, storage, _db) = storage();
    let (a, b) = swap_setup(&storage);
    let rows_before = movement_count(&storage);
    let printer_before = printer_revision(&storage, "prn-x");

    storage.inject_failure_once(FailurePoint::AfterDisplacement);
    let result = apply(
        &storage,
        "op-swap",
        &b,
        &to_slot("slt-x", Some(&a.id), Some("Shelf")),
    );
    assert!(
        matches!(
            result,
            Err(RepositoryError::Storage(StorageError::OperationFailed))
        ),
        "the injected failure must abort the move: {result:?}"
    );

    assert_eq!(spool(&storage, &a.id), a);
    assert_eq!(spool(&storage, &b.id), b);
    assert_eq!(movement_count(&storage), rows_before);
    assert_eq!(printer_revision(&storage, "prn-x"), printer_before);

    // The failure fires once: the same swap then succeeds.
    apply(
        &storage,
        "op-swap",
        &b,
        &to_slot("slt-x", Some(&a.id), Some("Shelf")),
    )
    .unwrap();
    assert_eq!(spool(&storage, &b.id).slot_id.as_deref(), Some("slt-x"));
}

/// 4. Wrong occupant: `expected_occupant = None` on an occupied slot is an
///    `OccupancyConflict` naming A, and nothing changes.
#[test]
fn a_wrong_expected_occupant_is_an_occupancy_conflict_naming_the_current_occupant() {
    let (_temp, _lease, storage, _db) = storage();
    let (a, b) = swap_setup(&storage);
    let rows_before = movement_count(&storage);

    let error = apply(&storage, "op-wrong", &b, &to_slot("slt-x", None, None)).unwrap_err();

    match error {
        RepositoryError::OccupancyConflict {
            slot_id,
            current_occupant_spool_id,
        } => {
            assert_eq!(slot_id, "slt-x");
            assert_eq!(current_occupant_spool_id.as_deref(), Some(a.id.as_str()));
        }
        other => panic!("expected OccupancyConflict, got {other:?}"),
    }
    assert_eq!(spool(&storage, &a.id), a);
    assert_eq!(spool(&storage, &b.id), b);
    assert_eq!(movement_count(&storage), rows_before);

    // The converse: expecting an occupant in an empty slot also conflicts.
    seed_printer_with_slot(&storage, "prn-y", "slt-y");
    let error = apply(
        &storage,
        "op-wrong-2",
        &b,
        &to_slot("slt-y", Some(&a.id), None),
    )
    .unwrap_err();
    assert!(matches!(
        error,
        RepositoryError::OccupancyConflict {
            current_occupant_spool_id: None,
            ..
        }
    ));
}

/// 5. Concurrent race: two threads load different Spools into the same
///    empty slot with `expected_occupant = None`. Exactly one wins.
#[test]
fn two_concurrent_loads_into_one_empty_slot_have_exactly_one_winner() {
    let (_temp, _lease, storage, _db) = storage();
    seed_printer_with_slot(&storage, "prn-x", "slt-x");
    let spools = [new_spool(&storage), new_spool(&storage)];
    let barrier = Arc::new(Barrier::new(2));

    let handles: Vec<_> = spools
        .iter()
        .enumerate()
        .map(|(index, contender)| {
            let storage = Arc::clone(&storage);
            let barrier = Arc::clone(&barrier);
            let contender = contender.clone();
            thread::spawn(move || {
                barrier.wait();
                apply(
                    &storage,
                    &format!("op-race-{index}"),
                    &contender,
                    &to_slot("slt-x", None, None),
                )
            })
        })
        .collect();
    let results: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();

    let winners: Vec<usize> = (0..2).filter(|index| results[*index].is_ok()).collect();
    assert_eq!(winners.len(), 1, "exactly one load must win: {results:?}");
    let winner = &spools[winners[0]];
    let loser = 1 - winners[0];
    match &results[loser] {
        Err(RepositoryError::OccupancyConflict {
            slot_id,
            current_occupant_spool_id,
        }) => {
            assert_eq!(slot_id, "slt-x");
            assert_eq!(
                current_occupant_spool_id.as_deref(),
                Some(winner.id.as_str())
            );
        }
        other => panic!("the loser must see OccupancyConflict, got {other:?}"),
    }
    assert_eq!(
        spool(&storage, &winner.id).slot_id.as_deref(),
        Some("slt-x")
    );
    assert_eq!(spool(&storage, &spools[loser].id).slot_id, None);
    assert_eq!(movement_count(&storage), 1);
}

/// 6. Replay: the same `operation_id` again returns `replayed = true` and
///    writes nothing.
#[test]
fn replaying_an_operation_id_returns_the_recorded_outcome_and_writes_nothing() {
    let (_temp, _lease, storage, _db) = storage();
    let (a, b) = swap_setup(&storage);
    let first = apply(
        &storage,
        "op-swap",
        &b,
        &to_slot("slt-x", Some(&a.id), Some("Shelf")),
    )
    .unwrap();
    let rows_after_first = movement_count(&storage);
    let b_after = spool(&storage, &b.id);
    let printer_after = printer_revision(&storage, "prn-x");

    // Same request (now-stale revision included): a replay, not a conflict.
    let replay = apply(
        &storage,
        "op-swap",
        &b,
        &to_slot("slt-x", Some(&a.id), Some("Shelf")),
    )
    .unwrap();

    assert!(replay.replayed);
    assert_eq!(replay.spool_ids, first.spool_ids);
    assert_eq!(replay.printer_ids, first.printer_ids);
    assert_eq!(replay.movements, first.movements);
    assert_eq!(movement_count(&storage), rows_after_first);
    assert_eq!(spool(&storage, &b.id), b_after);
    assert_eq!(printer_revision(&storage, "prn-x"), printer_after);

    let found = storage
        .write(|tx| movement::find_operation(tx, "op-swap"))
        .unwrap()
        .unwrap();
    assert!(found.replayed);
    assert_eq!(found.movements, first.movements);
    assert!(storage
        .write(|tx| movement::find_operation(tx, "op-missing"))
        .unwrap()
        .is_none());
}

/// Fix round 1, Critical 1: `apply_moves` applies several moves under one
/// `operationId`, checking the replay ONCE for the whole batch — a plain
/// `apply_move` per load would find the first load's row on the second call
/// and silently replay it instead of applying the second move.
#[test]
fn apply_moves_applies_a_batch_under_one_operation_id_and_replays_the_whole_batch_together() {
    let (_temp, _lease, storage, _db) = storage();
    seed_printer_with_slot(&storage, "prn-x", "slt-x");
    seed_printer_with_slot(&storage, "prn-y", "slt-y");
    let a = new_spool(&storage);
    let b = new_spool(&storage);

    let moves = vec![
        MoveRequest {
            spool_id: a.id.clone(),
            expected_spool_revision: a.revision,
            destination: to_slot("slt-x", None, None),
            reason_override: None,
        },
        MoveRequest {
            spool_id: b.id.clone(),
            expected_spool_revision: b.revision,
            destination: to_slot("slt-y", None, None),
            reason_override: None,
        },
    ];

    let outcome = apply_batch(&storage, "op-batch", &moves).unwrap();

    assert!(!outcome.replayed);
    assert_eq!(outcome.movements.len(), 2);
    assert!(outcome
        .movements
        .iter()
        .all(|row| row.operation_id == "op-batch"));
    assert_eq!(spool(&storage, &a.id).slot_id.as_deref(), Some("slt-x"));
    assert_eq!(spool(&storage, &b.id).slot_id.as_deref(), Some("slt-y"));
    assert_eq!(printer_revision(&storage, "prn-x"), 2);
    assert_eq!(printer_revision(&storage, "prn-y"), 2);
    assert_eq!(movement_count(&storage), 2);

    // Replaying the same batch (the same `operationId`) writes nothing and
    // returns the same combined outcome.
    let rows_before = movement_count(&storage);
    let x_before = printer_revision(&storage, "prn-x");
    let y_before = printer_revision(&storage, "prn-y");
    let replay = apply_batch(&storage, "op-batch", &moves).unwrap();

    assert!(replay.replayed);
    assert_eq!(replay.spool_ids, outcome.spool_ids);
    assert_eq!(replay.printer_ids, outcome.printer_ids);
    assert_eq!(replay.movements, outcome.movements);
    assert_eq!(movement_count(&storage), rows_before);
    assert_eq!(printer_revision(&storage, "prn-x"), x_before);
    assert_eq!(printer_revision(&storage, "prn-y"), y_before);
    assert_eq!(spool(&storage, &a.id).slot_id.as_deref(), Some("slt-x"));
    assert_eq!(spool(&storage, &b.id).slot_id.as_deref(), Some("slt-y"));
}

fn assert_validation(result: Result<MoveOutcome, RepositoryError>, expected_field: &str) {
    match result {
        Err(RepositoryError::Validation { field_path }) => assert_eq!(field_path, expected_field),
        other => panic!("expected Validation at {expected_field}, got {other:?}"),
    }
}

/// 7. Rejected moves: stale revision, removed slot, archived Printer's
///    slot, the Spool's current slot, and an archived Spool.
#[test]
fn stale_revisions_unusable_slots_and_archived_spools_are_rejected() {
    let (_temp, _lease, storage, _db) = storage();
    seed_printer_with_slot(&storage, "prn-x", "slt-x");
    seed_printer_with_slot(&storage, "prn-removed", "slt-removed");
    seed_printer_with_slot(&storage, "prn-archived", "slt-archived");
    storage
        .write(|tx| {
            tx.execute(
                "UPDATE material_slots SET removed_at = '2026-01-02T00:00:00.000Z' WHERE id = 'slt-removed'",
                [],
            )?;
            tx.execute(
                "UPDATE printers SET archived_at = '2026-01-02T00:00:00.000Z' WHERE id = 'prn-archived'",
                [],
            )?;
            Ok(())
        })
        .unwrap();
    let a = new_spool(&storage);

    // Stale expected revision.
    let stale = storage.write_repo(|tx| {
        movement::apply_move(
            tx,
            "op-stale",
            &a.id,
            a.revision + 1,
            &to_slot("slt-x", None, None),
            None,
        )
    });
    assert!(
        matches!(stale, Err(RepositoryError::Conflict { .. })),
        "{stale:?}"
    );

    assert_validation(
        apply(
            &storage,
            "op-removed",
            &a,
            &to_slot("slt-removed", None, None),
        ),
        "destination.slotId",
    );
    assert_validation(
        apply(
            &storage,
            "op-archived",
            &a,
            &to_slot("slt-archived", None, None),
        ),
        "destination.slotId",
    );
    assert_validation(
        apply(&storage, "op-unknown", &a, &to_slot("slt-nope", None, None)),
        "destination.slotId",
    );

    apply(&storage, "op-load", &a, &to_slot("slt-x", None, None)).unwrap();
    let loaded = spool(&storage, &a.id);
    assert_validation(
        apply(
            &storage,
            "op-same",
            &loaded,
            &to_slot("slt-x", Some(&a.id), None),
        ),
        "destination.slotId",
    );

    // An archived Spool (in storage, as the schema requires) can't load.
    let archived = new_spool(&storage);
    storage
        .write(|tx| {
            tx.execute(
                "UPDATE spools SET lifecycle = 'archived', archived_from = 'active' WHERE id = ?1",
                [&archived.id],
            )?;
            Ok(())
        })
        .unwrap();
    seed_printer_with_slot(&storage, "prn-y", "slt-y");
    assert_validation(
        apply(
            &storage,
            "op-archived-spool",
            &archived,
            &to_slot("slt-y", None, None),
        ),
        "spoolId",
    );

    // None of the rejected moves wrote anything.
    assert_eq!(movement_count(&storage), 1);
    assert_eq!(spool(&storage, &a.id).revision, loaded.revision);
}

/// 8. Between Printers: a direct slot->slot move from X to Y writes one
///    `Load` row from X's slot to Y's slot, and both Printers are bumped.
#[test]
fn a_direct_move_between_printers_writes_one_load_row_and_bumps_both_printers() {
    let (_temp, _lease, storage, _db) = storage();
    seed_printer_with_slot(&storage, "prn-x", "slt-x");
    seed_printer_with_slot(&storage, "prn-y", "slt-y");
    let a = new_spool(&storage);
    apply(&storage, "op-load", &a, &to_slot("slt-x", None, None)).unwrap();
    let a = spool(&storage, &a.id);
    let x_before = printer_revision(&storage, "prn-x");
    let y_before = printer_revision(&storage, "prn-y");

    let outcome = apply(&storage, "op-move", &a, &to_slot("slt-y", None, None)).unwrap();

    assert_eq!(outcome.movements.len(), 1);
    let row = &outcome.movements[0];
    assert_eq!(row.reason, MovementReason::Load);
    assert_eq!(row.from.slot_id.as_deref(), Some("slt-x"));
    assert_eq!(row.from.printer_id.as_deref(), Some("prn-x"));
    assert_eq!(row.to.slot_id.as_deref(), Some("slt-y"));
    assert_eq!(row.to.printer_id.as_deref(), Some("prn-y"));
    assert_eq!(
        outcome.printer_ids,
        vec!["prn-x".to_string(), "prn-y".to_string()]
    );
    assert_eq!(printer_revision(&storage, "prn-x"), x_before + 1);
    assert_eq!(printer_revision(&storage, "prn-y"), y_before + 1);
    assert_eq!(spool(&storage, &a.id).slot_id.as_deref(), Some("slt-y"));
}

/// The reason table's remaining rows: slot -> storage is `Unload`, storage
/// -> storage is `Relocate`, and `reason_override` replaces the derived
/// reason (used by archive dispositions and `markEmpty`).
#[test]
fn unload_relocate_and_reason_override_record_the_right_reason() {
    let (_temp, _lease, storage, _db) = storage();
    seed_printer_with_slot(&storage, "prn-x", "slt-x");
    let a = new_spool(&storage);

    let relocate = apply(
        &storage,
        "op-relocate",
        &a,
        &MoveDestination::Storage {
            storage_label: Some("Drybox".to_string()),
        },
    )
    .unwrap();
    assert_eq!(relocate.movements[0].reason, MovementReason::Relocate);
    assert_eq!(
        relocate.movements[0].to.storage_label.as_deref(),
        Some("Drybox")
    );
    assert!(relocate.printer_ids.is_empty());

    let a = spool(&storage, &a.id);
    apply(&storage, "op-load", &a, &to_slot("slt-x", None, None)).unwrap();
    let a = spool(&storage, &a.id);
    let unload = apply(
        &storage,
        "op-unload",
        &a,
        &MoveDestination::Storage {
            storage_label: None,
        },
    )
    .unwrap();
    assert_eq!(unload.movements[0].reason, MovementReason::Unload);
    assert_eq!(unload.printer_ids, vec!["prn-x".to_string()]);

    let a = spool(&storage, &a.id);
    apply(&storage, "op-load-2", &a, &to_slot("slt-x", None, None)).unwrap();
    let a = spool(&storage, &a.id);
    let consumed = storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-consumed",
                &a.id,
                a.revision,
                &MoveDestination::Storage {
                    storage_label: None,
                },
                Some(MovementReason::Consumed),
            )
        })
        .unwrap();
    assert_eq!(consumed.movements[0].reason, MovementReason::Consumed);

    let history = storage.write(|tx| movement::history(tx, &a.id)).unwrap();
    let reasons: Vec<_> = history.iter().map(|row| row.reason).collect();
    assert_eq!(
        reasons,
        vec![
            MovementReason::Relocate,
            MovementReason::Load,
            MovementReason::Unload,
            MovementReason::Load,
            MovementReason::Consumed,
        ]
    );
}

/// The wire shapes: `MoveDestination` is tagged by `kind`, camelCase.
#[test]
fn move_destination_and_movement_wire_shapes_are_camel_case() {
    let dest: MoveDestination = serde_json::from_value(serde_json::json!({
        "kind": "slot",
        "slotId": "slt-x",
        "expectedOccupantSpoolId": null,
        "displacedStorageLabel": "Shelf"
    }))
    .unwrap();
    assert_eq!(dest, to_slot("slt-x", None, Some("Shelf")));

    let storage_dest: MoveDestination =
        serde_json::from_value(serde_json::json!({ "kind": "storage" })).unwrap();
    assert_eq!(
        storage_dest,
        MoveDestination::Storage {
            storage_label: None
        }
    );

    assert_eq!(
        serde_json::to_value(MovementReason::PrinterArchived).unwrap(),
        serde_json::json!("printerArchived")
    );
}
