//! P2 task 12: the end-to-end tracer through the Tauri IPC path (spec
//! acceptance criterion 16).
//!
//! One Profile-only Printer, one connected Printer, a 4-row batch across
//! "Bay A" and "Bay B" with one row (`auth.local`) retained as
//! `createdSetupIncomplete`, one Printer archived, then a simulated app
//! restart over the *same* storage (mirrors `p2_lifecycle.rs`'s restart
//! pattern, which drives `restore_persisted_connections` directly rather
//! than through IPC): the archived Printer is still listed by id and name
//! with `archivedAt` set and no status, and every other Printer is present
//! with its correct setup state.
//!
//! See `.superpowers/sdd/2026-09-22-p2-printer-lifecycle-batch-setup/task-12-brief.md`.

mod common;

use std::sync::Arc;

use common::{a_catalog, a_ref_json, invoke, FakeConnection};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::ConnectionManager;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use serde_json::{json, Value};
use tauri::test::{mock_builder, mock_context, noop_assets};

fn none() -> Value {
    json!({"source": "none"})
}

fn connection(host: &str, port: u16) -> Value {
    json!({"kind": "moonraker", "host": host, "port": port, "useTls": false, "credential": none()})
}

fn printer_row<'a>(rows: &'a [Value], id: &str) -> &'a Value {
    rows.iter()
        .find(|row| row["id"] == json!(id))
        .unwrap_or_else(|| panic!("no Printer row with id {id} in {rows:?}"))
}

fn setup_gaps(printer: &Value) -> Vec<String> {
    printer["setupGaps"]
        .as_array()
        .unwrap()
        .iter()
        .map(|gap| gap.as_str().unwrap().to_string())
        .collect()
}

fn factory(
    config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    Some(Box::new(FakeConnection {
        host: config.host.clone(),
    }) as Box<dyn PrinterConnection>)
}

#[test]
fn the_tracer_creates_a_batch_archives_one_printer_and_survives_a_restart() {
    let (_temp, _lease, storage, _database) = common::storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (_app, webview, _manager, _services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::printers::commands::list_printers,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::archive_printer,
            farm3d_lib::printers::batch::create_printers_batch,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );

    // --- 1. A Profile-only Printer ------------------------------------------

    let profile_only = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "Profile Only",
            "catalogRef": a_ref_json(),
        }),
    )
    .unwrap();
    let profile_only_id = profile_only["data"]["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        profile_only["data"]["printer"]["setupGaps"],
        json!(["missingConnection"])
    );

    // --- 2. A connected Printer (fake factory) ------------------------------

    let connected = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "Connected Printer",
            "catalogRef": a_ref_json(),
            "connection": {
                "kind": "moonraker",
                "host": "voron.local",
                "port": 7125,
                "useTls": false,
                "credential": "s3cret-FIXTURE",
            },
        }),
    )
    .unwrap();
    let connected_id = connected["data"]["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(connected["data"]["printer"]["setupGaps"], json!([]));

    // --- 3. A 4-row batch across "Bay A" and "Bay B", one `auth.local` -----

    let batch_result = invoke(
        &webview,
        "create_printers_batch",
        json!({
            "contractVersion": 1,
            "input": {
                "batchId": format!("batch-{}", uuid::Uuid::new_v4()),
                "shared": {"catalogRef": a_ref_json(), "startSafety": "confirmBedClear"},
                "probe": true,
                "rows": [
                    {"rowId": "r1", "name": "Bay A 1", "location": "Bay A", "connection": connection("ok.local", 7125)},
                    {"rowId": "r2", "name": "Bay A 2", "location": "Bay A", "connection": connection("auth.local", 7125)},
                    {"rowId": "r3", "name": "Bay B 1", "location": "Bay B"},
                    {"rowId": "r4", "name": "Bay B 2", "location": "Bay B", "connection": connection("unreachable.local", 7125)},
                ],
            },
        }),
    )
    .unwrap();
    let batch_rows = batch_result["data"]["rows"].as_array().unwrap();
    let row_by_id = |row_id: &str| -> &Value {
        batch_rows
            .iter()
            .find(|row| row["rowId"] == json!(row_id))
            .unwrap_or_else(|| panic!("no batch result row {row_id}"))
    };

    let r1 = row_by_id("r1");
    assert_eq!(r1["outcome"], "created", "r1 (ok.local): {r1}");
    let bay_a_1_id = r1["printer"]["id"].as_str().unwrap().to_string();
    let bay_a_1_revision = r1["printer"]["revision"].as_i64().unwrap();

    let r2 = row_by_id("r2");
    assert_eq!(
        r2["outcome"], "createdSetupIncomplete",
        "the auth.local row must still be created, without a Connection: {r2}"
    );
    assert_eq!(
        r2["errors"].as_array().unwrap()[0]["code"],
        "AUTHENTICATION_FAILED"
    );
    assert_eq!(r2["printer"]["connection"], Value::Null);
    let bay_a_2_id = r2["printer"]["id"].as_str().unwrap().to_string();

    let r3 = row_by_id("r3");
    assert_eq!(r3["outcome"], "createdSetupIncomplete", "r3 (no connection): {r3}");
    let bay_b_1_id = r3["printer"]["id"].as_str().unwrap().to_string();

    let r4 = row_by_id("r4");
    assert_eq!(
        r4["outcome"], "createdSetupIncomplete",
        "r4 (unreachable.local): {r4}"
    );
    let bay_b_2_id = r4["printer"]["id"].as_str().unwrap().to_string();

    // --- 4. Archive one Printer (the batch-created, connected one) ---------

    let archived = invoke(
        &webview,
        "archive_printer",
        json!({"contractVersion": 1, "id": bay_a_1_id, "expectedRevision": bay_a_1_revision}),
    )
    .unwrap();
    assert!(archived["data"]["printer"]["archivedAt"].is_string());

    // --- 5. Rebuild the services over the same storage (restart) -----------
    // Mirrors `p2_lifecycle.rs`'s
    // `after_archiving_a_restart_never_supervises_the_archived_printer_but_keeps_it_listed`:
    // a brand-new manager/credential store driven through the same
    // `restore_persisted_connections` path `lib.rs` uses at real startup.

    let restart_app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let restart_manager = Arc::new(ConnectionManager::with_clock_and_factory(
        restart_app.handle().clone(),
        Arc::new(StatusRepository::new(Arc::clone(&storage))),
        chrono::Utc::now,
        factory,
    ));
    let restart_credentials =
        CredentialStore::file_backed(tempfile::tempdir().unwrap().path().to_path_buf());
    let catalog = a_catalog();
    farm3d_lib::restore_persisted_connections(
        &restart_manager,
        Arc::clone(&storage),
        &restart_credentials,
        &catalog,
    )
    .unwrap();

    // --- 6. After restart: the archived Printer keeps its identity and has
    //        no status; every other Printer is present with its correct
    //        setup state. -----------------------------------------------

    let listed = invoke(&webview, "list_printers", json!({"contractVersion": 1})).unwrap();
    let rows = listed["data"].as_array().unwrap().clone();
    assert_eq!(rows.len(), 6, "all six Printers must still be listed: {rows:?}");

    let archived_row = printer_row(&rows, &bay_a_1_id);
    assert_eq!(archived_row["name"], json!("Bay A 1"));
    assert!(archived_row["archivedAt"].is_string());
    assert!(
        !restart_manager.statuses().contains_key(&bay_a_1_id),
        "an archived Printer must have no status after restart"
    );

    let profile_only_row = printer_row(&rows, &profile_only_id);
    assert_eq!(profile_only_row["name"], json!("Profile Only"));
    assert_eq!(profile_only_row["archivedAt"], Value::Null);
    assert_eq!(setup_gaps(profile_only_row), vec!["missingConnection"]);

    let connected_row = printer_row(&rows, &connected_id);
    assert_eq!(connected_row["name"], json!("Connected Printer"));
    assert_eq!(connected_row["archivedAt"], Value::Null);
    assert!(setup_gaps(connected_row).is_empty());
    assert!(
        restart_manager.statuses().contains_key(&connected_id),
        "the connected Printer must be resupervised after restart"
    );

    let bay_a_2_row = printer_row(&rows, &bay_a_2_id);
    assert_eq!(bay_a_2_row["name"], json!("Bay A 2"));
    assert_eq!(bay_a_2_row["archivedAt"], Value::Null);
    assert_eq!(setup_gaps(bay_a_2_row), vec!["missingConnection"]);
    assert_eq!(bay_a_2_row["location"], json!("Bay A"));

    let bay_b_1_row = printer_row(&rows, &bay_b_1_id);
    assert_eq!(bay_b_1_row["name"], json!("Bay B 1"));
    assert_eq!(bay_b_1_row["archivedAt"], Value::Null);
    assert_eq!(setup_gaps(bay_b_1_row), vec!["missingConnection"]);
    assert_eq!(bay_b_1_row["location"], json!("Bay B"));

    let bay_b_2_row = printer_row(&rows, &bay_b_2_id);
    assert_eq!(bay_b_2_row["name"], json!("Bay B 2"));
    assert_eq!(bay_b_2_row["archivedAt"], Value::Null);
    assert_eq!(setup_gaps(bay_b_2_row), vec!["missingConnection"]);
    assert_eq!(bay_b_2_row["location"], json!("Bay B"));
}
