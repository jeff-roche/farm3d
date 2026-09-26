//! Fixtures shared by the integration tests (`p2_contract_path.rs`,
//! `p2_batch.rs`, `p4_import.rs`, and others): a leased temp `Storage`, a
//! one-variant catalog, a fixed-outcome fake `PrinterConnection`, a fake
//! Library file picker, and a mock-runtime builder that wires a
//! test-controlled connection factory, credential directory, and (for the
//! Library) picker into `RuntimeServices`.
//!
//! Each test crate uses a different subset, hence the `dead_code` allow.
#![allow(dead_code)]

pub mod fake_moonraker;
pub mod octoprint;

use std::path::{Path, PathBuf};
use std::sync::Arc;

use farm3d_lib::bootstrap::BootstrapState;
use farm3d_lib::catalog::{BedShape, Catalog, CatalogModel, CatalogVariant};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::ConnectionManager;
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionError, ConnectionObservation, PrinterConnection, ProbeResult,
    ReportedCapabilities, MOONRAKER_KIND,
};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::document_io::{DocumentIo, DocumentKind};
use farm3d_lib::library::selection::{ModelFileIo, SelectionPurpose};
use farm3d_lib::library::LibraryServices;
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::{CatalogRef, StoredPrinter};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, Invoke, InvokeBody};
use tauri::test::{mock_builder, mock_context, noop_assets, MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::{Manager, WebviewWindowBuilder};

pub struct UnusedDocuments;

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

/// A `ModelFileIo` whose picker always returns `picks` (`None` is a
/// cancelled dialog), so tests never open a native dialog.
pub struct FakeModelFileIo {
    pub picks: Option<Vec<PathBuf>>,
}

impl ModelFileIo for FakeModelFileIo {
    fn pick_files(&self, _purpose: SelectionPurpose) -> Result<Option<Vec<PathBuf>>, CommandError> {
        Ok(self.picks.clone())
    }
}

/// A leased `Storage` in a fresh temp directory, plus the SQLite database
/// file's path (for byte-level secret scans of the DB and its WAL).
pub fn storage() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>, PathBuf) {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let database = paths.database().to_path_buf();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    (temp, lease, storage, database)
}

pub fn pending_cleanup_count(storage: &Storage) -> i64 {
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

/// One model, one variant: a 256 x 256 x 256 mm rectangular bed whose
/// catalog `defaultBedType` is `"4"`.
pub fn a_catalog() -> Catalog {
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

pub fn a_ref() -> CatalogRef {
    CatalogRef {
        vendor: "TestVendor".to_string(),
        model: "Test Printer".to_string(),
        variant: "Test Printer 0.4 nozzle".to_string(),
        model_id: "TestVendor-TP".to_string(),
        printer_variant: "0.4".to_string(),
    }
}

pub fn a_ref_json() -> Value {
    json!({
        "vendor": "TestVendor",
        "model": "Test Printer",
        "variant": "Test Printer 0.4 nozzle",
        "modelId": "TestVendor-TP",
        "printerVariant": "0.4",
    })
}

pub fn a_stored_printer(id: &str) -> StoredPrinter {
    StoredPrinter {
        id: id.to_string(),
        name: "Test Printer".to_string(),
        catalog_ref: a_ref(),
        ..Default::default()
    }
}

/// The fixed probe outcome for a fixture host: `ok.local` is ready,
/// `auth.local` rejects the credential, `slow.local` times out, and
/// everything else is unreachable.
pub fn fixture_probe(host: &str) -> Result<ProbeResult, ConnectionError> {
    match host {
        "ok.local" => Ok(ready_probe(ReportedCapabilities::default())),
        "auth.local" => Err(ConnectionError::Auth("bad credential".to_string())),
        "slow.local" => Err(ConnectionError::Timeout),
        other => Err(ConnectionError::Unreachable(other.to_string())),
    }
}

pub fn ready_probe(reported: ReportedCapabilities) -> ProbeResult {
    ProbeResult {
        kind: MOONRAKER_KIND.to_string(),
        host_software: "Moonraker".to_string(),
        firmware: "Klipper".to_string(),
        reported_name: "Fixture Printer".to_string(),
        state: "ready".to_string(),
        state_message: String::new(),
        reported,
    }
}

/// A `PrinterConnection` whose `probe()` outcome is [`fixture_probe`] and
/// whose `subscribe()` pends forever rather than looping — real supervision
/// start calls `subscribe`, not `probe`, so this keeps the supervisor's
/// reconnect loop from spinning against the fake.
pub struct FakeConnection {
    pub host: String,
}

#[async_trait::async_trait]
impl PrinterConnection for FakeConnection {
    async fn probe(&self) -> Result<ProbeResult, ConnectionError> {
        fixture_probe(&self.host)
    }

    async fn subscribe(
        &self,
        _tx: tokio::sync::mpsc::Sender<ConnectionObservation>,
    ) -> Result<(), ConnectionError> {
        std::future::pending::<()>().await;
        Ok(())
    }
}

pub fn invoke(
    webview: &tauri::WebviewWindow<MockRuntime>,
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

/// A mock app with `handler` registered, a `ConnectionManager` built on
/// `factory`, and a credential store pointed at a directory the test
/// controls (`RuntimeServices::for_test` otherwise mints its own,
/// unreachable, temp directory) so the store's contents are checkable.
pub fn runtime(
    handler: impl Fn(Invoke<MockRuntime>) -> bool + Send + Sync + 'static,
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
    tauri::App<MockRuntime>,
    tauri::WebviewWindow<MockRuntime>,
    Arc<ConnectionManager<MockRuntime>>,
    Arc<RuntimeServices<MockRuntime>>,
) {
    runtime_customized(handler, storage, catalog, credentials_dir, factory, |_| {})
}

/// [`runtime`], with the Library's native picker replaced by `file_io`.
pub fn runtime_with_file_io(
    handler: impl Fn(Invoke<MockRuntime>) -> bool + Send + Sync + 'static,
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
    file_io: Arc<dyn ModelFileIo>,
) -> (
    tauri::App<MockRuntime>,
    tauri::WebviewWindow<MockRuntime>,
    Arc<ConnectionManager<MockRuntime>>,
    Arc<RuntimeServices<MockRuntime>>,
) {
    runtime_customized(
        handler,
        storage,
        catalog,
        credentials_dir,
        factory,
        move |services| {
            services.library = Arc::new(LibraryServices::new(
                Arc::clone(&services.library.content),
                file_io,
            ));
        },
    )
}

/// What the mock-runtime builders return: the app, its main webview, the
/// `ConnectionManager`, and the managed `RuntimeServices`.
pub type MockHarness = (
    tauri::App<MockRuntime>,
    tauri::WebviewWindow<MockRuntime>,
    Arc<ConnectionManager<MockRuntime>>,
    Arc<RuntimeServices<MockRuntime>>,
);

/// [`runtime`], with the Printers/Settings document dialogs replaced by
/// `documents` (for `import_printers`/`export_printers`).
pub fn runtime_with_documents(
    handler: impl Fn(Invoke<MockRuntime>) -> bool + Send + Sync + 'static,
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
    documents: Arc<dyn DocumentIo>,
) -> MockHarness {
    runtime_customized(
        handler,
        storage,
        catalog,
        credentials_dir,
        factory,
        move |services| services.documents = documents,
    )
}

fn runtime_customized(
    handler: impl Fn(Invoke<MockRuntime>) -> bool + Send + Sync + 'static,
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
    customize: impl FnOnce(&mut RuntimeServices<MockRuntime>),
) -> (
    tauri::App<MockRuntime>,
    tauri::WebviewWindow<MockRuntime>,
    Arc<ConnectionManager<MockRuntime>>,
    Arc<RuntimeServices<MockRuntime>>,
) {
    let app = mock_builder()
        .invoke_handler(handler)
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
    customize(&mut services);
    let services = Arc::new(services);
    app.manage(BootstrapState::ready_with(Arc::clone(&services)));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview, manager, services)
}

/// A snapshot of a credential store directory's presence/contents, cheap
/// enough to compare before/after a call that must not touch it.
pub fn credentials_dir_snapshot(dir: &Path) -> Option<Vec<u8>> {
    let path = farm3d_lib::connections::credentials::credentials_file_path(dir);
    std::fs::read(path).ok()
}
