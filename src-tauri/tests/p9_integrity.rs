//! The P9 reference-integrity catalogue (`persistence::integrity::check`,
//! D17): every rule has a positive test (seed exactly one violation, expect
//! exactly that rule) and a negative test (the near-miss that must stay
//! quiet); the tolerated rules report as tolerated; a clean every-domain
//! Farm reports nothing; counts equal `SELECT count(*)`; samples are
//! capped and never a path.

use std::fs;
use std::path::Path;

use farm3d_lib::persistence::integrity::{
    check, IntegrityOutcome, IntegrityReport, IntegrityRoots, IntegrityRule, SAMPLE_LIMIT,
};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};

#[path = "common/farm_seed.rs"]
mod farm_seed;
use farm_seed::*;

struct Farm {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Storage,
    connection: rusqlite::Connection,
    roots: IntegrityRoots,
}

/// A migrated Farm with every domain seeded, its blob and media files on
/// disk, and no violation.
fn farm() -> Farm {
    let temp = tempfile::tempdir().expect("temp");
    let paths =
        StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).expect("paths");
    let lease = MetadataRootLease::acquire(&paths).expect("lease");
    let storage = Storage::open(paths.clone(), &lease).expect("storage");
    let connection = rusqlite::Connection::open(paths.database()).expect("raw");
    connection
        .execute_batch("PRAGMA foreign_keys = ON")
        .expect("fks on");
    seed_every_domain(&connection);
    let roots = IntegrityRoots::from_paths(&paths);
    write_blob(&roots, GCODE_HASH, 200);
    write_media(&roots, "snapshots/2026/01/snp-a.jpg");
    write_media(&roots, "snapshots/2026/01/snp-b.jpg");
    Farm {
        _temp: temp,
        _lease: lease,
        storage,
        connection,
        roots,
    }
}

fn write_blob(roots: &IntegrityRoots, sha256: &str, size: usize) {
    let path = roots
        .content_root
        .join("blobs/sha256")
        .join(&sha256[..2])
        .join(sha256);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, vec![7_u8; size]).unwrap();
}

fn write_media(roots: &IntegrityRoots, relative: &str) {
    let path = roots.media_root.join(relative);
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, b"img").unwrap();
}

fn run(farm: &Farm) -> IntegrityReport {
    check(&farm.connection, Some(&farm.roots)).expect("check")
}

/// Exactly one finding, for `rule`, with `count` rows and this outcome.
fn assert_only(
    report: &IntegrityReport,
    rule: IntegrityRule,
    outcome: IntegrityOutcome,
    count: i64,
) {
    let rules: Vec<_> = report
        .findings
        .iter()
        .map(|f| (f.rule, f.outcome, f.count))
        .collect();
    assert_eq!(rules, vec![(rule, outcome, count)], "{report:#?}");
}

fn assert_clean(report: &IntegrityReport) {
    assert!(report.findings.is_empty(), "{report:#?}");
}

const V: IntegrityOutcome = IntegrityOutcome::Violation;
const T: IntegrityOutcome = IntegrityOutcome::Tolerated;

#[test]
fn a_clean_every_domain_farm_reports_nothing() {
    let farm = farm();
    let report = run(&farm);
    assert_clean(&report);
    assert!(report.violations().is_empty());
    assert!(report.tolerated().is_empty());
    // The seed reaches every catalogue domain (the negative tests below
    // depend on it).
    for table in [
        "jobs",
        "spool_amount_events",
        "spool_reservations",
        "attention_events",
        "camera_snapshots",
        "incident_events",
        "host_operations",
        "content_blobs",
        "slice_revisions",
        "slice_preparations",
        "pending_credential_cleanup",
        "operations",
    ] {
        assert!(report.counts[table] >= 1, "{table} is seeded");
    }
}

#[test]
fn counts_match_select_count_for_every_table() {
    let farm = farm();
    let report = run(&farm);
    let mut statement = farm
        .connection
        .prepare("SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'")
        .unwrap();
    let names: Vec<String> = statement
        .query_map([], |r| r.get(0))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    assert_eq!(report.counts.len(), names.len());
    for name in names {
        assert_eq!(
            report.counts[&name],
            count(
                &farm.connection,
                &format!("SELECT count(*) FROM \"{name}\"")
            ),
            "{name}"
        );
    }
}

#[test]
fn fk_fires_on_a_dangling_foreign_key_only() {
    let farm = farm();
    farm.connection
        .execute_batch("PRAGMA foreign_keys = OFF")
        .unwrap();
    exec(
        &farm.connection,
        "UPDATE jobs SET printer_id = 'prn-gone' WHERE id = 'job-a';",
    );
    let report = run(&farm);
    assert_only(&report, IntegrityRule::Fk, V, 1);
    assert_eq!(report.findings[0].sample, vec!["jobs#1".to_string()]);
}

#[test]
fn job_correction_event_fires_on_a_missing_or_foreign_spool_event() {
    let farm = farm();
    exec(
        &farm.connection,
        "UPDATE jobs SET correction_event_id = 'sev-gone' WHERE id = 'job-a';",
    );
    assert_only(&run(&farm), IntegrityRule::JobCorrectionEvent, V, 1);

    // An event that exists but belongs to another Spool is also broken.
    seed_spool(&farm.connection, "spl-b", 2);
    seed_amount_event(&farm.connection, "sev-b", "spl-b", 1, None);
    exec(
        &farm.connection,
        "UPDATE jobs SET correction_event_id = 'sev-b' WHERE id = 'job-a';",
    );
    assert_only(&run(&farm), IntegrityRule::JobCorrectionEvent, V, 1);

    // The valid event on the Job's own Spool, and NULL, are quiet.
    exec(
        &farm.connection,
        "UPDATE jobs SET correction_event_id = 'sev-a' WHERE id = 'job-a';",
    );
    assert_clean(&run(&farm));
    exec(
        &farm.connection,
        "UPDATE jobs SET correction_event_id = NULL WHERE id = 'job-a';",
    );
    assert_clean(&run(&farm));
}

#[test]
fn amount_event_reservation_fires_on_a_missing_reservation() {
    let farm = farm();
    seed_amount_event(&farm.connection, "sev-x", "spl-a", 2, Some("rsv-gone"));
    assert_only(&run(&farm), IntegrityRule::AmountEventReservation, V, 1);
}

#[test]
fn amount_event_reservation_is_quiet_for_a_present_or_null_reservation() {
    let farm = farm();
    seed_amount_event(&farm.connection, "sev-x", "spl-a", 2, Some("rsv-a"));
    seed_amount_event(&farm.connection, "sev-y", "spl-a", 3, None);
    assert_clean(&run(&farm));
}

#[test]
fn reservation_holder_fires_on_a_missing_job_or_another_holder_kind() {
    let farm = farm();
    seed_reservation(&farm.connection, "rsv-x", "spl-a", "job-gone");
    assert_only(&run(&farm), IntegrityRule::ReservationHolder, V, 1);
    exec(
        &farm.connection,
        &format!(
            "INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg,
               state, operation_id, created_at)
             VALUES ('rsv-y', 'spl-a', 'queueEntry', 'job-a', 1, 'active', 'rsv-y-op', '{NOW}');"
        ),
    );
    assert_only(&run(&farm), IntegrityRule::ReservationHolder, V, 2);
}

#[test]
fn reservation_holder_is_quiet_for_a_job_holder() {
    let farm = farm();
    seed_reservation(&farm.connection, "rsv-x", "spl-a", "job-a");
    assert_clean(&run(&farm));
}

#[test]
fn attention_source_fires_on_a_missing_printer_unless_resolved_source_removed() {
    let farm = farm();
    seed_printer_event(&farm.connection, "att-gone", "prn-gone", false);
    assert_only(&run(&farm), IntegrityRule::AttentionSource, V, 1);
}

#[test]
fn attention_source_tolerates_a_missing_printer_when_resolved_source_removed() {
    let farm = farm();
    seed_printer_event(&farm.connection, "att-gone", "prn-gone", true);
    assert_clean(&run(&farm));
}

/// Printer delete resolves only the Printer's *open* Events
/// `sourceRemoved` (P8 D8); one resolved earlier keeps its resolution and
/// loses only `printer_id`. Its missing source is history, not a dangling
/// reference (Task 11's matrix: deleting Printer B with a resolved
/// `printer.connectionError`).
#[test]
fn attention_source_tolerates_a_missing_printer_for_an_event_resolved_before_the_delete() {
    let farm = farm();
    seed_printer_event(&farm.connection, "att-gone", "prn-gone", true);
    exec(
        &farm.connection,
        "UPDATE attention_events SET resolution = 'conditionCleared' WHERE id = 'att-gone';",
    );
    assert_clean(&run(&farm));
}

#[test]
fn attention_source_fires_for_a_job_source_with_no_job_row() {
    // Without the foreign key a Job source can dangle too (a restore that
    // skipped `fk` would be caught by `fk` as well; the rule stands alone).
    let farm = farm();
    farm.connection
        .execute_batch("PRAGMA foreign_keys = OFF")
        .unwrap();
    seed_completed_event(&farm.connection, "att-x", "job-gone", None);
    let report = run(&farm);
    let rules: Vec<_> = report.findings.iter().map(|f| f.rule).collect();
    assert_eq!(
        rules,
        vec![IntegrityRule::Fk, IntegrityRule::AttentionSource]
    );
}

#[test]
fn completion_evidence_fires_when_captured_names_a_missing_snapshot() {
    let farm = farm();
    exec(
        &farm.connection,
        "UPDATE attention_events SET evidence_json = '{\"status\":\"captured\",\"snapshotId\":\"snp-gone\"}'
         WHERE id = 'att-done';",
    );
    assert_only(&run(&farm), IntegrityRule::CompletionEvidence, V, 1);
}

#[test]
fn completion_evidence_is_quiet_for_skipped_or_absent_evidence() {
    let farm = farm();
    exec(
        &farm.connection,
        "UPDATE attention_events SET evidence_json = '{\"status\":\"skipped\",\"reason\":\"noCamera\",\"errorKind\":null}'
         WHERE id = 'att-done';",
    );
    assert_clean(&run(&farm));
    exec(
        &farm.connection,
        "UPDATE attention_events SET evidence_json = NULL WHERE id = 'att-done';",
    );
    assert_clean(&run(&farm));
}

#[test]
fn host_operation_gcode_fires_for_an_unresolved_upload_or_start_with_no_blob_row() {
    let farm = farm();
    let missing = "d".repeat(64);
    seed_printer(&farm.connection, "prn-b");
    seed_host_operation(
        &farm.connection,
        "hop-u",
        "prn-b",
        "upload",
        "uncertain",
        &missing,
    );
    assert_only(&run(&farm), IntegrityRule::HostOperationGcode, V, 1);
    seed_printer(&farm.connection, "prn-c");
    seed_host_operation(
        &farm.connection,
        "hop-s",
        "prn-c",
        "start",
        "reconciling",
        &missing,
    );
    assert_only(&run(&farm), IntegrityRule::HostOperationGcode, V, 2);
}

#[test]
fn host_operation_gcode_is_quiet_for_terminal_or_non_gcode_operations() {
    let farm = farm();
    let missing = "d".repeat(64);
    seed_printer(&farm.connection, "prn-b");
    seed_host_operation(
        &farm.connection,
        "hop-t",
        "prn-b",
        "upload",
        "succeeded",
        &missing,
    );
    seed_host_operation(
        &farm.connection,
        "hop-f",
        "prn-b",
        "start",
        "failed",
        &missing,
    );
    seed_host_operation(
        &farm.connection,
        "hop-p",
        "prn-b",
        "pause",
        "abandoned",
        &missing,
    );
    seed_printer(&farm.connection, "prn-c");
    seed_host_operation(
        &farm.connection,
        "hop-c",
        "prn-c",
        "cancel",
        "dispatching",
        &missing,
    );
    assert_clean(&run(&farm));
}

// --- embeddedReference (Task 11's audit) ------------------------------------------

/// A `job_events` row for Job A with `detail_json`.
fn seed_job_event(connection: &rusqlite::Connection, id: &str, sequence: i64, detail: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO job_events(id, job_id, sequence, kind, from_state, to_state, detail_json, at)
             VALUES ('{id}', 'job-a', {sequence}, 'materialCorrected', 'completed', 'completed',
                     '{detail}', '{NOW}');"
        ),
    );
}

/// An `incident_events` note row on Incident A with `detail_json`.
fn seed_incident_note(connection: &rusqlite::Connection, id: &str, sequence: i64, detail: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO incident_events(id, incident_id, sequence, kind, detail_json, at)
             VALUES ('{id}', 'inc-a', {sequence}, 'noteAdded', '{detail}', '{NOW}');"
        ),
    );
}

#[test]
fn embedded_reference_fires_for_a_job_event_naming_a_missing_row() {
    let farm = farm();
    seed_job_event(
        &farm.connection,
        "jev-x1",
        1,
        "{\"correctionEventId\":\"sev-gone\"}",
    );
    assert_only(&run(&farm), IntegrityRule::EmbeddedReference, V, 1);
    seed_job_event(
        &farm.connection,
        "jev-x2",
        2,
        "{\"hostOperationId\":\"hop-gone\"}",
    );
    let report = run(&farm);
    assert_only(&report, IntegrityRule::EmbeddedReference, V, 2);
    assert_eq!(
        report
            .finding(IntegrityRule::EmbeddedReference)
            .unwrap()
            .sample,
        vec!["jev-x1", "jev-x2"]
    );
}

#[test]
fn embedded_reference_fires_for_a_job_failure_naming_a_missing_host_operation() {
    let farm = farm();
    exec(
        &farm.connection,
        "UPDATE jobs SET last_failure_json =
           '{\"kind\":\"hostOperationAbandoned\",\"at\":\"x\",\"hostOperationId\":\"hop-gone\"}'
         WHERE id = 'job-a';",
    );
    assert_only(&run(&farm), IntegrityRule::EmbeddedReference, V, 1);
}

#[test]
fn embedded_reference_fires_for_an_attention_detail_naming_a_missing_spool() {
    let farm = farm();
    exec(
        &farm.connection,
        "UPDATE attention_events SET detail_json =
           '{\"kind\":\"requirementMaterialReconciliation\",\"requirementStatus\":\"pending\",\"spoolId\":\"spl-gone\"}'
         WHERE id = 'att-off';",
    );
    assert_only(&run(&farm), IntegrityRule::EmbeddedReference, V, 1);
}

#[test]
fn embedded_reference_fires_for_an_incident_entry_naming_a_missing_event_or_snapshot() {
    let farm = farm();
    seed_incident_note(
        &farm.connection,
        "iev-x1",
        2,
        "{\"kind\":\"evidencePinned\",\"snapshotId\":\"snp-gone\"}",
    );
    assert_only(&run(&farm), IntegrityRule::EmbeddedReference, V, 1);
    seed_incident_note(
        &farm.connection,
        "iev-x2",
        3,
        "{\"kind\":\"eventLinked\",\"eventId\":\"att-gone\"}",
    );
    assert_only(&run(&farm), IntegrityRule::EmbeddedReference, V, 2);
}

#[test]
fn embedded_reference_is_quiet_when_every_embedded_id_names_its_row() {
    let farm = farm();
    // The seed's `iev-a` already names `snp-a` in its detail.
    seed_job_event(
        &farm.connection,
        "jev-x1",
        1,
        "{\"correctionEventId\":\"sev-a\"}",
    );
    seed_job_event(
        &farm.connection,
        "jev-x2",
        2,
        "{\"hostOperationId\":\"hop-a\",\"kind\":\"pause\"}",
    );
    seed_job_event(&farm.connection, "jev-x3", 3, "{\"hostJobId\":\"0000A1\"}");
    exec(
        &farm.connection,
        "INSERT INTO job_events(id, job_id, sequence, kind, to_state, at)
           VALUES ('jev-x4', 'job-a', 4, 'completed', 'completed', 'x');
         UPDATE jobs SET last_failure_json =
           '{\"kind\":\"hostOperationFailed\",\"at\":\"x\",\"hostOperationId\":\"hop-a\",\"failure\":{}}'
         WHERE id = 'job-a';
         UPDATE attention_events SET detail_json =
           '{\"kind\":\"requirementMaterialReconciliation\",\"requirementStatus\":\"pending\",\"spoolId\":\"spl-a\"}'
         WHERE id = 'att-off';",
    );
    seed_incident_note(
        &farm.connection,
        "iev-x1",
        2,
        "{\"kind\":\"eventLinked\",\"eventId\":\"att-off\"}",
    );
    seed_incident_note(
        &farm.connection,
        "iev-x2",
        3,
        "{\"kind\":\"noteAdded\",\"text\":\"x\"}",
    );
    assert_clean(&run(&farm));
}

// --- the Task 11 audit ------------------------------------------------------------

/// Every `*_json` column and every `TEXT` column ending `_id` that has no
/// foreign key, and how the catalogue covers it (spec D17, Task 11 Step 2).
/// A column added later fails the audit test below until it is classified
/// here and, if it is a reference, gets a rule or a named tolerance.
const LOOSE_COLUMNS: &[(&str, &str, &str)] = &[
    // Covered by a rule.
    ("attention_events", "source_id", "rule attentionSource"),
    (
        "attention_events",
        "evidence_json",
        "rule completionEvidence ($.snapshotId)",
    ),
    (
        "attention_events",
        "detail_json",
        "rule embeddedReference ($.spoolId)",
    ),
    (
        "incident_events",
        "detail_json",
        "rule embeddedReference ($.eventId, $.snapshotId)",
    ),
    (
        "job_events",
        "detail_json",
        "rule embeddedReference ($.correctionEventId, $.hostOperationId)",
    ),
    (
        "jobs",
        "last_failure_json",
        "rule embeddedReference ($.hostOperationId)",
    ),
    ("jobs", "correction_event_id", "rule jobCorrectionEvent"),
    (
        "spool_amount_events",
        "reservation_id",
        "rule amountEventReservation",
    ),
    ("spool_reservations", "holder_id", "rule reservationHolder"),
    (
        "slice_revisions",
        "target_json",
        "rule sliceTargetPrinter (tolerated)",
    ),
    (
        "slice_preparations",
        "document_json",
        "rule preparationTargetPrinter (tolerated)",
    ),
    // Named tolerances (integrity.rs, D17).
    (
        "pending_credential_cleanup",
        "printer_id",
        "tolerated: provenance; the Printer is usually gone",
    ),
    (
        "migration_warnings",
        "details_json",
        "tolerated: an advisory ledger of a one-time migration",
    ),
    (
        "printers",
        "connection_json",
        "tolerated: credentialRef names the credential store (D10)",
    ),
    // Not a reference to a row.
    (
        "attention_events",
        "subject_snapshot_json",
        "a point-in-time label snapshot",
    ),
    (
        "incidents",
        "printer_snapshot_json",
        "a point-in-time Printer snapshot",
    ),
    (
        "jobs",
        "printer_snapshot_json",
        "a point-in-time Printer snapshot",
    ),
    ("camera_snapshots", "operation_id", "an idempotency key"),
    ("host_operations", "operation_id", "an idempotency key"),
    ("incident_events", "operation_id", "an idempotency key"),
    ("job_events", "operation_id", "an idempotency key"),
    ("spool_movements", "operation_id", "an idempotency key"),
    ("spool_reservations", "operation_id", "an idempotency key"),
    (
        "host_operations",
        "endpoint_json",
        "the host endpoint at dispatch",
    ),
    (
        "host_operations",
        "failure_json",
        "a failure code and message",
    ),
    (
        "host_operations",
        "resolution_json",
        "evidence; historyJobId is the host's id",
    ),
    (
        "library_models",
        "link_observed_file_id",
        "a filesystem identity",
    ),
    (
        "model_source_revisions",
        "inspection_json",
        "the file's inspection",
    ),
    (
        "printer_status_snapshots",
        "telemetry_json",
        "a telemetry cache",
    ),
    ("printers", "catalog_model_id", "a bundled-catalog id"),
    ("printers", "last_known_good_json", "a resolved profile"),
    ("printers", "overrides_json", "profile overrides"),
    (
        "queue_entries",
        "lineage_id",
        "a grouping key with no table",
    ),
    (
        "reconciliation_requirements",
        "resolution_json",
        "a settlement method and amount",
    ),
    (
        "slice_operations",
        "failure_json",
        "a failure code and message",
    ),
    (
        "slice_operations",
        "plate_snapshot_json",
        "plate keys into the source's own geometry",
    ),
    ("slice_revisions", "estimates_json", "estimates"),
    ("slice_revisions", "facts_json", "slice facts"),
    (
        "slice_revisions",
        "runtime_json",
        "the slicer runtime's versions",
    ),
];

#[test]
fn the_audit_classifies_every_json_and_id_column_without_a_foreign_key() {
    let farm = farm();
    let mut statement = farm
        .connection
        .prepare(
            "SELECT m.name, p.name FROM sqlite_master m, pragma_table_info(m.name) p
              WHERE m.type = 'table' AND m.name NOT LIKE 'sqlite_%'
                AND (p.name LIKE '%!_json' ESCAPE '!'
                     OR (p.name LIKE '%!_id' ESCAPE '!' AND p.type = 'TEXT'))
                AND NOT EXISTS (SELECT 1 FROM pragma_foreign_key_list(m.name) f
                                 WHERE f.\"from\" = p.name)
              ORDER BY 1, 2",
        )
        .unwrap();
    let found: std::collections::BTreeSet<(String, String)> = statement
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))
        .unwrap()
        .map(Result::unwrap)
        .collect();
    let classified: std::collections::BTreeSet<(String, String)> = LOOSE_COLUMNS
        .iter()
        .map(|(table, column, _)| (table.to_string(), column.to_string()))
        .collect();
    assert_eq!(
        classified.len(),
        LOOSE_COLUMNS.len(),
        "no column is classified twice"
    );
    let unclassified: Vec<_> = found.difference(&classified).collect();
    assert!(unclassified.is_empty(), "classify these: {unclassified:?}");
    let stale: Vec<_> = classified.difference(&found).collect();
    assert!(stale.is_empty(), "no longer loose: {stale:?}");
}

#[test]
fn slice_target_printer_reports_tolerated_for_a_missing_target_printer() {
    let farm = farm();
    seed_slice_revision_targeting(
        &farm.connection,
        "slr-x",
        "{\"target\":{\"printerId\":\"prn-gone\"}}",
    );
    assert_only(&run(&farm), IntegrityRule::SliceTargetPrinter, T, 1);
    assert!(run(&farm).violations().is_empty());
}

#[test]
fn slice_target_printer_is_quiet_for_no_target_or_a_present_printer() {
    let farm = farm();
    seed_slice_revision(&farm.connection, "slr-x"); // no target
    assert_clean(&run(&farm)); // slr-t already targets the present prn-a
}

#[test]
fn preparation_target_printer_reports_tolerated_for_a_missing_target_printer() {
    let farm = farm();
    exec(
        &farm.connection,
        "UPDATE slice_preparations SET document_json = '{\"target\":{\"printerId\":\"prn-gone\"}}';",
    );
    assert_only(&run(&farm), IntegrityRule::PreparationTargetPrinter, T, 1);
}

#[test]
fn preparation_target_printer_is_quiet_for_no_target() {
    let farm = farm();
    exec(
        &farm.connection,
        "UPDATE slice_preparations SET document_json = '{}';",
    );
    assert_clean(&run(&farm));
}

#[test]
fn blob_file_fires_for_a_missing_or_wrong_size_file() {
    let farm = farm();
    let path = farm
        .roots
        .content_root
        .join("blobs/sha256")
        .join(&GCODE_HASH[..2])
        .join(GCODE_HASH);
    fs::remove_file(&path).unwrap();
    let report = run(&farm);
    assert_only(&report, IntegrityRule::BlobFile, V, 1);
    assert_eq!(report.findings[0].sample, vec![GCODE_HASH.to_string()]);

    write_blob(&farm.roots, GCODE_HASH, 199);
    assert_only(&run(&farm), IntegrityRule::BlobFile, V, 1);

    write_blob(&farm.roots, GCODE_HASH, 200);
    assert_clean(&run(&farm));
}

#[test]
fn media_file_fires_for_a_missing_unpruned_file_but_not_a_pruned_row() {
    let farm = farm();
    fs::remove_file(farm.roots.media_root.join("snapshots/2026/01/snp-b.jpg")).unwrap();
    let report = run(&farm);
    assert_only(&report, IntegrityRule::MediaFile, V, 1);
    assert_eq!(report.findings[0].sample, vec!["snp-b".to_string()]);

    // Once the row is pruned its file is not expected.
    exec(
        &farm.connection,
        &format!("UPDATE camera_snapshots SET pruned_at = '{NOW}', prune_reason = 'age' WHERE id = 'snp-b';"),
    );
    assert_clean(&run(&farm));
}

#[test]
fn orphan_blob_file_is_tolerated_and_a_pending_cleanup_row_claims_it() {
    let farm = farm();
    let orphan = "e".repeat(64);
    write_blob(&farm.roots, &orphan, 3);
    assert_only(&run(&farm), IntegrityRule::OrphanBlobFile, T, 1);

    exec(
        &farm.connection,
        &format!(
            "INSERT INTO pending_blob_cleanup(sha256, created_at) VALUES ('{orphan}', '{NOW}');"
        ),
    );
    assert_clean(&run(&farm));
}

#[test]
fn orphan_media_file_is_tolerated_and_only_image_files_count() {
    let farm = farm();
    write_media(&farm.roots, "snapshots/2026/01/snp-zzz.png");
    write_media(&farm.roots, "snapshots/2026/01/snp-zzz.tmp");
    let report = run(&farm);
    assert_only(&report, IntegrityRule::OrphanMediaFile, T, 1);
    assert_eq!(report.findings[0].sample, vec!["snp-zzz".to_string()]);

    // A pruned row's file is an orphan too: only unpruned rows claim files.
    exec(
        &farm.connection,
        &format!("UPDATE camera_snapshots SET pruned_at = '{NOW}', prune_reason = 'age' WHERE id = 'snp-b';"),
    );
    let report = run(&farm);
    assert_only(&report, IntegrityRule::OrphanMediaFile, T, 2);
}

#[test]
fn file_rules_are_skipped_without_roots() {
    let farm = farm();
    fs::remove_file(farm.roots.media_root.join("snapshots/2026/01/snp-b.jpg")).unwrap();
    write_blob(&farm.roots, &"e".repeat(64), 3);
    let report = check(&farm.connection, None).expect("check");
    assert_clean(&report);
}

#[test]
fn samples_are_capped_and_never_a_path() {
    let farm = farm();
    for index in 0..(SAMPLE_LIMIT + 5) {
        seed_slice_revision_targeting(
            &farm.connection,
            &format!("slr-x{index:02}"),
            "{\"target\":{\"printerId\":\"prn-gone\"}}",
        );
    }
    let report = run(&farm);
    let finding = report
        .finding(IntegrityRule::SliceTargetPrinter)
        .expect("finding");
    assert_eq!(finding.count, (SAMPLE_LIMIT + 5) as i64);
    assert_eq!(finding.sample.len(), SAMPLE_LIMIT);

    // File rules sample ids, hashes, and stems, not paths.
    write_media(&farm.roots, "snapshots/2026/01/snp-zzz.jpg");
    write_blob(&farm.roots, &"e".repeat(64), 3);
    fs::remove_file(farm.roots.media_root.join("snapshots/2026/01/snp-a.jpg")).unwrap();
    for finding in run(&farm).findings {
        for sample in finding.sample {
            assert!(!sample.contains('/') && !sample.contains('\\'), "{sample}");
            assert!(!Path::new(&sample).is_absolute());
        }
    }
}

#[test]
fn check_runs_in_one_transaction_and_composes_with_an_open_one() {
    let farm = farm();
    assert!(farm.connection.is_autocommit());
    let alone = run(&farm);
    assert!(
        farm.connection.is_autocommit(),
        "the read transaction is closed"
    );

    // Inside a caller's transaction the check reads that transaction.
    let transaction = farm.connection.unchecked_transaction().unwrap();
    seed_amount_event(&transaction, "sev-x", "spl-a", 2, Some("rsv-gone"));
    let inside = check(&transaction, None).expect("nested check");
    assert_eq!(inside.findings.len(), 1);
    assert!(
        !farm.connection.is_autocommit(),
        "the caller's transaction stays open"
    );
    transaction.rollback().unwrap();
    assert_eq!(run(&farm), alone);

    // And through the Storage read path, on a reader connection.
    let through_storage = farm
        .storage
        .read(|connection| Ok(check(connection, Some(&farm.roots)).expect("check")))
        .expect("read");
    assert_eq!(through_storage, alone);
}
