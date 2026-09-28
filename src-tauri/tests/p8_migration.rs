//! Migration 0009 (P8 `incidents`, `attention_events`, `camera_snapshots`,
//! `incident_events`, `printer_cameras`, `printer_alert_defaults`, and the
//! `settings` notification/retention columns). Covers: the v8->v9 upgrade
//! (a seeded P7 Job, pending Reconciliation Requirement, and `settings`
//! row survive unchanged, and the new `settings` columns take decision
//! 7's notification defaults and decision 11's retention defaults), every
//! new operation kind is accepted, the rebuilt `operations` ledger still
//! rejects an unknown kind, the partial unique index (at most one open
//! Attention Event per dedup key), the read-implication CHECKs
//! (acknowledged/resolved implies read), the `incident_events`
//! append-only triggers, `CURRENT_SCHEMA_VERSION == 9`, the migration's
//! crash-boundary behaviour, and that no new column can hold a
//! credential. See the P8 design spec's "Schema" section.

use sha2::{Digest, Sha256};

use farm3d_lib::persistence::test_support::{apply_through, apply_through_failing_before_commit};
use farm3d_lib::persistence::{
    MetadataRootLease, Storage, StorageError, StoragePaths, CURRENT_SCHEMA_VERSION,
};

const NOW: &str = "2026-01-01T00:00:00.000Z";

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

/// A migrated (v9) database's raw connection, for the constraint tests.
fn migrated() -> (tempfile::TempDir, rusqlite::Connection) {
    let (temp, paths, _lease, storage) = open_storage();
    drop(storage);
    let connection = raw_connection(&paths);
    (temp, connection)
}

/// A v8-only database (migrations 1-8) at `paths.database()`.
fn v8_database(paths: &StoragePaths) -> rusqlite::Connection {
    let mut connection = rusqlite::Connection::open(paths.database()).expect("v8 database");
    apply_through(&mut connection, 8).expect("v8 migrations");
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
    const GCODE_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";
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

/// A minimal `assigned` Job `id` (open settlement, no host operations).
fn seed_job(
    connection: &rusqlite::Connection,
    id: &str,
    queue_entry_id: &str,
    slice_revision_id: &str,
    printer_id: &str,
    spool_id: &str,
    reservation_id: &str,
) {
    exec(
        connection,
        &format!(
            "INSERT INTO jobs(
                 id, revision, queue_entry_id, slice_revision_id, printer_id, printer_snapshot_json,
                 spool_id, reservation_id, estimate_mg, state, settlement, assigned_by,
                 created_at, updated_at
             ) VALUES ('{id}', 1, '{queue_entry_id}', '{slice_revision_id}', '{printer_id}', '{{}}',
                       '{spool_id}', '{reservation_id}', 500000, 'assigned', 'open', 'operator',
                       '{NOW}', '{NOW}');"
        ),
    );
}

/// A whole Printer -> Slice Revision -> Spool -> Reservation -> Queue Entry
/// -> Job chain, minimal enough for the `attention_events`/`incidents`/
/// `camera_snapshots` FK tests. Returns the Job's id.
fn seed_job_chain(connection: &rusqlite::Connection, suffix: &str) -> String {
    let printer_id = format!("prn-{suffix}");
    let slice_revision_id = format!("slr-{suffix}");
    let spool_id = format!("spl-{suffix}");
    let reservation_id = format!("rsv-{suffix}");
    let queue_entry_id = format!("qen-{suffix}");
    let job_id = format!("job-{suffix}");
    seed_printer(connection, &printer_id);
    seed_slice_revision(connection, &slice_revision_id);
    seed_spool(connection, &spool_id, 1);
    seed_reservation(connection, &reservation_id, &spool_id, &job_id);
    seed_queue_entry(connection, &queue_entry_id, &slice_revision_id, 1);
    seed_job(
        connection,
        &job_id,
        &queue_entry_id,
        &slice_revision_id,
        &printer_id,
        &spool_id,
        &reservation_id,
    );
    job_id
}

/// A pending `materialReconciliation` Reconciliation Requirement `id` for
/// `job_id`/`spool_id`/`reservation_id`.
fn seed_requirement(
    connection: &rusqlite::Connection,
    id: &str,
    job_id: &str,
    spool_id: &str,
    reservation_id: &str,
) {
    exec(
        connection,
        &format!(
            "INSERT INTO reconciliation_requirements(id, job_id, kind, status, spool_id,
                 reservation_id, opened_at)
             VALUES ('{id}', '{job_id}', 'materialReconciliation', 'pending', '{spool_id}',
                     '{reservation_id}', '{NOW}');"
        ),
    );
}

/// A valid `printer.offline` Attention Event row for `printer_id`, open
/// (unread, unacknowledged, unresolved). `id` and `printer_id` both feed
/// `source_id`/`dedup_key`, so every FK and CHECK here is genuinely
/// satisfied.
fn insert_printer_event(
    connection: &rusqlite::Connection,
    id: &str,
    printer_id: &str,
) -> rusqlite::Result<usize> {
    connection.execute(
        "INSERT INTO attention_events(
             id, dedup_key, condition, severity, requires_action, resolution_mode,
             notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
             detail_json, summary, origin, first_observed_at, last_observed_at
         ) VALUES (?1, 'printer.offline:printer:' || ?2, 'printer.offline', 'warning', 1, 'auto',
                   'connectivity', 'printer', ?2, ?2, '{}', '{}', 'Offline.', 'live', ?3, ?3)",
        rusqlite::params![id, printer_id, NOW],
    )
}

/// A minimal open Incident for `printer_id`/`job_id` (`printer.hostFailed`
/// when `job_id` is `None`, else `job.failed`).
fn seed_incident(
    connection: &rusqlite::Connection,
    id: &str,
    printer_id: &str,
    job_id: Option<&str>,
) {
    let (kind, job_id_sql) = match job_id {
        Some(job_id) => ("job.failed", format!("'{job_id}'")),
        None => ("printer.hostFailed", "NULL".to_string()),
    };
    exec(
        connection,
        &format!(
            "INSERT INTO incidents(id, kind, printer_id, job_id, printer_snapshot_json, opened_at)
             VALUES ('{id}', '{kind}', '{printer_id}', {job_id_sql}, '{{}}', '{NOW}');"
        ),
    );
}

/// A minimal `manual` camera snapshot `id` for `printer_id` (the only
/// trigger that needs no Incident/Job).
fn seed_manual_snapshot(connection: &rusqlite::Connection, id: &str, printer_id: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO camera_snapshots(
                 id, printer_id, trigger, operation_id, captured_at, content_type, byte_len,
                 sha256, rel_path
             ) VALUES ('{id}', '{printer_id}', 'manual', '{id}-op', '{NOW}', 'image/jpeg', 100,
                       '{sha}', 'snapshots/{id}.jpg');",
            sha = "a".repeat(64),
        ),
    );
}

/// 1. A fresh database reaches `CURRENT_SCHEMA_VERSION` (9), with a
///    ledger row for `0009_p8_attention` whose checksum matches the
///    migration SQL.
#[test]
fn fresh_database_records_the_v9_ledger_row_with_a_matching_checksum() {
    let (_temp, _paths, _lease, storage) = open_storage();

    let (version, name, checksum) = storage
        .read(|connection| {
            let version: i64 = connection.query_row("PRAGMA user_version", [], |row| row.get(0))?;
            let (name, checksum): (String, String) = connection.query_row(
                "SELECT name, checksum FROM schema_migrations WHERE version = 9",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )?;
            Ok((version, name, checksum))
        })
        .expect("schema state");

    assert_eq!(version, CURRENT_SCHEMA_VERSION);
    assert_eq!(version, 9);
    assert_eq!(name, "0009_p8_attention");
    let expected_checksum = format!(
        "{:x}",
        Sha256::digest(include_str!("../migrations/0009_p8_attention.sql").as_bytes())
    );
    assert_eq!(checksum, expected_checksum);
}

/// 2. Upgrading v8 to v9 keeps a seeded P7 Job and pending Reconciliation
///    Requirement, and the existing `settings` row, unchanged; the new
///    `settings` columns take decision 7's notification-class defaults
///    (on for `fatal`/`confirmation`/`completion`, off for the rest) and
///    decision 11's retention defaults (30 days, 2048 MiB); the rebuilt
///    ledger accepts every P8 kind and still rejects an unknown one; and
///    the upgraded database reopens (restart) without change.
#[test]
fn upgrading_v8_to_v9_keeps_every_existing_row_and_survives_a_restart() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    {
        let connection = v8_database(&paths);
        seed_job_chain(&connection, "a");
        seed_requirement(&connection, "rrq-a", "job-a", "spl-a", "rsv-a");
        exec(
            &connection,
            &format!(
                "INSERT INTO settings(singleton_id, revision, theme_mode, updated_at)
                 VALUES (1, 1, 'dark', '{NOW}');"
            ),
        );
    }

    let storage = Storage::open(paths.clone(), &lease).expect("v9 storage");
    drop(storage);
    // Restart: a second open sees a current database and changes nothing.
    let storage = Storage::open(paths.clone(), &lease).expect("reopened storage");
    drop(storage);

    let connection = raw_connection(&paths);

    let job: (String, String) = connection
        .query_row(
            "SELECT state, settlement FROM jobs WHERE id = 'job-a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("job row");
    assert_eq!(job, ("assigned".to_string(), "open".to_string()));

    let requirement: (String, String) = connection
        .query_row(
            "SELECT kind, status FROM reconciliation_requirements WHERE id = 'rrq-a'",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .expect("requirement row");
    assert_eq!(
        requirement,
        ("materialReconciliation".to_string(), "pending".to_string())
    );

    let settings: (String, i64, i64, i64, i64, i64, i64, i64, i64) = connection
        .query_row(
            "SELECT theme_mode, notify_fatal, notify_confirmation, notify_completion,
                    notify_reconciliation, notify_connectivity, notify_inventory,
                    snapshot_retention_days, snapshot_disk_cap_mb
             FROM settings WHERE singleton_id = 1",
            [],
            |row| {
                Ok((
                    row.get(0)?,
                    row.get(1)?,
                    row.get(2)?,
                    row.get(3)?,
                    row.get(4)?,
                    row.get(5)?,
                    row.get(6)?,
                    row.get(7)?,
                    row.get(8)?,
                ))
            },
        )
        .expect("settings row");
    assert_eq!(
        settings,
        ("dark".to_string(), 1, 1, 1, 0, 0, 0, 30, 2048),
        "the pre-existing theme survives, and the new columns take D7/D11's defaults"
    );

    for kind in [
        "markAttentionRead",
        "acknowledgeAttention",
        "resolveAttention",
        "addIncidentNote",
        "setPrinterCamera",
        "clearPrinterCamera",
        "captureSnapshot",
        "setSnapshotPinned",
        "setPrinterAlertDefaults",
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
             VALUES ('op-bad', 'sendAttentionEmail', 'd', ?1)",
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

/// 3. `attention_events`' read-implication `CHECK`s reject an
///    acknowledged-but-unread row and a resolved-but-unread row (the
///    umbrella's "resolving implies read" is enforced even before Rust's
///    `lifecycle::apply` ever runs).
#[test]
fn attention_event_read_implication_checks_reject_unread_acknowledged_or_resolved() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");

    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at, acknowledged_at
             ) VALUES ('att-a', 'printer.offline:printer:prn-a', 'printer.offline', 'warning', 1,
                       'auto', 'connectivity', 'printer', 'prn-a', 'prn-a', '{}', '{}', 'Offline.',
                       'live', ?1, ?1, ?1)",
            [NOW],
        ),
        "an acknowledged event with no read_at",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at, resolved_at,
                 resolution
             ) VALUES ('att-a', 'printer.offline:printer:prn-a', 'printer.offline', 'warning', 1,
                       'auto', 'connectivity', 'printer', 'prn-a', 'prn-a', '{}', '{}', 'Offline.',
                       'live', ?1, ?1, ?1, 'conditionCleared')",
            [NOW],
        ),
        "a resolved event with no read_at",
    );

    // The passing counterpart: read, acknowledged, and resolved together.
    exec(
        &connection,
        &format!(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at, read_at,
                 acknowledged_at, resolved_at, resolution
             ) VALUES ('att-ok', 'printer.offline:printer:prn-a', 'printer.offline', 'warning', 1,
                       'auto', 'connectivity', 'printer', 'prn-a', 'prn-a', '{{}}', '{{}}',
                       'Offline.', 'live', '{NOW}', '{NOW}', '{NOW}', '{NOW}', '{NOW}',
                       'conditionCleared');"
        ),
    );
}

/// 4. Every other `attention_events` `CHECK`: id prefix, the `dedup_key`
///    formula, `resolved_at`/`resolution` pairing, `operatorResolved`
///    requires `resolution_mode = 'manual'`, and the source-kind/id
///    pairings (`printer`, `job`).
#[test]
fn attention_event_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    let job_id = seed_job_chain(&connection, "b");

    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at
             ) VALUES ('xyz-a', 'printer.offline:printer:prn-a', 'printer.offline', 'warning', 1,
                       'auto', 'connectivity', 'printer', 'prn-a', 'prn-a', '{}', '{}', 'Offline.',
                       'live', ?1, ?1)",
            [NOW],
        ),
        "a non-att id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at
             ) VALUES ('att-a', 'printer.offline:printer:prn-wrong', 'printer.offline', 'warning',
                       1, 'auto', 'connectivity', 'printer', 'prn-a', 'prn-a', '{}', '{}',
                       'Offline.', 'live', ?1, ?1)",
            [NOW],
        ),
        "a dedup_key that doesn't match condition:sourceKind:sourceId",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at, resolved_at
             ) VALUES ('att-a', 'printer.offline:printer:prn-a', 'printer.offline', 'warning', 1,
                       'auto', 'connectivity', 'printer', 'prn-a', 'prn-a', '{}', '{}', 'Offline.',
                       'live', ?1, ?1, ?1)",
            [NOW],
        ),
        "a resolved_at with no resolution",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at, read_at,
                 resolved_at, resolution
             ) VALUES ('att-a', 'printer.offline:printer:prn-a', 'printer.offline', 'warning', 1,
                       'auto', 'connectivity', 'printer', 'prn-a', 'prn-a', '{}', '{}', 'Offline.',
                       'live', ?1, ?1, ?1, ?1, 'operatorResolved')",
            [NOW],
        ),
        "operatorResolved on an auto (non-manual) event",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at
             ) VALUES ('att-a', 'printer.offline:printer:prn-a', 'printer.offline', 'warning', 1,
                       'auto', 'connectivity', 'printer', 'prn-a', 'prn-wrong', '{}', '{}',
                       'Offline.', 'live', ?1, ?1)",
            [NOW],
        ),
        "a printer-sourced event whose printer_id doesn't match source_id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at
             ) VALUES ('att-a', 'job.failed:job:' || ?2, 'job.failed', 'fatal', 1, 'manual',
                       'fatal', 'job', ?2, '{}', '{}', 'Failed.', 'live', ?1, ?1)",
            rusqlite::params![NOW, job_id],
        ),
        "a job-sourced event with no job_id",
    );

    insert_printer_event(&connection, "att-ok", "prn-a").expect("a valid printer event");
}

/// 5. The partial unique index allows at most one open Attention Event
///    per `dedup_key`, but a second Event for the same key is fine once
///    the first has `resolved_at` set.
#[test]
fn attention_events_one_open_per_dedup_key() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    insert_printer_event(&connection, "att-a", "prn-a").expect("first open event");

    assert_rejected(
        insert_printer_event(&connection, "att-b", "prn-a"),
        "a second open event for the same dedup_key",
    );

    exec(
        &connection,
        &format!(
            "UPDATE attention_events SET read_at = '{NOW}', resolved_at = '{NOW}',
                 resolution = 'conditionCleared' WHERE id = 'att-a';"
        ),
    );
    insert_printer_event(&connection, "att-c", "prn-a")
        .expect("a new open event once the first resolved");
}

/// 6. `incidents`' kind/`job_id` pairing: `printer.hostFailed` requires no
///    `job_id`; every other kind requires one. `incidents_one_per_job`
///    allows at most one Incident per Job.
#[test]
fn incident_checks_and_one_per_job_index() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    let job_id = seed_job_chain(&connection, "b");

    assert_rejected(
        connection.execute(
            "INSERT INTO incidents(id, kind, printer_id, job_id, printer_snapshot_json, opened_at)
             VALUES ('inc-a', 'printer.hostFailed', 'prn-a', ?1, '{}', ?2)",
            rusqlite::params![job_id, NOW],
        ),
        "a printer.hostFailed incident with a job_id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO incidents(id, kind, printer_id, printer_snapshot_json, opened_at)
             VALUES ('inc-a', 'job.failed', 'prn-a', '{}', ?1)",
            [NOW],
        ),
        "a job.failed incident with no job_id",
    );

    seed_incident(&connection, "inc-ok", "prn-a", Some(&job_id));
    assert_rejected(
        connection.execute(
            "INSERT INTO incidents(id, kind, printer_id, job_id, printer_snapshot_json, opened_at)
             VALUES ('inc-dup', 'job.failed', 'prn-a', ?1, '{}', ?2)",
            rusqlite::params![job_id, NOW],
        ),
        "a second Incident for the same job_id",
    );
}

/// 7. `incident_events` is append-only (the migration's triggers reject
///    an UPDATE and a DELETE), and its kind/reference `CHECK`s: `opened`
///    requires `attention_event_id`; `evidenceCaptured` requires
///    `snapshot_id`.
#[test]
fn incident_events_reject_update_delete_and_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    seed_incident(&connection, "inc-a", "prn-a", None);
    insert_printer_event(&connection, "att-a", "prn-a").expect("event");

    assert_rejected(
        connection.execute(
            "INSERT INTO incident_events(id, incident_id, sequence, kind, detail_json, at)
             VALUES ('iev-a', 'inc-a', 1, 'opened', '{}', ?1)",
            [NOW],
        ),
        "an opened entry with no attention_event_id",
    );
    seed_manual_snapshot(&connection, "snp-a", "prn-a");
    assert_rejected(
        connection.execute(
            "INSERT INTO incident_events(id, incident_id, sequence, kind, detail_json, at)
             VALUES ('iev-a', 'inc-a', 1, 'evidenceCaptured', '{}', ?1)",
            [NOW],
        ),
        "an evidenceCaptured entry with no snapshot_id",
    );

    exec(
        &connection,
        &format!(
            "INSERT INTO incident_events(id, incident_id, sequence, kind, attention_event_id,
                 detail_json, at)
             VALUES ('iev-a', 'inc-a', 1, 'opened', 'att-a', '{{}}', '{NOW}');"
        ),
    );
    assert_rejected(
        connection.execute(
            "UPDATE incident_events SET kind = 'closed' WHERE id = 'iev-a'",
            [],
        ),
        "an update to incident_events",
    );
    assert_rejected(
        connection.execute("DELETE FROM incident_events WHERE id = 'iev-a'", []),
        "a delete from incident_events",
    );
}

/// 8. `camera_snapshots`' trigger/reference `CHECK`s: `incident` requires
///    `incident_id`; `completion` requires `job_id` and no `incident_id`;
///    `manual` requires `operation_id`; and the pinned/pruned pairing
///    (`pinned_at` and `pruned_at` are mutually exclusive unless the
///    prune reason is `missingFile`).
#[test]
fn camera_snapshot_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");
    let job_id = seed_job_chain(&connection, "b");
    seed_incident(&connection, "inc-a", "prn-a", None);
    let sha = "a".repeat(64);

    assert_rejected(
        connection.execute(
            "INSERT INTO camera_snapshots(
                 id, printer_id, trigger, captured_at, content_type, byte_len, sha256, rel_path
             ) VALUES ('snp-a', 'prn-a', 'incident', ?1, 'image/jpeg', 100, ?2, 'snapshots/a.jpg')",
            rusqlite::params![NOW, sha],
        ),
        "an incident-triggered snapshot with no incident_id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO camera_snapshots(
                 id, printer_id, incident_id, trigger, captured_at, content_type, byte_len,
                 sha256, rel_path
             ) VALUES ('snp-a', 'prn-a', 'inc-a', 'completion', ?1, 'image/jpeg', 100, ?2,
                       'snapshots/a.jpg')",
            rusqlite::params![NOW, sha],
        ),
        "a completion-triggered snapshot with an incident_id instead of job_id",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO camera_snapshots(
                 id, printer_id, trigger, captured_at, content_type, byte_len, sha256, rel_path
             ) VALUES ('snp-a', 'prn-a', 'manual', ?1, 'image/jpeg', 100, ?2, 'snapshots/a.jpg')",
            rusqlite::params![NOW, sha],
        ),
        "a manual snapshot with no operation_id",
    );

    exec(
        &connection,
        &format!(
            "INSERT INTO camera_snapshots(
                 id, printer_id, job_id, trigger, captured_at, content_type, byte_len, sha256,
                 rel_path
             ) VALUES ('snp-ok', 'prn-a', '{job_id}', 'completion', '{NOW}', 'image/jpeg', 100,
                       '{sha}', 'snapshots/ok.jpg');"
        ),
    );
    assert_rejected(
        connection.execute(
            "UPDATE camera_snapshots SET pinned_at = ?1, pruned_at = ?1, prune_reason = 'age'
             WHERE id = 'snp-ok'",
            [NOW],
        ),
        "a pinned row pruned for age (only missingFile may coexist with a pin)",
    );
    exec(
        &connection,
        &format!(
            "UPDATE camera_snapshots SET pinned_at = '{NOW}', pruned_at = '{NOW}',
                 prune_reason = 'missingFile' WHERE id = 'snp-ok';"
        ),
    );
}

/// 9. `printer_cameras`' source-kind pairing: `hostWebcam` requires
///    `webcam_name` and no `snapshot_url`; `snapshotUrl` requires
///    `snapshot_url` and none of the host-webcam columns.
#[test]
fn printer_camera_checks_reject_invalid_rows() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");

    assert_rejected(
        connection.execute(
            "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at)
             VALUES ('prn-a', 'hostWebcam', 'http://192.0.2.1:8080/snapshot', ?1)",
            [NOW],
        ),
        "a hostWebcam row with no webcam_name",
    );
    assert_rejected(
        connection.execute(
            "INSERT INTO printer_cameras(printer_id, source_kind, webcam_name, updated_at)
             VALUES ('prn-a', 'snapshotUrl', 'webcam', ?1)",
            [NOW],
        ),
        "a snapshotUrl row with no snapshot_url",
    );

    exec(
        &connection,
        &format!(
            "INSERT INTO printer_cameras(printer_id, source_kind, webcam_name, updated_at)
             VALUES ('prn-a', 'hostWebcam', 'webcam', '{NOW}');"
        ),
    );
}

/// 10. `printer_alert_defaults.offline_after_minutes` only accepts NULL
///     (off), 1, 5, or 15.
#[test]
fn printer_alert_defaults_offline_minutes_are_constrained() {
    let (_temp, connection) = migrated();
    seed_printer(&connection, "prn-a");

    assert_rejected(
        connection.execute(
            "INSERT INTO printer_alert_defaults(
                 printer_id, offline_after_minutes, notifications, snapshot_on_incident,
                 snapshot_on_completion, updated_at
             ) VALUES ('prn-a', 10, 'follow', 1, 1, ?1)",
            [NOW],
        ),
        "an offline_after_minutes value outside {1,5,15}",
    );

    exec(
        &connection,
        &format!(
            "INSERT INTO printer_alert_defaults(
                 printer_id, offline_after_minutes, notifications, snapshot_on_incident,
                 snapshot_on_completion, updated_at
             ) VALUES ('prn-a', NULL, 'follow', 1, 1, '{NOW}');"
        ),
    );
}

/// 11. A crash before the migration's commit leaves the v8 database
///     completely unchanged (no v9 tables, no v9 ledger row, the
///     `operations` table's rebuilt schema rolled back, and the seeded
///     row still present).
#[test]
fn a_crash_before_commit_leaves_the_database_unchanged_at_v8() {
    let temp = tempfile::tempdir().expect("temporary root");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let _lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let mut connection = v8_database(&paths);
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

    let error = apply_through_failing_before_commit(&mut connection, 9)
        .expect_err("the injected failure must surface");
    assert!(matches!(error, StorageError::MigrationFailed));

    assert_eq!(
        count(&connection, "SELECT user_version FROM pragma_user_version"),
        8,
        "the schema version must roll back to v8"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM schema_migrations WHERE version = 9"
        ),
        0,
        "no v9 ledger row may survive the rollback"
    );
    assert_eq!(
        count(
            &connection,
            "SELECT COUNT(*) FROM sqlite_schema WHERE name IN
                 ('incidents', 'attention_events', 'camera_snapshots', 'incident_events',
                  'printer_cameras', 'printer_alert_defaults', 'operations_p8')"
        ),
        0,
        "the v9 tables must roll back too"
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

/// 12. No `incidents`, `attention_events`, `camera_snapshots`,
///     `incident_events`, `printer_cameras`, `printer_alert_defaults`, or
///     `settings` column name contains `credential`, `secret`, `token`,
///     or `password` (global constraint 2/3: credentials never enter
///     these tables).
#[test]
fn no_new_p8_column_can_hold_a_credential() {
    let (_temp, connection) = migrated();

    for table in [
        "incidents",
        "attention_events",
        "camera_snapshots",
        "incident_events",
        "printer_cameras",
        "printer_alert_defaults",
        "settings",
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
            for forbidden in ["credential", "secret", "token", "password"] {
                assert!(
                    !lower.contains(forbidden),
                    "{table}.{column} must not look like it holds a {forbidden}"
                );
            }
        }
    }
}
