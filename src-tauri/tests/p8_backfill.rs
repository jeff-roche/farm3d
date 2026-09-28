//! P8 Task 6 (D2 "Startup backfill", ADR-0014): the startup backfill is
//! the projector's own pass with every live-status family unknown. It
//! projects every durable P7 Reconciliation Requirement exactly once,
//! whatever its age, and running it again (or rebuilding the runtime over
//! the same database) changes nothing. Resolving a requirement resolves
//! its Event without erasing either history.
//!
//! The P7 state is real: three Jobs fail on `FakeMoonraker` through the
//! P7 dispatch rig, and one requirement is deferred through
//! `settle_job_material`. The attention epoch is then moved after them,
//! as if P8's migration ran on a database P7 had already filled.

mod common;
mod p7_dispatch_rig;

use std::time::Duration;

use farm3d_lib::attention::projector::{backfill, EventChange};
use farm3d_lib::attention::services::AttentionTimings;
use farm3d_lib::attention::{AttentionOrigin, AttentionResolution, ConditionKind};
use farm3d_lib::jobs::JobTimings;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;
use p7_dispatch_rig::{
    boot_tuned, boot_with_attention, fast, status_of, AttentionBoot, Driver, Roots, Running,
};
use serde_json::{json, Value};

fn attention() -> AttentionBoot {
    AttentionBoot {
        timings: AttentionTimings {
            pass_min_interval: Duration::ZERO,
            safety_tick: Duration::from_secs(3600),
        },
        clock: None,
        cameras: None,
        notifications: None,
    }
}

fn boot_attention(roots: &Roots) -> (Running, farm3d_lib::attention::projector::AppliedChanges) {
    boot_with_attention(roots, status_of(OperationalState::Ready), quiet(), attention())
}

/// No driver poll during the test: nothing moves a Job but the test.
fn quiet() -> JobTimings {
    JobTimings {
        history_poll: Duration::from_secs(3600),
        ..JobTimings::default()
    }
}

/// A Job that printed and failed on the fake: its material requirement
/// is `pending`.
fn failed_job(app: &Running, roots: &Roots) -> String {
    let job = app.printing();
    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&job, "failed");
    // The operator clears the host for the next print.
    roots.fake.with_state(|state| state.print_state = "standby".to_string());
    app.status(OperationalState::Ready);
    job
}

/// P7 state with two pending material requirements and one deferred,
/// every Job ended before the attention epoch. Returns the three Job ids
/// (the second is the deferred one).
fn seed_p7_state(roots: &Roots) -> Vec<String> {
    let app = boot_tuned(
        roots,
        Driver::Started,
        status_of(OperationalState::Ready),
        fast(),
        None,
    );
    let jobs: Vec<String> = (0..3).map(|_| failed_job(&app, roots)).collect();
    app.settle("op-defer", &jobs[1], json!({"kind": "defer"}))
        .expect("defer");
    app.stop_runtime();
    // These Jobs ended under P7: P8's migration ran after them.
    app.storage
        .write(|tx| {
            tx.execute(
                "UPDATE schema_migrations SET applied_at = ?1 WHERE version = 9",
                [(chrono::Utc::now() + chrono::Duration::minutes(1))
                    .to_rfc3339_opts(chrono::SecondsFormat::Millis, true)],
            )?;
            Ok(())
        })
        .unwrap();
    jobs
}

fn open_storage(roots: &Roots) -> farm3d_lib::persistence::Storage {
    farm3d_lib::persistence::Storage::open(roots.paths.clone(), &roots.lease).unwrap()
}

fn count(storage: &farm3d_lib::persistence::Storage, sql: &str) -> i64 {
    storage
        .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
        .unwrap()
}

fn requirement_and_history(app: &Running, job_id: &str) -> (Value, Value) {
    let history = app.history(job_id);
    (history["requirements"].clone(), history["events"].clone())
}

#[test]
fn backfill_projects_every_requirement_once_and_resolving_one_keeps_both_histories() {
    let roots = Roots::new(StartSafety::ConfirmBedClear);
    let jobs = seed_p7_state(&roots);

    // --- the first backfill: three Events, the deferred one acknowledged.
    let storage = open_storage(&roots);
    let changes = backfill(&storage, chrono::Utc::now()).expect("backfill");
    assert!(changes.notify.is_empty(), "a backfill never notifies");
    assert!(changes.capture.is_empty(), "a backfill never captures");
    assert_eq!(changes.events.len(), 3, "{:?}", changes.events);
    for applied in &changes.events {
        assert_eq!(applied.change, EventChange::Inserted { recurred: false });
        assert_eq!(applied.event.origin, AttentionOrigin::Backfill);
        assert_eq!(
            applied.event.condition,
            ConditionKind::RequirementMaterialReconciliation
        );
        assert_eq!(applied.event.printer_id.as_deref(), Some(p7_dispatch_rig::PRINTER));
        let deferred = applied.event.job_id.as_deref() == Some(jobs[1].as_str());
        assert_eq!(
            applied.event.acknowledged_at.is_some(),
            deferred,
            "only the deferred requirement's Event is acknowledged"
        );
    }
    assert_eq!(
        count(&storage, "SELECT COUNT(*) FROM attention_events WHERE resolved_at IS NULL"),
        3
    );

    // --- five more backfills change nothing.
    for _ in 0..5 {
        let again = backfill(&storage, chrono::Utc::now()).expect("backfill");
        assert!(again.events.is_empty(), "only unchanged amendments: {:?}", again.events);
        assert!(again.notify.is_empty());
    }
    assert_eq!(count(&storage, "SELECT COUNT(*) FROM attention_events"), 3);
    drop(storage);

    // --- two rebuilds of the runtime over the same database.
    for _ in 0..2 {
        let (app, backfilled) = boot_attention(&roots);
        assert!(backfilled.events.is_empty(), "{:?}", backfilled.events);
        app.attention_pass();
        drop(app);
    }
    let storage = open_storage(&roots);
    assert_eq!(count(&storage, "SELECT COUNT(*) FROM attention_events"), 3);
    assert_eq!(
        count(
            &storage,
            "SELECT COUNT(*) FROM (SELECT dedup_key FROM attention_events GROUP BY dedup_key HAVING COUNT(*) > 1)"
        ),
        0,
        "no duplicate rows"
    );
    drop(storage);

    // --- settling one requirement resolves exactly its Event.
    let (app, _) = boot_attention(&roots);
    app.attention_pass();
    let settled = app
        .settle("op-settle", &jobs[0], json!({"kind": "estimated"}))
        .expect("settle");
    assert_eq!(settled["requirements"][0]["status"], "resolved");
    let before = requirement_and_history(&app, &jobs[0]);

    app.wait_until("the settled requirement's Event resolves", || {
        app.attention_rows()
            .iter()
            .any(|event| event.job_id.as_deref() == Some(jobs[0].as_str()) && event.resolved_at.is_some())
    });
    app.attention_pass();
    let rows = app.attention_rows();
    assert_eq!(rows.len(), 3, "resolving never adds or removes a row");
    for event in &rows {
        if event.job_id.as_deref() == Some(jobs[0].as_str()) {
            assert_eq!(event.resolution, Some(AttentionResolution::ActionCompleted));
            assert!(event.read_at.is_some(), "a system resolution also reads");
        } else {
            assert_eq!(event.resolved_at, None, "{event:?}");
        }
    }
    assert_eq!(
        requirement_and_history(&app, &jobs[0]),
        before,
        "the projector never touches the requirement or the Job's own timeline"
    );
    assert_eq!(
        app.attention_stream("attention.event.changed", &rows_for(&rows, &jobs[0])).len(),
        1,
        "the resolution was published once"
    );
}

fn rows_for(rows: &[farm3d_lib::attention::AttentionEvent], job_id: &str) -> String {
    rows.iter()
        .find(|event| event.job_id.as_deref() == Some(job_id))
        .map(|event| event.id.clone())
        .unwrap()
}

/// A backfill with no Printer status never opens or resolves a
/// `printer.*` Event: an open `printer.offline` survives a restart.
#[test]
fn backfill_leaves_printer_events_alone() {
    let roots = Roots::new(StartSafety::Unattended);
    let clock = p7_dispatch_rig::ManualClock::new();
    let (app, _) = boot_with_attention(
        &roots,
        status_of(OperationalState::Ready),
        quiet(),
        AttentionBoot {
            clock: Some(clock.clone()),
            ..attention()
        },
    );
    app.attention_pass();
    app.seed(status_of(OperationalState::Offline));
    app.attention_pass();
    clock.advance(Duration::from_secs(6 * 60));
    app.services.attention.poke();
    app.attention_pass();
    let offline: Vec<_> = app
        .attention_rows()
        .into_iter()
        .filter(|event| event.condition == ConditionKind::PrinterOffline)
        .collect();
    assert_eq!(offline.len(), 1);
    drop(app);

    let storage = open_storage(&roots);
    let changes = backfill(&storage, chrono::Utc::now()).expect("backfill");
    assert!(changes.events.is_empty(), "{:?}", changes.events);
    assert_eq!(
        count(
            &storage,
            "SELECT COUNT(*) FROM attention_events WHERE condition = 'printer.offline' AND resolved_at IS NULL"
        ),
        1
    );
}
