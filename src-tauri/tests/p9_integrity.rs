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
