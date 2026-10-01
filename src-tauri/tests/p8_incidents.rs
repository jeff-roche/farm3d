//! Task 5 (P8 D3): the `incidents::repository` primitives, exercised
//! through the public crate API against a real migrated `Storage`, working
//! together with `attention::repository`. See the P8 design spec's D3
//! ("Incidents") in full: the timeline kinds, the revision/publishing
//! rule (every `incident_events` append and every `incidents` row change
//! bumps `revision` in the same transaction), and "one Incident per Job".
//!
//! The projector, commands, and event stream that will call these
//! functions are a later task (6); this file only proves the repository
//! primitives themselves.

use chrono::{DateTime, Utc};

use farm3d_lib::attention::AttentionSubject;
use farm3d_lib::attention::{
    repository as attention_repo, AttentionDetail, AttentionOrigin, AttentionResolution, Condition,
    ConditionKind,
};
use farm3d_lib::catalog::{BedShape, PrinterProfile};
use farm3d_lib::incidents::{
    repository as incidents_repo, IncidentEntryDetail, IncidentKind, IncidentState,
};
use farm3d_lib::jobs::PrinterSnapshot;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};

const NOW_TEXT: &str = "2026-09-28T10:00:00Z";

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

fn seed_printer(connection: &rusqlite::Connection, id: &str) {
    exec(
        connection,
        &format!(
            "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES ('{id}', 1, 'Printer', '', '', '', '', '', '', '{{}}', '{NOW_TEXT}', '{NOW_TEXT}');"
        ),
    );
}

/// A Printer -> Slice Revision -> Spool -> Reservation -> Queue Entry ->
/// Job chain, minimal enough for `jobs`/`attention_events`' FKs. The
/// Printer (`printer_id`) must already exist.
fn seed_job(connection: &rusqlite::Connection, job_id: &str, printer_id: &str) {
    let hash = "d".repeat(64);
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

fn printer_snapshot() -> PrinterSnapshot {
    PrinterSnapshot {
        name: "Voron".to_string(),
        location: Some("Bay A".to_string()),
        catalog_ref: None,
        adapter_kind: None,
        profile: PrinterProfile {
            bed_shape: BedShape::Rectangular {
                width_mm: 250.0,
                depth_mm: 250.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 250.0,
            bed_exclude_areas: Vec::new(),
            default_bed_type: "4".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "brass".to_string(),
            gcode_flavor: "marlin".to_string(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: None,
        },
    }
}

fn subject() -> AttentionSubject {
    AttentionSubject {
        printer_name: Some("Voron".to_string()),
        printer_location: Some("Bay A".to_string()),
        job_label: Some("Cube".to_string()),
        spool_number: None,
        spool_label: None,
    }
}

fn job_failed(job_id: &str, printer_id: &str) -> Condition {
    Condition {
        kind: ConditionKind::JobFailed,
        source_id: job_id.to_string(),
        printer_id: Some(printer_id.to_string()),
        job_id: Some(job_id.to_string()),
        spool_id: None,
        requirement_id: None,
        subject: subject(),
        detail: AttentionDetail::JobFailed {
            ended_at: NOW_TEXT.to_string(),
        },
        acknowledge: false,
    }
}

fn job_host_cancelled(job_id: &str, printer_id: &str) -> Condition {
    Condition {
        kind: ConditionKind::JobHostCancelled,
        source_id: job_id.to_string(),
        printer_id: Some(printer_id.to_string()),
        job_id: Some(job_id.to_string()),
        spool_id: None,
        requirement_id: None,
        subject: subject(),
        detail: AttentionDetail::JobHostCancelled {
            ended_at: NOW_TEXT.to_string(),
        },
        acknowledge: false,
    }
}

fn host_failed(printer_id: &str) -> Condition {
    Condition {
        kind: ConditionKind::PrinterHostFailed,
        source_id: printer_id.to_string(),
        printer_id: Some(printer_id.to_string()),
        job_id: None,
        spool_id: None,
        requirement_id: None,
        subject: subject(),
        detail: AttentionDetail::PrinterHostFailed,
        acknowledge: false,
    }
}

/// A full lifecycle through the public API: `open` writes `opened` and
/// links the seeding Event; a second `open` for the same Job is
/// idempotent; `close_if_settled` only closes once every linked actionable
/// Event is resolved; `link_event` on a closed Incident writes `reopened`
/// before `eventLinked`, and every append bumps `revision` by exactly one
/// in the same transaction.
#[test]
fn a_jobs_incident_opens_links_closes_and_reopens_with_the_right_revision_math() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            seed_printer(tx, "prn-a");
            seed_job(tx, "job-a", "prn-a");

            let t0 = now();
            let t1 = t0 + chrono::Duration::minutes(1);

            let failed = attention_repo::insert(
                tx,
                &job_failed("job-a", "prn-a"),
                None,
                false,
                AttentionOrigin::Live,
                t0,
            )?;
            let opened = incidents_repo::open(
                tx,
                IncidentKind::JobFailed,
                "prn-a",
                Some("job-a"),
                &printer_snapshot(),
                &failed.id,
                t0,
            )?;
            assert_eq!(opened.revision, 1, "opening costs no extra bump");
            assert_eq!(opened.state, IncidentState::Open);
            assert_eq!(opened.linked_event_ids, vec![failed.id.clone()]);

            // Idempotent: a second open for the same Job changes nothing.
            let reopened_attempt = incidents_repo::open(
                tx,
                IncidentKind::JobFailed,
                "prn-a",
                Some("job-a"),
                &printer_snapshot(),
                "att-unused",
                t0,
            )?;
            assert_eq!(reopened_attempt.id, opened.id);
            assert_eq!(reopened_attempt.revision, 1);

            // Resolve the only linked actionable Event: closes.
            attention_repo::resolve(tx, &failed.id, AttentionResolution::OperatorResolved, t0)?;
            let closed = incidents_repo::close_if_settled(tx, &opened.id, t0)?;
            assert_eq!(closed.state, IncidentState::Closed);
            assert!(closed.closed_at.is_some());
            assert_eq!(closed.revision, 2, "closing bumps once");

            // A new Event for the same Job links to the (closed) Incident:
            // reopened, then eventLinked -- two appends, two bumps.
            let cancelled = attention_repo::insert(
                tx,
                &job_host_cancelled("job-a", "prn-a"),
                None,
                false,
                AttentionOrigin::Live,
                t1,
            )?;
            let relinked = incidents_repo::link_event(tx, &opened.id, &cancelled.id, t1)?;
            assert_eq!(relinked.state, IncidentState::Open);
            assert_eq!(relinked.closed_at, None);
            assert_eq!(relinked.revision, 4);
            assert_eq!(
                relinked.linked_event_ids,
                vec![failed.id.clone(), cancelled.id.clone()]
            );

            let cancelled_after = attention_repo::load_event(tx, &cancelled.id)?.unwrap();
            assert_eq!(
                cancelled_after.incident_id.as_deref(),
                Some(opened.id.as_str())
            );

            // Still open: the new linked Event hasn't resolved yet.
            let still_open = incidents_repo::close_if_settled(tx, &opened.id, t1)?;
            assert_eq!(still_open.state, IncidentState::Open);
            assert_eq!(
                still_open.revision, 4,
                "an unsettled close check is a no-op"
            );

            // Resolve it too: closes again.
            attention_repo::resolve(tx, &cancelled.id, AttentionResolution::OperatorResolved, t1)?;
            let closed_again = incidents_repo::close_if_settled(tx, &opened.id, t1)?;
            assert_eq!(closed_again.state, IncidentState::Closed);
            assert_eq!(closed_again.revision, 5);
            Ok(())
        })
        .unwrap();
}

/// D3 "Kind": `printer.hostFailed` opens a fresh Incident with `job_id:
/// None` every time (never reused across recurrences).
#[test]
fn a_printer_host_failed_incident_has_no_job_and_never_reuses_an_earlier_one() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            seed_printer(tx, "prn-a");
            let t0 = now();
            let t1 = t0 + chrono::Duration::minutes(5);

            let e1 = attention_repo::insert(
                tx,
                &host_failed("prn-a"),
                None,
                false,
                AttentionOrigin::Live,
                t0,
            )?;
            let first = incidents_repo::open(
                tx,
                IncidentKind::PrinterHostFailed,
                "prn-a",
                None,
                &printer_snapshot(),
                &e1.id,
                t0,
            )?;
            assert_eq!(first.job_id, None);
            attention_repo::resolve(tx, &e1.id, AttentionResolution::ConditionCleared, t0)?;
            incidents_repo::close_if_settled(tx, &first.id, t0)?;

            let e2 = attention_repo::insert(
                tx,
                &host_failed("prn-a"),
                Some(&e1.id),
                false,
                AttentionOrigin::Live,
                t1,
            )?;
            let second = incidents_repo::open(
                tx,
                IncidentKind::PrinterHostFailed,
                "prn-a",
                None,
                &printer_snapshot(),
                &e2.id,
                t1,
            )?;
            assert_ne!(second.id, first.id, "a recurrence opens a new Incident");
            assert_eq!(second.job_id, None);
            Ok(())
        })
        .unwrap();
}

/// `append_entry`'s sequence is strictly monotonic per Incident (1 is
/// always the `opened` entry `open` itself writes).
#[test]
fn append_entry_sequence_is_monotonic() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            seed_printer(tx, "prn-a");
            let e1 = attention_repo::insert(
                tx,
                &host_failed("prn-a"),
                None,
                false,
                AttentionOrigin::Live,
                now(),
            )?;
            let incident = incidents_repo::open(
                tx,
                IncidentKind::PrinterHostFailed,
                "prn-a",
                None,
                &printer_snapshot(),
                &e1.id,
                now(),
            )?;
            let n1 = incidents_repo::append_entry(
                tx,
                &incident.id,
                &IncidentEntryDetail::NoteAdded {
                    text: "checked the hotend".to_string(),
                },
                None,
                now(),
            )?;
            let n2 = incidents_repo::append_entry(
                tx,
                &incident.id,
                &IncidentEntryDetail::NoteAdded {
                    text: "replaced the nozzle".to_string(),
                },
                None,
                now(),
            )?;
            assert_eq!(n1.sequence, 2);
            assert_eq!(n2.sequence, 3);
            Ok(())
        })
        .unwrap();
}

/// Global constraint 3: every new persisted payload gets a seeded-secret
/// test. The corpus is a URL with userinfo and a query token. Neither
/// `incidents.printer_snapshot_json` nor `incident_events.detail_json` has
/// a host/URL-shaped field; this proves that holds even when the corpus is
/// legitimately present elsewhere (`printer_cameras.snapshot_url`) and
/// when free text an operator could type (a note) is exercised alongside
/// it -- the corpus itself is never typed into the note here, matching D3
/// "farm3d never writes a URL, host, or credential into a timeline row
/// itself".
const SECRET_CORPUS: &str = "http://operator:s3cr3t@192.0.2.10:8080/webcam?token=abc123";

#[test]
fn the_seeded_secret_never_appears_in_incident_json_columns() {
    let (_temp, storage) = open_storage();
    storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            seed_printer(tx, "prn-a");
            seed_job(tx, "job-a", "prn-a");
            exec(
                tx,
                &format!(
                    "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at)
                     VALUES ('prn-a', 'snapshotUrl', '{SECRET_CORPUS}', '{NOW_TEXT}');"
                ),
            );

            let failed = attention_repo::insert(
                tx,
                &job_failed("job-a", "prn-a"),
                None,
                false,
                AttentionOrigin::Live,
                now(),
            )?;
            let incident = incidents_repo::open(
                tx,
                IncidentKind::JobFailed,
                "prn-a",
                Some("job-a"),
                &printer_snapshot(),
                &failed.id,
                now(),
            )?;
            incidents_repo::append_entry(
                tx,
                &incident.id,
                &IncidentEntryDetail::NoteAdded {
                    text: "camera looked fine on the bench".to_string(),
                },
                None,
                now(),
            )?;
            incidents_repo::append_entry(
                tx,
                &incident.id,
                &IncidentEntryDetail::EvidenceSkipped {
                    reason: farm3d_lib::cameras::EvidenceSkipReason::CameraError,
                    error_kind: Some(farm3d_lib::cameras::CameraErrorKind::Timeout),
                },
                None,
                now(),
            )?;
            attention_repo::resolve(tx, &failed.id, AttentionResolution::OperatorResolved, now())?;
            incidents_repo::close_if_settled(tx, &incident.id, now())?;

            let leaked_incidents: i64 = tx.query_row(
                "SELECT COUNT(*) FROM incidents WHERE printer_snapshot_json LIKE '%' || ?1 || '%'",
                [SECRET_CORPUS],
                |row| row.get(0),
            )?;
            let leaked_entries: i64 = tx.query_row(
                "SELECT COUNT(*) FROM incident_events WHERE detail_json LIKE '%' || ?1 || '%'",
                [SECRET_CORPUS],
                |row| row.get(0),
            )?;
            assert_eq!(leaked_incidents, 0);
            assert_eq!(leaked_entries, 0);

            // Sanity: the corpus really is in the database (its one
            // legitimate home), so this test isn't vacuous.
            let camera_rows: i64 = tx.query_row(
                "SELECT COUNT(*) FROM printer_cameras WHERE snapshot_url = ?1",
                [SECRET_CORPUS],
                |row| row.get(0),
            )?;
            assert_eq!(camera_rows, 1);
            Ok(())
        })
        .unwrap();
}
