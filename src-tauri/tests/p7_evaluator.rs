//! P7 Task 10 (spec D4 "Recovery order", D5 F6/F7, D6): the serialized
//! automatic evaluator, against `FakeMoonraker` through `p7_dispatch_rig`.
//!
//! The evaluator is one tokio task behind a coalescing trigger channel. It
//! evaluates the open Queue top to bottom, assigns `automatic` entries
//! through the same `jobs::assign::assign` the operator's command uses,
//! publishes `queue.eligibility.changed`, and reports
//! `nextAutomaticAction`, which `list_queue` serves once it has run.
//!
//! Every wait here is on a condition with a deadline. Runs are counted by
//! the evaluator's own counters and hooks, never by timing.

mod common;
mod p7_dispatch_rig;

use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fake_moonraker::{Fault, Route};
use farm3d_lib::connections::capabilities::EvidenceTier;
use farm3d_lib::jobs::JobTimings;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;
use farm3d_lib::queue::evaluator::{Proposal, Trigger};
use p7_dispatch_rig::{
    boot, boot_prepared, fast, id, status_of, Driver, Roots, Running, PRINTER, SECRET, SLR, WAIT,
};
use serde_json::{json, Value};

fn roots() -> Roots {
    Roots::new(StartSafety::ConfirmBedClear)
}

/// Adds `quantity` copies of the rig's revision with `policy`: the entry
/// ids, in position order.
fn add(app: &Running, policy: &str, quantity: i64) -> Vec<String> {
    let mut body = json!({
        "operationId": format!("add-{}", uuid::Uuid::new_v4()),
        "sliceRevisionId": SLR,
        "quantity": quantity,
        "policy": policy,
        "preference": "loadedFirst",
    });
    if policy == "manual" {
        body["manualPrinterId"] = json!(PRINTER);
    }
    let change = app.ok("add_to_queue", body);
    change["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(id)
        .collect()
}

fn list(app: &Running) -> Value {
    app.ok("list_queue", json!({}))
}

fn next_action(app: &Running) -> Value {
    list(app)["nextAutomaticAction"].clone()
}

/// Waits until `list_queue`'s `nextAutomaticAction` satisfies `done`.
fn wait_next(app: &Running, what: &str, done: impl Fn(&Value) -> bool) -> Value {
    let deadline = Instant::now() + WAIT;
    loop {
        let next = next_action(app);
        if done(&next) {
            return next;
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting until {what}: {next}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn is_waiting(next: &Value, entry_id: &str, code: &str) -> bool {
    next["kind"] == "waiting" && next["entryId"] == entry_id && next["blocker"]["code"] == code
}

/// Waits until every accepted trigger has been consumed by a run that
/// finished, and no run is in progress.
fn wait_idle(app: &Running) {
    let deadline = Instant::now() + WAIT;
    while !app.services.evaluator.is_idle() {
        assert!(Instant::now() < deadline, "the evaluator never went idle");
        std::thread::sleep(Duration::from_millis(10));
    }
}

/// Waits for the evaluator's first finished run, then for idle.
fn wait_settled(app: &Running) {
    app.wait_until("the evaluator ran", || app.services.evaluator.runs() >= 1);
    wait_idle(app);
}

fn job_count(app: &Running) -> i64 {
    app.scalar("SELECT COUNT(*) FROM jobs")
}

/// Waits for the one Job, and returns it as `get_job_history` reports it.
fn wait_one_job(app: &Running) -> Value {
    app.wait_until("a Job exists", || job_count(app) >= 1);
    assert_eq!(job_count(app), 1, "exactly one Job");
    let job_id = app.text("SELECT id FROM jobs").unwrap();
    app.job(&job_id)
}

fn summary<'a>(snapshot: &'a Value, entry_id: &str) -> &'a Value {
    snapshot["eligibility"]
        .as_array()
        .unwrap()
        .iter()
        .find(|summary| summary["entryId"] == entry_id)
        .unwrap_or_else(|| panic!("a summary for {entry_id}: {snapshot}"))
}

fn entry_revision(app: &Running, entry_id: &str) -> i64 {
    app.scalar(&format!(
        "SELECT revision FROM queue_entries WHERE id = '{entry_id}'"
    ))
}

fn move_to(app: &Running, entry_id: &str, position: i64) {
    app.ok(
        "move_queue_entry",
        json!({
            "operationId": format!("move-{}", uuid::Uuid::new_v4()),
            "entryId": entry_id,
            "expectedRevision": entry_revision(app, entry_id),
            "toPosition": position,
        }),
    );
}

/// The captured `queue.eligibility.changed` payloads, in order.
fn eligibility_events(app: &Running) -> Vec<Value> {
    app.events
        .lock()
        .unwrap()
        .iter()
        .map(|text| serde_json::from_str::<Value>(text).unwrap())
        .filter(|event| event["type"] == "queue.eligibility.changed")
        .collect()
}

/// A one-shot gate: the first run that reaches it signals `entered` and
/// blocks until `release` is sent; later runs pass straight through.
fn gate() -> (
    Box<dyn FnMut() + Send>,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
) {
    let (entered_tx, entered) = mpsc::channel();
    let (release, release_rx) = mpsc::channel::<()>();
    let mut armed = true;
    let hook = Box::new(move || {
        if armed {
            armed = false;
            let _ = entered_tx.send(());
            let _ = release_rx.recv_timeout(WAIT);
        }
    });
    (hook, entered, release)
}

#[test]
fn evaluator_waits_for_startup_recovery_before_the_first_run() {
    let roots = roots();
    // Before the restart: an Automatic entry and its Spool, loaded. The
    // runtime never ran, so nothing was assigned.
    {
        let setup = boot(&roots, Driver::Off);
        let spool = setup.spool();
        setup.load(&spool);
        add(&setup, "automatic", 1);
        assert_eq!(job_count(&setup), 0);
    }
    // The host-ops startup pass reads host facts first: hold it.
    roots.fake.fault(
        Route::ServerInfo,
        Fault::DelayResponse(Duration::from_millis(700)),
    );

    let seen: Arc<Mutex<Vec<(u64, bool)>>> = Arc::default();
    let record = Arc::clone(&seen);
    let app = boot_prepared(
        &roots,
        Driver::Started,
        status_of(OperationalState::Ready),
        JobTimings::default(),
        None,
        move |services| {
            let weak = Arc::downgrade(services);
            services.evaluator.on_run_start(Some(Box::new(move || {
                if let Some(services) = weak.upgrade() {
                    record.lock().unwrap().push((
                        services.jobs.resyncs(),
                        services.host_ops.startup_pass_done(),
                    ));
                }
            })));
        },
    );

    let job = wait_one_job(&app);
    assert_eq!(job["assignedBy"], "automatic");
    let first = seen.lock().unwrap()[0];
    assert!(
        first.0 >= 1,
        "the driver's first pass ran before the first run"
    );
    assert!(
        first.1,
        "the host-ops startup pass finished before the first run"
    );
    app.services.evaluator.on_run_start(None);
}

#[test]
fn top_to_bottom_the_first_automatic_entry_claims_the_printer() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    let entries = add(&app, "automatic", 2);
    let (e1, e2) = (entries[0].clone(), entries[1].clone());
    wait_next(&app, "E1 waits for its Spool", |next| {
        is_waiting(next, &e1, "SPOOL_NOT_LOADED")
    });
    // Position, not creation, decides: E2 moves to the top.
    move_to(&app, &e2, 1);
    wait_next(&app, "E2 is first", |next| {
        is_waiting(next, &e2, "SPOOL_NOT_LOADED")
    });

    app.load(&spool);

    let job = wait_one_job(&app);
    assert_eq!(job["queueEntryId"], json!(e2));
    assert_eq!(job["printerId"], PRINTER);
    assert_eq!(job["spoolId"], json!(spool));
    assert_eq!(job["assignedBy"], "automatic");
    // F6: E1 is not offered the claimed Printer.
    let next = wait_next(&app, "E1 waits on the Job", |next| {
        is_waiting(next, &e1, "JOB_ACTIVE")
    });
    assert_eq!(next["blocker"]["printerIds"], json!([PRINTER]));
    wait_idle(&app);
    let snapshot = list(&app);
    let e1_summary = summary(&snapshot, &e1);
    assert_eq!(e1_summary["verdict"], "blocked");
    assert_eq!(e1_summary["eligibleCount"], 0);
    assert_eq!(e1_summary["topBlocker"]["code"], "JOB_ACTIVE");
    assert_eq!(e1_summary["topBlocker"]["printerIds"], json!([PRINTER]));
    assert_eq!(job_count(&app), 1);
    // D7: the driver stages the evaluator's Job, once.
    app.wait_job(&id(&job), "awaitingStart");
    wait_idle(&app);
    assert_eq!(roots.uploads(), 1, "staged exactly once");
    // D4: the evaluator's own ledger id, under the assign kind.
    assert_eq!(
        app.scalar(
            "SELECT COUNT(*) FROM operations WHERE id LIKE 'auto-%' AND kind = 'assignQueueEntry'"
        ),
        1
    );
    // The evaluator's payloads carry no credential.
    assert!(!eligibility_events(&app).is_empty());
    assert!(app
        .events
        .lock()
        .unwrap()
        .iter()
        .all(|event| !event.contains(SECRET)));
}

#[test]
fn manual_and_recommended_entries_are_explained_but_never_assigned() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    wait_settled(&app);
    let spool = app.spool();
    app.load(&spool);
    let manual = add(&app, "manual", 1).remove(0);
    let recommended = add(&app, "recommended", 1).remove(0);
    wait_idle(&app);

    assert_eq!(job_count(&app), 0, "nothing assigned");
    let next = next_action(&app);
    assert_eq!(next["kind"], "noAutomaticEntries", "{next}");
    let last = eligibility_events(&app)
        .pop()
        .expect("an eligibility event");
    let summaries = last["payload"]["summaries"].as_array().unwrap();
    for entry_id in [&manual, &recommended] {
        let summary = summaries
            .iter()
            .find(|summary| summary["entryId"] == **entry_id)
            .unwrap_or_else(|| panic!("the evaluator explained {entry_id}: {last}"));
        assert_eq!(summary["verdict"], "awaitingOperator");
        assert_eq!(summary["candidatePrinterIds"], json!([PRINTER]));
        let explained = app.ok("explain_queue_entry", json!({"entryId": entry_id}));
        assert_eq!(explained["candidates"][0]["printerId"], PRINTER);
    }
}

#[test]
fn assignment_is_sticky_when_the_queue_is_reordered() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    app.load(&spool);
    let e1 = add(&app, "automatic", 1).remove(0);
    let job = wait_one_job(&app);
    let job_id = id(&job);
    let e2 = add(&app, "automatic", 1).remove(0);
    wait_next(&app, "E2 waits on the Job", |next| {
        is_waiting(next, &e2, "JOB_ACTIVE")
    });

    move_to(&app, &e2, 1);
    wait_idle(&app);

    assert_eq!(job_count(&app), 1, "reordering never re-assigns");
    let history = app.history(&job_id);
    assert_eq!(history["entry"]["id"], json!(e1));
    assert_eq!(history["entry"]["state"], "assigned");
    assert_eq!(history["entry"]["jobId"], json!(job_id));
    assert_ne!(history["job"]["state"], "cancelled");
    assert!(is_waiting(&next_action(&app), &e2, "JOB_ACTIVE"));
}

#[test]
fn a_spool_load_triggers_assignment() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    let entry = add(&app, "automatic", 1).remove(0);
    wait_next(&app, "the entry waits for its Spool", |next| {
        is_waiting(next, &entry, "SPOOL_NOT_LOADED")
    });
    assert_eq!(job_count(&app), 0);

    app.load(&spool);

    let job = wait_one_job(&app);
    assert_eq!(job["queueEntryId"], json!(entry));
    assert_eq!(job["spoolId"], json!(spool));
}

#[test]
fn a_printer_becoming_ready_triggers_assignment() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    app.load(&spool);
    app.status(OperationalState::Printing);
    let entry = add(&app, "automatic", 1).remove(0);
    wait_next(&app, "the entry waits for the Printer", |next| {
        is_waiting(next, &entry, "PRINTER_BUSY_EXTERNAL")
    });
    assert_eq!(job_count(&app), 0);

    app.status(OperationalState::Ready);

    let job = wait_one_job(&app);
    assert_eq!(job["queueEntryId"], json!(entry));
}

#[test]
fn triggers_coalesce_and_runs_never_overlap() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    add(&app, "recommended", 1);
    wait_settled(&app);
    let evaluator = &app.services.evaluator;
    let (hook, entered, release) = gate();
    evaluator.on_run_start(Some(hook));
    let before = evaluator.runs_started();

    evaluator.poke(Trigger::QueueChanged);
    entered.recv_timeout(WAIT).expect("a run started");
    for _ in 0..100 {
        evaluator.poke(Trigger::QueueChanged);
    }
    release.send(()).unwrap();
    wait_idle(&app);

    // The run in progress, plus exactly one for every trigger that arrived
    // during it: none is lost, and 100 coalesce into one.
    let runs = evaluator.runs_started() - before;
    assert_eq!(runs, 2, "101 pokes ran {runs} times");
    assert_eq!(evaluator.max_concurrent_runs(), 1, "runs never overlap");
    evaluator.on_run_start(None);
}

#[test]
fn evaluator_and_operator_assign_race_yields_one_job() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    let entry = add(&app, "automatic", 1).remove(0);
    wait_next(&app, "the entry waits for its Spool", |next| {
        is_waiting(next, &entry, "SPOOL_NOT_LOADED")
    });
    // The evaluator proposes, then waits while the operator assigns the
    // same entry to the same Printer.
    let (proposed_tx, proposed) = mpsc::channel::<Proposal>();
    let (go, go_rx) = mpsc::channel::<()>();
    app.services
        .evaluator
        .before_assign(Box::new(move |proposal| {
            let _ = proposed_tx.send(proposal.clone());
            let _ = go_rx.recv_timeout(WAIT);
        }));

    app.load(&spool);
    let proposal = proposed.recv_timeout(WAIT).expect("the evaluator proposed");
    assert_eq!(proposal.entry_id, entry);
    assert_eq!(proposal.printer_id, PRINTER);
    let operator = app.ok(
        "assign_queue_entry",
        json!({
            "operationId": "op-operator-assign",
            "entryId": entry,
            "printerId": PRINTER,
            "spoolId": spool,
        }),
    );
    go.send(()).unwrap();
    wait_idle(&app);

    assert_eq!(job_count(&app), 1, "one Job");
    assert_eq!(operator["jobs"][0]["assignedBy"], "operator");
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM spool_reservations WHERE holder_kind = 'job'"),
        1,
        "one reservation"
    );
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM operations WHERE id LIKE 'auto-%'"),
        0,
        "the refused assignment burned no id"
    );
}

#[test]
fn no_automatic_assignment_behind_an_unproven_adapter() {
    let roots = roots();
    *roots.evidence_tier.lock().unwrap() = EvidenceTier::ReadOnlyHardware;
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    app.load(&spool);
    let entry = add(&app, "automatic", 1).remove(0);

    let next = wait_next(&app, "the entry waits for a proven adapter", |next| {
        is_waiting(next, &entry, "ADAPTER_NOT_PROVEN")
    });
    assert_eq!(
        next["blocker"]["message"],
        "Automatic dispatch needs a simulator-proven Connection. Assign by hand."
    );
    wait_idle(&app);
    assert_eq!(job_count(&app), 0, "readOnlyHardware evidence never counts");

    // The same Printer with sim evidence is assigned: the tier alone held
    // it back.
    *roots.evidence_tier.lock().unwrap() = EvidenceTier::Sim;
    app.services.evaluator.poke(Trigger::CapabilitiesChanged);
    let job = wait_one_job(&app);
    assert_eq!(job["queueEntryId"], json!(entry));
}

#[test]
fn next_automatic_action_states_the_entry_printer_or_waiting_reason() {
    let roots = roots();
    let (hook, entered, release) = gate();
    let app = boot_prepared(
        &roots,
        Driver::Started,
        status_of(OperationalState::Ready),
        JobTimings::default(),
        None,
        move |services| services.evaluator.on_run_start(Some(hook)),
    );
    entered.recv_timeout(WAIT).expect("the first run started");
    // R3 until a run has finished.
    assert_eq!(next_action(&app), json!({"kind": "evaluatorNotRunning"}));
    release.send(()).unwrap();
    let none = wait_next(&app, "no automatic entries", |next| {
        next["kind"] == "noAutomaticEntries"
    });
    assert!(none["evaluatedAt"].is_string());

    let spool = app.spool();
    app.load(&spool);
    let e1 = add(&app, "automatic", 1).remove(0);
    let job_id = id(&wait_one_job(&app));
    app.wait_until("an assigned conclusion was published", || {
        eligibility_events(&app).iter().any(|event| {
            let next = &event["payload"]["nextAutomaticAction"];
            next["kind"] == "assigned"
                && next["entryId"] == json!(e1)
                && next["jobId"] == json!(job_id)
                && next["printerId"] == PRINTER
                && next["evaluatedAt"].is_string()
        })
    });

    let e2 = add(&app, "automatic", 1).remove(0);
    let waiting = wait_next(&app, "E2 waits", |next| is_waiting(next, &e2, "JOB_ACTIVE"));
    assert!(waiting["evaluatedAt"].is_string());
    assert_eq!(
        waiting["blocker"]["message"],
        "This Printer already has a Job."
    );
}

#[test]
fn failed_job_is_never_retried_automatically() {
    let roots = roots();
    let app = p7_dispatch_rig::boot_tuned(
        &roots,
        Driver::Started,
        status_of(OperationalState::Ready),
        fast(),
        None,
    );
    let spool = app.spool();
    app.load(&spool);
    add(&app, "automatic", 1);
    let job_id = id(&wait_one_job(&app));
    app.wait_job(&job_id, "awaitingStart");
    app.start(&format!("start-{job_id}"), &job_id, "ready")
        .expect("start_job");
    app.wait_job(&job_id, "printing");
    app.wait_until("pinned", || app.count_events(&job_id, "hostJobPinned") == 1);
    app.status(OperationalState::Printing);

    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&job_id, "failed");
    app.wait_passes(2);
    app.status(OperationalState::Ready);
    wait_next(&app, "nothing automatic is left", |next| {
        next["kind"] == "noAutomaticEntries"
    });
    wait_idle(&app);

    assert_eq!(job_count(&app), 1, "no second Job");
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM queue_entries"),
        1,
        "no retry entry"
    );
    assert_eq!(roots.uploads(), 1);
    assert_eq!(roots.starts(), 1);
}

/// D6 "Refused assignment": the operator gives the Printer a Job inside
/// the evaluator's window, so the in-transaction re-check refuses with
/// `JOB_ACTIVE`. The entry shows that blocker, the run pokes once more, and
/// then the evaluator goes idle.
#[test]
fn a_refused_assignment_records_its_blocker_and_pokes_once_more() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    let other = add(&app, "recommended", 1).remove(0);
    let entry = add(&app, "automatic", 1).remove(0);
    wait_next(&app, "the entry waits for its Spool", |next| {
        is_waiting(next, &entry, "SPOOL_NOT_LOADED")
    });
    wait_idle(&app);
    let evaluator = &app.services.evaluator;
    let (refused_before, repokes_before) = (evaluator.refused_runs(), evaluator.repokes());
    let (proposed_tx, proposed) = mpsc::channel::<Proposal>();
    let (go, go_rx) = mpsc::channel::<()>();
    evaluator.before_assign(Box::new(move |proposal| {
        let _ = proposed_tx.send(proposal.clone());
        let _ = go_rx.recv_timeout(WAIT);
    }));

    app.load(&spool);
    let proposal = proposed.recv_timeout(WAIT).expect("the evaluator proposed");
    assert_eq!(proposal.entry_id, entry);
    app.ok(
        "assign_queue_entry",
        json!({
            "operationId": "op-operator-other",
            "entryId": other,
            "printerId": PRINTER,
            "spoolId": spool,
        }),
    );
    go.send(()).unwrap();
    wait_idle(&app);

    assert_eq!(
        evaluator.refused_runs() - refused_before,
        1,
        "one refused run"
    );
    assert_eq!(
        evaluator.repokes() - repokes_before,
        1,
        "exactly one re-poke"
    );
    assert_eq!(job_count(&app), 1, "only the operator's Job");
    let snapshot = list(&app);
    let summary = summary(&snapshot, &entry);
    assert_eq!(summary["verdict"], "blocked");
    assert_eq!(summary["topBlocker"]["code"], "JOB_ACTIVE");
    assert!(is_waiting(
        &snapshot["nextAutomaticAction"],
        &entry,
        "JOB_ACTIVE"
    ));
    assert_eq!(
        app.scalar("SELECT COUNT(*) FROM operations WHERE id LIKE 'auto-%'"),
        0,
        "the refused assignment burned no id"
    );
}

/// Issue #17 end to end: a Printer whose adapter can't upload (OctoPrint's
/// `notVerified`) never takes an Automatic entry.
#[test]
fn no_automatic_assignment_behind_an_unsupported_capability() {
    let roots = roots();
    roots
        .upload_unsupported
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    app.load(&spool);
    let entry = add(&app, "automatic", 1).remove(0);

    let next = wait_next(&app, "the entry waits for a capable adapter", |next| {
        is_waiting(next, &entry, "CAPABILITY_UNSUPPORTED")
    });
    assert_eq!(
        next["blocker"]["detail"],
        "Uploads are switched off in this test."
    );
    wait_idle(&app);
    assert_eq!(job_count(&app), 0);
    let snapshot = list(&app);
    assert_eq!(
        summary(&snapshot, &entry)["topBlocker"]["code"],
        "CAPABILITY_UNSUPPORTED"
    );
    assert_eq!(roots.uploads(), 0);
}

/// A run that fails (a storage error) pokes once more, so a ready
/// Automatic entry isn't stranded until some unrelated change.
#[test]
fn a_failed_run_pokes_once_more_so_a_ready_entry_is_assigned() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    app.load(&spool);
    wait_settled(&app);
    let evaluator = &app.services.evaluator;
    let repokes_before = evaluator.repokes();
    evaluator.fail_next_runs(1);

    let entry = add(&app, "automatic", 1).remove(0);

    let job = wait_one_job(&app);
    assert_eq!(job["queueEntryId"], json!(entry));
    wait_idle(&app);
    assert_eq!(evaluator.repokes() - repokes_before, 1);
}

/// The failure re-poke is bounded: two failed runs in a row poke once,
/// then the evaluator waits for the next real trigger.
#[test]
fn failed_runs_poke_once_then_stop() {
    let roots = roots();
    let app = boot(&roots, Driver::Started);
    let spool = app.spool();
    app.load(&spool);
    wait_settled(&app);
    let evaluator = &app.services.evaluator;
    let (runs_before, repokes_before) = (evaluator.runs(), evaluator.repokes());
    evaluator.fail_next_runs(2);

    add(&app, "automatic", 1);
    wait_idle(&app);

    assert_eq!(
        evaluator.runs() - runs_before,
        2,
        "the failed run and its one retry"
    );
    assert_eq!(evaluator.repokes() - repokes_before, 1);
    assert_eq!(job_count(&app), 0, "nothing ran after the second failure");

    evaluator.poke(Trigger::QueueChanged);
    wait_one_job(&app);
}
