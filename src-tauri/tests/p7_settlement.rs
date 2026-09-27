//! P7 Task 9 (spec "Material settlement", D3's settlement column, D4's
//! settle/defer/correct/declare rows, `settlementPreview` ruling R4,
//! `JOB_ALREADY_SETTLED`/`JOB_ACTION_NOT_ALLOWED`): exactly-once
//! settlement of a Job's Spool reservation, through the Tauri IPC path
//! against `FakeMoonraker` via `p7_dispatch_rig`.

mod common;
mod p7_dispatch_rig;

use std::time::Duration;

use farm3d_lib::jobs::estimated_use_mg;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;
use p7_dispatch_rig::{boot_tuned, fast, id, status_of, Driver, Roots, PRINTER, ESTIMATE_MG, SLR};
use serde_json::{json, Value};

fn roots() -> Roots {
    Roots::new(StartSafety::ConfirmBedClear)
}

fn fast_boot(roots: &Roots) -> p7_dispatch_rig::Running {
    boot_tuned(roots, Driver::Started, status_of(OperationalState::Ready), fast(), None)
}

fn requirements(app: &p7_dispatch_rig::Running, job_id: &str) -> Vec<Value> {
    app.history(job_id)["requirements"].as_array().unwrap().clone()
}

fn material_requirement(app: &p7_dispatch_rig::Running, job_id: &str) -> Value {
    requirements(app, job_id)
        .into_iter()
        .find(|requirement| requirement["kind"] == "materialReconciliation")
        .expect("a materialReconciliation requirement")
}

fn reservation_id_of(job: &Value) -> String {
    job["reservationId"].as_str().unwrap().to_string()
}

const ESTIMATED: fn() -> Value = || json!({"kind": "estimated"});
fn measured(net_mg: i64) -> Value {
    json!({"kind": "measured", "entry": {"kind": "net", "netMg": net_mg, "confidence": "measured"}})
}
fn measured_with_confidence(net_mg: i64, confidence: &str) -> Value {
    json!({"kind": "measured", "entry": {"kind": "net", "netMg": net_mg, "confidence": confidence}})
}
const DEFER: fn() -> Value = || json!({"kind": "defer"});

/// `estimated_use_mg`'s pure rounding rule (owner decisions 5/6):
/// `ceil(estimateMg × maxProgressPct / 100)`, 0 at 0 %.
#[test]
fn estimated_use_is_progress_proportional_and_rounds_up() {
    assert_eq!(estimated_use_mg(12_500, 0), 0, "never printed");
    assert_eq!(estimated_use_mg(12_501, 37), 4_626, "rounds up: ceil(4625.37)");
    assert_eq!(estimated_use_mg(12_500, 100), 12_500, "the full estimate");
}

/// A completed Job's terminal transaction consumes the full estimate
/// (never progress-scaled), once, and marks the reservation `consumed`.
#[test]
fn completion_consumes_the_estimate_in_the_terminal_transaction() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    let job = app.job(&job_id);
    let spool_id = job["spoolId"].as_str().unwrap().to_string();
    let reservation_id = reservation_id_of(&job);

    roots.fake.finish_print("completed");
    let job = app.wait_job(&job_id, "completed");
    assert_eq!(job["settlement"], "settled");
    assert_eq!(job["settlementMethod"], "estimated");
    assert_eq!(job["settlementPreview"], Value::Null, "ruling R4: settled, not previewed");
    assert_eq!(app.reservation_state(&reservation_id), "consumed");

    let consumptions: Vec<Value> = app
        .amount_events(&spool_id)
        .into_iter()
        .filter(|event| event["kind"] == "consumption")
        .collect();
    assert_eq!(consumptions.len(), 1, "consumed exactly once");
    assert_eq!(consumptions[0]["reservationId"], json!(reservation_id));
    assert_eq!(consumptions[0]["afterMg"], json!(1_000_000 - ESTIMATE_MG));
    assert_eq!(app.spool_current_mg(&spool_id), 1_000_000 - ESTIMATE_MG);
}

/// A correction after completion appends one `Measurement` ledger row,
/// marked as a correction, referencing the reservation, and records
/// `correction_event_id` on the Job.
#[test]
fn completion_correction_records_one_measurement_marked_as_correction() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    let spool_id = app.job(&job_id)["spoolId"].as_str().unwrap().to_string();
    let reservation_id = reservation_id_of(&app.job(&job_id));
    roots.fake.finish_print("completed");
    app.wait_job(&job_id, "completed");

    let corrected = app.correct("op-correct", &job_id, json!({"kind": "net", "netMg": 600_000, "confidence": "measured"})).unwrap();
    assert_eq!(corrected["jobs"][0]["corrected"], true);
    assert_eq!(corrected["jobs"][0]["state"], "completed", "unchanged");
    assert_eq!(app.reservation_state(&reservation_id), "consumed", "unchanged");
    assert_eq!(app.spool_current_mg(&spool_id), 600_000);
    assert_eq!(app.count_events(&job_id, "materialCorrected"), 1);

    let events = app.amount_events(&spool_id);
    let last = events.last().unwrap();
    assert_eq!(last["kind"], "measurement");
    assert_eq!(last["reservationId"], json!(reservation_id));
    assert_eq!(last["isCorrection"], true);
    assert_eq!(last["note"], json!(format!("Correction for Job {job_id}")));
}

/// A second correction is `JOB_ALREADY_SETTLED{reason: corrected}`.
#[test]
fn second_correction_is_rejected() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.finish_print("completed");
    app.wait_job(&job_id, "completed");
    app.correct("op-correct-1", &job_id, json!({"kind": "net", "netMg": 600_000, "confidence": "measured"})).unwrap();

    let again = app
        .correct("op-correct-2", &job_id, json!({"kind": "net", "netMg": 500_000, "confidence": "measured"}))
        .unwrap_err();
    assert_eq!(again["code"], "JOB_ALREADY_SETTLED");
    assert_eq!(again["details"]["jobId"], json!(job_id));
    assert_eq!(again["details"]["reason"], "corrected");
    assert_eq!(app.count_events(&job_id, "materialCorrected"), 1, "the second attempt wrote nothing");
}

/// A failed (or cancelled) Job after start marks its reservation
/// `unresolved` and opens one `materialReconciliation` requirement,
/// `pending`.
#[test]
fn failed_job_marks_the_reservation_unresolved_and_opens_a_pending_requirement() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    let job = app.job(&job_id);
    let spool_id = job["spoolId"].as_str().unwrap().to_string();
    let reservation_id = reservation_id_of(&job);

    roots.fake.finish_print("klippy_shutdown");
    let job = app.wait_job(&job_id, "failed");
    assert_eq!(job["settlement"], "pending");
    assert_eq!(
        job["settlementPreview"],
        json!({"estimatedUseMg": estimated_use_mg(ESTIMATE_MG, job["maxProgressPct"].as_i64().unwrap())}),
        "ruling R4: previewed while pending"
    );
    assert_eq!(app.reservation_state(&reservation_id), "unresolved");

    let requirement = material_requirement(&app, &job_id);
    assert_eq!(requirement["status"], "pending");
    assert_eq!(requirement["spoolId"], json!(spool_id));
    assert_eq!(requirement["reservationId"], json!(reservation_id));
    assert_eq!(requirement["resolution"], Value::Null);
}

/// Fix round 1 (Important finding): owner decision 5 checked end to end
/// -- `settle(estimated)` on a Job with a non-zero `maxProgressPct` must
/// use `estimated_use_mg` (progress-scaled), not the full estimate and
/// not zero. A Job stuck at 0 % or one that ran to completion wouldn't
/// tell `job.estimate_mg`/`0` apart from the real, progress-scaled value.
#[test]
fn estimated_settlement_uses_the_progress_scaled_estimate() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    let spool_id = app.job(&job_id)["spoolId"].as_str().unwrap().to_string();
    let before_mg = app.spool_current_mg(&spool_id);

    roots.fake.set_progress(0.37);
    app.mirror(&roots.fake);
    app.wait_job_until(&job_id, |job| job["maxProgressPct"] == 37);

    roots.fake.finish_print("klippy_shutdown");
    let job = app.wait_job(&job_id, "failed");
    assert_eq!(job["maxProgressPct"], 37);
    let expected_used_mg = estimated_use_mg(ESTIMATE_MG, 37);
    assert_ne!(expected_used_mg, ESTIMATE_MG, "sanity: distinct from the full estimate");
    assert_ne!(expected_used_mg, 0, "sanity: distinct from zero");
    assert_eq!(
        job["settlementPreview"],
        json!({"estimatedUseMg": expected_used_mg}),
        "ruling R4: settlementPreview matched it beforehand"
    );

    let settled = app.settle("op-settle-progress", &job_id, ESTIMATED()).unwrap();
    let requirement = settled["requirements"][0].clone();
    assert_eq!(
        requirement["resolution"],
        json!({"kind": "settled", "method": "estimated", "usedMg": expected_used_mg}),
        "owner decision 5: progress-scaled, not the full estimate or zero"
    );

    let consumptions: Vec<Value> = app
        .amount_events(&spool_id)
        .into_iter()
        .filter(|event| event["kind"] == "consumption")
        .collect();
    assert_eq!(consumptions.len(), 1);
    assert_eq!(consumptions[0]["afterMg"], json!(before_mg - expected_used_mg));
    assert_eq!(app.spool_current_mg(&spool_id), before_mg - expected_used_mg);
}

/// Fix round 1 ruling R14(b)/(c): `settle_job_material`'s `measured`
/// choice must reject an entry that isn't genuinely measured (a `Net`
/// entry silently carrying `confidence: "estimated"` would promote a
/// guess into a Measurement ledger row), and an out-of-range weight --
/// both `VALIDATION`, naming the field under `choice.entry`, not `entry`
/// (the entry lives nested under `choice` in this command's request).
#[test]
fn settle_measured_rejects_a_non_measured_or_out_of_range_entry() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&job_id, "failed");

    let not_measured = app
        .settle("op-bad-confidence", &job_id, measured_with_confidence(100_000, "estimated"))
        .unwrap_err();
    assert_eq!(not_measured["code"], "VALIDATION");
    assert_eq!(not_measured["details"]["fieldPath"], "choice.entry.confidence");

    let out_of_range = app.settle("op-bad-range", &job_id, measured(-1)).unwrap_err();
    assert_eq!(out_of_range["code"], "VALIDATION");
    assert_eq!(out_of_range["details"]["fieldPath"], "choice.entry.netMg");

    assert_eq!(app.job(&job_id)["settlement"], "pending", "neither attempt settled the Job");
}

/// Fix round 1 ruling R14(b)/(c): `correct_job_material`'s entry has the
/// same two guards, with the field named at the request's own top-level
/// `entry` (no `choice` nesting for this command).
#[test]
fn correct_rejects_a_non_measured_or_out_of_range_entry() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.finish_print("completed");
    app.wait_job(&job_id, "completed");

    let not_measured = app
        .correct("op-bad-confidence", &job_id, json!({"kind": "net", "netMg": 100_000, "confidence": "estimated"}))
        .unwrap_err();
    assert_eq!(not_measured["code"], "VALIDATION");
    assert_eq!(not_measured["details"]["fieldPath"], "entry.confidence");

    let out_of_range = app
        .correct("op-bad-range", &job_id, json!({"kind": "net", "netMg": -1, "confidence": "measured"}))
        .unwrap_err();
    assert_eq!(out_of_range["code"], "VALIDATION");
    assert_eq!(out_of_range["details"]["fieldPath"], "entry.netMg");

    assert_eq!(app.job(&job_id)["corrected"], false, "neither attempt recorded a correction");
}

/// `measured` settlement consumes the reservation once (via the measured
/// remaining amount, not the used one) and resolves the requirement.
#[test]
fn measured_settlement_consumes_once_and_resolves_the_requirement() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    let job = app.job(&job_id);
    let spool_id = job["spoolId"].as_str().unwrap().to_string();
    let reservation_id = reservation_id_of(&job);
    let before_mg = app.spool_current_mg(&spool_id);
    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&job_id, "failed");

    let settled = app.settle("op-settle", &job_id, measured(before_mg - 9_500)).unwrap();
    assert_eq!(settled["jobs"][0]["settlement"], "settled");
    assert_eq!(settled["jobs"][0]["settlementMethod"], "measured");
    assert_eq!(app.reservation_state(&reservation_id), "consumed");
    assert_eq!(app.spool_current_mg(&spool_id), before_mg - 9_500);

    let requirement = settled["requirements"][0].clone();
    assert_eq!(requirement["status"], "resolved");
    assert_eq!(
        requirement["resolution"],
        json!({"kind": "settled", "method": "measured", "usedMg": 9_500})
    );

    // Exactly one measurement row on the ledger.
    let measurements: Vec<Value> = app
        .amount_events(&spool_id)
        .into_iter()
        .filter(|event| event["kind"] == "measurement")
        .collect();
    assert_eq!(measurements.len(), 1);
}

/// Deferring keeps the reservation `unresolved` (the amount stays
/// unavailable), and a second entry that needs the same Spool is blocked
/// on insufficient material.
#[test]
fn defer_keeps_the_amount_unavailable_and_blocks_over_reservation() {
    let roots = roots();
    let app = fast_boot(&roots);
    // A Spool sized to exactly one Job's estimate, so a second one finds
    // nothing available while the first stays unresolved.
    let spool = app.spool_sized(ESTIMATE_MG);
    let job_id = app.printing_with(&spool);
    let reservation_id = reservation_id_of(&app.job(&job_id));
    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&job_id, "failed");

    let deferred = app.settle("op-defer", &job_id, DEFER()).unwrap();
    assert_eq!(deferred["jobs"][0]["settlement"], "deferred");
    assert_eq!(deferred["requirements"][0]["status"], "deferred");
    assert_eq!(app.reservation_state(&reservation_id), "unresolved");
    assert_eq!(app.spool_current_mg(&spool), ESTIMATE_MG, "untouched");

    // The Printer is idle again (the failure ended the print); a second
    // entry against the same, fully-reserved Spool is blocked.
    app.status(OperationalState::Ready);
    let added = app.ok(
        "add_to_queue",
        json!({"operationId": "op-add-2", "sliceRevisionId": SLR, "quantity": 1,
               "policy": "recommended", "preference": "loadedFirst"}),
    );
    let entry_id = id(&added["entries"][0]);
    let blocked = app
        .call(
            "assign_queue_entry",
            json!({"operationId": "op-assign-2", "entryId": entry_id, "printerId": PRINTER, "spoolId": spool}),
        )
        .unwrap_err();
    assert_eq!(blocked["code"], "ASSIGNMENT_BLOCKED");
    assert_eq!(blocked["details"]["blockers"][0]["code"], "INSUFFICIENT_MATERIAL");
}

/// A deferred Job can later be settled by measurement, exactly once.
#[test]
fn deferred_then_measured_settles_exactly_once() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    let job = app.job(&job_id);
    let spool_id = job["spoolId"].as_str().unwrap().to_string();
    let reservation_id = reservation_id_of(&job);
    roots.fake.finish_print("cancelled");
    app.wait_job(&job_id, "cancelled");

    app.settle("op-defer", &job_id, DEFER()).unwrap();
    assert_eq!(app.reservation_state(&reservation_id), "unresolved");

    let settled = app.settle("op-settle", &job_id, measured(400_000)).unwrap();
    assert_eq!(settled["jobs"][0]["settlement"], "settled");
    assert_eq!(settled["jobs"][0]["settlementMethod"], "measured");
    assert_eq!(app.reservation_state(&reservation_id), "consumed");
    assert_eq!(app.count_events(&job_id, "materialDeferred"), 1);
    assert_eq!(app.count_events(&job_id, "materialSettled"), 1);
    assert_eq!(
        app.amount_events(&spool_id)
            .into_iter()
            .filter(|event| event["kind"] == "measurement")
            .count(),
        1,
        "settled exactly once"
    );
}

/// A replay of the same `operationId`/choice returns the current rows,
/// writes no second ledger row, and publishes nothing; a new
/// `operationId` after settlement is `JOB_ALREADY_SETTLED`.
#[test]
fn settle_replay_returns_the_same_result_and_writes_no_second_ledger_row() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    let spool_id = app.job(&job_id)["spoolId"].as_str().unwrap().to_string();
    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&job_id, "failed");

    let first = app.settle("op-settle", &job_id, ESTIMATED()).unwrap();
    assert_eq!(first["jobs"][0]["settlement"], "settled");
    let job_events_before = app.count_stream_events("queue.job.changed", &job_id);
    let ledger_before = app
        .amount_events(&spool_id)
        .into_iter()
        .filter(|event| event["kind"] == "consumption")
        .count();

    let replayed = app.settle("op-settle", &job_id, ESTIMATED()).unwrap();
    assert_eq!(replayed["jobs"][0]["id"], json!(job_id));
    assert_eq!(replayed["jobs"][0]["settlement"], "settled");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        app.count_stream_events("queue.job.changed", &job_id),
        job_events_before,
        "a replay publishes nothing"
    );
    let ledger_after = app
        .amount_events(&spool_id)
        .into_iter()
        .filter(|event| event["kind"] == "consumption")
        .count();
    assert_eq!(ledger_after, ledger_before, "no second ledger row");

    let again = app.settle("op-settle-again", &job_id, ESTIMATED()).unwrap_err();
    assert_eq!(again["code"], "JOB_ALREADY_SETTLED");
    assert_eq!(again["details"]["reason"], "settled");
}

/// A Job cancelled before start needs no settlement (`notRequired`) and
/// releases its reservation instead of consuming or deferring it.
#[test]
fn cancelled_before_start_needs_no_settlement_and_releases_the_reservation() {
    let roots = roots();
    let app = fast_boot(&roots);
    let spool = app.spool();
    let job_id = app.assign(&spool);
    let reservation_id = reservation_id_of(&app.job(&job_id));

    app.job_command("cancel_job", "op-cancel", &job_id).unwrap();
    let job = app.wait_job(&job_id, "cancelled");
    assert_eq!(job["settlement"], "notRequired");
    assert_eq!(job["cancelReason"], "cancelledBeforeStart");
    assert!(!job["allowedActions"].as_array().unwrap().contains(&json!("settleMaterial")));
    assert_eq!(app.reservation_state(&reservation_id), "released");

    let refused = app.settle("op-settle", &job_id, ESTIMATED()).unwrap_err();
    assert_eq!(refused["code"], "JOB_ACTION_NOT_ALLOWED");
}

/// The declare -> settle chain: an `outcomeUnknown` Job declared `failed`
/// opens a pending `materialReconciliation` requirement in the same
/// transaction, and can then be settled.
#[test]
fn outcome_unknown_declared_failed_then_settled() {
    let roots = roots();
    let app = fast_boot(&roots);
    let job_id = app.printing();
    roots.fake.with_state(|state| state.history.clear());
    app.wait_job(&job_id, "outcomeUnknown");

    let declared = app.declare("op-declare", &job_id, "failed").unwrap();
    assert_eq!(declared["jobs"][0]["state"], "failed");
    assert_eq!(declared["jobs"][0]["settlement"], "pending");
    let reservation_id = reservation_id_of(&declared["jobs"][0]);
    assert_eq!(app.reservation_state(&reservation_id), "unresolved");

    let requirements = declared["requirements"].as_array().unwrap();
    let outcome_unknown = requirements
        .iter()
        .find(|requirement| requirement["kind"] == "jobOutcomeUnknown")
        .expect("the jobOutcomeUnknown requirement");
    assert_eq!(outcome_unknown["status"], "resolved");
    let material = requirements
        .iter()
        .find(|requirement| requirement["kind"] == "materialReconciliation")
        .expect("a materialReconciliation requirement");
    assert_eq!(material["status"], "pending");

    let settled = app.settle("op-settle", &job_id, ESTIMATED()).unwrap();
    assert_eq!(settled["jobs"][0]["settlement"], "settled");
    assert_eq!(app.reservation_state(&reservation_id), "consumed");
}

/// Every settlement path (automatic completion, `settle_job_material`
/// estimated/defer, `correct_job_material`) publishes the Job, the
/// requirement (if any), and the Spool -- exactly once.
#[test]
fn every_settlement_path_publishes_job_spool_and_requirement_changes_once() {
    let roots = roots();
    let app = fast_boot(&roots);

    // Path 1: automatic completion (the tracker's terminal transaction).
    let completed_job = app.printing();
    let completed_spool = app.job(&completed_job)["spoolId"].as_str().unwrap().to_string();
    let before_job = app.count_stream_events("queue.job.changed", &completed_job);
    let before_spool = app.count_stream_events("spool.changed", &completed_spool);
    roots.fake.finish_print("completed");
    app.wait_job(&completed_job, "completed");
    std::thread::sleep(Duration::from_millis(200));
    assert_eq!(
        app.count_stream_events("queue.job.changed", &completed_job) - before_job,
        1,
        "completion publishes the Job once"
    );
    assert_eq!(
        app.count_stream_events("spool.changed", &completed_spool) - before_spool,
        1,
        "completion publishes the Spool once"
    );

    // Path 2: the tracker's failure path opens a requirement, once. The
    // fake is still reporting Path 1's finished print; reset it so this
    // Job can be assigned and started.
    roots.fake.restart();
    app.status(OperationalState::Ready);
    let failed_job = app.printing();
    let failed_spool = app.job(&failed_job)["spoolId"].as_str().unwrap().to_string();
    roots.fake.finish_print("klippy_shutdown");
    app.wait_job(&failed_job, "failed");
    let requirement_id = material_requirement(&app, &failed_job)["id"].as_str().unwrap().to_string();
    assert_eq!(app.count_stream_events("queue.requirement.changed", &requirement_id), 1);

    // Path 3: `settle_job_material` (estimated).
    let before_job = app.count_stream_events("queue.job.changed", &failed_job);
    let before_req = app.count_stream_events("queue.requirement.changed", &requirement_id);
    let before_spool = app.count_stream_events("spool.changed", &failed_spool);
    app.settle("op-settle-count", &failed_job, ESTIMATED()).unwrap();
    assert_eq!(app.count_stream_events("queue.job.changed", &failed_job) - before_job, 1);
    assert_eq!(app.count_stream_events("queue.requirement.changed", &requirement_id) - before_req, 1);
    assert_eq!(app.count_stream_events("spool.changed", &failed_spool) - before_spool, 1);

    // Path 4: `settle_job_material` (defer), on a fresh failed Job.
    roots.fake.restart();
    app.status(OperationalState::Ready);
    let deferred_job = app.printing();
    let deferred_spool = app.job(&deferred_job)["spoolId"].as_str().unwrap().to_string();
    roots.fake.finish_print("cancelled");
    app.wait_job(&deferred_job, "cancelled");
    let deferred_requirement_id =
        material_requirement(&app, &deferred_job)["id"].as_str().unwrap().to_string();
    let before_job = app.count_stream_events("queue.job.changed", &deferred_job);
    let before_req = app.count_stream_events("queue.requirement.changed", &deferred_requirement_id);
    let before_spool = app.count_stream_events("spool.changed", &deferred_spool);
    app.settle("op-defer-count", &deferred_job, DEFER()).unwrap();
    assert_eq!(app.count_stream_events("queue.job.changed", &deferred_job) - before_job, 1);
    assert_eq!(
        app.count_stream_events("queue.requirement.changed", &deferred_requirement_id) - before_req,
        1
    );
    assert_eq!(app.count_stream_events("spool.changed", &deferred_spool) - before_spool, 1);

    // Path 5: `correct_job_material`, on the first (completed) Job.
    let before_job = app.count_stream_events("queue.job.changed", &completed_job);
    let before_spool = app.count_stream_events("spool.changed", &completed_spool);
    app.correct("op-correct-count", &completed_job, json!({"kind": "net", "netMg": 100_000, "confidence": "measured"})).unwrap();
    assert_eq!(app.count_stream_events("queue.job.changed", &completed_job) - before_job, 1);
    assert_eq!(app.count_stream_events("spool.changed", &completed_spool) - before_spool, 1);
}
