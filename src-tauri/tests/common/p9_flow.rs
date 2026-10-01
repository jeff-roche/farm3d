//! The P9 tracer's shared rig (Task 18): one command rig over any Storage,
//! fake dialogs, and the "restart" (drop every handle, run the real startup
//! path `open_storage`). Used by `p9_tracer.rs` (the fakes) and the P9
//! section of `sim_moonraker.rs` (the simulator).
//!
//! Include with `#[path = "common/p9_flow.rs"] mod p9_flow;` next to
//! `mod common;`, `mod p9_farm;`, and `mod secrets;`.
#![allow(dead_code)]

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::test::MockRuntime;

use crate::common;
use crate::p9_farm::{ids, Farm};
use farm3d_lib::backup::dialogs::PortabilityDialogs;
use farm3d_lib::backup::installer::InstallReport;
use farm3d_lib::backup::restart::RecordingRestarter;
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::persistence::integrity::{self, IntegrityRoots};
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::RuntimeServices;

/// Dialogs whose answers the test sets.
#[derive(Default)]
pub struct FakeDialogs {
    pub open: Mutex<Option<PathBuf>>,
    pub save_backup: Mutex<Option<PathBuf>>,
    pub save_diagnostics: Mutex<Option<PathBuf>>,
}

impl PortabilityDialogs for FakeDialogs {
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.open.lock().unwrap().clone())
    }
    fn save_backup(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.save_backup.lock().unwrap().clone())
    }
    fn save_diagnostics(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.save_diagnostics.lock().unwrap().clone())
    }
}

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

pub struct Rig {
    pub app: tauri::App<MockRuntime>,
    pub webview: tauri::WebviewWindow<MockRuntime>,
    pub services: Arc<RuntimeServices<MockRuntime>>,
    pub restarter: Arc<RecordingRestarter>,
    pub dialogs: Arc<FakeDialogs>,
}

impl Rig {
    /// Every P9 command over `storage`, with the credential store kept in
    /// `credentials_dir` (an empty directory is the "new machine").
    pub fn new(storage: Arc<Storage>, credentials_dir: PathBuf) -> Rig {
        Rig::with_factory(storage, credentials_dir, no_connection)
    }

    /// [`Rig::new`] with the Connection factory the supervisor uses.
    pub fn with_factory(
        storage: Arc<Storage>,
        credentials_dir: PathBuf,
        factory: impl Fn(
                &ConnectionConfig,
                Option<zeroize::Zeroizing<String>>,
            ) -> Option<Box<dyn PrinterConnection>>
            + Send
            + Sync
            + 'static,
    ) -> Rig {
        let dialogs = Arc::new(FakeDialogs::default());
        let injected: Arc<dyn PortabilityDialogs> = dialogs.clone();
        let restarter = Arc::new(RecordingRestarter::default());
        let restart = Arc::clone(&restarter);
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::backup::commands::create_backup,
                farm3d_lib::backup::commands::preview_restore,
                farm3d_lib::backup::commands::discard_restore_preview,
                farm3d_lib::backup::commands::apply_restore,
                farm3d_lib::backup::commands::restore_status,
                farm3d_lib::backup::commands::acknowledge_restore_status,
                farm3d_lib::diagnostics::commands::export_diagnostics,
                farm3d_lib::diagnostics::commands::reset_preview,
                farm3d_lib::diagnostics::commands::reset_farm,
            ],
            storage,
            Arc::new(common::a_catalog()),
            credentials_dir,
            factory,
            move |services| {
                let backup = services
                    .backup
                    .with_dialogs(injected)
                    .with_restarter(restart);
                services.backup = Arc::new(backup);
                services.diagnostics = Arc::new(
                    farm3d_lib::diagnostics::DiagnosticsServices::default()
                        .with_home_dir(PathBuf::from(crate::secrets::HOME_PATH)),
                );
                services
                    .slicing
                    .set_discovery_env(farm3d_lib::slicing::runtime::DiscoveryEnv {
                        home: None,
                        path_var: None,
                        probe_timeout: std::time::Duration::from_millis(200),
                    });
            },
        );
        Rig {
            app,
            webview,
            services,
            restarter,
            dialogs,
        }
    }

    pub fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|envelope| envelope["data"].clone())
    }

    pub fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    }
}

/// The Farm's files as the next process finds them: a copy of both roots
/// in a directory of its own, so the old process's background tasks (which
/// keep their `Storage` handles for the life of the test process) can't
/// touch them. The database is checkpointed first, so the copy needs no
/// `-wal`. The credential file is left behind (a new machine's store is its
/// own).
pub struct Closed {
    pub temp: tempfile::TempDir,
    pub paths: StoragePaths,
}

pub fn relocate(paths: &StoragePaths) -> Closed {
    Connection::open(paths.database())
        .unwrap()
        .query_row("PRAGMA wal_checkpoint(TRUNCATE)", [], |_| Ok(()))
        .unwrap();
    let temp = tempfile::tempdir().expect("temp");
    let metadata = temp.path().join("metadata");
    let data = temp.path().join("data");
    copy_tree(paths.metadata_root(), &metadata);
    copy_tree(paths.app_data_root(), &data);
    let moved = StoragePaths::new(metadata, data).expect("paths");
    Closed { temp, paths: moved }
}

fn copy_tree(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for entry in std::fs::read_dir(from).unwrap() {
        let entry = entry.unwrap();
        let name = entry.file_name();
        let name_text = name.to_string_lossy();
        // Credentials stay behind: a new machine has none of its own.
        if name_text.ends_with("-wal")
            || name_text.ends_with("-shm")
            || name_text == "credentials.json"
        {
            continue;
        }
        let target = to.join(&name);
        if entry.file_type().unwrap().is_dir() {
            copy_tree(&entry.path(), &target);
        } else {
            std::fs::copy(entry.path(), target).unwrap();
        }
    }
}

/// The real startup path: the lease, the installer (a pending restore or
/// reset runs here), the log, then `Storage::open`. The lease is retained
/// in `slot` for the life of the returned Storage.
pub fn start(
    paths: &StoragePaths,
    slot: &Mutex<Option<MetadataRootLease>>,
    credentials_dir: &Path,
) -> (Arc<Storage>, InstallReport) {
    let credentials_dir = credentials_dir.to_path_buf();
    farm3d_lib::open_storage(paths.clone(), slot, move || {
        CredentialStore::file_backed(credentials_dir)
    })
    .map_err(|_| "startup failed")
    .expect("startup")
}

/// `integrity::check` over the database in `paths`: no violation. Returns
/// the report's counts.
pub fn assert_integrity_clean(paths: &StoragePaths) -> std::collections::BTreeMap<String, i64> {
    let connection = Connection::open(paths.database()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let report = integrity::check(&connection, Some(&IntegrityRoots::from_paths(paths))).unwrap();
    assert!(report.violations().is_empty(), "{report:?}");
    report.counts.into_iter().collect()
}

/// Runs `statements` in one write transaction on `farm`.
pub fn sql(farm: &Farm, statements: &str) {
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            tx.execute_batch(statements)
                .unwrap_or_else(|error| panic!("fixture SQL failed: {error}\n{statements}"));
            Ok(())
        })
        .expect("fixture write");
}

/// A finished Job with its own closed Queue Entry and reservation.
pub fn finished_job(suffix: &str) -> String {
    format!(
        "INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
           state, close_reason, position, policy, preference, estimate_mg, estimate_source,
           created_at, updated_at, closed_at)
         VALUES ('qen-{suffix}', 1, 'slr-a', 'qln-{suffix}', 1, 'closed', 'completed', NULL,
                 'manual', 'loadedFirst', 500000, 'operatorEntered', '{now}', '{now}', '{now}');
         INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg, state,
           operation_id, created_at)
         VALUES ('rsv-{suffix}', 'spl-a', 'job', 'job-{suffix}', 500000, 'active',
                 'rsv-{suffix}-op', '{now}');
         INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
           printer_snapshot_json, spool_id, reservation_id, estimate_mg, state, settlement,
           settlement_method, assigned_by, created_at, updated_at, ended_at)
         VALUES ('job-{suffix}', 1, 'qen-{suffix}', 'slr-a', 'prn-a', '{{}}', 'spl-a',
                 'rsv-{suffix}', 500000, 'completed', 'settled', 'estimated', 'operator',
                 '{now}', '{now}', '{now}');",
        now = ids::NOW
    )
}
