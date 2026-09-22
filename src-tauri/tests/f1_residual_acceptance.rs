use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use farm3d_lib::bootstrap::BootstrapState;
use farm3d_lib::connections::commands::{
    coordinate_connection_change, retry_pending_credential_cleanup, CredentialSubmission,
};
use farm3d_lib::connections::credentials::CredentialBackend;
use farm3d_lib::connections::ConnectionConfig;
use farm3d_lib::contracts::command::{CommandError, ErrorCode};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{CatalogRef, StoredPrinter};

#[derive(Default)]
struct InjectedCredentialStore {
    values: std::sync::Mutex<std::collections::BTreeMap<String, String>>,
    fail_set: std::sync::atomic::AtomicBool,
    fail_delete: std::sync::atomic::AtomicBool,
}

impl CredentialBackend for InjectedCredentialStore {
    fn set(&self, key: &str, secret: &str) -> Result<(), ()> {
        if self.fail_set.load(Ordering::SeqCst) {
            return Err(());
        }
        self.values
            .lock()
            .unwrap()
            .insert(key.to_string(), secret.to_string());
        Ok(())
    }

    fn get(&self, key: &str) -> Result<Option<String>, ()> {
        Ok(self.values.lock().unwrap().get(key).cloned())
    }

    fn delete(&self, key: &str) -> Result<(), ()> {
        if self.fail_delete.load(Ordering::SeqCst) {
            return Err(());
        }
        self.values.lock().unwrap().remove(key);
        Ok(())
    }
}

fn credential_fixture() -> (
    tempfile::TempDir,
    MetadataRootLease,
    Arc<Storage>,
    StoredPrinter,
) {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    let printer = PrinterRepository::new(Arc::clone(&storage))
        .create(StoredPrinter {
            id: "prn-credential-matrix".to_string(),
            name: "Credential matrix".to_string(),
            catalog_ref: CatalogRef {
                vendor: "Vendor".to_string(),
                model: "Model".to_string(),
                variant: "Variant".to_string(),
                model_id: "model".to_string(),
                printer_variant: "0.4".to_string(),
            },
            ..Default::default()
        })
        .unwrap();
    (temp, lease, storage, printer)
}

fn connection() -> ConnectionConfig {
    ConnectionConfig {
        kind: "moonraker".to_string(),
        host: "printer.invalid".to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: None,
    }
}

#[test]
fn retryable_bootstrap_failure_retries_once_and_recovers_waiting_commands() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&attempts);
    let bootstrap = Arc::new(BootstrapState::failed(
        CommandError::persistence_unavailable(),
        move || {
            observed.fetch_add(1, Ordering::SeqCst);
            Ok(Arc::new("ready".to_string()))
        },
    ));

    let barrier = Arc::new(std::sync::Barrier::new(9));
    let workers = (0..8)
        .map(|_| {
            let bootstrap = Arc::clone(&bootstrap);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                bootstrap.ready().map(|value| value.as_str().to_string())
            })
        })
        .collect::<Vec<_>>();
    barrier.wait();

    for worker in workers {
        assert_eq!(worker.join().unwrap().unwrap(), "ready");
    }
    assert_eq!(attempts.load(Ordering::SeqCst), 1);
    assert!(bootstrap.is_ready());
}

#[test]
fn non_retryable_bootstrap_failure_is_stable_and_never_retries() {
    let attempts = Arc::new(AtomicUsize::new(0));
    let observed = Arc::clone(&attempts);
    let expected = CommandError::migration_failed();
    let bootstrap = BootstrapState::<String>::failed(expected.clone(), move || {
        observed.fetch_add(1, Ordering::SeqCst);
        Ok(Arc::new("unexpected".to_string()))
    });

    assert_eq!(bootstrap.ready().unwrap_err(), expected);
    assert_eq!(bootstrap.ready().unwrap_err(), expected);
    assert_eq!(attempts.load(Ordering::SeqCst), 0);
    assert_eq!(
        bootstrap.failure().unwrap().code,
        ErrorCode::MigrationFailed
    );
}

#[test]
fn starting_bootstrap_returns_a_safe_retryable_error_without_domain_work() {
    let bootstrap = BootstrapState::<String>::starting();

    let error = bootstrap.ready().unwrap_err();

    assert_eq!(error.code, ErrorCode::PersistenceUnavailable);
    assert!(error.retryable);
}

#[test]
fn fallback_store_removes_interrupted_replacements_without_touching_live_data() {
    let temp = tempfile::tempdir().unwrap();
    let store = farm3d_lib::connections::credentials::CredentialStore::file_backed(
        temp.path().to_path_buf(),
    );
    store.set("existing", "kept").unwrap();
    std::fs::write(
        temp.path().join(".credentials.json.interrupted.tmp"),
        br#"{"existing":"lost","new":"partial"}"#,
    )
    .unwrap();

    let reopened = farm3d_lib::connections::credentials::CredentialStore::file_backed(
        temp.path().to_path_buf(),
    );

    assert_eq!(reopened.get("existing").unwrap().as_deref(), Some("kept"));
    assert!(!temp
        .path()
        .join(".credentials.json.interrupted.tmp")
        .exists());
}

#[test]
fn secret_write_failure_leaves_printer_unchanged_and_provisional_cleanup_durable() {
    let (_temp, _lease, storage, printer) = credential_fixture();
    let store = InjectedCredentialStore::default();
    store.fail_set.store(true, Ordering::SeqCst);

    let error = coordinate_connection_change(
        &storage,
        &store,
        &printer.id,
        printer.revision,
        connection(),
        CredentialSubmission::Replace("F1_SECRET_WRITE_SENTINEL"),
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::CredentialUnavailable);
    let unchanged = PrinterRepository::new(Arc::clone(&storage))
        .get(&printer.id)
        .unwrap()
        .unwrap();
    assert_eq!(unchanged.revision, printer.revision);
    assert!(unchanged.connection.is_none());
    let pending = storage
        .read(|db| {
            db.query_row(
                "SELECT COUNT(*) FROM pending_credential_cleanup WHERE reason='provisional'",
                [],
                |row| row.get::<_, i64>(0),
            )
        })
        .unwrap();
    assert_eq!(pending, 1);
}

#[test]
fn database_failure_after_secret_write_keeps_new_secret_queued_for_cleanup() {
    let (_temp, _lease, storage, printer) = credential_fixture();
    let store = InjectedCredentialStore::default();
    storage.write(|transaction| {
        transaction.execute_batch("CREATE TRIGGER reject_connection_update BEFORE UPDATE ON printers BEGIN SELECT RAISE(FAIL, 'injected'); END;")?;
        Ok(())
    }).unwrap();

    let error = coordinate_connection_change(
        &storage,
        &store,
        &printer.id,
        printer.revision,
        connection(),
        CredentialSubmission::Replace("F1_DB_FAILURE_SENTINEL"),
    )
    .unwrap_err();

    assert_eq!(error.code, ErrorCode::PersistenceUnavailable);
    let (reference, reason): (String, String) = storage
        .read(|db| {
            db.query_row(
                "SELECT credential_ref, reason FROM pending_credential_cleanup",
                [],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
        })
        .unwrap();
    assert_eq!(reason, "provisional");
    assert_eq!(
        store.get(&reference).unwrap().as_deref(),
        Some("F1_DB_FAILURE_SENTINEL")
    );
}

#[test]
fn cleanup_failure_is_counted_and_a_later_startup_retry_removes_the_orphan() {
    let (_temp, _lease, storage, printer) = credential_fixture();
    let store = InjectedCredentialStore::default();
    let committed = coordinate_connection_change(
        &storage,
        &store,
        &printer.id,
        printer.revision,
        connection(),
        CredentialSubmission::Replace("F1_CLEANUP_SENTINEL"),
    )
    .unwrap();
    let reference = committed
        .printer
        .connection
        .unwrap()
        .credential_ref
        .unwrap();
    let cleared = PrinterRepository::new(Arc::clone(&storage))
        .set_connection(
            &printer.id,
            committed.printer.revision,
            None,
            None,
            "cleared",
        )
        .unwrap();
    assert_eq!(cleared.revision, 3);
    store.fail_delete.store(true, Ordering::SeqCst);

    assert!(retry_pending_credential_cleanup(&storage, &store).is_err());
    let attempts = storage
        .read(|db| {
            db.query_row(
                "SELECT attempt_count FROM pending_credential_cleanup WHERE credential_ref=?1",
                [&reference],
                |row| row.get::<_, i64>(0),
            )
        })
        .unwrap();
    assert_eq!(attempts, 1);

    store.fail_delete.store(false, Ordering::SeqCst);
    retry_pending_credential_cleanup(&storage, &store).unwrap();
    assert_eq!(store.get(&reference).unwrap(), None);
    let remaining = storage
        .read(|db| {
            db.query_row(
                "SELECT COUNT(*) FROM pending_credential_cleanup WHERE credential_ref=?1",
                [&reference],
                |row| row.get::<_, i64>(0),
            )
        })
        .unwrap();
    assert_eq!(remaining, 0);
}

#[test]
fn every_registered_command_has_generated_request_and_result_contracts() {
    let manifest = farm3d_lib::contracts::inventory::command_contract_inventory();
    assert_eq!(manifest.len(), 27);
    assert_eq!(
        manifest
            .iter()
            .map(|entry| entry.command)
            .collect::<Vec<_>>(),
        farm3d_lib::COMMAND_NAMES
    );
    assert!(manifest
        .iter()
        .all(|entry| entry.request.ends_with("Request") && entry.result.ends_with("Result")));

    let generated = std::fs::read_to_string(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../src/generated/contracts/command/CommandContracts.ts"),
    )
    .unwrap();
    for entry in manifest {
        assert!(generated.contains(&format!("export type {} =", entry.request)));
        assert!(generated.contains(&format!("export type {} =", entry.result)));
    }
}

#[test]
fn submitted_secret_sentinel_is_confined_to_the_injected_credential_store() {
    const SENTINEL: &str = "F1_SUBMITTED_SECRET_7d8a9c2e";
    let (temp, _lease, storage, printer) = credential_fixture();
    let store = InjectedCredentialStore::default();
    let committed = coordinate_connection_change(
        &storage,
        &store,
        &printer.id,
        printer.revision,
        connection(),
        CredentialSubmission::Replace(SENTINEL),
    )
    .unwrap();
    let reference = committed
        .printer
        .connection
        .as_ref()
        .unwrap()
        .credential_ref
        .as_ref()
        .unwrap();
    assert_eq!(store.get(reference).unwrap().as_deref(), Some(SENTINEL));

    storage
        .create_snapshot(farm3d_lib::persistence::SnapshotKind::Printers)
        .unwrap();
    let response = serde_json::to_vec(&committed.printer).unwrap();
    let error = serde_json::to_vec(&CommandError::credential_unavailable("fallbackFile")).unwrap();
    let event = serde_json::to_vec(&farm3d_lib::contracts::event::EventEnvelope {
        contract_version: farm3d_lib::contracts::ContractVersion::V1,
        stream_id: "stream".to_string(),
        sequence: farm3d_lib::contracts::event::JsSafeInteger::try_from(1_u64).unwrap(),
        event_id: "event".to_string(),
        occurred_at: "2026-09-17T00:00:00Z".to_string(),
        event_type: "printer.status.changed".to_string(),
        subject: farm3d_lib::contracts::event::EventSubject {
            kind: "printer".to_string(),
            id: printer.id,
        },
        payload: farm3d_lib::connections::PrinterStatus::errored(
            "The credential store is temporarily unavailable.",
        ),
    })
    .unwrap();
    for (label, bytes) in [("response", response), ("error", error), ("event", event)] {
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SENTINEL),
            "{label}"
        );
    }

    fn files(path: &std::path::Path, output: &mut Vec<std::path::PathBuf>) {
        if path.is_dir() {
            for entry in std::fs::read_dir(path).unwrap() {
                files(&entry.unwrap().path(), output);
            }
        } else {
            output.push(path.to_path_buf());
        }
    }
    let mut durable_files = Vec::new();
    files(temp.path(), &mut durable_files);
    let generated =
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/generated/contracts");
    files(&generated, &mut durable_files);
    for path in durable_files {
        let bytes = std::fs::read(&path).unwrap();
        assert!(
            !String::from_utf8_lossy(&bytes).contains(SENTINEL),
            "sentinel escaped to {}",
            path.display()
        );
    }
}

#[test]
fn successful_runtime_services_retain_the_metadata_lease() {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths.clone(), &lease).unwrap());
    let services = farm3d_lib::RuntimeServicesLease::new(storage, lease);

    assert!(matches!(
        MetadataRootLease::acquire(&paths),
        Err(farm3d_lib::persistence::StorageError::PersistenceUnavailable)
    ));
    drop(services);
    assert!(MetadataRootLease::acquire(&paths).is_ok());
}

#[test]
fn corrupt_f0_printers_have_a_distinct_safe_nonretryable_mapping() {
    let temp = tempfile::tempdir().unwrap();
    let metadata = temp.path().join("metadata");
    std::fs::create_dir_all(&metadata).unwrap();
    std::fs::write(metadata.join("printers.json"), b"not json").unwrap();
    let paths = StoragePaths::new(&metadata, temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Storage::open(paths, &lease).unwrap();

    let error = farm3d_lib::persistence::migrate_legacy(&storage).unwrap_err();
    let command = farm3d_lib::startup_command_error(error).unwrap();

    assert_eq!(command.code, ErrorCode::CorruptData);
    assert!(!command.retryable);
    assert!(command.recovery.is_empty());
    assert_eq!(
        command.details.unwrap().get("sourceName"),
        Some(&farm3d_lib::contracts::command::JsonValue::String(
            "printers.json".to_string()
        ))
    );
}

#[test]
fn sqlite_not_a_database_maps_to_nonretryable_database_corruption() {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    std::fs::write(paths.database(), b"this is not sqlite").unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();

    let storage_error = match Storage::open(paths, &lease) {
        Ok(_) => panic!("non-SQLite bytes must not open as a database"),
        Err(error) => error,
    };
    let command = farm3d_lib::startup_command_error(storage_error).unwrap();

    assert_eq!(command.code, ErrorCode::CorruptData);
    assert!(!command.retryable);
    assert!(command.recovery.is_empty());
    assert_eq!(
        command.details.unwrap().get("sourceName"),
        Some(&farm3d_lib::contracts::command::JsonValue::String(
            "database".to_string()
        ))
    );
}
