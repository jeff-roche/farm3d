use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use farm3d_lib::bootstrap::BootstrapState;
use farm3d_lib::connections::supervisor::ConnectionManager;
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::document_io::{DocumentIo, DocumentKind};
use farm3d_lib::persistence::{FailurePoint, MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{CatalogRef, StartSafety, StoredPrinter};
use farm3d_lib::settings::commands::{MonitorDensity, MonitorSection};
use farm3d_lib::settings::repository::SettingsRepository;
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindowBuilder};

#[derive(Default)]
struct InjectedDocuments {
    open: Mutex<Option<PathBuf>>,
    save: Mutex<Option<PathBuf>>,
    bytes: Mutex<Option<Vec<u8>>>,
    read_fails: std::sync::atomic::AtomicBool,
    write_fails: std::sync::atomic::AtomicBool,
    writes: Mutex<Vec<Vec<u8>>>,
    after_snapshot: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    after_printers_commit: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}

impl DocumentIo for InjectedDocuments {
    fn open_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.open.lock().unwrap().clone())
    }

    fn save_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.save.lock().unwrap().clone())
    }

    fn read(&self, _path: &Path) -> Result<Vec<u8>, CommandError> {
        if self.read_fails.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CommandError::persistence_unavailable());
        }
        Ok(self.bytes.lock().unwrap().clone().unwrap_or_default())
    }

    fn atomic_write(&self, _path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        if self.write_fails.load(std::sync::atomic::Ordering::SeqCst) {
            return Err(CommandError::persistence_unavailable());
        }
        self.writes.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }

    fn after_snapshot(&self, _kind: DocumentKind) -> Result<(), CommandError> {
        if let Some(callback) = self.after_snapshot.lock().unwrap().take() {
            callback();
        }
        Ok(())
    }

    fn after_printers_commit(&self) {
        if let Some(callback) = self.after_printers_commit.lock().unwrap().take() {
            callback();
        }
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

fn settings_runtime(
    storage: Arc<Storage>,
    documents: Arc<InjectedDocuments>,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
) {
    let documents: Arc<dyn DocumentIo> = documents;
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::settings::commands::export_settings,
            farm3d_lib::settings::commands::import_settings,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../public/catalog/printer-catalog.json");
    let catalog = Arc::new(farm3d_lib::catalog::load_snapshot(&catalog_path).unwrap());
    let manager = Arc::new(ConnectionManager::new(
        app.handle().clone(),
        Arc::new(
            farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(&storage)),
        ),
    ));
    app.manage(BootstrapState::ready_with(Arc::new(
        RuntimeServices::for_test(storage, catalog, manager, documents),
    )));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview)
}

fn storage() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>) {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    SettingsRepository::new(Arc::clone(&storage))
        .ensure_default()
        .unwrap();
    (temp, lease, storage)
}

fn settings_document(theme: &str) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "exportedAt": "2026-09-17T00:00:00Z",
        "settings": { "themeMode": theme },
    }))
    .unwrap()
}

fn printers_document(printers: Value) -> Vec<u8> {
    printers_document_versioned(1, printers)
}

fn printers_document_versioned(schema_version: i64, printers: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schemaVersion": schema_version,
        "exportedAt": "2026-09-17T00:00:00Z",
        "printers": printers,
    }))
    .unwrap()
}

fn imported_printer(id: &str, kind: &str) -> Value {
    json!({
        "id": id,
        "revision": 1,
        "name": "Imported",
        "catalogRef": {
            "vendor": "Unknown Vendor",
            "model": "Unknown Model",
            "variant": "Unknown Variant",
            "modelId": "unknown",
            "printerVariant": "0.4"
        },
        "notes": "",
        "overrides": {},
        "lastKnownGood": null,
        "connection": {
            "kind": kind,
            "host": "printer.invalid",
            "port": 7125,
            "useTls": false
        }
    })
}

fn printers_runtime(
    storage: Arc<Storage>,
    documents: Arc<InjectedDocuments>,
) -> (
    tauri::App<tauri::test::MockRuntime>,
    tauri::WebviewWindow<tauri::test::MockRuntime>,
    Arc<ConnectionManager<tauri::test::MockRuntime>>,
) {
    let documents: Arc<dyn DocumentIo> = documents;
    let catalog_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../public/catalog/printer-catalog.json");
    let catalog = Arc::new(farm3d_lib::catalog::load_snapshot(&catalog_path).unwrap());
    let app = mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::printers::commands::export_printers,
            farm3d_lib::printers::commands::import_printers,
        ])
        .build(mock_context(noop_assets()))
        .unwrap();
    let manager = Arc::new(ConnectionManager::new(
        app.handle().clone(),
        Arc::new(
            farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(&storage)),
        ),
    ));
    app.manage(BootstrapState::ready_with(Arc::new(
        RuntimeServices::for_test(storage, catalog, Arc::clone(&manager), documents),
    )));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview, manager)
}

fn error_code(error: &Value) -> &str {
    error["code"].as_str().unwrap()
}

#[test]
fn settings_dialog_cancellation_has_no_snapshot_read_write_or_revision_change() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    let (_app, webview) = settings_runtime(Arc::clone(&storage), Arc::clone(&documents));

    let imported = invoke(
        &webview,
        "import_settings",
        json!({"contractVersion":1,"expectedRevision":1}),
    )
    .unwrap();
    let exported = invoke(&webview, "export_settings", json!({"contractVersion":1})).unwrap();

    assert_eq!(imported["data"]["status"], "cancelled");
    assert_eq!(exported["data"]["status"], "cancelled");
    assert_eq!(SettingsRepository::new(storage).load().unwrap().revision, 1);
    assert!(documents.writes.lock().unwrap().is_empty());
}

#[test]
fn incompatible_request_version_is_a_structured_error_before_bootstrap_or_dialog_work() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    let (_app, webview) = settings_runtime(storage, Arc::clone(&documents));

    let error = invoke(&webview, "export_settings", json!({"contractVersion":2})).unwrap_err();

    assert_eq!(error["code"], "INCOMPATIBLE_CONTRACT_VERSION");
    assert_eq!(error["recovery"], json!(["UPGRADE_FARM3D"]));
    assert_eq!(
        error["details"],
        json!({"supportedVersion":1,"receivedVersion":2})
    );
    assert!(documents.writes.lock().unwrap().is_empty());
}

#[test]
fn settings_import_maps_read_snapshot_validation_and_stale_failures_without_writes() {
    for scenario in ["read", "snapshot", "validation", "stale"] {
        let (_temp, _lease, storage) = storage();
        let documents = Arc::new(InjectedDocuments::default());
        *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
        *documents.bytes.lock().unwrap() = Some(settings_document("farm3d-dark"));
        match scenario {
            "read" => documents
                .read_fails
                .store(true, std::sync::atomic::Ordering::SeqCst),
            "snapshot" => storage.inject_failure_once(FailurePoint::Snapshot),
            "validation" => *documents.bytes.lock().unwrap() = Some(b"{}".to_vec()),
            "stale" => {
                let storage = Arc::clone(&storage);
                *documents.after_snapshot.lock().unwrap() = Some(Box::new(move || {
                    SettingsRepository::new(storage)
                        .save(
                            1,
                            "farm3d-light",
                            MonitorSection::PrinterModel,
                            MonitorDensity::Comfortable,
                        )
                        .unwrap();
                }));
            }
            _ => unreachable!(),
        }
        let (_app, webview) = settings_runtime(Arc::clone(&storage), documents);

        let error = invoke(
            &webview,
            "import_settings",
            json!({"contractVersion":1,"expectedRevision":1}),
        )
        .unwrap_err();
        let expected = match scenario {
            "read" | "snapshot" => "PERSISTENCE_UNAVAILABLE",
            "validation" => "VALIDATION",
            "stale" => "CONFLICT",
            _ => unreachable!(),
        };
        assert_eq!(error_code(&error), expected, "scenario {scenario}: {error}");
        let record = SettingsRepository::new(storage).load().unwrap();
        if scenario == "stale" {
            assert_eq!(
                (record.revision, record.theme_mode.as_str()),
                (2, "farm3d-light")
            );
        } else {
            assert_eq!((record.revision, record.theme_mode.as_str()), (1, "system"));
        }
    }
}

#[test]
fn settings_import_transaction_failure_rolls_back_and_export_io_failure_is_structured() {
    let (_temp, _lease, storage) = storage();
    storage.write(|transaction| {
        transaction.execute_batch("CREATE TRIGGER reject_settings_update BEFORE UPDATE ON settings BEGIN SELECT RAISE(FAIL, 'injected'); END;")?;
        Ok(())
    }).unwrap();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
    *documents.save.lock().unwrap() = Some(PathBuf::from("destination.json"));
    *documents.bytes.lock().unwrap() = Some(settings_document("farm3d-dark"));
    documents
        .write_fails
        .store(true, std::sync::atomic::Ordering::SeqCst);
    let (_app, webview) = settings_runtime(Arc::clone(&storage), documents);

    let import_error = invoke(
        &webview,
        "import_settings",
        json!({"contractVersion":1,"expectedRevision":1}),
    )
    .unwrap_err();
    let export_error =
        invoke(&webview, "export_settings", json!({"contractVersion":1})).unwrap_err();

    assert_eq!(error_code(&import_error), "PERSISTENCE_UNAVAILABLE");
    assert_eq!(error_code(&export_error), "PERSISTENCE_UNAVAILABLE");
    assert_eq!(SettingsRepository::new(storage).load().unwrap().revision, 1);
}

#[test]
fn printers_dialog_cancellation_and_injected_failures_leave_the_set_unchanged() {
    for scenario in [
        "cancel",
        "read",
        "snapshot",
        "validation",
        "stale",
        "rollback",
    ] {
        let (_temp, _lease, storage) = storage();
        let documents = Arc::new(InjectedDocuments::default());
        if scenario != "cancel" {
            *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
            *documents.bytes.lock().unwrap() = Some(printers_document(json!([])));
        }
        match scenario {
            "read" => documents
                .read_fails
                .store(true, std::sync::atomic::Ordering::SeqCst),
            "snapshot" => storage.inject_failure_once(FailurePoint::Snapshot),
            "validation" => *documents.bytes.lock().unwrap() = Some(b"{}".to_vec()),
            "stale" => {
                let storage = Arc::clone(&storage);
                *documents.after_snapshot.lock().unwrap() = Some(Box::new(move || {
                    PrinterRepository::new(storage)
                        .create(StoredPrinter {
                            id: "concurrent".to_string(),
                            name: "Concurrent".to_string(),
                            catalog_ref: CatalogRef {
                                vendor: "v".to_string(),
                                model: "m".to_string(),
                                variant: "x".to_string(),
                                model_id: "i".to_string(),
                                printer_variant: "0.4".to_string(),
                            },
                            ..Default::default()
                        })
                        .unwrap();
                }));
            }
            "rollback" => {
                *documents.bytes.lock().unwrap() =
                    Some(printers_document(json!([imported_printer(
                        "imported",
                        "moonraker"
                    )])));
                storage.write(|transaction| {
                    transaction.execute_batch("CREATE TRIGGER reject_printer_insert BEFORE INSERT ON printers BEGIN SELECT RAISE(FAIL, 'injected'); END;")?;
                    Ok(())
                }).unwrap();
            }
            _ => {}
        }
        let (_app, webview, _manager) = printers_runtime(Arc::clone(&storage), documents);
        let result = invoke(
            &webview,
            "import_printers",
            json!({
                "contractVersion": 1,
                "expectedRevisions": []
            }),
        );

        if scenario == "cancel" {
            assert_eq!(result.unwrap()["data"]["status"], "cancelled");
        } else {
            let error = result.unwrap_err();
            let expected = match scenario {
                "read" | "snapshot" | "rollback" => "PERSISTENCE_UNAVAILABLE",
                "validation" => "VALIDATION",
                "stale" => "CONFLICT",
                _ => unreachable!(),
            };
            assert_eq!(error_code(&error), expected, "scenario {scenario}: {error}");
        }
        let ids = PrinterRepository::new(storage)
            .list()
            .unwrap()
            .into_iter()
            .map(|printer| printer.id)
            .collect::<Vec<_>>();
        assert_eq!(
            ids,
            if scenario == "stale" {
                vec!["concurrent".to_string()]
            } else {
                vec![]
            }
        );
    }
}

#[test]
fn printers_export_io_failure_is_structured_and_post_commit_supervisor_failure_is_a_warning() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.save.lock().unwrap() = Some(PathBuf::from("destination.json"));
    documents
        .write_fails
        .store(true, std::sync::atomic::Ordering::SeqCst);
    *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
    *documents.bytes.lock().unwrap() = Some(printers_document(json!([imported_printer(
        "future-adapter",
        "future-adapter"
    )])));
    let (_app, webview, manager) = printers_runtime(Arc::clone(&storage), documents);

    let export_error =
        invoke(&webview, "export_printers", json!({"contractVersion":1})).unwrap_err();
    let imported = invoke(
        &webview,
        "import_printers",
        json!({
            "contractVersion": 1,
            "expectedRevisions": []
        }),
    )
    .unwrap();

    assert_eq!(error_code(&export_error), "PERSISTENCE_UNAVAILABLE");
    assert_eq!(imported["data"]["status"], "applied");
    assert!(imported["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| { warning["code"] == "SUPERVISOR_RECONCILIATION_FAILED" }));
    assert_eq!(PrinterRepository::new(storage).list().unwrap().len(), 1);
    assert_eq!(
        manager.statuses()["future-adapter"].connection_state,
        farm3d_lib::connections::ConnectionState::Offline
    );
    assert_eq!(
        manager.statuses()["future-adapter"].operational_state,
        farm3d_lib::printers::operational::OperationalState::SetupIncomplete
    );
}

#[test]
fn printers_import_returns_committed_rows_when_a_post_commit_reread_would_fail() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
    *documents.bytes.lock().unwrap() = Some(printers_document(json!([imported_printer(
        "committed-printer",
        "moonraker"
    )])));
    let storage_after_commit = Arc::clone(&storage);
    *documents.after_printers_commit.lock().unwrap() = Some(Box::new(move || {
        storage_after_commit
            .write(|transaction| {
                transaction.execute("DROP TABLE printers", [])?;
                Ok(())
            })
            .unwrap();
    }));
    let (_app, webview, _manager) = printers_runtime(storage, documents);

    let imported = invoke(
        &webview,
        "import_printers",
        json!({"contractVersion":1,"expectedRevisions":[]}),
    )
    .unwrap();

    assert_eq!(imported["data"]["status"], "applied");
    assert_eq!(imported["data"]["printers"][0]["id"], "committed-printer");
}

#[test]
fn printers_replacement_queues_removed_credentials_as_nonautomatic_import_orphans() {
    let (_temp, _lease, storage) = storage();
    let mut existing = StoredPrinter {
        id: "existing".to_string(),
        name: "Existing".to_string(),
        catalog_ref: CatalogRef {
            vendor: "v".to_string(),
            model: "m".to_string(),
            variant: "x".to_string(),
            model_id: "i".to_string(),
            printer_variant: "0.4".to_string(),
        },
        ..Default::default()
    };
    existing.connection = Some(farm3d_lib::connections::ConnectionConfig {
        kind: "moonraker".to_string(),
        host: "old.invalid".to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: Some("farm3d/credential/6ba7b810-9dad-4f83-a131-2a6f44cbbf89".to_string()),
    });
    let existing = PrinterRepository::new(Arc::clone(&storage))
        .create(existing)
        .unwrap();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
    *documents.bytes.lock().unwrap() = Some(printers_document(json!([])));
    let (_app, webview, _manager) = printers_runtime(Arc::clone(&storage), documents);

    invoke(
        &webview,
        "import_printers",
        json!({
            "contractVersion": 1,
            "expectedRevisions": [{"id": existing.id, "revision": existing.revision}]
        }),
    )
    .unwrap();

    let reason: String = storage
        .read(|db| {
            db.query_row("SELECT reason FROM pending_credential_cleanup", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(reason, "import_orphan");
}

/// Task 6, step 1 item 1: export writes every Printer's
/// `location`/`startSafety`/`archivedAt` — `null` when unset. P3 Task 5
/// bumped the schema to 3 (`materialSlots`, D12) without changing this.
#[test]
fn printers_export_writes_schema_version_3_with_lifecycle_fields() {
    let (_temp, _lease, storage) = storage();
    let with_location = PrinterRepository::new(Arc::clone(&storage))
        .create(StoredPrinter {
            id: "prn-with-location".to_string(),
            name: "Has A Location".to_string(),
            catalog_ref: CatalogRef {
                vendor: "v".to_string(),
                model: "m".to_string(),
                variant: "x".to_string(),
                model_id: "i".to_string(),
                printer_variant: "0.4".to_string(),
            },
            location: Some("Bay 3".to_string()),
            start_safety: StartSafety::Unattended,
            ..Default::default()
        })
        .unwrap();
    PrinterRepository::new(Arc::clone(&storage))
        .create(StoredPrinter {
            id: "prn-without-location".to_string(),
            name: "No Location".to_string(),
            catalog_ref: CatalogRef {
                vendor: "v".to_string(),
                model: "m".to_string(),
                variant: "x".to_string(),
                model_id: "i".to_string(),
                printer_variant: "0.4".to_string(),
            },
            ..Default::default()
        })
        .unwrap();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.save.lock().unwrap() = Some(PathBuf::from("destination.json"));
    let (_app, webview, _manager) = printers_runtime(Arc::clone(&storage), documents.clone());

    let exported = invoke(&webview, "export_printers", json!({"contractVersion":1})).unwrap();

    assert_eq!(exported["data"]["status"], "exported");
    let writes = documents.writes.lock().unwrap();
    let document: Value = serde_json::from_slice(&writes[0]).unwrap();
    assert_eq!(document["schemaVersion"], 3);
    let printers = document["printers"].as_array().unwrap();
    let by_id = |id: &str| printers.iter().find(|row| row["id"] == id).unwrap();
    let with_location = by_id(&with_location.id);
    assert_eq!(with_location["location"], "Bay 3");
    assert_eq!(with_location["startSafety"], "unattended");
    assert_eq!(with_location["archivedAt"], Value::Null);
    let without_location = by_id("prn-without-location");
    assert_eq!(without_location["location"], Value::Null);
    assert_eq!(without_location["startSafety"], "confirmBedClear");
    assert_eq!(without_location["archivedAt"], Value::Null);
}

/// Task 6, step 1 item 3: a v1 document — one that never carries
/// `location`/`startSafety`/`archivedAt` at all — still imports, defaulted
/// to `null`/`confirmBedClear`/not archived.
#[test]
fn printers_import_defaults_lifecycle_fields_for_a_v1_document() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
    *documents.bytes.lock().unwrap() = Some(printers_document(json!([imported_printer(
        "v1-printer",
        "moonraker"
    )])));
    let (_app, webview, _manager) = printers_runtime(Arc::clone(&storage), documents);

    let imported = invoke(
        &webview,
        "import_printers",
        json!({"contractVersion":1,"expectedRevisions":[]}),
    )
    .unwrap();

    assert_eq!(imported["data"]["status"], "applied");
    assert_eq!(imported["data"]["printers"][0]["location"], Value::Null);
    assert_eq!(
        imported["data"]["printers"][0]["startSafety"],
        "confirmBedClear"
    );
    assert_eq!(imported["data"]["printers"][0]["archivedAt"], Value::Null);
    let stored = PrinterRepository::new(storage)
        .get("v1-printer")
        .unwrap()
        .unwrap();
    assert_eq!(stored.location, None);
    assert_eq!(stored.start_safety, StartSafety::ConfirmBedClear);
    assert_eq!(stored.archived_at, None);
}

/// Task 6, step 1 item 5 and the Task 1 deferred-minor coverage: an import
/// document with two active Printers on the same host imports the later
/// one (in document order) archived, with a `DUPLICATE_HOST_ARCHIVED`
/// warning — and this is the case that would otherwise have hit the
/// `printers_active_host_identity` partial unique index inside
/// `replace_all` had `archive_duplicates` not run first.
#[test]
fn printers_import_archives_a_later_duplicate_host_and_warns() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
    *documents.bytes.lock().unwrap() = Some(printers_document(json!([
        imported_printer("prn-first", "moonraker"),
        imported_printer("prn-second", "moonraker"),
    ])));
    let (_app, webview, _manager) = printers_runtime(Arc::clone(&storage), documents);

    let imported = invoke(
        &webview,
        "import_printers",
        json!({"contractVersion":1,"expectedRevisions":[]}),
    )
    .unwrap();

    assert_eq!(imported["data"]["status"], "applied");
    assert!(imported["data"]["warnings"]
        .as_array()
        .unwrap()
        .iter()
        .any(|warning| warning["code"] == "DUPLICATE_HOST_ARCHIVED"
            && warning["entityId"] == "prn-second"));
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let first = repository.get("prn-first").unwrap().unwrap();
    let second = repository.get("prn-second").unwrap().unwrap();
    assert_eq!(first.archived_at, None);
    assert!(second.archived_at.is_some());
    assert_eq!(repository.list().unwrap().len(), 2);
}

/// An archive that commits between the import's `replace_all` and its
/// supervision pass must win: import supervises the persisted row, not its
/// in-memory copy, so the archived Printer gets no live status.
#[test]
fn printers_import_does_not_supervise_a_printer_archived_after_its_commit() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    *documents.open.lock().unwrap() = Some(PathBuf::from("selected.json"));
    *documents.bytes.lock().unwrap() = Some(printers_document(json!([imported_printer(
        "raced-printer",
        "moonraker"
    )])));
    let storage_after_commit = Arc::clone(&storage);
    *documents.after_printers_commit.lock().unwrap() = Some(Box::new(move || {
        let repository = PrinterRepository::new(storage_after_commit);
        let committed = repository.get("raced-printer").unwrap().unwrap();
        repository
            .archive(&committed.id, committed.revision, "op-archive", &[])
            .unwrap();
    }));
    let (_app, webview, manager) = printers_runtime(Arc::clone(&storage), documents);

    let imported = invoke(
        &webview,
        "import_printers",
        json!({"contractVersion":1,"expectedRevisions":[]}),
    )
    .unwrap();

    assert_eq!(imported["data"]["status"], "applied");
    assert!(
        !manager.statuses().contains_key("raced-printer"),
        "a Printer archived after the import committed must not be supervised"
    );
    assert!(!manager
        .status_backfill()
        .statuses
        .iter()
        .any(|row| row.printer_id == "raced-printer"));
}
