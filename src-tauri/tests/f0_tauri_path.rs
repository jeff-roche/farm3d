#![cfg(target_os = "linux")]

use std::io::ErrorKind;
use std::net::{TcpListener, TcpStream};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::thread::JoinHandle;
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

const CHILD_PROCESS: &str = "FARM3D_F0_TAURI_PATH_CHILD";

struct LoopbackEndpoint {
    port: u16,
    shutdown: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl LoopbackEndpoint {
    fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = listener.local_addr().unwrap().port();
        listener.set_nonblocking(true).unwrap();
        let shutdown = Arc::new(AtomicBool::new(false));
        let thread_shutdown = Arc::clone(&shutdown);
        let thread = std::thread::spawn(move || {
            while !thread_shutdown.load(Ordering::SeqCst) {
                match listener.accept() {
                    Ok((stream, _)) => close(stream),
                    Err(error) if error.kind() == ErrorKind::WouldBlock => {
                        std::thread::sleep(Duration::from_millis(5));
                    }
                    Err(error) => panic!("loopback listener failed: {error}"),
                }
            }
        });
        Self {
            port,
            shutdown,
            thread: Some(thread),
        }
    }
}

impl Drop for LoopbackEndpoint {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::SeqCst);
        if let Some(thread) = self.thread.take() {
            thread.join().unwrap();
        }
    }
}

fn close(stream: TcpStream) {
    drop(stream);
}

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
    if std::env::var_os(CHILD_PROCESS).is_some() {
        run_ipc_tracer();
        return;
    }

    let config_home = tempfile::tempdir().unwrap();
    let status = Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "ipc_create_connection_survives_restart_and_backfills_status",
            "--nocapture",
        ])
        .env(CHILD_PROCESS, "1")
        .env("XDG_CONFIG_HOME", config_home.path())
        .status()
        .unwrap();

    assert!(
        status.success(),
        "isolated IPC tracer child failed: {status}"
    );
}

fn run_ipc_tracer() {
    let endpoint = LoopbackEndpoint::start();

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
                "host": "127.0.0.1",
                "port": endpoint.port,
                "useTls": false,
                "apiKey": "",
            }
        }),
    )
    .unwrap();
    assert_eq!(connected["connection"]["host"], "127.0.0.1");
    assert_eq!(connected["connection"]["port"], endpoint.port);

    tauri::async_runtime::block_on(manager.stop_and_wait(&printer_id));
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
    let ipc_responses = serde_json::to_string(&json!([created, connected, statuses])).unwrap();
    assert!(!ipc_responses.contains("apiKey"));
    assert!(!ipc_responses.contains("secret"));
    assert!(!ipc_responses.contains("F0_FIXTURE_SENTINEL"));

    tauri::async_runtime::block_on(restarted_manager.stop_and_wait(&printer_id));
    drop(restarted_webview);
    drop(restarted_app);
    drop(restarted_manager);
}
