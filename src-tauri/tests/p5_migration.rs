//! Migration 0006 (P5 runtime slicing and Slice Revisions). Covers: a fresh
//! database's ledger row, the v5->v6 upgrade (Printers, Spools, Library
//! Models, their revisions, and the P3 operations ledger unchanged, plus a
//! restart), every `CHECK` the migration adds, the UPDATE-only
//! immutability triggers, the `RESTRICT` and `CASCADE` foreign keys, and
//! the migration's crash-boundary behaviour. See spec D14.

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::{apply_through, apply_through_failing_before_commit};
use farm3d_lib::persistence::{
    MetadataRootLease, Storage, StorageError, StoragePaths, CURRENT_SCHEMA_VERSION,
};

const NOW: &str = "2026-01-01T00:00:00.000Z";
const SOURCE_HASH: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";
const GCODE_HASH: &str = "bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb";
const LOG_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

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

/// A migrated database's raw connection, for the constraint tests.
fn migrated() -> (tempfile::TempDir, rusqlite::Connection) {
    let (temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);
    (temp, connection)
}

/// A v5-only database (migrations 1-5) at `paths.database()`.
fn v5_database(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v5 database");
    apply_through(&mut connection, 5).expect("v5 migrations");
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

/// One managed STL Model (`mdl-a`) with one revision (`msr-a`) over
/// `SOURCE_HASH`, plus the G-code and log blobs a revision refers to.
fn seed_model(connection: &rusqlite::Connection) {
    exec(
        connection,
        &format!(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at) VALUES
               ('{SOURCE_HASH}', 100, '{NOW}'),
               ('{GCODE_HASH}', 200, '{NOW}'),
               ('{LOG_HASH}', 10, '{NOW}');
             INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
               VALUES ('mdl-a', 1, 'Model', 'stl', 'managed', '{NOW}', '{NOW}');
             INSERT INTO model_source_revisions(
               id, model_id, sequence, content_sha256, size_bytes, format, origin,
               source_file_name, source_path, captured_at, inspector_version, inspection_json
             ) VALUES ('msr-a', 'mdl-a', 1, '{SOURCE_HASH}', 100, 'stl', 'import', 'model.stl',
                       '/tmp/model.stl', '{NOW}', 1, '{{}}');"
        ),
    );
}

fn insert_preparation(connection: &rusqlite::Connection) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO slice_preparations(id, model_id, source_revision_id, revision, document_json,
                                        created_at, updated_at)
         VALUES ('prp-a', 'mdl-a', 'msr-a', 1, '{}', ?1, ?1)",
        [NOW],
    )
}

fn insert_operation(
    connection: &rusqlite::Connection,
    id: &str,
    state: &str,
    failure_json: Option<&str>,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO slice_operations(id, preparation_id, source_revision_id, plate_key,
                                      plate_snapshot_json, state, failure_json, queued_at)
         VALUES (?1, 'prp-a', 'msr-a', 'plate-1', '{}', ?2, ?3, ?4)",
        rusqlite::params![id, state, failure_json, NOW],
    )
}

struct RevisionRow<'a> {
    id: &'a str,
    kind: &'a str,
    plate_key: Option<&'a str>,
    plate_index: Option<i64>,
    plate_name: Option<&'a str>,
    runtime_json: Option<&'a str>,
    gcode_size: i64,
    requires_manual: i64,
}

impl<'a> RevisionRow<'a> {
    fn farm3d(id: &'a str) -> Self {
        Self {
            id,
            kind: "farm3d",
            plate_key: Some("plate-1"),
            plate_index: Some(1),
            plate_name: Some("Plate 1"),
            runtime_json: Some("{}"),
            gcode_size: 200,
            requires_manual: 0,
        }
    }

    fn external(id: &'a str) -> Self {
        Self {
            id,
            kind: "external",
            plate_key: None,
            plate_index: None,
            plate_name: None,
            runtime_json: None,
            gcode_size: 200,
            requires_manual: 1,
        }
    }
}

fn insert_revision(
    connection: &rusqlite::Connection,
    row: &RevisionRow<'_>,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, plate_key, plate_index,
                                     plate_name, gcode_sha256, gcode_size, target_json, facts_json,
                                     requires_manual_printer_selection, estimates_json,
                                     runtime_json, created_at)
         VALUES (?1, ?2, 'mdl-a', 'msr-a', ?3, ?4, ?5, ?6, ?7, '{}', '{}', ?8, '{}', ?9, ?10)",
        rusqlite::params![
            row.id,
            row.kind,
            row.plate_key,
            row.plate_index,
            row.plate_name,
            GCODE_HASH,
            row.gcode_size,
            row.requires_manual,
            row.runtime_json,
            NOW,
        ],
    )
}

fn insert_revision_blob(
    connection: &rusqlite::Connection,
    revision_id: &str,
    role: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO slice_revision_blobs(revision_id, role, sha256) VALUES (?1, ?2, ?3)",
        rusqlite::params![revision_id, role, LOG_HASH],
    )
}

fn assert_rejected(result: rusqlite::Result<usize>, what: &str) {
    assert!(result.is_err(), "{what} must be rejected");
}

/// 1. A fresh database reaches `CURRENT_SCHEMA_VERSION`, with a ledger row
///    for `0006_p5_slicing` whose checksum matches the migration SQL.
#[test]
fn fresh_database_records_the_v6_ledger_row_with_a_matching_checksum() {
    let (_temp, _paths, _lease, storage) = open_storage();

    let (version, name, checksum) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 6",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((version, name, checksum))
        })
        .expect("schema state");

    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    assert_eq!(name, "0006_p5_slicing");
    let expected_checksum = format!(
        "{:x}",
        Sha256::digest(include_str!("../migrations/0006_p5_slicing.sql").as_bytes())
    );
    assert_eq!(checksum, expected_checksum);
}

/// 2. Upgrading a v5 database keeps its Printers, Spools, Library Models,
///    Model Source Revisions, and operations ledger rows unchanged; the
///    rebuilt ledger accepts the two P5 kinds and still rejects unknown
///    ones; and the upgraded database reopens (restart) without change.
#[test]
fn upgrading_v5_to_v6_keeps_every_existing_row_and_survives_a_restart() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v5_database(&paths);
        exec(
            &connection,
            &format!(
                "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                   catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                   overrides_json, created_at, updated_at)
                 VALUES ('prn-a', 3, 'Printer', '', '', '', '', '', '', '{{}}', '{NOW}', '{NOW}');
                 INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
                   color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
                   created_at, updated_at)
                 VALUES ('spl-a', 2, 7, 'Polymaker', 'PLA', 'Black', '1.75', 1000000, 900000,
                   'measured', 'active', '{NOW}', '{NOW}');
                 INSERT INTO operations(id, kind, request_digest, created_at) VALUES
                   ('op-move', 'moveSpool', 'digest-1', '{NOW}'),
                   ('op-archive', 'archivePrinter', 'digest-2', '{NOW}'),
                   ('op-life', 'spoolLifecycle', 'digest-3', '{NOW}');"
            ),
        );
        seed_model(&connection);
    }

    let storage = Storage::open(paths.clone(), &lease).expect("v6 storage");
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
    let spool: (i64, i64, i64) = connection
        .query_row(
            "SELECT revision, spool_number, current_mg FROM spools WHERE id = 'spl-a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .expect("spool");
    assert_eq!(spool, (2, 7, 900000));
    let model: (String, String) = connection
        .query_row(
            "SELECT m.name, r.content_sha256 FROM library_models m
             JOIN model_source_revisions r ON r.model_id = m.id WHERE m.id = 'mdl-a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("model");
    assert_eq!(model, ("Model".to_string(), SOURCE_HASH.to_string()));

    let ledger: Vec<(String, String, String, String)> = {
        let mut statement = connection
            .prepare("SELECT id, kind, request_digest, created_at FROM operations ORDER BY id")
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("collect")
    };
    let row = |id: &str, kind: &str, digest: &str| {
        (
            id.to_string(),
            kind.to_string(),
            digest.to_string(),
            NOW.to_string(),
        )
    };
    assert_eq!(
        ledger,
        vec![
            row("op-archive", "archivePrinter", "digest-2"),
            row("op-life", "spoolLifecycle", "digest-3"),
            row("op-move", "moveSpool", "digest-1"),
        ]
    );

    for kind in ["startSlice", "createExternalSliceRevision"] {
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

/// 3a. `slicer_runtime_config` is a singleton with a positive revision and
///     non-empty paths.
#[test]
fn runtime_config_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    let insert = |singleton: i64, revision: i64, engine: Option<&str>, presets: Option<&str>| {
        connection.execute(
            "INSERT INTO slicer_runtime_config(singleton_id, revision, engine_path,
                                               preset_source_path, updated_at)
             VALUES (?1, ?2, ?3, ?4, ?5)",
            rusqlite::params![singleton, revision, engine, presets, NOW],
        )
    };

    assert_rejected(insert(2, 1, None, None), "a second singleton");
    assert_rejected(insert(1, 0, None, None), "revision 0");
    assert_rejected(insert(1, 1, Some(""), None), "an empty engine path");
    assert_rejected(insert(1, 1, None, Some("")), "an empty preset source path");
    insert(1, 1, Some("/opt/orca"), None).expect("a valid row");
}

/// 3b. `slice_preparations`: the id prefix, an object document, and one
///     Preparation per Model.
#[test]
fn preparation_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_model(&connection);
    let insert = |id: &str, document: &str| {
        connection.execute(
            "INSERT INTO slice_preparations(id, model_id, source_revision_id, revision,
                                            document_json, created_at, updated_at)
             VALUES (?1, 'mdl-a', 'msr-a', 1, ?2, ?3, ?3)",
            rusqlite::params![id, document, NOW],
        )
    };

    assert_rejected(insert("xyz-a", "{}"), "a non-prp id");
    assert_rejected(insert("prp-a", "[]"), "an array document");
    assert_rejected(insert("prp-a", "not json"), "invalid JSON");
    insert("prp-a", "{}").expect("a valid row");
    assert_rejected(insert("prp-b", "{}"), "a second Preparation for one Model");
}

/// 3c. `slice_operations`: the id prefix, the state list, and the pairing
///     of `failed` with `failure_json`.
#[test]
fn operation_checks_pair_failed_with_its_failure() {
    let (_temp, connection) = migrated();
    seed_model(&connection);
    insert_preparation(&connection).expect("preparation");

    assert_rejected(
        insert_operation(&connection, "xyz-a", "queued", None),
        "a non-sop id",
    );
    assert_rejected(
        insert_operation(&connection, "sop-a", "paused", None),
        "an unknown state",
    );
    assert_rejected(
        insert_operation(&connection, "sop-a", "failed", None),
        "failed without a failure",
    );
    assert_rejected(
        insert_operation(&connection, "sop-a", "queued", Some("{}")),
        "a failure on a queued operation",
    );
    assert_rejected(
        insert_operation(&connection, "sop-a", "failed", Some("not json")),
        "invalid failure JSON",
    );
    for (id, state, failure) in [
        ("sop-q", "queued", None),
        ("sop-r", "running", None),
        ("sop-f", "failed", Some("{}")),
        ("sop-c", "cancelled", None),
        ("sop-i", "interrupted", None),
    ] {
        insert_operation(&connection, id, state, failure)
            .unwrap_or_else(|error| panic!("{state} must be accepted: {error}"));
    }
    assert_rejected(
        insert_operation(&connection, "sop-s", "succeeded", None),
        "succeeded with neither a revision nor a finish time",
    );
}

/// 3d. `slice_revisions`: a farm3d revision needs a plate identity and a
///     runtime; an external revision has neither; plus the id prefix, kind,
///     size, and boolean checks.
#[test]
fn revision_checks_enforce_plate_and_runtime_exclusivity() {
    let (_temp, connection) = migrated();
    seed_model(&connection);

    let external_with_plate = RevisionRow {
        plate_key: Some("plate-1"),
        ..RevisionRow::external("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &external_with_plate),
        "an external revision with a plate",
    );
    let external_with_index = RevisionRow {
        plate_index: Some(1),
        ..RevisionRow::external("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &external_with_index),
        "an external revision with a plate index",
    );
    let external_with_name = RevisionRow {
        plate_name: Some("Plate 1"),
        ..RevisionRow::external("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &external_with_name),
        "an external revision with a plate name",
    );
    let external_with_runtime = RevisionRow {
        runtime_json: Some("{}"),
        ..RevisionRow::external("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &external_with_runtime),
        "an external revision with a runtime",
    );
    let farm3d_without_plate = RevisionRow {
        plate_key: None,
        ..RevisionRow::farm3d("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &farm3d_without_plate),
        "a farm3d revision without a plate key",
    );
    let farm3d_plate_zero = RevisionRow {
        plate_index: Some(0),
        ..RevisionRow::farm3d("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &farm3d_plate_zero),
        "a farm3d revision with plate index 0",
    );
    let farm3d_without_index = RevisionRow {
        plate_index: None,
        ..RevisionRow::farm3d("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &farm3d_without_index),
        "a farm3d revision without a plate index",
    );
    let farm3d_without_runtime = RevisionRow {
        runtime_json: None,
        ..RevisionRow::farm3d("slr-a")
    };
    assert_rejected(
        insert_revision(&connection, &farm3d_without_runtime),
        "a farm3d revision without a runtime",
    );
    assert_rejected(
        insert_revision(
            &connection,
            &RevisionRow {
                kind: "imported",
                ..RevisionRow::external("slr-a")
            },
        ),
        "an unknown kind",
    );
    assert_rejected(
        insert_revision(&connection, &RevisionRow::external("xyz-a")),
        "a non-slr id",
    );
    assert_rejected(
        insert_revision(
            &connection,
            &RevisionRow {
                gcode_size: 0,
                ..RevisionRow::external("slr-a")
            },
        ),
        "an empty G-code",
    );
    assert_rejected(
        insert_revision(
            &connection,
            &RevisionRow {
                requires_manual: 2,
                ..RevisionRow::external("slr-a")
            },
        ),
        "a non-boolean requires_manual_printer_selection",
    );

    insert_revision(&connection, &RevisionRow::farm3d("slr-farm")).expect("farm3d revision");
    insert_revision(
        &connection,
        &RevisionRow {
            plate_name: None,
            ..RevisionRow::farm3d("slr-unnamed")
        },
    )
    .expect("a farm3d revision without a plate name");
    insert_revision(&connection, &RevisionRow::external("slr-ext")).expect("external revision");

    assert_rejected(
        insert_revision_blob(&connection, "slr-farm", "thumbnail"),
        "an unknown blob role",
    );
    for role in [
        "plate3mf",
        "machinePreset",
        "processPreset",
        "filamentPreset",
        "manifest",
        "log",
    ] {
        insert_revision_blob(&connection, "slr-farm", role)
            .unwrap_or_else(|error| panic!("{role} must be accepted: {error}"));
    }
    assert_rejected(
        insert_revision_blob(&connection, "slr-farm", "log"),
        "a second blob for one role",
    );
}

/// 4. Neither `slice_revisions` nor `slice_revision_blobs` accepts an
///    UPDATE. A DELETE is allowed (deletion is guarded in Rust by the
///    blocker registry, D14): it cascades to the revision's blobs and
///    clears the operation's `slice_revision_id`.
#[test]
fn revisions_and_their_blobs_reject_updates_but_allow_a_guarded_delete() {
    let (_temp, connection) = migrated();
    seed_model(&connection);
    insert_preparation(&connection).expect("preparation");
    insert_revision(&connection, &RevisionRow::farm3d("slr-a")).expect("revision");
    insert_revision_blob(&connection, "slr-a", "log").expect("blob");
    connection
        .execute(
            "INSERT INTO slice_operations(id, preparation_id, source_revision_id, plate_key,
                                          plate_snapshot_json, state, slice_revision_id,
                                          queued_at, finished_at)
             VALUES ('sop-a', 'prp-a', 'msr-a', 'plate-1', '{}', 'succeeded', 'slr-a', ?1, ?1)",
            [NOW],
        )
        .expect("succeeded operation");

    let error = connection
        .execute(
            "UPDATE slice_revisions SET plate_name = 'x' WHERE id = 'slr-a'",
            [],
        )
        .expect_err("revisions are immutable");
    assert!(
        error.to_string().contains("slice revisions are immutable"),
        "unexpected error: {error}"
    );
    let error = connection
        .execute(
            "UPDATE slice_revision_blobs SET role = 'manifest' WHERE revision_id = 'slr-a'",
            [],
        )
        .expect_err("revision blobs are immutable");
    assert!(
        error
            .to_string()
            .contains("slice revision blobs are immutable"),
        "unexpected error: {error}"
    );

    connection
        .execute("DELETE FROM slice_revisions WHERE id = 'slr-a'", [])
        .expect("a delete is allowed");
    assert_eq!(
        count(&connection, "SELECT COUNT(*) FROM slice_revision_blobs"),
        0
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM slice_operations
             WHERE id = 'sop-a' AND slice_revision_id IS NULL AND state = 'succeeded'"
        ),
        1,
        "the operation keeps its state and loses its revision link"
    );
}

/// 5a. `RESTRICT`: a Model with a Slice Revision can't be deleted; a Model
///     Source Revision a Preparation, operation, or revision references
///     can't be deleted; nor can a content blob a revision, revision blob,
///     or operation log references.
#[test]
fn referenced_models_revisions_and_blobs_are_restricted() {
    let (_temp, connection) = migrated();
    seed_model(&connection);
    insert_revision(&connection, &RevisionRow::farm3d("slr-a")).expect("revision");

    assert_rejected(
        connection.execute("DELETE FROM library_models WHERE id = 'mdl-a'", []),
        "deleting a Model with a Slice Revision",
    );
    assert_rejected(
        connection.execute("DELETE FROM model_source_revisions WHERE id = 'msr-a'", []),
        "deleting a source revision a Slice Revision uses",
    );
    assert_rejected(
        connection.execute("DELETE FROM content_blobs WHERE sha256 = ?1", [GCODE_HASH]),
        "deleting a revision's G-code blob",
    );

    connection
        .execute("DELETE FROM slice_revisions WHERE id = 'slr-a'", [])
        .expect("delete revision");
    insert_preparation(&connection).expect("preparation");
    assert_rejected(
        connection.execute("DELETE FROM model_source_revisions WHERE id = 'msr-a'", []),
        "deleting a source revision a Preparation uses",
    );

    insert_revision(&connection, &RevisionRow::farm3d("slr-b")).expect("revision");
    insert_revision_blob(&connection, "slr-b", "manifest").expect("blob");
    connection
        .execute("DELETE FROM slice_revisions WHERE id = 'slr-b'", [])
        .expect("delete revision");
    insert_revision(&connection, &RevisionRow::farm3d("slr-c")).expect("revision");
    insert_revision_blob(&connection, "slr-c", "log").expect("blob");
    assert_rejected(
        connection.execute("DELETE FROM content_blobs WHERE sha256 = ?1", [LOG_HASH]),
        "deleting a revision blob's content",
    );
    connection
        .execute("DELETE FROM slice_revisions WHERE id = 'slr-c'", [])
        .expect("delete revision");

    insert_operation(&connection, "sop-a", "cancelled", None).expect("operation");
    connection
        .execute(
            "UPDATE slice_operations SET log_sha256 = ?1 WHERE id = 'sop-a'",
            [LOG_HASH],
        )
        .expect("operation log");
    assert_rejected(
        connection.execute("DELETE FROM content_blobs WHERE sha256 = ?1", [LOG_HASH]),
        "deleting an operation's log",
    );
}

/// 5b. `CASCADE`: deleting a Model without Slice Revisions removes its
///     Preparation and the Preparation's operations with it.
#[test]
fn deleting_a_model_cascades_to_its_preparation_and_operations() {
    let (_temp, connection) = migrated();
    seed_model(&connection);
    insert_preparation(&connection).expect("preparation");
    insert_operation(&connection, "sop-a", "queued", None).expect("operation");
    insert_operation(&connection, "sop-b", "failed", Some("{}")).expect("operation");

    connection
        .execute("DELETE FROM library_models WHERE id = 'mdl-a'", [])
        .expect("delete model");

    assert_eq!(
        count(&connection, "SELECT COUNT(*) FROM slice_preparations"),
        0
    );
    assert_eq!(
        count(&connection, "SELECT COUNT(*) FROM slice_operations"),
        0
    );
    assert_eq!(
        count(&connection, "SELECT COUNT(*) FROM model_source_revisions"),
        0
    );
}

/// 6. Crash boundary: a failure after the v6 migration's SQL and ledger row
///    but before commit leaves the database exactly as it was at v5,
///    including the old operations ledger and its rows.
#[test]
fn a_crash_before_commit_leaves_the_database_unchanged_at_v5() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let mut connection = v5_database(&paths);
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

    let error = apply_through_failing_before_commit(&mut connection, 6)
        .expect_err("the injected failure must surface");
    assert!(matches!(error, StorageError::MigrationFailed));

    assert_eq!(
        count(&connection, "SELECT user_version FROM pragma_user_version"),
        5,
        "the schema version must roll back to v5"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 6"
        ),
        0,
        "no v6 ledger row may survive the rollback"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_schema
             WHERE name IN ('slicer_runtime_config', 'slice_preparations', 'slice_operations',
                            'slice_revisions', 'slice_revision_blobs', 'operations_p5')"
        ),
        0,
        "the v6 tables must roll back too"
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
