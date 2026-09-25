//! Migration 0007 (P6 `host_operations`). Covers: a fresh database's ledger
//! row, the v6->v7 upgrade (Printers, Spools, Library, slicing, and the P5
//! operations ledger unchanged, plus a restart), every `CHECK` the
//! migration adds, the partial unique index (at most one unresolved row
//! per Printer), the `BEFORE UPDATE` terminal-row trigger, the `RESTRICT`/
//! `SET NULL` foreign keys, the migration's crash-boundary behaviour, and
//! that no column can hold a credential. See spec D2.

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::{apply_through, apply_through_failing_before_commit};
use farm3d_lib::persistence::{
    MetadataRootLease, Storage, StorageError, StoragePaths, CURRENT_SCHEMA_VERSION,
};

const NOW: &str = "2026-01-01T00:00:00.000Z";
const ENDPOINT: &str = r#"{"kind":"moonraker","host":"192.0.2.1","port":7125}"#;
const GCODE_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";

fn open_storage() -> (tempfile::TempDir, StoragePaths, MetadataRootLease, Storage) {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let storage = Storage::open(paths.clone(), &lease).expect("storage");
    (temp, paths, lease, storage)
}

/// A raw connection with foreign keys on, so `CHECK`, trigger, and FK
/// failures keep their SQLite message (`StorageError::from` collapses them).
fn raw_connection(paths: &StoragePaths) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(paths.database()).expect("raw connection");
    connection
        .execute_batch("PRAGMA foreign_keys = ON")
        .expect("foreign keys on");
    connection
}

/// A migrated (v7) database's raw connection, for the constraint tests.
fn migrated() -> (tempfile::TempDir, rusqlite::Connection) {
    let (temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);
    (temp, connection)
}

/// A v6-only database (migrations 1-6) at `paths.database()`.
fn v6_database(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v6 database");
    apply_through(&mut connection, 6).expect("v6 migrations");
    connection
}

fn exec(connection: &rusqlite::Connection, sql: &str) {
    connection
        .execute_batch(sql)
        .unwrap_or_else(|error| panic!("seed failed: {error}\n{sql}"));
}

fn count(connection: &rusqlite::Connection, sql: &str) -> i64 {
    connection
        .query_row(sql, [], |row| row.get(0))
        .expect("count")
}

fn assert_rejected(result: rusqlite::Result<usize>, what: &str) {
    assert!(result.is_err(), "{what} must be rejected");
}

/// Seeds Printer `id` (bare enough for the FK tests).
fn seed_printer(connection: &rusqlite::Connection, id: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES ('{id}', 1, 'Printer', '', '', '', '', '', '', '{{}}', '{NOW}', '{NOW}');"
        ),
    );
}

/// A minimal `upload`, `dispatching` row for Printer `printer_id`.
struct HostOperationRow<'a> {
    id: &'a str,
    operation_id: &'a str,
    printer_id: &'a str,
    kind: &'a str,
    state: &'a str,
    gcode_sha256: Option<&'a str>,
    gcode_size: Option<i64>,
    history_mark: Option<i64>,
    source_host_operation_id: Option<&'a str>,
    failure_json: Option<&'a str>,
}

impl<'a> HostOperationRow<'a> {
    fn upload(id: &'a str) -> Self {
        Self {
            id,
            operation_id: id,
            printer_id: "prn-a",
            kind: "upload",
            state: "dispatching",
            gcode_sha256: Some(GCODE_HASH),
            gcode_size: Some(200),
            history_mark: None,
            source_host_operation_id: None,
            failure_json: None,
        }
    }
}

fn insert_host_operation(
    connection: &rusqlite::Connection,
    row: &HostOperationRow<'_>,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO host_operations(
             id, operation_id, printer_id, kind, slice_revision_id, source_host_operation_id,
             gcode_sha256, gcode_size, host_path, history_mark, endpoint_json, state,
             failure_json, created_at
         ) VALUES (?1, ?2, ?3, ?4, NULL, ?5, ?6, ?7, 'farm3d/a.gcode', ?8, ?9, ?10, ?11, ?12)",
        rusqlite::params![
            row.id,
            row.operation_id,
            row.printer_id,
            row.kind,
            row.source_host_operation_id,
            row.gcode_sha256,
            row.gcode_size,
            row.history_mark,
            ENDPOINT,
            row.state,
            row.failure_json,
            NOW,
        ],
    )
}

/// 1. A fresh database reaches `CURRENT_SCHEMA_VERSION`, with a ledger row
///    for `0007_p6_host_operations` whose checksum matches the migration
///    SQL.
#[test]
fn fresh_database_records_the_v7_ledger_row_with_a_matching_checksum() {
    let (_temp, _paths, _lease, storage) = open_storage();

    let (version, name, checksum) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 7",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((version, name, checksum))
        })
        .expect("schema state");

    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    assert_eq!(name, "0007_p6_host_operations");
    let expected_checksum = format!(
        "{:x}",
        Sha256::digest(include_str!("../migrations/0007_p6_host_operations.sql").as_bytes())
    );
    assert_eq!(checksum, expected_checksum);
}

/// 2. Upgrading a v6 database keeps its Printers, Spools, and the P5
///    operations ledger rows unchanged; the rebuilt ledger accepts the six
///    P6 kinds and still rejects unknown ones; and the upgraded database
///    reopens (restart) without change.
#[test]
fn upgrading_v6_to_v7_keeps_every_existing_row_and_survives_a_restart() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v6_database(&paths);
        exec(
            &connection,
            &format!(
                "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                   catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                   overrides_json, created_at, updated_at)
                 VALUES ('prn-a', 3, 'Printer', '', '', '', '', '', '', '{{}}', '{NOW}', '{NOW}');
                 INSERT INTO operations(id, kind, request_digest, created_at) VALUES
                   ('op-move', 'moveSpool', 'digest-1', '{NOW}'),
                   ('op-start-slice', 'startSlice', 'digest-2', '{NOW}');"
            ),
        );
    }

    let storage = Storage::open(paths.clone(), &lease).expect("v7 storage");
    drop(storage);
    // Restart: a second open sees a current database and changes nothing.
    let storage = Storage::open(paths.clone(), &lease).expect("reopened storage");
    drop(storage);

    let connection = raw_connection(&paths);
    let printer: (String, i64, String) = connection
        .query_row(
            "SELECT id, revision, name FROM printers WHERE id = 'prn-a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("printer");
    assert_eq!(printer, ("prn-a".to_string(), 3, "Printer".to_string()));

    let ledger: Vec<(String, String)> = {
        let mut statement = connection
            .prepare("SELECT id, kind FROM operations ORDER BY id")
            .expect("prepare");
        statement
            .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("collect")
    };
    assert_eq!(
        ledger,
        vec![
            ("op-move".to_string(), "moveSpool".to_string()),
            ("op-start-slice".to_string(), "startSlice".to_string()),
        ]
    );

    for kind in [
        "stageSliceRevision",
        "startStagedArtifact",
        "pauseHostPrint",
        "resumeHostPrint",
        "cancelHostPrint",
        "abandonHostOperation",
    ] {
        connection
            .execute(
                "INSERT INTO operations(id, kind, request_digest, created_at)
                 VALUES (?1, ?1, 'd', ?2)",
                rusqlite::params![kind, NOW],
            )
            .unwrap_or_else(|error| panic!("{kind} must be accepted: {error}"));
    }
    assert_rejected(
        connection.execute(
            "INSERT INTO operations(id, kind, request_digest, created_at)
             VALUES ('op-bad', 'queueJob', 'd', ?1)",
            [NOW],
        ),
        "an unknown operation kind",
    );

    let versions: (i64, i64) = connection
        .query_row(
            "SELECT (SELECT user_version FROM pragma_user_version),
                    (SELECT COUNT(*) FROM schema_migrations)",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("versions");
    assert_eq!(versions, (CURRENT_SCHEMA_VERSION, CURRENT_SCHEMA_VERSION));
}

/// 3. `host_operations`' `CHECK`s: id prefix/length, `kind`, the
///    upload/start-only `gcode_sha256`/`gcode_size` pairing, the start-only
///    `history_mark`, `source_host_operation_id` restricted to `start`,
///    `endpoint_json`/`failure_json`/`resolution_json` JSON validity,
///    `state`, `no_longer_pending`, and `abandon_note`'s length.
#[test]
fn host_operation_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");

    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                id: "xyz-a",
                ..HostOperationRow::upload("xyz-a")
            },
        ),
        "a non-hop id",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                kind: "printing",
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "an unknown kind",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                gcode_sha256: None,
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "an upload without gcode_sha256",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                gcode_size: None,
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "an upload without gcode_size",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                gcode_size: Some(0),
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "gcode_size 0",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                kind: "pause",
                gcode_sha256: Some(GCODE_HASH),
                gcode_size: Some(200),
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "a pause with gcode_sha256/gcode_size set",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                kind: "start",
                gcode_sha256: Some(GCODE_HASH),
                gcode_size: Some(200),
                history_mark: None,
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "a start without history_mark",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                history_mark: Some(0),
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "an upload with history_mark set",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                source_host_operation_id: Some("hop-missing"),
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "an upload with source_host_operation_id set",
    );
    assert_rejected(
        insert_host_operation(
            &connection,
            &HostOperationRow {
                state: "printing",
                ..HostOperationRow::upload("hop-a")
            },
        ),
        "an unknown state",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO host_operations(
                 id, operation_id, printer_id, kind, host_path, endpoint_json, state, created_at
             ) VALUES ('hop-b', 'hop-b', 'prn-a', 'pause', 'farm3d/a.gcode', 'not json', 'dispatching', ?1)",
            [NOW],
        ),
        "invalid endpoint_json",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO host_operations(
                 id, operation_id, printer_id, kind, host_path, endpoint_json, state, failure_json, created_at
             ) VALUES ('hop-b', 'hop-b', 'prn-a', 'pause', 'farm3d/a.gcode', ?1, 'failed', 'not json', ?2)",
            rusqlite::params![ENDPOINT, NOW],
        ),
        "invalid failure_json",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO host_operations(
                 id, operation_id, printer_id, kind, host_path, endpoint_json, state, no_longer_pending, created_at
             ) VALUES ('hop-b', 'hop-b', 'prn-a', 'pause', 'farm3d/a.gcode', ?1, 'dispatching', 2, ?2)",
            rusqlite::params![ENDPOINT, NOW],
        ),
        "a non-boolean no_longer_pending",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO host_operations(
                 id, operation_id, printer_id, kind, host_path, endpoint_json, state, abandoned_at, abandon_note, created_at
             ) VALUES ('hop-b', 'hop-b', 'prn-a', 'pause', 'farm3d/a.gcode', ?1, 'abandoned', ?2, ?3, ?2)",
            rusqlite::params![ENDPOINT, NOW, "x".repeat(501)],
        ),
        "an abandon_note over 500 characters",
    );

    insert_host_operation(&connection, &HostOperationRow::upload("hop-a")).expect("valid upload");
}

/// 4. The partial unique index: at most one unresolved (`dispatching`,
///    `uncertain`, `reconciling`) row per Printer. A second terminal row
///    for the same Printer, or an unresolved row for a different Printer,
///    is fine.
#[test]
fn only_one_unresolved_row_is_allowed_per_printer() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    seed_printer(&connection, "prn-b");

    insert_host_operation(&connection, &HostOperationRow::upload("hop-a"))
        .expect("first unresolved");
    assert_rejected(
        insert_host_operation(&connection, &HostOperationRow::upload("hop-b")),
        "a second unresolved row for the same printer",
    );
    insert_host_operation(
        &connection,
        &HostOperationRow {
            printer_id: "prn-b",
            ..HostOperationRow::upload("hop-c")
        },
    )
    .expect("an unresolved row for a different printer is fine");

    connection
        .execute(
            "UPDATE host_operations SET state = 'failed', failure_json = '{}' WHERE id = 'hop-a'",
            [],
        )
        .expect("resolve hop-a");
    insert_host_operation(&connection, &HostOperationRow::upload("hop-d"))
        .expect("a second unresolved row is fine once the first is resolved");
}

/// 5. The `BEFORE UPDATE` trigger stops any update to a terminal row
///    (`succeeded`, `failed`, `abandoned`), and allows one on a
///    non-terminal row.
#[test]
fn terminal_rows_reject_every_update_but_non_terminal_rows_accept_one() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    insert_host_operation(&connection, &HostOperationRow::upload("hop-a")).expect("insert");
    connection
        .execute(
            "UPDATE host_operations SET state = 'succeeded', resolution_json = '{}' WHERE id = 'hop-a'",
            [],
        )
        .expect("resolve");

    let error = connection
        .execute(
            "UPDATE host_operations SET abandon_note = 'x' WHERE id = 'hop-a'",
            [],
        )
        .expect_err("a terminal row must reject any update");
    assert!(
        error.to_string().contains("host operation is terminal"),
        "unexpected error: {error}"
    );

    insert_host_operation(&connection, &HostOperationRow::upload("hop-b")).expect("insert");
    connection
        .execute(
            "UPDATE host_operations SET attempts = attempts + 1 WHERE id = 'hop-b'",
            [],
        )
        .expect("a non-terminal row accepts an update");
}

/// 6. `RESTRICT`: a Printer with any Host Operation row (even terminal)
///    can't be deleted. `SET NULL`: `source_host_operation_id` clears when
///    the referenced row is deleted.
#[test]
fn printer_is_restricted_and_source_host_operation_id_clears_on_delete() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    insert_host_operation(&connection, &HostOperationRow::upload("hop-a")).expect("insert");
    connection
        .execute(
            "UPDATE host_operations SET state = 'failed', failure_json = '{}' WHERE id = 'hop-a'",
            [],
        )
        .expect("resolve");

    assert_rejected(
        connection.execute("DELETE FROM printers WHERE id = 'prn-a'", []),
        "deleting a Printer with a (terminal) Host Operation row",
    );

    insert_host_operation(
        &connection,
        &HostOperationRow {
            kind: "start",
            history_mark: Some(0),
            source_host_operation_id: Some("hop-a"),
            ..HostOperationRow::upload("hop-b")
        },
    )
    .expect("a start referencing the terminal upload row");

    connection
        .execute("DELETE FROM host_operations WHERE id = 'hop-a'", [])
        .expect("delete the referenced row");
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM host_operations WHERE id = 'hop-b' AND source_host_operation_id IS NULL"
        ),
        1,
        "source_host_operation_id must clear, not block the delete"
    );
}

/// 7. Crash boundary: a failure after the v7 migration's SQL and ledger
///    row but before commit leaves the database exactly as it was at v6,
///    including the old operations ledger and its rows.
#[test]
fn a_crash_before_commit_leaves_the_database_unchanged_at_v6() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let mut connection = v6_database(&paths);
    exec(
        &connection,
        &format!(
            "INSERT INTO operations(id, kind, request_digest, created_at)
             VALUES ('op-move', 'moveSpool', 'digest-1', '{NOW}');"
        ),
    );
    let operations_sql_before: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'operations'",
            [],
            |row| row.get(0),
        )
        .expect("operations sql");

    let error = apply_through_failing_before_commit(&mut connection, 7)
        .expect_err("the injected failure must surface");
    assert!(matches!(error, StorageError::MigrationFailed));

    assert_eq!(
        count(&connection, "SELECT user_version FROM pragma_user_version"),
        6,
        "the schema version must roll back to v6"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 7"
        ),
        0,
        "no v7 ledger row may survive the rollback"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_schema WHERE name IN ('host_operations', 'operations_p6')"
        ),
        0,
        "the v7 tables must roll back too"
    );
    let operations_sql_after: String = connection
        .query_row(
            "SELECT sql FROM sqlite_schema WHERE type = 'table' AND name = 'operations'",
            [],
            |row| row.get(0),
        )
        .expect("operations sql");
    assert_eq!(operations_sql_after, operations_sql_before);
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM operations WHERE id = 'op-move'"
        ),
        1
    );
}

/// 8. No `host_operations` column name contains `credential`, `secret`, or
///    `key` (global constraint 3: credentials never enter this table).
#[test]
fn no_host_operations_column_can_hold_a_credential() {
    let (_temp, connection) = migrated();
    let mut statement = connection
        .prepare("SELECT name FROM pragma_table_info('host_operations')")
        .expect("prepare");
    let columns: Vec<String> = statement
        .query_map([], |row| row.get(0))
        .expect("query")
        .collect::<rusqlite::Result<_>>()
        .expect("collect");

    assert!(!columns.is_empty(), "host_operations must have columns");
    for column in &columns {
        let lower = column.to_lowercase();
        for forbidden in ["credential", "secret", "key"] {
            assert!(
                !lower.contains(forbidden),
                "column {column} must not look like it holds a {forbidden}"
            );
        }
    }
}
