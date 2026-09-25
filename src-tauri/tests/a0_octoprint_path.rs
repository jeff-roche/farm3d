//! A0.2: the OctoPrint status-only monitoring path through the real Tauri
//! IPC surface and the PRODUCTION connection factory, against a canned
//! OctoPrint REST server on loopback.
//!
//! Proves the backend path the frontend relies on before it offers
//! OctoPrint: probe, create with a credential, supervise, report status in
//! the shared vocabulary, tolerate a printer disconnected from OctoPrint
//! (HTTP 409), and resume after a simulated restart — with the API key never
//! reaching the database, a status event, or an IPC response.
//!
//! This is supporting evidence only. Issue #10's completion gate is a live
//! OctoPrint instance; see `a0_octoprint_live.rs` and
//! `docs/verification/2026-09-25-a0-2-octoprint.md`.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::octoprint::OctoPrintStub;
use common::{a_catalog, a_ref_json, invoke};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::{build_connection, ConnectionManager, STATUS_EVENT};
use farm3d_lib::connections::{ConnectionState, PrinterStatus};
use serde_json::{json, Value};
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::Listener;

const API_KEY: &str = "A0-2-FIXTURE-KEY-7f3c9d1e";

fn submission(port: u16, credential: &str) -> Value {
    json!({
        "kind": "octoprint",
        "host": "127.0.0.1",
        "port": port,
        "useTls": false,
        "credential": credential,
    })
}

/// Polls `statuses` until `accept` holds, or panics with the last status.
fn wait_for(
    statuses: impl Fn() -> Option<PrinterStatus>,
    accept: impl Fn(&PrinterStatus) -> bool,
) -> PrinterStatus {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        let status = statuses();
        if let Some(status) = status.as_ref().filter(|status| accept(status)) {
            return status.clone();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for status; last: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn assert_no_key(label: &str, bytes: &[u8]) {
    assert!(
        !bytes
            .windows(API_KEY.len())
            .any(|window| window == API_KEY.as_bytes()),
        "{label} contains the OctoPrint API key"
    );
}

#[test]
fn octoprint_monitoring_is_set_up_supervised_and_resumed_after_restart() {
    let stub = OctoPrintStub::start(Some(API_KEY));
    let (temp, _lease, storage, database) = common::storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (app, webview, manager, _services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::printers::create::probe_connection,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::list_printers,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        build_connection,
    );
    let events = Arc::new(Mutex::new(Vec::<String>::new()));
    {
        let events = Arc::clone(&events);
        app.listen(STATUS_EVENT, move |event| {
            events.lock().unwrap().push(event.payload().to_string());
        });
    }

    // --- 1. A wrong key is a credential problem, not a reachability one ---
    let rejected = invoke(
        &webview,
        "probe_connection",
        json!({"contractVersion": 1, "submission": submission(stub.port, "wrong-key")}),
    )
    .unwrap_err();
    assert_eq!(rejected["code"], "AUTHENTICATION_FAILED");

    // --- 2. The right key probes OctoPrint's identity and build volume ----
    let probe = invoke(
        &webview,
        "probe_connection",
        json!({"contractVersion": 1, "submission": submission(stub.port, API_KEY)}),
    )
    .unwrap();
    assert_eq!(probe["data"]["kind"], "octoprint");
    assert_eq!(probe["data"]["hostSoftware"], "1.10.3");
    assert_eq!(probe["data"]["reportedName"], "Stub Printer");
    assert_eq!(probe["data"]["state"], "Operational");
    assert_eq!(probe["data"]["reported"]["bedWidthMm"], 256.0);

    // --- 3. Create a Printer with the credential; it is supervisable ------
    let created = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "OctoPrint Printer",
            "catalogRef": a_ref_json(),
            "connection": submission(stub.port, API_KEY),
        }),
    )
    .unwrap();
    let id = created["data"]["printer"]["id"].as_str().unwrap().to_string();
    assert_eq!(created["data"]["printer"]["setupGaps"], json!([]));
    assert_eq!(created["data"]["printer"]["connection"]["kind"], "octoprint");
    assert!(created["data"]["printer"]["connection"]["credentialRef"].is_string());

    // --- 4. Live status in the shared vocabulary, percentage converted ----
    let status = wait_for(
        || manager.statuses().get(&id).cloned(),
        |status| status.telemetry.progress.is_some(),
    );
    assert_eq!(status.connection_state, ConnectionState::Online);
    assert_eq!(status.telemetry.progress, Some(0.425));
    assert_eq!(status.telemetry.nozzle_temp_c, Some(214.5));
    assert_eq!(status.telemetry.bed_target_c, Some(60.0));
    assert_eq!(status.telemetry.job_name.as_deref(), Some("benchy.gcode"));
    assert_eq!(
        serde_json::to_value(status.operational_state).unwrap(),
        "printing"
    );

    // --- 5. Printer disconnected from OctoPrint: 409, still supervised ----
    stub.state.lock().unwrap().printer_attached = false;
    let offline = wait_for(
        || manager.statuses().get(&id).cloned(),
        |status| status.connection_state == ConnectionState::Offline,
    );
    assert_eq!(offline.telemetry.nozzle_temp_c, None);
    assert_eq!(offline.telemetry.host_activity_name.as_deref(), Some("Offline"));
    assert!(offline.error.is_none(), "a 409 is not a connection error");
    stub.state.lock().unwrap().printer_attached = true;
    wait_for(
        || manager.statuses().get(&id).cloned(),
        |status| status.connection_state == ConnectionState::Online,
    );

    // --- 6. Simulated restart over the same storage and credential store --
    // The first manager goes away as it would on quit.
    assert!(tauri::async_runtime::block_on(manager.stop(&id)));
    let restart_app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let restart_manager = Arc::new(ConnectionManager::with_clock_and_factory(
        restart_app.handle().clone(),
        Arc::new(StatusRepository::new(Arc::clone(&storage))),
        chrono::Utc::now,
        build_connection,
    ));
    let requests_before_restart = stub.requests().len();
    farm3d_lib::restore_persisted_connections(
        &restart_manager,
        Arc::clone(&storage),
        &CredentialStore::file_backed(credentials_dir.path().to_path_buf()),
        &a_catalog(),
    )
    .unwrap();
    let resumed = wait_for(
        || restart_manager.statuses().get(&id).cloned(),
        |status| {
            status.connection_state == ConnectionState::Online
                && status.telemetry.progress == Some(0.425)
                && status.last_observed_at.is_some()
        },
    );
    assert!(resumed.error.is_none());
    let after_restart = &stub.requests()[requests_before_restart..];
    assert!(!after_restart.is_empty());
    assert!(
        after_restart
            .iter()
            .all(|request| request.headers.get("x-api-key").map(String::as_str) == Some(API_KEY)),
        "the restarted supervisor must read the key back from the credential store"
    );

    // --- 7. The key never left the credential store ------------------------
    let listed = invoke(&webview, "list_printers", json!({"contractVersion": 1})).unwrap();
    assert_no_key("list_printers", listed.to_string().as_bytes());
    assert_no_key("create_printer", created.to_string().as_bytes());
    assert_no_key("probe_connection", probe.to_string().as_bytes());
    for (label, status) in [("status", &status), ("resumed status", &resumed)] {
        assert_no_key(label, serde_json::to_string(status).unwrap().as_bytes());
    }
    let events = events.lock().unwrap().clone();
    assert!(!events.is_empty(), "status events should have been emitted");
    for event in &events {
        assert_no_key("status event", event.as_bytes());
    }
    for suffix in ["", "-wal", "-shm"] {
        let path = format!("{}{suffix}", database.display());
        assert_no_key(&path, &std::fs::read(&path).unwrap_or_default());
    }
    for entry in walk(temp.path()) {
        assert_no_key(&entry.display().to_string(), &std::fs::read(&entry).unwrap_or_default());
    }

    tauri::async_runtime::block_on(restart_manager.stop(&id));
}

fn walk(root: &std::path::Path) -> Vec<std::path::PathBuf> {
    let mut files = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
            } else {
                files.push(path);
            }
        }
    }
    files
}
