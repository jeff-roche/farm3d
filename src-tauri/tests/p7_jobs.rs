//! P7 Task 6 (spec D2, D3, D4, D5, "Commands", "Events", "Error codes"):
//! the atomic assignment transaction, release, retry, cancel before start,
//! and `get_job_history`, through the Tauri IPC path; plus the concurrent
//! assignment races, driven straight through `jobs::assign::assign` from
//! real threads over one `Storage`.
//!
//! P7 Task 8a (spec D3, D4, D7, D9): the Job handoffs to P6 (stage,
//! start, pause, resume, cancel after start), the dispatch driver, and
//! `allowedActions`/`startBlockers`, against `FakeMoonraker` through
//! `p7_dispatch_rig`.

mod common;
mod p7_dispatch_rig;
mod p7_rig;

use std::sync::{Arc, Barrier};

use farm3d_lib::jobs::assign::{self, AssignRequest};
use farm3d_lib::jobs::repository::{self as jobs_repository, JobChange};
use farm3d_lib::jobs::{AssignedBy, JobEventKind};
use farm3d_lib::persistence::RepositoryError;
use farm3d_lib::queue::eligibility::AssignMode;
use p7_rig::{FixedWorld, Rig, PRINTER_A, PRINTER_B, SECRET, SLR_ESTIMATE_MG, SLR_UNCONFIRMED};
use serde_json::{json, Value};

const NOW: &str = "2026-09-27T12:00:00Z";

fn id(value: &Value) -> String {
    value["id"].as_str().unwrap().to_string()
}

/// One entry assigned to Printer A with a fresh 1 kg Spool: the entry id,
/// the Job id, and the Spool id.
fn assigned(rig: &Rig) -> (String, String, String) {
    let spool = rig.spool(1_000_000);
    let entry = id(&rig.add("op-add", 1)[0]);
    let change = rig.assign("op-assign", &entry, PRINTER_A, &spool).unwrap();
    (entry, id(&change["jobs"][0]), spool)
}

#[test]
fn assign_reserves_the_spool_and_creates_the_job_atomically() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entry = id(&rig.add("op-add", 1)[0]);
    let before = rig.queue_events().len();

    let change = rig.assign("op-assign", &entry, PRINTER_A, &spool).unwrap();

    let job = &change["jobs"][0];
    assert_eq!(job["state"], "assigned");
    assert_eq!(job["settlement"], "open");
    assert_eq!(job["assignedBy"], "operator");
    assert_eq!(job["printerId"], PRINTER_A);
    assert_eq!(job["spoolId"], json!(spool));
    assert_eq!(job["estimateMg"], SLR_ESTIMATE_MG);
    assert_eq!(job["printerSnapshot"]["name"], "Alpha");
    assert_eq!(job["printerSnapshot"]["adapterKind"], "moonraker");
    let row = &change["entries"][0];
    assert_eq!(row["state"], "assigned");
    assert_eq!(row["jobId"], job["id"]);
    assert_eq!(
        row["position"], 1,
        "assignment is sticky: the position stays"
    );

    assert_eq!(rig.available_mg(&spool), 1_000_000 - SLR_ESTIMATE_MG);
    assert_eq!(
        rig.count(&format!(
            "SELECT COUNT(*) FROM spool_reservations
             WHERE holder_kind = 'job' AND holder_id = '{}' AND state = 'active'",
            id(job)
        )),
        1
    );
    let history = rig
        .call("get_job_history", json!({"jobId": job["id"]}))
        .unwrap();
    assert_eq!(history["events"].as_array().unwrap().len(), 1);
    assert_eq!(history["events"][0]["kind"], "assigned");
    assert_eq!(history["events"][0]["fromState"], Value::Null);

    // Entry first, then Job, after commit; plus the Spool's inventory
    // events.
    let events = rig.queue_events();
    let kinds: Vec<&str> = events[before..]
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect();
    assert_eq!(kinds, ["queue.entry.changed", "queue.job.changed"]);
    let spool_events = rig
        .events
        .lock()
        .unwrap()
        .iter()
        .filter(|text| text.contains("spool.availability.changed") && text.contains(&spool))
        .count();
    assert_eq!(spool_events, 1);
}

#[test]
fn assign_rolls_back_everything_when_reserve_fails() {
    let rig = Rig::new();
    // Too little on the Spool for the 12.5 g estimate.
    let spool = rig.spool(10_000);
    let entry = id(&rig.add("op-add", 1)[0]);
    let events = rig.event_count();

    let refused = rig
        .assign("op-assign", &entry, PRINTER_A, &spool)
        .unwrap_err();

    assert_eq!(refused["code"], "ASSIGNMENT_BLOCKED");
    assert_eq!(
        refused["details"]["blockers"][0]["code"],
        "INSUFFICIENT_MATERIAL"
    );
    assert_eq!(rig.count("SELECT COUNT(*) FROM jobs"), 0);
    assert_eq!(rig.count("SELECT COUNT(*) FROM spool_reservations"), 0);
    assert_eq!(rig.count("SELECT COUNT(*) FROM job_events"), 0);
    assert_eq!(rig.entry(&entry)["state"], "queued");
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM operations WHERE id = 'op-assign'"),
        0,
        "the ledger id is not burned"
    );
    assert_eq!(rig.event_count(), events, "a refusal publishes nothing");

    // The same id then succeeds against a Spool with enough on it.
    let enough = rig.spool(1_000_000);
    let change = rig.assign("op-assign", &entry, PRINTER_A, &enough).unwrap();
    assert_eq!(change["jobs"][0]["spoolId"], json!(enough));
}

#[test]
fn assign_replay_returns_the_same_job_and_publishes_nothing() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entry = id(&rig.add("op-add", 1)[0]);
    let first = rig.assign("op-assign", &entry, PRINTER_A, &spool).unwrap();
    let events = rig.event_count();

    let replayed = rig.assign("op-assign", &entry, PRINTER_A, &spool).unwrap();

    assert_eq!(replayed["jobs"][0]["id"], first["jobs"][0]["id"]);
    assert_eq!(replayed["entries"][0]["id"], first["entries"][0]["id"]);
    assert_eq!(rig.event_count(), events, "a replay publishes nothing");
    assert_eq!(rig.count("SELECT COUNT(*) FROM jobs"), 1);
    assert_eq!(rig.count("SELECT COUNT(*) FROM spool_reservations"), 1);
}

#[test]
fn assign_reused_operation_id_is_rejected() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 2);
    rig.assign("op-assign", &id(&entries[0]), PRINTER_A, &spool)
        .unwrap();

    let reused = rig
        .assign("op-assign", &id(&entries[1]), PRINTER_B, &spool)
        .unwrap_err();

    assert_eq!(reused["code"], "VALIDATION");
    assert_eq!(reused["details"]["fieldPath"], "operationId");
    assert_eq!(rig.count("SELECT COUNT(*) FROM jobs"), 1);
}

#[test]
fn second_assign_to_a_printer_with_an_active_job_is_job_active() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 2);
    let first = rig
        .assign("op-assign-1", &id(&entries[0]), PRINTER_A, &spool)
        .unwrap();

    let refused = rig
        .assign("op-assign-2", &id(&entries[1]), PRINTER_A, &spool)
        .unwrap_err();

    assert_eq!(refused["code"], "JOB_ACTIVE");
    assert_eq!(
        refused["message"],
        "This Printer has an active Job. Use the Job's controls."
    );
    assert_eq!(refused["recovery"], json!(["OPEN_JOB"]));
    assert_eq!(refused["details"]["printerId"], PRINTER_A);
    assert_eq!(refused["details"]["jobId"], first["jobs"][0]["id"]);
    assert_eq!(rig.entry(&id(&entries[1]))["state"], "queued");
}

#[test]
fn assigning_an_assigned_entry_is_queue_entry_action_not_allowed() {
    let rig = Rig::new();
    let (entry, _job, spool) = assigned(&rig);

    let refused = rig
        .assign("op-again", &entry, PRINTER_B, &spool)
        .unwrap_err();

    assert_eq!(refused["code"], "QUEUE_ENTRY_ACTION_NOT_ALLOWED");
    assert_eq!(refused["details"]["action"], "assign");
    assert_eq!(refused["details"]["state"], "assigned");
}

#[test]
fn assign_to_an_archived_printer_reports_printer_archived() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entry = id(&rig.add("op-add", 1)[0]);
    rig.storage
        .write_repo(|tx| {
            tx.execute(
                "UPDATE printers SET archived_at = '2026-09-01T00:00:00Z' WHERE id = ?1",
                [PRINTER_B],
            )?;
            Ok(())
        })
        .unwrap();

    let refused = rig
        .assign("op-assign", &entry, PRINTER_B, &spool)
        .unwrap_err();
    assert_eq!(refused["code"], "ASSIGNMENT_BLOCKED");
    assert_eq!(
        refused["details"]["blockers"][0]["code"],
        "PRINTER_ARCHIVED"
    );

    let unknown = rig
        .assign("op-assign", &entry, "prn-missing", &spool)
        .unwrap_err();
    assert_eq!(unknown["code"], "NOT_FOUND");
}

#[test]
fn a_manual_entry_with_unconfirmed_facts_needs_the_acknowledgement() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let added = rig
        .call(
            "add_to_queue",
            json!({
                "operationId": "op-add", "sliceRevisionId": SLR_UNCONFIRMED, "quantity": 1,
                "policy": "manual", "preference": "loadedFirst", "manualPrinterId": PRINTER_A,
                "materialEstimate": {"amountMg": 5_000, "source": "operatorEntered"},
            }),
        )
        .unwrap();
    let entry = id(&added["entries"][0]);

    let unacknowledged = rig
        .assign("op-assign", &entry, PRINTER_A, &spool)
        .unwrap_err();
    assert_eq!(unacknowledged["code"], "VALIDATION");
    assert_eq!(
        unacknowledged["details"]["fieldPath"],
        "acknowledgeManualFacts"
    );

    let change = rig
        .call(
            "assign_queue_entry",
            json!({
                "operationId": "op-assign", "entryId": entry, "printerId": PRINTER_A,
                "spoolId": spool, "acknowledgeManualFacts": true,
            }),
        )
        .unwrap();
    assert_eq!(change["jobs"][0]["state"], "assigned");
    assert_eq!(
        rig.count("SELECT manual_facts_acknowledged FROM jobs"),
        1,
        "the Job records the acknowledgement"
    );
}

fn request(operation_id: &str, entry_id: &str, printer_id: &str, spool_id: &str) -> AssignRequest {
    AssignRequest {
        operation_id: operation_id.to_string(),
        entry_id: entry_id.to_string(),
        printer_id: printer_id.to_string(),
        spool_id: spool_id.to_string(),
        mode: AssignMode::Operator {
            acknowledge_manual_facts: false,
        },
        assigned_by: AssignedBy::Operator,
    }
}

/// Runs each request's `assign` in its own `write_repo` on its own thread,
/// released together by a barrier.
fn race(rig: &Rig, requests: Vec<AssignRequest>) -> Vec<Result<String, RepositoryError>> {
    let barrier = Arc::new(Barrier::new(requests.len()));
    let handles: Vec<_> = requests
        .into_iter()
        .map(|request| {
            let storage = Arc::clone(&rig.storage);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                let world = FixedWorld::new();
                barrier.wait();
                storage
                    .write_repo(|tx| assign::assign(tx, &world, &request, NOW))
                    .map(|assigned| assigned.job.id)
            })
        })
        .collect();
    handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect()
}

#[test]
fn concurrent_assigns_of_one_entry_yield_exactly_one_job() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entry = id(&rig.add("op-add", 1)[0]);

    let results = race(
        &rig,
        vec![
            request("op-a", &entry, PRINTER_A, &spool),
            request("op-b", &entry, PRINTER_B, &spool),
        ],
    );

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let loser = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert!(
        matches!(loser, RepositoryError::QueueEntryActionNotAllowed { .. }),
        "{loser:?}"
    );
    assert_eq!(rig.count("SELECT COUNT(*) FROM jobs"), 1);
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM spool_reservations WHERE state = 'active'"),
        1
    );
    assert_eq!(rig.available_mg(&spool), 1_000_000 - SLR_ESTIMATE_MG);
}

#[test]
fn concurrent_assigns_of_two_entries_to_one_printer_yield_one_job() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 2);

    let results = race(
        &rig,
        vec![
            request("op-a", &id(&entries[0]), PRINTER_A, &spool),
            request("op-b", &id(&entries[1]), PRINTER_A, &spool),
        ],
    );

    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    let loser = results
        .iter()
        .find_map(|result| result.as_ref().err())
        .unwrap();
    assert!(
        matches!(loser, RepositoryError::JobActive { .. }),
        "{loser:?}"
    );
    assert_eq!(rig.count("SELECT COUNT(*) FROM jobs"), 1);
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM queue_entries WHERE state = 'queued'"),
        1
    );
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM operations WHERE kind = 'assignQueueEntry'"),
        1,
        "the loser's id is not burned"
    );
}

#[test]
fn release_cancels_the_job_releases_the_reservation_and_replaces_the_entry_in_place() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 3);
    let middle = id(&entries[1]);
    let job = id(&rig.assign("op-assign", &middle, PRINTER_A, &spool).unwrap()["jobs"][0]);
    let before = rig.queue_events().len();

    let change = rig.job_command("release_job", "op-release", &job).unwrap();

    let released = &change["jobs"][0];
    assert_eq!(released["state"], "cancelled");
    assert_eq!(released["cancelReason"], "releasedBeforeStart");
    assert_eq!(released["settlement"], "notRequired");
    assert!(released["endedAt"].is_string());
    let rows = change["entries"].as_array().unwrap();
    assert_eq!(rows[0]["id"], json!(middle));
    assert_eq!(rows[0]["state"], "closed");
    assert_eq!(rows[0]["closeReason"], "released");
    assert_eq!(rows[0]["position"], Value::Null);
    let replacement = &rows[1];
    assert_eq!(replacement["state"], "queued");
    assert_eq!(
        replacement["position"], 2,
        "the replacement takes the freed slot"
    );
    assert_eq!(replacement["originEntryId"], json!(middle));
    assert_eq!(replacement["originKind"], "release");
    assert_eq!(replacement["lineageId"], entries[1]["lineageId"]);
    assert_eq!(
        replacement["copyIndex"], 2,
        "it keeps its origin's copy index"
    );

    assert_eq!(rig.available_mg(&spool), 1_000_000);
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM spool_reservations WHERE state = 'released'"),
        1
    );
    // Nothing else moved.
    assert_eq!(rig.entry(&id(&entries[0]))["position"], 1);
    assert_eq!(rig.entry(&id(&entries[2]))["position"], 3);
    let kinds: Vec<String> = rig.queue_events()[before..]
        .iter()
        .map(|event| event["type"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        kinds,
        [
            "queue.entry.changed",
            "queue.entry.changed",
            "queue.job.changed"
        ]
    );

    let history = rig.call("get_job_history", json!({"jobId": job})).unwrap();
    let events = history["events"].as_array().unwrap();
    assert_eq!(events[1]["kind"], "released");
    assert_eq!(events[1]["operationId"], "op-release");

    // Replayed: same rows, nothing published.
    let events_before = rig.event_count();
    let replayed = rig.job_command("release_job", "op-release", &job).unwrap();
    assert_eq!(replayed["entries"][1]["id"], replacement["id"]);
    assert_eq!(rig.event_count(), events_before);
}

#[test]
fn release_after_start_handoff_is_job_action_not_allowed() {
    let rig = Rig::new();
    let (entry, job, _spool) = assigned(&rig);
    rig.storage
        .write_repo(|tx| {
            jobs_repository::transition(
                tx,
                &job,
                JobEventKind::StageHandedOff,
                JobChange::default(),
                NOW,
            )
        })
        .unwrap();

    let refused = rig
        .job_command("release_job", "op-release", &job)
        .unwrap_err();

    assert_eq!(refused["code"], "JOB_ACTION_NOT_ALLOWED");
    assert_eq!(
        refused["message"],
        "This Job can't release while it is staging."
    );
    assert_eq!(refused["recovery"], json!(["RELOAD"]));
    assert_eq!(
        refused["details"],
        json!({"jobId": job, "action": "release", "state": "staging"})
    );
    assert_eq!(rig.entry(&entry)["state"], "assigned");
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM operations WHERE id = 'op-release'"),
        0
    );
}

#[test]
fn cancel_before_start_closes_the_entry_and_releases_the_reservation() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 2);
    let job = id(&rig
        .assign("op-assign", &id(&entries[0]), PRINTER_A, &spool)
        .unwrap()["jobs"][0]);

    let change = rig.job_command("cancel_job", "op-cancel", &job).unwrap();

    assert_eq!(change["jobs"][0]["state"], "cancelled");
    assert_eq!(change["jobs"][0]["cancelReason"], "cancelledBeforeStart");
    assert_eq!(change["jobs"][0]["settlement"], "notRequired");
    let rows = change["entries"].as_array().unwrap();
    assert_eq!(rows[0]["closeReason"], "cancelled");
    assert_eq!(rows[1]["id"], entries[1]["id"]);
    assert_eq!(rows[1]["position"], 1, "later entries move up");
    assert_eq!(rig.available_mg(&spool), 1_000_000);

    let again = rig
        .job_command("cancel_job", "op-cancel-2", &job)
        .unwrap_err();
    assert_eq!(again["code"], "JOB_ACTION_NOT_ALLOWED");
    assert_eq!(again["details"]["state"], "cancelled");
}

#[test]
fn retry_creates_a_linked_entry_at_the_end_and_leaves_history_untouched() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 2);
    let first = id(&entries[0]);
    let job = id(&rig.assign("op-assign", &first, PRINTER_A, &spool).unwrap()["jobs"][0]);
    rig.job_command("cancel_job", "op-cancel", &job).unwrap();
    let job_before = rig.call("get_job_history", json!({"jobId": job})).unwrap();

    let change = rig.job_command("retry_job", "op-retry", &job).unwrap();

    let retry = &change["entries"][0];
    assert_eq!(change["entries"].as_array().unwrap().len(), 1);
    assert_eq!(change["jobs"], json!([]));
    assert_eq!(retry["state"], "queued");
    assert_eq!(retry["originEntryId"], json!(first));
    assert_eq!(retry["originKind"], "retry");
    assert_eq!(retry["lineageId"], entries[0]["lineageId"]);
    assert_eq!(retry["copyIndex"], 1);
    assert_eq!(retry["position"], 2, "at the end of the open entries");
    assert_eq!(retry["policy"], "recommended");

    let job_after = rig.call("get_job_history", json!({"jobId": job})).unwrap();
    // The row is untouched; only the read-time `allowedActions` changes:
    // `retry` disappears once the Job has been retried (D3).
    assert_eq!(job_before["job"]["allowedActions"], json!(["retry"]));
    assert_eq!(job_after["job"]["allowedActions"], json!([]));
    let without_actions = |job: &Value| {
        let mut job = job.clone();
        job.as_object_mut().unwrap().remove("allowedActions");
        job
    };
    assert_eq!(
        without_actions(&job_after["job"]),
        without_actions(&job_before["job"]),
        "the Job is untouched"
    );
    assert_eq!(job_after["events"], job_before["events"]);
    assert_eq!(job_after["entry"]["closeReason"], "cancelled");
    assert_eq!(
        job_after["lineage"].as_array().unwrap().len(),
        3,
        "the lineage holds both copies and the retry"
    );
}

#[test]
fn second_retry_of_the_same_job_is_job_already_retried() {
    let rig = Rig::new();
    let (_entry, job, _spool) = assigned(&rig);
    rig.job_command("cancel_job", "op-cancel", &job).unwrap();
    let first = rig.job_command("retry_job", "op-retry", &job).unwrap();

    let second = rig
        .job_command("retry_job", "op-retry-2", &job)
        .unwrap_err();

    assert_eq!(second["code"], "JOB_ALREADY_RETRIED");
    assert_eq!(second["message"], "This Job was already retried.");
    assert_eq!(
        second["details"],
        json!({"jobId": job, "retryEntryId": first["entries"][0]["id"]})
    );
    // The same id replays.
    let replayed = rig.job_command("retry_job", "op-retry", &job).unwrap();
    assert_eq!(replayed["entries"][0]["id"], first["entries"][0]["id"]);
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM queue_entries WHERE origin_kind = 'retry'"),
        1
    );
}

#[test]
fn a_released_or_active_job_cannot_be_retried() {
    let rig = Rig::new();
    let (_entry, job, _spool) = assigned(&rig);

    let active = rig.job_command("retry_job", "op-retry", &job).unwrap_err();
    assert_eq!(active["code"], "JOB_ACTION_NOT_ALLOWED");
    assert_eq!(active["details"]["state"], "assigned");

    rig.job_command("release_job", "op-release", &job).unwrap();
    let released = rig.job_command("retry_job", "op-retry", &job).unwrap_err();
    assert_eq!(released["code"], "JOB_ACTION_NOT_ALLOWED");
    assert_eq!(released["details"]["action"], "retry");
}

#[test]
fn three_copies_are_independently_assignable_releasable_and_retryable() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 3);
    let (one, two, three) = (id(&entries[0]), id(&entries[1]), id(&entries[2]));

    let job_one = id(&rig.assign("op-a1", &one, PRINTER_A, &spool).unwrap()["jobs"][0]);
    let job_two = id(&rig.assign("op-a2", &two, PRINTER_B, &spool).unwrap()["jobs"][0]);
    assert_eq!(rig.entry(&three)["state"], "queued", "copy 3 is untouched");

    let released = rig.job_command("release_job", "op-r1", &job_one).unwrap();
    let replacement = id(&released["entries"][1]);
    assert_eq!(rig.entry(&two)["state"], "assigned", "copy 2 keeps its Job");

    rig.job_command("cancel_job", "op-c2", &job_two).unwrap();
    let retried = rig.job_command("retry_job", "op-t2", &job_two).unwrap();
    assert_eq!(retried["entries"][0]["copyIndex"], 2);

    let job_three = id(&rig.assign("op-a3", &three, PRINTER_A, &spool).unwrap()["jobs"][0]);
    assert_ne!(job_three, job_one);

    let lineage = entries[0]["lineageId"].as_str().unwrap();
    let snapshot = rig.list();
    let in_lineage: Vec<&Value> = snapshot["entries"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|entry| entry["lineageId"] == lineage)
        .collect();
    assert_eq!(in_lineage.len(), 5, "3 copies, 1 replacement, 1 retry");
    assert!(in_lineage.iter().all(|entry| entry["copyCount"] == 3));
    assert_eq!(rig.entry(&replacement)["copyIndex"], 1);
    assert_eq!(
        rig.available_mg(&spool),
        1_000_000 - SLR_ESTIMATE_MG,
        "only copy 3's Job holds material"
    );
}

#[test]
fn get_job_history_of_an_unknown_job_is_not_found() {
    let rig = Rig::new();

    let error = rig
        .call("get_job_history", json!({"jobId": "job-missing"}))
        .unwrap_err();

    assert_eq!(error["code"], "NOT_FOUND");
    assert_eq!(error["details"]["entityId"], "job-missing");
    for command in ["release_job", "cancel_job", "retry_job"] {
        let error = rig.job_command(command, "op", "job-missing").unwrap_err();
        assert_eq!(error["code"], "NOT_FOUND", "{command}");
    }
}

#[test]
fn events_carry_no_credential() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 2);
    let job = id(&rig
        .assign("op-a", &id(&entries[0]), PRINTER_A, &spool)
        .unwrap()["jobs"][0]);
    rig.job_command("release_job", "op-r", &job).unwrap();
    rig.job_command("retry_job", "op-t", &job).unwrap_err();
    let job_two = id(&rig
        .assign("op-b", &id(&entries[1]), PRINTER_B, &spool)
        .unwrap()["jobs"][0]);
    rig.job_command("cancel_job", "op-c", &job_two).unwrap();
    rig.job_command("retry_job", "op-t2", &job_two).unwrap();
    let error = rig
        .assign("op-x", &id(&entries[1]), PRINTER_A, &spool)
        .unwrap_err();
    let snapshot = rig.list();
    let history = rig
        .call("get_job_history", json!({"jobId": job_two}))
        .unwrap();

    let credential_ref = "farm3d/printer/prn-a/apikey";
    assert!(!rig.queue_events().is_empty());
    for event in rig.events.lock().unwrap().iter() {
        assert!(!event.contains(SECRET), "event leaks the secret");
        assert!(!event.contains(credential_ref), "event leaks the reference");
    }
    for text in [snapshot.to_string(), history.to_string(), error.to_string()] {
        assert!(!text.contains(SECRET));
        assert!(!text.contains(credential_ref));
        assert!(
            !text.contains("192.0.2."),
            "no endpoint in Queue or Job rows"
        );
    }
    let persisted: String = rig
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT (SELECT group_concat(printer_snapshot_json) FROM jobs)
                     || (SELECT group_concat(COALESCE(detail_json, '') || operation_id) FROM job_events)",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert!(!persisted.contains(SECRET));
    assert!(!persisted.contains(credential_ref));
}

// --- Task 8a: handoffs and the dispatch driver ---------------------------------

mod dispatch {
    use std::time::Duration;

    use farm3d_lib::host_ops::HostOperationState;
    use farm3d_lib::printers::operational::OperationalState;
    use farm3d_lib::printers::StartSafety;
    use serde_json::{json, Value};

    use crate::common::fake_moonraker::{Fault, Route, StartTrace};
    use crate::p7_dispatch_rig::{boot, id, Driver, Roots, Running, HOST_PATH, PRINTER, SECRET};

    fn strings(value: &Value) -> Vec<String> {
        value
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item.as_str().unwrap().to_string())
            .collect()
    }

    fn codes(blockers: &Value) -> Vec<String> {
        blockers
            .as_array()
            .unwrap()
            .iter()
            .map(|blocker| blocker["code"].as_str().unwrap().to_string())
            .collect()
    }

    fn settle() {
        std::thread::sleep(Duration::from_millis(400));
    }

    fn started(start_safety: StartSafety) -> (Roots, Running) {
        let roots = Roots::new(start_safety);
        let app = boot(&roots, Driver::Started);
        (roots, app)
    }

    #[test]
    fn assignment_stages_immediately_and_reaches_awaiting_start() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let spool = app.spool();

        let job_id = app.assign(&spool);
        let job = app.wait_job(&job_id, "awaitingStart");

        assert_eq!(roots.uploads(), 1);
        let ops = app.ops(&job_id);
        assert_eq!(ops.len(), 1);
        assert_eq!(ops[0].state, HostOperationState::Succeeded);
        assert_eq!(job["uploadHostOperationId"], json!(ops[0].id));
        assert_eq!(job["activeHostOperationId"], Value::Null);
        assert_eq!(job["hostPath"], HOST_PATH);
        assert_eq!(job["lastFailure"], Value::Null);
        assert_eq!(
            app.event_kinds(&job_id),
            ["assigned", "stageHandedOff", "stageSucceeded"]
        );
        // D4: the driver's own ids, the Host Operation's derived from it.
        let ledger = app
            .text(&format!(
                "SELECT operation_id FROM job_events WHERE job_id = '{job_id}' AND kind = 'stageHandedOff'"
            ))
            .unwrap();
        assert!(ledger.starts_with("drv-"), "{ledger}");
        assert_eq!(
            app.scalar(&format!(
                "SELECT COUNT(*) FROM operations WHERE id = '{ledger}' AND kind = 'stageJob'"
            )),
            1
        );
        assert_eq!(
            app.scalar(&format!(
                "SELECT COUNT(*) FROM operations WHERE id = '{ledger}#hostOperation' AND kind = 'stageSliceRevision'"
            )),
            1
        );
        // C1: Rust computes what's offered and what blocks the start.
        assert_eq!(
            strings(&job["allowedActions"]),
            ["stage", "start", "cancel", "release"]
        );
        assert_eq!(codes(&job["startBlockers"]), ["SPOOL_NOT_LOADED"]);
        assert!(job["startBlockers"][0]["message"]
            .as_str()
            .unwrap()
            .starts_with("Awaiting material: load Spool #"));
        // The Job was published as it moved.
        let published: Vec<Value> = app
            .job_events(&job_id)
            .iter()
            .map(|job| job["state"].clone())
            .collect();
        assert!(published.contains(&json!("staging")), "{published:?}");
        assert!(published.contains(&json!("awaitingStart")), "{published:?}");
        // No second upload, ever.
        settle();
        assert_eq!(roots.uploads(), 1);
    }

    #[test]
    fn upload_failure_returns_the_job_to_assigned_with_last_failure() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        roots.fake.fault(Route::Upload, Fault::checksum_mismatch());
        let spool = app.spool();

        let job_id = app.assign(&spool);
        let upload = app.wait_op(&app.first_op(&job_id).id, |row| {
            row.state != HostOperationState::Dispatching
        });
        assert_eq!(upload.state, HostOperationState::Failed);
        let job = app.wait_job(&job_id, "assigned");
        assert_eq!(job["lastFailure"]["kind"], "hostOperationFailed");
        assert_eq!(job["lastFailure"]["hostOperationId"], json!(upload.id));
        assert_eq!(job["lastFailure"]["failure"]["code"], "checksumRejected");
        assert_eq!(job["uploadHostOperationId"], Value::Null);
        assert_eq!(job["activeHostOperationId"], Value::Null);
        assert_eq!(
            app.event_kinds(&job_id),
            ["assigned", "stageHandedOff", "stageFailed"]
        );
        assert!(strings(&job["allowedActions"]).contains(&"stage".to_string()));

        // The driver never stages again by itself...
        settle();
        assert_eq!(roots.uploads(), 1);
        // ...the operator does.
        let change = app.job_command("stage_job", "op-restage", &job_id).unwrap();
        assert_eq!(change["jobs"][0]["state"], "staging");
        let job = app.wait_job(&job_id, "awaitingStart");
        assert_eq!(job["lastFailure"], Value::Null);
        assert_eq!(roots.uploads(), 2);
    }

    #[test]
    fn a_refused_driver_stage_records_last_failure_and_never_retries() {
        // The Printer goes offline between the assignment and the driver's
        // stage (its first pass, here).
        let roots = Roots::new(StartSafety::ConfirmBedClear);
        let app = boot(&roots, Driver::Off);
        let spool = app.spool();
        let job_id = app.assign(&spool);
        app.status(OperationalState::Offline);
        farm3d_lib::start_jobs_runtime(&app.services, app.app.handle());

        let job = app.wait_job_until(&job_id, |job| job["lastFailure"] != Value::Null);
        assert_eq!(job["state"], "assigned");
        assert_eq!(job["lastFailure"]["kind"], "refused");
        assert_eq!(job["lastFailure"]["code"], "PRINTER_UNREACHABLE");
        assert!(job["revision"].as_i64().unwrap() > 1, "the revision goes up");
        assert_eq!(app.event_kinds(&job_id), ["assigned"], "no event row");
        assert!(app.ops(&job_id).is_empty());

        app.status(OperationalState::Ready);
        settle();
        assert_eq!(roots.uploads(), 0, "it never stages again by itself");
        assert_eq!(app.job(&job_id)["state"], "assigned");
    }

    #[test]
    fn start_requires_bed_clear_and_the_loaded_spool() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let spool = app.spool();
        let job_id = app.assign(&spool);
        app.wait_job(&job_id, "awaitingStart");

        let refused = app
            .call(
                "start_job",
                json!({"operationId": "op-start", "jobId": job_id, "priorState": "ready",
                       "acknowledgement": "hostStateUnknown"}),
            )
            .unwrap_err();
        assert_eq!(refused["code"], "VALIDATION", "{refused}");
        assert_eq!(refused["details"]["fieldPath"], "acknowledgement");

        let blocked = app.start("op-start", &job_id, "ready").unwrap_err();
        assert_eq!(blocked["code"], "JOB_START_BLOCKED", "{blocked}");
        assert_eq!(codes(&blocked["details"]["blockers"]), ["SPOOL_NOT_LOADED"]);
        assert_eq!(blocked["recovery"], json!(["LOAD_SPOOL"]));
        assert_eq!(
            app.scalar("SELECT COUNT(*) FROM operations WHERE id LIKE 'op-start%'"),
            0,
            "a refusal never burns the id"
        );

        app.load(&spool);
        assert_eq!(app.job(&job_id)["startBlockers"], json!([]));
        let change = app.start("op-start", &job_id, "ready").unwrap();
        assert_eq!(change["jobs"][0]["state"], "starting");
        assert_eq!(change["jobs"][0]["startConfirmation"], "bedClear");
        let job = app.wait_job(&job_id, "printing");
        assert!(job["startedAt"].is_string());
        assert_eq!(roots.starts(), 1);
        assert_eq!(
            app.event_kinds(&job_id)[3..],
            ["startHandedOff", "startSucceeded"]
        );
        assert_eq!(strings(&job["allowedActions"]), ["pause", "cancel"]);
        assert_eq!(
            app.scalar(
                "SELECT COUNT(*) FROM operations WHERE id = 'op-start#hostOperation' AND kind = 'startStagedArtifact'"
            ),
            1
        );
    }

    #[test]
    fn start_from_finished_requires_prior_state_finished() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.awaiting_start();
        roots.fake.with_state(|state| state.print_state = "complete".to_string());
        app.status(OperationalState::Finished);

        let refused = app.start("op-ready", &job_id, "ready").unwrap_err();
        assert_eq!(refused["code"], "START_PRECONDITION_CHANGED", "{refused}");
        assert_eq!(app.job(&job_id)["state"], "awaitingStart");

        app.start("op-finished", &job_id, "finished").unwrap();
        app.wait_job(&job_id, "printing");
        assert_eq!(roots.starts(), 1);
    }

    #[test]
    fn unattended_printer_starts_without_confirmation_only_from_ready() {
        let (roots, app) = started(StartSafety::Unattended);
        roots.fake.with_state(|state| state.print_state = "complete".to_string());
        app.status(OperationalState::Finished);
        let spool = app.spool();
        app.load(&spool);
        let job_id = app.assign(&spool);
        app.wait_job(&job_id, "awaitingStart");
        settle();
        assert_eq!(roots.starts(), 0, "never from finished");
        assert_eq!(app.job(&job_id)["state"], "awaitingStart");

        roots.fake.with_state(|state| state.print_state = "standby".to_string());
        app.status(OperationalState::Ready);
        let job = app.wait_job(&job_id, "printing");
        assert_eq!(job["startConfirmation"], "unattended");
        assert_eq!(roots.starts(), 1);
        let start = app
            .ops(&job_id)
            .into_iter()
            .find(|op| op.kind == farm3d_lib::host_ops::HostOperationKind::Start)
            .unwrap();
        let ledger = app
            .text(&format!(
                "SELECT operation_id FROM host_operations WHERE id = '{}'",
                start.id
            ))
            .unwrap();
        assert!(ledger.starts_with("drv-") && ledger.ends_with("#hostOperation"), "{ledger}");
        settle();
        assert_eq!(roots.starts(), 1);
    }

    #[test]
    fn confirm_bed_clear_printer_never_auto_starts() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.awaiting_start();
        app.status(OperationalState::Ready);
        settle();
        assert_eq!(roots.starts(), 0);
        assert_eq!(app.job(&job_id)["state"], "awaitingStart");
    }

    #[test]
    fn start_blockers_are_republished_when_the_printers_status_changes() {
        let (_roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.awaiting_start();
        let before = app.job_events(&job_id).len();

        app.status(OperationalState::Printing);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        loop {
            let published = app.job_events(&job_id);
            if published.len() > before
                && codes(&published.last().unwrap()["startBlockers"]) == ["PRINTER_NOT_READY"]
            {
                break;
            }
            assert!(std::time::Instant::now() < deadline, "{published:?}");
            std::thread::sleep(Duration::from_millis(20));
        }
        let listed = app.ok("list_queue", json!({}));
        let job = listed["jobs"]
            .as_array()
            .unwrap()
            .iter()
            .find(|job| job["id"] == json!(job_id))
            .unwrap()
            .clone();
        assert_eq!(codes(&job["startBlockers"]), ["PRINTER_NOT_READY"]);
        assert_eq!(
            job["startBlockers"][0]["message"],
            "The printer can't start now: it is printing."
        );
    }

    #[test]
    fn pause_resume_cancel_through_the_job_link_the_host_operations() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.printing();

        let paused = app.job_command("pause_job", "op-pause", &job_id).unwrap();
        let pause_op = paused["jobs"][0]["activeHostOperationId"].as_str().unwrap().to_string();
        assert_eq!(app.row(&pause_op).job_id.as_deref(), Some(job_id.as_str()));
        app.wait_resolved(&pause_op, HostOperationState::Succeeded);
        let job = app.wait_job(&job_id, "paused");
        assert_eq!(job["activeHostOperationId"], Value::Null);
        assert_eq!(strings(&job["allowedActions"]), ["resume", "cancel"]);
        app.status(OperationalState::Paused);

        app.job_command("resume_job", "op-resume", &job_id).unwrap();
        app.wait_job(&job_id, "printing");
        app.status(OperationalState::Printing);

        let cancelled = app.job_command("cancel_job", "op-cancel", &job_id).unwrap();
        let cancel_op = cancelled["jobs"][0]["activeHostOperationId"]
            .as_str()
            .unwrap()
            .to_string();
        assert_eq!(cancelled["jobs"][0]["state"], "printing");
        assert_eq!(cancelled["entries"], json!([]), "the entry stays open");
        app.wait_resolved(&cancel_op, HostOperationState::Succeeded);
        // A succeeded cancel only clears the active op: the tracker proves
        // the end from history (Task 8b).
        app.wait_op(&cancel_op, |_| {
            app.job(&job_id)["activeHostOperationId"] == Value::Null
        });
        assert_eq!(app.job(&job_id)["state"], "printing");
        assert_eq!(
            app.event_kinds(&job_id)[5..],
            ["pauseHandedOff", "paused", "resumeHandedOff", "resumed", "cancelHandedOff"]
        );
        for (path, count) in [
            ("/printer/print/pause", 1),
            ("/printer/print/resume", 1),
            ("/printer/print/cancel", 1),
        ] {
            assert_eq!(roots.count_requests("POST", path), count, "{path}");
        }
        for (operation, kind) in [
            ("op-pause", "pauseJob"),
            ("op-resume", "resumeJob"),
            ("op-cancel", "cancelJob"),
        ] {
            assert_eq!(
                app.scalar(&format!(
                    "SELECT COUNT(*) FROM operations WHERE id = '{operation}' AND kind = '{kind}'"
                )),
                1,
                "{operation}"
            );
        }
    }

    #[test]
    fn control_on_another_file_is_job_not_on_printer_and_sends_nothing() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.printing();
        roots
            .fake
            .with_state(|state| state.print_filename = "someone/else.gcode".to_string());

        let refused = app.job_command("pause_job", "op-pause", &job_id).unwrap_err();
        assert_eq!(refused["code"], "JOB_NOT_ON_PRINTER", "{refused}");
        assert_eq!(refused["details"]["jobId"], json!(job_id));
        assert_eq!(refused["details"]["printerId"], PRINTER);
        assert_eq!(roots.count_requests("POST", "/printer/print/pause"), 0);
        assert_eq!(
            app.scalar("SELECT COUNT(*) FROM host_operations WHERE kind = 'pause'"),
            0
        );
        assert_eq!(
            app.scalar("SELECT COUNT(*) FROM operations WHERE id LIKE 'op-pause%'"),
            0
        );
        assert_eq!(app.job(&job_id)["state"], "printing");
    }

    #[test]
    fn job_actions_outside_the_state_table_are_job_action_not_allowed() {
        let (_roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.awaiting_start();
        for (command, action) in [("pause_job", "pause"), ("resume_job", "resume")] {
            let refused = app.job_command(command, "op-x", &job_id).unwrap_err();
            assert_eq!(refused["code"], "JOB_ACTION_NOT_ALLOWED", "{refused}");
            assert_eq!(refused["details"]["action"], action);
            assert_eq!(refused["details"]["state"], "awaitingStart");
        }
        let missing = app.job_command("stage_job", "op-y", "job-missing").unwrap_err();
        assert_eq!(missing["code"], "NOT_FOUND");
    }

    #[test]
    fn stage_again_from_awaiting_start_after_staged_artifact_invalid() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.awaiting_start();
        let first_upload = app.job(&job_id)["uploadHostOperationId"].clone();
        roots.fake.with_state(|state| {
            state.files.clear();
        });

        let refused = app.start("op-start-1", &job_id, "ready").unwrap_err();
        assert_eq!(refused["code"], "STAGED_ARTIFACT_INVALID", "{refused}");
        assert_eq!(app.job(&job_id)["state"], "awaitingStart");

        let change = app.job_command("stage_job", "op-restage", &job_id).unwrap();
        assert_eq!(change["jobs"][0]["state"], "staging");
        let job = app.wait_job(&job_id, "awaitingStart");
        assert_ne!(job["uploadHostOperationId"], first_upload);
        assert_eq!(roots.uploads(), 2);

        app.start("op-start-2", &job_id, "ready").unwrap();
        app.wait_job(&job_id, "printing");
        assert_eq!(roots.starts(), 1);
    }

    #[test]
    fn linked_upload_abandoned_via_p6_returns_the_job_to_assigned() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        roots.fake.fault(Route::Upload, Fault::DropMidBody);
        let spool = app.spool();
        let job_id = app.assign(&spool);
        let upload = app.wait_op(&app.first_op(&job_id).id, |row| {
            row.state == HostOperationState::Uncertain
        });
        assert_eq!(app.job(&job_id)["state"], "staging");
        assert_eq!(app.job(&job_id)["allowedActions"], json!([]));

        // P6's own exits are not JOB_ACTIVE-guarded.
        app.ok("reconcile_host_operation", json!({"hostOperationId": upload.id}));
        app.wait_op(&upload.id, |row| {
            row.state == HostOperationState::Uncertain && row.attempts >= 1
        });
        app.ok(
            "abandon_host_operation",
            json!({"operationId": "op-abandon", "hostOperationId": upload.id,
                   "acknowledgement": "hostStateUnknown"}),
        );
        let job = app.wait_job(&job_id, "assigned");
        assert_eq!(job["lastFailure"]["kind"], "hostOperationAbandoned");
        assert_eq!(job["lastFailure"]["hostOperationId"], json!(upload.id));
        assert_eq!(job["activeHostOperationId"], Value::Null);
        assert_eq!(
            app.event_kinds(&job_id),
            ["assigned", "stageHandedOff", "stageFailed"]
        );
    }

    #[test]
    fn abandoned_start_makes_the_job_outcome_unknown_with_a_requirement() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.awaiting_start();
        roots
            .fake
            .fault(Route::Start, Fault::ApplyStartThenDrop(StartTrace::PrintingOnly));

        let change = app.start("op-start", &job_id, "ready").unwrap();
        let start_op = change["jobs"][0]["activeHostOperationId"].as_str().unwrap().to_string();
        app.wait_op(&start_op, |row| row.state == HostOperationState::Uncertain);
        roots.fake.set_reachable(false);
        app.ok("reconcile_host_operation", json!({"hostOperationId": start_op}));
        app.wait_op(&start_op, |row| {
            row.state == HostOperationState::Uncertain && row.attempts >= 1
        });
        app.ok(
            "abandon_host_operation",
            json!({"operationId": "op-abandon", "hostOperationId": start_op,
                   "acknowledgement": "hostStateUnknown"}),
        );

        let job = app.wait_job(&job_id, "outcomeUnknown");
        assert_eq!(strings(&job["allowedActions"]), ["declareOutcome"]);
        assert_eq!(job["settlement"], "open", "the reservation stays active");
        let requirements = app.history(&job_id)["requirements"].clone();
        assert_eq!(requirements.as_array().unwrap().len(), 1);
        assert_eq!(requirements[0]["kind"], "jobOutcomeUnknown");
        assert_eq!(requirements[0]["status"], "pending");
        assert_eq!(roots.starts(), 1, "no second start");
        roots.fake.set_reachable(true);
    }

    #[test]
    fn job_commands_replay_and_reuse_follow_the_operations_ledger() {
        let (roots, app) = started(StartSafety::ConfirmBedClear);
        roots.fake.fault(Route::Upload, Fault::checksum_mismatch());
        let spool = app.spool();
        app.load(&spool);
        let job_id = app.assign(&spool);
        app.wait_job_until(&job_id, |job| job["lastFailure"] != Value::Null);

        let first = app.job_command("stage_job", "op-stage", &job_id).unwrap();
        app.wait_job(&job_id, "awaitingStart");
        let events = app.job_events(&job_id).len();
        let replayed = app.job_command("stage_job", "op-stage", &job_id).unwrap();
        assert_eq!(replayed["jobs"][0]["id"], first["jobs"][0]["id"]);
        assert_eq!(replayed["jobs"][0]["state"], "awaitingStart", "the current rows");
        assert_eq!(roots.uploads(), 2, "a replay never re-stages");
        std::thread::sleep(Duration::from_millis(100));
        assert_eq!(app.job_events(&job_id).len(), events, "a replay publishes nothing");

        // The same id for another request is VALIDATION on operationId.
        for (command, body) in [
            ("pause_job", json!({"operationId": "op-stage", "jobId": job_id})),
            ("stage_job", json!({"operationId": "op-stage", "jobId": "job-other"})),
            (
                "start_job",
                json!({"operationId": "op-stage", "jobId": job_id, "priorState": "ready",
                       "acknowledgement": "bedClear"}),
            ),
        ] {
            let reused = app.call(command, body).unwrap_err();
            assert_eq!(reused["code"], "VALIDATION", "{command}: {reused}");
            assert_eq!(reused["details"]["fieldPath"], "operationId", "{command}");
        }
        // A Job command never reuses the Host Operation's derived id.
        let derived = app
            .job_command("stage_job", "op-stage#hostOperation", &job_id)
            .unwrap_err();
        assert_eq!(derived["code"], "VALIDATION", "{derived}");

        app.start("op-go", &job_id, "ready").unwrap();
        app.wait_job(&job_id, "printing");
        let replayed = app.start("op-go", &job_id, "ready").unwrap();
        assert_eq!(replayed["jobs"][0]["state"], "printing");
        assert_eq!(roots.starts(), 1, "a replayed start sends nothing");
    }

    #[test]
    fn dispatch_rows_and_events_never_carry_the_credential() {
        let (_roots, app) = started(StartSafety::ConfirmBedClear);
        let job_id = app.printing();
        app.job_command("pause_job", "op-pause", &job_id).unwrap();
        app.wait_job(&job_id, "paused");
        let history = app.history(&job_id).to_string();
        assert!(!history.contains(SECRET));
        for event in app.events.lock().unwrap().iter() {
            assert!(!event.contains(SECRET), "event leaks the secret");
        }
        let persisted: String = app
            .text(
                "SELECT COALESCE((SELECT group_concat(COALESCE(last_failure_json, '')) FROM jobs), '')
                     || (SELECT group_concat(COALESCE(detail_json, '') || COALESCE(operation_id, '')) FROM job_events)",
            )
            .unwrap();
        assert!(!persisted.contains(SECRET));
        let _ = id;
    }
}
