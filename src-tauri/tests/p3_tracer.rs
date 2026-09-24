//! P3 task 12: the end-to-end tracer through the Tauri IPC path (spec
//! acceptance criterion 14).
//!
//! A Profile-only Printer X (default `[Main]` layout), a Spool S1 created in
//! storage and loaded into Main, a Spool S2 loaded into Main displacing S1
//! back to storage (the swap), S2 moved on to another storage location, and
//! a scale measurement on S1 that moves it from estimated to measured
//! confidence. Then a simulated app restart — a brand-new `RuntimeServices`
//! and IPC app rebuilt over the *same* `Storage` (mirrors `p2_tracer.rs`,
//! which does the same for `ConnectionManager`/`CredentialStore` rather than
//! reopening the on-disk database) — and `spool_history`/`list_spools`
//! prove the ledger, movement history, and locations are intact and
//! ordered.
//!
//! See
//! `.superpowers/sdd/2026-09-23-p3-spools-material-slots/task-12-brief.md`.

mod common;

use std::sync::Arc;

use common::{a_catalog, a_ref_json, invoke};
use serde_json::{json, Value};

fn no_connection(
    _config: &farm3d_lib::connections::ConnectionConfig,
    _secret: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn farm3d_lib::connections::PrinterConnection>> {
    None
}

fn spool_fields(manufacturer: &str, color: &str) -> Value {
    json!({
        "manufacturer": manufacturer,
        "materialFamily": "PLA",
        "colorName": color,
        "diameter": "1.75",
        "nominalMg": 1_000_000,
        "lowThresholdMg": 100_000,
    })
}

fn amount_kinds(history: &Value) -> Vec<String> {
    history["amountEvents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["kind"].as_str().unwrap().to_string())
        .collect()
}

fn confidence_after(history: &Value) -> Vec<String> {
    history["amountEvents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| event["confidenceAfter"].as_str().unwrap().to_string())
        .collect()
}

fn movement_reasons(history: &Value) -> Vec<String> {
    history["movements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|movement| movement["reason"].as_str().unwrap().to_string())
        .collect()
}

fn find_spool<'a>(spools: &'a [Value], id: &str) -> &'a Value {
    spools
        .iter()
        .find(|spool| spool["id"] == json!(id))
        .unwrap_or_else(|| panic!("no Spool {id} in list_spools: {spools:?}"))
}

#[test]
fn the_tracer_loads_swaps_relocates_measures_and_survives_a_restart() {
    let (_temp, _lease, storage, _database) = common::storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (_app, webview, _manager, _services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::spools::commands::list_spools,
            farm3d_lib::spools::commands::spool_history,
            farm3d_lib::spools::commands::create_spool,
            farm3d_lib::spools::commands::move_spool,
            farm3d_lib::spools::commands::record_spool_amount,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        no_connection,
    );

    // --- 1. Printer X: Profile-only, gets the default [Main] layout --------

    let printer = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "Printer X",
            "catalogRef": a_ref_json(),
        }),
    )
    .unwrap()["data"]["printer"]
        .clone();
    assert_eq!(printer["setupGaps"], json!(["missingConnection"]));
    let slots = printer["materialSlots"].as_array().unwrap();
    assert_eq!(slots.len(), 1, "a fresh Printer gets exactly one slot");
    assert_eq!(slots[0]["name"], json!("Main"));
    let main_slot_id = slots[0]["id"].as_str().unwrap().to_string();

    // --- 2. S1: 1 kg, estimated, in storage "Shelf" -------------------------

    let s1 = invoke(
        &webview,
        "create_spool",
        json!({
            "contractVersion": 1,
            "fields": spool_fields("Polymaker", "Black"),
            "initialAmount": {"kind": "net", "netMg": 1_000_000, "confidence": "estimated"},
            "storageLabel": "Shelf",
        }),
    )
    .unwrap()["data"]["spool"]
        .clone();
    let s1_id = s1["id"].as_str().unwrap().to_string();
    assert_eq!(s1["facets"]["confidence"], json!("estimated"));
    assert_eq!(
        s1["location"],
        json!({"kind": "storage", "storageLabel": "Shelf"})
    );

    // --- 3. Move S1 into X's Main slot ---------------------------------------

    let s1_loaded = invoke(
        &webview,
        "move_spool",
        json!({
            "contractVersion": 1,
            "operationId": uuid::Uuid::new_v4().to_string(),
            "spoolId": s1_id,
            "expectedSpoolRevision": s1["revision"],
            "destination": {
                "kind": "slot",
                "slotId": main_slot_id,
                "expectedOccupantSpoolId": null,
            },
        }),
    )
    .unwrap()["data"]["spools"][0]
        .clone();
    assert_eq!(s1_loaded["location"]["slotId"], json!(main_slot_id));

    // --- 4. Create S2 and load it into Main, displacing S1 to "Shelf" ------
    //        (the swap)

    let s2 = invoke(
        &webview,
        "create_spool",
        json!({
            "contractVersion": 1,
            "fields": spool_fields("Prusament", "Galaxy Black"),
            "initialAmount": {"kind": "net", "netMg": 1_000_000, "confidence": "estimated"},
        }),
    )
    .unwrap()["data"]["spool"]
        .clone();
    let s2_id = s2["id"].as_str().unwrap().to_string();

    let swap = invoke(
        &webview,
        "move_spool",
        json!({
            "contractVersion": 1,
            "operationId": uuid::Uuid::new_v4().to_string(),
            "spoolId": s2_id,
            "expectedSpoolRevision": s2["revision"],
            "destination": {
                "kind": "slot",
                "slotId": main_slot_id,
                "expectedOccupantSpoolId": s1_id,
                "displacedStorageLabel": "Shelf",
            },
        }),
    )
    .unwrap()["data"]
        .clone();
    // D6: the moved Spool first, then the displaced one.
    let swap_spools = swap["spools"].as_array().unwrap();
    assert_eq!(swap_spools[0]["id"], json!(s2_id));
    assert_eq!(swap_spools[0]["location"]["slotId"], json!(main_slot_id));
    assert_eq!(swap_spools[1]["id"], json!(s1_id));
    assert_eq!(
        swap_spools[1]["location"],
        json!({"kind": "storage", "storageLabel": "Shelf"})
    );
    let swap_reasons: Vec<&str> = swap["movements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|m| m["reason"].as_str().unwrap())
        .collect();
    assert_eq!(swap_reasons, vec!["displaced", "load"]);
    // S1's revision after being displaced, for the scale entry below.
    let s1_after_swap_revision = swap_spools[1]["revision"].clone();

    // --- 5. Move S2 (currently loaded) to storage "Dry box" -----------------

    let relocated = invoke(
        &webview,
        "move_spool",
        json!({
            "contractVersion": 1,
            "operationId": uuid::Uuid::new_v4().to_string(),
            "spoolId": s2_id,
            "expectedSpoolRevision": swap_spools[0]["revision"],
            "destination": {"kind": "storage", "storageLabel": "Dry box"},
        }),
    )
    .unwrap()["data"]["spools"][0]
        .clone();
    assert_eq!(
        relocated["location"],
        json!({"kind": "storage", "storageLabel": "Dry box"})
    );

    // --- 6. Record S1's amount by scale: estimated -> measured --------------

    let measured = invoke(
        &webview,
        "record_spool_amount",
        json!({
            "contractVersion": 1,
            "id": s1_id,
            "expectedRevision": s1_after_swap_revision,
            "entry": {"kind": "scale", "grossMg": 750_000, "tareMg": 50_000},
            "note": "kitchen scale",
        }),
    )
    .unwrap()["data"]["spool"]
        .clone();
    assert_eq!(measured["facets"]["confidence"], json!("measured"));
    assert_eq!(measured["availability"]["currentMg"], json!(700_000));

    // --- 7. Restart: rebuild the runtime over the same storage paths -------
    //        (mirrors p2_tracer.rs's restart, which rebuilds services rather
    //        than reopening the on-disk database file directly).

    let restart_credentials = tempfile::tempdir().unwrap();
    let (_restart_app, restart_webview, _restart_manager, _restart_services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::spools::commands::list_spools,
            farm3d_lib::spools::commands::spool_history,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        restart_credentials.path().to_path_buf(),
        no_connection,
    );

    // --- 8. spool_history for S1 and S2; list_spools locations -------------

    let s1_history = invoke(
        &restart_webview,
        "spool_history",
        json!({"contractVersion": 1, "spoolId": s1_id}),
    )
    .unwrap()["data"]
        .clone();
    assert_eq!(amount_kinds(&s1_history), vec!["initial", "measurement"]);
    assert_eq!(confidence_after(&s1_history), vec!["estimated", "measured"]);
    assert_eq!(
        s1_history["amountEvents"][1]["afterMg"],
        json!(700_000),
        "S1's history: {s1_history}"
    );
    assert_eq!(
        s1_history["amountEvents"][1]["note"],
        json!("kitchen scale")
    );
    assert_eq!(
        movement_reasons(&s1_history),
        vec!["load", "displaced"],
        "S1: loaded into Main, then displaced back to storage by the swap"
    );

    let s2_history = invoke(
        &restart_webview,
        "spool_history",
        json!({"contractVersion": 1, "spoolId": s2_id}),
    )
    .unwrap()["data"]
        .clone();
    assert_eq!(amount_kinds(&s2_history), vec!["initial"]);
    assert_eq!(confidence_after(&s2_history), vec!["estimated"]);
    assert_eq!(
        movement_reasons(&s2_history),
        vec!["load", "unload"],
        "S2: loaded into Main by the swap, then unloaded on to storage"
    );

    let snapshot = invoke(
        &restart_webview,
        "list_spools",
        json!({"contractVersion": 1}),
    )
    .unwrap()["data"]
        .clone();
    let spools = snapshot["spools"].as_array().unwrap();
    let s1_final = find_spool(spools, &s1_id);
    assert_eq!(
        s1_final["location"],
        json!({"kind": "storage", "storageLabel": "Shelf"})
    );
    assert_eq!(s1_final["facets"]["confidence"], json!("measured"));
    assert_eq!(s1_final["availability"]["currentMg"], json!(700_000));

    let s2_final = find_spool(spools, &s2_id);
    assert_eq!(
        s2_final["location"],
        json!({"kind": "storage", "storageLabel": "Dry box"})
    );
    assert_eq!(s2_final["facets"]["confidence"], json!("estimated"));
}
