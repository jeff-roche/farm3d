//! P2 task 4: `create_printer` with options, `probe_connection`, and safe
//! Connection replacement. See
//! `.superpowers/sdd/2026-09-22-p2-printer-lifecycle-batch-setup/task-4-brief.md`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use farm3d_lib::bootstrap::BootstrapState;
use farm3d_lib::catalog::{BedShape, Catalog, CatalogModel, CatalogVariant};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::{ConnectionManager, STATUS_EVENT};
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionError, ConnectionObservation, PrinterConnection, ProbeResult,
    ReportedCapabilities, MOONRAKER_KIND,
};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::document_io::{DocumentIo, DocumentKind};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{CatalogRef, StoredPrinter};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Listener, Manager, WebviewWindowBuilder};

struct UnusedDocuments;

impl DocumentIo for UnusedDocuments {
    fn open_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn read(&self, _path: &Path) -> Result<Vec<u8>, CommandError> {
        Err(CommandError::internal())
    }
    fn atomic_write(&self, _path: &Path, _bytes: &[u8]) -> Result<(), CommandError> {
        Err(CommandError::internal())
    }
}

fn storage() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>) {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    (temp, lease, storage)
}

fn pending_cleanup_count(storage: &Storage) -> i64 {
    storage
        .read(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM pending_credential_cleanup",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .unwrap()
}

fn a_catalog() -> Catalog {
    Catalog {
        generated_at: "2026-09-22T00:00:00Z".to_string(),
        source_tag: "v-test".to_string(),
        notice: "test".to_string(),
        models: vec![CatalogModel {
            model_id: "TestVendor-TP".to_string(),
            vendor: "TestVendor".to_string(),
            model: "Test Printer".to_string(),
            variants: vec![CatalogVariant {
                variant: "Test Printer 0.4 nozzle".to_string(),
                printer_variant: "0.4".to_string(),
                bed_shape: BedShape::Rectangular {
                    width_mm: 256.0,
                    depth_mm: 256.0,
                    origin_x_mm: 0.0,
                    origin_y_mm: 0.0,
                },
                printable_height_mm: 256.0,
                bed_exclude_areas: vec![],
                default_bed_type: "4".to_string(),
                nozzle_diameter_mm: vec![0.4],
                nozzle_type: "hardened_steel".to_string(),
                gcode_flavor: "klipper".to_string(),
                has_auxiliary_fan: true,
                supports_air_filtration: true,
                supports_multi_filament: false,
                suggested_host_type: None,
            }],
        }],
    }
}

fn a_ref() -> CatalogRef {
    CatalogRef {
        vendor: "TestVendor".to_string(),
        model: "Test Printer".to_string(),
        variant: "Test Printer 0.4 nozzle".to_string(),
        model_id: "TestVendor-TP".to_string(),
        printer_variant: "0.4".to_string(),
    }
}

fn a_ref_json() -> Value {
    json!({
        "vendor": "TestVendor",
        "model": "Test Printer",
        "variant": "Test Printer 0.4 nozzle",
        "modelId": "TestVendor-TP",
        "printerVariant": "0.4",
    })
}

fn a_stored_printer(id: &str) -> StoredPrinter {
    StoredPrinter {
        id: id.to_string(),
        name: "Test Printer".to_string(),
        catalog_ref: a_ref(),
        ..Default::default()
    }
}

/// A `PrinterConnection` whose `probe()` outcome is fixed by host name
/// (`ok.local` / `auth.local` / `slow.local`, everything else unreachable)
/// and whose `subscribe()` pends forever rather than looping — real
/// supervision start calls `subscribe`, not `probe`, so this keeps the
/// supervisor's reconnect loop from spinning against the fake.
struct FakeConnection {
    host: String,
}

#[async_trait::async_trait]
impl PrinterConnection for FakeConnection {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        match self.host.as_str() {
            "ok.local" => Ok(ProbeResult {
                kind: MOONRAKER_KIND.to_string(),
                host_software: "Moonraker".to_string(),
                firmware: "Klipper".to_string(),
                reported_name: "Fixture Printer".to_string(),
                state: "ready".to_string(),
                state_message: String::new(),
                reported: ReportedCapabilities::default(),
            }),
            "auth.local" => Err(ConnectionError::Auth("bad credential".to_string())),
            "slow.local" => Err(ConnectionError::Timeout),
            other => Err(ConnectionError::Unreachable(other.to_string())),
        }
    }

    async fn subscribe(
        &self,
        _tx: tokio::sync::mpsc::Sender<ConnectionObservation>,
    ) -> Result<(), ConnectionError> {
        std::future::pending::<()>().await;
        Ok(())
    }
}

/// Records every host the manager's connection factory was asked to build a
/// Connection for, together with whether a Printer whose Connection names
/// that host was already committed to `storage` at call time — the probe
/// for test 4 ("the supervisor started after the row exists").
fn recording_factory(
    storage: Arc<Storage>,
) -> (
    impl Fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Option<Box<dyn PrinterConnection>>
        + Send
        + Sync
        + 'static,
    Receiver<(String, bool)>,
) {
    let (tx, rx) = mpsc::channel::<(String, bool)>();
    let tx = Mutex::new(tx);
    let factory = move |config: &ConnectionConfig, _key: Option<zeroize::Zeroizing<String>>| {
        let committed = PrinterRepository::new(Arc::clone(&storage))
            .list()
            .unwrap_or_default()
            .iter()
            .any(|printer| {
                printer.connection.as_ref().map(|c| c.host.as_str()) == Some(config.host.as_str())
            });
        let _ = tx.lock().unwrap().send((config.host.clone(), committed));
        Some(Box::new(FakeConnection {
            host: config.host.clone(),
        }) as Box<dyn PrinterConnection>)
    };
    (factory, rx)
}

fn invoke(
    webview: &tauri::WebviewWindow<tauri::test::MockRuntime>,
    command: &str,
    body: Value,
) -> Result<Value, Value> {
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
    .map(|response| response.deserialize().unwrap())
}

/// Like `p2_lifecycle.rs`'s `runtime()`, plus a credential store pointed at
/// a directory this test controls (`RuntimeServices::for_test` otherwise
/// mints its own, unreachable, temp directory) so "the credential store
/// directory is unchanged" is actually checkable.
fn runtime(
    storage: Arc<Storage>,
    catalog: Arc<Catalog>,
    credentials_dir: PathBuf,
    factory: impl Fn(
            &ConnectionConfig,
            Option<zeroize::Zeroizing<String>>,
        ) -> Option<Box<dyn PrinterConnection>>
        + Send
        + Sync
        + 'static,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
    Arc<ConnectionManager<tauri::test::MockRuntime>>,
    Arc<RuntimeServices<tauri::test::MockRuntime>>,
) {
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::printers::commands::list_printers,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::update_printer,
            farm3d_lib::connections::commands::set_printer_connection,
            farm3d_lib::printers::create::probe_connection,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let manager = Arc::new(ConnectionManager::with_clock_and_factory(
        app.handle().clone(),
        Arc::new(StatusRepository::new(Arc::clone(&storage))),
        chrono::Utc::now,
        factory,
    ));
    let documents: Arc<dyn DocumentIo> = Arc::new(UnusedDocuments);
    let mut services = RuntimeServices::for_test(
        Arc::clone(&storage),
        catalog,
        Arc::clone(&manager),
        documents,
    );
    services.credentials = Arc::new(CredentialStore::file_backed(credentials_dir));
    let services = Arc::new(services);
    app.manage(BootstrapState::ready_with(Arc::clone(&services)));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview, manager, services)
}

/// A snapshot of a credential store directory's presence/contents, cheap
/// enough to compare before/after a call that must not touch it.
fn credentials_dir_snapshot(dir: &Path) -> Option<Vec<u8>> {
    let path = farm3d_lib::connections::credentials::credentials_file_path(dir);
    std::fs::read(path).ok()
}

// --- 1. `probe_connection` has no side effects -----------------------------

#[test]
fn probe_connection_has_no_side_effects() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (app, webview, manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );
    // A pre-existing Printer makes the "unchanged" row-dump comparison
    // non-trivial.
    PrinterRepository::new(Arc::clone(&storage))
        .create(a_stored_printer("printer-a"))
        .unwrap();

    let before_printers = PrinterRepository::new(Arc::clone(&storage)).list().unwrap();
    let before_pending = pending_cleanup_count(&storage);
    let before_credentials = credentials_dir_snapshot(credentials_dir.path());
    let before_statuses = manager.statuses();

    let (event_tx, event_rx) = mpsc::channel::<String>();
    app.listen(STATUS_EVENT, move |event| {
        let _ = event_tx.send(event.payload().to_string());
    });

    let response = invoke(
        &webview,
        "probe_connection",
        json!({
            "contractVersion": 1,
            "submission": {
                "kind": "moonraker",
                "host": "ok.local",
                "port": 7125,
                "useTls": false,
                "credential": "s3cret-FIXTURE",
            }
        }),
    )
    .unwrap();
    assert_eq!(response["data"]["state"], json!("ready"));

    assert_eq!(
        PrinterRepository::new(Arc::clone(&storage)).list().unwrap(),
        before_printers
    );
    assert_eq!(pending_cleanup_count(&storage), before_pending);
    assert_eq!(
        credentials_dir_snapshot(credentials_dir.path()),
        before_credentials
    );
    assert_eq!(manager.statuses(), before_statuses);
    assert_eq!(
        event_rx.recv_timeout(Duration::from_millis(200)),
        Err(mpsc::RecvTimeoutError::Timeout),
        "probe_connection must never publish a status event"
    );
}

// --- 2. `probe_connection` to auth.local ------------------------------------

#[test]
fn probe_connection_to_auth_local_is_authentication_failed_and_never_leaks_the_secret() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );

    let error = invoke(
        &webview,
        "probe_connection",
        json!({
            "contractVersion": 1,
            "submission": {
                "kind": "moonraker",
                "host": "auth.local",
                "port": 7125,
                "useTls": false,
                "credential": "s3cret-FIXTURE",
            }
        }),
    )
    .unwrap_err();

    assert_eq!(error["code"], "AUTHENTICATION_FAILED");
    assert!(!error.to_string().contains("s3cret-FIXTURE"));
}

// --- 3. `create_printer` Profile-only ---------------------------------------

#[test]
fn create_printer_profile_only_trims_location_and_reconciles_to_setup_incomplete() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );

    let response = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "Bay Printer",
            "catalogRef": a_ref_json(),
            "location": " Bay A ",
        }),
    )
    .unwrap();

    let printer = &response["data"]["printer"];
    assert_eq!(printer["location"], json!("Bay A"));
    assert_eq!(printer["startSafety"], json!("confirmBedClear"));
    assert_eq!(printer["setupGaps"], json!(["missingConnection"]));
    let id = printer["id"].as_str().unwrap();
    assert_eq!(
        manager.statuses()[id].operational_state,
        OperationalState::SetupIncomplete
    );
}

// --- 4. `create_printer` with a connection and credential -------------------

#[test]
fn create_printer_with_connection_and_credential_starts_supervision_after_commit() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );

    let response = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "Connected Printer",
            "catalogRef": a_ref_json(),
            "connection": {
                "kind": "moonraker",
                "host": "ok.local",
                "port": 7125,
                "useTls": false,
                "credential": "s3cret-FIXTURE",
            },
        }),
    )
    .unwrap();

    let printer = &response["data"]["printer"];
    let credential_ref = printer["connection"]["credentialRef"].as_str().unwrap();
    let uuid_part = credential_ref.strip_prefix("farm3d/credential/").expect(
        "credentialRef must be farm3d/credential/<uuid>",
    );
    assert!(uuid::Uuid::parse_str(uuid_part).is_ok());
    assert_eq!(
        services.credentials.get(credential_ref).unwrap().as_deref(),
        Some("s3cret-FIXTURE")
    );

    let (host, row_existed_at_call_time) = calls
        .recv_timeout(Duration::from_secs(2))
        .expect("the connection factory should have been called for supervision start");
    assert_eq!(host, "ok.local");
    assert!(
        row_existed_at_call_time,
        "the Printer row must already be committed before supervision starts"
    );

    assert_eq!(pending_cleanup_count(&storage), 0);
}

// --- 5. `create_printer` with `defaultBedType` ------------------------------

#[test]
fn create_printer_default_bed_type_is_an_override_only_when_it_differs_from_the_catalog() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );

    let differing = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "P1",
            "catalogRef": a_ref_json(),
            "defaultBedType": "6",
        }),
    )
    .unwrap();
    assert_eq!(
        differing["data"]["printer"]["overrides"]["defaultBedType"],
        json!("6")
    );

    let matching = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "P2",
            "catalogRef": a_ref_json(),
            "defaultBedType": "4",
        }),
    )
    .unwrap();
    assert_eq!(matching["data"]["printer"]["overrides"], json!({}));
}

// --- 6. `create_printer` with an empty/whitespace name ----------------------

#[test]
fn create_printer_with_a_blank_name_is_a_validation_error_at_name() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );

    let error = invoke(
        &webview,
        "create_printer",
        json!({"contractVersion": 1, "name": "   ", "catalogRef": a_ref_json()}),
    )
    .unwrap_err();

    assert_eq!(error["code"], "VALIDATION");
    assert_eq!(error["details"]["fieldPath"], json!("name"));
}

// --- 7. `create_printer` with a duplicate active host -----------------------

#[test]
fn create_printer_with_a_duplicate_active_host_persists_nothing() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );
    let existing_connection = ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: "dup.local".to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: None,
    };
    let mut existing = a_stored_printer("printer-existing");
    existing.connection = Some(existing_connection);
    PrinterRepository::new(Arc::clone(&storage))
        .create(existing)
        .unwrap();
    let before = PrinterRepository::new(Arc::clone(&storage)).list().unwrap();

    let error = invoke(
        &webview,
        "create_printer",
        json!({
            "contractVersion": 1,
            "name": "Duplicate",
            "catalogRef": a_ref_json(),
            "connection": {
                "kind": "moonraker",
                "host": "dup.local",
                "port": 7125,
                "useTls": false,
                "credential": "s3cret-FIXTURE",
            },
        }),
    )
    .unwrap_err();

    assert_eq!(error["code"], "DUPLICATE_HOST");
    assert_eq!(
        PrinterRepository::new(Arc::clone(&storage)).list().unwrap(),
        before
    );
    assert_eq!(pending_cleanup_count(&storage), 0);
    assert_eq!(
        services.credentials.get("s3cret-FIXTURE").unwrap(),
        None,
        "no credential must have been stored for the rejected create"
    );
}

// --- 8. Replacing a working connection with auth.local ----------------------

#[test]
fn set_printer_connection_probes_before_replacing_a_working_connection() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_stored_printer("printer-a"))
        .unwrap();

    let first = invoke(
        &webview,
        "set_printer_connection",
        json!({
            "contractVersion": 1,
            "id": printer.id,
            "expectedRevision": printer.revision,
            "submission": {"kind": "moonraker", "host": "ok.local", "port": 7125, "useTls": false},
        }),
    )
    .unwrap();
    let revision = first["data"]["printer"]["revision"].as_i64().unwrap();

    let error = invoke(
        &webview,
        "set_printer_connection",
        json!({
            "contractVersion": 1,
            "id": printer.id,
            "expectedRevision": revision,
            "submission": {"kind": "moonraker", "host": "auth.local", "port": 7125, "useTls": false},
        }),
    )
    .unwrap_err();
    assert_eq!(error["code"], "AUTHENTICATION_FAILED");

    let stored = PrinterRepository::new(Arc::clone(&storage))
        .get(&printer.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.connection.as_ref().unwrap().host, "ok.local");
    assert_eq!(stored.revision, revision);

    let retried = invoke(
        &webview,
        "set_printer_connection",
        json!({
            "contractVersion": 1,
            "id": printer.id,
            "expectedRevision": revision,
            "submission": {"kind": "moonraker", "host": "auth.local", "port": 7125, "useTls": false},
            "acceptUnverified": true,
        }),
    )
    .unwrap();
    assert_eq!(
        retried["data"]["printer"]["connection"]["host"],
        json!("auth.local")
    );
}

// --- 9. First-time `set_printer_connection` to slow.local -------------------

#[test]
fn first_time_set_printer_connection_never_probes() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_stored_printer("printer-a"))
        .unwrap();

    let response = invoke(
        &webview,
        "set_printer_connection",
        json!({
            "contractVersion": 1,
            "id": printer.id,
            "expectedRevision": printer.revision,
            "submission": {"kind": "moonraker", "host": "slow.local", "port": 7125, "useTls": false},
        }),
    )
    .unwrap();
    assert_eq!(
        response["data"]["printer"]["connection"]["host"],
        json!("slow.local")
    );

    // Exactly one factory call is expected: supervision start. A second,
    // separate call would mean a probe was (wrongly) performed before the
    // first-ever set.
    let mut factory_calls = 0;
    while calls.recv_timeout(Duration::from_millis(300)).is_ok() {
        factory_calls += 1;
    }
    assert_eq!(
        factory_calls, 1,
        "a first-time set must not probe — only supervision start should call the factory"
    );
}

// --- 10. `update_printer` location/startSafety ------------------------------

#[test]
fn update_printer_clears_location_with_null_and_persists_start_safety() {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, _calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );
    let mut printer = a_stored_printer("printer-a");
    printer.location = Some("Bay A".to_string());
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(printer)
        .unwrap();

    let cleared = invoke(
        &webview,
        "update_printer",
        json!({
            "contractVersion": 1,
            "id": printer.id,
            "expectedRevision": printer.revision,
            "patch": {"location": null},
        }),
    )
    .unwrap();
    assert_eq!(cleared["data"]["printer"]["location"], json!(null));
    let revision = cleared["data"]["printer"]["revision"].as_i64().unwrap();

    let unattended = invoke(
        &webview,
        "update_printer",
        json!({
            "contractVersion": 1,
            "id": printer.id,
            "expectedRevision": revision,
            "patch": {"startSafety": "unattended"},
        }),
    )
    .unwrap();
    assert_eq!(
        unattended["data"]["printer"]["startSafety"],
        json!("unattended")
    );
}
