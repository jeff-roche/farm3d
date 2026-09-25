//! P6 Task 5: `printer_capabilities` and `adapter_capability_matrix` through
//! the Tauri IPC path (mirrors `tests/p2_contract_path.rs`'s harness). See
//! `.superpowers/sdd/2026-09-25-p6-connection-command-capabilities/task-5-brief.md`.

mod common;

use std::sync::Arc;

use common::{a_catalog, a_stored_printer, invoke};
use farm3d_lib::connections::{
    ConnectionConfig, PrinterConnection, MOONRAKER_KIND, OCTOPRINT_KIND,
};
use farm3d_lib::persistence::{MetadataRootLease, Storage};
use farm3d_lib::printers::repository::PrinterRepository;
use serde_json::json;

/// The two P6 Task 5 commands.
const P6_COMMANDS: &[&str] = &["printer_capabilities", "adapter_capability_matrix"];

fn storage() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>) {
    let (temp, lease, storage, _database) = common::storage();
    (temp, lease, storage)
}

/// Never actually called: every test here reads capabilities without
/// starting supervision.
fn unused_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

fn runtime(
    storage: Arc<Storage>,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
) {
    let credentials_dir = tempfile::tempdir().unwrap();
    let (app, webview, _manager, _services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::connections::commands::printer_capabilities,
            farm3d_lib::connections::commands::adapter_capability_matrix,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        unused_factory,
    );
    (app, webview)
}

#[test]
fn every_p6_task_5_command_is_registered_with_a_contract() {
    let manifest = farm3d_lib::contracts::inventory::command_contract_inventory();
    assert_eq!(manifest.len(), farm3d_lib::COMMAND_NAMES.len());
    for command in P6_COMMANDS {
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
fn a_printer_with_no_connection_is_unsupported_everywhere_over_ipc() {
    let (_temp, _lease, storage) = storage();
    PrinterRepository::new(Arc::clone(&storage))
        .create(a_stored_printer("printer-a"))
        .unwrap();
    let (_app, webview) = runtime(storage);

    let response = invoke(
        &webview,
        "printer_capabilities",
        json!({"contractVersion": 1, "printerId": "printer-a"}),
    )
    .unwrap();

    assert_eq!(response["data"]["printerId"], "printer-a");
    assert_eq!(response["data"]["adapterKind"], serde_json::Value::Null);
    assert_eq!(response["data"]["hostFacts"], serde_json::Value::Null);
    let upload = &response["data"]["capabilities"]["upload"];
    assert_eq!(upload["status"], "unsupported");
    assert_eq!(upload["reason"], "adapter");
    assert_eq!(upload["detail"], "No Connection");
}

#[test]
fn an_unknown_printer_id_is_not_found_over_ipc() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview) = runtime(storage);

    let error = invoke(
        &webview,
        "printer_capabilities",
        json!({"contractVersion": 1, "printerId": "missing"}),
    )
    .unwrap_err();

    assert_eq!(error["code"], "NOT_FOUND");
}

#[test]
fn the_adapter_matrix_lists_the_registry_in_order_all_not_verified_over_ipc() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview) = runtime(storage);

    let response = invoke(
        &webview,
        "adapter_capability_matrix",
        json!({"contractVersion": 1}),
    )
    .unwrap();

    let rows = response["data"].as_array().unwrap();
    assert_eq!(rows.len(), 2);
    assert_eq!(rows[0]["adapterKind"], MOONRAKER_KIND);
    assert_eq!(rows[1]["adapterKind"], OCTOPRINT_KIND);
    for row in rows {
        for key in [
            "upload",
            "start",
            "pause",
            "resume",
            "cancel",
            "hostState",
            "artifactIdentity",
            "camera",
        ] {
            let state = &row["capabilities"][key];
            assert_eq!(state["status"], "unsupported", "{key}");
            assert_eq!(state["reason"], "notVerified", "{key}");
        }
    }
}
