//! Migration 0003 (P2 printer lifecycle / host identity). Covers: a fresh
//! database's ledger row, the v2→v3 duplicate-host backfill/archive, the
//! post-upgrade unique-index backstop, the `location`/`start_safety` CHECK
//! constraints, and the migration's crash-boundary behaviour. See "Migration
//! 0003" in the P2 design spec and Step 3 of the task-1 brief.

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::{apply_through, apply_through_failing_before_commit};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};

fn open_storage() -> (tempfile::TempDir, StoragePaths, MetadataRootLease, Storage) {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let storage = Storage::open(paths.clone(), &lease).expect("storage");
    (temp, paths, lease, storage)
}

/// Builds a v2-only database (migrations 1-2 only, via `apply_through`) at
/// `paths.database()` and returns the raw connection so the caller can seed
/// v1/v2-shaped printer rows before ever letting `Storage::open` see it.
fn v2_database(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v2 database");
    apply_through(&mut connection, 2).expect("v2 migrations");
    connection
}

fn insert_v2_printer(
    connection: &rusqlite::Connection,
    id: &str,
    created_at: &str,
    host: &str,
    port: u16,
) {
    let connection_json = serde_json::json!({
        "kind": "moonraker",
        "host": host,
        "port": port,
        "useTls": false,
    })
    .to_string();
    connection
        .execute(
            "INSERT INTO printers(
                id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                catalog_model_id, catalog_printer_variant, notes, overrides_json,
                connection_json, created_at, updated_at
             ) VALUES (?1, 1, 'Printer', '', '', '', '', '', '', '{}', ?2, ?3, ?3)",
            rusqlite::params![id, connection_json, created_at],
        )
        .expect("insert v2 printer");
}

fn insert_v3_printer_with_identity(
    storage: &Storage,
    id: &str,
    created_at: &str,
    host_identity: &str,
) -> Result<(), farm3d_lib::persistence::StorageError> {
    storage.write(|transaction| {
        transaction.execute(
            "INSERT INTO printers(
                id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                catalog_model_id, catalog_printer_variant, notes, overrides_json,
                host_identity, created_at, updated_at
             ) VALUES (?1, 1, 'Printer', '', '', '', '', '', '', '{}', ?2, ?3, ?3)",
            rusqlite::params![id, host_identity, created_at],
        )?;
        Ok(())
    })
}

/// 1. A fresh database reaches the current schema version (later phases
///    bump it further, so this checks the ledger row rather than hardcoding
///    `PRAGMA user_version`), with a ledger row for `0003_p2_printer_lifecycle`
///    whose checksum matches the migration SQL on disk.
#[test]
fn fresh_database_records_the_v3_ledger_row_with_a_matching_checksum() {
    let (_temp, _paths, _lease, storage) = open_storage();

    let (version, name, checksum) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 3",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((version, name, checksum))
        })
        .expect("schema state");

    assert_eq!(version, farm3d_lib::persistence::CURRENT_SCHEMA_VERSION);
    assert_eq!(name, "0003_p2_printer_lifecycle");
    let expected_checksum = format!(
        "{:x}",
        Sha256::digest(include_str!("../migrations/0003_p2_printer_lifecycle.sql").as_bytes())
    );
    assert_eq!(checksum, expected_checksum);
}

/// 2. Upgrading a v2 database with two Printers on the same host archives
///    every Printer but the oldest, keeps their Connections, and records one
///    `DUPLICATE_HOST_ARCHIVED` warning.
#[test]
fn upgrading_v2_archives_every_duplicate_host_printer_but_the_oldest() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v2_database(&paths);
        insert_v2_printer(
            &connection,
            "prn-old",
            "2026-01-01T00:00:00.000Z",
            "Voron.local",
            7125,
        );
        insert_v2_printer(
            &connection,
            "prn-new",
            "2026-01-02T00:00:00.000Z",
            "voron.local.",
            7125,
        );
    }

    let storage = Storage::open(paths, &lease).expect("v3 storage");

    let (old_identity, old_archived, new_identity, new_archived, new_has_connection, warning_count) = storage
        .read(|connection| {
            let (old_identity, old_archived): (Option<String>, Option<String>) = connection
                .query_row(
                    "SELECT host_identity, archived_at FROM printers WHERE id = 'prn-old'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )?;
            let (new_identity, new_archived, new_has_connection): (Option<String>, Option<String>, bool) =
                connection.query_row(
                    "SELECT host_identity, archived_at, connection_json IS NOT NULL FROM printers WHERE id = 'prn-new'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
                )?;
            let warning_count: i64 = connection.query_row(
                "SELECT count(*) FROM migration_warnings WHERE code = 'DUPLICATE_HOST_ARCHIVED'",
                [],
                |row| row.get(0),
            )?;
            Ok((
                old_identity,
                old_archived,
                new_identity,
                new_archived,
                new_has_connection,
                warning_count,
            ))
        })
        .expect("post-upgrade state");

    assert_eq!(old_identity.as_deref(), Some("voron.local:7125"));
    assert_eq!(old_archived, None, "the older printer must stay active");
    assert_eq!(new_identity.as_deref(), Some("voron.local:7125"));
    assert!(
        new_archived.is_some(),
        "the newer duplicate must be archived"
    );
    assert!(
        new_has_connection,
        "the archived printer keeps its connection"
    );
    assert_eq!(warning_count, 1);
}

/// The UI reads the migration's duplicate-host archives back through the
/// repository as (archived, kept) Printer id pairs.
#[test]
fn duplicate_host_migration_archives_are_listed_for_the_ui() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v2_database(&paths);
        insert_v2_printer(
            &connection,
            "prn-old",
            "2026-01-01T00:00:00.000Z",
            "Voron.local",
            7125,
        );
        insert_v2_printer(
            &connection,
            "prn-new",
            "2026-01-02T00:00:00.000Z",
            "voron.local.",
            7125,
        );
        insert_v2_printer(
            &connection,
            "prn-solo",
            "2026-01-03T00:00:00.000Z",
            "solo.local",
            7125,
        );
    }

    let storage = std::sync::Arc::new(Storage::open(paths, &lease).expect("v3 storage"));
    let archives = farm3d_lib::printers::host_identity::duplicate_host_archives(&storage)
        .expect("archives read");

    assert_eq!(archives.len(), 1);
    assert_eq!(archives[0].archived_printer_id, "prn-new");
    assert_eq!(archives[0].kept_printer_id, "prn-old");
    assert!(!archives[0].warning_id.is_empty());
}

/// 3. After the v2→v3 upgrade, the partial unique index rejects a third
///    active Printer on the same host identity.
#[test]
fn after_upgrade_a_third_active_printer_on_the_same_host_conflicts() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v2_database(&paths);
        insert_v2_printer(
            &connection,
            "prn-old",
            "2026-01-01T00:00:00.000Z",
            "voron.local",
            7125,
        );
    }
    let storage = Storage::open(paths, &lease).expect("v3 storage");

    let error = insert_v3_printer_with_identity(
        &storage,
        "prn-third",
        "2026-01-03T00:00:00.000Z",
        "voron.local:7125",
    )
    .expect_err("the partial unique index must reject a second active host");

    assert!(matches!(
        error,
        farm3d_lib::persistence::StorageError::Database
    ));
}

/// 4. The 0003 CHECK constraints reject an untrimmed `location` and an
///    out-of-enum `start_safety`.
#[test]
fn location_and_start_safety_check_constraints_reject_invalid_values() {
    let (_temp, _paths, _lease, storage) = open_storage();
    storage
        .write(|transaction| {
            transaction.execute(
                "INSERT INTO printers(
                    id, revision, name, catalog_vendor, catalog_model, catalog_variant,
                    catalog_model_id, catalog_printer_variant, notes, overrides_json,
                    created_at, updated_at
                 ) VALUES ('prn-a', 1, 'Printer', '', '', '', '', '', '', '{}', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                [],
            )?;
            Ok(())
        })
        .expect("seed printer");

    let location_error = storage.write(|transaction| {
        transaction.execute("UPDATE printers SET location = ' x' WHERE id = 'prn-a'", [])?;
        Ok(())
    });
    assert!(
        location_error.is_err(),
        "untrimmed location must be rejected"
    );

    let start_safety_error = storage.write(|transaction| {
        transaction.execute(
            "UPDATE printers SET start_safety = 'yolo' WHERE id = 'prn-a'",
            [],
        )?;
        Ok(())
    });
    assert!(
        start_safety_error.is_err(),
        "an out-of-enum start_safety must be rejected"
    );
}

/// 5. A crash after the v3 migration's SQL/backfill/ledger row but before
///    commit must leave the database exactly as it was at v2.
#[test]
fn a_crash_before_commit_leaves_the_database_unchanged_at_v2() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let mut connection = v2_database(&paths);
    insert_v2_printer(
        &connection,
        "prn-a",
        "2026-01-01T00:00:00.000Z",
        "voron.local",
        7125,
    );

    let error = apply_through_failing_before_commit(&mut connection, 3)
        .expect_err("the injected failure must surface");
    assert!(matches!(
        error,
        farm3d_lib::persistence::StorageError::MigrationFailed
    ));

    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .expect("user_version");
    assert_eq!(user_version, 2, "the schema version must roll back to v2");

    let v3_ledger_rows: i64 = connection
        .query_row(
            "SELECT count(*) FROM schema_migrations WHERE version = 3",
            [],
            |row| row.get(0),
        )
        .expect("ledger rows");
    assert_eq!(
        v3_ledger_rows, 0,
        "no v3 ledger row must survive the rollback"
    );

    let has_location_column: bool = connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM pragma_table_info('printers') WHERE name = 'location')",
            [],
            |row| row.get(0),
        )
        .expect("column check");
    assert!(
        !has_location_column,
        "the v3 ALTER TABLE columns must roll back too"
    );
}
