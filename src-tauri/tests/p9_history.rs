//! P9 Task 4 (D11): the Job history read model. `history::repository::list`
//! (filters, escaping, keyset paging, index use) and `timeline` (order,
//! every item kind, pruned evidence, immutability), over a real migrated
//! `Storage` with raw-SQL and P8-API seeds.

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::json;

use farm3d_lib::attention::{
    repository as attention_repo, AttentionDetail, AttentionOrigin, AttentionSubject, Condition,
    ConditionKind,
};
use farm3d_lib::catalog::{BedShape, PrinterProfile};
use farm3d_lib::history::repository::{build_list_sql, list, timeline, validate, ValidQuery};
use farm3d_lib::history::{
    JobHistoryPage, JobHistoryQuery, JobHistoryState, JobTimeline, JobTimelineItem,
    PrinterLifecycleFilter,
};
use farm3d_lib::incidents::{repository as incidents_repo, IncidentEntryDetail, IncidentKind};
use farm3d_lib::jobs::PrinterSnapshot;
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};

#[path = "common/farm_seed.rs"]
mod farm_seed;
use farm_seed::{exec, seed_host_operation, GCODE_HASH};

struct Farm {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Storage,
}

fn farm() -> Farm {
    let temp = tempfile::tempdir().expect("temp");
    let paths =
        StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).expect("paths");
    let lease = MetadataRootLease::acquire(&paths).expect("lease");
    let storage = Storage::open(paths, &lease).expect("storage");
    Farm {
        _temp: temp,
        _lease: lease,
        storage,
    }
}

impl Farm {
    fn write(&self, seed: impl FnOnce(&Connection)) {
        self.storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed(tx);
                Ok(())
            })
            .expect("seed");
    }

    fn list(&self, query: &JobHistoryQuery) -> JobHistoryPage {
        let valid = validate(query).expect("valid query");
        self.storage
            .read_transaction(|tx| Ok(list(tx, &valid)))
            .expect("read")
            .expect("list")
    }

    fn ids(&self, query: &JobHistoryQuery) -> Vec<String> {
        self.list(query)
            .rows
            .into_iter()
            .map(|row| row.job_id)
            .collect()
    }

    fn timeline(&self, job_id: &str) -> Option<JobTimeline> {
        self.storage
            .read_transaction(|tx| Ok(timeline(tx, job_id)))
            .expect("read")
            .expect("timeline")
    }

    fn timeline_json(&self, job_id: &str) -> serde_json::Value {
        serde_json::to_value(self.timeline(job_id).expect("job exists")).unwrap()
    }
}

fn printer_snapshot(name: &str) -> PrinterSnapshot {
    PrinterSnapshot {
        name: name.to_string(),
        location: None,
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
            suggested_port: None,
        },
    }
}

const ABSENT_FACT: &str = r#"{"provenance":"absent","value":null}"#;

fn facts_json() -> String {
    format!(
        r#"{{"printerProfile":{a},"nozzleDiameterMm":{a},"materialFamily":{a},"filamentDiameterMm":{a}}}"#,
        a = ABSENT_FACT
    )
}

/// An external Slice Revision's stored estimates: none.
const ESTIMATES_JSON: &str = r#"{"estimates":null}"#;

/// One settled (or `outcomeUnknown`) Job and the rows it needs.
#[derive(Clone)]
struct Seed {
    id: &'static str,
    printer: &'static str,
    printer_name: &'static str,
    spool: &'static str,
    spool_number: i64,
    model: &'static str,
    model_name: &'static str,
    plate: Option<&'static str>,
    state: &'static str,
    created: &'static str,
    ended: Option<&'static str>,
}

impl Seed {
    fn new(
        id: &'static str,
        state: &'static str,
        created: &'static str,
        ended: Option<&'static str>,
    ) -> Self {
        Seed {
            id,
            printer: "prn-a",
            printer_name: "Voron",
            spool: "spl-a",
            spool_number: 1,
            model: "mdl-a",
            model_name: "Benchy",
            plate: None,
            state,
            created,
            ended,
        }
    }
    fn printer(mut self, id: &'static str, name: &'static str) -> Self {
        self.printer = id;
        self.printer_name = name;
        self
    }
    fn spool(mut self, id: &'static str, number: i64) -> Self {
        self.spool = id;
        self.spool_number = number;
        self
    }
    fn model(mut self, id: &'static str, name: &'static str) -> Self {
        self.model = id;
        self.model_name = name;
        self
    }
    fn plate(mut self, name: &'static str) -> Self {
        self.plate = Some(name);
        self
    }
}

fn seed_job(conn: &Connection, seed: &Seed) {
    let Seed {
        id,
        printer,
        printer_name,
        spool,
        spool_number,
        model,
        model_name,
        state,
        created,
        ..
    } = seed.clone();
    let snapshot = serde_json::to_string(&printer_snapshot(printer_name)).unwrap();
    let ended = seed
        .ended
        .map_or("NULL".to_string(), |at| format!("'{at}'"));
    let (settlement, method, cancel) = match state {
        "completed" => ("settled", "'estimated'", "NULL"),
        "failed" => ("pending", "NULL", "NULL"),
        "cancelled" => ("pending", "NULL", "'cancelledByOperator'"),
        _ => ("open", "NULL", "NULL"),
    };
    let (plate_kind, plate_columns, plate_values, runtime) = match seed.plate {
        Some(name) => (
            "farm3d",
            ", plate_key, plate_index, plate_name",
            format!(", 'p1', 1, '{name}'"),
            "'{}'",
        ),
        None => ("external", "", String::new(), "NULL"),
    };
    let target = "null";
    let facts = facts_json();
    exec(
        conn,
        &format!(
            "INSERT OR IGNORE INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES ('{printer}', 1, '{printer_name}', '', '', '', '', '', '', '{{}}', '{created}', '{created}');
             INSERT OR IGNORE INTO content_blobs(sha256, size_bytes, created_at)
               VALUES ('{GCODE_HASH}', 200, '{created}');
             INSERT OR IGNORE INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
               VALUES ('{model}', 1, '{model_name}', 'gcode', 'managed', '{created}', '{created}');
             INSERT OR IGNORE INTO model_source_revisions(id, model_id, sequence, content_sha256,
               size_bytes, format, origin, source_file_name, source_path, captured_at,
               inspector_version, inspection_json)
               VALUES ('msr-{model}', '{model}', 1, '{GCODE_HASH}', 200, 'gcode', 'import',
                       'p.gcode', '/p.gcode', '{created}', 1, '{{}}');
             INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
               gcode_size, target_json, facts_json, requires_manual_printer_selection,
               estimates_json, runtime_json, created_at{plate_columns})
               VALUES ('slr-{id}', '{plate_kind}', '{model}', 'msr-{model}', '{GCODE_HASH}', 200,
                       '{target}', '{facts}', 1, '{ESTIMATES_JSON}', {runtime}, '{created}'{plate_values});
             INSERT OR IGNORE INTO spools(id, revision, spool_number, manufacturer, material_family,
               color_name, diameter, nominal_mg, current_mg, confidence, lifecycle, created_at,
               updated_at)
               VALUES ('{spool}', 1, {spool_number}, 'Acme', 'PLA', 'Black', '1.75', 1000000,
                       1000000, 'measured', 'active', '{created}', '{created}');
             INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg, state,
               operation_id, created_at)
               VALUES ('rsv-{id}', '{spool}', 'job', '{id}', 500000, 'consumed', 'rsv-{id}-op', '{created}');
             INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
               state, close_reason, policy, preference, estimate_mg, estimate_source, created_at,
               updated_at, closed_at)
               VALUES ('qen-{id}', 1, 'slr-{id}', 'qln-{id}', 1, 'closed', 'completed', 'manual',
                       'loadedFirst', 500000, 'operatorEntered', '{created}', '{created}', '{created}');
             INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
               printer_snapshot_json, spool_id, reservation_id, estimate_mg, state, cancel_reason,
               settlement, settlement_method, assigned_by, created_at, updated_at, ended_at)
               VALUES ('{id}', 1, 'qen-{id}', 'slr-{id}', '{printer}', '{snapshot}', '{spool}',
                       'rsv-{id}', 500000, '{state}', {cancel}, '{settlement}', {method},
                       'operator', '{created}', '{created}', {ended});
             UPDATE queue_entries SET job_id = '{id}' WHERE id = 'qen-{id}';",
            snapshot = snapshot.replace('\'', "''"),
        ),
    );
}

fn q() -> JobHistoryQuery {
    JobHistoryQuery::default()
}

fn text(text: &str) -> JobHistoryQuery {
    JobHistoryQuery {
        text: Some(text.to_string()),
        ..q()
    }
}

/// Four settled Jobs over two Printers, two Spools, and two Models, plus
/// an `outcomeUnknown` one on a third Printer.
fn standard(farm: &Farm) {
    farm.write(|conn| {
        seed_job(
            conn,
            &Seed::new(
                "job-1",
                "completed",
                "2026-03-01T08:00:00.000Z",
                Some("2026-03-01T09:00:00.000Z"),
            )
            .plate("Plate One"),
        );
        seed_job(
            conn,
            &Seed::new(
                "job-2",
                "failed",
                "2026-03-02T08:00:00.000Z",
                Some("2026-03-02T09:00:00.000Z"),
            )
            .printer("prn-b", "Trident")
            .spool("spl-b", 2),
        );
        seed_job(
            conn,
            &Seed::new(
                "job-3",
                "cancelled",
                "2026-03-03T08:00:00.000Z",
                Some("2026-03-03T09:00:00.000Z"),
            )
            .model("mdl-b", "Gear"),
        );
        seed_job(
            conn,
            &Seed::new(
                "job-4",
                "completed",
                "2026-03-04T08:00:00.000Z",
                Some("2026-03-04T09:00:00.000Z"),
            )
            .printer("prn-b", "Trident")
            .spool("spl-b", 2)
            .model("mdl-b", "Gear"),
        );
        seed_job(
            conn,
            &Seed::new("job-u", "outcomeUnknown", "2026-03-02T12:00:00.000Z", None)
                .printer("prn-c", "Ender")
                .spool("spl-c", 3),
        );
    });
}

// --- filters ---------------------------------------------------------------

#[test]
fn the_default_query_lists_settled_jobs_newest_first_and_hides_outcome_unknown() {
    let farm = farm();
    standard(&farm);
    let page = farm.list(&q());
    assert_eq!(
        page.rows
            .iter()
            .map(|row| row.job_id.as_str())
            .collect::<Vec<_>>(),
        ["job-4", "job-3", "job-2", "job-1"]
    );
    assert_eq!(page.next_cursor, None);
    let first = &page.rows[3];
    assert_eq!(first.state, JobHistoryState::Completed);
    assert_eq!(first.history_at, "2026-03-01T09:00:00.000Z");
    assert_eq!(first.printer_snapshot_name, "Voron");
    assert_eq!(first.spool_number, 1);
    assert_eq!(first.model_name, "Benchy");
    assert_eq!(first.plate_name.as_deref(), Some("Plate One"));
    assert!(!first.printer_archived);
    assert_eq!(first.snapshot_count, 0);
    assert_eq!(first.incident_id, None);
    assert!(page.rows[1].cancel_reason.is_some());
}

#[test]
fn every_filter_works_alone() {
    let farm = farm();
    standard(&farm);
    let by = |query: JobHistoryQuery| farm.ids(&query);
    assert_eq!(
        by(JobHistoryQuery {
            states: Some(vec![JobHistoryState::Failed]),
            ..q()
        }),
        ["job-2"]
    );
    assert_eq!(
        by(JobHistoryQuery {
            states: Some(vec![JobHistoryState::Completed, JobHistoryState::Cancelled]),
            ..q()
        }),
        ["job-4", "job-3", "job-1"]
    );
    assert_eq!(
        by(JobHistoryQuery {
            printer_id: Some("prn-b".into()),
            ..q()
        }),
        ["job-4", "job-2"]
    );
    assert_eq!(
        by(JobHistoryQuery {
            spool_id: Some("spl-a".into()),
            ..q()
        }),
        ["job-3", "job-1"]
    );
    assert_eq!(
        by(JobHistoryQuery {
            model_id: Some("mdl-b".into()),
            ..q()
        }),
        ["job-4", "job-3"]
    );
    // After is inclusive, before exclusive.
    assert_eq!(
        by(JobHistoryQuery {
            ended_after: Some("2026-03-02T09:00:00.000Z".into()),
            ..q()
        }),
        ["job-4", "job-3", "job-2"]
    );
    assert_eq!(
        by(JobHistoryQuery {
            ended_before: Some("2026-03-02T09:00:00.000Z".into()),
            ..q()
        }),
        ["job-1"]
    );
    // An offset is normalised to UTC: 10:00+01:00 is 09:00Z.
    assert_eq!(
        by(JobHistoryQuery {
            ended_after: Some("2026-03-04T10:00:00+01:00".into()),
            ..q()
        }),
        ["job-4"]
    );
    assert_eq!(by(text("Trident")), ["job-4", "job-2"]);
    assert_eq!(by(text("gear")), ["job-4", "job-3"]);
    assert_eq!(by(text("plate one")), ["job-1"]);
    assert_eq!(by(text("job-3")), ["job-3"]);
    assert_eq!(by(text("#2")), ["job-4", "job-2"]);
    assert_eq!(
        by(text("   ")),
        ["job-4", "job-3", "job-2", "job-1"],
        "blank means absent"
    );
}

#[test]
fn filters_combine() {
    let farm = farm();
    standard(&farm);
    let combined = JobHistoryQuery {
        states: Some(vec![JobHistoryState::Completed]),
        printer_id: Some("prn-b".into()),
        spool_id: Some("spl-b".into()),
        model_id: Some("mdl-b".into()),
        ended_after: Some("2026-03-04T00:00:00Z".into()),
        ended_before: Some("2026-03-05T00:00:00Z".into()),
        text: Some("gear".into()),
        printer_lifecycle: Some(PrinterLifecycleFilter::Active),
        ..q()
    };
    assert_eq!(farm.ids(&combined), ["job-4"]);
    assert_eq!(
        farm.ids(&JobHistoryQuery {
            model_id: Some("mdl-a".into()),
            ..combined
        }),
        Vec::<String>::new()
    );
}

#[test]
fn the_archived_filter_finds_jobs_by_the_snapshot_name_of_an_archived_printer() {
    let farm = farm();
    standard(&farm);
    // The Printer is archived and renamed afterwards: the snapshot name
    // is what the Job remembers.
    farm.write(|conn| {
        exec(
            conn,
            "UPDATE printers SET archived_at = '2026-04-01T00:00:00.000Z', name = 'Renamed' WHERE id = 'prn-b';",
        );
    });
    let archived = JobHistoryQuery {
        printer_lifecycle: Some(PrinterLifecycleFilter::Archived),
        ..q()
    };
    assert_eq!(farm.ids(&archived), ["job-4", "job-2"]);
    assert_eq!(
        farm.ids(&JobHistoryQuery {
            text: Some("Trident".into()),
            ..archived.clone()
        }),
        ["job-4", "job-2"]
    );
    assert_eq!(
        farm.ids(&JobHistoryQuery {
            text: Some("Renamed".into()),
            ..archived.clone()
        }),
        Vec::<String>::new()
    );
    assert_eq!(
        farm.ids(&JobHistoryQuery {
            printer_lifecycle: Some(PrinterLifecycleFilter::Active),
            ..q()
        }),
        ["job-3", "job-1"]
    );
    let row = &farm.list(&archived).rows[0];
    assert!(row.printer_archived);
    assert_eq!(row.printer_snapshot_name, "Trident");
}

#[test]
fn text_escapes_like_wildcards_and_handles_non_ascii() {
    let farm = farm();
    farm.write(|conn| {
        let jobs = [
            ("job-p", "100% PLA", "2026-03-01T09:00:00.000Z"),
            ("job-u", "a_b", "2026-03-02T09:00:00.000Z"),
            ("job-x", "axb", "2026-03-03T09:00:00.000Z"),
            ("job-s", "back\\slash", "2026-03-04T09:00:00.000Z"),
            ("job-n", "Ünïcode", "2026-03-05T09:00:00.000Z"),
        ];
        for (index, (id, name, ended)) in jobs.into_iter().enumerate() {
            let model: &'static str = ["mdl-p", "mdl-u", "mdl-x", "mdl-s", "mdl-n"][index];
            seed_job(
                conn,
                &Seed::new(id, "completed", "2026-03-01T00:00:00.000Z", Some(ended))
                    .model(model, name),
            );
        }
    });
    assert_eq!(farm.ids(&text("%")), ["job-p"], "a percent is literal");
    assert_eq!(farm.ids(&text("_")), ["job-u"], "an underscore is literal");
    assert_eq!(farm.ids(&text("a_b")), ["job-u"]);
    assert_eq!(farm.ids(&text("\\")), ["job-s"], "a backslash is literal");
    assert_eq!(farm.ids(&text("Ünï")), ["job-n"]);
    // LIKE folds ASCII case only: non-ASCII is case-sensitive.
    assert_eq!(farm.ids(&text("ünï")), Vec::<String>::new());
    assert_eq!(farm.ids(&text("PLA")), ["job-p"]);
    assert_eq!(farm.ids(&text("pla")), ["job-p"], "ASCII folds");
}

// --- paging ----------------------------------------------------------------

#[test]
fn keyset_paging_is_stable_while_new_jobs_finish() {
    let farm = farm();
    farm.write(|conn| {
        for (index, id) in ["job-01", "job-02", "job-03", "job-04", "job-05"]
            .into_iter()
            .enumerate()
        {
            let ended: &'static str = [
                "2026-03-01T09:00:00.000Z",
                "2026-03-02T09:00:00.000Z",
                "2026-03-03T09:00:00.000Z",
                "2026-03-04T09:00:00.000Z",
                "2026-03-05T09:00:00.000Z",
            ][index];
            seed_job(
                conn,
                &Seed::new(id, "completed", "2026-03-01T00:00:00.000Z", Some(ended)),
            );
        }
    });
    let first = farm.list(&JobHistoryQuery {
        limit: Some(2),
        ..q()
    });
    assert_eq!(
        first
            .rows
            .iter()
            .map(|r| r.job_id.as_str())
            .collect::<Vec<_>>(),
        ["job-05", "job-04"]
    );
    let cursor = first.next_cursor.clone().expect("more rows follow");

    // A Job finishes between pages: it sorts above the first page.
    farm.write(|conn| {
        seed_job(
            conn,
            &Seed::new(
                "job-06",
                "completed",
                "2026-03-06T00:00:00.000Z",
                Some("2026-03-06T09:00:00.000Z"),
            ),
        );
    });
    let second = farm.list(&JobHistoryQuery {
        limit: Some(2),
        after: Some(cursor),
        ..q()
    });
    assert_eq!(
        second
            .rows
            .iter()
            .map(|r| r.job_id.as_str())
            .collect::<Vec<_>>(),
        ["job-03", "job-02"]
    );
    let third = farm.list(&JobHistoryQuery {
        limit: Some(2),
        after: second.next_cursor.clone(),
        ..q()
    });
    assert_eq!(
        third
            .rows
            .iter()
            .map(|r| r.job_id.as_str())
            .collect::<Vec<_>>(),
        ["job-01"]
    );
    assert_eq!(third.next_cursor, None, "no extra row, no cursor");
}

#[test]
fn ties_on_the_history_time_page_by_id() {
    let farm = farm();
    farm.write(|conn| {
        for id in ["job-a", "job-b", "job-c", "job-d"] {
            seed_job(
                conn,
                &Seed::new(
                    id,
                    "completed",
                    "2026-03-01T00:00:00.000Z",
                    Some("2026-03-01T09:00:00.000Z"),
                ),
            );
        }
    });
    let mut seen = Vec::new();
    let mut after = None;
    loop {
        let page = farm.list(&JobHistoryQuery {
            limit: Some(1),
            after: after.clone(),
            ..q()
        });
        seen.extend(page.rows.iter().map(|r| r.job_id.clone()));
        after = page.next_cursor;
        if after.is_none() {
            break;
        }
    }
    assert_eq!(seen, ["job-d", "job-c", "job-b", "job-a"]);
}

#[test]
fn outcome_unknown_is_opt_in_and_pages_by_created_at() {
    let farm = farm();
    farm.write(|conn| {
        seed_job(
            conn,
            &Seed::new(
                "job-1",
                "completed",
                "2026-03-01T00:00:00.000Z",
                Some("2026-03-01T09:00:00.000Z"),
            ),
        );
        seed_job(
            conn,
            &Seed::new("job-u1", "outcomeUnknown", "2026-03-02T08:00:00.000Z", None)
                .printer("prn-u1", "U1"),
        );
        seed_job(
            conn,
            &Seed::new("job-u2", "outcomeUnknown", "2026-03-03T08:00:00.000Z", None)
                .printer("prn-u2", "U2"),
        );
    });
    assert_eq!(farm.ids(&q()), ["job-1"]);
    let all = JobHistoryQuery {
        states: Some(JobHistoryState::ALL.to_vec()),
        limit: Some(2),
        ..q()
    };
    let first = farm.list(&all);
    assert_eq!(
        first
            .rows
            .iter()
            .map(|r| r.job_id.as_str())
            .collect::<Vec<_>>(),
        ["job-u2", "job-u1"]
    );
    assert_eq!(
        first.rows[0].history_at, "2026-03-03T08:00:00.000Z",
        "created_at, no ended_at"
    );
    assert_eq!(first.rows[0].ended_at, None);
    let second = farm.list(&JobHistoryQuery {
        after: first.next_cursor,
        ..all
    });
    assert_eq!(
        second
            .rows
            .iter()
            .map(|r| r.job_id.as_str())
            .collect::<Vec<_>>(),
        ["job-1"]
    );
    assert_eq!(
        farm.ids(&JobHistoryQuery {
            states: Some(vec![JobHistoryState::OutcomeUnknown]),
            ..q()
        }),
        ["job-u2", "job-u1"]
    );
}

#[test]
fn the_query_is_validated() {
    let bad = |query: JobHistoryQuery| validate(&query).expect_err("invalid").field;
    assert_eq!(
        bad(JobHistoryQuery {
            limit: Some(0),
            ..q()
        }),
        "query.limit"
    );
    assert_eq!(
        bad(JobHistoryQuery {
            limit: Some(201),
            ..q()
        }),
        "query.limit"
    );
    assert_eq!(
        bad(JobHistoryQuery {
            limit: Some(-1),
            ..q()
        }),
        "query.limit"
    );
    assert!(validate(&JobHistoryQuery {
        limit: Some(1),
        ..q()
    })
    .is_ok());
    assert!(validate(&JobHistoryQuery {
        limit: Some(200),
        ..q()
    })
    .is_ok());
    assert_eq!(
        bad(JobHistoryQuery {
            states: Some(vec![]),
            ..q()
        }),
        "query.states"
    );
    assert_eq!(
        bad(JobHistoryQuery {
            states: Some(vec![JobHistoryState::Failed, JobHistoryState::Failed]),
            ..q()
        }),
        "query.states"
    );
    assert!(validate(&JobHistoryQuery {
        states: Some(JobHistoryState::ALL.to_vec()),
        ..q()
    })
    .is_ok());
    assert_eq!(bad(text(&"x".repeat(201))), "query.text");
    assert!(validate(&text(&"x".repeat(200))).is_ok());
    assert!(
        validate(&text(&format!("  {}  ", "x".repeat(200)))).is_ok(),
        "trimmed first"
    );
    assert!(
        validate(&text(&"é".repeat(200))).is_ok(),
        "characters, not bytes"
    );
    assert_eq!(
        bad(JobHistoryQuery {
            after: Some("not a cursor".into()),
            ..q()
        }),
        "query.after"
    );
    assert_eq!(
        bad(JobHistoryQuery {
            after: Some("bm9waXBl".into()),
            ..q()
        }),
        "query.after"
    );
    assert_eq!(
        bad(JobHistoryQuery {
            ended_after: Some("yesterday".into()),
            ..q()
        }),
        "query.endedAfter"
    );
    assert_eq!(
        bad(JobHistoryQuery {
            ended_before: Some("2026-03-01".into()),
            ..q()
        }),
        "query.endedBefore"
    );
}

// --- index use ---------------------------------------------------------------

fn plan(farm: &Farm, query: &JobHistoryQuery) -> Vec<String> {
    let valid: ValidQuery = validate(query).unwrap();
    let (sql, params) = build_list_sql(&valid);
    let details = farm
        .storage
        .read_transaction(|tx| {
            let mut statement = tx.prepare(&format!("EXPLAIN QUERY PLAN {sql}"))?;
            let rows = statement
                .query_map(rusqlite::params_from_iter(params), |row| {
                    row.get::<_, String>(3)
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(rows)
        })
        .expect("plan");
    println!("PLAN for {query:?}:");
    for line in &details {
        println!("  {line}");
    }
    details
}

fn assert_plan_uses(details: &[String], index: &str) {
    let scan = details
        .iter()
        .find(|line| line.starts_with("SCAN j") || line.starts_with("SEARCH j"))
        .unwrap_or_else(|| panic!("no j step in {details:?}"));
    assert!(scan.contains(&format!("USING INDEX {index}")), "{scan}");
    assert!(
        !details.iter().any(|line| line.contains("TEMP B-TREE")),
        "the index order serves ORDER BY: {details:?}"
    );
}

#[test]
fn the_default_query_uses_jobs_history_with_a_seekable_keyset() {
    let farm = farm();
    standard(&farm);
    assert_plan_uses(&plan(&farm, &q()), "jobs_history");
    let cursor = farm
        .list(&JobHistoryQuery {
            limit: Some(1),
            ..q()
        })
        .next_cursor;
    let seeking = plan(
        &farm,
        &JobHistoryQuery {
            after: cursor,
            ..q()
        },
    );
    assert_plan_uses(&seeking, "jobs_history");
    let scan = seeking
        .iter()
        .find(|line| line.contains("jobs_history"))
        .unwrap();
    assert!(
        scan.starts_with("SEARCH j"),
        "the keyset seeks instead of scanning: {scan}"
    );
    assert!(scan.contains('<'), "{scan}");
}

#[test]
fn the_printer_and_text_filters_use_the_matching_index() {
    let farm = farm();
    standard(&farm);
    let printer = JobHistoryQuery {
        printer_id: Some("prn-b".into()),
        ..q()
    };
    assert_plan_uses(&plan(&farm, &printer), "jobs_history_printer");
    let with_text = JobHistoryQuery {
        text: Some("Trident".into()),
        ..printer.clone()
    };
    assert_plan_uses(&plan(&farm, &with_text), "jobs_history_printer");
    let seeking = JobHistoryQuery {
        after: farm
            .list(&JobHistoryQuery {
                limit: Some(1),
                ..printer.clone()
            })
            .next_cursor,
        ..printer
    };
    assert_plan_uses(&plan(&farm, &seeking), "jobs_history_printer");
    // Text alone has no Printer to seek on: the default index serves it.
    assert_plan_uses(&plan(&farm, &text("Trident")), "jobs_history");
    let wide = JobHistoryQuery {
        states: Some(vec![JobHistoryState::Failed]),
        model_id: Some("mdl-a".into()),
        spool_id: Some("spl-a".into()),
        printer_lifecycle: Some(PrinterLifecycleFilter::Archived),
        ended_after: Some("2026-01-01T00:00:00Z".into()),
        ended_before: Some("2027-01-01T00:00:00Z".into()),
        ..q()
    };
    assert_plan_uses(&plan(&farm, &wide), "jobs_history");
}

// --- timeline ----------------------------------------------------------------

fn subject() -> AttentionSubject {
    AttentionSubject {
        printer_name: Some("Voron".to_string()),
        printer_location: None,
        job_label: Some("Benchy".to_string()),
        spool_number: None,
        spool_label: None,
    }
}

fn at(text: &str) -> DateTime<Utc> {
    text.parse().unwrap()
}

/// A completed, corrected Job with one of everything on its timeline.
fn timeline_farm() -> Farm {
    let farm = farm();
    farm.write(|conn| {
        seed_job(
            conn,
            &Seed::new("job-t", "completed", "2026-03-01T10:00:00.000Z", Some("2026-03-01T12:00:00.000Z")),
        );
        exec(
            conn,
            "UPDATE spool_reservations SET created_at = '2026-03-01T10:00:00.000Z', settled_at = '2026-03-01T12:00:01.000Z' WHERE id = 'rsv-job-t';
             UPDATE jobs SET correction_event_id = 'sev-fix' WHERE id = 'job-t';
             INSERT INTO job_events(id, job_id, sequence, kind, from_state, to_state, at)
               VALUES ('jev-1', 'job-t', 1, 'assigned', NULL, 'assigned', '2026-03-01T10:00:00.000Z'),
                      ('jev-2', 'job-t', 2, 'completed', 'printing', 'completed', '2026-03-01T12:00:00.000Z');
             INSERT INTO spool_amount_events(id, spool_id, sequence, kind, before_mg, after_mg,
                 confidence_after, reservation_id, occurred_at)
               VALUES ('sev-0', 'spl-a', 1, 'initial', NULL, 1000000, 'measured', NULL, '2026-02-01T00:00:00.000Z'),
                      ('sev-use', 'spl-a', 2, 'consumption', 1000000, 600000, 'estimated', 'rsv-job-t', '2026-03-01T12:00:00.000Z');
             INSERT INTO spool_amount_events(id, spool_id, sequence, kind, before_mg, after_mg,
                 confidence_after, note, occurred_at)
               VALUES ('sev-fix', 'spl-a', 3, 'measurement', 600000, 550000, 'measured', 'weighed', '2026-03-02T08:00:00.000Z');
             INSERT INTO reconciliation_requirements(id, job_id, kind, status, spool_id,
                 reservation_id, opened_at, resolved_at, resolution_json)
               VALUES ('rrq-t', 'job-t', 'materialReconciliation', 'resolved', 'spl-a', 'rsv-job-t',
                       '2026-03-01T12:00:00.000Z', '2026-03-01T12:00:01.000Z',
                       '{\"kind\":\"settled\",\"method\":\"estimated\",\"usedMg\":400000}');",
        );
        seed_host_operation(conn, "hop-t", "prn-a", "upload", "dispatching", GCODE_HASH);
        exec(
            conn,
            "UPDATE host_operations SET job_id = 'job-t', created_at = '2026-03-01T10:05:00.000Z',
                endpoint_json = '{\"kind\":\"moonraker\",\"host\":\"192.0.2.10\",\"port\":7125}'
             WHERE id = 'hop-t';",
        );
    });
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            let condition = Condition {
                kind: ConditionKind::JobFailed,
                source_id: "job-t".to_string(),
                printer_id: Some("prn-a".to_string()),
                job_id: Some("job-t".to_string()),
                spool_id: None,
                requirement_id: None,
                subject: subject(),
                detail: AttentionDetail::JobFailed {
                    ended_at: "2026-03-01T12:00:00.000Z".to_string(),
                },
                acknowledge: false,
            };
            let event = attention_repo::insert(
                tx,
                &condition,
                None,
                false,
                AttentionOrigin::Live,
                at("2026-03-01T12:00:00Z"),
            )?;
            let incident = incidents_repo::open(
                tx,
                IncidentKind::JobFailed,
                "prn-a",
                Some("job-t"),
                &printer_snapshot("Voron"),
                &event.id,
                at("2026-03-01T12:00:00Z"),
            )?;
            incidents_repo::append_entry(
                tx,
                &incident.id,
                &IncidentEntryDetail::NoteAdded {
                    text: "checked the nozzle".to_string(),
                },
                None,
                at("2026-03-01T13:00:00Z"),
            )?;
            exec(
                tx,
                &format!(
                    "INSERT INTO camera_snapshots(id, printer_id, incident_id, job_id, trigger,
                         captured_at, content_type, byte_len, sha256, rel_path)
                       VALUES ('snp-live', 'prn-a', '{incident}', 'job-t', 'incident',
                               '2026-03-01T12:00:02.000Z', 'image/jpeg', 100, '{a}', 'snapshots/2026/03/snp-live.jpg');
                     INSERT INTO camera_snapshots(id, printer_id, job_id, trigger,
                         captured_at, content_type, byte_len, sha256, rel_path)
                       VALUES ('snp-done', 'prn-a', 'job-t', 'completion',
                               '2026-03-01T12:00:03.000Z', 'image/png', 100, '{b}', 'snapshots/2026/03/snp-done.png');",
                    incident = incident.id,
                    a = "a".repeat(64),
                    b = "b".repeat(64),
                ),
            );
            Ok(())
        })
        .expect("p8 seed");
    farm
}

fn sources(timeline: &JobTimeline) -> Vec<String> {
    let value = serde_json::to_value(&timeline.items).unwrap();
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|item| item["source"].as_str().unwrap().to_string())
        .collect()
}

#[test]
fn an_unknown_job_has_no_timeline() {
    let farm = farm();
    assert!(farm.timeline("job-missing").is_none());
}

#[test]
fn the_timeline_carries_every_item_kind_in_a_deterministic_order() {
    let farm = timeline_farm();
    let timeline = farm.timeline("job-t").expect("timeline");
    assert_eq!(timeline.job.id, "job-t");
    assert_eq!(timeline.printer_snapshot.name, "Voron");
    assert_eq!(timeline.spool_number, 1);
    assert_eq!(timeline.slice_revision.id, "slr-job-t");
    assert_eq!(timeline.lineage.len(), 1);
    assert!(timeline.incident.is_some());

    let kinds = sources(&timeline);
    for kind in [
        "job",
        "hostOperation",
        "reservation",
        "amountEvent",
        "requirement",
        "attention",
        "incident",
        "snapshot",
    ] {
        assert!(
            kinds.iter().any(|k| k == kind),
            "{kind} missing from {kinds:?}"
        );
    }
    // By time, then source, then the source's sequence.
    assert_eq!(
        kinds,
        [
            "job",           // 10:00:00 assigned (jev-1)
            "reservation",   // 10:00:00
            "hostOperation", // 10:05
            "job",           // 12:00:00 completed (jev-2)
            "amountEvent",   // 12:00:00 consumption
            "requirement",   // 12:00:00
            "attention",     // 12:00:00 first observed
            "incident",      // 12:00:00 opened
            "snapshot",      // 12:00:02
            "snapshot",      // 12:00:03
            "incident",      // 13:00 note
            "amountEvent",   // 2026-03-02 correction
        ]
    );
}

#[test]
fn the_timeline_order_is_pinned() {
    let farm = timeline_farm();
    let timeline = farm.timeline("job-t").unwrap();
    let shape: Vec<(String, String)> = serde_json::to_value(&timeline.items)
        .unwrap()
        .as_array()
        .unwrap()
        .iter()
        .map(|item| {
            (
                item["at"].as_str().unwrap().to_string(),
                item["source"].as_str().unwrap().to_string(),
            )
        })
        .collect();
    let sorted = {
        let mut sorted = shape.clone();
        sorted.sort_by(|a, b| a.0.cmp(&b.0));
        sorted
    };
    assert_eq!(
        shape.iter().map(|s| &s.0).collect::<Vec<_>>(),
        sorted.iter().map(|s| &s.0).collect::<Vec<_>>(),
        "ascending by time"
    );
    let rank = |source: &str| {
        [
            "job",
            "hostOperation",
            "reservation",
            "amountEvent",
            "requirement",
            "attention",
            "incident",
            "snapshot",
        ]
        .iter()
        .position(|s| *s == source)
        .unwrap()
    };
    for pair in shape.windows(2) {
        if pair[0].0 == pair[1].0 {
            assert!(rank(&pair[0].1) <= rank(&pair[1].1), "{pair:?}");
        }
    }
    // Only this Job's ledger rows: not the Spool's initial event.
    let amount_ids: Vec<String> = timeline
        .items
        .iter()
        .filter_map(|item| match item {
            JobTimelineItem::AmountEvent {
                amount_event,
                is_correction,
                ..
            } => Some(format!("{}:{is_correction}", amount_event.id)),
            _ => None,
        })
        .collect();
    assert_eq!(amount_ids, ["sev-use:false", "sev-fix:true"]);
    let jobs: Vec<i64> = timeline
        .items
        .iter()
        .filter_map(|item| match item {
            JobTimelineItem::Job { event, .. } => Some(event.sequence),
            _ => None,
        })
        .collect();
    assert_eq!(jobs, [1, 2]);
    // Repeated reads are identical.
    assert_eq!(farm.timeline_json("job-t"), farm.timeline_json("job-t"));
}

#[test]
fn a_pruned_snapshot_shows_as_pruned() {
    let farm = timeline_farm();
    farm.write(|conn| {
        exec(
            conn,
            "UPDATE camera_snapshots SET revision = revision + 1, pruned_at = '2026-04-01T00:00:00.000Z',
                prune_reason = 'age' WHERE id = 'snp-done';",
        );
    });
    let timeline = farm.timeline("job-t").unwrap();
    let pruned: Vec<_> = timeline
        .items
        .iter()
        .filter_map(|item| match item {
            JobTimelineItem::Snapshot { snapshot, .. } => {
                Some((snapshot.id.clone(), snapshot.pruned_at.is_some()))
            }
            _ => None,
        })
        .collect();
    assert_eq!(
        pruned,
        [
            ("snp-live".to_string(), false),
            ("snp-done".to_string(), true)
        ]
    );
    let history = farm.list(&q());
    assert_eq!(history.rows[0].snapshot_count, 2, "pruned rows still count");
    assert!(history.rows[0].incident_id.is_some());
}

/// Removes the fields D11 allows to change when media is pruned.
fn without_prune_fields(mut value: serde_json::Value) -> serde_json::Value {
    for item in value["items"].as_array_mut().unwrap() {
        if item["source"] == "snapshot" {
            let snapshot = item["snapshot"].as_object_mut().unwrap();
            for key in ["revision", "prunedAt", "pruneReason"] {
                snapshot.remove(key);
            }
        }
    }
    value
}

#[test]
fn the_timeline_is_immutable_across_archiving_and_pruning() {
    let farm = timeline_farm();
    let before = farm.timeline_json("job-t");

    // Archive the Printer (and rename it).
    farm.write(|conn| {
        exec(conn, "UPDATE printers SET archived_at = '2026-05-01T00:00:00.000Z', revision = revision + 1 WHERE id = 'prn-a';");
    });
    assert_eq!(
        farm.timeline_json("job-t"),
        before,
        "archiving the Printer changes nothing"
    );
    farm.write(|conn| {
        exec(
            conn,
            "UPDATE printers SET name = 'Something else' WHERE id = 'prn-a';",
        );
    });
    assert_eq!(
        farm.timeline_json("job-t"),
        before,
        "the Printer's current name never shows"
    );

    // Archive the Spool.
    farm.write(|conn| {
        exec(conn, "UPDATE spools SET lifecycle = 'archived', archived_from = 'active', revision = revision + 1 WHERE id = 'spl-a';");
    });
    assert_eq!(
        farm.timeline_json("job-t"),
        before,
        "archiving the Spool changes nothing"
    );

    // Prune media.
    farm.write(|conn| {
        exec(
            conn,
            "UPDATE camera_snapshots SET revision = revision + 1, pruned_at = '2026-06-01T00:00:00.000Z',
                prune_reason = 'age';",
        );
    });
    let after = farm.timeline_json("job-t");
    assert_ne!(after, before, "the pruned flag shows");
    assert_eq!(without_prune_fields(after), without_prune_fields(before));
}

#[test]
fn history_rows_carry_the_current_links_the_timeline_omits() {
    let farm = timeline_farm();
    farm.write(|conn| {
        exec(
            conn,
            "UPDATE printers SET archived_at = '2026-05-01T00:00:00.000Z' WHERE id = 'prn-a';
             UPDATE library_models SET name = 'Renamed model' WHERE id = 'mdl-a';",
        );
    });
    let row = farm.list(&q()).rows.remove(0);
    assert!(row.printer_archived);
    assert_eq!(row.model_name, "Renamed model");
    assert_eq!(row.printer_snapshot_name, "Voron");
    let json = farm.timeline_json("job-t");
    assert_eq!(json["printerSnapshot"]["name"], json!("Voron"));
    assert!(json.get("printerArchived").is_none());
}
