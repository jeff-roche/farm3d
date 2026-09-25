//! P2 task 5: `create_printers_batch` / `cancel_printer_batch` — bounded
//! probing, per-row commits, and cancellation (spec D1, D3, D9, D13, D14).
//! See `.superpowers/sdd/2026-09-22-p2-printer-lifecycle-batch-setup/task-5-brief.md`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{a_catalog, a_ref_json, a_stored_printer, fixture_probe, invoke, ready_probe};
use farm3d_lib::connections::supervisor::ConnectionManager;
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionError, ConnectionObservation, PrinterConnection, ProbeResult,
    ReportedCapabilities, MOONRAKER_KIND,
};
use farm3d_lib::persistence::Storage;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tokio::sync::Semaphore;

const SHARED_SECRET: &str = "SHARED-s3cret-FIXTURE";
const ROW_SECRET: &str = "ROW-s3cret-FIXTURE";
/// How long a test waits for an event that must arrive before failing
/// rather than hanging — a safety net, never a synchronisation mechanism.
const EVENT_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug)]
enum Event {
    /// A `gate.local` probe has started (and is now counted in-flight).
    ProbeStarted,
    /// Supervision called `subscribe` for this endpoint; `committed` says
    /// whether a Printer with that Connection was already in storage.
    Subscribed {
        host: String,
        port: u16,
        committed: bool,
    },
}

/// Test-side controls for the fake connection factory.
struct Harness {
    /// `gate.local` probes wait for one permit each.
    gate: Arc<Semaphore>,
    max_in_flight: Arc<AtomicUsize>,
    events: Receiver<Event>,
}

struct BatchFake {
    config: ConnectionConfig,
    storage: Arc<Storage>,
    gate: Arc<Semaphore>,
    in_flight: Arc<AtomicUsize>,
    max_in_flight: Arc<AtomicUsize>,
    events: Sender<Event>,
}

/// Decrements the in-flight gauge even when the probe future is dropped
/// mid-wait (a cancelled batch drops in-flight probes).
struct InFlight(Arc<AtomicUsize>);

impl Drop for InFlight {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

#[async_trait::async_trait]
impl PrinterConnection for BatchFake {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        match self.config.host.as_str() {
            "gate.local" => {
                let now = self.in_flight.fetch_add(1, Ordering::SeqCst) + 1;
                let _in_flight = InFlight(Arc::clone(&self.in_flight));
                self.max_in_flight.fetch_max(now, Ordering::SeqCst);
                let _ = self.events.send(Event::ProbeStarted);
                self.gate.acquire().await.unwrap().forget();
                Ok(ready_probe(ReportedCapabilities::default()))
            }
            "mismatch.local" => Ok(ready_probe(ReportedCapabilities {
                bed_width_mm: Some(266.0),
                bed_depth_mm: Some(256.0),
                printable_height_mm: None,
            })),
            other => fixture_probe(other),
        }
    }

    async fn subscribe(
        &self,
        _tx: tokio::sync::mpsc::Sender<ConnectionObservation>,
    ) -> Result<(), ConnectionError> {
        let committed = PrinterRepository::new(Arc::clone(&self.storage))
            .list()
            .unwrap_or_default()
            .iter()
            .any(|printer| {
                printer.connection.as_ref().is_some_and(|connection| {
                    connection.host == self.config.host && connection.port == self.config.port
                })
            });
        let _ = self.events.send(Event::Subscribed {
            host: self.config.host.clone(),
            port: self.config.port,
            committed,
        });
        std::future::pending::<()>().await;
        Ok(())
    }
}

struct Fixture {
    _temp: tempfile::TempDir,
    _lease: farm3d_lib::persistence::MetadataRootLease,
    storage: Arc<Storage>,
    database: PathBuf,
    credentials_dir: tempfile::TempDir,
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    manager: Arc<ConnectionManager<MockRuntime>>,
    _services: Arc<RuntimeServices<MockRuntime>>,
    harness: Harness,
}

fn fixture() -> Fixture {
    let (temp, lease, storage, database) = common::storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let gate = Arc::new(Semaphore::new(0));
    let in_flight = Arc::new(AtomicUsize::new(0));
    let max_in_flight = Arc::new(AtomicUsize::new(0));
    let (tx, events) = mpsc::channel();
    let tx = Mutex::new(tx);
    let factory = {
        let storage = Arc::clone(&storage);
        let gate = Arc::clone(&gate);
        let max_in_flight = Arc::clone(&max_in_flight);
        move |config: &ConnectionConfig, _key: Option<zeroize::Zeroizing<String>>| {
            Some(Box::new(BatchFake {
                config: config.clone(),
                storage: Arc::clone(&storage),
                gate: Arc::clone(&gate),
                in_flight: Arc::clone(&in_flight),
                max_in_flight: Arc::clone(&max_in_flight),
                events: tx.lock().unwrap().clone(),
            }) as Box<dyn PrinterConnection>)
        }
    };
    let (app, webview, manager, services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::printers::commands::list_printers,
            farm3d_lib::printers::commands::set_printer_override,
            farm3d_lib::printers::batch::create_printers_batch,
            farm3d_lib::printers::batch::cancel_printer_batch,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );
    Fixture {
        _temp: temp,
        _lease: lease,
        storage,
        database,
        credentials_dir,
        _app: app,
        webview,
        manager,
        _services: services,
        harness: Harness {
            gate,
            max_in_flight,
            events,
        },
    }
}

fn batch_id() -> String {
    format!("batch-{}", uuid::Uuid::new_v4())
}

fn connection(host: &str, port: u16, credential: Value) -> Value {
    json!({"kind": "moonraker", "host": host, "port": port, "useTls": false, "credential": credential})
}

fn none() -> Value {
    json!({"source": "none"})
}

fn shared() -> Value {
    json!({"source": "shared"})
}

fn row_secret(value: &str) -> Value {
    json!({"source": "row", "value": value})
}

fn batch_body(
    batch_id: &str,
    shared: Value,
    shared_credential: Option<&str>,
    probe: bool,
    rows: Value,
) -> Value {
    let mut input = json!({
        "batchId": batch_id,
        "shared": shared,
        "probe": probe,
        "rows": rows,
    });
    if let Some(secret) = shared_credential {
        input["sharedCredential"] = json!(secret);
    }
    json!({"contractVersion": 1, "input": input})
}

fn shared_block(extra: Value) -> Value {
    let mut block = json!({"catalogRef": a_ref_json(), "startSafety": "confirmBedClear"});
    for (key, value) in extra.as_object().unwrap() {
        block[key] = value.clone();
    }
    block
}

fn run_batch(fixture: &Fixture, body: Value) -> Result<Value, Value> {
    invoke(&fixture.webview, "create_printers_batch", body)
}

fn list_printers(fixture: &Fixture) -> Vec<Value> {
    invoke(
        &fixture.webview,
        "list_printers",
        json!({"contractVersion": 1}),
    )
    .unwrap()["data"]
        .as_array()
        .unwrap()
        .clone()
}

fn row<'a>(result: &'a Value, row_id: &str) -> &'a Value {
    result["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["rowId"] == json!(row_id))
        .unwrap_or_else(|| panic!("no result row {row_id}: {result}"))
}

fn error_codes(row: &Value) -> Vec<String> {
    row["errors"]
        .as_array()
        .unwrap()
        .iter()
        .map(|error| error["code"].as_str().unwrap().to_string())
        .collect()
}

fn warning_codes(row: &Value) -> Vec<String> {
    row["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|warning| warning["code"].as_str().unwrap().to_string())
        .collect()
}

fn next_event(harness: &Harness) -> Event {
    harness
        .events
        .recv_timeout(EVENT_TIMEOUT)
        .expect("expected a fake-connection event")
}

fn wait_for_probe_starts(harness: &Harness, count: usize) {
    let mut started = 0;
    while started < count {
        if let Event::ProbeStarted = next_event(harness) {
            started += 1;
        }
    }
}

/// Runs the batch on its own thread so the test can drive the gate and
/// cancel while it is in flight.
fn spawn_batch(fixture: &Fixture, body: Value) -> std::thread::JoinHandle<Result<Value, Value>> {
    let webview = fixture.webview.clone();
    std::thread::spawn(move || invoke(&webview, "create_printers_batch", body))
}

// --- 1. Partial success -----------------------------------------------------

#[test]
fn a_partial_batch_creates_valid_rows_and_rejects_only_invalid_identity() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})),
            Some(SHARED_SECRET),
            true,
            json!([
                {"rowId": "r1", "name": "Bay A 1", "location": "Bay A", "connection": connection("ok.local", 7125, shared())},
                {"rowId": "r2", "name": "Bay A 2", "connection": connection("auth.local", 7125, none())},
                {"rowId": "r3", "name": ""},
                {"rowId": "r4", "name": "Bay A 4"},
            ]),
        ),
    )
    .unwrap();

    let ids: Vec<&str> = result["data"]["rows"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["rowId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, ["r1", "r2", "r3", "r4"], "request order");

    let r1 = row(&result, "r1");
    assert_eq!(r1["outcome"], "created");
    assert_eq!(r1["credentialStored"], true);
    assert_eq!(r1["printer"]["location"], "Bay A");
    assert_eq!(r1["probe"]["state"], "ready");
    assert_eq!(error_codes(r1), Vec::<String>::new());

    let r2 = row(&result, "r2");
    assert_eq!(r2["outcome"], "createdSetupIncomplete");
    assert_eq!(error_codes(r2), ["AUTHENTICATION_FAILED"]);
    assert_eq!(r2["printer"]["connection"], Value::Null);
    assert_eq!(r2["credentialStored"], false);

    let r3 = row(&result, "r3");
    assert_eq!(r3["outcome"], "rejected");
    assert_eq!(error_codes(r3), ["VALIDATION"]);
    assert_eq!(r3["errors"][0]["fieldPath"], "rows[2].name");
    assert!(r3.get("printer").is_none());

    let r4 = row(&result, "r4");
    assert_eq!(r4["outcome"], "createdSetupIncomplete");
    assert_eq!(error_codes(r4), Vec::<String>::new());

    let printers = list_printers(&fixture);
    assert_eq!(printers.len(), 3);
    let persisted: Vec<&str> = printers
        .iter()
        .map(|p| p["name"].as_str().unwrap())
        .collect();
    assert!(
        persisted.contains(&"Bay A 2"),
        "r2 is persisted: {persisted:?}"
    );
    assert!(!persisted.contains(&""), "r3 is not persisted");
}

// --- 2. Correlation ---------------------------------------------------------

#[test]
fn duplicate_or_empty_row_ids_fail_the_whole_request_and_persist_nothing() {
    let fixture = fixture();
    for rows in [
        json!([{"rowId": "r1", "name": "A"}, {"rowId": "r1", "name": "B"}]),
        json!([{"rowId": "r1", "name": "A"}, {"rowId": "", "name": "B"}]),
    ] {
        let error = run_batch(
            &fixture,
            batch_body(&batch_id(), shared_block(json!({})), None, false, rows),
        )
        .unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{error}");
    }
    assert!(list_printers(&fixture).is_empty());
}

#[test]
fn an_unresolvable_shared_catalog_ref_rejects_every_row() {
    let fixture = fixture();
    let mut shared = shared_block(json!({}));
    shared["catalogRef"]["variant"] = json!("No Such Variant");
    shared["catalogRef"]["model"] = json!("No Such Model");
    shared["catalogRef"]["modelId"] = json!("nope");
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared,
            None,
            false,
            json!([{"rowId": "r1", "name": "A"}, {"rowId": "r2", "name": "B"}]),
        ),
    )
    .unwrap();
    for row_id in ["r1", "r2"] {
        let row = row(&result, row_id);
        assert_eq!(row["outcome"], "rejected");
        assert_eq!(error_codes(row), ["VALIDATION"]);
        assert_eq!(row["errors"][0]["fieldPath"], "shared.catalogRef");
    }
    assert!(list_printers(&fixture).is_empty());
}

// --- 3. In-batch duplicate host ---------------------------------------------

#[test]
fn a_later_row_duplicating_an_earlier_rows_host_is_created_without_a_connection() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})),
            None,
            true,
            json!([
                {"rowId": "r1", "name": "A", "connection": connection("ok.local", 7125, none())},
                {"rowId": "r2", "name": "B", "connection": connection(" OK.local. ", 7125, none())},
            ]),
        ),
    )
    .unwrap();

    assert_eq!(row(&result, "r1")["outcome"], "created");
    let r2 = row(&result, "r2");
    assert_eq!(r2["outcome"], "createdSetupIncomplete");
    assert_eq!(error_codes(r2), ["DUPLICATE_HOST"]);
    assert_eq!(r2["errors"][0]["conflictingRowId"], "r1");
    assert_eq!(r2["printer"]["connection"], Value::Null);
}

// --- 4. DB duplicate host ---------------------------------------------------

#[test]
fn a_row_duplicating_an_active_printers_host_is_created_without_a_connection() {
    let fixture = fixture();
    let mut existing = a_stored_printer("printer-existing");
    existing.connection = Some(ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: "ok.local".to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: None,
    });
    PrinterRepository::new(Arc::clone(&fixture.storage))
        .create(existing)
        .unwrap();

    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})), None,
            true,
            json!([{"rowId": "r1", "name": "A", "connection": connection("ok.local", 7125, none())}]),
        ),
    )
    .unwrap();

    let r1 = row(&result, "r1");
    assert_eq!(r1["outcome"], "createdSetupIncomplete");
    assert_eq!(error_codes(r1), ["DUPLICATE_HOST"]);
    assert_eq!(r1["errors"][0]["conflictingPrinterId"], "printer-existing");
}

// --- Connection validation --------------------------------------------------

#[test]
fn connection_validation_errors_keep_the_row_but_drop_its_connection() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})), None,
            true,
            json!([
                {"rowId": "r1", "name": "A", "connection": {"kind": "elegoolink", "host": "ok.local", "port": 80, "credential": none()}},
                {"rowId": "r2", "name": "B", "connection": connection("  ", 7125, none())},
                {"rowId": "r3", "name": "C", "connection": connection("ok.local", 7125, shared())},
            ]),
        ),
    )
    .unwrap();

    let r1 = row(&result, "r1");
    assert_eq!(r1["outcome"], "createdSetupIncomplete");
    assert_eq!(error_codes(r1), ["UNSUPPORTED_ADAPTER"]);
    let r2 = row(&result, "r2");
    assert_eq!(error_codes(r2), ["VALIDATION"]);
    assert_eq!(r2["errors"][0]["fieldPath"], "rows[1].connection.host");
    let r3 = row(&result, "r3");
    assert_eq!(r3["outcome"], "createdSetupIncomplete");
    assert_eq!(error_codes(r3), ["VALIDATION"]);
    assert_eq!(
        r3["errors"][0]["fieldPath"],
        "rows[2].connection.credential"
    );
    assert_eq!(list_printers(&fixture).len(), 3);
}

#[test]
fn duplicate_names_are_a_warning_not_an_error() {
    let fixture = fixture();
    PrinterRepository::new(Arc::clone(&fixture.storage))
        .create(a_stored_printer("printer-existing"))
        .unwrap();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})),
            None,
            false,
            json!([
                {"rowId": "r1", "name": "test printer"},
                {"rowId": "r2", "name": "Fresh"},
                {"rowId": "r3", "name": "FRESH"},
            ]),
        ),
    )
    .unwrap();

    assert_eq!(warning_codes(row(&result, "r1")), ["DUPLICATE_NAME"]);
    assert_eq!(warning_codes(row(&result, "r2")), Vec::<String>::new());
    assert_eq!(warning_codes(row(&result, "r3")), ["DUPLICATE_NAME"]);
    for row_id in ["r1", "r2", "r3"] {
        assert_eq!(row(&result, row_id)["outcome"], "createdSetupIncomplete");
    }
}

#[test]
fn without_probe_connected_rows_are_committed_unprobed() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})), None,
            false,
            json!([{"rowId": "r1", "name": "A", "connection": connection("auth.local", 7125, none())}]),
        ),
    )
    .unwrap();
    let r1 = row(&result, "r1");
    assert_eq!(r1["outcome"], "created");
    assert!(r1.get("probe").is_none());
    assert_eq!(r1["printer"]["connection"]["host"], "auth.local");
}

// --- 5. Bounded concurrency -------------------------------------------------

#[test]
fn at_most_four_probes_are_in_flight() {
    let fixture = fixture();
    let rows: Vec<Value> = (0..10)
        .map(|i| json!({"rowId": format!("r{i}"), "name": format!("Gate {i}"), "connection": connection("gate.local", 8000 + i, none())}))
        .collect();
    let batch = spawn_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})),
            None,
            true,
            json!(rows),
        ),
    );

    wait_for_probe_starts(&fixture.harness, 4);
    // Released only once four are provably waiting: from here on, a new
    // probe can start only when an earlier one has left the gate.
    fixture.harness.gate.add_permits(10);
    let result = batch.join().unwrap().unwrap();

    assert_eq!(fixture.harness.max_in_flight.load(Ordering::SeqCst), 4);
    for i in 0..10 {
        assert_eq!(row(&result, &format!("r{i}"))["outcome"], "created");
    }
}

// --- 6. Cancellation --------------------------------------------------------

#[test]
fn cancelling_keeps_committed_rows_and_cancels_the_rest() {
    let fixture = fixture();
    let id = batch_id();
    let rows: Vec<Value> = (0..6)
        .map(|i| json!({"rowId": format!("r{i}"), "name": format!("Gate {i}"), "connection": connection("gate.local", 9000 + i, none())}))
        .collect();
    let batch = spawn_batch(
        &fixture,
        batch_body(&id, shared_block(json!({})), None, true, json!(rows)),
    );

    wait_for_probe_starts(&fixture.harness, 4);
    fixture.harness.gate.add_permits(1);
    let committed_port = loop {
        if let Event::Subscribed {
            host,
            port,
            committed,
        } = next_event(&fixture.harness)
        {
            assert_eq!(host, "gate.local");
            assert!(committed, "supervision starts only after commit");
            break port;
        }
    };

    // A second request reusing a running batchId is refused outright.
    let conflict = run_batch(
        &fixture,
        batch_body(
            &id,
            shared_block(json!({})),
            None,
            false,
            json!([{"rowId": "x", "name": "Should Not Exist"}]),
        ),
    )
    .unwrap_err();
    assert_eq!(conflict["code"], "CONFLICT");

    let cancelled = invoke(
        &fixture.webview,
        "cancel_printer_batch",
        json!({"contractVersion": 1, "batchId": id}),
    )
    .unwrap();
    assert_eq!(cancelled["data"], json!({}));
    fixture.harness.gate.add_permits(10);
    let result = batch.join().unwrap().unwrap();

    let committed_row = format!("r{}", committed_port - 9000);
    for i in 0..6 {
        let row_id = format!("r{i}");
        let row = row(&result, &row_id);
        if row_id == committed_row {
            assert_eq!(row["outcome"], "created");
        } else {
            assert_eq!(row["outcome"], "cancelled", "{row_id}: {row}");
            assert_eq!(error_codes(row), ["CANCELLED"]);
            assert!(row.get("printer").is_none());
        }
    }
    let printers = list_printers(&fixture);
    assert_eq!(printers.len(), 1);
    assert_eq!(printers[0]["connection"]["port"], json!(committed_port));

    // The finished batch's id is free again, and unknown ids are a no-op.
    for unknown in [id.as_str(), "never-started"] {
        let response = invoke(
            &fixture.webview,
            "cancel_printer_batch",
            json!({"contractVersion": 1, "batchId": unknown}),
        )
        .unwrap();
        assert_eq!(response["data"], json!({}));
    }
}

// --- 7. Redaction -----------------------------------------------------------

fn assert_no_secret(label: &str, bytes: &[u8]) {
    for secret in [SHARED_SECRET, ROW_SECRET] {
        assert!(
            !bytes
                .windows(secret.len())
                .any(|window| window == secret.as_bytes()),
            "{label} contains a fixture secret"
        );
    }
}

fn file_bytes(path: &Path) -> Vec<u8> {
    std::fs::read(path).unwrap_or_default()
}

/// Everything except the process's own output streams, which
/// [`batch_output_streams_never_contain_the_secrets`] checks by re-running
/// this test in a child process.
#[test]
fn batch_secrets_never_leave_the_credential_store() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})),
            Some(SHARED_SECRET),
            true,
            json!([
                {"rowId": "r1", "name": "A", "connection": connection("ok.local", 7125, shared())},
                {"rowId": "r2", "name": "B", "connection": connection("ok.local", 7126, row_secret(ROW_SECRET))},
                {"rowId": "r3", "name": "C", "connection": connection("ok.local", 7127, shared())},
                {"rowId": "r4", "name": "D", "connection": connection("auth.local", 7125, row_secret(ROW_SECRET))},
                {"rowId": "r5", "name": "E", "connection": connection("ok.local", 7125, row_secret(ROW_SECRET))},
            ]),
        ),
    )
    .unwrap();
    assert_no_secret("the batch result", result.to_string().as_bytes());
    for row in result["data"]["rows"].as_array().unwrap() {
        assert_no_secret("a row warning", row["warnings"].to_string().as_bytes());
        assert_no_secret("a row error", row["errors"].to_string().as_bytes());
    }

    let error = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})),
            Some(SHARED_SECRET),
            true,
            json!([
                {"rowId": "r1", "name": "A", "connection": connection("ok.local", 7130, row_secret(ROW_SECRET))},
                {"rowId": "r1", "name": "B", "connection": connection("ok.local", 7131, shared())},
            ]),
        ),
    )
    .unwrap_err();
    assert_eq!(error["code"], "VALIDATION");
    assert_no_secret("the correlation error", error.to_string().as_bytes());

    let mut wal = fixture.database.clone().into_os_string();
    wal.push("-wal");
    assert_no_secret("the database", &file_bytes(&fixture.database));
    assert_no_secret("the WAL", &file_bytes(Path::new(&wal)));

    // One stored secret per created connected Printer (r1, r2, r3; r4 failed
    // its probe and r5 duplicated r1's host), each under its own ref.
    let store: serde_json::Map<String, Value> = serde_json::from_slice(&file_bytes(
        &farm3d_lib::connections::credentials::credentials_file_path(
            fixture.credentials_dir.path(),
        ),
    ))
    .unwrap();
    assert_eq!(store.len(), 3, "{:?}", store.keys().collect::<Vec<_>>());
    let refs: Vec<String> = ["r1", "r2", "r3"]
        .iter()
        .map(|row_id| {
            let row = row(&result, row_id);
            assert_eq!(row["outcome"], "created");
            assert_eq!(row["credentialStored"], true);
            row["printer"]["connection"]["credentialRef"]
                .as_str()
                .unwrap()
                .to_string()
        })
        .collect();
    assert_eq!(
        refs.iter().collect::<std::collections::BTreeSet<_>>().len(),
        3
    );
    assert_eq!(store[&refs[0]], json!(SHARED_SECRET));
    assert_eq!(store[&refs[1]], json!(ROW_SECRET));
    assert_eq!(store[&refs[2]], json!(SHARED_SECRET));
    for row_id in ["r4", "r5"] {
        assert_eq!(row(&result, row_id)["credentialStored"], false);
    }
}

#[test]
fn batch_output_streams_never_contain_the_secrets() {
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args([
            "--exact",
            "batch_secrets_never_leave_the_credential_store",
            "--nocapture",
            "--test-threads=1",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "the redaction scenario failed in the child process"
    );
    assert!(
        String::from_utf8_lossy(&output.stdout).contains("1 passed"),
        "the child process must actually run the redaction scenario"
    );
    assert_no_secret("the child's stdout", &output.stdout);
    assert_no_secret("the child's stderr", &output.stderr);
}

// --- 8. Shared bed type is copied, not shared -------------------------------

#[test]
fn each_created_printer_gets_its_own_default_bed_type_override() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({"defaultBedType": "6"})),
            None,
            false,
            json!([
                {"rowId": "r1", "name": "A"},
                {"rowId": "r2", "name": "B"},
                {"rowId": "r3", "name": "C"},
            ]),
        ),
    )
    .unwrap();
    for row_id in ["r1", "r2", "r3"] {
        assert_eq!(
            row(&result, row_id)["printer"]["overrides"]["defaultBedType"],
            "6"
        );
    }

    let first = &row(&result, "r1")["printer"];
    invoke(
        &fixture.webview,
        "set_printer_override",
        json!({
            "contractVersion": 1,
            "id": first["id"],
            "expectedRevision": first["revision"],
            "field": "defaultBedType",
            "value": null,
        }),
    )
    .unwrap();

    for printer in list_printers(&fixture) {
        let expected = if printer["id"] == first["id"] {
            Value::Null
        } else {
            json!("6")
        };
        assert_eq!(printer["overrides"]["defaultBedType"], expected);
    }
}

// --- 9. Supervision ---------------------------------------------------------

#[test]
fn created_rows_are_supervised_after_commit() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})),
            None,
            true,
            json!([
                {"rowId": "r1", "name": "A", "connection": connection("ok.local", 7125, none())},
                {"rowId": "r2", "name": "B"},
            ]),
        ),
    )
    .unwrap();

    match next_event(&fixture.harness) {
        Event::Subscribed {
            host, committed, ..
        } => {
            assert_eq!(host, "ok.local");
            assert!(committed, "supervision starts only after commit");
        }
        other => panic!("unexpected event {other:?}"),
    }
    let incomplete_id = row(&result, "r2")["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    assert_eq!(
        fixture.manager.statuses()[&incomplete_id].operational_state,
        OperationalState::SetupIncomplete
    );
}

// --- 10. Capability mismatch ------------------------------------------------

#[test]
fn a_capability_mismatch_is_a_warning_and_the_row_is_still_created() {
    let fixture = fixture();
    let result = run_batch(
        &fixture,
        batch_body(
            &batch_id(),
            shared_block(json!({})), None,
            true,
            json!([{"rowId": "r1", "name": "A", "connection": connection("mismatch.local", 7125, none())}]),
        ),
    )
    .unwrap();
    let r1 = row(&result, "r1");
    assert_eq!(r1["outcome"], "created");
    assert_eq!(warning_codes(r1), ["CAPABILITY_MISMATCH"]);
    assert_eq!(
        r1["warnings"][0]["message"],
        "Bed width: catalog says 256 mm, the printer reports 266 mm"
    );
}
