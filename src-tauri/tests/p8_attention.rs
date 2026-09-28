//! Task 5 (P8 D1/D2/D9): the `attention::repository` primitives, exercised
//! through the public crate API against a real migrated `Storage`, and the
//! per-Printer alert-default repository (`printers::alerts`). See the P8
//! design spec's D1 ("Vocabulary and ownership"), D2 ("Apply"/"Planner
//! rules"), and "Commands" (`list_attention`'s ordering/cursor).
//!
//! The projector, commands, and event stream that will call these
//! functions are a later task (6); this file only proves the repository
//! primitives themselves.

use chrono::{DateTime, Utc};

use farm3d_lib::attention::{
    dedup_key, repository as attention_repo, AttentionDetail, AttentionOrigin,
    AttentionResolution, AttentionSeverity, AttentionSource, AttentionSourceKind,
    AttentionSubject, Condition, ConditionKind, EvidenceOutcome,
};
use farm3d_lib::cameras::EvidenceSkipReason;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::alerts::{self, AlertDefaults, NotificationMode, OfflineAlertMinutes};

const NOW_TEXT: &str = "2026-09-28T09:00:00Z";

fn now() -> DateTime<Utc> {
    NOW_TEXT.parse().unwrap()
}

fn open_storage() -> (tempfile::TempDir, Storage) {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths =
        StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).expect("paths");
    let lease = MetadataRootLease::acquire(&paths).expect("lease");
    let storage = Storage::open(paths, &lease).expect("storage");
    (temp, storage)
}

fn exec(connection: &rusqlite::Connection, sql: &str) {
    connection
        .execute_batch(sql)
        .unwrap_or_else(|error| panic!("seed failed: {error}\n{sql}"));
}

fn seed_printer(connection: &rusqlite::Connection, id: &str, name: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES ('{id}', 1, '{name}', '', '', '', '', '', '', '{{}}', '{NOW_TEXT}', '{NOW_TEXT}');"
        ),
    );
}

/// A Printer -> Slice Revision -> Spool -> Reservation -> Queue Entry ->
/// Job chain, minimal enough for `attention_events`' FKs. The Printer
/// (`printer_id`) must already exist.
fn seed_job(connection: &rusqlite::Connection, job_id: &str, printer_id: &str) {
    let hash = "c".repeat(64);
    exec(
        connection,
        &format!(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at)
               VALUES ('{hash}', 1, '{NOW_TEXT}');
             INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
               VALUES ('mdl-{job_id}', 1, 'Model', 'gcode', 'managed', '{NOW_TEXT}', '{NOW_TEXT}');
             INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256, size_bytes,
               format, origin, source_file_name, source_path, captured_at, inspector_version,
               inspection_json)
               VALUES ('msr-{job_id}', 'mdl-{job_id}', 1, '{hash}', 1, 'gcode', 'import', 'p.gcode',
                       '/p.gcode', '{NOW_TEXT}', 1, '{{}}');
             INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
               gcode_size, target_json, facts_json, requires_manual_printer_selection,
               estimates_json, created_at)
               VALUES ('slr-{job_id}', 'external', 'mdl-{job_id}', 'msr-{job_id}', '{hash}', 1,
                       '{{}}', '{{}}', 1, '{{}}', '{NOW_TEXT}');
             INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
               color_name, diameter, nominal_mg, current_mg, confidence, lifecycle, created_at,
               updated_at)
               VALUES ('spl-{job_id}', 1, 1, 'Acme', 'PLA', 'Black', '1.75', 1000000, 1000000,
                       'measured', 'active', '{NOW_TEXT}', '{NOW_TEXT}');
             INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg, state,
               operation_id, created_at)
               VALUES ('rsv-{job_id}', 'spl-{job_id}', 'job', '{job_id}', 500000, 'active',
                       'rsv-{job_id}-op', '{NOW_TEXT}');
             INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
               state, position, policy, preference, estimate_mg, estimate_source, created_at,
               updated_at)
               VALUES ('qen-{job_id}', 1, 'slr-{job_id}', 'qln-{job_id}', 1, 'queued', 1, 'manual',
                       'loadedFirst', 500000, 'operatorEntered', '{NOW_TEXT}', '{NOW_TEXT}');
             INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
               printer_snapshot_json, spool_id, reservation_id, estimate_mg, state, settlement,
               assigned_by, created_at, updated_at)
               VALUES ('{job_id}', 1, 'qen-{job_id}', 'slr-{job_id}', '{printer_id}', '{{}}',
                       'spl-{job_id}', 'rsv-{job_id}', 500000, 'assigned', 'open', 'operator',
                       '{NOW_TEXT}', '{NOW_TEXT}');"
        ),
    );
}

fn subject(printer_name: &str) -> AttentionSubject {
    AttentionSubject {
        printer_name: Some(printer_name.to_string()),
        printer_location: Some("Bay A".to_string()),
        job_label: None,
        spool_number: None,
        spool_label: None,
    }
}

fn printer_offline(printer_id: &str, printer_name: &str) -> Condition {
    Condition {
        kind: ConditionKind::PrinterOffline,
        source_id: printer_id.to_string(),
        printer_id: Some(printer_id.to_string()),
        job_id: None,
        spool_id: None,
        requirement_id: None,
        subject: subject(printer_name),
        detail: AttentionDetail::PrinterOffline {
            unreachable_since: NOW_TEXT.to_string(),
        },
        acknowledge: false,
    }
}

#[test]
fn insert_amend_resolve_round_trip_through_the_public_api() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            seed_printer(connection, "prn-a", "Voron");
            let condition = printer_offline("prn-a", "Voron");

            let inserted =
                attention_repo::insert(tx, &condition, None, false, AttentionOrigin::Live, now())?;
            assert_eq!(inserted.revision, 1);
            assert_eq!(inserted.dedup_key, dedup_key(ConditionKind::PrinterOffline, "prn-a"));
            assert_eq!(inserted.source, AttentionSource {
                kind: AttentionSourceKind::Printer,
                id: "prn-a".to_string(),
            });

            let later = now() + chrono::Duration::minutes(1);
            let amended = attention_repo::amend(
                tx,
                &inserted.id,
                &inserted.detail,
                AttentionSeverity::Fatal,
                "Voron (Bay A) is offline, still.",
                true,
                later,
            )?;
            assert_eq!(amended.revision, 2);
            assert_eq!(amended.severity, AttentionSeverity::Fatal);

            let (resolved, changed) =
                attention_repo::resolve(tx, &inserted.id, AttentionResolution::ConditionCleared, later)?;
            assert!(changed);
            assert_eq!(resolved.revision, 3);
            assert!(resolved.resolved_at.is_some());
            assert!(resolved.read_at.is_some());
            Ok(())
        })
        .unwrap();
}

#[test]
fn record_evidence_writes_once_and_amend_never_touches_it() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            seed_printer(connection, "prn-a", "Voron");
            seed_job(connection, "job-a", "prn-a");

            let condition = Condition {
                kind: ConditionKind::JobCompleted,
                source_id: "job-a".to_string(),
                printer_id: Some("prn-a".to_string()),
                job_id: Some("job-a".to_string()),
                spool_id: None,
                requirement_id: None,
                subject: subject("Voron"),
                detail: AttentionDetail::JobCompleted {
                    ended_at: NOW_TEXT.to_string(),
                },
                acknowledge: false,
            };
            let inserted =
                attention_repo::insert(tx, &condition, None, false, AttentionOrigin::Live, now())?;

            let outcome = EvidenceOutcome::Skipped {
                reason: EvidenceSkipReason::CameraError,
                error_kind: None,
            };
            let (recorded, changed) = attention_repo::record_evidence(tx, &inserted.id, &outcome)?;
            assert!(changed);
            assert_eq!(recorded.evidence, Some(outcome.clone()));
            assert_eq!(recorded.revision, 2);

            // A second call is a no-op.
            let other = EvidenceOutcome::Captured {
                snapshot_id: "snp-a".to_string(),
            };
            let (again, changed_again) = attention_repo::record_evidence(tx, &inserted.id, &other)?;
            assert!(!changed_again);
            assert_eq!(again.evidence, Some(outcome.clone()));
            assert_eq!(again.revision, 2);

            // amend leaves evidence untouched, whether or not it changes
            // detail/severity.
            let amended = attention_repo::amend(
                tx,
                &inserted.id,
                &AttentionDetail::JobCompleted {
                    ended_at: NOW_TEXT.to_string(),
                },
                AttentionSeverity::Info,
                "Cube finished.",
                true,
                now(),
            )?;
            assert_eq!(amended.evidence, Some(outcome));
            assert_eq!(amended.revision, 3);
            Ok(())
        })
        .unwrap();
}

#[test]
fn list_attention_orders_open_by_severity_and_resolved_by_recency_with_a_cursor() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            for i in 0..3 {
                seed_printer(connection, &format!("prn-{i}"), "Voron");
            }
            let t0 = now();
            let t1 = t0 + chrono::Duration::minutes(1);

            let warn_old = attention_repo::insert(
                tx,
                &printer_offline("prn-0", "Voron"),
                None,
                false,
                AttentionOrigin::Live,
                t0,
            )?;
            let warn_new = attention_repo::insert(
                tx,
                &printer_offline("prn-1", "Voron"),
                None,
                false,
                AttentionOrigin::Live,
                t1,
            )?;
            let fatal = Condition {
                kind: ConditionKind::PrinterHostFailed,
                source_id: "prn-2".to_string(),
                printer_id: Some("prn-2".to_string()),
                job_id: None,
                spool_id: None,
                requirement_id: None,
                subject: subject("Voron"),
                detail: AttentionDetail::PrinterHostFailed,
                acknowledge: false,
            };
            let fatal_event =
                attention_repo::insert(tx, &fatal, None, false, AttentionOrigin::Live, t0)?;

            let page = attention_repo::list_attention(connection, None, 200)?;
            let open_ids: Vec<_> = page.open.iter().map(|e| e.id.clone()).collect();
            assert_eq!(
                open_ids,
                [fatal_event.id.clone(), warn_new.id.clone(), warn_old.id.clone()],
                "fatal first, then warning by first_observed_at descending"
            );

            attention_repo::resolve(tx, &warn_old.id, AttentionResolution::ConditionCleared, t0)?;
            attention_repo::resolve(tx, &warn_new.id, AttentionResolution::ConditionCleared, t1)?;

            let (first_page, cursor) = attention_repo::list_resolved(connection, None, 1)?;
            assert_eq!(first_page.len(), 1);
            assert_eq!(first_page[0].id, warn_new.id);
            let cursor = cursor.expect("one more resolved Event remains");

            let (second_page, cursor2) = attention_repo::list_resolved(connection, Some(&cursor), 1)?;
            assert_eq!(second_page.len(), 1);
            assert_eq!(second_page[0].id, warn_old.id);
            assert!(cursor2.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn latest_resolved_by_key_reports_the_newest_resolution() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            seed_printer(connection, "prn-a", "Voron");
            let condition = printer_offline("prn-a", "Voron");
            let t0 = now();
            let t1 = t0 + chrono::Duration::minutes(10);
            let t2 = t1 + chrono::Duration::minutes(10);

            let first = attention_repo::insert(tx, &condition, None, false, AttentionOrigin::Live, t0)?;
            attention_repo::resolve(tx, &first.id, AttentionResolution::ConditionCleared, t1)?;
            let second =
                attention_repo::insert(tx, &condition, Some(&first.id), false, AttentionOrigin::Live, t1)?;
            attention_repo::resolve(tx, &second.id, AttentionResolution::ConditionCleared, t2)?;

            let map = attention_repo::latest_resolved_by_key(connection)?;
            assert_eq!(map.get(&condition.dedup_key()), Some(&second.id));
            Ok(())
        })
        .unwrap();
}

#[test]
fn resolve_for_printer_resolves_only_that_printers_open_events_source_removed() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            seed_printer(connection, "prn-a", "Voron");
            seed_printer(connection, "prn-b", "Trident");

            let a = attention_repo::insert(
                tx,
                &printer_offline("prn-a", "Voron"),
                None,
                false,
                AttentionOrigin::Live,
                now(),
            )?;
            let b = attention_repo::insert(
                tx,
                &printer_offline("prn-b", "Trident"),
                None,
                false,
                AttentionOrigin::Live,
                now(),
            )?;

            let changed = attention_repo::resolve_for_printer(tx, "prn-a", now())?;
            assert_eq!(changed.len(), 1);
            assert_eq!(changed[0].id, a.id);
            assert_eq!(changed[0].resolution, Some(AttentionResolution::SourceRemoved));

            let b_after = attention_repo::load_event(connection, &b.id)?.unwrap();
            assert!(b_after.resolved_at.is_none());
            Ok(())
        })
        .unwrap();
}

#[test]
fn alert_defaults_get_returns_the_decision_defaults_and_set_upserts() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            seed_printer(connection, "prn-a", "Voron");

            let defaults = alerts::get(connection, "prn-a")?;
            assert_eq!(defaults.revision, None);
            assert_eq!(defaults.updated_at, None);
            assert_eq!(
                defaults.alert_defaults,
                AlertDefaults {
                    offline_after_minutes: Some(OfflineAlertMinutes::Five),
                    notifications: NotificationMode::Follow,
                    snapshot_on_incident: true,
                    snapshot_on_completion: true,
                }
            );

            let muted = AlertDefaults {
                offline_after_minutes: None,
                notifications: NotificationMode::Muted,
                snapshot_on_incident: false,
                snapshot_on_completion: false,
            };
            let saved = alerts::set(tx, "prn-a", &muted, NOW_TEXT)?;
            assert_eq!(saved.revision, Some(1));
            assert_eq!(saved.alert_defaults, muted);

            let saved_again = alerts::set(tx, "prn-a", &muted, "2026-09-28T09:05:00Z")?;
            assert_eq!(saved_again.revision, Some(2), "every set upserts and bumps");
            Ok(())
        })
        .unwrap();
}

/// Global constraint 3: every new persisted payload gets a seeded-secret
/// test. The corpus is a URL with userinfo and a query token — the shape
/// a camera snapshot URL or a Connection host could take. Nothing in this
/// task's scope (`AttentionSubject`, `AttentionDetail`, `EvidenceOutcome`)
/// has a host/URL-shaped field, so this proves the guarantee holds even
/// when a Printer's own free-text name happens to look like one (an
/// operator could type anything there) *and* when the same corpus is
/// legitimately stored elsewhere (`printer_cameras.snapshot_url`, which
/// D4 allows to hold a manual URL): the two never mix in
/// `attention_events`' JSON columns.
const SECRET_CORPUS: &str = "http://operator:s3cr3t@192.0.2.10:8080/webcam?token=abc123";

#[test]
fn the_seeded_secret_never_appears_in_attention_events_json_columns() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            seed_printer(connection, "prn-a", "Voron");
            seed_job(connection, "job-a", "prn-a");
            // Legitimately holds the corpus (D4): never read by this task's
            // repository functions, but seeded to prove it doesn't leak
            // sideways into `attention_events`.
            exec(
                connection,
                &format!(
                    "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at)
                     VALUES ('prn-a', 'snapshotUrl', '{SECRET_CORPUS}', '{NOW_TEXT}');"
                ),
            );

            let offline = attention_repo::insert(
                tx,
                &printer_offline("prn-a", "Voron"),
                None,
                false,
                AttentionOrigin::Live,
                now(),
            )?;
            attention_repo::amend(
                tx,
                &offline.id,
                &offline.detail,
                offline.severity,
                &offline.summary,
                false,
                now() + chrono::Duration::minutes(1),
            )?;
            attention_repo::resolve(tx, &offline.id, AttentionResolution::ConditionCleared, now())?;

            let completed = Condition {
                kind: ConditionKind::JobCompleted,
                source_id: "job-a".to_string(),
                printer_id: Some("prn-a".to_string()),
                job_id: Some("job-a".to_string()),
                spool_id: None,
                requirement_id: None,
                subject: subject("Voron"),
                detail: AttentionDetail::JobCompleted {
                    ended_at: NOW_TEXT.to_string(),
                },
                acknowledge: false,
            };
            let completed_event =
                attention_repo::insert(tx, &completed, None, false, AttentionOrigin::Live, now())?;
            attention_repo::record_evidence(
                tx,
                &completed_event.id,
                &EvidenceOutcome::Skipped {
                    reason: EvidenceSkipReason::CameraError,
                    error_kind: None,
                },
            )?;

            let leaked: i64 = connection.query_row(
                "SELECT COUNT(*) FROM attention_events
                 WHERE subject_snapshot_json LIKE '%' || ?1 || '%'
                    OR detail_json LIKE '%' || ?1 || '%'
                    OR summary LIKE '%' || ?1 || '%'
                    OR (evidence_json IS NOT NULL AND evidence_json LIKE '%' || ?1 || '%')",
                [SECRET_CORPUS],
                |row| row.get(0),
            )?;
            assert_eq!(leaked, 0, "the secret corpus must never reach attention_events");

            // Sanity: the corpus really is in the database somewhere (its
            // one legitimate home), so this test isn't vacuous.
            let camera_rows: i64 = connection.query_row(
                "SELECT COUNT(*) FROM printer_cameras WHERE snapshot_url = ?1",
                [SECRET_CORPUS],
                |row| row.get(0),
            )?;
            assert_eq!(camera_rows, 1);
            Ok(())
        })
        .unwrap();
}
