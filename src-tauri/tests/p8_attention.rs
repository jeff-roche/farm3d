//! Task 5 (P8 D1/D2/D9): the `attention::repository` primitives, exercised
//! through the public crate API against a real migrated `Storage`, and the
//! per-Printer alert-default repository (`printers::alerts`). See the P8
//! design spec's D1 ("Vocabulary and ownership"), D2 ("Apply"/"Planner
//! rules"), and "Commands" (`list_attention`'s ordering/cursor).
//!
//! Task 6 adds the projector over `FakeMoonraker` through the P7 dispatch
//! rig (D2 "The pass", "Runtime and wakes"): the offline grace, quiet
//! amendments, resolution and recurrence, lagged receivers, concurrent
//! wakes, the error-cause path, and the Job-failure Incident; then the
//! Attention and Incident commands (D7), their operation-id replay and
//! reuse, and the seeded-secret scan of every new row and event.

mod common;
mod p7_dispatch_rig;

use std::sync::Arc;
use std::time::Duration;

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
fn latest_resolved_for_keys_reports_the_newest_resolution() {
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

            let map = attention_repo::latest_resolved_for_keys(connection, [condition.dedup_key().as_str()])?;
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

/// Controller carry 5: two resolved Events with the exact same
/// `resolved_at` page by id, with no skip and no repeat.
#[test]
fn the_resolved_cursor_is_stable_across_an_identical_resolved_at() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let connection = tx;
            let mut ids = Vec::new();
            for i in 0..4 {
                let printer = format!("prn-{i}");
                seed_printer(connection, &printer, "Voron");
                let event = attention_repo::insert(
                    tx,
                    &printer_offline(&printer, "Voron"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                // Every resolution at the same instant.
                attention_repo::resolve(tx, &event.id, AttentionResolution::ConditionCleared, now())?;
                ids.push(event.id);
            }
            ids.sort();
            ids.reverse();

            let mut seen = Vec::new();
            let mut cursor = None;
            loop {
                let (page, next) = attention_repo::list_resolved(connection, cursor.as_ref(), 1)?;
                seen.extend(page.into_iter().map(|event| event.id));
                match next {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            assert_eq!(seen, ids, "id descending within one resolved_at, each exactly once");
            Ok(())
        })
        .unwrap();
}

/// Final review M8: a tie group larger than the page. Events resolved
/// before, at, and after one shared instant page by `resolvedAt` then id,
/// descending, each exactly once, when the page (7) is smaller than the
/// tie group (25) and doesn't divide it.
#[test]
fn keyset_pages_cross_a_tie_group_larger_than_the_page() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let tie = now();
            let mut expected: Vec<(DateTime<Utc>, String)> = Vec::new();
            for i in 0..31 {
                let printer = format!("prn-{i:02}");
                seed_printer(tx, &printer, "Voron");
                let event = attention_repo::insert(
                    tx,
                    &printer_offline(&printer, "Voron"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    tie,
                )?;
                // Three before the tie, 25 at it, three after.
                let resolved_at = match i {
                    0..=2 => tie - chrono::Duration::minutes(3 - i),
                    28..=30 => tie + chrono::Duration::minutes(i - 27),
                    _ => tie,
                };
                attention_repo::resolve(tx, &event.id, AttentionResolution::ConditionCleared, resolved_at)?;
                expected.push((resolved_at, event.id));
            }
            expected.sort();
            expected.reverse();
            let expected: Vec<String> = expected.into_iter().map(|(_, id)| id).collect();

            let mut seen = Vec::new();
            let mut cursor = None;
            let mut pages = 0;
            loop {
                let (page, next) = attention_repo::list_resolved(tx, cursor.as_ref(), 7)?;
                assert!(page.len() <= 7);
                pages += 1;
                seen.extend(page.into_iter().map(|event| event.id));
                match next {
                    Some(next) => cursor = Some(next),
                    None => break,
                }
            }
            assert_eq!(pages, 5, "31 rows at 7 a page");
            assert_eq!(seen, expected, "every Event exactly once, in order");
            Ok(())
        })
        .unwrap();
}

// --- Task 6: the projector over FakeMoonraker ----------------------------------

use farm3d_lib::attention::services::{run_pass, AttentionTimings};
use farm3d_lib::attention::AttentionEvent;
use farm3d_lib::connections::supervisor::PrinterSetupFacts;
use farm3d_lib::host_ops::Clock;
use farm3d_lib::jobs::JobTimings;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;
use p7_dispatch_rig::{
    boot_with_attention, status_of, AttentionBoot, ManualClock, Roots, Running, PRINTER, SECRET,
};
use serde_json::{json, Value};

/// Past the default five-minute offline grace.
const PAST_GRACE: Duration = Duration::from_secs(5 * 60 + 1);

struct ProjectorRig {
    roots: Roots,
    app: Running,
    clock: Arc<ManualClock>,
}

impl ProjectorRig {
    fn new(start_safety: StartSafety) -> Self {
        let roots = Roots::new(start_safety);
        let clock = ManualClock::new();
        let (app, _) = boot_with_attention(
            &roots,
            status_of(OperationalState::Ready),
            JobTimings {
                history_poll: Duration::from_millis(200),
                ..JobTimings::default()
            },
            AttentionBoot {
                timings: AttentionTimings {
                    pass_min_interval: Duration::ZERO,
                    safety_tick: Duration::from_secs(3600),
                },
                clock: Some(clock.clone() as Arc<dyn Clock>),
                cameras: None,
                notifications: None,
            },
        );
        app.attention_pass();
        Self { roots, app, clock }
    }

    fn of(&self, condition: ConditionKind) -> Vec<AttentionEvent> {
        self.app
            .attention_rows()
            .into_iter()
            .filter(|event| event.condition == condition)
            .collect()
    }

    /// The Printer goes offline and stays so past its grace.
    fn go_offline_past_grace(&self) {
        self.app.seed(status_of(OperationalState::Offline));
        self.app.attention_pass();
        self.clock.advance(PAST_GRACE);
        self.app.services.attention.poke();
        self.app.attention_pass();
    }
}

fn open_of(events: &[AttentionEvent]) -> Vec<&AttentionEvent> {
    events.iter().filter(|event| event.resolved_at.is_none()).collect()
}

#[test]
fn offline_opens_once_amends_quietly_resolves_and_recurs() {
    let rig = ProjectorRig::new(StartSafety::Unattended);
    let app = &rig.app;

    app.seed(status_of(OperationalState::Offline));
    app.attention_pass();
    assert!(rig.of(ConditionKind::PrinterOffline).is_empty(), "inside the grace");

    rig.clock.advance(PAST_GRACE);
    app.services.attention.poke();
    app.attention_pass();
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "one Event past the grace");
    let first = offline[0].clone();
    assert_eq!(first.origin, AttentionOrigin::Live);
    assert_eq!(first.printer_id.as_deref(), Some(PRINTER));

    for _ in 0..20 {
        app.seed(status_of(OperationalState::Offline));
    }
    app.attention_pass();
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "still one Event");
    assert_eq!(offline[0], first, "inside the minute an unchanged observation writes nothing");
    // Decision 40: a minute on, the observation is persisted once.
    rig.clock.advance(Duration::from_secs(60));
    app.services.attention.poke();
    app.attention_pass();
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "still one Event");
    assert_eq!(offline[0].observation_count, first.observation_count + 1);
    assert_ne!(offline[0].last_observed_at, first.last_observed_at);
    assert_eq!(offline[0].revision, first.revision, "an unchanged amendment bumps nothing");
    assert_eq!(
        app.attention_stream("attention.event.changed", &first.id).len(),
        1,
        "only the insert was published"
    );

    app.seed(status_of(OperationalState::Ready));
    app.attention_pass();
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline[0].resolution, Some(AttentionResolution::ConditionCleared));
    assert!(offline[0].read_at.is_some());
    assert_eq!(app.attention_stream("attention.event.changed", &first.id).len(), 2);

    rig.go_offline_past_grace();
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 2, "a recurrence is a new Event");
    let second = offline.iter().find(|event| event.id != first.id).unwrap();
    assert_eq!(second.recurrence_of.as_deref(), Some(first.id.as_str()));
    assert_eq!(second.resolved_at, None);
}

#[test]
fn a_lagged_receiver_forces_a_full_pass_without_duplicates() {
    let rig = ProjectorRig::new(StartSafety::Unattended);
    let app = &rig.app;
    rig.go_offline_past_grace();
    assert_eq!(rig.of(ConditionKind::PrinterOffline).len(), 1);

    let lagged = app.services.attention.lagged();
    app.services.attention.hold();
    // Far past the status broadcast's capacity while the projector is held.
    for _ in 0..600 {
        app.seed(status_of(OperationalState::Offline));
    }
    app.services.attention.release();
    app.attention_pass();
    assert!(app.services.attention.lagged() > lagged, "the projector saw Lagged");
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "no duplicate: {offline:?}");
    assert_eq!(open_of(&offline).len(), 1);
}

#[test]
fn a_hundred_concurrent_wakes_open_exactly_one_event() {
    let rig = ProjectorRig::new(StartSafety::Unattended);
    let app = &rig.app;
    app.seed(status_of(OperationalState::Offline));
    app.attention_pass();
    app.services.attention.hold();
    rig.clock.advance(PAST_GRACE);

    std::thread::scope(|scope| {
        for i in 0..100 {
            let services = &app.services;
            scope.spawn(move || {
                if i % 2 == 0 {
                    services.attention.poke();
                } else {
                    run_pass(services).expect("a pass");
                }
            });
        }
    });
    app.services.attention.release();
    app.attention_pass();
    let offline = rig.of(ConditionKind::PrinterOffline);
    assert_eq!(offline.len(), 1, "{offline:?}");
    assert_eq!(open_of(&offline).len(), 1);
    assert_eq!(
        app.attention_stream("attention.event.changed", &offline[0].id).len(),
        1,
        "inserted and published once"
    );
}

/// D2 "Reachability": `report_error` records `protocol`, so the status is
/// `Misconfigured` and `printer.connectionError` opens at once (no grace).
#[test]
fn a_protocol_error_opens_a_connection_error_and_a_live_status_clears_it() {
    let rig = ProjectorRig::new(StartSafety::Unattended);
    let app = &rig.app;
    tauri::async_runtime::block_on(app.manager.report_error(
        PRINTER,
        "The Printer answered in a way farm3d couldn't read.",
        PrinterSetupFacts::complete(),
    ));
    app.attention_pass();
    let errors = rig.of(ConditionKind::PrinterConnectionError);
    assert_eq!(errors.len(), 1);
    assert_eq!(
        serde_json::to_value(&errors[0].detail).unwrap(),
        json!({"kind": "printerConnectionError", "cause": "protocol"})
    );
    assert!(rig.of(ConditionKind::PrinterOffline).is_empty());

    app.seed(status_of(OperationalState::Ready));
    app.attention_pass();
    let errors = rig.of(ConditionKind::PrinterConnectionError);
    assert_eq!(errors[0].resolution, Some(AttentionResolution::ConditionCleared));
}

// --- Task 6: the Job-failure Incident and the commands ---------------------------

fn call(app: &Running, command: &str, body: Value) -> Result<Value, Value> {
    app.call(command, body)
}

fn ok(app: &Running, command: &str, body: Value) -> Value {
    app.ok(command, body)
}

/// Task 7 (global constraint 3): a manual camera URL carrying a userinfo
/// password, an RFC 5737 host, and a query token, stored on the Printer so
/// every projector pass, command, and event below runs with it in place.
const CAMERA_SEED_PASS: &str = "SEEDED-P8-ATTENTION-CAMERA-PASS-91ab";
const CAMERA_SEED_TOKEN: &str = "SEEDED-P8-ATTENTION-CAMERA-TOKEN-4c7e";
const CAMERA_SEED_HOST: &str = "192.0.2.61";

/// A Job fails on the fake: the projector opens `job.failed` and the
/// material requirement's Event, and one Incident for the Job links both.
/// The Printer has the seeded camera URL (written as-is: the row, not its
/// validation, is what's under test).
fn failed_job_rig() -> (ProjectorRig, String) {
    let rig = ProjectorRig::new(StartSafety::ConfirmBedClear);
    rig.app
        .storage
        .write(|tx| {
            tx.execute(
                "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at) \
                 VALUES (?1, 'snapshotUrl', ?2, '2026-09-28T09:00:00.000Z')",
                rusqlite::params![
                    PRINTER,
                    format!("http://{CAMERA_SEED_HOST}:8080/snap?user={CAMERA_SEED_PASS}&token={CAMERA_SEED_TOKEN}")
                ],
            )?;
            Ok(())
        })
        .unwrap();
    let job = rig.app.printing();
    rig.roots.fake.finish_print("klippy_shutdown");
    rig.app.wait_job(&job, "failed");
    rig.app.wait_until("the failure's Events are projected", || {
        rig.app.attention_rows().iter().filter(|event| {
            event.job_id.as_deref() == Some(job.as_str()) && event.incident_id.is_some()
        }).count()
            == 2
    });
    (rig, job)
}

fn event_of(rig: &ProjectorRig, condition: ConditionKind, job: &str) -> AttentionEvent {
    rig.of(condition)
        .into_iter()
        .find(|event| event.job_id.as_deref() == Some(job))
        .unwrap_or_else(|| panic!("no {condition:?} Event for {job}"))
}

#[test]
fn a_failed_job_opens_one_incident_that_the_commands_settle_and_close() {
    let (rig, job) = failed_job_rig();
    let app = &rig.app;
    let failed = event_of(&rig, ConditionKind::JobFailed, &job);
    let material = event_of(&rig, ConditionKind::RequirementMaterialReconciliation, &job);
    let incident_id = failed.incident_id.clone().unwrap();
    assert_eq!(material.incident_id.as_deref(), Some(incident_id.as_str()));

    // list_attention: the backfill shape, listen-before-backfill fields.
    let listed = ok(app, "list_attention", json!({}));
    assert!(!listed["streamId"].as_str().unwrap().is_empty());
    assert!(listed["snapshotSequence"].as_i64().unwrap() >= 1);
    let open_ids: Vec<&str> = listed["open"].as_array().unwrap().iter().map(|e| e["id"].as_str().unwrap()).collect();
    assert!(open_ids.contains(&failed.id.as_str()) && open_ids.contains(&material.id.as_str()));
    assert_eq!(listed["openIncidents"][0]["id"], json!(incident_id));
    assert_eq!(listed["openIncidents"][0]["kind"], "job.failed");
    // The seeded camera's health, with no URL (scanned below).
    assert_eq!(
        listed["cameraHealth"],
        json!([{"printerId": PRINTER, "state": "unknown", "sourceKind": "snapshotUrl",
                "lastSuccessAt": null, "lastFailureAt": null, "lastFailureKind": null}])
    );
    let listed_text = listed.to_string();
    let older = ok(app, "list_attention", json!({"resolvedBefore": format!("2999-01-01T00:00:00Z|att-z"), "limit": 5}));
    assert_eq!(older["open"], json!([]));
    assert_eq!(older["openIncidents"], json!([]));

    // mark_attention_read: an unknown id fails the whole batch.
    let missing = call(
        app,
        "mark_attention_read",
        json!({"operationId": "op-read-missing", "eventIds": [failed.id, "att-missing"]}),
    )
    .unwrap_err();
    assert_eq!(missing["code"], "NOT_FOUND");
    assert_eq!(event_of(&rig, ConditionKind::JobFailed, &job).read_at, None, "nothing was written");
    let read = ok(
        app,
        "mark_attention_read",
        json!({"operationId": "op-read", "eventIds": [failed.id, material.id]}),
    );
    assert_eq!(read["events"].as_array().unwrap().len(), 2);
    assert!(read["events"].as_array().unwrap().iter().all(|e| !e["readAt"].is_null()));
    let published = app.attention_stream("attention.event.changed", &failed.id).len();
    let replay = ok(
        app,
        "mark_attention_read",
        json!({"operationId": "op-read", "eventIds": [failed.id, material.id]}),
    );
    assert_eq!(replay["events"].as_array().unwrap().len(), 2);
    assert_eq!(app.attention_stream("attention.event.changed", &failed.id).len(), published, "a replay publishes nothing");
    let reused = call(
        app,
        "mark_attention_read",
        json!({"operationId": "op-read", "eventIds": [failed.id]}),
    )
    .unwrap_err();
    assert_eq!(reused["code"], "VALIDATION");
    assert_eq!(reused["details"]["fieldPath"], "operationId");

    // acknowledge: a linked Event adds a timeline row, and never resolves.
    let acknowledged = ok(
        app,
        "acknowledge_attention_event",
        json!({"operationId": "op-ack", "eventId": failed.id}),
    );
    assert!(!acknowledged["events"][0]["acknowledgedAt"].is_null());
    assert!(acknowledged["events"][0]["resolvedAt"].is_null(), "acknowledge never resolves");
    assert_eq!(acknowledged["incidents"][0]["id"], json!(incident_id));
    assert_eq!(acknowledged["incidents"][0]["state"], "open");

    // resolve: an `action` Event is not the operator's to resolve.
    let not_manual = call(
        app,
        "resolve_attention_event",
        json!({"operationId": "op-resolve-material", "eventId": material.id}),
    )
    .unwrap_err();
    assert_eq!(not_manual["code"], "ATTENTION_NOT_MANUAL");
    assert_eq!(not_manual["recovery"], json!(["RELOAD"]));
    assert_eq!(
        not_manual["details"],
        json!({
            "eventId": material.id,
            "condition": "requirement.materialReconciliation",
            "resolutionMode": "action",
        })
    );

    // Settling the material resolves its Event; the Incident stays open.
    app.settle("op-settle", &job, json!({"kind": "estimated"})).expect("settle");
    app.wait_until("the material Event resolves", || {
        event_of(&rig, ConditionKind::RequirementMaterialReconciliation, &job)
            .resolved_at
            .is_some()
    });
    let detail = ok(app, "get_incident", json!({"incidentId": incident_id}));
    assert_eq!(detail["incident"]["state"], "open");

    // Resolving the last linked Event closes the Incident.
    let resolved = ok(
        app,
        "resolve_attention_event",
        json!({"operationId": "op-resolve", "eventId": failed.id}),
    );
    assert_eq!(resolved["events"][0]["resolution"], "operatorResolved");
    assert_eq!(resolved["incidents"][0]["state"], "closed");
    assert_eq!(resolved["incidents"][0]["openLinkedEventCount"], 0);
    let incident_published = app.attention_stream("attention.incident.changed", &incident_id).len();
    let again = ok(
        app,
        "resolve_attention_event",
        json!({"operationId": "op-resolve", "eventId": failed.id}),
    );
    assert_eq!(again["events"][0]["id"], json!(failed.id));
    assert_eq!(
        app.attention_stream("attention.incident.changed", &incident_id).len(),
        incident_published,
        "a replay publishes nothing"
    );

    // list_incidents / get_incident / add_incident_note.
    let closed = ok(app, "list_incidents", json!({"state": "closed"}));
    assert_eq!(closed["incidents"][0]["id"], json!(incident_id));
    assert_eq!(ok(app, "list_incidents", json!({"state": "open"}))["incidents"], json!([]));
    assert_eq!(
        ok(app, "list_incidents", json!({"printerId": PRINTER}))["incidents"][0]["id"],
        json!(incident_id)
    );
    let blank = call(
        app,
        "add_incident_note",
        json!({"operationId": "op-note-blank", "incidentId": incident_id, "text": "   "}),
    )
    .unwrap_err();
    assert_eq!(blank["code"], "VALIDATION");
    assert_eq!(blank["details"]["fieldPath"], "text");
    let long = call(
        app,
        "add_incident_note",
        json!({"operationId": "op-note-long", "incidentId": incident_id, "text": "x".repeat(2001)}),
    )
    .unwrap_err();
    assert_eq!(long["details"]["fieldPath"], "text");
    let noted = ok(
        app,
        "add_incident_note",
        json!({"operationId": "op-note", "incidentId": incident_id, "text": "Nozzle clogged."}),
    );
    let kinds = |detail: &Value| -> Vec<String> {
        detail["timeline"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|item| item["source"] == "incident")
            .map(|item| item["entry"]["kind"].as_str().unwrap().to_string())
            .collect()
    };
    assert_eq!(
        kinds(&noted),
        [
            "opened",
            "eventLinked",
            "eventAcknowledged",
            "eventResolved",
            "eventResolved",
            "closed",
            "noteAdded"
        ]
    );
    let note = noted["timeline"].as_array().unwrap().iter().find(|item| item["entry"]["kind"] == "noteAdded").unwrap();
    assert_eq!(note["entry"]["detail"]["text"], "Nozzle clogged.");
    assert_eq!(note["entry"]["operationId"], "op-note");
    let acknowledgement = noted["timeline"].as_array().unwrap().iter().find(|item| item["entry"]["kind"] == "eventAcknowledged").unwrap();
    assert_eq!(acknowledgement["entry"]["detail"]["by"], "operator");
    assert!(
        noted["timeline"].as_array().unwrap().iter().any(|item| item["source"] == "job"),
        "the Job's own timeline is merged in"
    );
    let ats: Vec<DateTime<Utc>> = noted["timeline"]
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            let at = if item["source"] == "incident" { &item["entry"]["at"] } else { &item["event"]["at"] };
            at.as_str().unwrap().parse().unwrap()
        })
        .collect();
    assert!(ats.windows(2).all(|pair| pair[0] <= pair[1]), "ordered by at: {ats:?}");
    assert_eq!(noted["events"].as_array().unwrap().len(), 2);
    let replay = ok(
        app,
        "add_incident_note",
        json!({"operationId": "op-note", "incidentId": incident_id, "text": "Nozzle clogged."}),
    );
    assert_eq!(kinds(&replay).iter().filter(|kind| *kind == "noteAdded").count(), 1);
    let reused = call(
        app,
        "add_incident_note",
        json!({"operationId": "op-note", "incidentId": incident_id, "text": "Something else."}),
    )
    .unwrap_err();
    assert_eq!(reused["details"]["fieldPath"], "operationId");

    assert_eq!(call(app, "get_incident", json!({"incidentId": "inc-missing"})).unwrap_err()["code"], "NOT_FOUND");
    assert_eq!(
        call(app, "acknowledge_attention_event", json!({"operationId": "op-ack-missing", "eventId": "att-missing"}))
            .unwrap_err()["code"],
        "NOT_FOUND"
    );

    // Global Constraint 3: no credential or endpoint in any new row or event.
    let host = rig.roots.fake.config().host;
    let port = rig.roots.fake.config().port.to_string();
    let rows: String = app
        .storage
        .read(|connection| {
            let mut text = String::new();
            for sql in [
                "SELECT subject_snapshot_json || detail_json || summary || COALESCE(evidence_json, '') FROM attention_events",
                "SELECT printer_snapshot_json FROM incidents",
                "SELECT detail_json FROM incident_events",
            ] {
                let mut statement = connection.prepare(sql)?;
                for row in statement.query_map([], |row| row.get::<_, String>(0))? {
                    text.push_str(&row?);
                }
            }
            Ok(text)
        })
        .unwrap();
    let emitted: String = app
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|text| text.contains("\"attention."))
        .cloned()
        .collect();
    for haystack in [&rows, &emitted, &listed_text] {
        assert!(!haystack.is_empty());
        assert!(!haystack.contains(SECRET), "the API key leaked");
        assert!(!haystack.contains(&format!("{host}:{port}")), "the endpoint leaked");
        assert!(!haystack.contains("credentialRef") && !haystack.contains("apikey"));
        // Task 7: nothing of the Printer's camera URL crosses into
        // Attention or Incident rows, events, or `list_attention`.
        for needle in [CAMERA_SEED_PASS, CAMERA_SEED_TOKEN, CAMERA_SEED_HOST, "snap?user="] {
            assert!(!haystack.contains(needle), "{needle} leaked");
        }
    }
}

#[test]
fn command_arguments_are_validated() {
    let rig = ProjectorRig::new(StartSafety::Unattended);
    let app = &rig.app;
    for (command, body, field) in [
        ("list_attention", json!({"limit": 0}), "limit"),
        ("list_attention", json!({"limit": 201}), "limit"),
        ("list_attention", json!({"resolvedBefore": "not-a-cursor"}), "resolvedBefore"),
        ("mark_attention_read", json!({"operationId": "op-a", "eventIds": []}), "eventIds"),
        ("mark_attention_read", json!({"operationId": "op-b", "eventIds": ["att-1", "att-1"]}), "eventIds"),
        ("list_incidents", json!({"limit": 0}), "limit"),
        ("list_incidents", json!({"before": "not-a-cursor"}), "before"),
    ] {
        let error = call(app, command, body.clone()).unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{command} {body}");
        assert_eq!(error["details"]["fieldPath"], field, "{command} {body}");
    }
    let many: Vec<String> = (0..201).map(|i| format!("att-{i}")).collect();
    let error = call(app, "mark_attention_read", json!({"operationId": "op-c", "eventIds": many})).unwrap_err();
    assert_eq!(error["details"]["fieldPath"], "eventIds");
    assert_eq!(ok(app, "list_incidents", json!({}))["incidents"], json!([]));
}
