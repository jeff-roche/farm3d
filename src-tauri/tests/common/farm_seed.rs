//! Raw-SQL seeds shared by the P9 migration and integrity tests. Every
//! helper writes valid rows (all CHECKs and foreign keys satisfied) so a
//! test can then break exactly one thing. Included with
//! `#[path = "common/farm_seed.rs"] mod farm_seed;` so the P9 tests don't
//! pull in the whole `common` harness.
#![allow(dead_code)]

use rusqlite::Connection;

pub const NOW: &str = "2026-01-01T00:00:00.000Z";
pub const GCODE_HASH: &str = "cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc";

pub fn exec(connection: &Connection, sql: &str) {
    connection
        .execute_batch(sql)
        .unwrap_or_else(|error| panic!("seed failed: {error}\n{sql}"));
}

pub fn count(connection: &Connection, sql: &str) -> i64 {
    connection
        .query_row(sql, [], |row| row.get(0))
        .expect("count")
}

pub fn seed_printer(connection: &Connection, id: &str) {
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

/// A blob row (no file).
pub fn seed_blob(connection: &Connection, sha256: &str, size: i64) {
    exec(
        connection,
        &format!(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at)
             VALUES ('{sha256}', {size}, '{NOW}');"
        ),
    );
}

/// An external Slice Revision `id` over the shared G-code blob (which it
/// creates on first use) and a Model. `target_json` is `{}` (no target).
pub fn seed_slice_revision(connection: &Connection, id: &str) {
    seed_slice_revision_targeting(connection, id, "{}");
}

/// [`seed_slice_revision`] with an explicit `target_json` (Slice Revisions
/// are immutable, so the target is set at insert).
pub fn seed_slice_revision_targeting(connection: &Connection, id: &str, target_json: &str) {
    exec(
        connection,
        &format!(
            "INSERT OR IGNORE INTO content_blobs(sha256, size_bytes, created_at)
               VALUES ('{GCODE_HASH}', 200, '{NOW}');
             INSERT OR IGNORE INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
               VALUES ('mdl-a', 1, 'Model', 'gcode', 'managed', '{NOW}', '{NOW}');
             INSERT OR IGNORE INTO model_source_revisions(
               id, model_id, sequence, content_sha256, size_bytes, format, origin,
               source_file_name, source_path, captured_at, inspector_version, inspection_json
             ) VALUES ('msr-a', 'mdl-a', 1, '{GCODE_HASH}', 200, 'gcode', 'import', 'part.gcode',
                       '/src/part.gcode', '{NOW}', 1, '{{}}');
             INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
               gcode_size, target_json, facts_json, requires_manual_printer_selection,
               estimates_json, created_at)
             VALUES ('{id}', 'external', 'mdl-a', 'msr-a', '{GCODE_HASH}', 200, '{target_json}', '{{}}', 1,
                     '{{}}', '{NOW}');"
        ),
    );
}

pub fn seed_spool(connection: &Connection, id: &str, spool_number: i64) {
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

pub fn seed_amount_event(
    connection: &Connection,
    id: &str,
    spool_id: &str,
    sequence: i64,
    reservation_id: Option<&str>,
) {
    let reservation = reservation_id.map_or("NULL".to_string(), |id| format!("'{id}'"));
    exec(
        connection,
        &format!(
            "INSERT INTO spool_amount_events(id, spool_id, sequence, kind, before_mg, after_mg,
               confidence_after, reservation_id, occurred_at)
             VALUES ('{id}', '{spool_id}', {sequence}, 'consumption', 1000000, 900000,
                     'estimated', {reservation}, '{NOW}');"
        ),
    );
}

pub fn seed_reservation(connection: &Connection, id: &str, spool_id: &str, holder_id: &str) {
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

pub fn seed_queue_entry(connection: &Connection, id: &str, slice_revision_id: &str, position: i64) {
    exec(
        connection,
        &format!(
            "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
               state, position, policy, preference, estimate_mg, estimate_source,
               created_at, updated_at)
             VALUES ('{id}', 1, '{slice_revision_id}', 'qln-{id}', 1, 'queued', {position},
                     'manual', 'loadedFirst', 500000, 'operatorEntered', '{NOW}', '{NOW}');"
        ),
    );
}

/// Printer -> Slice Revision -> Spool -> Reservation -> Queue Entry ->
/// assigned Job, ids suffixed with `suffix`. Returns the Job's id.
pub fn seed_job_chain(connection: &Connection, suffix: &str) -> String {
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
    exec(
        connection,
        &format!(
            "INSERT INTO jobs(
                 id, revision, queue_entry_id, slice_revision_id, printer_id, printer_snapshot_json,
                 spool_id, reservation_id, estimate_mg, state, settlement, assigned_by,
                 created_at, updated_at
             ) VALUES ('{job_id}', 1, '{queue_entry_id}', '{slice_revision_id}', '{printer_id}', '{{}}',
                       '{spool_id}', '{reservation_id}', 500000, 'assigned', 'open', 'operator',
                       '{NOW}', '{NOW}');"
        ),
    );
    job_id
}

/// An open `printer.offline` Attention Event for `printer_id` (the source
/// row need not exist: only `printer_id` is a foreign key, and it is
/// nullable).
pub fn seed_printer_event(connection: &Connection, id: &str, printer_id: &str, resolved: bool) {
    let (resolved_at, resolution, read_at) = if resolved {
        (
            "'2026-01-02T00:00:00.000Z'",
            "'sourceRemoved'",
            "'2026-01-02T00:00:00.000Z'",
        )
    } else {
        ("NULL", "NULL", "NULL")
    };
    exec(
        connection,
        &format!(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at,
                 read_at, resolved_at, resolution
             ) VALUES ('{id}', 'printer.offline:printer:{printer_id}', 'printer.offline',
                       'warning', 1, 'auto', 'connectivity', 'printer', '{printer_id}', NULL,
                       '{{}}', '{{}}', 'Offline.', 'live', '{NOW}', '{NOW}',
                       {read_at}, {resolved_at}, {resolution});"
        ),
    );
}

/// A `job.completed` Attention Event for `job_id` with `evidence_json`.
pub fn seed_completed_event(
    connection: &Connection,
    id: &str,
    job_id: &str,
    evidence_json: Option<&str>,
) {
    let evidence = evidence_json.map_or("NULL".to_string(), |json| format!("'{json}'"));
    exec(
        connection,
        &format!(
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, job_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at, evidence_json
             ) VALUES ('{id}', 'job.completed:job:{job_id}', 'job.completed', 'info', 0, 'manual',
                       'completion', 'job', '{job_id}', '{job_id}', '{{}}', '{{}}', 'Done.',
                       'live', '{NOW}', '{NOW}', {evidence});"
        ),
    );
}

pub fn seed_incident(connection: &Connection, id: &str, printer_id: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO incidents(id, kind, printer_id, job_id, printer_snapshot_json, opened_at)
             VALUES ('{id}', 'printer.hostFailed', '{printer_id}', NULL, '{{}}', '{NOW}');"
        ),
    );
}

/// An `incident`-trigger snapshot `id` for `printer_id`/`incident_id`, its
/// file at `rel_path`.
pub fn seed_incident_snapshot(
    connection: &Connection,
    id: &str,
    printer_id: &str,
    incident_id: &str,
    rel_path: &str,
) {
    exec(
        connection,
        &format!(
            "INSERT INTO camera_snapshots(
                 id, printer_id, incident_id, trigger, captured_at, content_type, byte_len,
                 sha256, rel_path
             ) VALUES ('{id}', '{printer_id}', '{incident_id}', 'incident', '{NOW}',
                       'image/jpeg', 100, '{sha}', '{rel_path}');",
            sha = "a".repeat(64),
        ),
    );
}

/// A `manual` snapshot `id` (needs no Incident or Job).
pub fn seed_manual_snapshot(connection: &Connection, id: &str, printer_id: &str, rel_path: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO camera_snapshots(
                 id, printer_id, trigger, operation_id, captured_at, content_type, byte_len,
                 sha256, rel_path
             ) VALUES ('{id}', '{printer_id}', 'manual', '{id}-op', '{NOW}', 'image/jpeg', 100,
                       '{sha}', '{rel_path}');",
            sha = "b".repeat(64),
        ),
    );
}

pub fn seed_evidence_captured_event(
    connection: &Connection,
    id: &str,
    incident_id: &str,
    snapshot_id: &str,
) {
    exec(
        connection,
        &format!(
            "INSERT INTO incident_events(id, incident_id, sequence, kind, snapshot_id, detail_json, at)
             VALUES ('{id}', '{incident_id}', 1, 'evidenceCaptured', '{snapshot_id}', '{{}}', '{NOW}');"
        ),
    );
}

/// A Host Operation `id` on `printer_id` in `state`; `upload`/`start` kinds
/// carry `gcode_sha256`.
pub fn seed_host_operation(
    connection: &Connection,
    id: &str,
    printer_id: &str,
    kind: &str,
    state: &str,
    gcode_sha256: &str,
) {
    let (sha, size, mark) = match kind {
        "upload" => (format!("'{gcode_sha256}'"), "200", "NULL"),
        "start" => (format!("'{gcode_sha256}'"), "200", "0"),
        _ => ("NULL".to_string(), "NULL", "NULL"),
    };
    exec(
        connection,
        &format!(
            "INSERT INTO host_operations(id, operation_id, printer_id, kind, gcode_sha256,
               gcode_size, host_path, history_mark, endpoint_json, state, created_at)
             VALUES ('{id}', '{id}-op', '{printer_id}', '{kind}', {sha}, {size}, 'part.gcode',
                     {mark}, '{{}}', '{state}', '{NOW}');"
        ),
    );
}

pub fn seed_pending_credential_cleanup(connection: &Connection, reference: &str, reason: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO pending_credential_cleanup(credential_ref, printer_id, reason, created_at)
             VALUES ('{reference}', NULL, '{reason}', '{NOW}');"
        ),
    );
}

pub fn seed_operation(connection: &Connection, id: &str, kind: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO operations(id, kind, request_digest, created_at)
             VALUES ('{id}', '{kind}', 'digest', '{NOW}');"
        ),
    );
}

/// A Farm with a row in every domain the catalogue reads and no
/// violation. Callers own `PRAGMA foreign_keys` (leave it on). Files:
/// `blob_file_hash`'s blob has a row only (the caller writes files for
/// the roots tests).
pub fn seed_every_domain(connection: &Connection) {
    let job_id = seed_job_chain(connection, "a");
    // A completed Job with a correction event on its own Spool, backed by
    // a reservation the amount event names.
    exec(
        connection,
        "UPDATE jobs SET state = 'completed', settlement = 'settled', settlement_method = 'estimated',
                ended_at = '2026-01-02T00:00:00.000Z' WHERE id = 'job-a';",
    );
    seed_amount_event(connection, "sev-a", "spl-a", 1, Some("rsv-a"));
    exec(
        connection,
        "UPDATE jobs SET correction_event_id = 'sev-a' WHERE id = 'job-a';",
    );
    // Printer, incident, incident evidence, completion evidence.
    seed_incident(connection, "inc-a", "prn-a");
    seed_incident_snapshot(
        connection,
        "snp-a",
        "prn-a",
        "inc-a",
        "snapshots/2026/01/snp-a.jpg",
    );
    seed_evidence_captured_event(connection, "iev-a", "inc-a", "snp-a");
    seed_manual_snapshot(connection, "snp-b", "prn-a", "snapshots/2026/01/snp-b.jpg");
    seed_completed_event(
        connection,
        "att-done",
        &job_id,
        Some("{\"status\":\"captured\",\"snapshotId\":\"snp-b\"}"),
    );
    seed_printer_event(connection, "att-off", "prn-a", false);
    // An unresolved upload whose blob exists, and a slice target naming a
    // present Printer.
    seed_host_operation(
        connection,
        "hop-a",
        "prn-a",
        "upload",
        "dispatching",
        GCODE_HASH,
    );
    seed_slice_revision_targeting(
        connection,
        "slr-t",
        "{\"target\":{\"printerId\":\"prn-a\"}}",
    );
    exec(
        connection,
        &format!(
            "INSERT INTO slice_preparations(id, model_id, source_revision_id, revision,
               document_json, created_at, updated_at)
             VALUES ('prp-a', 'mdl-a', 'msr-a', 1,
               '{{\"target\":{{\"printerId\":\"prn-a\"}}}}', '{NOW}', '{NOW}');"
        ),
    );
    seed_pending_credential_cleanup(connection, "cred-a", "cleared");
    seed_operation(connection, "op-a", "moveSpool");
}
