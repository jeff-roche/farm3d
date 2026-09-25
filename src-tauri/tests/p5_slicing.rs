//! P5 Task 8: slice operations, the `slicing` stream, the commands, and
//! restart recovery, through the Tauri IPC path against `fake-orca` (spec
//! D5, D9, D10, D13, D17, §Commands).
//!
//! Each test runs a [`p5_harness::Farm`]: `fake-orca` installed and
//! configured as the engine in `slicer_runtime_config`, with the TestVendor
//! preset fixtures beside it.
//!
//! Needs `--features test-support`, which builds `fake-orca`.

mod common;
mod p5_harness;

use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::persistence::Storage;
use farm3d_lib::slicing::invocation::WORK_ROOT_DIR;
use farm3d_lib::slicing::operations::{recover_after_restart, SchedulerPoint};
use farm3d_lib::slicing::repository::{load_runtime_config, save_runtime_config};
use farm3d_lib::slicing::runtime::DiscoveryEnv;
use serde_json::{json, Value};

use common::{a_ref_json, a_stored_printer};
use p5_harness::*;

/// The 21 P5 commands.
const P5_COMMANDS: &[&str] = &[
    "get_slicer_runtime",
    "check_slicer_runtime",
    "pick_slicer_engine",
    "pick_preset_source",
    "reset_slicer_runtime",
    "list_slice_options",
    "get_revision_geometry",
    "get_revision_mesh",
    "list_slicing",
    "create_preparation",
    "update_preparation",
    "reload_preparation",
    "delete_preparation",
    "start_slice",
    "cancel_slice_operation",
    "get_slice_operation_log",
    "list_slice_revisions",
    "get_slice_revision",
    "get_slice_revision_log",
    "create_external_slice_revision",
    "delete_slice_revision",
];

/// D16: `create_external_slice_revision`'s `facts` with every fact
/// confirmed to a fixed, arbitrary value, regardless of what the source
/// G-code itself claims. The numbers (0.6, 2.85) and family (PETG) are
/// deliberately values none of the G-code library fixtures claim (they
/// only ever claim `nozzle_diameter = 0.4`, `filament_type = PLA`), so a
/// claim-to-fact leak would show up as a mismatch rather than an
/// accidental match.
fn confirmed_facts() -> Value {
    json!({
        "printerProfile": {
            "kind": "confirmed",
            "value": { "kind": "profile", "catalogRef": a_ref_json() },
        },
        "nozzleDiameterMm": { "kind": "confirmed", "value": 0.6 },
        "materialFamily": { "kind": "confirmed", "value": "PETG" },
        "filamentDiameterMm": { "kind": "confirmed", "value": 2.85 },
    })
}

#[test]
fn every_p5_command_is_registered_with_a_contract() {
    let manifest = farm3d_lib::contracts::inventory::command_contract_inventory();
    assert_eq!(manifest.len(), farm3d_lib::COMMAND_NAMES.len());
    for command in P5_COMMANDS {
        assert!(
            farm3d_lib::COMMAND_NAMES.contains(command),
            "{command} missing from COMMAND_NAMES"
        );
        assert!(
            manifest.iter().any(|entry| entry.command == *command),
            "{command} missing from COMMAND_CONTRACTS"
        );
    }
}

#[test]
fn the_runtime_status_names_the_chosen_engine_and_why() {
    let farm = Farm::new();
    let running = farm.start();
    let status = running.ok("get_slicer_runtime", json!({}));
    assert_eq!(status["canSlice"], true, "{status}");
    assert_eq!(status["engine"]["state"], "available");
    assert_eq!(status["engine"]["source"], "configured");
    assert_eq!(status["engine"]["executableName"], "orca-slicer");
    assert_eq!(status["presetSource"]["origin"], "engine");
    assert_eq!(
        status["engineCandidates"],
        json!([{
            "source": "configured",
            "executableName": "orca-slicer",
            "path": farm.engine.to_str().unwrap(),
            "result": { "kind": "chosen", "version": "2.4.2" },
        }])
    );

    // A configured engine that fails its probe falls through to PATH, and
    // the status says so.
    let broken = farm.orca.path().join("broken");
    fs::write(&broken, "not a program").unwrap();
    running
        .services
        .storage
        .write_repo(|tx| {
            let current = load_runtime_config(tx)?;
            save_runtime_config(tx, current.revision, Some(broken.to_str().unwrap()), None)
        })
        .unwrap();
    running.services.slicing.set_discovery_env(DiscoveryEnv {
        home: None,
        path_var: Some(farm.orca.path().join("bin").into_os_string()),
        probe_timeout: Duration::from_secs(10),
    });
    let status = running.ok("check_slicer_runtime", json!({}));
    assert_eq!(status["engine"]["state"], "available", "{status}");
    assert_eq!(status["engine"]["source"], "path");
    let candidates = status["engineCandidates"].as_array().unwrap();
    assert_eq!(candidates.len(), 2, "{status}");
    assert_eq!(candidates[0]["source"], "configured");
    assert_eq!(candidates[0]["executableName"], "broken");
    assert_eq!(candidates[0]["result"]["kind"], "probeFailed");
    assert_eq!(candidates[1]["source"], "path");
    assert_eq!(candidates[1]["result"]["kind"], "chosen");
    assert!(
        running
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"] == "slicing.runtime.changed"
                && event["payload"]["engine"]["source"] == "path"),
        "the change goes out on the stream"
    );
}

#[test]
fn a_two_plate_preparation_slices_into_two_revisions_of_one_source() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-two-plates.3mf", "two.3mf"), "managed");
    let model_id = model["id"].as_str().unwrap();
    let source_revision = model["currentRevision"]["id"].clone();

    let preparation = running.prepare(model_id);
    assert_eq!(preparation["stale"], false);
    assert_eq!(preparation["sourceRevisionId"], source_revision);
    let plates = plates(&preparation);
    assert_eq!(plates.len(), 2, "one plate per Orca plate: {preparation}");
    assert!(preparation["document"]["processPreset"].is_string());
    assert!(preparation["document"]["filamentPreset"].is_string());
    // Asking again returns the same Preparation.
    assert_eq!(running.prepare(model_id)["id"], preparation["id"]);

    // Geometry and mesh transfer (D6).
    let geometry = running.ok(
        "get_revision_geometry",
        json!({ "revisionId": source_revision }),
    );
    let object_key = geometry["objects"][0]["objectKey"].as_u64().unwrap() as u32;
    let mesh = running.mesh(source_revision.as_str().unwrap(), object_key);
    assert_eq!(&mesh[..4], b"F3DM");

    let operations = running.start("op-two-plates", &preparation, &[&plates[0], &plates[1]]);
    assert_eq!(operations.len(), 2);
    assert!(operations
        .iter()
        .all(|operation| operation["state"] == "queued"));
    let succeeded: Vec<Value> = ids(&operations)
        .iter()
        .map(|id| running.wait_state(id, "succeeded"))
        .collect();

    let revisions = running.ok("list_slice_revisions", json!({ "modelId": model_id }));
    let revisions = revisions.as_array().unwrap();
    assert_eq!(revisions.len(), 2, "{revisions:?}");
    let mut plate_keys: Vec<&Value> = revisions
        .iter()
        .map(|revision| &revision["plate"]["plateKey"])
        .collect();
    plate_keys.sort_by_key(|key| key.to_string());
    plate_keys.dedup();
    assert_eq!(plate_keys.len(), 2, "distinct plate keys");
    assert!(revisions
        .iter()
        .all(|revision| revision["sourceRevisionId"] == source_revision));
    for operation in &succeeded {
        let revision_id = operation["sliceRevisionId"].as_str().unwrap();
        let record = running.ok(
            "get_slice_revision",
            json!({ "sliceRevisionId": revision_id }),
        );
        assert_eq!(record["plate"]["plateKey"], operation["plateKey"]);
        assert_eq!(record["plate"]["plateIndex"], operation["plateIndex"]);
        assert_eq!(record["kind"], "farm3d");
        assert_eq!(record["runtime"]["engineVersion"], "2.4.2");
        assert_eq!(record["blobs"].as_array().unwrap().len(), 6);
        assert!(!running.work_dir(operation["id"].as_str().unwrap()).exists());
    }
}

#[test]
fn a_success_log_is_the_revision_log_with_its_noise_tagged() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-log", &preparation, &[plate])).remove(0);
    let operation = running.wait_state(&id, "succeeded");

    let log = running.ok("get_slice_operation_log", json!({ "sliceOperationId": id }));
    let text = log["text"].as_str().unwrap();
    assert!(text.contains("fake-orca scenario success"), "{text}");
    assert_eq!(log["truncated"], false);
    let noisy = log["noiseLines"].as_array().unwrap();
    assert_eq!(noisy.len(), 1, "{log}");
    let line = text
        .lines()
        .nth(noisy[0].as_u64().unwrap() as usize - 1)
        .unwrap();
    assert!(line.contains("unable to open display"));

    // It is the revision's `log` blob (D13), not a separate operation log.
    let revision_id = operation["sliceRevisionId"].as_str().unwrap();
    let log_blob: String = running
        .services
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT sha256 FROM slice_revision_blobs WHERE revision_id = ?1 AND role = 'log'",
                [revision_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(running.blob(&log_blob), text.as_bytes());
    let operation_log: Option<String> = running
        .services
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT log_sha256 FROM slice_operations WHERE id = ?1",
                [&id],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(operation_log, None);
}

/// D21: a revision's log is read by the revision's own id, so it outlives
/// its operation (here, deleted with its Preparation). An external
/// revision has none; an unknown one is NOT_FOUND.
#[test]
fn a_revision_log_is_read_by_revision_id_and_external_revisions_have_none() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-revision-log", &preparation, &[plate])).remove(0);
    let operation = running.wait_state(&id, "succeeded");
    let revision_id = operation["sliceRevisionId"].as_str().unwrap().to_string();
    let by_operation = running.ok("get_slice_operation_log", json!({ "sliceOperationId": id }));

    running.ok(
        "delete_preparation",
        json!({
            "preparationId": preparation["id"],
            "expectedRevision": preparation["revision"],
        }),
    );
    let error = running.error("get_slice_operation_log", json!({ "sliceOperationId": id }));
    assert_eq!(
        error["code"], "NOT_FOUND",
        "the operation went with its Preparation"
    );

    let by_revision = running.ok(
        "get_slice_revision_log",
        json!({ "sliceRevisionId": revision_id }),
    );
    assert_eq!(by_revision["log"], by_operation, "{by_revision}");
    assert!(by_revision["log"]["text"]
        .as_str()
        .unwrap()
        .contains("fake-orca scenario success"));

    let gcode = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_revision_id = gcode["currentRevision"]["id"].as_str().unwrap().to_string();
    let external = running.create_external("ext-log", &source_revision_id, absent_facts());
    assert_eq!(
        running.ok(
            "get_slice_revision_log",
            json!({ "sliceRevisionId": external["id"] }),
        ),
        json!({ "log": null })
    );

    let error = running.error(
        "get_slice_revision_log",
        json!({ "sliceRevisionId": "slr-missing" }),
    );
    assert_eq!(error["code"], "NOT_FOUND", "{error}");
}

#[test]
fn a_failed_slice_keeps_its_log_and_publishes_nothing() {
    let farm = Farm::new();
    let running = farm.start();
    running.scenario(&[("FAKE_ORCA_SCENARIO", "fail:-50")]);
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let model_id = model["id"].as_str().unwrap();
    let preparation = running.prepare(model_id);
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-fail", &preparation, &[plate])).remove(0);
    let operation = running.wait_state(&id, "failed");
    assert_eq!(operation["failure"]["code"]["kind"], "objectsOutsidePlate");
    assert_eq!(
        operation["failure"]["message"],
        "An object is outside the printable area."
    );
    assert!(operation.get("sliceRevisionId").is_none());

    let log = running.ok("get_slice_operation_log", json!({ "sliceOperationId": id }));
    let text = log["text"].as_str().unwrap();
    assert!(text.contains("fake-orca scenario fail:-50"), "{text}");
    let content_root = running.services.storage.paths().content_root();
    assert!(
        !text.contains(content_root.to_str().unwrap()),
        "the work directory is redacted"
    );
    assert!(
        !text.contains(farm.engine.to_str().unwrap()),
        "the engine is redacted"
    );
    assert_eq!(
        running.ok("list_slice_revisions", json!({ "modelId": model_id })),
        json!([]),
        "no revision"
    );
    assert!(!running.work_dir(&id).exists());
}

#[test]
fn cancelling_a_running_slice_stops_it_and_a_queued_one_never_starts() {
    let farm = Farm::new();
    let running = farm.start();
    running.scenario(&[("FAKE_ORCA_SCENARIO", "hang")]);
    let model = running.import(&farm.source("orca-two-plates.3mf", "two.3mf"), "managed");
    let model_id = model["id"].as_str().unwrap();
    let preparation = running.prepare(model_id);
    let plates = plates(&preparation);
    let operations = ids(&running.start("op-cancel", &preparation, &[&plates[0], &plates[1]]));
    let (first, second) = (&operations[0], &operations[1]);
    running.wait_state(first, "running");

    // The second waits behind the first, and cancels without spawning.
    let cancelled = running.ok(
        "cancel_slice_operation",
        json!({ "sliceOperationId": second }),
    );
    assert_eq!(cancelled["state"], "cancelled");
    assert!(cancelled.get("startedAt").is_none(), "it never started");
    assert!(!running.work_dir(second).exists());
    assert!(
        running
            .events_of(second)
            .iter()
            .all(|event| event["payload"]["state"] != "running"),
        "it was never running"
    );

    let started = Instant::now();
    let cancelled = running.ok(
        "cancel_slice_operation",
        json!({ "sliceOperationId": first }),
    );
    assert_eq!(cancelled["state"], "cancelled", "{cancelled}");
    assert!(started.elapsed() < Duration::from_secs(6));
    assert!(!running.work_dir(first).exists());
    assert_eq!(
        running.ok("list_slice_revisions", json!({ "modelId": model_id })),
        json!([])
    );
    let log = running.ok(
        "get_slice_operation_log",
        json!({ "sliceOperationId": first }),
    );
    assert!(log["text"]
        .as_str()
        .unwrap()
        .contains("fake-orca scenario hang"));

    // A finished operation can't be cancelled.
    let error = running.error(
        "cancel_slice_operation",
        json!({ "sliceOperationId": first }),
    );
    assert_eq!(error["code"], "OPERATION_NOT_CANCELLABLE", "{error}");
}

/// `start_slice` refuses a Preparation with no chosen preset as
/// `VALIDATION` at the document field the panel links: `processPreset` or
/// `filamentPreset`, as the other preset errors name them.
#[test]
fn a_slice_without_a_chosen_preset_names_the_preset_field() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let mut preparation = running.prepare(model["id"].as_str().unwrap());
    for field in ["processPreset", "filamentPreset"] {
        let mut document = preparation["document"].clone();
        let chosen = document[field].take();
        document.as_object_mut().unwrap().remove(field);
        preparation = running.ok(
            "update_preparation",
            json!({
                "preparationId": preparation["id"],
                "expectedRevision": preparation["revision"],
                "document": document,
            }),
        );
        let plate = &plates(&preparation)[0];
        let error = running.error(
            "start_slice",
            json!({
                "operationId": format!("op-no-{field}"),
                "preparationId": preparation["id"],
                "expectedRevision": preparation["revision"],
                "plateKeys": [plate["plateKey"]],
            }),
        );
        assert_eq!(error["code"], "VALIDATION", "{error}");
        assert_eq!(error["details"]["fieldPath"], field, "{error}");

        // Put it back for the next field.
        let mut document = preparation["document"].clone();
        document[field] = chosen;
        preparation = running.ok(
            "update_preparation",
            json!({
                "preparationId": preparation["id"],
                "expectedRevision": preparation["revision"],
                "document": document,
            }),
        );
    }
    assert_eq!(running.slicing()["activeAndRecentOperations"], json!([]));
}

/// D15: a farm3d revision's filament facts come from the filament preset
/// it sliced with. A preset chain with no `filament_diameter` is refused as
/// `PRESET_INVALID` before anything is queued, rather than recorded as
/// 1.75 mm.
#[test]
fn a_filament_preset_without_a_diameter_refuses_the_slice() {
    let farm = Farm::new();
    let common = farm
        .orca
        .path()
        .join("resources/profiles/OrcaFilamentLibrary/filament/fdm_filament_common.json");
    let mut preset: Value = serde_json::from_slice(&fs::read(&common).unwrap()).unwrap();
    preset.as_object_mut().unwrap().remove("filament_diameter");
    fs::write(&common, serde_json::to_vec_pretty(&preset).unwrap()).unwrap();

    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let filament = preparation["document"]["filamentPreset"].clone();
    assert!(filament.is_string(), "{preparation}");
    let plate = &plates(&preparation)[0];
    let error = running.error(
        "start_slice",
        json!({
            "operationId": "op-no-diameter",
            "preparationId": preparation["id"],
            "expectedRevision": preparation["revision"],
            "plateKeys": [plate["plateKey"]],
        }),
    );
    assert_eq!(error["code"], "PRESET_INVALID", "{error}");
    assert_eq!(error["details"]["kind"], "filament", "{error}");
    assert_eq!(error["details"]["preset"], filament, "{error}");
    assert_eq!(
        error["details"]["reason"], "it has no filament_diameter.",
        "{error}"
    );
    assert_eq!(running.slicing()["activeAndRecentOperations"], json!([]));
}

#[test]
fn start_slice_replays_by_operation_id_and_refuses_a_reused_id() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-two-plates.3mf", "two.3mf"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plates = plates(&preparation);
    let first = running.start("op-replay", &preparation, &[&plates[0]]);
    let id = ids(&first).remove(0);
    running.wait_state(&id, "succeeded");

    // The same request: the same operation, nothing new queued.
    let replay = running.start("op-replay", &preparation, &[&plates[0]]);
    assert_eq!(ids(&replay), vec![id.clone()]);
    assert_eq!(replay[0]["state"], "succeeded");
    let count: i64 = running
        .services
        .storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM slice_operations", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(count, 1);

    // The same id for another request is the P3 ledger's reuse error.
    let error = running.error(
        "start_slice",
        json!({
            "operationId": "op-replay",
            "preparationId": preparation["id"],
            "expectedRevision": preparation["revision"],
            "plateKeys": [plates[1]["plateKey"]],
        }),
    );
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "operationId");
}

#[test]
fn a_gcode_model_has_no_preparation() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let error = running.error(
        "create_preparation",
        json!({ "modelId": model["id"], "target": { "kind": "profile", "catalogRef": a_ref_json() } }),
    );
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "modelId");
    assert_eq!(running.slicing()["preparations"], json!([]));
}

#[test]
fn with_no_printers_a_preparation_needs_a_profile_target() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");

    // No Printer to default to: the error names the target, which the
    // app answers with a printer profile.
    let error = running.error("create_preparation", json!({ "modelId": model["id"] }));
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "target");
    assert_eq!(running.slicing()["preparations"], json!([]));

    let preparation = running.ok(
        "create_preparation",
        json!({ "modelId": model["id"], "target": { "kind": "profile", "catalogRef": a_ref_json() } }),
    );
    assert_eq!(preparation["document"]["target"]["kind"], "profile");
    assert_eq!(
        preparation["document"]["target"]["catalogRef"],
        a_ref_json()
    );
}

#[test]
fn a_stale_source_is_refused_then_continued_and_reload_keeps_transforms() {
    let farm = Farm::new();
    let running = farm.start();
    let source = farm.source("cube-binary.stl", "cube.stl");
    let model = running.import(&source, "linked");
    let model_id = model["id"].as_str().unwrap().to_string();
    let first_revision = model["currentRevision"]["id"].as_str().unwrap().to_string();
    let preparation = running.prepare(&model_id);

    // Move the object, then change the linked source.
    let mut document = preparation["document"].clone();
    document["plates"][0]["instances"][0]["transform"]["translateMm"] = json!([60.0, 70.0]);
    document["plates"][0]["instances"][0]["transform"]["rotateDeg"] = json!([0.0, 0.0, 45.0]);
    let preparation = running.ok(
        "update_preparation",
        json!({
            "preparationId": preparation["id"],
            "expectedRevision": preparation["revision"],
            "document": document,
        }),
    );
    let moved = preparation["document"]["plates"][0]["instances"][0].clone();
    fs::copy(fixtures().join("library/cube-ascii.stl"), &source).unwrap();
    let changed = running.ok("check_linked_sources", json!({ "modelIds": [model_id] }));
    assert_eq!(changed[0]["revisionCount"], 2, "{changed}");

    // The Preparation is now stale, and the stream says so.
    let preparation_id = preparation["id"].as_str().unwrap().to_string();
    let stale = wait_for("a stale preparation.changed", || {
        running
            .events_of(&preparation_id)
            .into_iter()
            .rev()
            .find(|event| {
                event["type"] == "slicing.preparation.changed" && event["payload"]["stale"] == true
            })
    });
    assert_eq!(stale["payload"]["sourceRevisionId"], first_revision);
    let preparation = stale["payload"].clone();
    let plate = &plates(&preparation)[0];

    let request = |operation_id: &str, continue_with: Option<&str>| {
        let mut body = json!({
            "operationId": operation_id,
            "preparationId": preparation_id,
            "expectedRevision": preparation["revision"],
            "plateKeys": [plate["plateKey"]],
        });
        if let Some(revision) = continue_with {
            body["continueWithSourceRevision"] = json!(revision);
        }
        body
    };
    let error = running.error("start_slice", request("op-stale", None));
    assert_eq!(error["code"], "PREPARATION_STALE", "{error}");
    assert_eq!(error["recovery"][0], "RELOAD_PREPARATION");

    // Continuing explicitly slices the pinned revision.
    let started = running.ok("start_slice", request("op-continue", Some(&first_revision)));
    let id = started["operations"][0]["id"].as_str().unwrap().to_string();
    let operation = running.wait_state(&id, "succeeded");
    assert_eq!(operation["sourceRevisionId"], first_revision);

    // Reload keeps the surviving object's transform.
    let reloaded = running.ok(
        "reload_preparation",
        json!({ "preparationId": preparation_id, "expectedRevision": preparation["revision"] }),
    );
    assert_eq!(reloaded["removedObjectKeys"], json!([]));
    assert_eq!(reloaded["addedObjectKeys"], json!([]));
    let preparation = &reloaded["preparation"];
    assert_eq!(preparation["stale"], false);
    assert_ne!(preparation["sourceRevisionId"], first_revision);
    assert_eq!(preparation["document"]["plates"][0]["instances"][0], moved);
}

#[test]
fn events_are_ordered_and_the_backfill_covers_what_came_before() {
    let farm = Farm::new();
    let running = farm.start();
    running.scenario(&[("FAKE_ORCA_STEP_MS", "100")]);
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-events", &preparation, &[plate])).remove(0);
    let operation = running.wait_state(&id, "succeeded");
    let revision_id = operation["sliceRevisionId"].as_str().unwrap().to_string();

    let events = running.events.lock().unwrap().clone();
    // One stream, every sequence number once and in order.
    let stream = &events[0]["streamId"];
    for (index, event) in events.iter().enumerate() {
        assert_eq!(&event["streamId"], stream);
        assert_eq!(
            event["sequence"].as_u64().unwrap(),
            events[0]["sequence"].as_u64().unwrap() + index as u64,
            "{events:#?}"
        );
    }
    let kinds: Vec<(String, String)> = events
        .iter()
        .filter(|event| {
            event["subject"]["id"] == id.as_str() || event["subject"]["id"] == revision_id.as_str()
        })
        .map(|event| {
            (
                event["type"].as_str().unwrap().to_string(),
                event["payload"]["state"].as_str().unwrap_or("").to_string(),
            )
        })
        .filter(|(kind, _)| kind != "slicing.operation.progress")
        .collect();
    assert_eq!(
        kinds,
        vec![
            (
                "slicing.operation.changed".to_string(),
                "queued".to_string()
            ),
            (
                "slicing.operation.changed".to_string(),
                "running".to_string()
            ),
            ("slicing.revision.created".to_string(), String::new()),
            (
                "slicing.operation.changed".to_string(),
                "succeeded".to_string()
            ),
        ]
    );
    assert!(events
        .iter()
        .any(|event| event["type"] == "slicing.preparation.changed"
            && event["subject"]["id"] == preparation["id"]));

    // Progress: at most one per 250 ms, and the last update always arrives.
    let progress: Vec<&Value> = events
        .iter()
        .filter(|event| event["type"] == "slicing.operation.progress")
        .collect();
    assert!(!progress.is_empty());
    assert!(
        progress.len() < 5,
        "fake-orca wrote 5 updates 100 ms apart: {progress:#?}"
    );
    assert_eq!(progress.last().unwrap()["payload"]["totalPercent"], 100);
    let times: Vec<chrono::DateTime<chrono::Utc>> = progress
        .iter()
        .map(|event| event["occurredAt"].as_str().unwrap().parse().unwrap())
        .collect();
    for pair in times[..times.len() - 1].windows(2) {
        assert!(
            pair[1] - pair[0] >= chrono::Duration::milliseconds(240),
            "throttled: {times:?}"
        );
    }

    // The backfill: its sequence is the last event's, and it holds what
    // the events built.
    let snapshot = running.slicing();
    assert_eq!(&snapshot["streamId"], stream);
    assert_eq!(
        snapshot["snapshotSequence"],
        events.last().unwrap()["sequence"]
    );
    assert_eq!(snapshot["preparations"][0]["id"], preparation["id"]);
    assert_eq!(
        snapshot["activeAndRecentOperations"][0]["state"],
        "succeeded"
    );
    assert_eq!(snapshot["revisions"][0]["id"], revision_id);
    assert_eq!(snapshot["runtime"]["canSlice"], true);

    // Events after a snapshot carry larger sequences.
    running.scenario(&[("FAKE_ORCA_SCENARIO", "fail:-6")]);
    let before = running.slicing()["snapshotSequence"].as_u64().unwrap();
    let id = ids(&running.start("op-events-2", &preparation, &[plate])).remove(0);
    running.wait_state(&id, "failed");
    let later = running.events_of(&id);
    assert!(later
        .iter()
        .all(|event| event["sequence"].as_u64().unwrap() > before));
}

#[test]
fn a_restart_mid_slice_interrupts_it_and_keeps_earlier_revisions() {
    let farm = Farm::new();
    let first = farm.start();
    let model = first.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let model_id = model["id"].as_str().unwrap().to_string();
    let preparation = first.prepare(&model_id);
    let plate = &plates(&preparation)[0];
    let done = ids(&first.start("op-before", &preparation, &[plate])).remove(0);
    let revision_id = first.wait_state(&done, "succeeded")["sliceRevisionId"]
        .as_str()
        .unwrap()
        .to_string();
    let revision_before = first.ok(
        "get_slice_revision",
        json!({ "sliceRevisionId": revision_id }),
    );
    let gcode_sha256: String = first
        .services
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT gcode_sha256 FROM slice_revisions WHERE id = ?1",
                [&revision_id],
                |row| row.get(0),
            )
        })
        .unwrap();
    let gcode_before = first.blob(&gcode_sha256);

    first.scenario(&[("FAKE_ORCA_SCENARIO", "hang")]);
    // The old worker is held once its engine exits, as a dead farm3d's
    // would be, so only recovery can remove the work directory.
    let (exited, on_exit) = std::sync::mpsc::channel::<String>();
    let (release, released) = std::sync::mpsc::channel::<()>();
    let (exited, released) = (Mutex::new(exited), Mutex::new(released));
    first
        .services
        .slicing
        .set_scheduler_hook(Some(Arc::new(move |point| {
            if let SchedulerPoint::Exited(id) = point {
                exited.lock().unwrap().send(id.to_string()).unwrap();
                released.lock().unwrap().recv().unwrap();
            }
        })));
    let hung = ids(&first.start("op-hung", &preparation, &[plate])).remove(0);
    first.wait_state(&hung, "running");
    let work = first.work_dir(&hung);
    assert!(work.exists());
    let engine_pid = first.pid(&hung);
    let stored_before = first.stored_counts();

    // Restart: a new Storage over the same roots runs D10's recovery, as
    // `build_runtime_services` does, while the old engine still runs.
    let storage = Storage::open(farm.paths.clone(), &farm.lease).unwrap();
    let recovery = recover_after_restart(&storage).unwrap();
    assert_eq!(recovery.interrupted.len(), 1);
    assert_eq!(recovery.interrupted[0].id, hung);
    assert!(recovery.interrupted[0].was_running);
    assert_eq!(
        recovery.stopped,
        vec![hung.clone()],
        "the engine was still ours"
    );
    assert_eq!(
        on_exit.recv_timeout(DEADLINE).unwrap(),
        hung,
        "the old worker saw its engine end and is held"
    );
    assert_eq!(recovery.work_dirs_removed, 1, "{recovery:?}");
    assert!(!work.exists(), "recovery removed the work directory");
    assert!(!storage
        .paths()
        .content_root()
        .join(WORK_ROOT_DIR)
        .read_dir()
        .map(|mut entries| entries.next().is_some())
        .unwrap_or(false));

    let second = farm.start();
    // Let the old worker go: it must store nothing.
    assert!(!process_alive(engine_pid), "recovery stopped the engine");
    release.send(()).unwrap();
    wait_for("the old worker to go idle", || {
        first.services.slicing.scheduler_idle().then_some(())
    });
    first.services.slicing.set_scheduler_hook(None);
    assert!(!work.exists());
    assert_eq!(second.stored_counts(), stored_before, "nothing new stored");
    let interrupted = second.operation(&hung);
    assert_eq!(interrupted["state"], "interrupted");
    assert!(interrupted["finishedAt"].is_string());
    assert!(second.slicing()["activeAndRecentOperations"]
        .as_array()
        .unwrap()
        .iter()
        .all(|operation| operation["state"] != "running" && operation["state"] != "queued"));
    let revisions = second.ok("list_slice_revisions", json!({ "modelId": model_id }));
    assert_eq!(revisions.as_array().unwrap().len(), 1);
    assert_eq!(
        second.ok(
            "get_slice_revision",
            json!({ "sliceRevisionId": revision_id }),
        ),
        revision_before,
        "the earlier revision is unchanged"
    );
    assert_eq!(second.blob(&gcode_sha256), gcode_before);
    drop(first);
}

#[test]
fn preparations_are_edited_deleted_and_removed_with_their_model() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());

    // A stale expected revision is CONFLICT.
    let error = running.error(
        "update_preparation",
        json!({
            "preparationId": preparation["id"],
            "expectedRevision": 99,
            "document": preparation["document"],
        }),
    );
    assert_eq!(error["code"], "CONFLICT");
    // An invalid document is VALIDATION.
    let mut document = preparation["document"].clone();
    document["plates"] = json!([]);
    let error = running.error(
        "update_preparation",
        json!({
            "preparationId": preparation["id"],
            "expectedRevision": preparation["revision"],
            "document": document,
        }),
    );
    assert_eq!(error["code"], "VALIDATION");

    running.ok(
        "delete_preparation",
        json!({ "preparationId": preparation["id"], "expectedRevision": preparation["revision"] }),
    );
    assert_eq!(running.slicing()["preparations"], json!([]));
    let preparation_id = preparation["id"].as_str().unwrap().to_string();
    assert!(running
        .events_of(&preparation_id)
        .iter()
        .any(|event| event["type"] == "slicing.preparation.removed"));

    // A Preparation that cascades with its Model goes out as removed too.
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let preparation_id = preparation["id"].as_str().unwrap().to_string();
    running.ok(
        "delete_model",
        json!({ "id": model["id"], "expectedRevision": model["revision"] }),
    );
    wait_for("preparation.removed after the Model", || {
        running
            .events_of(&preparation_id)
            .into_iter()
            .find(|event| event["type"] == "slicing.preparation.removed")
    });
}

#[test]
fn a_revision_can_be_deleted_and_the_stream_says_so() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let model_id = model["id"].as_str().unwrap();
    let preparation = running.prepare(model_id);
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-delete", &preparation, &[plate])).remove(0);
    let revision_id = running.wait_state(&id, "succeeded")["sliceRevisionId"]
        .as_str()
        .unwrap()
        .to_string();
    running.ok(
        "delete_slice_revision",
        json!({ "sliceRevisionId": revision_id }),
    );
    assert_eq!(
        running.ok("list_slice_revisions", json!({ "modelId": model_id })),
        json!([])
    );
    let events = running.events.lock().unwrap().clone();
    let removed = events
        .iter()
        .position(|event| {
            event["type"] == "slicing.revision.removed" && event["subject"]["id"] == revision_id
        })
        .expect("revision.removed");
    // The operation that made it loses its link, and the stream says so
    // right after.
    let changed = &events[removed + 1];
    assert_eq!(changed["type"], "slicing.operation.changed");
    assert_eq!(changed["subject"]["id"], id);
    assert_eq!(changed["payload"]["state"], "succeeded");
    assert!(
        changed["payload"].get("sliceRevisionId").is_none(),
        "{changed}"
    );
    let error = running.error(
        "get_slice_revision",
        json!({ "sliceRevisionId": revision_id }),
    );
    assert_eq!(error["code"], "NOT_FOUND");
    // Its log was the revision's, so it went too: NOT_FOUND, not empty.
    let error = running.error("get_slice_operation_log", json!({ "sliceOperationId": id }));
    assert_eq!(error["code"], "NOT_FOUND", "{error}");
    assert_eq!(
        error["message"],
        "This slice's log was deleted with its Slice Revision."
    );
}

#[test]
fn a_cancel_racing_the_enqueue_waits_for_it_and_nothing_spawns() {
    use farm3d_lib::slicing::operations::cancel_slice_operation;
    use std::sync::mpsc;

    enum Seen {
        CancelWaits,
        Cancelled(Value),
    }
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plate = plates(&preparation)[0].clone();

    // Hold `start_slice` between its commit and its enqueue.
    let (committed, on_commit) = mpsc::channel::<Vec<String>>();
    let (release, released) = mpsc::channel::<()>();
    let released = Mutex::new(released);
    let (seen, on_seen) = mpsc::channel::<Seen>();
    let seen_waiting = Mutex::new(seen.clone());
    let spawned = Arc::new(Mutex::new(Vec::<String>::new()));
    let spawning = Arc::clone(&spawned);
    running
        .services
        .slicing
        .set_scheduler_hook(Some(Arc::new(move |point| match point {
            SchedulerPoint::Committed(ids) => {
                committed.send(ids.to_vec()).unwrap();
                released.lock().unwrap().recv().unwrap();
            }
            SchedulerPoint::CancelAwaitsStart(_) => {
                seen_waiting
                    .lock()
                    .unwrap()
                    .send(Seen::CancelWaits)
                    .unwrap();
            }
            SchedulerPoint::Spawning(id) => spawning.lock().unwrap().push(id.to_string()),
            SchedulerPoint::Exited(_) => {}
        })));
    let slicing = Arc::clone(&running.services.slicing);
    let request = farm3d_lib::slicing::operations::StartSliceRequest {
        operation_id: "op-race".to_string(),
        preparation_id: preparation["id"].as_str().unwrap().to_string(),
        expected_revision: preparation["revision"].as_i64().unwrap(),
        plate_keys: vec![plate["plateKey"].as_str().unwrap().to_string()],
        continue_with_source_revision: None,
    };
    let starter = std::thread::spawn(move || {
        farm3d_lib::slicing::operations::start_slice(&slicing, &request).unwrap()
    });
    let id = on_commit.recv().unwrap().remove(0);
    assert_eq!(running.operation(&id)["state"], "queued", "committed");

    let slicing = Arc::clone(&running.services.slicing);
    let cancel_id = id.clone();
    let canceller = std::thread::spawn(move || {
        let record = cancel_slice_operation(&slicing, &cancel_id).unwrap();
        seen.send(Seen::Cancelled(serde_json::to_value(record).unwrap()))
            .unwrap();
    });
    // The cancel must wait for the start, not end the row under it.
    assert!(
        matches!(on_seen.recv().unwrap(), Seen::CancelWaits),
        "the cancel finished while the start still held its jobs"
    );
    // And it stays blocked while the start holds its lock (a fixed cancel
    // can never finish here, so this can't flake; it only bounds how long
    // a broken one gets to show itself).
    assert!(
        matches!(
            on_seen.recv_timeout(Duration::from_millis(300)),
            Err(mpsc::RecvTimeoutError::Timeout)
        ),
        "the cancel finished while the start still held its jobs"
    );
    release.send(()).unwrap();
    starter.join().unwrap();
    let Seen::Cancelled(record) = on_seen.recv().unwrap() else {
        panic!("expected the cancel's result");
    };
    canceller.join().unwrap();
    assert_eq!(record["state"], "cancelled", "{record}");

    wait_for("the scheduler to go idle", || {
        running.services.slicing.scheduler_idle().then_some(())
    });
    running.services.slicing.set_scheduler_hook(None);
    assert!(
        !spawned.lock().unwrap().contains(&id),
        "OrcaSlicer was started for a cancelled operation"
    );
    let operation = running.operation(&id);
    assert_eq!(operation["state"], "cancelled");
    assert!(operation.get("startedAt").is_none(), "it never spawned");
    // A spawned engine reports progress even when its row can't start.
    assert!(
        running.events_of(&id).iter().all(|event| {
            event["type"] != "slicing.operation.progress" && event["payload"]["state"] != "running"
        }),
        "it never spawned: {:#?}",
        running.events_of(&id)
    );
    assert!(!running.work_dir(&id).exists());

    // The queue still runs.
    let next = ids(&running.start("op-after-race", &preparation, &[&plate])).remove(0);
    running.wait_state(&next, "succeeded");
}

#[test]
fn deleting_a_model_mid_slice_stops_it_and_the_queue_drains() {
    let farm = Farm::new();
    let running = farm.start();
    running.scenario(&[("FAKE_ORCA_SCENARIO", "hang")]);
    let doomed = running.import(&farm.source("orca-two-plates.3mf", "two.3mf"), "managed");
    let kept = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let doomed_id = doomed["id"].as_str().unwrap().to_string();
    let preparation = running.prepare(&doomed_id);
    let two = plates(&preparation);
    let operations = ids(&running.start("op-doomed", &preparation, &[&two[0], &two[1]]));
    running.wait_state(&operations[0], "running");
    let engine_pid = running.pid(&operations[0]);
    let stored_before = running.stored_counts();

    let doomed = running.ok("list_library", json!({}))["models"]
        .as_array()
        .unwrap()
        .iter()
        .find(|model| model["id"] == doomed_id.as_str())
        .cloned()
        .unwrap();
    running.ok(
        "delete_model",
        json!({ "id": doomed_id, "expectedRevision": doomed["revision"] }),
    );
    wait_for("the engine to be stopped", || {
        (!process_alive(engine_pid)).then_some(())
    });
    wait_for("the scheduler to go idle", || {
        running.services.slicing.scheduler_idle().then_some(())
    });
    for id in &operations {
        assert!(!running.work_dir(id).exists());
    }
    let rows: i64 = running
        .services
        .storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM slice_operations", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(rows, 0, "the operations went with the Model");
    assert_eq!(running.stored_counts().1, stored_before.1, "no revision");

    // The next slice, of another Model, runs.
    running.scenario(&[]);
    let preparation = running.prepare(kept["id"].as_str().unwrap());
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-kept", &preparation, &[plate])).remove(0);
    running.wait_state(&id, "succeeded");
}

#[test]
fn a_preparation_with_active_slices_is_not_deleted() {
    let farm = Farm::new();
    let running = farm.start();
    running.scenario(&[("FAKE_ORCA_SCENARIO", "hang")]);
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-busy", &preparation, &[plate])).remove(0);
    running.wait_state(&id, "running");
    let error = running.error(
        "delete_preparation",
        json!({ "preparationId": preparation["id"], "expectedRevision": preparation["revision"] }),
    );
    assert_eq!(error["code"], "CONFLICT", "{error}");
    running.ok("cancel_slice_operation", json!({ "sliceOperationId": id }));
    running.ok(
        "delete_preparation",
        json!({ "preparationId": preparation["id"], "expectedRevision": preparation["revision"] }),
    );
}

#[test]
fn continuing_with_another_source_revision_is_invalid() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plate = &plates(&preparation)[0];
    let error = running.error(
        "start_slice",
        json!({
            "operationId": "op-mismatch",
            "preparationId": preparation["id"],
            "expectedRevision": preparation["revision"],
            "plateKeys": [plate["plateKey"]],
            "continueWithSourceRevision": "msr-00000000-0000-0000-0000-000000000000",
        }),
    );
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "continueWithSourceRevision");
    assert_eq!(running.slicing()["activeAndRecentOperations"], json!([]));
}

#[test]
fn a_slice_that_cannot_be_stored_fails_with_its_log() {
    use farm3d_lib::library::content::ContentFailurePoint;

    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let model_id = model["id"].as_str().unwrap();
    let preparation = running.prepare(model_id);
    let plate = &plates(&preparation)[0];
    // The revision's placement fails once, as a full disk would.
    running
        .services
        .library
        .content
        .inject_failure_once(ContentFailurePoint::AfterPlacementBeforeCommit);
    let id = ids(&running.start("op-unstored", &preparation, &[plate])).remove(0);
    let operation = running.wait_state(&id, "failed");
    assert_eq!(
        operation["failure"]["code"]["kind"], "storageFailed",
        "{operation}"
    );
    assert_eq!(
        running.ok("list_slice_revisions", json!({ "modelId": model_id })),
        json!([])
    );
    let log = running.ok("get_slice_operation_log", json!({ "sliceOperationId": id }));
    assert!(
        log["text"]
            .as_str()
            .unwrap()
            .contains("fake-orca scenario success"),
        "the log is kept: {log}"
    );
}

// ---------------------------------------------------------------------------
// Task 9: `create_external_slice_revision` (D16)
// ---------------------------------------------------------------------------

#[test]
fn a_worker_panic_fails_the_slice_with_internal_error_and_the_queue_continues() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let model_id = model["id"].as_str().unwrap();
    let preparation = running.prepare(model_id);
    let plate = &plates(&preparation)[0];
    // The worker panics once, after its engine has run. `resume_unwind`
    // skips the panic hook, so the test output stays quiet.
    let armed = Arc::new(Mutex::new(true));
    let fire = Arc::clone(&armed);
    running
        .services
        .slicing
        .set_scheduler_hook(Some(Arc::new(move |point| {
            if matches!(point, SchedulerPoint::Exited(_))
                && std::mem::take(&mut *fire.lock().unwrap())
            {
                std::panic::resume_unwind(Box::new("injected worker panic"));
            }
        })));

    let id = ids(&running.start("op-panic", &preparation, &[plate])).remove(0);
    let operation = running.wait_state(&id, "failed");
    assert_eq!(
        operation["failure"]["code"]["kind"], "internalError",
        "{operation}"
    );
    assert_eq!(
        operation["failure"]["message"],
        "farm3d stopped this slice after an internal error."
    );
    assert!(!*armed.lock().unwrap(), "the panic was injected");
    assert!(operation.get("sliceRevisionId").is_none());
    assert!(!running.work_dir(&id).exists());
    assert_eq!(
        running.ok("list_slice_revisions", json!({ "modelId": model_id })),
        json!([])
    );
    // A panic path has no log to keep; the log read still answers.
    let log = running.ok("get_slice_operation_log", json!({ "sliceOperationId": id }));
    assert_eq!(log["text"], "", "{log}");

    // The queue continues.
    let next = ids(&running.start("op-after-panic", &preparation, &[plate])).remove(0);
    running.wait_state(&next, "succeeded");
    running.services.slicing.set_scheduler_hook(None);
}

#[test]
fn an_external_revision_takes_only_confirmed_or_absent_facts_and_reuses_the_source_blob() {
    let farm = Farm::new();
    let running = farm.start();
    let source_path = farm.source("orca-cube.gcode", "cube.gcode");
    let source_bytes = fs::read(&source_path).unwrap();
    let model = running.import(&source_path, "managed");
    let model_id = model["id"].as_str().unwrap().to_string();
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();
    let source_sha256 = model["currentRevision"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    let (blobs_after_import, _) = running.stored_counts();

    let absent = running.create_external("ext-absent", &source_revision_id, absent_facts());
    assert_eq!(absent["kind"], "external");
    assert_eq!(absent["modelId"], model_id);
    assert_eq!(absent["sourceRevisionId"], source_revision_id);
    assert!(absent.get("plate").is_none(), "{absent}");
    assert!(absent.get("runtime").is_none(), "{absent}");
    assert!(absent.get("target").is_none(), "{absent}");
    assert_eq!(absent["estimates"], Value::Null);
    assert_eq!(absent["requiresManualPrinterSelection"], true, "{absent}");
    assert_eq!(absent["blobs"], json!([]));
    for fact in [
        "printerProfile",
        "nozzleDiameterMm",
        "materialFamily",
        "filamentDiameterMm",
    ] {
        assert_eq!(
            absent["facts"][fact]["provenance"], "absent",
            "{fact}: {absent}"
        );
    }

    let confirmed =
        running.create_external("ext-confirmed", &source_revision_id, confirmed_facts());
    assert_eq!(
        confirmed["requiresManualPrinterSelection"], false,
        "{confirmed}"
    );
    for fact in [
        "printerProfile",
        "nozzleDiameterMm",
        "materialFamily",
        "filamentDiameterMm",
    ] {
        assert_eq!(
            confirmed["facts"][fact]["provenance"], "operatorConfirmed",
            "{fact}: {confirmed}"
        );
    }
    assert_eq!(confirmed["facts"]["nozzleDiameterMm"]["value"], 0.6);
    assert_eq!(confirmed["facts"]["materialFamily"]["value"], "PETG");
    assert_eq!(confirmed["facts"]["filamentDiameterMm"]["value"], 2.85);
    assert_eq!(
        confirmed["facts"]["printerProfile"]["value"]["catalogRef"],
        a_ref_json()
    );
    assert_eq!(confirmed["blobs"], json!([]));

    // Both revisions share the source's own claims: the file's estimates
    // reach `claimedEstimates`, marked untrusted, never `facts`.
    for revision in [&absent, &confirmed] {
        assert_eq!(revision["claimedEstimates"]["source"], "fileClaim");
        assert_eq!(revision["claimedEstimates"]["trusted"], false);
        assert_eq!(revision["producer"]["name"], "OrcaSlicer");
    }

    // No copy: creating two external revisions added zero content blobs.
    let (blobs_after_external, revision_count) = running.stored_counts();
    assert_eq!(blobs_after_external, blobs_after_import);
    assert_eq!(revision_count, 2);
    assert_eq!(running.blob(&source_sha256), source_bytes);

    // M2: each external revision's `gcode_sha256` is the source revision's
    // own sha256, fetched via IPC (`model["currentRevision"]["sha256"]`) —
    // not merely a byte-for-byte-equal but separately-identified blob.
    for revision in [&absent, &confirmed] {
        let revision_id = revision["id"].as_str().unwrap();
        assert_eq!(running.gcode_sha256(revision_id), source_sha256);
    }

    let events = running.events.lock().unwrap().clone();
    for revision in [&absent, &confirmed] {
        let id = revision["id"].as_str().unwrap();
        assert!(
            events.iter().any(|event| {
                event["type"] == "slicing.revision.created" && event["subject"]["id"] == id
            }),
            "no slicing.revision.created for {id}"
        );
    }
}

#[test]
fn external_facts_never_pick_up_a_files_own_claims_for_every_gcode_fixture() {
    let farm = Farm::new();
    let running = farm.start();

    let mut gcode_fixtures: Vec<PathBuf> = fs::read_dir(fixtures().join("library"))
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().and_then(|ext| ext.to_str()) == Some("gcode"))
        .collect();
    gcode_fixtures.sort();
    assert!(!gcode_fixtures.is_empty());

    // The confirmed constants below (0.6 mm, ABS, 2.85 mm) are chosen so no
    // fixture's own claim equals them, but that's re-checked dynamically
    // here rather than merely assumed: each fixture's actual parsed claims
    // (D11) are fetched via `list_model_revisions`, and a confirmed fact is
    // asserted to never equal the matching claim, whatever it is.
    const CONFIRMED_NOZZLE_MM: f64 = 0.6;
    const CONFIRMED_MATERIAL: &str = "ABS";
    const CONFIRMED_FILAMENT_MM: f64 = 2.85;

    for (fixture_index, source) in gcode_fixtures.iter().enumerate() {
        let name = source.file_name().unwrap().to_str().unwrap();
        let model = running.import(
            &farm.source(name, &format!("prop-{fixture_index}.gcode")),
            "managed",
        );
        let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();

        let revisions = running.ok("list_model_revisions", json!({ "modelId": model["id"] }));
        let claims = revisions[0]["inspection"]["claims"].as_array().unwrap();
        let claim = |key: &str| -> Option<String> {
            claims
                .iter()
                .find(|claim| claim["key"] == key)
                .map(|claim| claim["value"].as_str().unwrap().to_string())
        };
        let nozzle_claim_mm: Option<f64> = claim("nozzle_diameter").and_then(|v| v.parse().ok());
        let material_claim: Option<String> = claim("filament_type");
        // `filament_diameter` isn't in the G-code inspector's claim
        // allowlist (`CLAIM_KEYS`), so it can never be claimed at all —
        // there is nothing for `filamentDiameterMm` to leak from.
        assert!(claim("filament_diameter").is_none(), "{name}");
        if let Some(nozzle_claim_mm) = nozzle_claim_mm {
            assert_ne!(
                CONFIRMED_NOZZLE_MM, nozzle_claim_mm,
                "{name}: the confirmed nozzle coincides with the file's own claim"
            );
        }
        if let Some(material_claim) = &material_claim {
            assert_ne!(
                CONFIRMED_MATERIAL, material_claim,
                "{name}: the confirmed material coincides with the file's own claim"
            );
        }

        for mask in 0u8..16 {
            let bit = |n: u8| mask & (1 << n) != 0;
            let confirmed_or_absent = |confirmed: bool, value: Value| {
                if confirmed {
                    json!({ "kind": "confirmed", "value": value })
                } else {
                    json!({ "kind": "absent" })
                }
            };
            let facts = json!({
                "printerProfile": confirmed_or_absent(
                    bit(0),
                    json!({ "kind": "profile", "catalogRef": a_ref_json() }),
                ),
                "nozzleDiameterMm": confirmed_or_absent(bit(1), json!(CONFIRMED_NOZZLE_MM)),
                "materialFamily": confirmed_or_absent(bit(2), json!(CONFIRMED_MATERIAL)),
                "filamentDiameterMm": confirmed_or_absent(bit(3), json!(CONFIRMED_FILAMENT_MM)),
            });
            let operation_id = format!("prop-{fixture_index}-{mask}");
            let revision = running.create_external(&operation_id, &source_revision_id, facts);

            let expected_provenance = |confirmed: bool| {
                if confirmed {
                    "operatorConfirmed"
                } else {
                    "absent"
                }
            };
            assert_eq!(
                revision["facts"]["printerProfile"]["provenance"],
                expected_provenance(bit(0)),
                "{name} mask {mask:04b}: {revision}"
            );
            assert_eq!(
                revision["facts"]["nozzleDiameterMm"]["provenance"],
                expected_provenance(bit(1)),
                "{name} mask {mask:04b}: {revision}"
            );
            assert_eq!(
                revision["facts"]["materialFamily"]["provenance"],
                expected_provenance(bit(2)),
                "{name} mask {mask:04b}: {revision}"
            );
            assert_eq!(
                revision["facts"]["filamentDiameterMm"]["provenance"],
                expected_provenance(bit(3)),
                "{name} mask {mask:04b}: {revision}"
            );
            if bit(0) {
                assert_eq!(
                    revision["facts"]["printerProfile"]["value"]["catalogRef"],
                    a_ref_json()
                );
            }
            if bit(1) {
                assert_eq!(
                    revision["facts"]["nozzleDiameterMm"]["value"],
                    CONFIRMED_NOZZLE_MM
                );
            }
            if bit(2) {
                assert_eq!(
                    revision["facts"]["materialFamily"]["value"],
                    CONFIRMED_MATERIAL
                );
            }
            if bit(3) {
                assert_eq!(
                    revision["facts"]["filamentDiameterMm"]["value"],
                    CONFIRMED_FILAMENT_MM
                );
            }
            assert_eq!(
                revision["requiresManualPrinterSelection"],
                mask != 0b1111,
                "{name} mask {mask:04b}"
            );
        }
    }
}

#[test]
fn a_non_gcode_source_is_rejected_and_the_operation_id_can_be_reused() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();

    let error = running.error(
        "create_external_slice_revision",
        json!({
            "operationId": "ext-bad-source",
            "sourceRevisionId": source_revision_id,
            "facts": absent_facts(),
        }),
    );
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "sourceRevisionId");
    assert_eq!(running.slicing()["revisions"], json!([]));
    // M1: a rejection emits no revision event (the startup runtime probe's
    // own `slicing.runtime.changed` is unrelated background noise).
    assert!(
        !running
            .events
            .lock()
            .unwrap()
            .iter()
            .any(|event| event["type"]
                .as_str()
                .unwrap_or("")
                .starts_with("slicing.revision.")),
        "a rejected create_external_slice_revision must emit no slicing.revision.* event"
    );

    // No dangling ledger claim: the same `operationId` works for a real
    // (G-code) source afterward.
    let gcode_model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let gcode_source_id = gcode_model["currentRevision"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = running.create_external("ext-bad-source", &gcode_source_id, absent_facts());
    assert_eq!(revision["kind"], "external");
}

#[test]
fn an_idempotent_retry_returns_the_same_revision_and_creates_nothing_new() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let model_id = model["id"].as_str().unwrap().to_string();
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();

    let first = running.create_external("ext-retry", &source_revision_id, confirmed_facts());
    let (_, revision_count_after_first) = running.stored_counts();

    let second = running.create_external("ext-retry", &source_revision_id, confirmed_facts());
    assert_eq!(first, second);
    let (_, revision_count_after_second) = running.stored_counts();
    assert_eq!(revision_count_after_first, revision_count_after_second);
    assert_eq!(
        running
            .ok("list_slice_revisions", json!({ "modelId": model_id }))
            .as_array()
            .unwrap()
            .len(),
        1
    );
    // M1: the replay emits no second `slicing.revision.created`.
    let revision_id = first["id"].as_str().unwrap();
    let created_count = running
        .events_of(revision_id)
        .into_iter()
        .filter(|event| event["type"] == "slicing.revision.created")
        .count();
    assert_eq!(
        created_count, 1,
        "a replay must not emit a second slicing.revision.created"
    );

    // The same id with a DIFFERENT request is refused, and burns nothing.
    let error = running.error(
        "create_external_slice_revision",
        json!({
            "operationId": "ext-retry",
            "sourceRevisionId": source_revision_id,
            "facts": absent_facts(),
        }),
    );
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], "operationId");
    let (_, revision_count_after_conflict) = running.stored_counts();
    assert_eq!(revision_count_after_conflict, revision_count_after_second);
    // M1: the rejected conflict emits nothing (still just the one
    // `slicing.revision.created` from the original creation).
    let created_count_after_conflict = running
        .events_of(revision_id)
        .into_iter()
        .filter(|event| event["type"] == "slicing.revision.created")
        .count();
    assert_eq!(created_count_after_conflict, 1);
}

#[test]
fn an_external_revision_is_unchanged_after_a_restart() {
    let farm = Farm::new();
    let first = farm.start();
    let model = first.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_sha256 = model["currentRevision"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();
    let revision_before =
        first.create_external("ext-restart", &source_revision_id, confirmed_facts());
    let revision_id = revision_before["id"].as_str().unwrap().to_string();
    let gcode_sha256 = first.gcode_sha256(&revision_id);
    // M2: the revision's stored hash is the source revision's own sha256.
    assert_eq!(gcode_sha256, source_sha256);
    let gcode_before = first.blob(&gcode_sha256);
    drop(first);

    let second = farm.start();
    let revision_after = second.ok(
        "get_slice_revision",
        json!({ "sliceRevisionId": revision_id }),
    );
    assert_eq!(revision_before, revision_after);
    // M2, after restart: still the source revision's sha256, unchanged.
    assert_eq!(second.gcode_sha256(&revision_id), source_sha256);
    assert_eq!(second.blob(&gcode_sha256), gcode_before);
}

/// Asserts that `body` is rejected as `VALIDATION` at `field_path`, and
/// that the attempt left no ledger claim: the same `operationId` works
/// afterward for a request that would otherwise be a legitimate first use.
fn assert_validation_rejection_leaves_no_claim(
    running: &Running,
    operation_id: &str,
    field_path: &str,
    bad_facts: Value,
    good_facts: Value,
    source_revision_id: &str,
) {
    let error = running.error(
        "create_external_slice_revision",
        json!({
            "operationId": operation_id,
            "sourceRevisionId": source_revision_id,
            "facts": bad_facts,
        }),
    );
    assert_eq!(error["code"], "VALIDATION", "{error}");
    assert_eq!(error["details"]["fieldPath"], field_path, "{error}");

    let revision = running.create_external(operation_id, source_revision_id, good_facts);
    assert_eq!(revision["kind"], "external");
}

#[test]
fn confirmed_diameters_out_of_range_are_rejected_before_any_claim() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();

    // NaN and infinity have no JSON representation (`serde_json` encodes
    // them as `null`, which fails to deserialize as `f64` before this
    // command ever runs), so only finite-but-out-of-range values are
    // reachable over the wire; `validate_diameter_mm`'s `is_finite` check
    // is unit-tested directly in `slicing::external` instead (see below).
    for (bad_nozzle, label) in [(0.0, "zero"), (-0.4, "negative"), (5.1, "over 5mm")] {
        let mut facts = absent_facts();
        facts["nozzleDiameterMm"] = json!({ "kind": "confirmed", "value": bad_nozzle });
        assert_validation_rejection_leaves_no_claim(
            &running,
            &format!("ext-bad-nozzle-{label}"),
            "facts.nozzleDiameterMm",
            facts,
            absent_facts(),
            &source_revision_id,
        );
    }

    for (bad_filament, label) in [(0.0, "zero"), (-1.75, "negative"), (5.5, "over 5mm")] {
        let mut facts = absent_facts();
        facts["filamentDiameterMm"] = json!({ "kind": "confirmed", "value": bad_filament });
        assert_validation_rejection_leaves_no_claim(
            &running,
            &format!("ext-bad-filament-{label}"),
            "facts.filamentDiameterMm",
            facts,
            absent_facts(),
            &source_revision_id,
        );
    }
}

#[test]
fn confirmed_material_other_follows_the_spool_rule() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();

    // OTHER with no materialOther.
    let mut facts = absent_facts();
    facts["materialFamily"] = json!({ "kind": "confirmed", "value": "OTHER" });
    assert_validation_rejection_leaves_no_claim(
        &running,
        "ext-other-missing",
        "facts.materialOther",
        facts,
        absent_facts(),
        &source_revision_id,
    );

    // OTHER with a materialOther that's too long (33 characters).
    let mut facts = absent_facts();
    facts["materialFamily"] = json!({ "kind": "confirmed", "value": "OTHER" });
    facts["materialOther"] = json!("x".repeat(33));
    assert_validation_rejection_leaves_no_claim(
        &running,
        "ext-other-too-long",
        "facts.materialOther",
        facts,
        absent_facts(),
        &source_revision_id,
    );

    // A non-OTHER family with a materialOther forbids one.
    let mut facts = absent_facts();
    facts["materialFamily"] = json!({ "kind": "confirmed", "value": "PLA" });
    facts["materialOther"] = json!("Wood-filled PLA");
    assert_validation_rejection_leaves_no_claim(
        &running,
        "ext-pla-with-other",
        "facts.materialOther",
        facts,
        absent_facts(),
        &source_revision_id,
    );

    // A valid OTHER + materialOther is accepted.
    let mut facts = absent_facts();
    facts["materialFamily"] = json!({ "kind": "confirmed", "value": "OTHER" });
    facts["materialOther"] = json!("Wood-filled PLA");
    let revision = running.create_external("ext-other-ok", &source_revision_id, facts);
    assert_eq!(revision["facts"]["materialFamily"]["value"], "OTHER");
}

// ---------------------------------------------------------------------------
// D16: the `{kind:"printer", printerId}` branch
// ---------------------------------------------------------------------------

#[test]
fn a_seeded_printer_gives_an_operator_confirmed_printer_profile_fact() {
    let farm = Farm::new();
    let running = farm.start();
    let printer = farm3d_lib::printers::repository::PrinterRepository::new(Arc::clone(
        &running.services.storage,
    ))
    .create(a_stored_printer("printer-external"))
    .expect("create printer");

    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();

    let mut facts = absent_facts();
    facts["printerProfile"] = json!({
        "kind": "confirmed",
        "value": { "kind": "printer", "printerId": printer.id },
    });
    let revision = running.create_external("ext-printer", &source_revision_id, facts);
    assert_eq!(
        revision["facts"]["printerProfile"]["provenance"],
        "operatorConfirmed"
    );
    assert_eq!(
        revision["facts"]["printerProfile"]["value"]["catalogRef"],
        a_ref_json()
    );
}

#[test]
fn an_unknown_printer_id_is_not_found_and_leaves_no_claim() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();

    let mut facts = absent_facts();
    facts["printerProfile"] = json!({
        "kind": "confirmed",
        "value": { "kind": "printer", "printerId": "no-such-printer" },
    });
    let error = running.error(
        "create_external_slice_revision",
        json!({
            "operationId": "ext-unknown-printer",
            "sourceRevisionId": source_revision_id,
            "facts": facts.clone(),
        }),
    );
    assert_eq!(error["code"], "NOT_FOUND", "{error}");

    // No dangling claim: the same operationId succeeds for a good request.
    let revision =
        running.create_external("ext-unknown-printer", &source_revision_id, absent_facts());
    assert_eq!(revision["kind"], "external");
}

// ---------------------------------------------------------------------------
// M3: a Model with an external Slice Revision can't be deleted
// ---------------------------------------------------------------------------

#[test]
fn a_model_with_an_external_revision_blocks_delete_model() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();
    running.create_external("ext-blocks-delete", &source_revision_id, absent_facts());

    let error = running.error(
        "delete_model",
        json!({ "id": model["id"], "expectedRevision": model["revision"] }),
    );
    assert_eq!(error["code"], "LIFECYCLE_BLOCKED", "{error}");
    assert_eq!(
        error["details"]["blockers"][0]["code"],
        "SLICE_REVISIONS_EXIST"
    );
}

// ---------------------------------------------------------------------------
// M4: replaying a create after its revision was deleted
// ---------------------------------------------------------------------------

#[test]
fn replaying_after_the_revision_was_deleted_is_not_found() {
    let farm = Farm::new();
    let running = farm.start();
    let model = running.import(&farm.source("orca-cube.gcode", "cube.gcode"), "managed");
    let source_revision_id = model["currentRevision"]["id"].as_str().unwrap().to_string();
    let revision = running.create_external(
        "ext-replay-after-delete",
        &source_revision_id,
        absent_facts(),
    );
    let revision_id = revision["id"].as_str().unwrap().to_string();

    running.ok(
        "delete_slice_revision",
        json!({ "sliceRevisionId": revision_id }),
    );

    let error = running.error(
        "create_external_slice_revision",
        json!({
            "operationId": "ext-replay-after-delete",
            "sourceRevisionId": source_revision_id,
            "facts": absent_facts(),
        }),
    );
    assert_eq!(error["code"], "NOT_FOUND", "{error}");
}

/// The same IPC path against a real OrcaSlicer (spec D23): `FARM3D_ORCA`
/// is the engine; the TestVendor fixtures are the preset source, so the
/// test catalog's printer resolves. Run with `just test-orca`.
#[test]
#[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA (just test-orca)"]
fn real_orca_slices_a_cube_through_the_commands() {
    let farm = Farm::with_real_orca();
    let running = farm.start();
    let status = running.ok("check_slicer_runtime", json!({}));
    assert_eq!(status["canSlice"], true, "{status}");
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let preparation = running.prepare(model["id"].as_str().unwrap());
    let plate = &plates(&preparation)[0];
    let id = ids(&running.start("op-real", &preparation, &[plate])).remove(0);
    let deadline = Instant::now() + Duration::from_secs(120);
    let operation = loop {
        let operation = running.operation(&id);
        if operation["state"] != "queued" && operation["state"] != "running" {
            break operation;
        }
        assert!(Instant::now() < deadline, "the slice took over 2 minutes");
        std::thread::sleep(Duration::from_millis(200));
    };
    let log = running.ok("get_slice_operation_log", json!({ "sliceOperationId": id }));
    assert_eq!(
        operation["state"], "succeeded",
        "{operation}\n{}",
        log["text"]
    );
    let revision = running.ok(
        "get_slice_revision",
        json!({ "sliceRevisionId": operation["sliceRevisionId"] }),
    );
    assert_eq!(
        revision["runtime"]["engineVersion"],
        status["engine"]["version"]
    );
}

/// Waits for a real OrcaSlicer slice to finish, allowing two minutes, and
/// panics with its log unless it succeeded.
fn wait_for_real_success(running: &Running, id: &str) -> Value {
    let deadline = Instant::now() + Duration::from_secs(120);
    let operation = loop {
        let operation = running.operation(id);
        if operation["state"] != "queued" && operation["state"] != "running" {
            break operation;
        }
        assert!(Instant::now() < deadline, "the slice took over 2 minutes");
        std::thread::sleep(Duration::from_millis(200));
    };
    if operation["state"] != "succeeded" {
        let log = running.ok("get_slice_operation_log", json!({ "sliceOperationId": id }));
        panic!("{operation}\n{}", log["text"]);
    }
    operation
}

/// The `; key = value` claims of a G-code's `CONFIG_BLOCK`.
fn config_claims(gcode: &[u8]) -> std::collections::BTreeMap<String, String> {
    String::from_utf8_lossy(gcode)
        .lines()
        .skip_while(|line| line.trim() != "; CONFIG_BLOCK_START")
        .take_while(|line| line.trim() != "; CONFIG_BLOCK_END")
        .filter_map(|line| {
            let (key, value) = line.strip_prefix("; ")?.split_once(" = ")?;
            Some((key.to_string(), value.to_string()))
        })
        .collect()
}

/// Spec AC3: writing each mapped farm3d control changes the G-code's header
/// claim for its OrcaSlicer key, against a real OrcaSlicer (spec D23). One
/// baseline slice with no controls, then one slice per control, each with a
/// value the TestVendor preset does not have. Run with `just test-orca`.
#[test]
#[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA (just test-orca)"]
fn real_orca_each_mapped_control_changes_its_gcode_header_claim() {
    use farm3d_lib::slicing::mapping::CONTROL_MAPPINGS;

    // Each control, the value written, and the header claim expected for
    // each OrcaSlicer key it maps to.
    let cases: [(&str, Value, &[(&str, &str)]); 11] = [
        ("layerHeightMm", json!(0.12), &[("layer_height", "0.12")]),
        ("wallLoops", json!(5), &[("wall_loops", "5")]),
        ("topShellLayers", json!(6), &[("top_shell_layers", "6")]),
        (
            "bottomShellLayers",
            json!(5),
            &[("bottom_shell_layers", "5")],
        ),
        (
            "infillDensityPercent",
            json!(35),
            &[("sparse_infill_density", "35%")],
        ),
        (
            "infillPattern",
            json!("gyroid"),
            &[("sparse_infill_pattern", "gyroid")],
        ),
        (
            "supports",
            json!("tree(auto)"),
            &[("enable_support", "1"), ("support_type", "tree(auto)")],
        ),
        (
            "supportThresholdAngleDeg",
            json!(45),
            &[("support_threshold_angle", "45")],
        ),
        (
            "brimType",
            json!("outer_only"),
            &[("brim_type", "outer_only")],
        ),
        ("brimWidthMm", json!(3), &[("brim_width", "3")]),
        ("skirtLoops", json!(2), &[("skirt_loops", "2")]),
    ];
    // Every mapped control, and every key it writes, has a case.
    for (control, keys) in CONTROL_MAPPINGS {
        let case = cases
            .iter()
            .find(|(name, _, _)| *name == control)
            .unwrap_or_else(|| panic!("no case for {control}"));
        let mut expected: Vec<&str> = case.2.iter().map(|(key, _)| *key).collect();
        expected.sort_unstable();
        let mut mapped = keys.to_vec();
        mapped.sort_unstable();
        assert_eq!(expected, mapped, "{control}");
    }

    let farm = Farm::with_real_orca();
    let running = farm.start();
    let status = running.ok("check_slicer_runtime", json!({}));
    assert_eq!(status["canSlice"], true, "{status}");
    let model = running.import(&farm.source("cube-binary.stl", "cube.stl"), "managed");
    let mut preparation = running.prepare(model["id"].as_str().unwrap());

    let mut slice_with = |label: &str, controls: Value| {
        let mut document = preparation["document"].clone();
        document["controls"] = controls;
        preparation = running.ok(
            "update_preparation",
            json!({
                "preparationId": preparation["id"],
                "expectedRevision": preparation["revision"],
                "document": document,
            }),
        );
        let plate = &plates(&preparation)[0];
        let id = ids(&running.start(&format!("op-real-{label}"), &preparation, &[plate])).remove(0);
        let operation = wait_for_real_success(&running, &id);
        let revision = operation["sliceRevisionId"].as_str().unwrap();
        config_claims(&running.blob(&running.gcode_sha256(revision)))
    };

    let baseline = slice_with("baseline", json!({}));
    assert!(!baseline.is_empty(), "the G-code has no CONFIG_BLOCK");
    for (control, value, claims) in &cases {
        let header = slice_with(control, json!({ *control: value }));
        for (key, expected) in *claims {
            let before = baseline.get(*key).map(String::as_str);
            let after = header.get(*key).map(String::as_str);
            eprintln!("{control} = {value}: {key} {before:?} -> {after:?}");
            assert_eq!(
                after,
                Some(*expected),
                "{control}: the header claim of {key}"
            );
            assert_ne!(before, after, "{control}: {key} did not change");
        }
    }
}
