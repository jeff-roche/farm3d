pub mod attention;
pub mod bootstrap;
pub mod cameras;
pub mod catalog;
pub mod connections;
pub mod contracts;
pub mod diagnostics;
pub mod document_io;
mod file_links;
pub mod host_ops;
pub mod incidents;
pub mod jobs;
pub mod library;
pub mod notifications;
pub mod persistence;
pub mod printers;
pub mod queue;
pub mod settings;
pub mod slicing;
pub mod spools;

use attention::commands::{
    acknowledge_attention_event, list_attention, mark_attention_read, resolve_attention_event,
};
use cameras::commands::{
    camera_preview_frame, capture_snapshot, clear_printer_camera, get_printer_camera,
    list_host_webcams, list_snapshots, media_usage, set_printer_camera, set_snapshot_pinned,
    snapshot_image, test_camera,
};
use catalog::commands::{
    catalog_info, list_catalog_models, list_catalog_variants, preview_profile,
};
use connections::commands::{
    adapter_capability_matrix, clear_printer_connection, credential_store_info, discover_printers,
    printer_capabilities, printer_statuses, set_printer_connection, test_printer_connection,
};
use connections::supervisor::ConnectionManager;
use host_ops::commands::{
    abandon_host_operation, cancel_host_print, list_host_operations, pause_host_print,
    reconcile_host_operation, resume_host_print, stage_slice_revision, start_staged_artifact,
};
use incidents::commands::{add_incident_note, get_incident, list_incidents};
use jobs::commands::{
    assign_queue_entry, cancel_job, correct_job_material, declare_job_outcome, get_job_history,
    pause_job, release_job, resume_job, retry_job, settle_job_material, stage_job, start_job,
};
use library::commands::{
    cancel_import_selection, check_linked_sources, convert_model_to_managed, create_project,
    delete_model, delete_project, get_revision_thumbnail, import_models, inspect_import_selection,
    library_content_info, list_library, list_model_revisions, locate_linked_source,
    pick_model_files, rename_project, set_model_projects, update_model,
};
use notifications::commands::{notification_status, send_test_notification};
use printers::alerts::{get_printer_alert_defaults, set_printer_alert_defaults};
use printers::batch::{cancel_printer_batch, create_printers_batch};
use printers::commands::{
    archive_printer, create_printer, delete_printer, export_printers, import_printers,
    list_duplicate_host_archives, list_printers, printer_lifecycle_eligibility, rebind_printer,
    resolve_profile_drift, set_material_slot_layout, set_printer_override, unarchive_printer,
    update_printer,
};
use printers::create::probe_connection;
use queue::commands::{
    add_to_queue, explain_queue_entry, list_queue, move_queue_entry, remove_queue_entry,
    update_queue_entry,
};
use settings::commands::{export_settings, import_settings, load_settings, save_settings};
use slicing::commands::{
    cancel_slice_operation, check_slicer_runtime, create_external_slice_revision,
    create_preparation, delete_preparation, delete_slice_revision, get_revision_geometry,
    get_revision_mesh, get_slice_operation_log, get_slice_revision, get_slice_revision_log,
    get_slicer_runtime, list_slice_options, list_slice_revisions, list_slicing, pick_preset_source,
    pick_slicer_engine, reload_preparation, reset_slicer_runtime, start_slice, update_preparation,
};
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
    /// P4: the content store, Library stream, selections, and picker.
    pub library: Arc<library::LibraryServices<R>>,
    /// P5: the slicer runtime, the scheduler, and the `slicing` stream.
    pub slicing: Arc<slicing::SlicingServices<R>>,
    /// P6: Host Operations (executor, reconciler, `hostOperations` stream).
    pub host_ops: Arc<host_ops::HostOperationServices<R>>,
    /// P7: the `queue` event stream's id and sequence.
    pub queue_stream: queue::events::QueueStream,
    /// P7 D7: the dispatch driver's state and timings.
    pub jobs: Arc<jobs::JobServices<R>>,
    /// P7 D6: the automatic evaluator's trigger channel and last run.
    pub evaluator: Arc<queue::evaluator::Evaluator>,
    /// P8 D2: the Attention projector's runtime and the `attention` stream.
    pub attention: Arc<attention::services::AttentionServices<R>>,
    /// P8 D4/D5: camera health, one fetch per Printer, the last preview
    /// frame, the capture runtime, and the `MediaJanitor`.
    pub cameras: Arc<cameras::services::CameraServices<R>>,
    /// P8 D6: the notify policy, the platform sink, focus, and click
    /// activation.
    pub notifications: Arc<notifications::services::NotificationService<R>>,
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
        let content = Arc::new(
            library::content::ContentStore::open(storage.paths().content_root())
                .expect("content store"),
        );
        let slicing = Arc::new(slicing::SlicingServices::new(
            Arc::clone(&storage),
            Arc::clone(&content),
            Arc::clone(&catalog),
            Arc::new(slicing::runtime::FixedSlicerRuntimeFileIo::default()),
            storage.paths().metadata_root().join("slicer-cache"),
        ));
        let host_ops = Arc::new(host_ops::HostOperationServices::production(
            Arc::clone(&storage),
            Arc::clone(&content),
            Arc::clone(&manager),
        ));
        Self {
            library: Arc::new(library::LibraryServices::new(
                content,
                Arc::new(library::selection::CancelledModelFileIo),
            )),
            slicing,
            host_ops,
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
            queue_stream: queue::events::QueueStream::default(),
            jobs: Arc::new(jobs::JobServices::new(jobs::JobTimings::default())),
            evaluator: Arc::default(),
            attention: Arc::default(),
            cameras: Arc::default(),
            notifications: Arc::default(),
            _lease: None,
        }
    }
}

pub const COMMAND_NAMES: [&str; 129] = [
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
    "pick_model_files",
    "inspect_import_selection",
    "cancel_import_selection",
    "import_models",
    "list_library",
    "create_project",
    "rename_project",
    "delete_project",
    "update_model",
    "set_model_projects",
    "delete_model",
    "list_model_revisions",
    "get_revision_thumbnail",
    "library_content_info",
    "check_linked_sources",
    "locate_linked_source",
    "convert_model_to_managed",
    "get_slicer_runtime",
    "check_slicer_runtime",
    "pick_slicer_engine",
    "pick_preset_source",
    "reset_slicer_runtime",
    "list_slice_options",
    "get_revision_geometry",
    "get_revision_mesh",
    "list_slicing",
    "create_preparation",
    "update_preparation",
    "reload_preparation",
    "delete_preparation",
    "start_slice",
    "cancel_slice_operation",
    "get_slice_operation_log",
    "list_slice_revisions",
    "get_slice_revision",
    "get_slice_revision_log",
    "create_external_slice_revision",
    "delete_slice_revision",
    "printer_capabilities",
    "adapter_capability_matrix",
    "list_host_operations",
    "stage_slice_revision",
    "start_staged_artifact",
    "pause_host_print",
    "resume_host_print",
    "cancel_host_print",
    "reconcile_host_operation",
    "abandon_host_operation",
    "list_queue",
    "add_to_queue",
    "update_queue_entry",
    "move_queue_entry",
    "remove_queue_entry",
    "explain_queue_entry",
    "assign_queue_entry",
    "release_job",
    "retry_job",
    "cancel_job",
    "get_job_history",
    "stage_job",
    "start_job",
    "pause_job",
    "resume_job",
    "declare_job_outcome",
    "settle_job_material",
    "correct_job_material",
    "list_attention",
    "mark_attention_read",
    "acknowledge_attention_event",
    "resolve_attention_event",
    "list_incidents",
    "get_incident",
    "add_incident_note",
    "get_printer_camera",
    "set_printer_camera",
    "clear_printer_camera",
    "list_host_webcams",
    "test_camera",
    "camera_preview_frame",
    "capture_snapshot",
    "list_snapshots",
    "snapshot_image",
    "set_snapshot_pinned",
    "media_usage",
    "get_printer_alert_defaults",
    "set_printer_alert_defaults",
    "notification_status",
    "send_test_notification",
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

/// P4 D15: starts the Library's link supervisor for `services` and, in the
/// background, its startup pass over every linked Model. Never blocks.
/// `build_runtime_services` calls it on every successful start, including a
/// bootstrap retry; tests call it the same way. A second call for the same
/// services does nothing.
pub fn start_library_runtime<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &tauri::AppHandle<R>,
    policy: library::links::WatchPolicy,
) {
    let supervisor = library::links::LinkSupervisor::start(
        Arc::downgrade(&services.library),
        Arc::clone(&services.storage),
        app.clone(),
        policy,
    );
    if services.library.links.set(Arc::clone(&supervisor)).is_err() {
        supervisor.shutdown();
        return;
    }
    tauri::async_runtime::spawn(async move { supervisor.reconcile_all().await });
}

/// P5: attaches the slicing services to `app` and has them follow the
/// Library (a linked source's new revision makes a Preparation stale, D5).
/// `build_runtime_services` calls it on every successful start; tests call
/// it the same way. A second call for the same services does nothing.
pub fn start_slicing_runtime<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &tauri::AppHandle<R>,
) {
    services.slicing.start(app);
}

/// P6: starts the Host Operation services for `services` — the Online
/// hook, the host facts of Printers already Online, and one reconcile
/// attempt per `uncertain` row (D5 "When reconciliation runs"). Startup
/// recovery (`host_ops::recover_after_restart`) must already have run.
/// `build_runtime_services` calls it after `restore_persisted_connections`;
/// tests call it the same way. A second call does nothing.
pub fn start_host_ops_runtime<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &tauri::AppHandle<R>,
) {
    services.host_ops.start(app, &services.credentials);
}

/// P7 D7: starts the dispatch driver for `services` — its first pass
/// stages every Job a restart left unstaged, then it follows the host-ops
/// change broadcast, Printer status, and inventory changes. Then P7 D6's
/// automatic evaluator, whose first run waits for the driver's first pass
/// and the host-ops startup pass (D4 "Recovery order"); it starts with the
/// driver because every Job it assigns needs the driver to stage it.
/// Startup recovery (`jobs::recover_after_restart`) and
/// `start_host_ops_runtime` must already have run. `build_runtime_services`
/// calls it; tests call it the same way. A second call does nothing.
pub fn start_jobs_runtime<R: tauri::Runtime>(
    services: &Arc<RuntimeServices<R>>,
    app: &tauri::AppHandle<R>,
) {
    jobs::services::start(services, app);
    queue::evaluator::start(services);
}

/// P8 D2 "Runtime and wakes": starts the Attention projector for
/// `services`. It records when the supervisors started, subscribes to the
/// status, queue, and inventory broadcasts, then spawns the projector,
/// whose first action is a full pass; it publishes the startup backfill's
/// changes first. The startup backfill (`attention::projector::backfill`)
/// must already have run. `build_runtime_services` calls it after
/// `start_jobs_runtime`; tests call it the same way. A second call does
/// nothing.
pub fn start_attention_runtime<R: tauri::Runtime>(
    services: &Arc<RuntimeServices<R>>,
    app: &tauri::AppHandle<R>,
) {
    attention::services::start(services, app);
}

/// P8 D4/D5: starts the camera capture runtime for `services` — it
/// publishes the startup sweep's changes, subscribes to the Attention
/// projector's committed passes (capture intents run on their own tasks,
/// never inside a pass), and starts the `MediaJanitor`, whose first prune
/// pass runs at once. It must start **before** `start_attention_runtime`,
/// so the projector's first pass is heard. The startup sweep
/// (`cameras::media::startup_sweep`) must already have run and its outcome
/// been handed to `CameraServices::apply_startup_sweep`.
/// `build_runtime_services` calls it; tests call it the same way. A second
/// call does nothing.
pub fn start_camera_runtime<R: tauri::Runtime>(
    services: &Arc<RuntimeServices<R>>,
    app: &tauri::AppHandle<R>,
) {
    cameras::capture::start(services, app);
}

/// P8 D6: starts the notification runtime for `services` — it subscribes
/// to the Attention projector's committed passes, listens to the sink's
/// signals, and connects the sink in the background. It must start
/// **before** `start_attention_runtime`, so the projector's first live
/// pass is heard. `build_runtime_services` calls it; tests call it the
/// same way. A second call does nothing.
pub fn start_notification_runtime<R: tauri::Runtime>(
    services: &Arc<RuntimeServices<R>>,
    app: &tauri::AppHandle<R>,
) {
    notifications::services::start(services, app);
}

/// D6's `app_icon`: the installed `farm3d` icon name, else the bundled
/// icon's absolute path (`just dev`, AppImage), else none.
fn notification_icon<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> String {
    #[cfg(target_os = "linux")]
    {
        let bundled = app
            .path()
            .resolve("resources/farm3d-notification.png", BaseDirectory::Resource)
            .ok();
        notifications::dbus::resolve_icon(&notifications::dbus::xdg_data_dirs(), bundled.as_deref())
    }
    #[cfg(not(target_os = "linux"))]
    {
        let _ = app;
        String::new()
    }
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
    f3d_log!(error, "startup.failed", error = error);
    match startup_command_error(error) {
        Ok(error) => StartupFailure::Recoverable(error),
        Err(()) => StartupFailure::Fatal,
    }
}

fn build_runtime_services<R: tauri::Runtime>(
    app: &tauri::AppHandle<R>,
    retained_lease: &Arc<std::sync::Mutex<Option<persistence::MetadataRootLease>>>,
) -> Result<Arc<RuntimeServices<R>>, StartupFailure> {
    let metadata_root = app
        .path()
        .app_config_dir()
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    let data_root = app
        .path()
        .app_data_dir()
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    let log_root = app
        .path()
        .app_log_dir()
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    let paths = persistence::StoragePaths::new(metadata_root, data_root)
        .and_then(|paths| paths.with_log_root(log_root))
        .map_err(startup_error)?;
    // P9 D12: the log opens before `Storage::open`, so a failed open is on
    // record. (Task 7's installer runs before this line.)
    diagnostics::log::init(paths.log_root());
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
    // P4 D4: reconcile the content store before any command is served.
    let content = Arc::new(
        library::content::ContentStore::open(storage.paths().content_root())
            .map_err(startup_error)?,
    );
    content.startup_sweep(&storage).map_err(startup_error)?;
    // P5 D10: interrupt what a previous run left queued or running, and
    // remove every work directory, before any command is served.
    slicing::operations::recover_after_restart(&storage).map_err(startup_error)?;
    // Recovery pruned old operation logs; unlink them now. A blob that
    // can't be unlinked is retried by the next startup sweep.
    let _ = content.release_unreferenced(&storage);
    // P6 D3: a `dispatching` row never sent fails `neverSent`; one that may
    // have been sent becomes `uncertain`; a `reconciling` row goes back to
    // `uncertain`. Before any command is served or any attempt runs.
    host_ops::recover_after_restart(&storage, chrono::Utc::now()).map_err(startup_error)?;
    // P7 D4: a Job whose linked Host Operation moved while farm3d was
    // closed catches up with it, before any command is served.
    let recovered_jobs =
        jobs::recover_after_restart(&storage, chrono::Utc::now()).map_err(startup_error)?;
    // P8 D2 "Startup backfill": project every durable Condition with live
    // status unknown, before any command is served. Published once the
    // attention runtime starts.
    let backfilled = attention::projector::backfill(&storage, chrono::Utc::now()).map_err(
        |error| match error {
            persistence::RepositoryError::Storage(error) => startup_error(error),
            _ => StartupFailure::Recoverable(contracts::command::CommandError::internal()),
        },
    )?;
    // P8 D5 "Startup sweep": repair whatever a crash left in the media
    // store, before any command is served. Published once the camera
    // runtime starts. A failure never blocks startup (the camera is
    // optional): evidence capture is off until a later sweep succeeds.
    let swept = cameras::media::startup_sweep(&storage, chrono::Utc::now());

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
    let slicer_cache = app
        .path()
        .app_cache_dir()
        .map_err(|_| StartupFailure::Recoverable(contracts::command::CommandError::internal()))?;
    let slicing = Arc::new(slicing::SlicingServices::new(
        Arc::clone(&storage),
        Arc::clone(&content),
        Arc::clone(&catalog),
        Arc::new(slicing::runtime::NativeSlicerRuntimeFileIo::new(
            app.clone(),
        )),
        slicer_cache,
    ));
    let host_ops = Arc::new(host_ops::HostOperationServices::production(
        Arc::clone(&storage),
        Arc::clone(&content),
        Arc::clone(&manager),
    ));
    let jobs = Arc::new(jobs::JobServices::new(jobs::JobTimings::default()));
    jobs.set_recovered(recovered_jobs);
    let attention = Arc::new(attention::services::AttentionServices::new(
        attention::services::AttentionTimings::default(),
    ));
    attention.set_backfilled(backfilled);
    let services = Arc::new(RuntimeServices {
        storage: Arc::clone(&storage),
        catalog,
        manager,
        documents: Arc::new(document_io::NativeDocumentIo::new(app.clone())),
        credentials: credential_store,
        inventory_stream: spools::events::InventoryStream::default(),
        inventory_changes: inventory_changes(),
        library: Arc::new(library::LibraryServices::new(
            content,
            Arc::new(library::selection::NativeModelFileIo::new(app.clone())),
        )),
        slicing,
        host_ops,
        queue_stream: queue::events::QueueStream::default(),
        jobs,
        evaluator: Arc::default(),
        attention,
        cameras: Arc::new(cameras::services::CameraServices::new(
            cameras::services::CameraTimings::default(),
        )),
        notifications: Arc::new(notifications::services::NotificationService::platform(
            app.try_state::<notifications::focus::Focus>()
                .map(|focus| focus.inner().clone())
                .unwrap_or_default(),
            notification_icon(app),
        )),
        _lease: Some(RuntimeServicesLease::new(Arc::clone(&storage), lease)),
    });
    start_library_runtime(&services, app, library::links::WatchPolicy::native());
    start_slicing_runtime(&services, app);
    // P6: after `restore_persisted_connections`, the first reconcile pass.
    start_host_ops_runtime(&services, app);
    // P7 D4: then the dispatch driver.
    start_jobs_runtime(&services, app);
    // P8 D4/D5: the capture runtime and the janitor, subscribed before the
    // projector's first pass.
    services.cameras.apply_startup_sweep(swept);
    start_camera_runtime(&services, app);
    // P8 D6: the notification runtime, also subscribed before the
    // projector's first pass.
    start_notification_runtime(&services, app);
    // P8 D2: then the Attention projector.
    start_attention_runtime(&services, app);
    // D2: the startup probe runs in the background; `get_slicer_runtime`
    // meanwhile waits on the same probe rather than starting another.
    services.slicing.probe_in_background();
    Ok(services)
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

/// P4 D7: a file drop on the window becomes an import selection. Runs off
/// the event loop; a drop before bootstrap is ready is ignored.
fn handle_window_drop<R: tauri::Runtime>(app: tauri::AppHandle<R>, paths: Vec<std::path::PathBuf>) {
    tauri::async_runtime::spawn_blocking(move || {
        let Some(bootstrap) = app.try_state::<bootstrap::BootstrapState<RuntimeServices<R>>>()
        else {
            return;
        };
        if let Ok(services) = bootstrap.ready() {
            library::selection::handle_drop(&app, &services.library, paths);
        }
    });
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_dialog::init())
        // P8 D6 "Focus": the main window's focus, from its
        // `WindowEvent::Focused` after the one seed in `setup`.
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::Focused(focused) = event {
                if window.label() == notifications::activation::MAIN_WINDOW {
                    if let Some(focus) = window.try_state::<notifications::focus::Focus>() {
                        focus.set(*focused);
                    }
                }
            }
        })
        .on_webview_event(|webview, event| {
            if let tauri::WebviewEvent::DragDrop(tauri::DragDropEvent::Drop { paths, .. }) = event
            {
                handle_window_drop(webview.app_handle().clone(), paths.clone());
            }
        })
        .setup(|app| {
            // Before the services, so a bootstrap retry keeps the same
            // focus. Tauri builds (and shows) the config's main window
            // before `setup`, so seed from its `is_focused()` now, once
            // (decision 41); `false` if it errors or there is no window.
            let focus = notifications::focus::Focus::default();
            focus.seed(
                app.get_webview_window(notifications::activation::MAIN_WINDOW)
                    .ok_or(())
                    .and_then(|window| window.is_focused().map_err(|_| ())),
            );
            app.manage(focus);
            let handle = app.handle().clone();
            let retained_lease = Arc::new(std::sync::Mutex::new(None));
            match build_runtime_services(&handle, &retained_lease) {
                Ok(services) => {
                    app.manage(bootstrap::BootstrapState::ready_with(services));
                    Ok(())
                }
                Err(StartupFailure::Recoverable(error)) => {
                    let retry_handle = handle.clone();
                    let retry_lease = Arc::clone(&retained_lease);
                    app.manage(bootstrap::BootstrapState::failed(error, move || {
                        build_runtime_services(&retry_handle, &retry_lease)
                            .map_err(|failure| match failure {
                                StartupFailure::Recoverable(error) => error,
                                StartupFailure::Fatal => contracts::command::CommandError::internal(),
                            })
                    }));
                    Ok(())
                }
                Err(StartupFailure::Fatal) => {
                    // The line `startup_error` logged must be on disk before
                    // setup returns its error and the app exits.
                    diagnostics::log::shutdown();
                    Err(std::io::Error::other(
                    "Unsupported metadata locking. This platform or data location cannot run farm3d; contact support with the platform and filesystem type.",
                    )
                    .into())
                }
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
            pick_model_files,
            inspect_import_selection,
            cancel_import_selection,
            import_models,
            list_library,
            create_project,
            rename_project,
            delete_project,
            update_model,
            set_model_projects,
            delete_model,
            list_model_revisions,
            get_revision_thumbnail,
            library_content_info,
            check_linked_sources,
            locate_linked_source,
            convert_model_to_managed,
            get_slicer_runtime,
            check_slicer_runtime,
            pick_slicer_engine,
            pick_preset_source,
            reset_slicer_runtime,
            list_slice_options,
            get_revision_geometry,
            get_revision_mesh,
            list_slicing,
            create_preparation,
            update_preparation,
            reload_preparation,
            delete_preparation,
            start_slice,
            cancel_slice_operation,
            get_slice_operation_log,
            list_slice_revisions,
            get_slice_revision,
            get_slice_revision_log,
            create_external_slice_revision,
            delete_slice_revision,
            printer_capabilities,
            adapter_capability_matrix,
            list_host_operations,
            stage_slice_revision,
            start_staged_artifact,
            pause_host_print,
            resume_host_print,
            cancel_host_print,
            reconcile_host_operation,
            abandon_host_operation,
            list_queue,
            add_to_queue,
            update_queue_entry,
            move_queue_entry,
            remove_queue_entry,
            explain_queue_entry,
            assign_queue_entry,
            release_job,
            retry_job,
            cancel_job,
            get_job_history,
            stage_job,
            start_job,
            pause_job,
            resume_job,
            declare_job_outcome,
            settle_job_material,
            correct_job_material,
            list_attention,
            mark_attention_read,
            acknowledge_attention_event,
            resolve_attention_event,
            list_incidents,
            get_incident,
            add_incident_note,
            get_printer_camera,
            set_printer_camera,
            clear_printer_camera,
            list_host_webcams,
            test_camera,
            camera_preview_frame,
            capture_snapshot,
            list_snapshots,
            snapshot_image,
            set_snapshot_pinned,
            media_usage,
            get_printer_alert_defaults,
            set_printer_alert_defaults,
            notification_status,
            send_test_notification,
            #[cfg(debug_assertions)]
            spools::commands::debug_seed_reservation,
        ])
        .build(tauri::generate_context!())
        .expect("error while building tauri application")
        .run(|_app, event| {
            // P9 D12: queued log lines reach disk before the process ends.
            if matches!(event, tauri::RunEvent::Exit) {
                diagnostics::log::shutdown();
            }
        });
}
