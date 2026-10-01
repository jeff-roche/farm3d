//! P7 spec D4 "Restart matrix", Task 8a's rows (R1–R8, R15–R17, R19) and
//! Task 8b's tracker rows (R9–R11, R18, R20–R22):
//! each runs to its crash point — a P6 fault injection, a stopped
//! dispatch driver, or a transaction that never commits — then rebuilds
//! `RuntimeServices` over the same roots (`p6_tracer.rs`'s pattern: host-ops
//! recovery, then `jobs::recover_after_restart`, then the runtimes) and
//! asserts the Job, Queue Entry, reservation, and Host Operation states,
//! plus FakeMoonraker's request counts: no duplicate upload and no
//! unconfirmed or second start.
//!
//! Task 9 adds the material assertions to R10, R11, and R20, and rows
//! R12–R14.

mod common;
mod p7_dispatch_rig;

use std::sync::mpsc::Receiver;
use std::time::Duration;

use common::fake_moonraker::FakeMoonraker;
use common::fake_moonraker::{Fault, Route, StartTrace};
use farm3d_lib::host_ops::Clock;
use farm3d_lib::host_ops::{FaultAction, FaultPoint, HostOperationKind, HostOperationState};
use farm3d_lib::jobs::assign::{self, AssignRequest};
use farm3d_lib::jobs::settlement;
use farm3d_lib::jobs::{AssignedBy, SettleChoice};
use farm3d_lib::persistence::{RepositoryError, StorageError};
use farm3d_lib::printers::now_rfc3339;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StartSafety;
use farm3d_lib::queue::eligibility::AssignMode;
use farm3d_lib::queue::world::LiveWorld;
use p7_dispatch_rig::{
    boot, boot_tuned, connecting, fast, id, no_poll, status_from, status_of, Driver, ManualClock,
    Roots, Running, HOST_PATH, PRINTER, SECRET, SLR,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn roots() -> Roots {
    Roots::new(StartSafety::ConfirmBedClear)
}

/// Waits until the executor reached the injected fault point. It fires
/// just before the executor returns, and a `Crash` writes nothing more.
fn wait_fired(fired: Receiver<()>) {
    fired
        .recv_timeout(Duration::from_secs(10))
        .expect("the fault point was reached");
}

/// Stops the old runtime and drops everything, as a crash would. The
/// driver and the evaluator have finished before the next boot, so the
/// old runtime can't write while the new one runs.
fn crash(app: Running) {
    app.stop_runtime();
    drop(app);
}

fn crashed_rollback() -> RepositoryError {
    RepositoryError::Storage(StorageError::OperationFailed)
}

/// Adds one entry: its id.
fn add(app: &Running, operation_id: &str) -> String {
    id(&app.ok(
        "add_to_queue",
        json!({
            "operationId": operation_id,
            "sliceRevisionId": SLR,
            "quantity": 1,
            "policy": "recommended",
            "preference": "loadedFirst",
        }),
    )["entries"][0])
}

fn entry_of(app: &Running, job_id: &str) -> Value {
    app.history(job_id)["entry"].clone()
}

fn reservation_state(app: &Running, job_id: &str) -> String {
    app.text(&format!(
        "SELECT state FROM spool_reservations WHERE holder_kind = 'job' AND holder_id = '{job_id}'"
    ))
    .unwrap()
}

fn op_of_kind(
    app: &Running,
    job_id: &str,
    kind: HostOperationKind,
) -> farm3d_lib::host_ops::HostOperation {
    app.ops(job_id)
        .into_iter()
        .rfind(|op| op.kind == kind)
        .unwrap_or_else(|| panic!("a {kind:?} op for {job_id}"))
}

#[test]
fn restart_inside_assign_rolls_back_everything() {
    let roots = roots();
    let app = boot(&roots, Driver::Off);
    let spool = app.spool();
    let entry = add(&app, "op-add");
    // The assign transaction runs in full, then the process dies before
    // its commit.
    let request = AssignRequest {
        operation_id: "op-assign".to_string(),
        entry_id: entry.clone(),
        printer_id: p7_dispatch_rig::PRINTER.to_string(),
        spool_id: spool.clone(),
        mode: AssignMode::Operator {
            acknowledge_manual_facts: false,
        },
        assigned_by: AssignedBy::Operator,
    };
    let world = LiveWorld::of(&app.services);
    let result = app.storage.write_repo(|tx| {
        assign::assign(tx, &world, &request, &now_rfc3339())?;
        Err::<(), _>(crashed_rollback())
    });
    assert!(result.is_err());
    crash(app);

    let app = boot(&roots, Driver::Started);
    assert_eq!(app.scalar("SELECT COUNT(*) FROM jobs"), 0);
    assert_eq!(app.scalar("SELECT COUNT(*) FROM spool_reservations"), 0);
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM operations WHERE id = 'op-assign'"),
        0,
        "the id is not burned"
    );
    assert_eq!(
        app.text(&format!(
            "SELECT state FROM queue_entries WHERE id = '{entry}'"
        )),
        Some("queued".to_string())
    );
    // The same operationId assigns again, and the Job stages once.
    let change = app.ok(
        "assign_queue_entry",
        json!({"operationId": "op-assign", "entryId": entry,
               "printerId": p7_dispatch_rig::PRINTER, "spoolId": spool}),
    );
    let job_id = id(&change["jobs"][0]);
    app.wait_job(&job_id, "awaitingStart");
    app.quiesce();
    assert_eq!(roots.uploads(), 1);
    assert_eq!(roots.starts(), 0);
}

#[test]
fn restart_after_assign_stages_once() {
    let roots = roots();
    let app = boot(&roots, Driver::Off);
    let spool = app.spool();
    let job_id = app.assign(&spool);
    assert_eq!(app.job(&job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    assert!(app.ops(&job_id).is_empty(), "no upload op yet");
    crash(app);

    let app = boot(&roots, Driver::Started);
    let job = app.wait_job(&job_id, "awaitingStart");
    assert_eq!(job["lastFailure"], Value::Null);
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    app.quiesce();
    assert_eq!(roots.uploads(), 1, "the driver staged once");
    crash(app);

    // Another restart never stages it again.
    let app = boot(&roots, Driver::Started);
    app.quiesce();
    assert_eq!(app.job(&job_id)["state"], "awaitingStart");
    assert_eq!(roots.uploads(), 1);
    assert_eq!(roots.starts(), 0);
}

#[test]
fn restart_before_upload_send_returns_the_job_to_assigned() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Crash);
    let spool = app.spool();
    let job_id = app.assign(&spool);
    wait_fired(fired);
    assert_eq!(app.job(&job_id)["state"], "staging");
    let upload = app.first_op(&job_id);
    assert_eq!(upload.state, HostOperationState::Dispatching);
    assert!(upload.dispatched_at.is_none());
    crash(app);

    let app = boot(&roots, Driver::Started);
    let upload = app.row(&upload.id);
    assert_eq!(
        upload.state,
        HostOperationState::Failed,
        "P6: failed{{neverSent}}"
    );
    let job = app.job(&job_id);
    assert_eq!(job["state"], "assigned", "Job recovery: StageFailed");
    assert_eq!(job["lastFailure"]["kind"], "hostOperationFailed");
    assert_eq!(job["lastFailure"]["failure"]["code"], "neverSent");
    assert_eq!(job["activeHostOperationId"], Value::Null);
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    app.quiesce();
    assert_eq!(app.ops(&job_id).len(), 1, "nothing re-staged by itself");
    assert_eq!(roots.uploads(), 0);
}

/// P9 Task 7 (spec acceptance 7): the row above with a restored database.
/// The crashed Farm is backed up, the live Farm is replaced by an empty
/// one (a restore refuses over active work), and the backup is restored
/// through the journal and the startup installer. The restored active Job
/// then goes through P7 startup recovery exactly as after a restart.
#[test]
fn restored_database_before_upload_send_returns_the_job_to_assigned() {
    use farm3d_lib::backup::installer::{self, InstallOutcome};
    use farm3d_lib::backup::lease::{BackupLease, LeaseActivity};
    use farm3d_lib::backup::staging::{self, StagingOptions};
    use farm3d_lib::backup::writer::{write_backup, BackupRequest, WriterHooks};
    use farm3d_lib::backup::{apply, BackupMediaChoice, BackupOrigin};
    use farm3d_lib::persistence::Storage;

    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Crash);
    let spool = app.spool();
    let job_id = app.assign(&spool);
    wait_fired(fired);
    assert_eq!(app.job(&job_id)["state"], "staging");
    let upload = app.first_op(&job_id);
    assert_eq!(upload.state, HostOperationState::Dispatching);
    crash(app);

    // A backup of the crashed Farm.
    let exports = tempfile::tempdir().unwrap();
    let archive = exports.path().join("crashed.farm3d-backup");
    {
        let storage = Storage::open(roots.paths.clone(), &roots.lease).unwrap();
        let lease = BackupLease::new();
        let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
        write_backup(
            &storage,
            &guard,
            &archive,
            &BackupRequest {
                media: BackupMediaChoice::None,
                origin: BackupOrigin::Operator,
                created_at: None,
                app_version: "0.1.0".to_string(),
            },
            &WriterHooks::default(),
        )
        .unwrap();
    }
    // An empty live Farm, then the restore: staged, journaled, installed.
    for name in ["farm3d.sqlite3", "farm3d.sqlite3-wal", "farm3d.sqlite3-shm"] {
        let _ = std::fs::remove_file(roots.paths.metadata_root().join(name));
    }
    {
        let storage = Storage::open(roots.paths.clone(), &roots.lease).unwrap();
        let lease = BackupLease::new();
        let guard = lease.try_acquire(LeaseActivity::RestorePreview).unwrap();
        let candidate =
            staging::stage(&roots.paths, &guard, &archive, &StagingOptions::default()).unwrap();
        drop(guard);
        apply::write_pending_journal(&storage, &candidate, "sfb-p7", chrono::Utc::now()).unwrap();
    }
    let report = installer::run(&roots.paths, &roots.lease, || {
        panic!("a restore never opens the credential store")
    })
    .unwrap();
    assert_eq!(report.outcome, InstallOutcome::Installed);

    // The same outcome as `restart_before_upload_send_returns_the_job_to_assigned`.
    let app = boot(&roots, Driver::Started);
    let upload = app.row(&upload.id);
    assert_eq!(
        upload.state,
        HostOperationState::Failed,
        "P6: failed{{neverSent}}"
    );
    let job = app.job(&job_id);
    assert_eq!(job["state"], "assigned", "Job recovery: StageFailed");
    assert_eq!(job["lastFailure"]["kind"], "hostOperationFailed");
    assert_eq!(job["lastFailure"]["failure"]["code"], "neverSent");
    assert_eq!(job["activeHostOperationId"], Value::Null);
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    app.quiesce();
    assert_eq!(app.ops(&job_id).len(), 1, "nothing re-staged by itself");
    assert_eq!(roots.uploads(), 0);
}

#[test]
fn restart_during_upload_reconciles_then_awaits_start() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::AfterSend, FaultAction::Crash);
    let spool = app.spool();
    let job_id = app.assign(&spool);
    wait_fired(fired);
    let upload = app.first_op(&job_id);
    assert!(upload.dispatched_at.is_some(), "sent");
    assert_eq!(app.job(&job_id)["state"], "staging");
    crash(app);

    let app = boot(&roots, Driver::Started);
    let row = app.row(&upload.id);
    assert!(
        matches!(
            row.state,
            HostOperationState::Uncertain
                | HostOperationState::Reconciling
                | HostOperationState::Succeeded
        ),
        "{row:?}"
    );
    if app.job(&job_id)["state"] == "staging"
        && app.row(&upload.id).state == HostOperationState::Uncertain
    {
        let _ = app.call(
            "reconcile_host_operation",
            json!({"hostOperationId": upload.id}),
        );
    }
    let job = app.wait_job(&job_id, "awaitingStart");
    assert_eq!(job["uploadHostOperationId"], json!(upload.id));
    assert_eq!(app.row(&upload.id).state, HostOperationState::Succeeded);
    app.quiesce();
    assert_eq!(roots.uploads(), 1, "no duplicate upload");
    assert_eq!(roots.starts(), 0);
}

#[test]
fn restart_after_upload_success_applies_the_outcome() {
    let roots = roots();
    let app = boot(&roots, Driver::Off);
    let spool = app.spool();
    let job_id = app.assign(&spool);
    // The driver is down: the operator stages, and nothing applies the
    // upload's outcome before the crash.
    app.job_command("stage_job", "op-stage", &job_id).unwrap();
    let upload = app.wait_resolved(&app.first_op(&job_id).id, HostOperationState::Succeeded);
    app.quiesce();
    assert_eq!(app.job(&job_id)["state"], "staging");
    crash(app);

    // Recovery alone applies it (the driver stays off).
    let app = boot(&roots, Driver::Off);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "awaitingStart");
    assert_eq!(job["uploadHostOperationId"], json!(upload.id));
    assert_eq!(job["activeHostOperationId"], Value::Null);
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    assert_eq!(roots.uploads(), 1);
    assert_eq!(roots.starts(), 0);
}

#[test]
fn restart_before_start_send_returns_to_awaiting_start() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Crash);
    app.start("op-start", &job_id, "ready").unwrap();
    wait_fired(fired);
    assert_eq!(app.job(&job_id)["state"], "starting");
    let start = op_of_kind(&app, &job_id, HostOperationKind::Start);
    crash(app);

    let app = boot(&roots, Driver::Started);
    assert_eq!(app.row(&start.id).state, HostOperationState::Failed);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "awaitingStart", "StartFailed");
    assert_eq!(job["lastFailure"]["failure"]["code"], "neverSent");
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    app.quiesce();
    assert_eq!(roots.starts(), 0, "no start sent");
    assert_eq!(roots.uploads(), 1);
}

#[test]
fn restart_after_start_reply_lost_reconciles_without_a_second_start() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::AfterSend, FaultAction::Crash);
    app.start("op-start", &job_id, "ready").unwrap();
    wait_fired(fired);
    let start = op_of_kind(&app, &job_id, HostOperationKind::Start);
    assert!(app.row(&start.id).dispatched_at.is_some(), "sent");
    assert_eq!(app.job(&job_id)["state"], "starting");
    crash(app);

    let app = boot(&roots, Driver::Started);
    app.status(OperationalState::Printing);
    if app.row(&start.id).state == HostOperationState::Uncertain {
        let _ = app.call(
            "reconcile_host_operation",
            json!({"hostOperationId": start.id}),
        );
    }
    let job = app.wait_job(&job_id, "printing");
    assert!(job["startedAt"].is_string());
    assert_eq!(app.row(&start.id).state, HostOperationState::Succeeded);
    app.quiesce();
    assert_eq!(roots.starts(), 1, "no second start");
    assert_eq!(roots.uploads(), 1);
}

#[test]
fn restart_after_start_success_applies_the_outcome() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    app.stop_runtime();
    app.start("op-start", &job_id, "ready").unwrap();
    let start = app.wait_resolved(
        &op_of_kind(&app, &job_id, HostOperationKind::Start).id,
        HostOperationState::Succeeded,
    );
    assert_eq!(app.job(&job_id)["state"], "starting", "not yet applied");
    crash(app);

    let app = boot(&roots, Driver::Off);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "printing");
    assert_eq!(
        job["startedAt"],
        json!(start.dispatched_at.clone().expect("the start was sent")),
        "ruling R13(b): startedAt is when the start was sent, not when it was applied"
    );
    assert_eq!(job["activeHostOperationId"], Value::Null);
    let mark = app.scalar(&format!(
        "SELECT history_mark FROM jobs WHERE id = '{job_id}'"
    ));
    assert_eq!(
        Some(mark),
        start.history_mark,
        "the start op's history mark"
    );
    assert_eq!(reservation_state(&app, &job_id), "active");
    assert_eq!(roots.starts(), 1);
    assert_eq!(roots.uploads(), 1);
}

#[test]
fn restart_inside_release_rolls_back() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    let position = entry_of(&app, &job_id)["position"].clone();
    let result = app.storage.write_repo(|tx| {
        assign::release(tx, "op-release", &job_id, &now_rfc3339())?;
        Err::<(), _>(crashed_rollback())
    });
    assert!(result.is_err());
    crash(app);

    let app = boot(&roots, Driver::Started);
    assert_eq!(app.job(&job_id)["state"], "awaitingStart");
    let entry = entry_of(&app, &job_id);
    assert_eq!(entry["state"], "assigned");
    assert_eq!(entry["position"], position);
    assert_eq!(reservation_state(&app, &job_id), "active");
    assert_eq!(app.scalar("SELECT COUNT(*) FROM queue_entries"), 1);
    // Release again succeeds, with the same operationId.
    let change = app
        .job_command("release_job", "op-release", &job_id)
        .unwrap();
    assert_eq!(change["jobs"][0]["cancelReason"], "releasedBeforeStart");
    assert_eq!(roots.uploads(), 1);
    assert_eq!(roots.starts(), 0);
}

#[test]
fn restart_after_release_keeps_the_replacement_in_place() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    let old_entry = entry_of(&app, &job_id);
    let change = app
        .job_command("release_job", "op-release", &job_id)
        .unwrap();
    let replacement = id(&change["entries"][1]);
    crash(app);

    let app = boot(&roots, Driver::Started);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "cancelled");
    assert_eq!(job["cancelReason"], "releasedBeforeStart");
    assert_eq!(job["settlement"], "notRequired");
    assert_eq!(reservation_state(&app, &job_id), "released");
    let entry = entry_of(&app, &job_id);
    assert_eq!(entry["state"], "closed");
    assert_eq!(entry["closeReason"], "released");
    assert_eq!(
        app.text(&format!(
            "SELECT state || ':' || position FROM queue_entries WHERE id = '{replacement}'"
        )),
        Some(format!("queued:{}", old_entry["position"]))
    );
    app.quiesce();
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM jobs"),
        1,
        "nothing assigned by itself"
    );
    assert_eq!(roots.uploads(), 1);
    assert_eq!(roots.starts(), 0);
}

#[test]
fn restart_after_abandoned_start_is_outcome_unknown() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    app.stop_runtime();
    roots.fake.fault(
        Route::Start,
        Fault::ApplyStartThenDrop(StartTrace::PrintingOnly),
    );
    app.start("op-start", &job_id, "ready").unwrap();
    let start = op_of_kind(&app, &job_id, HostOperationKind::Start);
    app.wait_op(&start.id, |row| row.state == HostOperationState::Uncertain);
    roots.fake.set_reachable(false);
    app.ok(
        "reconcile_host_operation",
        json!({"hostOperationId": start.id}),
    );
    app.wait_op(&start.id, |row| {
        row.state == HostOperationState::Uncertain && row.attempts >= 1
    });
    app.ok(
        "abandon_host_operation",
        json!({"operationId": "op-abandon", "hostOperationId": start.id,
               "acknowledgement": "hostStateUnknown"}),
    );
    assert_eq!(app.row(&start.id).state, HostOperationState::Abandoned);
    assert_eq!(app.job(&job_id)["state"], "starting", "not yet applied");
    crash(app);
    roots.fake.set_reachable(true);

    let app = boot(&roots, Driver::Off);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "outcomeUnknown");
    assert_eq!(job["settlement"], "open");
    assert_eq!(reservation_state(&app, &job_id), "active");
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(
        app.scalar(&format!(
            "SELECT COUNT(*) FROM reconciliation_requirements
             WHERE job_id = '{job_id}' AND kind = 'jobOutcomeUnknown' AND status = 'pending'"
        )),
        1
    );
    crash(app);
    // Recovery is idempotent: a second restart opens nothing more.
    let app = boot(&roots, Driver::Started);
    app.quiesce();
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM reconciliation_requirements"),
        1
    );
    assert_eq!(roots.starts(), 1, "no second start");
    assert_eq!(roots.uploads(), 1);
}

#[test]
fn restart_after_pause_success_applies_the_outcome() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.printing();
    app.stop_runtime();
    app.job_command("pause_job", "op-pause", &job_id).unwrap();
    let pause = app.wait_resolved(
        &op_of_kind(&app, &job_id, HostOperationKind::Pause).id,
        HostOperationState::Succeeded,
    );
    assert_eq!(app.job(&job_id)["state"], "printing", "not yet applied");
    crash(app);

    let app = boot(&roots, Driver::Off);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "paused");
    assert_eq!(job["activeHostOperationId"], Value::Null);
    assert_eq!(app.row(&pause.id).state, HostOperationState::Succeeded);
    assert_eq!(roots.count_requests("POST", "/printer/print/pause"), 1);
    assert_eq!(roots.starts(), 1);
    assert_eq!(roots.uploads(), 1);
}

/// R2 as production boots: `start_jobs_runtime` runs while the restored
/// Connection is still `Connecting`. "Not online yet" defers the stage (no
/// `lastFailure`); the Printer coming Online stages it, once.
#[test]
fn restart_after_assign_stages_once_when_the_printer_connects_late() {
    let roots = roots();
    let app = boot(&roots, Driver::Off);
    let spool = app.spool();
    let job_id = app.assign(&spool);
    crash(app);

    let app = boot_tuned(&roots, Driver::Started, connecting(), no_poll(), None);
    app.wait_first_pass();
    app.quiesce();
    let job = app.job(&job_id);
    assert_eq!(job["state"], "assigned");
    assert_eq!(
        job["lastFailure"],
        Value::Null,
        "not reachable yet is a deferral"
    );
    assert!(app.ops(&job_id).is_empty());

    // The status change stages it: the driver's poll never ran again.
    app.status(OperationalState::Ready);
    let job = app.wait_job(&job_id, "awaitingStart");
    assert_eq!(job["lastFailure"], Value::Null);
    assert_eq!(
        app.services.jobs.resyncs(),
        1,
        "no poll ran after the first pass"
    );
    app.quiesce();
    app.status(OperationalState::Ready);
    app.quiesce();
    assert_eq!(roots.uploads(), 1, "staged exactly once");
    assert_eq!(app.ops(&job_id).len(), 1);
    assert_eq!(roots.starts(), 0);
}

/// An Unattended Printer's awaitingStart Job, staged while the Printer
/// was `finished` (so nothing started it yet): the Job id.
fn awaiting_start_unattended(app: &Running) -> String {
    app.status(OperationalState::Finished);
    app.awaiting_start()
}

/// R6 on an Unattended Printer: the driver's own start dies before it is
/// sent. After the restart the Printer is Ready and fresh again, and the
/// driver never starts the Job by itself.
#[test]
fn restart_before_unattended_start_send_never_starts_again() {
    let roots = Roots::new(StartSafety::Unattended);
    let app = boot(&roots, Driver::Started);
    let job_id = awaiting_start_unattended(&app);
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::BeforeMarkSent, FaultAction::Crash);
    app.status(OperationalState::Ready);
    wait_fired(fired);
    let start = op_of_kind(&app, &job_id, HostOperationKind::Start);
    assert_eq!(
        app.job(&job_id)["startConfirmation"],
        "unattended",
        "the driver started it"
    );
    crash(app);

    let app = boot(&roots, Driver::Started);
    assert_eq!(app.row(&start.id).state, HostOperationState::Failed);
    let job = app.wait_job(&job_id, "awaitingStart");
    assert_eq!(job["lastFailure"]["failure"]["code"], "neverSent");
    app.status(OperationalState::Ready);
    app.quiesce();
    app.status(OperationalState::Ready);
    app.quiesce();
    assert_eq!(app.job(&job_id)["state"], "awaitingStart");
    assert_eq!(roots.starts(), 0, "no start by itself after a failed one");
    assert_eq!(
        app.ops(&job_id)
            .iter()
            .filter(|op| op.kind == HostOperationKind::Start)
            .count(),
        1
    );
}

/// R7 on an Unattended Printer: the driver's start was sent and its reply
/// lost. After the restart, reconciliation, and the Printer Ready and
/// fresh again, the original start is the only one.
#[test]
fn restart_after_unattended_start_reply_lost_never_starts_again() {
    let roots = Roots::new(StartSafety::Unattended);
    let app = boot(&roots, Driver::Started);
    let job_id = awaiting_start_unattended(&app);
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::AfterSend, FaultAction::Crash);
    app.status(OperationalState::Ready);
    wait_fired(fired);
    let start = op_of_kind(&app, &job_id, HostOperationKind::Start);
    assert!(app.row(&start.id).dispatched_at.is_some(), "sent");
    crash(app);

    let app = boot(&roots, Driver::Started);
    app.status(OperationalState::Printing);
    if app.row(&start.id).state == HostOperationState::Uncertain {
        let _ = app.call(
            "reconcile_host_operation",
            json!({"hostOperationId": start.id}),
        );
    }
    app.wait_job(&job_id, "printing");
    app.status(OperationalState::Ready);
    app.quiesce();
    app.status(OperationalState::Ready);
    app.quiesce();
    assert_eq!(roots.starts(), 1, "only the original start");
    assert_eq!(
        app.ops(&job_id)
            .iter()
            .filter(|op| op.kind == HostOperationKind::Start)
            .count(),
        1
    );
}

// --- Task 8b: the tracker rows ------------------------------------------------------

fn fast_boot(roots: &Roots) -> Running {
    boot_tuned(
        roots,
        Driver::Started,
        status_of(OperationalState::Ready),
        fast(),
        None,
    )
}

fn requirements(app: &Running, job_id: &str) -> Vec<Value> {
    app.history(job_id)["requirements"]
        .as_array()
        .unwrap()
        .clone()
}

/// R9: a restart while printing. The Job stays printing, keeps its last
/// persisted progress, and the tracker's first pass pins it if it was
/// unpinned — once.
#[test]
fn restart_during_printing_keeps_tracking() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    // The start's outcome is applied only by the next boot's recovery, so
    // the Job restarts printing and unpinned.
    app.stop_runtime();
    app.start("op-start", &job_id, "ready").unwrap();
    app.wait_resolved(
        &op_of_kind(&app, &job_id, HostOperationKind::Start).id,
        HostOperationState::Succeeded,
    );
    crash(app);

    let app = boot(&roots, Driver::Off);
    assert_eq!(app.job(&job_id)["state"], "printing");
    assert_eq!(app.job_column(&job_id, "host_job_id"), None, "unpinned");
    crash(app);

    let app = fast_boot(&roots);
    app.wait_until("pinned", || app.count_events(&job_id, "hostJobPinned") == 1);
    roots.fake.set_progress(0.42);
    app.mirror(&roots.fake);
    app.wait_job_until(&job_id, |job| job["maxProgressPct"] == 42);
    let pin = app.job_column(&job_id, "host_job_id");
    assert!(pin.is_some());
    crash(app);

    let app = fast_boot(&roots);
    app.wait_first_pass();
    app.wait_passes(3);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "printing");
    assert_eq!(job["maxProgressPct"], 42, "the last persisted progress");
    assert_eq!(app.job_column(&job_id, "host_job_id"), pin);
    assert_eq!(app.count_events(&job_id, "hostJobPinned"), 1, "pinned once");
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    assert_eq!(roots.starts(), 1);
    assert_eq!(roots.uploads(), 1);
}

/// R10: the host completed while farm3d was closed. The first tracker
/// pass completes the Job once and closes its entry, consuming the
/// estimate once.
#[test]
fn restart_after_host_completed_completes_once() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    crash(app);
    roots.fake.finish_print("completed");

    let app = fast_boot(&roots);
    let job = app.wait_job(&job_id, "completed");
    assert_eq!(job["settlement"], "settled");
    assert_eq!(job["settlementMethod"], "estimated");
    assert_eq!(job["settlementPreview"], Value::Null);
    let entry = entry_of(&app, &job_id);
    assert_eq!(entry["state"], "closed");
    assert_eq!(entry["closeReason"], "completed");
    assert!(requirements(&app, &job_id).is_empty());
    assert_eq!(reservation_state(&app, &job_id), "consumed");
    crash(app);

    let app = fast_boot(&roots);
    app.wait_first_pass();
    app.quiesce();
    assert_eq!(app.job(&job_id)["state"], "completed");
    assert_eq!(app.count_events(&job_id, "completed"), 1, "completed once");
    assert_eq!(
        reservation_state(&app, &job_id),
        "consumed",
        "unchanged across restarts"
    );
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM spool_amount_events WHERE kind = 'consumption'"),
        1,
        "the estimate was consumed exactly once"
    );
    assert_eq!(roots.starts(), 1);
    assert_eq!(roots.uploads(), 1);
}

/// Every `materialReconciliation` requirement on `job_id`.
fn material_requirements(app: &Running, job_id: &str) -> Vec<Value> {
    requirements(app, job_id)
        .into_iter()
        .filter(|requirement| requirement["kind"] == "materialReconciliation")
        .collect()
}

/// R11: the host cancelled or failed while farm3d was closed. The
/// reservation goes `unresolved` and one `materialReconciliation`
/// requirement opens, `pending` -- once, even across a second restart.
#[test]
fn restart_after_host_failed_opens_one_requirement() {
    for (status, state, cancel_reason) in [
        ("cancelled", "cancelled", json!("hostCancelled")),
        ("klippy_shutdown", "failed", Value::Null),
    ] {
        let roots = roots();
        let app = fast_boot(&roots);
        let job_id = app.printing();
        crash(app);
        roots.fake.finish_print(status);

        let app = fast_boot(&roots);
        let job = app.wait_job(&job_id, state);
        assert_eq!(job["cancelReason"], cancel_reason, "{status}");
        assert_eq!(job["settlement"], "pending", "{status}");
        let entry = entry_of(&app, &job_id);
        assert_eq!(entry["state"], "closed");
        assert_eq!(entry["closeReason"], state);
        assert!(
            requirements(&app, &job_id)
                .iter()
                .all(|requirement| requirement["kind"] != "jobOutcomeUnknown"),
            "{status}"
        );
        assert_eq!(reservation_state(&app, &job_id), "unresolved", "{status}");
        let material = material_requirements(&app, &job_id);
        assert_eq!(material.len(), 1, "{status}");
        assert_eq!(material[0]["status"], "pending", "{status}");
        assert_eq!(material[0]["resolution"], Value::Null, "{status}");
        crash(app);

        let app = fast_boot(&roots);
        app.wait_first_pass();
        app.quiesce();
        assert_eq!(app.job(&job_id)["state"], state);
        assert_eq!(app.count_events(&job_id, state), 1, "{status}: ended once");
        assert_eq!(
            reservation_state(&app, &job_id),
            "unresolved",
            "{status}: unchanged"
        );
        assert_eq!(
            material_requirements(&app, &job_id).len(),
            1,
            "{status}: opened once"
        );
        assert_eq!(roots.starts(), 1);
    }
}

/// R12: settlement work is inside the terminal transaction (D4), so
/// "after the terminal commit, before settlement" can't happen on disk --
/// a restart right after a failure sees the settlement already `pending`,
/// the reservation `unresolved`, and one requirement `pending`, and
/// changes none of it.
#[test]
fn restart_after_terminal_keeps_settlement_pending() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.finish_print("klippy_shutdown");
    let job = app.wait_job(&job_id, "failed");
    assert_eq!(job["settlement"], "pending");
    assert_eq!(reservation_state(&app, &job_id), "unresolved");
    assert_eq!(material_requirements(&app, &job_id).len(), 1);
    let failed_events = app.count_events(&job_id, "failed");
    let consumptions =
        app.scalar("SELECT COUNT(*) FROM spool_amount_events WHERE kind = 'consumption'");
    crash(app);

    let app = fast_boot(&roots);
    app.wait_first_pass();
    app.quiesce();
    let job = app.job(&job_id);
    assert_eq!(job["state"], "failed");
    assert_eq!(job["settlement"], "pending");
    assert_eq!(reservation_state(&app, &job_id), "unresolved");
    assert_eq!(
        material_requirements(&app, &job_id).len(),
        1,
        "not re-opened"
    );
    assert_eq!(
        app.count_events(&job_id, "failed"),
        failed_events,
        "not re-run"
    );
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM spool_amount_events WHERE kind = 'consumption'"),
        consumptions,
        "nothing re-consumed"
    );
}

/// R13: a crash inside a settle transaction, before commit -- exactly
/// R1's pattern, for `jobs::settlement::settle`. Nothing is written, the
/// id isn't burned, and the same `operationId` settles once afterward.
#[test]
fn restart_inside_settle_rolls_back() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&job_id, "failed");

    let result = app.storage.write_repo(|tx| {
        settlement::settle(
            tx,
            "op-settle",
            &job_id,
            &SettleChoice::Estimated,
            &now_rfc3339(),
        )?;
        Err::<(), _>(crashed_rollback())
    });
    assert!(result.is_err());
    assert_eq!(app.job(&job_id)["settlement"], "pending", "unchanged");
    assert_eq!(reservation_state(&app, &job_id), "unresolved", "unchanged");
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM operations WHERE id = 'op-settle'"),
        0,
        "the id is not burned"
    );
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM spool_amount_events WHERE kind = 'consumption'"),
        0,
        "no ledger row"
    );
    crash(app);

    let app = fast_boot(&roots);
    app.wait_first_pass();
    app.quiesce();
    let settled = app
        .settle("op-settle", &job_id, json!({"kind": "estimated"}))
        .unwrap();
    assert_eq!(settled["jobs"][0]["settlement"], "settled");
    assert_eq!(reservation_state(&app, &job_id), "consumed");
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM spool_amount_events WHERE kind = 'consumption'"),
        1,
        "settled exactly once"
    );
}

/// R14: after a defer, a restart keeps the settlement `deferred`, the
/// requirement `deferred`, and the reservation `unresolved` -- the amount
/// stays unavailable.
#[test]
fn restart_after_defer_keeps_the_amount_unavailable() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.finish_print("cancelled");
    app.wait_job(&job_id, "cancelled");
    app.settle("op-defer", &job_id, json!({"kind": "defer"}))
        .unwrap();
    assert_eq!(reservation_state(&app, &job_id), "unresolved");
    crash(app);

    let app = fast_boot(&roots);
    app.wait_first_pass();
    app.quiesce();
    let job = app.job(&job_id);
    assert_eq!(job["state"], "cancelled");
    assert_eq!(job["settlement"], "deferred");
    assert_eq!(
        reservation_state(&app, &job_id),
        "unresolved",
        "the amount stays unavailable"
    );
    let material = material_requirements(&app, &job_id);
    assert_eq!(material.len(), 1);
    assert_eq!(material[0]["status"], "deferred");
}

/// R18: the cancel was sent and its reply lost. After the restart the
/// cancel op is uncertain; the tracker proves `cancelled{cancelledByOperator}`
/// from history, once; the op's own resolution later only clears the
/// Job's active op.
#[test]
fn restart_after_cancel_reply_lost_ends_cancelled_once() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    // The crash point: the cancel was sent (the host applied it) and
    // nothing after the send ran, the tracker included.
    app.stop_runtime();
    let fired = app
        .services
        .host_ops
        .inject_fault(FaultPoint::AfterSend, FaultAction::Crash);
    app.job_command("cancel_job", "op-cancel", &job_id).unwrap();
    wait_fired(fired);
    let cancel = op_of_kind(&app, &job_id, HostOperationKind::Cancel);
    assert_eq!(app.job(&job_id)["state"], "printing");
    crash(app);

    let app = fast_boot(&roots);
    let job = app.wait_job(&job_id, "cancelled");
    assert_eq!(job["cancelReason"], "cancelledByOperator");
    assert_eq!(entry_of(&app, &job_id)["closeReason"], "cancelled");
    if app.row(&cancel.id).state == HostOperationState::Uncertain {
        let _ = app.call(
            "reconcile_host_operation",
            json!({"hostOperationId": cancel.id}),
        );
    }
    app.wait_resolved(&cancel.id, HostOperationState::Succeeded);
    let job = app.wait_job_until(&job_id, |job| job["activeHostOperationId"].is_null());
    assert_eq!(job["state"], "cancelled");
    assert_eq!(app.count_events(&job_id, "cancelled"), 1);
    assert_eq!(app.count_events(&job_id, "controlFailed"), 0);
    assert_eq!(roots.count_requests("POST", "/printer/print/cancel"), 1);
    assert_eq!(roots.starts(), 1);
}

/// R20: after a declare committed, a restart changes nothing.
#[test]
fn restart_after_declare_is_stable() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.with_state(|state| state.history.clear());
    app.wait_job(&job_id, "outcomeUnknown");
    app.declare("op-declare", &job_id, "failed").unwrap();
    crash(app);

    let app = fast_boot(&roots);
    app.wait_first_pass();
    app.quiesce();
    let job = app.job(&job_id);
    assert_eq!(job["state"], "failed");
    assert_eq!(job["settlement"], "pending");
    assert_eq!(job["cancelReason"], Value::Null);
    let entry = entry_of(&app, &job_id);
    assert_eq!(entry["state"], "closed");
    assert_eq!(entry["closeReason"], "failed");
    let outcome_unknown: Vec<Value> = requirements(&app, &job_id)
        .into_iter()
        .filter(|requirement| requirement["kind"] == "jobOutcomeUnknown")
        .collect();
    assert_eq!(outcome_unknown.len(), 1);
    assert_eq!(outcome_unknown[0]["status"], "resolved");
    assert_eq!(
        outcome_unknown[0]["resolution"],
        json!({"kind": "declared", "outcome": "failed"})
    );
    // A declared failure runs the same terminal settlement work as a
    // tracker-proved one: the reservation is unresolved, and a new
    // `materialReconciliation` requirement is `pending`.
    assert_eq!(reservation_state(&app, &job_id), "unresolved");
    let material = material_requirements(&app, &job_id);
    assert_eq!(material.len(), 1);
    assert_eq!(material[0]["status"], "pending");
    assert_eq!(app.count_events(&job_id, "declaredFailed"), 1);
    // The same operationId replays; nothing moves.
    let replayed = app.declare("op-declare", &job_id, "failed").unwrap();
    assert_eq!(replayed["jobs"][0]["state"], "failed");
    assert_eq!(app.count_events(&job_id, "declaredFailed"), 1);
    assert_eq!(roots.starts(), 1);
}

/// R21: while history polls can't run (the host is gone, though status
/// still streams), `host_unreachable_since` is set, kept across a restart,
/// and never cleared by status; 30 minutes after it, a declare from
/// `printing` is allowed. Only a successful history poll clears it.
#[test]
fn restart_keeps_host_unreachable_since_and_allows_declare_after_30_minutes() {
    let roots = roots();
    let clock = ManualClock::new();
    let clocked = |roots: &Roots| {
        boot_tuned(
            roots,
            Driver::Started,
            status_of(OperationalState::Ready),
            fast(),
            Some(Arc::clone(&clock) as Arc<dyn Clock>),
        )
    };
    let app = clocked(&roots);
    let job_id = app.printing();
    roots.fake.set_reachable(false);
    let since = app.wait_job_until(&job_id, |job| job["hostUnreachableSince"].is_string())
        ["hostUnreachableSince"]
        .clone();
    crash(app);

    clock.advance(Duration::from_secs(10 * 60));
    let mut streaming = status_from(&roots.fake);
    streaming.telemetry.job_name = Some(HOST_PATH.to_string());
    let app = boot_tuned(
        &roots,
        Driver::Started,
        streaming,
        fast(),
        Some(Arc::clone(&clock) as Arc<dyn Clock>),
    );
    app.wait_first_pass();
    app.wait_passes(2);
    let job = app.job(&job_id);
    assert_eq!(job["state"], "printing");
    assert_eq!(
        job["hostUnreachableSince"], since,
        "kept across the restart"
    );
    assert!(!job["allowedActions"]
        .as_array()
        .unwrap()
        .contains(&json!("declareOutcome")));

    clock.advance(Duration::from_secs(20 * 60));
    app.wait_job_until(&job_id, |job| {
        job["allowedActions"]
            .as_array()
            .unwrap()
            .contains(&json!("declareOutcome"))
    });
    crash(app);

    // Still allowed after another restart, then declared from printing.
    let app = clocked(&roots);
    app.wait_first_pass();
    let declared = app.declare("op-declare", &job_id, "cancelled").unwrap();
    assert_eq!(declared["jobs"][0]["state"], "cancelled");
    assert_eq!(declared["jobs"][0]["cancelReason"], "operatorDeclared");
    assert_eq!(declared["jobs"][0]["hostUnreachableSince"], Value::Null);
    crash(app);
    roots.fake.set_reachable(true);
    let app = clocked(&roots);
    app.wait_first_pass();
    assert_eq!(app.job(&job_id)["state"], "cancelled");
    assert_eq!(roots.starts(), 1);
    assert_eq!(
        roots.count_requests("POST", "/printer/print/cancel"),
        0,
        "a declare sends nothing"
    );
}

/// R22: a Connection endpoint change during printing, then a restart. The
/// tracker reads the new endpoint, where a same-named job is never pinned;
/// every poll is inconclusive, so the Job becomes outcomeUnknown. Nothing
/// is written to either endpoint.
#[test]
fn endpoint_change_during_printing_never_pins_a_job_on_the_new_host() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    app.stop_runtime();
    app.start("op-start", &job_id, "ready").unwrap();
    app.wait_resolved(
        &op_of_kind(&app, &job_id, HostOperationKind::Start).id,
        HostOperationState::Succeeded,
    );
    let other = FakeMoonraker::start();
    other.with_state(|state| {
        state.api_key = Some(SECRET.to_string());
        state.next_job_id = 900;
        state.print_state = "printing".to_string();
        state.print_filename = HOST_PATH.to_string();
        state.push_job(HOST_PATH, "in_progress");
    });
    let printers = PrinterRepository::new(Arc::clone(&app.storage));
    let printer = printers.get(PRINTER).unwrap().unwrap();
    let mut config = other.config();
    config.credential_ref = printer.connection.as_ref().unwrap().credential_ref.clone();
    printers
        .set_connection(PRINTER, printer.revision, Some(config), None, "test")
        .unwrap();
    crash(app);
    let old_posts = roots
        .fake
        .requests()
        .iter()
        .filter(|r| r.method == "POST")
        .count();

    let app = boot_tuned(&roots, Driver::Started, status_from(&other), fast(), None);
    let job = app.wait_job(&job_id, "outcomeUnknown");
    assert_eq!(app.job_column(&job_id, "host_job_id"), None, "never pinned");
    assert_eq!(app.count_events(&job_id, "hostJobPinned"), 0);
    assert_eq!(job["hostUnreachableSince"], Value::Null);
    assert_eq!(
        requirements(&app, &job_id)
            .iter()
            .filter(|requirement| requirement["kind"] == "jobOutcomeUnknown")
            .count(),
        1
    );
    assert!(other
        .requests()
        .iter()
        .any(|request| request.path() == "/server/history/list"));
    assert_eq!(
        other
            .requests()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        0
    );
    assert_eq!(
        roots
            .fake
            .requests()
            .iter()
            .filter(|r| r.method == "POST")
            .count(),
        old_posts
    );
    assert_eq!(roots.starts(), 1);
}
