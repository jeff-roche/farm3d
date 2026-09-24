use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;

use farm3d_lib::bootstrap::BootstrapState;
use farm3d_lib::persistence::{migrate_legacy, MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Listener, Manager, WebviewWindowBuilder};

#[derive(Default)]
struct TracerDocuments {
    settings_import: Mutex<Vec<u8>>,
    printers_import: Mutex<Vec<u8>>,
    settings_export: Mutex<Vec<u8>>,
    printers_export: Mutex<Vec<u8>>,
}

impl farm3d_lib::document_io::DocumentIo for TracerDocuments {
    fn open_json(
        &self,
        kind: farm3d_lib::document_io::DocumentKind,
    ) -> Result<Option<PathBuf>, farm3d_lib::contracts::command::CommandError> {
        Ok(Some(match kind {
            farm3d_lib::document_io::DocumentKind::Settings => PathBuf::from("settings.json"),
            farm3d_lib::document_io::DocumentKind::Printers => PathBuf::from("printers.json"),
        }))
    }

    fn save_json(
        &self,
        kind: farm3d_lib::document_io::DocumentKind,
    ) -> Result<Option<PathBuf>, farm3d_lib::contracts::command::CommandError> {
        self.open_json(kind)
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, farm3d_lib::contracts::command::CommandError> {
        Ok(if path.file_name().unwrap() == "settings.json" {
            self.settings_import.lock().unwrap().clone()
        } else {
            self.printers_import.lock().unwrap().clone()
        })
    }

    fn atomic_write(
        &self,
        path: &Path,
        bytes: &[u8],
    ) -> Result<(), farm3d_lib::contracts::command::CommandError> {
        if path.file_name().unwrap() == "settings.json" {
            *self.settings_export.lock().unwrap() = bytes.to_vec();
        } else {
            *self.printers_export.lock().unwrap() = bytes.to_vec();
        }
        Ok(())
    }
}

fn fixture(name: &str) -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/persistence/v1")
        .join(name)
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

#[test]
fn f0_fixture_migrates_and_survives_repository_restart() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    fs::copy(fixture("settings.json"), metadata.join("settings.json")).unwrap();
    fs::copy(fixture("printers.json"), metadata.join("printers.json")).unwrap();
    fs::copy(
        fixture("credentials.json"),
        metadata.join("credentials.json"),
    )
    .unwrap();
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths.clone(), &lease).unwrap());
    migrate_legacy(&storage).unwrap();
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let before = repository.list().unwrap();
    let target = before
        .iter()
        .find(|printer| printer.id == "prn-f0-connected")
        .unwrap();
    let updated = repository
        .update(&target.id, target.revision, |printer| {
            printer.notes = "survives restart".to_string()
        })
        .unwrap();
    assert_eq!(updated.revision, 2);
    drop((repository, storage));

    let reopened = Arc::new(Storage::open(paths, &lease).unwrap());
    let after = PrinterRepository::new(reopened)
        .get("prn-f0-connected")
        .unwrap()
        .unwrap();
    assert_eq!(
        (after.revision, after.notes.as_str()),
        (2, "survives restart")
    );
    let database = fs::read(metadata.join("farm3d.sqlite3")).unwrap();
    assert!(!String::from_utf8_lossy(&database).contains("F0_FIXTURE_SENTINEL"));
}

#[test]
fn versioned_mock_runtime_command_mutation_survives_restart() {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths.clone(), &lease).unwrap());
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../public/catalog/printer-catalog.json");
    let catalog = Arc::new(farm3d_lib::catalog::load_snapshot(&catalog_path).unwrap());
    let model = &catalog.models[0];
    let variant = &model.variants[0];
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::list_printers
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let manager = Arc::new(farm3d_lib::connections::supervisor::ConnectionManager::new(
        app.handle().clone(),
        Arc::new(
            farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(&storage)),
        ),
    ));
    let documents: Arc<dyn farm3d_lib::document_io::DocumentIo> = Arc::new(
        farm3d_lib::document_io::NativeDocumentIo::new(app.handle().clone()),
    );
    app.manage(BootstrapState::ready_with(Arc::new(
        RuntimeServices::for_test(
            Arc::clone(&storage),
            Arc::clone(&catalog),
            manager,
            documents,
        ),
    )));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    let created = invoke(&webview, "create_printer", json!({"contractVersion":1,
        "name":"F1 tracer", "catalogRef":{"vendor":model.vendor,"model":model.model,"variant":variant.variant,"modelId":model.model_id,"printerVariant":variant.printer_variant}
    })).unwrap();
    assert_eq!(created["contractVersion"], 1);
    let id = created["data"]["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    drop((webview, app, storage));

    let reopened = Arc::new(Storage::open(paths, &lease).unwrap());
    let restarted = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::printers::commands::list_printers
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let manager = Arc::new(farm3d_lib::connections::supervisor::ConnectionManager::new(
        restarted.handle().clone(),
        Arc::new(
            farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(
                &reopened,
            )),
        ),
    ));
    let documents: Arc<dyn farm3d_lib::document_io::DocumentIo> = Arc::new(
        farm3d_lib::document_io::NativeDocumentIo::new(restarted.handle().clone()),
    );
    restarted.manage(BootstrapState::ready_with(Arc::new(
        RuntimeServices::for_test(Arc::clone(&reopened), catalog, manager, documents),
    )));
    let restarted_webview = WebviewWindowBuilder::new(&restarted, "main", Default::default())
        .build()
        .unwrap();
    let listed = invoke(
        &restarted_webview,
        "list_printers",
        json!({"contractVersion":1}),
    )
    .unwrap();
    assert!(listed["data"]
        .as_array()
        .unwrap()
        .iter()
        .any(|printer| printer["id"] == id));
    let serialized = serde_json::to_string(&listed).unwrap();
    assert!(!serialized.contains("F0_FIXTURE_SENTINEL"));
}

#[test]
fn complete_f1_mock_runtime_tracer_crosses_migration_restart_events_and_documents() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    fs::create_dir_all(&metadata).unwrap();
    for name in ["settings.json", "printers.json", "credentials.json"] {
        fs::copy(fixture(name), metadata.join(name)).unwrap();
    }
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths.clone(), &lease).unwrap());
    migrate_legacy(&storage).unwrap();
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../public/catalog/printer-catalog.json");
    let catalog = Arc::new(farm3d_lib::catalog::load_snapshot(&catalog_path).unwrap());
    let documents = Arc::new(TracerDocuments::default());
    *documents.settings_import.lock().unwrap() = serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "exportedAt": "2026-09-17T00:00:00Z",
        "settings": {"themeMode": "farm3d-light"}
    }))
    .unwrap();

    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::settings::commands::load_settings,
            farm3d_lib::settings::commands::export_settings,
            farm3d_lib::settings::commands::import_settings,
            farm3d_lib::printers::commands::list_printers,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::export_printers,
            farm3d_lib::printers::commands::import_printers,
            farm3d_lib::connections::commands::printer_statuses,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let manager = Arc::new(farm3d_lib::connections::supervisor::ConnectionManager::new(
        app.handle().clone(),
        Arc::new(
            farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(&storage)),
        ),
    ));
    let document_boundary: Arc<dyn farm3d_lib::document_io::DocumentIo> = documents.clone();
    app.manage(BootstrapState::ready_with(Arc::new(
        RuntimeServices::for_test(
            Arc::clone(&storage),
            Arc::clone(&catalog),
            Arc::clone(&manager),
            document_boundary,
        ),
    )));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let initial = invoke(&webview, "list_printers", json!({"contractVersion":1})).unwrap();
    assert_eq!(initial["data"].as_array().unwrap().len(), 2);
    let model = &catalog.models[0];
    let variant = &model.variants[0];
    let created = invoke(&webview, "create_printer", json!({"contractVersion":1,
        "name":"Vertical tracer",
        "catalogRef":{"vendor":model.vendor,"model":model.model,"variant":variant.variant,"modelId":model.model_id,"printerVariant":variant.printer_variant}
    })).unwrap();
    let created_id = created["data"]["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    invoke(&webview, "export_settings", json!({"contractVersion":1})).unwrap();
    invoke(&webview, "export_printers", json!({"contractVersion":1})).unwrap();
    assert!(!documents.settings_export.lock().unwrap().is_empty());
    let printers_export = documents.printers_export.lock().unwrap().clone();
    assert!(!printers_export.is_empty());
    *documents.printers_import.lock().unwrap() = printers_export;
    invoke(
        &webview,
        "import_settings",
        json!({"contractVersion":1,"expectedRevision":1}),
    )
    .unwrap();
    let before_import = invoke(&webview, "list_printers", json!({"contractVersion":1})).unwrap();
    let expected = before_import["data"]
        .as_array()
        .unwrap()
        .iter()
        .map(|printer| {
            json!({
                "id": printer["id"], "revision": printer["revision"]
            })
        })
        .collect::<Vec<_>>();
    invoke(
        &webview,
        "import_printers",
        json!({
            "contractVersion":1, "expectedRevisions": expected
        }),
    )
    .unwrap();

    let (event_tx, event_rx) = std::sync::mpsc::channel();
    app.listen(
        farm3d_lib::connections::supervisor::STATUS_EVENT,
        move |event| {
            event_tx.send(event.payload().to_string()).unwrap();
        },
    );
    tauri::async_runtime::block_on(manager.report_error(
        &created_id,
        "safe tracer status",
        farm3d_lib::connections::supervisor::PrinterSetupFacts::complete(),
    ));
    let event: Value = serde_json::from_str(&event_rx.recv().unwrap()).unwrap();
    let backfill = invoke(&webview, "printer_statuses", json!({"contractVersion":1})).unwrap();
    assert_eq!(event["sequence"], backfill["data"]["snapshotSequence"]);
    assert_eq!(event["streamId"], backfill["data"]["streamId"]);
    assert_eq!(
        backfill["data"]["statuses"]
            .as_array()
            .unwrap()
            .iter()
            .filter(|row| row["printerId"] == created_id)
            .count(),
        1
    );

    drop((webview, app, storage));
    let reopened = Arc::new(Storage::open(paths, &lease).unwrap());
    let persisted = PrinterRepository::new(Arc::clone(&reopened))
        .get(&created_id)
        .unwrap()
        .unwrap();
    assert!(persisted.revision >= 2);
    let settings = farm3d_lib::settings::repository::SettingsRepository::new(reopened)
        .load()
        .unwrap();
    assert_eq!(
        (settings.revision, settings.theme_mode.as_str()),
        (2, "farm3d-light")
    );

    let generated = fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/generated/contracts/command/CommandContracts.ts"),
    )
    .unwrap();
    assert!(!generated.contains("F0_FIXTURE_SENTINEL"));
}

#[test]
fn status_runtime_hydrates_stale_then_publishes_live_and_removal_once() {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths.clone(), &lease).unwrap());
    PrinterRepository::new(Arc::clone(&storage))
        .create(farm3d_lib::printers::StoredPrinter {
            id: "prn-status".to_string(),
            name: "Status tracer".to_string(),
            ..Default::default()
        })
        .unwrap();
    let first = mock_builder().build(mock_context(noop_assets())).unwrap();
    let first_manager = Arc::new(farm3d_lib::connections::supervisor::ConnectionManager::new(
        first.handle().clone(),
        Arc::new(
            farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(&storage)),
        ),
    ));
    let (first_events_tx, first_events_rx) = std::sync::mpsc::channel();
    first.listen(
        farm3d_lib::connections::supervisor::STATUS_EVENT,
        move |event| {
            first_events_tx.send(event.payload().to_string()).unwrap();
        },
    );
    first_manager.apply_observation(
        "prn-status",
        farm3d_lib::connections::ConnectionObservation::Telemetry(
            farm3d_lib::connections::status_repository::PrinterTelemetry {
                host_activity: farm3d_lib::printers::operational::HostActivity::Idle,
                host_activity_name: Some("standby".to_string()),
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
            },
        ),
        farm3d_lib::connections::supervisor::PrinterSetupFacts::complete(),
    );
    let first_event: Value = serde_json::from_str(&first_events_rx.recv().unwrap()).unwrap();
    assert_eq!(first_event["payload"]["type"], "changed");
    assert_eq!(first_event["payload"]["status"]["freshness"], "fresh");
    drop((first_manager, first, storage));

    let reopened = Arc::new(Storage::open(paths, &lease).unwrap());
    let second = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::connections::commands::printer_statuses
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let second_manager = Arc::new(
        farm3d_lib::connections::supervisor::ConnectionManager::with_clock(
            second.handle().clone(),
            Arc::new(
                farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(
                    &reopened,
                )),
            ),
            || {
                chrono::DateTime::parse_from_rfc3339("2026-09-18T12:00:10Z")
                    .unwrap()
                    .into()
            },
        ),
    );
    let catalog = Arc::new(farm3d_lib::catalog::Catalog {
        generated_at: String::new(),
        source_tag: String::new(),
        notice: String::new(),
        models: Vec::new(),
    });
    let documents: Arc<dyn farm3d_lib::document_io::DocumentIo> = Arc::new(
        farm3d_lib::document_io::NativeDocumentIo::new(second.handle().clone()),
    );
    second.manage(BootstrapState::ready_with(Arc::new(
        RuntimeServices::for_test(
            Arc::clone(&reopened),
            catalog,
            Arc::clone(&second_manager),
            documents,
        ),
    )));
    let second_webview = WebviewWindowBuilder::new(&second, "main", Default::default())
        .build()
        .unwrap();
    let backfill = invoke(
        &second_webview,
        "printer_statuses",
        json!({"contractVersion": 1}),
    )
    .unwrap()["data"]
        .clone();
    assert_eq!(backfill["statuses"][0]["status"]["freshness"], "stale");
    assert_eq!(
        backfill["statuses"][0]["status"]["telemetry"]["nozzleTempC"],
        215.0
    );

    let (events_tx, events_rx) = std::sync::mpsc::channel();
    second.listen(
        farm3d_lib::connections::supervisor::STATUS_EVENT,
        move |event| {
            events_tx.send(event.payload().to_string()).unwrap();
        },
    );
    second_manager.apply_observation(
        "prn-status",
        farm3d_lib::connections::ConnectionObservation::Telemetry(
            farm3d_lib::connections::status_repository::PrinterTelemetry {
                host_activity: farm3d_lib::printers::operational::HostActivity::Idle,
                host_activity_name: Some("standby".to_string()),
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
            },
        ),
        farm3d_lib::connections::supervisor::PrinterSetupFacts::complete(),
    );
    let live_event: Value = serde_json::from_str(&events_rx.recv().unwrap()).unwrap();
    assert_eq!(live_event["type"], "printer.status.changed");
    assert_eq!(live_event["payload"]["type"], "changed");
    assert_eq!(live_event["payload"]["status"]["freshness"], "fresh");
    let racing_backfill = invoke(
        &second_webview,
        "printer_statuses",
        json!({"contractVersion": 1}),
    )
    .unwrap()["data"]
        .clone();
    assert_eq!(racing_backfill["streamId"], live_event["streamId"]);
    assert_eq!(racing_backfill["snapshotSequence"], live_event["sequence"]);
    assert_eq!(racing_backfill["statuses"].as_array().unwrap().len(), 1);
    assert_eq!(
        racing_backfill["statuses"][0]["status"],
        live_event["payload"]["status"]
    );
    assert!(
        events_rx.try_recv().is_err(),
        "one observation must emit one event"
    );

    tauri::async_runtime::block_on(second_manager.stop_and_wait("prn-status"));
    let removed: Value = serde_json::from_str(&events_rx.recv().unwrap()).unwrap();
    assert_eq!(removed["payload"]["type"], "removed");
    assert!(second_manager.status_backfill().statuses.is_empty());
}

#[test]
fn printer_statuses_command_exposes_recoverable_cache_write_warning() {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::connections::commands::printer_statuses
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let manager = Arc::new(farm3d_lib::connections::supervisor::ConnectionManager::new(
        app.handle().clone(),
        Arc::new(
            farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(&storage)),
        ),
    ));
    let (events_tx, events_rx) = std::sync::mpsc::channel();
    app.listen(
        farm3d_lib::connections::supervisor::STATUS_EVENT,
        move |event| events_tx.send(event.payload().to_string()).unwrap(),
    );
    manager.apply_observation(
        "prn-cache-warning",
        farm3d_lib::connections::ConnectionObservation::Telemetry(
            farm3d_lib::connections::status_repository::PrinterTelemetry {
                host_activity: farm3d_lib::printers::operational::HostActivity::Idle,
                host_activity_name: None,
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
            },
        ),
        farm3d_lib::connections::supervisor::PrinterSetupFacts::complete(),
    );
    let warning_event: Value = serde_json::from_str(&events_rx.recv().unwrap()).unwrap();
    assert_eq!(warning_event["type"], "printer.status.changed");
    assert_eq!(
        warning_event["payload"]["status"]["connectionState"],
        "online"
    );
    assert_eq!(
        warning_event["payload"]["status"]["cacheWarnings"][0]["operation"],
        "save"
    );
    let catalog = Arc::new(farm3d_lib::catalog::Catalog {
        generated_at: String::new(),
        source_tag: String::new(),
        notice: String::new(),
        models: Vec::new(),
    });
    let documents: Arc<dyn farm3d_lib::document_io::DocumentIo> = Arc::new(
        farm3d_lib::document_io::NativeDocumentIo::new(app.handle().clone()),
    );
    app.manage(BootstrapState::ready_with(Arc::new(
        RuntimeServices::for_test(storage, catalog, manager, documents),
    )));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();

    let backfill = invoke(&webview, "printer_statuses", json!({"contractVersion": 1})).unwrap();

    assert_eq!(
        backfill["data"]["statuses"][0]["status"]["connectionState"],
        "online"
    );
    assert_eq!(backfill["data"]["cacheWarnings"][0]["operation"], "save");
    assert_eq!(
        backfill["data"]["cacheWarnings"][0]["printerId"],
        "prn-cache-warning"
    );
}
