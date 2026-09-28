//! Migration 0008 (P7 `queue_entries`, `jobs`, `job_events`,
//! `reconciliation_requirements`). Covers: the v7->v8 upgrade (a seeded
//! P6 `operations` row and a `host_operations` row survive unchanged),
//! every new operation kind is accepted, the rebuilt `operations` ledger
//! still rejects an unknown kind, the partial unique index (at most one
//! active Job per Printer), `CURRENT_SCHEMA_VERSION == 8`, the migration's
//! crash-boundary behaviour, and that no new column can hold a
//! credential. See the P7 design spec's "Schema" section.

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::{apply_through, apply_through_failing_before_commit};
use farm3d_lib::persistence::{
    MetadataRootLease, Storage, StorageError, StoragePaths, CURRENT_SCHEMA_VERSION,
};

const NOW: &str = "2026-01-01T00:00:00.000Z";
const GCODE_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
const ENDPOINT: &str = r#"{"kind":"moonraker","host":"192.0.2.1","port":7125}"#;

fn open_storage() -> (tempfile::TempDir, StoragePaths, MetadataRootLease, Storage) {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let storage = Storage::open(paths.clone(), &lease).expect("storage");
    (temp, paths, lease, storage)
}

/// A raw connection with foreign keys on, so `CHECK`, trigger, and FK
/// failures keep their SQLite message.
fn raw_connection(paths: &StoragePaths) -> rusqlite::Connection {
    let connection = rusqlite::Connection::open(paths.database()).expect("raw connection");
    connection
        .execute_batch("PRAGMA foreign_keys = ON")
        .expect("foreign keys on");
    connection
}

/// A migrated (v8) database's raw connection, for the constraint tests.
fn migrated() -> (tempfile::TempDir, rusqlite::Connection) {
    let (temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);
    (temp, connection)
}

/// A v7-only database (migrations 1-7) at `paths.database()`.
fn v7_database(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v7 database");
    apply_through(&mut connection, 7).expect("v7 migrations");
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

/// An external Slice Revision `id` (with the blob, Model, and source
/// revision it needs) for the `slice_revision_id` link.
fn seed_slice_revision(connection: &rusqlite::Connection, id: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at)
               VALUES ('{GCODE_HASH}', 200, '{NOW}');
             INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
               VALUES ('mdl-a', 1, 'Model', 'gcode', 'managed', '{NOW}', '{NOW}');
             INSERT INTO model_source_revisions(
               id, model_id, sequence, content_sha256, size_bytes, format, origin,
               source_file_name, source_path, captured_at, inspector_version, inspection_json
             ) VALUES ('msr-a', 'mdl-a', 1, '{GCODE_HASH}', 200, 'gcode', 'import', 'part.gcode',
                       '/src/part.gcode', '{NOW}', 1, '{{}}');
             INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
               gcode_size, target_json, facts_json, requires_manual_printer_selection,
               estimates_json, created_at)
             VALUES ('{id}', 'external', 'mdl-a', 'msr-a', '{GCODE_HASH}', 200, '{{}}', '{{}}', 1,
                     '{{}}', '{NOW}');"
        ),
    );
}

/// A minimal Spool `id`.
fn seed_spool(connection: &rusqlite::Connection, id: &str, spool_number: i64) {
    exec(
        connection,
        &format!(
            "INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
               color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
               created_at, updated_at)
             VALUES ('{id}', 1, {spool_number}, 'Acme', 'PLA', 'Black', '1.75', 1000000,
                     1000000, 'measured', 'active', '{NOW}', '{NOW}');"
        ),
    );
}

/// A minimal `active` reservation `id` on `spool_id`, held by a Job.
fn seed_reservation(connection: &rusqlite::Connection, id: &str, spool_id: &str, holder_id: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg,
               state, operation_id, created_at)
             VALUES ('{id}', '{spool_id}', 'job', '{holder_id}', 500000, 'active',
                     '{id}-op', '{NOW}');"
        ),
    );
}

/// A minimal `queued` Queue Entry `id` at `position`, over `slice_revision_id`.
fn seed_queue_entry(
    connection: &rusqlite::Connection,
    id: &str,
    slice_revision_id: &str,
    position: i64,
) {
    exec(
        connection,
        &format!(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
               state, position, policy, preference, estimate_mg, estimate_source,
               created_at, updated_at)
             VALUES ('{id}', 1, '{slice_revision_id}', 'qln-a', 1, 'queued', {position},
                     'manual', 'loadedFirst', 500000, 'operatorEntered', '{NOW}', '{NOW}');"
        ),
    );
}

struct JobRow<'a> {
    id: &'a str,
    queue_entry_id: &'a str,
    printer_id: &'a str,
    spool_id: &'a str,
    reservation_id: &'a str,
    state: &'a str,
    settlement: &'a str,
    settlement_method: Option<&'a str>,
    ended_at: Option<&'a str>,
    cancel_reason: Option<&'a str>,
    upload_host_operation_id: Option<&'a str>,
    start_host_operation_id: Option<&'a str>,
    start_confirmation: Option<&'a str>,
    started_at: Option<&'a str>,
    history_mark: Option<i64>,
    host_unreachable_since: Option<&'a str>,
    correction_event_id: Option<&'a str>,
}

impl<'a> JobRow<'a> {
    fn assigned(
        id: &'a str,
        queue_entry_id: &'a str,
        printer_id: &'a str,
        spool_id: &'a str,
        reservation_id: &'a str,
    ) -> Self {
        Self {
            id,
            queue_entry_id,
            printer_id,
            spool_id,
            reservation_id,
            state: "assigned",
            settlement: "open",
            settlement_method: None,
            ended_at: None,
            cancel_reason: None,
            upload_host_operation_id: None,
            start_host_operation_id: None,
            start_confirmation: None,
            started_at: None,
            history_mark: None,
            host_unreachable_since: None,
            correction_event_id: None,
        }
    }
}

fn insert_job(connection: &rusqlite::Connection, row: &JobRow<'_>) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO jobs(
             id, revision, queue_entry_id, slice_revision_id, printer_id, printer_snapshot_json,
             spool_id, reservation_id, estimate_mg, state, cancel_reason, settlement,
             settlement_method, assigned_by, upload_host_operation_id, start_host_operation_id,
             start_confirmation, started_at, history_mark, host_unreachable_since,
             correction_event_id, created_at, updated_at, ended_at
         ) VALUES (?1, 1, ?2, 'slr-a', ?3, '{}', ?4, ?5, 500000, ?6, ?7, ?8,
                   ?9, 'operator', ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?17, ?18)",
        rusqlite::params![
            row.id,
            row.queue_entry_id,
            row.printer_id,
            row.spool_id,
            row.reservation_id,
            row.state,
            row.cancel_reason,
            row.settlement,
            row.settlement_method,
            row.upload_host_operation_id,
            row.start_host_operation_id,
            row.start_confirmation,
            row.started_at,
            row.history_mark,
            row.host_unreachable_since,
            row.correction_event_id,
            NOW,
            row.ended_at,
        ],
    )
}

/// A minimal `upload`, `dispatching` `host_operations` row for `printer_id`.
fn insert_host_operation(
    connection: &rusqlite::Connection,
    id: &str,
    printer_id: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO host_operations(
             id, operation_id, printer_id, kind, host_path, gcode_sha256, gcode_size,
             endpoint_json, state, created_at
         ) VALUES (?1, ?1, ?2, 'upload', 'farm3d/a.gcode', ?3, 200, ?4, 'dispatching', ?5)",
        rusqlite::params![id, printer_id, GCODE_HASH, ENDPOINT, NOW],
    )
}

/// A terminal (`succeeded`) `host_operations` row for `printer_id`, so more
/// than one can exist per printer (the partial unique index only limits
/// *unresolved* rows) — used purely as an FK anchor for
/// `jobs.upload_host_operation_id`/`start_host_operation_id` in the
/// active-state CHECK tests, which don't care about the host operation's
/// own state.
fn seed_terminal_host_operation(connection: &rusqlite::Connection, id: &str, printer_id: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO host_operations(
                 id, operation_id, printer_id, kind, host_path, gcode_sha256, gcode_size,
                 endpoint_json, state, created_at)
             VALUES ('{id}', '{id}', '{printer_id}', 'upload', 'farm3d/a.gcode', '{GCODE_HASH}',
                     200, '{ENDPOINT}', 'succeeded', '{NOW}');"
        ),
    );
}

/// 1. A fresh database reaches `CURRENT_SCHEMA_VERSION` (8), with a
///    ledger row for `0008_p7_queue_jobs` whose checksum matches the
///    migration SQL.
#[test]
fn fresh_database_records_the_v8_ledger_row_with_a_matching_checksum() {
    let (_temp, _paths, _lease, storage) = open_storage();

    let (version, name, checksum) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 8",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((version, name, checksum))
        })
        .expect("schema state");

    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    assert_eq!(version, 9);
    assert_eq!(name, "0008_p7_queue_jobs");
    let expected_checksum = format!(
        "{:x}",
        Sha256::digest(include_str!("../migrations/0008_p7_queue_jobs.sql").as_bytes())
    );
    assert_eq!(checksum, expected_checksum);
}

/// 2. Upgrading v7 to v8 keeps a seeded P5 `operations` row and a P6
///    `host_operations` row unchanged; the rebuilt ledger accepts every
///    P7 kind and still rejects an unknown one; and the upgraded
///    database reopens (restart) without change.
#[test]
fn upgrading_v7_to_v8_keeps_every_existing_row_and_survives_a_restart() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v7_database(&paths);
        seed_printer(&connection, "prn-a");
        exec(
            &connection,
            &format!(
                "INSERT INTO operations(id, kind, request_digest, created_at) VALUES
                   ('op-move', 'moveSpool', 'digest-1', '{NOW}');"
            ),
        );
        insert_host_operation(&connection, "hop-old", "prn-a").expect("seed host operation");
    }

    let storage = Storage::open(paths.clone(), &lease).expect("v8 storage");
    drop(storage);
    // Restart: a second open sees a current database and changes nothing.
    let storage = Storage::open(paths.clone(), &lease).expect("reopened storage");
    drop(storage);

    let connection = raw_connection(&paths);

    let ledger: Vec<(String, String, String, String)> = {
        let mut statement = connection
            .prepare(
                "SELECT id, kind, request_digest, created_at FROM operations WHERE id = 'op-move'",
            )
            .expect("prepare");
        statement
            .query_map([], |row| {
                Ok((row.get(0)?, row.get(1)?, row.get(2)?, row.get(3)?))
            })
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("collect")
    };
    assert_eq!(
        ledger,
        vec![(
            "op-move".to_string(),
            "moveSpool".to_string(),
            "digest-1".to_string(),
            NOW.to_string(),
        )]
    );

    let host_op: (String, Option<String>) = connection
        .query_row(
            "SELECT state, job_id FROM host_operations WHERE id = 'hop-old'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("host operation row");
    assert_eq!(host_op, ("dispatching".to_string(), None));

    for kind in [
        "addToQueue",
        "updateQueueEntry",
        "moveQueueEntry",
        "removeQueueEntry",
        "assignQueueEntry",
        "stageJob",
        "startJob",
        "pauseJob",
        "resumeJob",
        "cancelJob",
        "releaseJob",
        "retryJob",
        "declareJobOutcome",
        "settleJobMaterial",
        "correctJobMaterial",
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
             VALUES ('op-bad', 'printJob', 'd', ?1)",
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

/// 3. `queue_entries`' `CHECK`s reject invalid rows: id prefix, `state`,
///    the closed/`close_reason`/`position`/`closed_at` triple, and the
///    `origin_entry_id`/`origin_kind` pairing.
#[test]
fn queue_entry_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_slice_revision(&connection, "slr-a");

    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 state, position, policy, preference, estimate_mg, estimate_source, created_at, updated_at)
             VALUES ('xyz-a', 1, 'slr-a', 'qln-a', 1, 'queued', 1, 'manual', 'loadedFirst',
                     500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "a non-qen id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 state, position, policy, preference, estimate_mg, estimate_source, created_at, updated_at)
             VALUES ('qen-a', 1, 'slr-a', 'qln-a', 1, 'pending', 1, 'manual', 'loadedFirst',
                     500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "an unknown state",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 state, position, policy, preference, estimate_mg, estimate_source, created_at, updated_at)
             VALUES ('qen-a', 1, 'slr-a', 'qln-a', 1, 'closed', 1, 'manual', 'loadedFirst',
                     500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "a closed entry with a position and no close_reason/closed_at",
    );
    exec(
        &connection,
        &format!(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 state, close_reason, policy, preference, estimate_mg, estimate_source, created_at, updated_at, closed_at)
             VALUES ('qen-closed', 1, 'slr-a', 'qln-a', 1, 'closed', 'removed', 'manual', 'loadedFirst',
                     500000, 'operatorEntered', '{NOW}', '{NOW}', '{NOW}');"
        ),
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 state, position, policy, preference, estimate_mg, estimate_source, created_at, updated_at)
             VALUES ('qen-a', 1, 'slr-a', 'qln-a', 1, 'assigned', 1, 'manual', 'loadedFirst',
                     500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "an assigned entry with no job_id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 origin_kind, state, position, policy, preference, estimate_mg, estimate_source,
                 created_at, updated_at)
             VALUES ('qen-a', 1, 'slr-a', 'qln-a', 1, 'retry', 'queued', 1, 'manual',
                     'loadedFirst', 500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "origin_kind without origin_entry_id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 state, job_id, position, policy, preference, estimate_mg, estimate_source, created_at, updated_at)
             VALUES ('qen-a', 1, 'slr-a', 'qln-a', 1, 'queued', 'job-a', 1, 'manual',
                     'loadedFirst', 500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "a queued entry with job_id set",
    );

    seed_queue_entry(&connection, "qen-ok", "slr-a", 1);
}

/// 4. The open-position partial unique index rejects a second open entry
///    at the same position, and the `origin_entry_id` partial unique
///    index rejects a second successor for the same origin.
#[test]
fn queue_entry_position_and_successor_are_unique() {
    let (_temp, connection) = migrated();
    seed_slice_revision(&connection, "slr-a");
    seed_queue_entry(&connection, "qen-a", "slr-a", 1);

    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 state, position, policy, preference, estimate_mg, estimate_source, created_at, updated_at)
             VALUES ('qen-b', 1, 'slr-a', 'qln-a', 2, 'queued', 1, 'manual', 'loadedFirst',
                     500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "a second open entry at position 1",
    );

    exec(
        &connection,
        &format!(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 origin_entry_id, origin_kind, state, position, policy, preference, estimate_mg,
                 estimate_source, created_at, updated_at)
             VALUES ('qen-b', 1, 'slr-a', 'qln-a', 1, 'qen-a', 'retry', 'queued', 2, 'manual',
                     'loadedFirst', 500000, 'operatorEntered', '{NOW}', '{NOW}');"
        ),
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
                 origin_entry_id, origin_kind, state, position, policy, preference, estimate_mg,
                 estimate_source, created_at, updated_at)
             VALUES ('qen-c', 1, 'slr-a', 'qln-a', 1, 'qen-a', 'release', 'queued', 3, 'manual',
                     'loadedFirst', 500000, 'operatorEntered', ?1, ?1)",
            [NOW],
        ),
        "a second successor for the same origin_entry_id",
    );
}

/// 5. `jobs`' `CHECK`s reject invalid rows: id prefix, `state`, the
///    cancelled/`cancel_reason` pairing, the terminal/`ended_at`/
///    `settlement` pairing, the active-state companion-column
///    requirements (`upload_host_operation_id`, `start_confirmation`,
///    `started_at`/`history_mark`, `start_host_operation_id`), the
///    settlement/`settlement_method` and settlement/`cancel_reason`
///    pairings, and `host_unreachable_since`/`correction_event_id`'s
///    state restrictions. Every case below also has a passing
///    counterpart that satisfies the same CHECK, so the assertion is
///    tied to that one CHECK and not some other constraint.
#[test]
fn job_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    seed_printer(&connection, "prn-b");
    seed_printer(&connection, "prn-c");
    seed_slice_revision(&connection, "slr-a");
    seed_spool(&connection, "spl-a", 1);
    seed_queue_entry(&connection, "qen-a", "slr-a", 1);
    seed_queue_entry(&connection, "qen-b", "slr-a", 2);
    seed_queue_entry(&connection, "qen-c", "slr-a", 3);
    seed_queue_entry(&connection, "qen-d", "slr-a", 4);
    seed_queue_entry(&connection, "qen-e", "slr-a", 5);
    seed_queue_entry(&connection, "qen-f", "slr-a", 6);
    seed_reservation(&connection, "rsv-a", "spl-a", "job-a");
    seed_reservation(&connection, "rsv-b", "spl-a", "job-b");
    seed_reservation(&connection, "rsv-c", "spl-a", "job-c");
    seed_reservation(&connection, "rsv-d", "spl-a", "job-d");
    seed_reservation(&connection, "rsv-e", "spl-a", "job-e");
    seed_reservation(&connection, "rsv-f", "spl-a", "job-f");
    seed_terminal_host_operation(&connection, "hop-b-upload", "prn-b");
    seed_terminal_host_operation(&connection, "hop-b-start", "prn-b");
    seed_terminal_host_operation(&connection, "hop-c-upload", "prn-c");
    seed_terminal_host_operation(&connection, "hop-c-start", "prn-c");

    assert_rejected(
        insert_job(
            &connection,
            &JobRow::assigned("xyz-a", "qen-a", "prn-a", "spl-a", "rsv-a"),
        ),
        "a non-job id",
    );
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "printing_",
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "an unknown state",
    );
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "cancelled",
                cancel_reason: None,
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a cancelled job with no cancel_reason",
    );
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                cancel_reason: Some("cancelledByOperator"),
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "an assigned job with cancel_reason set",
    );
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "completed",
                settlement: "open",
                ended_at: Some(NOW),
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a completed job with settlement still open",
    );
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "completed",
                settlement: "settled",
                ended_at: None,
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a completed job with no ended_at",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO jobs(
                 id, revision, queue_entry_id, slice_revision_id, printer_id, printer_snapshot_json,
                 spool_id, reservation_id, estimate_mg, state, settlement, assigned_by,
                 created_at, updated_at
             ) VALUES ('job-a', 1, 'qen-a', 'slr-a', 'prn-a', '{}', 'spl-a', 'rsv-a', 500000,
                       'awaitingStart', 'open', 'operator', ?1, ?1)",
            [NOW],
        ),
        "an awaitingStart job with no upload_host_operation_id",
    );

    // `(settlement = 'settled') = (settlement_method IS NOT NULL)`.
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "completed",
                settlement: "settled",
                settlement_method: None,
                ended_at: Some(NOW),
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a settled job with no settlement_method",
    );
    insert_job(
        &connection,
        &JobRow {
            state: "completed",
            settlement: "settled",
            settlement_method: Some("estimated"),
            ended_at: Some(NOW),
            ..JobRow::assigned("job-b", "qen-b", "prn-a", "spl-a", "rsv-b")
        },
    )
    .expect("a settled job with settlement_method set");

    // `settlement <> 'notRequired' OR cancel_reason IN ('releasedBeforeStart','cancelledBeforeStart')`.
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "cancelled",
                settlement: "notRequired",
                cancel_reason: Some("cancelledByOperator"),
                ended_at: Some(NOW),
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a notRequired job whose cancel_reason isn't released/cancelledBeforeStart",
    );
    insert_job(
        &connection,
        &JobRow {
            state: "cancelled",
            settlement: "notRequired",
            cancel_reason: Some("cancelledBeforeStart"),
            ended_at: Some(NOW),
            ..JobRow::assigned("job-c", "qen-c", "prn-a", "spl-a", "rsv-c")
        },
    )
    .expect("a notRequired job cancelled before start");

    // `state NOT IN ('starting','printing','paused') OR start_confirmation IS NOT NULL`,
    // isolated from the neighboring `start_host_operation_id` CHECK below by
    // setting every other active-state companion column a `starting` row
    // needs.
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "starting",
                upload_host_operation_id: Some("hop-b-upload"),
                start_host_operation_id: Some("hop-b-start"),
                start_confirmation: None,
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a starting job with no start_confirmation",
    );

    // `state NOT IN ('starting','printing','paused') OR start_host_operation_id IS NOT NULL`,
    // isolated the same way (every other companion column present).
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "starting",
                upload_host_operation_id: Some("hop-b-upload"),
                start_host_operation_id: None,
                start_confirmation: Some("bedClear"),
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a starting job with no start_host_operation_id",
    );
    // One valid `starting` row proves both of the above CHECKs are
    // satisfiable together (`starting` needs neither `started_at`,
    // `history_mark`, nor `ended_at`).
    insert_job(
        &connection,
        &JobRow {
            state: "starting",
            upload_host_operation_id: Some("hop-b-upload"),
            start_host_operation_id: Some("hop-b-start"),
            start_confirmation: Some("bedClear"),
            ..JobRow::assigned("job-d", "qen-d", "prn-b", "spl-a", "rsv-d")
        },
    )
    .expect("a valid starting job");

    // `state NOT IN ('printing','paused') OR (started_at IS NOT NULL AND history_mark IS NOT NULL)`,
    // isolated by setting every other `printing` companion column
    // (`upload_host_operation_id`, `start_confirmation`,
    // `start_host_operation_id`) that a separate CHECK would otherwise
    // also reject on.
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                state: "printing",
                upload_host_operation_id: Some("hop-c-upload"),
                start_host_operation_id: Some("hop-c-start"),
                start_confirmation: Some("bedClear"),
                started_at: None,
                history_mark: None,
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "a printing job with no started_at/history_mark",
    );
    // One valid `printing` row proves that CHECK is satisfiable, and
    // doubles as the passing counterpart for `host_unreachable_since`
    // (below): it's only legal while `printing`/`paused`.
    insert_job(
        &connection,
        &JobRow {
            state: "printing",
            upload_host_operation_id: Some("hop-c-upload"),
            start_host_operation_id: Some("hop-c-start"),
            start_confirmation: Some("bedClear"),
            started_at: Some(NOW),
            history_mark: Some(1),
            host_unreachable_since: Some(NOW),
            ..JobRow::assigned("job-e", "qen-e", "prn-c", "spl-a", "rsv-e")
        },
    )
    .expect("a valid printing job, unreachable since NOW");

    // `host_unreachable_since IS NULL OR state IN ('printing','paused')`.
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                host_unreachable_since: Some(NOW),
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "an assigned job with host_unreachable_since set",
    );

    // `correction_event_id IS NULL OR state = 'completed'`.
    assert_rejected(
        insert_job(
            &connection,
            &JobRow {
                correction_event_id: Some("jev-fake"),
                ..JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a")
            },
        ),
        "an assigned job with correction_event_id set",
    );
    insert_job(
        &connection,
        &JobRow {
            state: "completed",
            settlement: "settled",
            settlement_method: Some("estimated"),
            ended_at: Some(NOW),
            correction_event_id: Some("jev-real"),
            ..JobRow::assigned("job-f", "qen-f", "prn-a", "spl-a", "rsv-f")
        },
    )
    .expect("a completed job with correction_event_id set");

    insert_job(
        &connection,
        &JobRow::assigned("job-ok", "qen-a", "prn-a", "spl-a", "rsv-a"),
    )
    .expect("a valid assigned job");
}

/// 6. The partial unique index allows at most one active (non-terminal)
///    Job per Printer; a second terminal Job for the same Printer is
///    fine.
#[test]
fn at_most_one_active_job_per_printer() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    seed_slice_revision(&connection, "slr-a");
    seed_spool(&connection, "spl-a", 1);
    seed_spool(&connection, "spl-b", 2);
    seed_queue_entry(&connection, "qen-a", "slr-a", 1);
    seed_queue_entry(&connection, "qen-b", "slr-a", 2);
    seed_reservation(&connection, "rsv-a", "spl-a", "job-a");
    seed_reservation(&connection, "rsv-b", "spl-b", "job-b");

    insert_job(
        &connection,
        &JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a"),
    )
    .expect("first active job");
    assert_rejected(
        insert_job(
            &connection,
            &JobRow::assigned("job-b", "qen-b", "prn-a", "spl-b", "rsv-b"),
        ),
        "a second active job for the same printer",
    );

    // A terminal Job for the same Printer is fine alongside the active one.
    exec(
        &connection,
        &format!(
            "UPDATE jobs SET state = 'cancelled', cancel_reason = 'cancelledBeforeStart',
                 settlement = 'notRequired', ended_at = '{NOW}' WHERE id = 'job-a';
             INSERT INTO jobs(
                 id, revision, queue_entry_id, slice_revision_id, printer_id, printer_snapshot_json,
                 spool_id, reservation_id, estimate_mg, state, cancel_reason, settlement,
                 assigned_by, created_at, updated_at, ended_at
             ) VALUES ('job-b', 1, 'qen-b', 'slr-a', 'prn-a', '{{}}', 'spl-b', 'rsv-b', 500000,
                       'cancelled', 'cancelledBeforeStart', 'notRequired', 'operator', '{NOW}', '{NOW}', '{NOW}');"
        ),
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM jobs WHERE printer_id = 'prn-a'"
        ),
        2
    );
}

/// 7. `job_events` is append-only: the migration's triggers reject an
///    UPDATE and a DELETE.
#[test]
fn job_events_reject_update_and_delete() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    seed_slice_revision(&connection, "slr-a");
    seed_spool(&connection, "spl-a", 1);
    seed_queue_entry(&connection, "qen-a", "slr-a", 1);
    seed_reservation(&connection, "rsv-a", "spl-a", "job-a");
    insert_job(
        &connection,
        &JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a"),
    )
    .expect("job");
    exec(
        &connection,
        &format!(
            "INSERT INTO job_events(id, job_id, sequence, kind, to_state, at)
             VALUES ('jev-a', 'job-a', 1, 'assigned', 'assigned', '{NOW}');"
        ),
    );

    assert_rejected(
        connection.execute(
            "UPDATE job_events SET kind = 'stageHandedOff' WHERE id = 'jev-a'",
            [],
        ),
        "an update to job_events",
    );
    assert_rejected(
        connection.execute("DELETE FROM job_events WHERE id = 'jev-a'", []),
        "a delete from job_events",
    );
}

/// 8. `reconciliation_requirements`' `CHECK`s: `materialReconciliation`
///    requires `spool_id`/`reservation_id`, `jobOutcomeUnknown` can never
///    be `deferred`, and `resolved` requires `resolved_at`/`resolution_json`.
#[test]
fn reconciliation_requirement_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    seed_slice_revision(&connection, "slr-a");
    seed_spool(&connection, "spl-a", 1);
    seed_queue_entry(&connection, "qen-a", "slr-a", 1);
    seed_reservation(&connection, "rsv-a", "spl-a", "job-a");
    insert_job(
        &connection,
        &JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a"),
    )
    .expect("job");

    assert_rejected(
        connection.execute(
            "INSERT INTO reconciliation_requirements(id, job_id, kind, status, opened_at)
             VALUES ('rrq-a', 'job-a', 'materialReconciliation', 'pending', ?1)",
            [NOW],
        ),
        "a materialReconciliation requirement with no spool_id/reservation_id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO reconciliation_requirements(id, job_id, kind, status, deferred_at, opened_at)
             VALUES ('rrq-a', 'job-a', 'jobOutcomeUnknown', 'deferred', ?1, ?1)",
            [NOW],
        ),
        "a jobOutcomeUnknown requirement that's deferred",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO reconciliation_requirements(id, job_id, kind, status, resolved_at, opened_at)
             VALUES ('rrq-a', 'job-a', 'jobOutcomeUnknown', 'resolved', ?1, ?1)",
            [NOW],
        ),
        "a resolved requirement with no resolution_json",
    );

    exec(
        &connection,
        &format!(
            "INSERT INTO reconciliation_requirements(id, job_id, kind, status, spool_id,
                 reservation_id, opened_at)
             VALUES ('rrq-ok', 'job-a', 'materialReconciliation', 'pending', 'spl-a', 'rsv-a', '{NOW}');"
        ),
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO reconciliation_requirements(id, job_id, kind, status, spool_id,
                 reservation_id, opened_at)
             VALUES ('rrq-dup', 'job-a', 'materialReconciliation', 'pending', 'spl-a', 'rsv-a', ?1)",
            [NOW],
        ),
        "a second materialReconciliation requirement for the same job",
    );
}

/// 9. `host_operations.job_id` links to a Job and rejects an unknown one;
///    the terminal-row trigger (P6) still rejects any other update to a
///    terminal row, so `job_id` is only ever written at insert.
#[test]
fn host_operations_job_id_links_and_the_terminal_trigger_still_holds() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    seed_slice_revision(&connection, "slr-a");
    seed_spool(&connection, "spl-a", 1);
    seed_queue_entry(&connection, "qen-a", "slr-a", 1);
    seed_reservation(&connection, "rsv-a", "spl-a", "job-a");
    insert_job(
        &connection,
        &JobRow::assigned("job-a", "qen-a", "prn-a", "spl-a", "rsv-a"),
    )
    .expect("job");

    connection
        .execute(
            "INSERT INTO host_operations(
                 id, operation_id, printer_id, kind, host_path, gcode_sha256, gcode_size,
                 endpoint_json, state, job_id, created_at
             ) VALUES ('hop-a', 'hop-a', 'prn-a', 'upload', 'farm3d/a.gcode', ?1, 200, ?2,
                       'succeeded', 'job-a', ?3)",
            rusqlite::params![GCODE_HASH, ENDPOINT, NOW],
        )
        .expect("host operation with job_id");
    assert_rejected(
        connection.execute(
            "INSERT INTO host_operations(
                 id, operation_id, printer_id, kind, host_path, gcode_sha256, gcode_size,
                 endpoint_json, state, job_id, created_at
             ) VALUES ('hop-b', 'hop-b', 'prn-a', 'upload', 'farm3d/a.gcode', ?1, 200, ?2,
                       'succeeded', 'job-missing', ?3)",
            rusqlite::params![GCODE_HASH, ENDPOINT, NOW],
        ),
        "an unknown job_id",
    );

    // The terminal trigger still rejects an ordinary update.
    assert_rejected(
        connection.execute(
            "UPDATE host_operations SET state = 'failed' WHERE id = 'hop-a'",
            [],
        ),
        "an update to a terminal host_operations row",
    );
    // The trigger doesn't list `job_id` among the columns an unlinking
    // update may change, so even an update that touches only `job_id`
    // still raises on a terminal row (the migration's note: P7 only
    // ever writes `job_id` at insert).
    assert_rejected(
        connection.execute(
            "UPDATE host_operations SET job_id = NULL WHERE id = 'hop-a'",
            [],
        ),
        "an update to job_id on a terminal host_operations row",
    );
}

/// 10. A crash before the migration's commit leaves the v7 database
///     completely unchanged (no v8 tables, no v8 ledger row, the
///     `operations` table's rebuilt schema rolled back, and the seeded
///     row still present).
#[test]
fn a_crash_before_commit_leaves_the_database_unchanged_at_v7() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let mut connection = v7_database(&paths);
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

    let error = apply_through_failing_before_commit(&mut connection, 8)
        .expect_err("the injected failure must surface");
    assert!(matches!(error, StorageError::MigrationFailed));

    assert_eq!(
        count(&connection, "SELECT user_version FROM pragma_user_version"),
        7,
        "the schema version must roll back to v7"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 8"
        ),
        0,
        "no v8 ledger row may survive the rollback"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_schema WHERE name IN
                 ('queue_entries', 'jobs', 'job_events', 'reconciliation_requirements', 'operations_p7')"
        ),
        0,
        "the v8 tables must roll back too"
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

/// 11. No `queue_entries`, `jobs`, `job_events`, or
///     `reconciliation_requirements` column name contains `credential`,
///     `secret`, or `key` (global constraint 3: credentials never enter
///     these tables).
#[test]
fn no_new_p7_column_can_hold_a_credential() {
    let (_temp, connection) = migrated();

    for table in [
        "queue_entries",
        "jobs",
        "job_events",
        "reconciliation_requirements",
    ] {
        let mut statement = connection
            .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))
            .expect("prepare");
        let columns: Vec<String> = statement
            .query_map([], |row| row.get(0))
            .expect("query")
            .collect::<rusqlite::Result<_>>()
            .expect("collect");

        assert!(!columns.is_empty(), "{table} must have columns");
        for column in &columns {
            let lower = column.to_lowercase();
            for forbidden in ["credential", "secret", "key"] {
                assert!(
                    !lower.contains(forbidden),
                    "{table}.{column} must not look like it holds a {forbidden}"
                );
            }
        }
    }
}
