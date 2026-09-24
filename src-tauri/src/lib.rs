pub mod bootstrap;
pub mod catalog;
pub mod connections;
pub mod contracts;
pub mod document_io;
pub mod library;
pub mod persistence;
pub mod printers;
pub mod settings;
pub mod spools;

use catalog::commands::{
    catalog_info, list_catalog_models, list_catalog_variants, preview_profile,
};
use connections::commands::{
    clear_printer_connection, credential_store_info, discover_printers, printer_statuses,
    set_printer_connection, test_printer_connection,
};
use connections::supervisor::ConnectionManager;
use printers::batch::{cancel_printer_batch, create_printers_batch};
use printers::commands::{
    archive_printer, create_printer, delete_printer, export_printers, import_printers,
    list_duplicate_host_archives, list_printers, printer_lifecycle_eligibility, rebind_printer,
    resolve_profile_drift, set_material_slot_layout, set_printer_override, unarchive_printer,
    update_printer,
};
use printers::create::probe_connection;
use settings::commands::{export_settings, import_settings, load_settings, save_settings};
use spools::commands::{
    create_spool, create_tare, delete_tare, list_spools, move_spool, record_spool_amount,
    set_spool_lifecycle, spool_history, update_spool, update_tare,
};
use std::sync::Arc;
use tauri::path::BaseDirectory;
use tauri::Manager;

pub struct RuntimeServices<R: tauri::Runtime> {
    pub storage: Arc<persistence::Storage>,
    pub catalog: Arc<catalog::Catalog>,
    pub manager: Arc<ConnectionManager<R>>,
    pub documents: Arc<dyn document_io::DocumentIo>,
    pub credentials: Arc<connections::credentials::CredentialStore>,
    /// D11: the inventory event stream's id and sequence.
    pub inventory_stream: spools::events::InventoryStream,
    /// D11's availability signal: one `InventoryChange` per committed
    /// inventory write. P7's evaluator subscribes; P3 has only tests.
    pub inventory_changes: tokio::sync::broadcast::Sender<spools::events::InventoryChange>,
    _lease: Option<RuntimeServicesLease>,
}

fn inventory_changes() -> tokio::sync::broadcast::Sender<spools::events::InventoryChange> {
    tokio::sync::broadcast::channel(spools::events::INVENTORY_CHANGE_CAPACITY).0
}

pub struct RuntimeServicesLease {
    pub storage: Arc<persistence::Storage>,
    _lease: persistence::MetadataRootLease,
}

impl RuntimeServicesLease {
    pub fn new(storage: Arc<persistence::Storage>, lease: persistence::MetadataRootLease) -> Self {
        Self {
            storage,
            _lease: lease,
        }
    }
}

impl<R: tauri::Runtime> RuntimeServices<R> {
    pub fn for_test(
        storage: Arc<persistence::Storage>,
        catalog: Arc<catalog::Catalog>,
        manager: Arc<ConnectionManager<R>>,
        documents: Arc<dyn document_io::DocumentIo>,
    ) -> Self {
        Self {
            storage,
            catalog,
            manager,
            documents,
            credentials: Arc::new(connections::credentials::CredentialStore::file_backed(
                std::env::temp_dir()
                    .join(format!("farm3d-test-credentials-{}", uuid::Uuid::new_v4())),
            )),
            inventory_stream: spools::events::InventoryStream::default(),
            inventory_changes: inventory_changes(),
            _lease: None,
        }
    }
}

pub const COMMAND_NAMES: [&str; 41] = [
    "load_settings",
    "save_settings",
    "export_settings",
    "import_settings",
    "list_printers",
    "create_printer",
    "set_material_slot_layout",
    "update_printer",
    "delete_printer",
    "set_printer_override",
    "rebind_printer",
    "resolve_profile_drift",
    "printer_lifecycle_eligibility",
    "archive_printer",
    "unarchive_printer",
    "export_printers",
    "import_printers",
    "list_catalog_models",
    "list_catalog_variants",
    "preview_profile",
    "catalog_info",
    "set_printer_connection",
    "clear_printer_connection",
    "test_printer_connection",
    "credential_store_info",
    "discover_printers",
    "printer_statuses",
    "probe_connection",
    "create_printers_batch",
    "cancel_printer_batch",
    "list_duplicate_host_archives",
    "list_spools",
    "spool_history",
    "create_spool",
    "update_spool",
    "record_spool_amount",
    "move_spool",
    "set_spool_lifecycle",
    "create_tare",
    "update_tare",
    "delete_tare",
];

/// `pub` (rather than crate-private) solely so `tests/p2_lifecycle.rs` can
/// exercise a simulated app restart over the same storage — mirrors what
/// `build_runtime_services` does at real startup.
pub fn restore_persisted_connections<R: tauri::Runtime>(
    manager: &Arc<ConnectionManager<R>>,
    storage: Arc<persistence::Storage>,
    store: &connections::credentials::CredentialStore,
    catalog: &catalog::Catalog,
) -> Result<(), persistence::StorageError> {
    let stored_printers = printers::repository::PrinterRepository::new(storage).list()?;
    for printer in &stored_printers {
        tauri::async_runtime::block_on(printers::setup::supervise_printer(
            manager, store, catalog, printer,
        ));
    }
    Ok(())
}

enum StartupFailure {
    Fatal,
    Recoverable(contracts::command::CommandError),
}

pub fn startup_command_error(
    error: persistence::StorageError,
) -> Result<contracts::command::CommandError, ()> {
    use contracts::command::CommandError;
    match error {
        persistence::StorageError::UnsupportedLocking => Err(()),
        persistence::StorageError::MigrationFailed => Ok(CommandError::migration_failed()),
        persistence::StorageError::UnsupportedSchemaVersion => Ok(
            CommandError::unsupported_schema(persistence::CURRENT_SCHEMA_VERSION),
        ),
        persistence::StorageError::CorruptData {
            source_name,
            source_sha256,
        } => Ok(CommandError::legacy_corrupt(source_name, source_sha256)),
        persistence::StorageError::Database | persistence::StorageError::InvalidSnapshot => {
            Ok(CommandError::database_corrupt())
        }
        persistence::StorageError::PathCollision
        | persistence::StorageError::PersistenceUnavailable
        | persistence::StorageError::Filesystem
        | persistence::StorageError::OperationFailed => Ok(CommandError::persistence_unavailable()),
        // Startup only ever reads through storage (restoring persisted
        // Connections); this write-path variant cannot occur here.
        persistence::StorageError::DuplicateHost(_) => Ok(CommandError::internal()),
    }
}

fn startup_error(error: persistence::StorageError) -> StartupFailure {
    match startup_command_error(error) {
        Ok(error) => StartupFailure::Recoverable(error),
        Err(()) => StartupFailure::Fatal,
    }
}

fn build_runtime_services<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    retained_lease: &Arc<std::sync::Mutex<Option<persistence::MetadataRootLease>>>,
) -> Result<RuntimeServices<R>, StartupFailure> {
    let metadata_root = app
        .path()
        .app_config_dir()
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    let paths = persistence::StoragePaths::new(metadata_root, data_root).map_err(startup_error)?;
    let storage = {
        let mut lease = retained_lease.lock().map_err(|_| {
            StartupFailure::Recoverable(contracts::command::CommandError::internal())
        })?;
        if lease.is_none() {
            *lease = Some(persistence::MetadataRootLease::acquire(&paths).map_err(startup_error)?);
        }
        Arc::new(
            persistence::Storage::open(paths, lease.as_ref().expect("lease was initialized"))
                .map_err(startup_error)?,
        )
    };
    persistence::migrate_legacy(&storage).map_err(startup_error)?;
    settings::repository::SettingsRepository::new(Arc::clone(&storage))
        .ensure_default()
        .map_err(startup_error)?;

    let resource_path = app
        .path()
        .resolve("resources/printer-catalog.json", BaseDirectory::Resource)
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    let catalog =
        Arc::new(catalog::load_snapshot(&resource_path).map_err(|_| {
            StartupFailure::Recoverable(contracts::command::CommandError::internal())
        })?);

    let cleanup_pending = storage
        .read(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM pending_credential_cleanup WHERE reason != 'import_orphan')",
                [],
                |row| row.get::<_, bool>(0),
            )
        })
        .map_err(startup_error)?;
    let credential_store = Arc::new(connections::credentials::CredentialStore::detect(
        app.path().app_config_dir().map_err(|_| {
            StartupFailure::Recoverable(contracts::command::CommandError::internal())
        })?,
    ));
    if cleanup_pending {
        connections::commands::retry_pending_credential_cleanup(
            &storage,
            credential_store.as_ref(),
        )
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    }

    let manager = Arc::new(ConnectionManager::new(
        app.clone(),
        Arc::new(connections::status_repository::StatusRepository::new(
            Arc::clone(&storage),
        )),
    ));
    restore_persisted_connections(
        &manager,
        Arc::clone(&storage),
        credential_store.as_ref(),
        &catalog,
    )
    .map_err(startup_error)?;
    let lease = retained_lease
        .lock()
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?
        .take()
        .ok_or_else(|| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    Ok(RuntimeServices {
        storage: Arc::clone(&storage),
        catalog,
        manager,
        documents: Arc::new(document_io::NativeDocumentIo::new(app.clone())),
        credentials: credential_store,
        inventory_stream: spools::events::InventoryStream::default(),
        inventory_changes: inventory_changes(),
        _lease: Some(RuntimeServicesLease::new(Arc::clone(&storage), lease)),
    })
}

#[cfg(test)]
pub(crate) fn test_storage() -> (
    tempfile::TempDir,
    persistence::MetadataRootLease,
    Arc<persistence::Storage>,
) {
    let temporary_root = tempfile::tempdir().expect("temporary root");
    let paths = persistence::StoragePaths::new(
        temporary_root.path().join("metadata"),
        temporary_root.path().join("data"),
    )
    .expect("storage paths");
    let lease = persistence::MetadataRootLease::acquire(&paths).expect("metadata lease");
    let storage = Arc::new(persistence::Storage::open(paths, &lease).expect("storage"));
    (temporary_root, lease, storage)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printers::StoredPrinter;

    #[test]
    fn startup_propagates_printer_decode_failure() {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            persistence::StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
                .unwrap();
        let lease = persistence::MetadataRootLease::acquire(&paths).unwrap();
        let storage = Arc::new(persistence::Storage::open(paths, &lease).unwrap());
        let printer = printers::repository::PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-corrupt-startup".to_string(),
                name: "Corrupt startup fixture".to_string(),
                ..Default::default()
            })
            .unwrap();
        storage
            .write(|transaction| {
                transaction.execute(
                    "UPDATE printers SET overrides_json='{\"printableHeightMm\":\"not-a-number\"}' WHERE id=?1",
                    [&printer.id],
                )?;
                Ok(())
            })
            .unwrap();
        let app = tauri::test::mock_builder()
            .build(tauri::test::mock_context(tauri::test::noop_assets()))
            .unwrap();
        let manager = Arc::new(ConnectionManager::new(
            app.handle().clone(),
            Arc::new(connections::status_repository::StatusRepository::new(
                Arc::clone(&storage),
            )),
        ));
        let credentials =
            connections::credentials::CredentialStore::file_backed(temp.path().join("credentials"));

        let catalog = Arc::new(catalog::Catalog {
            generated_at: String::new(),
            source_tag: String::new(),
            notice: String::new(),
            models: vec![],
        });
        let error =
            restore_persisted_connections(&manager, storage, &credentials, &catalog).unwrap_err();

        assert!(matches!(
            error,
            persistence::StorageError::CorruptData {
                source_name: "database",
                source_sha256: None
            }
        ));
    }
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        .setup(|app| {
            let handle = app.handle().clone();
            let retained_lease = Arc::new(std::sync::Mutex::new(None));
            match build_runtime_services(&handle, &retained_lease) {
                Ok(services) => {
                    app.manage(bootstrap::BootstrapState::ready_with(Arc::new(services)));
                    Ok(())
                }
                Err(StartupFailure::Recoverable(error)) => {
                    let retry_handle = handle.clone();
                    let retry_lease = Arc::clone(&retained_lease);
                    app.manage(bootstrap::BootstrapState::failed(error, move || {
                        build_runtime_services(&retry_handle, &retry_lease)
                            .map(Arc::new)
                            .map_err(|failure| match failure {
                                StartupFailure::Recoverable(error) => error,
                                StartupFailure::Fatal => contracts::command::CommandError::internal(),
                            })
                    }));
                    Ok(())
                }
                Err(StartupFailure::Fatal) => Err(std::io::Error::other(
                    "Unsupported metadata locking. This platform or data location cannot run farm3d; contact support with the platform and filesystem type.",
                )
                .into()),
            }
        })
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            export_settings,
            import_settings,
            list_printers,
            create_printer,
            set_material_slot_layout,
            update_printer,
            delete_printer,
            set_printer_override,
            rebind_printer,
            resolve_profile_drift,
            printer_lifecycle_eligibility,
            archive_printer,
            unarchive_printer,
            export_printers,
            import_printers,
            list_catalog_models,
            list_catalog_variants,
            preview_profile,
            catalog_info,
            set_printer_connection,
            clear_printer_connection,
            test_printer_connection,
            credential_store_info,
            discover_printers,
            printer_statuses,
            probe_connection,
            create_printers_batch,
            cancel_printer_batch,
            list_duplicate_host_archives,
            list_spools,
            spool_history,
            create_spool,
            update_spool,
            record_spool_amount,
            move_spool,
            set_spool_lifecycle,
            create_tare,
            update_tare,
            delete_tare,
            #[cfg(debug_assertions)]
            spools::commands::debug_seed_reservation,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
