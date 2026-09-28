//! P7 Task 6 (spec D2, D4, "Commands", "Events", ruling R3): the six Queue
//! commands through the Tauri IPC path — Add to Queue with its estimate
//! and policy rules, update, move, remove, explain, and `list_queue`'s
//! listen-before-backfill snapshot with synchronous eligibility.

mod common;
mod p7_rig;

use p7_rig::{Rig, EXTERNAL_CLAIM_MG, PRINTER_A, SLR, SLR_ESTIMATE_MG, SLR_EXTERNAL};
use serde_json::{json, Value};

fn field_path(error: &Value) -> &str {
    error["details"]["fieldPath"].as_str().unwrap_or_default()
}

#[test]
fn add_to_queue_with_quantity_three_creates_three_linked_entries() {
    let rig = Rig::new();

    let entries = rig.add("op-add", 3);

    assert_eq!(entries.len(), 3);
    let lineage = entries[0]["lineageId"].as_str().unwrap();
    assert!(lineage.starts_with("qln-"));
    for (index, entry) in entries.iter().enumerate() {
        assert_eq!(entry["lineageId"], lineage, "one lineage");
        assert_eq!(entry["copyIndex"], json!(index + 1));
        assert_eq!(entry["copyCount"], 3);
        assert_eq!(entry["position"], json!(index + 1));
        assert_eq!(entry["state"], "queued");
        assert_eq!(entry["sliceRevisionId"], SLR);
        assert_eq!(
            entry["estimate"],
            json!({"amountMg": SLR_ESTIMATE_MG, "source": "sliceEstimate"}),
            "a farm3d revision's estimate is filled in from filamentGrams"
        );
        assert_eq!(
            entry["allowedActions"],
            json!(["assign", "update", "move", "remove"])
        );
    }
    let events = rig.queue_events();
    assert_eq!(events.len(), 3, "one queue.entry.changed per entry");
    assert!(events
        .iter()
        .all(|event| event["type"] == "queue.entry.changed"
            && event["subject"]["kind"] == "queueEntry"));
}

#[test]
fn add_to_queue_replay_returns_the_same_entries_and_publishes_nothing() {
    let rig = Rig::new();
    let first = rig.add("op-add", 2);
    let events = rig.event_count();

    let replayed = rig.add("op-add", 2);

    assert_eq!(
        replayed
            .iter()
            .map(|entry| &entry["id"])
            .collect::<Vec<_>>(),
        first.iter().map(|entry| &entry["id"]).collect::<Vec<_>>()
    );
    assert_eq!(rig.event_count(), events, "a replay publishes nothing");
    assert_eq!(rig.count("SELECT COUNT(*) FROM queue_entries"), 2);

    let reused = rig
        .call(
            "add_to_queue",
            json!({
                "operationId": "op-add", "sliceRevisionId": SLR, "quantity": 1,
                "policy": "recommended", "preference": "loadedFirst",
            }),
        )
        .unwrap_err();
    assert_eq!(reused["code"], "VALIDATION");
    assert_eq!(field_path(&reused), "operationId");
}

#[test]
fn add_to_queue_without_an_estimate_for_an_external_revision_is_rejected() {
    let rig = Rig::new();
    let add = |operation_id: &str, estimate: Option<Value>| {
        let mut body = json!({
            "operationId": operation_id, "sliceRevisionId": SLR_EXTERNAL, "quantity": 1,
            "policy": "recommended", "preference": "loadedFirst",
        });
        if let Some(estimate) = estimate {
            body["materialEstimate"] = estimate;
        }
        rig.call("add_to_queue", body)
    };

    let missing = add("op-1", None).unwrap_err();
    assert_eq!(missing["code"], "VALIDATION");
    assert_eq!(field_path(&missing), "materialEstimate");

    let wrong_claim = add(
        "op-1",
        Some(json!({"amountMg": EXTERNAL_CLAIM_MG - 1, "source": "fileClaimConfirmed"})),
    )
    .unwrap_err();
    assert_eq!(field_path(&wrong_claim), "materialEstimate");
    assert_eq!(rig.count("SELECT COUNT(*) FROM queue_entries"), 0);
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM operations WHERE id = 'op-1'"),
        0,
        "a rejected command never burns its id"
    );

    let confirmed = add(
        "op-1",
        Some(json!({"amountMg": EXTERNAL_CLAIM_MG, "source": "fileClaimConfirmed"})),
    )
    .unwrap();
    assert_eq!(
        confirmed["entries"][0]["estimate"]["amountMg"],
        EXTERNAL_CLAIM_MG
    );
    let entered = add(
        "op-2",
        Some(json!({"amountMg": 5_000, "source": "operatorEntered"})),
    )
    .unwrap();
    assert_eq!(
        entered["entries"][0]["estimate"]["source"],
        "operatorEntered"
    );
}

#[test]
fn add_to_queue_validates_quantity_revision_and_pin() {
    let rig = Rig::new();
    let add = |body: Value| {
        let mut request = json!({
            "operationId": "op", "sliceRevisionId": SLR, "quantity": 1,
            "policy": "recommended", "preference": "loadedFirst",
        });
        for (key, value) in body.as_object().unwrap() {
            request[key] = value.clone();
        }
        rig.call("add_to_queue", request)
    };

    for quantity in [0, 51] {
        let error = add(json!({"quantity": quantity})).unwrap_err();
        assert_eq!(field_path(&error), "quantity", "quantity {quantity}");
    }
    let missing = add(json!({"sliceRevisionId": "slr-missing"})).unwrap_err();
    assert_eq!(missing["code"], "NOT_FOUND");
    let pinned_not_manual = add(json!({"manualPrinterId": PRINTER_A})).unwrap_err();
    assert_eq!(field_path(&pinned_not_manual), "manualPrinterId");
    let unknown_pin =
        add(json!({"policy": "manual", "manualPrinterId": "prn-missing"})).unwrap_err();
    assert_eq!(unknown_pin["code"], "NOT_FOUND");
    let wrong_estimate =
        add(json!({"materialEstimate": {"amountMg": 1, "source": "sliceEstimate"}})).unwrap_err();
    assert_eq!(field_path(&wrong_estimate), "materialEstimate");

    let pinned = add(json!({"policy": "manual", "manualPrinterId": PRINTER_A})).unwrap();
    assert_eq!(pinned["entries"][0]["manualPrinterId"], PRINTER_A);
}

#[test]
fn list_queue_snapshot_sequence_precedes_rows() {
    let rig = Rig::new();
    let before = rig.list();
    assert_eq!(before["snapshotSequence"], 0);
    assert_eq!(before["entries"], json!([]));

    rig.add("op-add", 2);
    let events = rig.queue_events();
    let last_sequence = events.last().unwrap()["sequence"].as_u64().unwrap();
    assert!(events
        .iter()
        .all(|event| event["sequence"].as_u64().unwrap() > 0
            && event["streamId"] == before["streamId"]));

    let after = rig.list();
    // Everything at or below `snapshotSequence` is in the rows; a change
    // the rows miss carries a larger sequence.
    assert_eq!(after["snapshotSequence"], json!(last_sequence));
    assert_eq!(after["entries"].as_array().unwrap().len(), 2);
    assert_eq!(after["streamId"], before["streamId"]);
}

#[test]
fn list_queue_computes_eligibility_synchronously_until_the_evaluator_runs() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entries = rig.add("op-add", 1);

    let snapshot = rig.list();

    assert_eq!(
        snapshot["nextAutomaticAction"],
        json!({"kind": "evaluatorNotRunning"})
    );
    let summaries = snapshot["eligibility"].as_array().unwrap();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0]["entryId"], entries[0]["id"]);
    assert_eq!(summaries[0]["verdict"], "awaitingOperator");
    assert_eq!(summaries[0]["eligibleCount"], 2);
    assert_eq!(summaries[0]["topBlocker"], Value::Null);

    let explained = rig
        .call("explain_queue_entry", json!({"entryId": entries[0]["id"]}))
        .unwrap();
    assert_eq!(explained["verdict"], "awaitingOperator");
    assert_eq!(explained["candidates"][0]["spool"]["spoolId"], json!(spool));

    let unknown = rig
        .call("explain_queue_entry", json!({"entryId": "qen-missing"}))
        .unwrap_err();
    assert_eq!(unknown["code"], "NOT_FOUND");
}

#[test]
fn list_queue_summary_names_the_top_blocker_when_no_spool_fits() {
    let rig = Rig::new();
    rig.add("op-add", 1);

    let summary = &rig.list()["eligibility"][0];

    assert_eq!(summary["verdict"], "blocked");
    assert_eq!(summary["eligibleCount"], 0);
    assert_eq!(summary["topBlocker"]["code"], "NO_COMPATIBLE_SPOOL");
}

#[test]
fn update_changes_policy_and_preference_and_refuses_automatic_on_a_pin() {
    let rig = Rig::new();
    let entry = &rig.add("op-add", 1)[0];

    let updated = rig
        .call(
            "update_queue_entry",
            json!({
                "operationId": "op-update", "entryId": entry["id"], "expectedRevision": 1,
                "policy": "automatic", "preference": "leastRecentlyUsed",
            }),
        )
        .unwrap();
    assert_eq!(updated["entries"][0]["policy"], "automatic");
    assert_eq!(updated["entries"][0]["preference"], "leastRecentlyUsed");
    assert_eq!(updated["entries"][0]["revision"], 2);

    let stale = rig
        .call(
            "update_queue_entry",
            json!({"operationId": "op-stale", "entryId": entry["id"], "expectedRevision": 1, "policy": "manual"}),
        )
        .unwrap_err();
    assert_eq!(stale["code"], "CONFLICT");

    let pinned = rig
        .call(
            "add_to_queue",
            json!({
                "operationId": "op-pin", "sliceRevisionId": SLR, "quantity": 1,
                "policy": "manual", "preference": "loadedFirst", "manualPrinterId": PRINTER_A,
            }),
        )
        .unwrap();
    let refused = rig
        .call(
            "update_queue_entry",
            json!({
                "operationId": "op-unpin", "entryId": pinned["entries"][0]["id"],
                "expectedRevision": 1, "policy": "automatic",
            }),
        )
        .unwrap_err();
    assert_eq!(field_path(&refused), "policy");
}

#[test]
fn move_renumbers_densely_and_rejects_a_position_outside_the_queue() {
    let rig = Rig::new();
    let entries = rig.add("op-add", 3);

    let moved = rig
        .call(
            "move_queue_entry",
            json!({"operationId": "op-move", "entryId": entries[2]["id"], "expectedRevision": 1, "toPosition": 1}),
        )
        .unwrap();
    let positions: Vec<(Value, Value)> = moved["entries"]
        .as_array()
        .unwrap()
        .iter()
        .map(|entry| (entry["id"].clone(), entry["position"].clone()))
        .collect();
    assert_eq!(
        positions,
        vec![
            (entries[2]["id"].clone(), json!(1)),
            (entries[0]["id"].clone(), json!(2)),
            (entries[1]["id"].clone(), json!(3)),
        ]
    );

    let outside = rig
        .call(
            "move_queue_entry",
            json!({"operationId": "op-move-2", "entryId": entries[0]["id"], "expectedRevision": 2, "toPosition": 4}),
        )
        .unwrap_err();
    assert_eq!(field_path(&outside), "toPosition");
}

#[test]
fn remove_closes_a_queued_entry_and_moves_later_entries_up() {
    let rig = Rig::new();
    let entries = rig.add("op-add", 3);

    let removed = rig
        .call(
            "remove_queue_entry",
            json!({"operationId": "op-remove", "entryId": entries[0]["id"], "expectedRevision": 1}),
        )
        .unwrap();

    let rows = removed["entries"].as_array().unwrap();
    assert_eq!(rows[0]["state"], "closed");
    assert_eq!(rows[0]["closeReason"], "removed");
    assert_eq!(rows[0]["position"], Value::Null);
    assert_eq!(rows[1]["id"], entries[1]["id"]);
    assert_eq!(rows[1]["position"], 1);
    assert_eq!(rows[2]["position"], 2);

    let again = rig
        .call(
            "remove_queue_entry",
            json!({"operationId": "op-remove-2", "entryId": entries[0]["id"], "expectedRevision": 1}),
        )
        .unwrap_err();
    assert_eq!(again["code"], "QUEUE_ENTRY_ACTION_NOT_ALLOWED");
    assert_eq!(again["details"]["state"], "closed");
    assert_eq!(again["details"]["action"], "remove");
}

#[test]
fn removing_an_assigned_entry_points_at_its_job() {
    let rig = Rig::new();
    let spool = rig.spool(1_000_000);
    let entry = &rig.add("op-add", 1)[0];
    rig.assign(
        "op-assign",
        entry["id"].as_str().unwrap(),
        PRINTER_A,
        &spool,
    )
    .unwrap();

    let refused = rig
        .call(
            "remove_queue_entry",
            json!({"operationId": "op-remove", "entryId": entry["id"], "expectedRevision": 2}),
        )
        .unwrap_err();

    assert_eq!(refused["code"], "QUEUE_ENTRY_ACTION_NOT_ALLOWED");
    assert_eq!(
        refused["message"],
        "This Queue Entry is assigned. Release or cancel its Job instead."
    );
    assert_eq!(refused["recovery"], json!(["RELOAD"]));
    assert_eq!(refused["details"]["entryId"], entry["id"]);
}
