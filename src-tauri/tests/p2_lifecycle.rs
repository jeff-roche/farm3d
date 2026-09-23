//! P2 task 3: lifecycle eligibility, archive, unarchive, and guarded delete.
//! See `.superpowers/sdd/2026-09-22-p2-printer-lifecycle-batch-setup/task-3-brief.md`.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use farm3d_lib::bootstrap::BootstrapState;
use farm3d_lib::catalog::{BedShape, Catalog, CatalogModel, CatalogVariant};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::ConnectionManager;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection, MOONRAKER_KIND};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::document_io::{DocumentIo, DocumentKind};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{CatalogRef, StoredPrinter};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindowBuilder};

const CREDENTIAL_REF: &str = "farm3d/credential/6ba7b810-9dad-4f83-a131-2a6f44cbbf89";

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

fn moonraker_config(host: &str, credential_ref: Option<&str>) -> ConnectionConfig {
    ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: host.to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: credential_ref.map(str::to_string),
    }
}

fn a_printer(id: &str, connection: Option<ConnectionConfig>) -> StoredPrinter {
    StoredPrinter {
        id: id.to_string(),
        name: "Test Printer".to_string(),
        catalog_ref: a_ref(),
        connection,
        ..Default::default()
    }
}

/// A connection factory that records every host it was asked to build a
/// Connection for, and always declines (returns `None`) — the supervisor
/// then reports a benign "unsupported by this build" error and stops, which
/// is all these tests need: whether the factory was *called*, not what it
/// returns.
fn recording_factory() -> (
    impl Fn(
            &ConnectionConfig,
            Option<zeroize::Zeroizing<String>>,
        ) -> Option<Box<dyn PrinterConnection>>
        + Send
        + Sync
        + 'static,
    Receiver<String>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    let factory = move |config: &ConnectionConfig, _key: Option<zeroize::Zeroizing<String>>| {
        let _ = tx.lock().unwrap().send(config.host.clone());
        None
    };
    (factory, rx)
}

fn expect_a_call(calls: &Receiver<String>) -> String {
    calls
        .recv_timeout(Duration::from_secs(2))
        .expect("the connection factory should have been called")
}

fn expect_no_call(calls: &Receiver<String>) {
    match calls.recv_timeout(Duration::from_millis(200)) {
        Err(RecvTimeoutError::Timeout) => {}
        other => panic!("expected no further factory call, got {other:?}"),
    }
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

fn runtime(
    storage: Arc<Storage>,
    catalog: Arc<Catalog>,
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
            farm3d_lib::printers::commands::printer_lifecycle_eligibility,
            farm3d_lib::printers::commands::archive_printer,
            farm3d_lib::printers::commands::unarchive_printer,
            farm3d_lib::printers::commands::delete_printer,
            farm3d_lib::connections::commands::clear_printer_connection,
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
    let services = Arc::new(RuntimeServices::for_test(
        Arc::clone(&storage),
        catalog,
        Arc::clone(&manager),
        documents,
    ));
    app.manage(BootstrapState::ready_with(Arc::clone(&services)));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview, manager, services)
}

#[test]
fn eligibility_of_an_active_printer_allows_only_archive() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _manager, _services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), |_c, _k| None);
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_printer("printer-a", None))
        .unwrap();

    let response = invoke(
        &webview,
        "printer_lifecycle_eligibility",
        json!({"contractVersion": 1, "id": printer.id}),
    )
    .unwrap();

    assert_eq!(response["data"]["canArchive"], json!(true));
    assert_eq!(response["data"]["canUnarchive"], json!(false));
    assert_eq!(response["data"]["canDelete"], json!(false));
    let blockers = response["data"]["blockers"].as_array().unwrap();
    let actions_and_codes: Vec<(&str, &str)> = blockers
        .iter()
        .map(|blocker| {
            (
                blocker["action"].as_str().unwrap(),
                blocker["code"].as_str().unwrap(),
            )
        })
        .collect();
    assert_eq!(
        actions_and_codes,
        vec![("delete", "NOT_ARCHIVED"), ("unarchive", "NOT_ARCHIVED")]
    );
}

#[test]
fn delete_of_an_active_printer_is_lifecycle_blocked_and_leaves_the_row_untouched() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _manager, _services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), |_c, _k| None);
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_printer("printer-a", None))
        .unwrap();

    let error = invoke(
        &webview,
        "delete_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": printer.revision}),
    )
    .unwrap_err();

    assert_eq!(error["code"], "LIFECYCLE_BLOCKED");
    assert_eq!(error["details"]["blockers"][0]["code"], "NOT_ARCHIVED");
    assert!(PrinterRepository::new(Arc::clone(&storage))
        .get(&printer.id)
        .unwrap()
        .is_some());
}

#[test]
fn archiving_a_connected_printer_stops_supervision_and_preserves_the_connection_and_credential() {
    let (_temp, _lease, storage) = storage();
    let (factory, calls) = recording_factory();
    let (_app, webview, manager, services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    services.credentials.set(CREDENTIAL_REF, "s3cret").unwrap();
    let connection = moonraker_config("voron.local", Some(CREDENTIAL_REF));
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_printer("printer-a", Some(connection.clone())))
        .unwrap();
    // Simulate the Printer already being under active supervision (as it
    // would be after `set_printer_connection`/startup) before archiving it.
    tauri::async_runtime::block_on(farm3d_lib::printers::setup::supervise_printer(
        &manager,
        services.credentials.as_ref(),
        &services.catalog,
        &printer,
    ));
    expect_a_call(&calls);

    let response = invoke(
        &webview,
        "archive_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": printer.revision, "operationId": "op-archive", "spoolDispositions": []}),
    )
    .unwrap();

    assert!(response["data"]["printer"]["archivedAt"].is_string());
    assert_eq!(
        response["data"]["printer"]["revision"],
        json!(printer.revision + 1)
    );
    assert!(
        !manager.statuses().contains_key(&printer.id),
        "an archived Printer must not appear in printer_statuses"
    );
    expect_no_call(&calls);
    let stored = PrinterRepository::new(Arc::clone(&storage))
        .get(&printer.id)
        .unwrap()
        .unwrap();
    assert_eq!(stored.connection, Some(connection));
    assert_eq!(
        services.credentials.get(CREDENTIAL_REF).unwrap(),
        Some("s3cret".to_string())
    );
}

#[test]
fn after_archiving_a_restart_never_supervises_the_archived_printer_but_keeps_it_listed() {
    let (_temp, _lease, storage) = storage();
    let (factory, _calls) = recording_factory();
    let (_app, webview, _manager, _services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_printer("printer-a", None))
        .unwrap();
    invoke(
        &webview,
        "archive_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": printer.revision, "operationId": "op-archive", "spoolDispositions": []}),
    )
    .unwrap();

    // Simulate an app restart: a brand-new manager/credential store over the
    // *same* storage, driven through the same restore path `lib.rs` uses at
    // startup.
    let (restart_factory, restart_calls) = recording_factory();
    let restart_app = mock_builder().build(mock_context(noop_assets())).unwrap();
    let restart_manager = Arc::new(ConnectionManager::with_clock_and_factory(
        restart_app.handle().clone(),
        Arc::new(StatusRepository::new(Arc::clone(&storage))),
        chrono::Utc::now,
        restart_factory,
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

    expect_no_call(&restart_calls);
    assert!(!restart_manager
        .status_backfill()
        .statuses
        .iter()
        .any(|row| row.printer_id == printer.id));
    let listed = invoke(&webview, "list_printers", json!({"contractVersion": 1})).unwrap();
    let rows = listed["data"].as_array().unwrap();
    let row = rows
        .iter()
        .find(|row| row["id"] == json!(printer.id))
        .expect("the archived Printer must still be listed");
    assert_eq!(row["name"], json!(printer.name));
    assert_eq!(row["createdAt"], json!(printer.created_at));
}

#[test]
fn unarchiving_restarts_supervision() {
    let (_temp, _lease, storage) = storage();
    let (factory, calls) = recording_factory();
    let (_app, webview, _manager, _services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    let connection = moonraker_config("voron.local", None);
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_printer("printer-a", Some(connection)))
        .unwrap();
    let archived = invoke(
        &webview,
        "archive_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": printer.revision, "operationId": "op-archive", "spoolDispositions": []}),
    )
    .unwrap();
    let archived_revision = archived["data"]["printer"]["revision"].as_i64().unwrap();

    invoke(
        &webview,
        "unarchive_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": archived_revision}),
    )
    .unwrap();

    let host = expect_a_call(&calls);
    assert_eq!(host, "voron.local");
}

#[test]
fn unarchiving_into_a_reclaimed_host_fails_with_duplicate_host() {
    let (_temp, _lease, storage) = storage();
    let (factory, _calls) = recording_factory();
    let (_app, webview, _manager, _services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let connection = moonraker_config("shared.invalid", None);
    let printer_a = repository
        .create(a_printer("printer-a", Some(connection.clone())))
        .unwrap();
    let archived = invoke(
        &webview,
        "archive_printer",
        json!({"contractVersion": 1, "id": printer_a.id, "expectedRevision": printer_a.revision, "operationId": "op-archive", "spoolDispositions": []}),
    )
    .unwrap();
    let archived_revision = archived["data"]["printer"]["revision"].as_i64().unwrap();
    // Archived Printers don't reserve their host, so this create succeeds.
    let printer_b = repository
        .create(a_printer("printer-b", Some(connection)))
        .unwrap();

    let error = invoke(
        &webview,
        "unarchive_printer",
        json!({"contractVersion": 1, "id": printer_a.id, "expectedRevision": archived_revision}),
    )
    .unwrap_err();

    assert_eq!(error["code"], "DUPLICATE_HOST");
    assert_eq!(
        error["details"]["conflictingPrinterId"],
        json!(printer_b.id)
    );
}

#[test]
fn deleting_an_archived_printer_succeeds_and_removes_its_credential() {
    let (_temp, _lease, storage) = storage();
    let (factory, _calls) = recording_factory();
    let (_app, webview, _manager, services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    services.credentials.set(CREDENTIAL_REF, "s3cret").unwrap();
    let connection = moonraker_config("voron.local", Some(CREDENTIAL_REF));
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_printer("printer-a", Some(connection)))
        .unwrap();
    let archived = invoke(
        &webview,
        "archive_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": printer.revision, "operationId": "op-archive", "spoolDispositions": []}),
    )
    .unwrap();
    let archived_revision = archived["data"]["printer"]["revision"].as_i64().unwrap();

    invoke(
        &webview,
        "delete_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": archived_revision}),
    )
    .unwrap();

    assert!(PrinterRepository::new(Arc::clone(&storage))
        .get(&printer.id)
        .unwrap()
        .is_none());
    assert_eq!(services.credentials.get(CREDENTIAL_REF).unwrap(), None);
}

#[test]
fn clearing_the_connection_of_an_archived_printer_publishes_no_status() {
    let (_temp, _lease, storage) = storage();
    let (factory, calls) = recording_factory();
    let (_app, webview, manager, services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(a_printer(
            "printer-a",
            Some(moonraker_config("voron.local", None)),
        ))
        .unwrap();
    tauri::async_runtime::block_on(farm3d_lib::printers::setup::supervise_printer(
        &manager,
        services.credentials.as_ref(),
        &services.catalog,
        &printer,
    ));
    expect_a_call(&calls);
    let archived = invoke(
        &webview,
        "archive_printer",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": printer.revision, "operationId": "op-archive", "spoolDispositions": []}),
    )
    .unwrap();
    let archived_revision = archived["data"]["printer"]["revision"].as_i64().unwrap();

    let cleared = invoke(
        &webview,
        "clear_printer_connection",
        json!({"contractVersion": 1, "id": printer.id, "expectedRevision": archived_revision}),
    )
    .unwrap();

    assert_eq!(cleared["data"]["printer"]["connection"], json!(null));
    assert!(
        !manager.statuses().contains_key(&printer.id),
        "an archived Printer must not get a live status from clearing its Connection"
    );
    assert!(!manager
        .status_backfill()
        .statuses
        .iter()
        .any(|row| row.printer_id == printer.id));
    expect_no_call(&calls);
}

#[test]
fn supervising_the_persisted_state_after_a_concurrent_archive_starts_nothing() {
    let (_temp, _lease, storage) = storage();
    let (factory, calls) = recording_factory();
    let (_app, _webview, manager, services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let stale = repository
        .create(a_printer(
            "printer-a",
            Some(moonraker_config("voron.local", None)),
        ))
        .unwrap();
    // An interleaved archive commits after the caller captured `stale`.
    repository.archive(&stale.id, stale.revision, "op-archive", &[]).unwrap();

    tauri::async_runtime::block_on(farm3d_lib::printers::setup::supervise_persisted(
        &manager,
        &storage,
        services.credentials.as_ref(),
        &services.catalog,
        &stale,
    ));

    expect_no_call(&calls);
    assert!(!manager.statuses().contains_key(&stale.id));
}

#[test]
fn supervising_the_persisted_state_of_a_deleted_printer_removes_its_status() {
    let (_temp, _lease, storage) = storage();
    let (factory, calls) = recording_factory();
    let (_app, _webview, manager, services) =
        runtime(Arc::clone(&storage), Arc::new(a_catalog()), factory);
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let stale = repository.create(a_printer("printer-a", None)).unwrap();
    repository.archive(&stale.id, stale.revision, "op-archive", &[]).unwrap();
    repository.delete(&stale.id, stale.revision + 1).unwrap();

    tauri::async_runtime::block_on(farm3d_lib::printers::setup::supervise_persisted(
        &manager,
        &storage,
        services.credentials.as_ref(),
        &services.catalog,
        &stale,
    ));

    expect_no_call(&calls);
    assert!(!manager.statuses().contains_key(&stale.id));
}
