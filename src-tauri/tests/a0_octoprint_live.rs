//! A0.2: opt-in checks against a REAL OctoPrint instance. Ignored by
//! default; run with `just test-octoprint-live` (see
//! `docs/verification/a0-2-octoprint-live-validation.md` for the whole
//! procedure, which also covers the app-level checks these cannot).
//!
//! Environment:
//! - `FARM3D_OCTOPRINT_HOST` (required), e.g. `octopi.local` or `127.0.0.1`
//! - `FARM3D_OCTOPRINT_PORT` (default 80)
//! - `FARM3D_OCTOPRINT_API_KEY` (optional; omit for an instance with access
//!   control disabled). Never printed.
//! - `FARM3D_OCTOPRINT_POLL_SECONDS` (default 10): how long to watch status.
//!
//! The output is meant to be pasted into the validation record: it prints
//! the probe result and each status observation, never the key.

mod common;

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::{a_catalog, a_ref_json, invoke};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::octoprint::OctoPrintConnection;
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::{build_connection, ConnectionManager, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionError, ConnectionObservation, ConnectionState, PrinterConnection,
    PrinterStatus, OCTOPRINT_KIND,
};
use serde_json::json;
use tauri::test::{mock_builder, mock_context, noop_assets};
use tauri::Listener;

struct Live {
    config: ConnectionConfig,
    api_key: Option<String>,
    poll_seconds: u64,
}

fn live() -> Option<Live> {
    let host = std::env::var("FARM3D_OCTOPRINT_HOST").ok()?;
    let port = std::env::var("FARM3D_OCTOPRINT_PORT")
        .ok()
        .map(|port| port.parse().expect("FARM3D_OCTOPRINT_PORT must be a port number"))
        .unwrap_or(80);
    let api_key = std::env::var("FARM3D_OCTOPRINT_API_KEY")
        .ok()
        .filter(|key| !key.is_empty());
    let poll_seconds = std::env::var("FARM3D_OCTOPRINT_POLL_SECONDS")
        .ok()
        .map(|seconds| seconds.parse().expect("FARM3D_OCTOPRINT_POLL_SECONDS must be a number"))
        .unwrap_or(10);
    Some(Live {
        config: ConnectionConfig {
            kind: OCTOPRINT_KIND.to_string(),
            host,
            port,
            use_tls: false,
            credential_ref: None,
        },
        api_key,
        poll_seconds,
    })
}

fn require_live() -> Option<Live> {
    let live = live();
    if live.is_none() {
        eprintln!("FARM3D_OCTOPRINT_HOST is not set; skipping the live OctoPrint check");
    }
    live
}

#[tokio::test]
#[ignore = "needs a real OctoPrint instance; run `just test-octoprint-live`"]
async fn live_octoprint_probe_and_status_poll() {
    let Some(live) = require_live() else { return };
    println!(
        "target: http://{}:{} (API key {})",
        live.config.host,
        live.config.port,
        if live.api_key.is_some() { "supplied" } else { "not supplied" }
    );

    let connection = OctoPrintConnection::new(live.config.clone(), live.api_key.clone());
    let probe = connection.probe().await.expect("probe");
    println!("probe: {}", serde_json::to_string_pretty(&probe).unwrap());
    assert_eq!(probe.kind, OCTOPRINT_KIND);
    assert!(!probe.host_software.is_empty(), "OctoPrint reported no server version");

    let (tx, mut rx) = tokio::sync::mpsc::channel(16);
    let task = tokio::spawn(async move { connection.subscribe(tx).await });
    let deadline = tokio::time::Instant::now() + Duration::from_secs(live.poll_seconds);
    let mut telemetry_count = 0;
    while let Ok(Some(observation)) = tokio::time::timeout_at(deadline, rx.recv()).await {
        match &observation {
            ConnectionObservation::Telemetry(telemetry) => {
                telemetry_count += 1;
                // Compare `progress` by hand against OctoPrint's own UI:
                // 0.425 here must read as 42.5% there. The adapter clamps to
                // 0..=1, so an unconverted percentage would show as a
                // stuck 1.0, not as an out-of-range number.
                println!("telemetry: {}", serde_json::to_string(telemetry).unwrap());
            }
            ConnectionObservation::Health { state, observed_at } => {
                println!("health: {state:?} at {observed_at}");
            }
        }
    }
    drop(rx);
    let outcome = tokio::time::timeout(Duration::from_secs(15), task)
        .await
        .expect("subscribe should end once its receiver is dropped")
        .expect("subscribe task");
    assert_eq!(outcome, Ok(()), "the poll loop ended with an error");
    assert!(
        telemetry_count >= 2,
        "expected at least two polls in {}s, saw {telemetry_count}",
        live.poll_seconds
    );
}

#[tokio::test]
#[ignore = "needs a real, access-controlled OctoPrint instance; run `just test-octoprint-live`"]
async fn live_octoprint_rejects_a_wrong_key_as_a_credential_error() {
    let Some(live) = require_live() else { return };
    if live.api_key.is_none() {
        eprintln!("no FARM3D_OCTOPRINT_API_KEY: assuming access control is off; skipping");
        return;
    }
    let error = OctoPrintConnection::new(
        live.config,
        Some("farm3d-deliberately-wrong-key".to_string()),
    )
    .probe()
    .await
    .expect_err("a wrong key must not probe successfully");
    println!("wrong key: {error:?}");
    assert!(matches!(error, ConnectionError::Auth(_)), "{error:?}");
}

fn wait_for(
    label: &str,
    statuses: impl Fn() -> Option<PrinterStatus>,
    accept: impl Fn(&PrinterStatus) -> bool,
) -> PrinterStatus {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let status = statuses();
        if let Some(status) = status.as_ref().filter(|status| accept(status)) {
            return status.clone();
        }
        assert!(
            Instant::now() < deadline,
            "timed out waiting for {label}; last: {status:?}"
        );
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// The app's own setup -> supervise -> restart path (real Tauri IPC
/// commands, the production connection factory, a file-backed credential
/// store) against the live instance. The UI is the one thing this does not
/// cover; the validation record's `just dev` steps do.
#[test]
#[ignore = "needs a real OctoPrint instance; run `just test-octoprint-live`"]
fn live_octoprint_setup_supervision_and_restart_through_ipc() {
    let Some(live) = require_live() else { return };
    let (temp, _lease, storage, database) = common::storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (app, webview, manager, _services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::printers::create::probe_connection,
            farm3d_lib::printers::commands::create_printer,
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
    let submission = json!({
        "kind": "octoprint",
        "host": live.config.host,
        "port": live.config.port,
        "useTls": false,
        "credential": live.api_key.clone().unwrap_or_default(),
    });

    let probe = invoke(
        &webview,
        "probe_connection",
        json!({"contractVersion": 1, "submission": submission}),
    )
    .expect("probe_connection");
    println!("ipc probe: {}", probe["data"]);

    let created = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "Live OctoPrint",
            "catalogRef": a_ref_json(),
            "connection": submission,
        }),
    )
    .expect("create_printer");
    let id = created["data"]["printer"]["id"].as_str().unwrap().to_string();
    println!(
        "created: setupGaps={} connection={}",
        created["data"]["printer"]["setupGaps"], created["data"]["printer"]["connection"]
    );
    assert_eq!(created["data"]["printer"]["setupGaps"], json!([]));

    let status = wait_for(
        "live telemetry",
        || manager.statuses().get(&id).cloned(),
        |status| status.last_observed_at.is_some(),
    );
    println!("status: {}", serde_json::to_string(&status).unwrap());
    assert!(status.error.is_none(), "{status:?}");
    assert_ne!(status.connection_state, ConnectionState::Error);

    // Restart: the first manager stops as on quit; a new one resumes from
    // the same database and credential store.
    assert!(tauri::async_runtime::block_on(manager.stop(&id)));
    let restart_app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let restart_manager = Arc::new(ConnectionManager::with_clock_and_factory(
        restart_app.handle().clone(),
        Arc::new(StatusRepository::new(Arc::clone(&storage))),
        chrono::Utc::now,
        build_connection,
    ));
    farm3d_lib::restore_persisted_connections(
        &restart_manager,
        Arc::clone(&storage),
        &CredentialStore::file_backed(credentials_dir.path().to_path_buf()),
        &a_catalog(),
    )
    .unwrap();
    let hydrated = restart_manager.statuses().get(&id).cloned();
    println!(
        "after restart (before first poll): {}",
        serde_json::to_string(&hydrated).unwrap()
    );
    let resumed = wait_for(
        "resumed telemetry",
        || restart_manager.statuses().get(&id).cloned(),
        |status| {
            status.last_observed_at.is_some()
                && status.last_observed_at != hydrated.as_ref().and_then(|h| h.last_observed_at.clone())
        },
    );
    println!("resumed: {}", serde_json::to_string(&resumed).unwrap());
    assert!(resumed.error.is_none(), "{resumed:?}");
    tauri::async_runtime::block_on(restart_manager.stop(&id));

    if let Some(key) = live.api_key.as_deref() {
        let mut scanned = vec![
            ("probe_connection".to_string(), probe.to_string().into_bytes()),
            ("create_printer".to_string(), created.to_string().into_bytes()),
        ];
        scanned.extend(
            events
                .lock()
                .unwrap()
                .iter()
                .map(|event| ("status event".to_string(), event.clone().into_bytes())),
        );
        for suffix in ["", "-wal", "-shm"] {
            let path = format!("{}{suffix}", database.display());
            scanned.push((path.clone(), std::fs::read(&path).unwrap_or_default()));
        }
        let mut pending = vec![temp.path().to_path_buf()];
        while let Some(dir) = pending.pop() {
            for entry in std::fs::read_dir(&dir).into_iter().flatten().flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                } else {
                    scanned.push((path.display().to_string(), std::fs::read(&path).unwrap_or_default()));
                }
            }
        }
        for (label, bytes) in &scanned {
            assert!(
                !bytes.windows(key.len()).any(|window| window == key.as_bytes()),
                "{label} contains the API key"
            );
        }
        println!(
            "key scan: {} surfaces checked (IPC responses, {} status events, database, WAL, metadata/data dirs); key absent from all",
            scanned.len(),
            events.lock().unwrap().len()
        );
    }
}
