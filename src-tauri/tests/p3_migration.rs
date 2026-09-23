//! Migration 0004 (P3 spools and material slots). Covers: the v3->v4
//! upgrade and its `Main`-slot backfill, the occupancy/lifecycle CHECK
//! constraints (raw SQL, no Rust checks), the amount ledger's append-only
//! triggers, the Printer-delete cascade, and the migration's
//! crash-boundary behaviour. See "Migration 0004" in the P3 design spec
//! and Step 2 of the task-1 brief.

use std::fs;

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::{apply_through, apply_through_failing_before_commit};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StorageError, StoragePaths};

fn open_storage() -> (tempfile::TempDir, StoragePaths, MetadataRootLease, Storage) {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let storage = Storage::open(paths.clone(), &lease).expect("storage");
    (temp, paths, lease, storage)
}

/// Builds a v3-only database (migrations 1-3, via `apply_through`) at
/// `paths.database()` and returns the raw connection so the caller can seed
/// v3-shaped Printer rows before ever letting `Storage::open` see it.
fn v3_database(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v3 database");
    apply_through(&mut connection, 3).expect("v3 migrations");
    connection
}

fn insert_v3_printer(connection: &rusqlite::Connection, id: &str, created_at: &str, archived: bool) {
    let archived_at = archived.then(|| created_at.to_string());
    connection
        .execute(
            "INSERT INTO printers(
                id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                catalog_model_id, catalog_printer_variant, notes, overrides_json,
                created_at, updated_at, archived_at
             ) VALUES (?1, 1, 'Printer', '', '', '', '', '', '', '{}', ?2, ?2, ?3)",
            rusqlite::params![id, created_at, archived_at],
        )
        .expect("insert v3 printer");
}

/// Seeds a Printer (`printer_id`) and one live slot (`slot_id`, position 0,
/// named "Main") directly with raw SQL — Task 1 has no Rust API for either
/// yet, and the occupancy tests below are deliberately raw SQL anyway.
fn seed_printer_and_slot(transaction: &rusqlite::Transaction<'_>, printer_id: &str, slot_id: &str) {
    transaction
        .execute(
            "INSERT INTO printers(
                id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                catalog_model_id, catalog_printer_variant, notes, overrides_json,
                created_at, updated_at
             ) VALUES (?1, 1, 'Printer', '', '', '', '', '', '', '{}', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
            [printer_id],
        )
        .expect("seed printer");
    transaction
        .execute(
            "INSERT INTO material_slots(id, printer_id, position, name, created_at)
             VALUES (?1, ?2, 0, 'Main', '2026-01-01T00:00:00.000Z')",
            rusqlite::params![slot_id, printer_id],
        )
        .expect("seed slot");
}

#[allow(clippy::too_many_arguments)]
fn insert_spool(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
    spool_number: i64,
    material_family: &str,
    material_other: Option<&str>,
    lifecycle: &str,
    archived_from: Option<&str>,
    slot_id: Option<&str>,
    storage_label: Option<&str>,
) -> rusqlite::Result<usize> {
    transaction.execute(
        "INSERT INTO spools(
            id, revision, spool_number, manufacturer, material_family, material_other,
            color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
            archived_from, slot_id, storage_label, created_at, updated_at
         ) VALUES (?1, 1, ?2, 'Polymaker', ?3, ?4, 'Black', '1.75', 1000000, 1000000, 'measured', ?5, ?6, ?7, ?8,
                   '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
        rusqlite::params![
            id,
            spool_number,
            material_family,
            material_other,
            lifecycle,
            archived_from,
            slot_id,
            storage_label,
        ],
    )
}

/// 1. Upgrade: after `Storage::open`, `user_version == 4`, the ledger has
///    the 0004 row, and each Printer (archived ones included) has exactly
///    one live slot `Main` at position 0 with a `slt-` id.
#[test]
fn upgrading_v3_reaches_v4_and_backfills_one_main_slot_per_printer() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v3_database(&paths);
        insert_v3_printer(&connection, "prn-active", "2026-01-01T00:00:00.000Z", false);
        insert_v3_printer(&connection, "prn-archived", "2026-01-02T00:00:00.000Z", true);
    }

    let storage = Storage::open(paths, &lease).expect("v4 storage");

    let (version, ledger_name, ledger_checksum, slots) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 4",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            let mut statement = connection.prepare(
                "SELECT printer_id, id, position, name, removed_at FROM material_slots
                 ORDER BY printer_id",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, i64>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, Option<String>>(4)?,
                    ))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((version, name, checksum, rows))
        })
        .expect("post-upgrade state");

    assert_eq!(version, 4);
    assert_eq!(ledger_name, "0004_p3_spools_material_slots");
    let expected_checksum = format!(
        "{:x}",
        Sha256::digest(
            include_str!("../migrations/0004_p3_spools_material_slots.sql").as_bytes()
        )
    );
    assert_eq!(ledger_checksum, expected_checksum);

    assert_eq!(slots.len(), 2, "every printer must get exactly one slot");
    let printer_ids: Vec<&str> = slots.iter().map(|(id, ..)| id.as_str()).collect();
    assert!(printer_ids.contains(&"prn-active"));
    assert!(printer_ids.contains(&"prn-archived"));
    for (_, slot_id, position, name, removed_at) in &slots {
        assert!(slot_id.starts_with("slt-"), "{slot_id} must be slt-<uuid>");
        assert_eq!(*position, 0);
        assert_eq!(name, "Main");
        assert!(removed_at.is_none());
    }
}

/// 2a. Two Spools cannot occupy the same slot.
#[test]
fn two_spools_cannot_share_one_slot() {
    let (_temp, _paths, _lease, storage) = open_storage();
    storage
        .write(|tx| {
            seed_printer_and_slot(tx, "prn-a", "slt-a");
            insert_spool(tx, "spl-1", 1, "PLA", None, "active", None, Some("slt-a"), None)?;
            Ok(())
        })
        .expect("first spool loads into the slot");

    let error = storage
        .write(|tx| {
            insert_spool(tx, "spl-2", 2, "PLA", None, "active", None, Some("slt-a"), None)?;
            Ok(())
        })
        .expect_err("a second spool in the same slot must be rejected");
    assert!(matches!(error, StorageError::Database));
}

/// 2b. A Spool cannot have both a `slotId` and a `storageLabel`.
#[test]
fn a_spool_cannot_have_both_a_slot_and_a_storage_label() {
    let (_temp, _paths, _lease, storage) = open_storage();
    storage
        .write(|tx| {
            seed_printer_and_slot(tx, "prn-a", "slt-a");
            Ok(())
        })
        .expect("seed printer and slot");

    let error = storage
        .write(|tx| {
            insert_spool(
                tx,
                "spl-1",
                1,
                "PLA",
                None,
                "active",
                None,
                Some("slt-a"),
                Some("Shelf 1"),
            )?;
            Ok(())
        })
        .expect_err("slot and storage label together must be rejected");
    assert!(matches!(error, StorageError::Database));
}

/// 2c. An archived Spool cannot keep a `slotId`.
#[test]
fn an_archived_spool_cannot_keep_a_slot() {
    let (_temp, _paths, _lease, storage) = open_storage();
    storage
        .write(|tx| {
            seed_printer_and_slot(tx, "prn-a", "slt-a");
            Ok(())
        })
        .expect("seed printer and slot");

    let error = storage
        .write(|tx| {
            insert_spool(
                tx,
                "spl-1",
                1,
                "PLA",
                None,
                "archived",
                Some("active"),
                Some("slt-a"),
                None,
            )?;
            Ok(())
        })
        .expect_err("an archived spool with a slot must be rejected");
    assert!(matches!(error, StorageError::Database));
}

/// 2d. `OTHER` without `materialOther` is rejected.
#[test]
fn other_family_without_material_other_is_rejected() {
    let (_temp, _paths, _lease, storage) = open_storage();

    let error = storage
        .write(|tx| {
            insert_spool(tx, "spl-1", 1, "OTHER", None, "active", None, None, Some("Shelf 1"))?;
            Ok(())
        })
        .expect_err("OTHER without materialOther must be rejected");
    assert!(matches!(error, StorageError::Database));
}

/// 3. `spool_amount_events` is append-only: UPDATE and DELETE both fail
///    with the trigger's "append-only" message.
#[test]
fn spool_amount_events_reject_update_and_delete_as_append_only() {
    let (_temp, paths, _lease, storage) = open_storage();
    storage
        .write(|tx| {
            insert_spool(tx, "spl-1", 1, "PLA", None, "active", None, None, Some("Shelf 1"))?;
            tx.execute(
                "INSERT INTO spool_amount_events(
                    id, spool_id, sequence, kind, before_mg, after_mg, confidence_after, occurred_at
                 ) VALUES ('evt-1', 'spl-1', 1, 'initial', NULL, 1000000, 'measured', '2026-01-01T00:00:00.000Z')",
                [],
            )?;
            Ok(())
        })
        .expect("seed spool and ledger row");

    let connection = rusqlite::Connection::open(paths.database()).expect("raw connection");
    let update_error = connection
        .execute("UPDATE spool_amount_events SET after_mg = 0 WHERE id = 'evt-1'", [])
        .expect_err("update must be rejected");
    assert!(update_error.to_string().contains("append-only"));

    let delete_error = connection
        .execute("DELETE FROM spool_amount_events WHERE id = 'evt-1'", [])
        .expect_err("delete must be rejected");
    assert!(delete_error.to_string().contains("append-only"));
}

/// 4. Cascade: deleting a Printer removes its `material_slots` and every
///    `spool_movements` row whose `from_slot_id`/`to_slot_id` references
///    them, and `PRAGMA foreign_key_check` is then empty. The Spool itself
///    (in storage, not occupying the slot) survives untouched.
#[test]
fn deleting_a_printer_cascades_its_slots_and_touching_movements() {
    let (_temp, _paths, _lease, storage) = open_storage();
    storage
        .write(|tx| {
            seed_printer_and_slot(tx, "prn-a", "slt-a");
            insert_spool(tx, "spl-1", 1, "PLA", None, "active", None, None, Some("Shelf 1"))?;
            tx.execute(
                "INSERT INTO spool_movements(
                    id, operation_id, spool_id, reason, from_slot_id, to_slot_id, occurred_at
                 ) VALUES ('mov-1', 'op-1', 'spl-1', 'unload', 'slt-a', NULL, '2026-01-01T00:00:00.000Z')",
                [],
            )?;
            tx.execute(
                "INSERT INTO spool_movements(
                    id, operation_id, spool_id, reason, from_slot_id, to_slot_id, occurred_at
                 ) VALUES ('mov-2', 'op-2', 'spl-1', 'load', NULL, 'slt-a', '2026-01-01T00:00:01.000Z')",
                [],
            )?;
            Ok(())
        })
        .expect("seed printer, slot, spool, and movements");

    storage
        .write(|tx| {
            tx.execute("DELETE FROM printers WHERE id = 'prn-a'", [])?;
            Ok(())
        })
        .expect("delete the printer");

    let (slot_count, movement_count, spool_count, fk_violations) = storage
        .read(|connection| {
            let slot_count: i64 = connection.query_row(
                "SELECT count(*) FROM material_slots WHERE printer_id = 'prn-a'",
                [],
                |row| row.get(0),
            )?;
            let movement_count: i64 = connection.query_row(
                "SELECT count(*) FROM spool_movements WHERE id IN ('mov-1', 'mov-2')",
                [],
                |row| row.get(0),
            )?;
            let spool_count: i64 =
                connection.query_row("SELECT count(*) FROM spools WHERE id = 'spl-1'", [], |row| {
                    row.get(0)
                })?;
            let fk_violations: i64 =
                connection.query_row("SELECT count(*) FROM pragma_foreign_key_check()", [], |row| {
                    row.get(0)
                })?;
            Ok((slot_count, movement_count, spool_count, fk_violations))
        })
        .expect("post-delete state");

    assert_eq!(slot_count, 0, "the printer's slot must be gone");
    assert_eq!(movement_count, 0, "movements touching the slot must be gone");
    assert_eq!(spool_count, 1, "the spool itself must survive the printer delete");
    assert_eq!(fk_violations, 0);
}

/// 5. A crash after the v4 migration's SQL/backfill/ledger row but before
///    commit must leave the database byte-identical to its pre-upgrade
///    (v3) state.
#[test]
fn a_crash_before_commit_leaves_the_database_byte_identical_at_v3() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let mut connection = v3_database(&paths);
    insert_v3_printer(&connection, "prn-a", "2026-01-01T00:00:00.000Z", false);
    insert_v3_printer(&connection, "prn-b", "2026-01-02T00:00:00.000Z", true);

    let before = fs::read(paths.database()).expect("pre-crash bytes");

    let error = apply_through_failing_before_commit(&mut connection, 4)
        .expect_err("the injected failure must surface");
    assert!(matches!(error, StorageError::MigrationFailed));
    drop(connection);

    let after = fs::read(paths.database()).expect("post-crash bytes");
    assert_eq!(before, after, "a rolled-back v4 upgrade must not touch the file");
}
