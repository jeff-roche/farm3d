#![cfg(target_os = "linux")]

use std::sync::Arc;
use std::time::{Duration, Instant};

use farm3d_lib::catalog::Catalog;
use farm3d_lib::connections::supervisor::ConnectionManager;
use farm3d_lib::connections::ConnectionState;
use farm3d_lib::restore_stored_connections;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindow, WebviewWindowBuilder};

fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: serde_json::Value,
) -> Result<serde_json::Value, serde_json::Value> {
    tauri::test::get_ipc_response(
        webview,
        InvokeRequest {
            cmd: command.to_string(),
            callback: CallbackFn(0),
            error: CallbackFn(1),
            url: "tauri://localhost".parse().unwrap(),
            body: InvokeBody::Json(body),
            headers: Default::default(),
            invoke_key: INVOKE_KEY.to_string(),
        },
    )
    .map(|response| response.deserialize::<Value>().unwrap())
}

fn mock_app(
    catalog: Arc<Catalog>,
) -> (
    tauri::App<MockRuntime>,
    Arc<ConnectionManager<MockRuntime>>,
    WebviewWindow<MockRuntime>,
) {
    let app = mock_builder()
        .manage(catalog)
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::printers::create_printer,
            farm3d_lib::printers::list_printers,
            farm3d_lib::connections::commands::set_printer_connection,
            farm3d_lib::connections::commands::printer_statuses,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let manager = Arc::new(ConnectionManager::new(app.handle().clone()));
    app.manage(Arc::clone(&manager));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, manager, webview)
}

#[test]
fn ipc_create_connection_survives_restart_and_backfills_status() {
    let original_config_home = std::env::var_os("XDG_CONFIG_HOME");
    let config_home = tempfile::tempdir().unwrap();
    std::env::set_var("XDG_CONFIG_HOME", config_home.path());

    let catalog_path =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/printer-catalog.json");
    let catalog = Arc::new(farm3d_lib::catalog::load_snapshot(&catalog_path).unwrap());
    let model = catalog.models.first().unwrap();
    let variant = model.variants.first().unwrap();
    let catalog_ref = json!({
        "vendor": model.vendor,
        "model": model.model,
        "variant": variant.variant,
        "modelId": model.model_id,
        "printerVariant": variant.printer_variant,
    });

    let (app, manager, webview) = mock_app(Arc::clone(&catalog));
    let created = invoke(
        &webview,
        "create_printer",
        json!({
            "draft": {
                "name": "F0 IPC Printer",
                "catalogRef": catalog_ref,
                "group": "F0",
            }
        }),
    )
    .unwrap();
    let printer_id = created["id"].as_str().unwrap().to_string();
    assert_eq!(created["name"], "F0 IPC Printer");

    let connected = invoke(
        &webview,
        "set_printer_connection",
        json!({
            "id": printer_id,
            "submission": {
                "kind": "moonraker",
                "host": "moonraker.invalid",
                "port": 7125,
                "useTls": false,
                "apiKey": "",
            }
        }),
    )
    .unwrap();
    assert_eq!(connected["connection"]["host"], "moonraker.invalid");
    let ipc_responses = serde_json::to_string(&json!([created, connected])).unwrap();
    assert!(!ipc_responses.contains("apiKey"));
    assert!(!ipc_responses.contains("secret"));
    assert!(!ipc_responses.contains("F0_FIXTURE_SENTINEL"));

    manager.stop(&printer_id);
    drop(webview);
    drop(app);
    drop(manager);

    let (restarted_app, restarted_manager, restarted_webview) = mock_app(catalog);
    restore_stored_connections(restarted_app.handle(), &restarted_manager);

    let deadline = Instant::now() + Duration::from_secs(2);
    while !restarted_manager.statuses().contains_key(&printer_id) && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    let statuses = invoke(&restarted_webview, "printer_statuses", json!({})).unwrap();
    let state = statuses[&printer_id]["connectionState"].as_str().unwrap();
    assert!(matches!(state, "connecting" | "offline" | "error"));
    assert!(matches!(
        restarted_manager.statuses()[&printer_id].connection_state,
        ConnectionState::Connecting | ConnectionState::Offline | ConnectionState::Error
    ));

    restarted_manager.stop(&printer_id);
    drop(restarted_webview);
    drop(restarted_app);
    drop(restarted_manager);
    config_home.close().unwrap();
    match original_config_home {
        Some(value) => std::env::set_var("XDG_CONFIG_HOME", value),
        None => std::env::remove_var("XDG_CONFIG_HOME"),
    }
}
