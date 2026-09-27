//! P7 spec D4 "Restart matrix", Task 8a's rows (R1–R8, R15–R17, R19):
//! each runs to its crash point — a P6 fault injection, a stopped
//! dispatch driver, or a transaction that never commits — then rebuilds
//! `RuntimeServices` over the same roots (`p6_tracer.rs`'s pattern: host-ops
//! recovery, then `jobs::recover_after_restart`, then the runtimes) and
//! asserts the Job, Queue Entry, reservation, and Host Operation states,
//! plus FakeMoonraker's request counts: no duplicate upload and no
//! unconfirmed or second start.
//!
//! The tracker rows (R9–R14, R18, R20–R22) belong to Tasks 8b and 9.

mod common;
mod p7_dispatch_rig;

use std::sync::mpsc::Receiver;
use std::time::Duration;

use common::fake_moonraker::{Fault, Route, StartTrace};
use farm3d_lib::host_ops::{FaultAction, FaultPoint, HostOperationKind, HostOperationState};
use farm3d_lib::jobs::assign::{self, AssignRequest};
use farm3d_lib::jobs::AssignedBy;
use farm3d_lib::persistence::{RepositoryError, StorageError};
use farm3d_lib::printers::now_rfc3339;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;
use farm3d_lib::queue::eligibility::AssignMode;
use farm3d_lib::queue::world::LiveWorld;
use p7_dispatch_rig::{boot, boot_with, connecting, id, Driver, Roots, Running, SLR};
use serde_json::{json, Value};

fn roots() -> Roots {
    Roots::new(StartSafety::ConfirmBedClear)
}

fn wait_fired(fired: Receiver<()>) {
    fired
        .recv_timeout(Duration::from_secs(10))
        .expect("the fault point was reached");
    std::thread::sleep(Duration::from_millis(100));
}

/// Stops the old runtime's driver and drops everything, as a crash would.
fn crash(app: Running) {
    app.services.jobs.stop();
    std::thread::sleep(Duration::from_millis(100));
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

fn op_of_kind(app: &Running, job_id: &str, kind: HostOperationKind) -> farm3d_lib::host_ops::HostOperation {
    app.ops(job_id)
        .into_iter()
        .rfind(|op| op.kind == kind)
        .unwrap_or_else(|| panic!("a {kind:?} op for {job_id}"))
}

fn settle() {
    std::thread::sleep(Duration::from_millis(400));
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
        app.text(&format!("SELECT state FROM queue_entries WHERE id = '{entry}'")),
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
    settle();
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
    settle();
    assert_eq!(roots.uploads(), 1, "the driver staged once");
    crash(app);

    // Another restart never stages it again.
    let app = boot(&roots, Driver::Started);
    settle();
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
    assert_eq!(upload.state, HostOperationState::Failed, "P6: failed{{neverSent}}");
    let job = app.job(&job_id);
    assert_eq!(job["state"], "assigned", "Job recovery: StageFailed");
    assert_eq!(job["lastFailure"]["kind"], "hostOperationFailed");
    assert_eq!(job["lastFailure"]["failure"]["code"], "neverSent");
    assert_eq!(job["activeHostOperationId"], Value::Null);
    assert_eq!(entry_of(&app, &job_id)["state"], "assigned");
    assert_eq!(reservation_state(&app, &job_id), "active");
    settle();
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
            HostOperationState::Uncertain | HostOperationState::Reconciling | HostOperationState::Succeeded
        ),
        "{row:?}"
    );
    if app.job(&job_id)["state"] == "staging" && app.row(&upload.id).state == HostOperationState::Uncertain {
        let _ = app.call("reconcile_host_operation", json!({"hostOperationId": upload.id}));
    }
    let job = app.wait_job(&job_id, "awaitingStart");
    assert_eq!(job["uploadHostOperationId"], json!(upload.id));
    assert_eq!(app.row(&upload.id).state, HostOperationState::Succeeded);
    settle();
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
    settle();
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
    settle();
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
        let _ = app.call("reconcile_host_operation", json!({"hostOperationId": start.id}));
    }
    let job = app.wait_job(&job_id, "printing");
    assert!(job["startedAt"].is_string());
    assert_eq!(app.row(&start.id).state, HostOperationState::Succeeded);
    settle();
    assert_eq!(roots.starts(), 1, "no second start");
    assert_eq!(roots.uploads(), 1);
}

#[test]
fn restart_after_start_success_applies_the_outcome() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    app.services.jobs.stop();
    settle();
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
    let mark = app.scalar(&format!("SELECT history_mark FROM jobs WHERE id = '{job_id}'"));
    assert_eq!(Some(mark), start.history_mark, "the start op's history mark");
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
    let change = app.job_command("release_job", "op-release", &job_id).unwrap();
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
    let change = app.job_command("release_job", "op-release", &job_id).unwrap();
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
    settle();
    assert_eq!(app.scalar("SELECT COUNT(*) FROM jobs"), 1, "nothing assigned by itself");
    assert_eq!(roots.uploads(), 1);
    assert_eq!(roots.starts(), 0);
}

#[test]
fn restart_after_abandoned_start_is_outcome_unknown() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let job_id = app.awaiting_start();
    app.services.jobs.stop();
    settle();
    roots
        .fake
        .fault(Route::Start, Fault::ApplyStartThenDrop(StartTrace::PrintingOnly));
    app.start("op-start", &job_id, "ready").unwrap();
    let start = op_of_kind(&app, &job_id, HostOperationKind::Start);
    app.wait_op(&start.id, |row| row.state == HostOperationState::Uncertain);
    roots.fake.set_reachable(false);
    app.ok("reconcile_host_operation", json!({"hostOperationId": start.id}));
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
    settle();
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
    app.services.jobs.stop();
    settle();
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

    let app = boot_with(&roots, Driver::Started, connecting());
    app.wait_first_pass();
    settle();
    let job = app.job(&job_id);
    assert_eq!(job["state"], "assigned");
    assert_eq!(job["lastFailure"], Value::Null, "not reachable yet is a deferral");
    assert!(app.ops(&job_id).is_empty());

    // The status change stages it, well before the driver's 10 s poll.
    app.status(OperationalState::Ready);
    let job = app.wait_job_within(&job_id, "awaitingStart", Duration::from_secs(5));
    assert_eq!(job["lastFailure"], Value::Null);
    settle();
    app.status(OperationalState::Ready);
    settle();
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
    assert_eq!(app.job(&job_id)["startConfirmation"], "unattended", "the driver started it");
    crash(app);

    let app = boot(&roots, Driver::Started);
    assert_eq!(app.row(&start.id).state, HostOperationState::Failed);
    let job = app.wait_job(&job_id, "awaitingStart");
    assert_eq!(job["lastFailure"]["failure"]["code"], "neverSent");
    app.status(OperationalState::Ready);
    settle();
    app.status(OperationalState::Ready);
    settle();
    assert_eq!(app.job(&job_id)["state"], "awaitingStart");
    assert_eq!(roots.starts(), 0, "no start by itself after a failed one");
    assert_eq!(
        app.ops(&job_id).iter().filter(|op| op.kind == HostOperationKind::Start).count(),
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
        let _ = app.call("reconcile_host_operation", json!({"hostOperationId": start.id}));
    }
    app.wait_job(&job_id, "printing");
    app.status(OperationalState::Ready);
    settle();
    app.status(OperationalState::Ready);
    settle();
    assert_eq!(roots.starts(), 1, "only the original start");
    assert_eq!(
        app.ops(&job_id).iter().filter(|op| op.kind == HostOperationKind::Start).count(),
        1
    );
}
