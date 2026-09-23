//! P2 task 4: `create_printer` with options, `probe_connection`, and safe
//! Connection replacement. See
//! `.superpowers/sdd/2026-09-22-p2-printer-lifecycle-batch-setup/task-4-brief.md`.

mod common;

use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use common::{
    a_catalog, a_ref_json, a_stored_printer, credentials_dir_snapshot, invoke,
    pending_cleanup_count, FakeConnection,
};
use farm3d_lib::catalog::Catalog;
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::supervisor::{ConnectionManager, STATUS_EVENT};
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection, MOONRAKER_KIND};
use farm3d_lib::persistence::Storage;
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::RuntimeServices;
use serde_json::json;
use tauri::Listener;

fn storage() -> (tempfile::TempDir, farm3d_lib::persistence::MetadataRootLease, Arc<Storage>) {
    let (temp, lease, storage, _database) = common::storage();
    (temp, lease, storage)
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

/// [`common::runtime`] with this file's command set registered.
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
    common::runtime(
        tauri::generate_handler![
            farm3d_lib::printers::commands::list_printers,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::update_printer,
            farm3d_lib::connections::commands::set_printer_connection,
            farm3d_lib::connections::commands::test_printer_connection,
            farm3d_lib::printers::create::probe_connection,
        ],
        storage,
        catalog,
        credentials_dir,
        factory,
    )
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
    // `probe_connection` has no Printer id to attach.
    assert!(error["details"].get("entityId").is_none());
}

// --- Regression guard: `test_printer_connection` still attaches `entityId` -

/// `probe_submission` is shared by `probe_connection` (no Printer id) and
/// `test_printer_connection`/`set_printer_connection`'s replace check
/// (both have one). Before this test existed, extracting `probe_error`
/// silently dropped the `entityId` detail from `test_printer_connection`'s
/// network errors — the same "behaviour identical" ruling `probe_error`'s
/// doc comment describes. Pins it down so it can't regress again.
#[test]
fn test_printer_connection_still_attaches_entity_id_on_a_network_error() {
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

    let error = invoke(
        &webview,
        "test_printer_connection",
        json!({
            "contractVersion": 1,
            "id": printer.id,
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
    assert_eq!(error["details"]["entityId"], json!(printer.id));
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
    let (_app, webview, _manager, _services) = runtime(
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
    let before_credentials = credentials_dir_snapshot(credentials_dir.path());

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
    // A real check, not a lookup of the raw secret text as a reference (which
    // is always `None` regardless of whether anything was actually stored):
    // the credential store's own backing file must be byte-identical to
    // before the rejected create, i.e. nothing was ever written to it.
    assert_eq!(
        credentials_dir_snapshot(credentials_dir.path()),
        before_credentials,
        "no credential must have been stored for the rejected create"
    );
}

/// The orphan-cleanup path `create_printer_with` takes when `create_in`
/// itself rejects the insert (the transactional `precheck_duplicate_host`
/// defense-in-depth, not the early `find_active_by_host_identity` check the
/// test above exercises): the provisional row and its secret survive the
/// rolled-back insert, and `retry_pending_credential_cleanup` — called right
/// after, per the brief's Step 4 — reclaims both. Exercised directly against
/// `PrinterRepository`/`CredentialStore` rather than through the command,
/// since `create_printer_with`'s own early pre-check makes this particular
/// failure unreachable through a single, non-racing command invocation.
#[test]
fn create_in_failure_leaves_a_reclaimable_provisional_row_and_secret() {
    use farm3d_lib::persistence::RepositoryError;

    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let credentials = CredentialStore::file_backed(credentials_dir.path().to_path_buf());
    let repository = PrinterRepository::new(Arc::clone(&storage));

    let mut existing = a_stored_printer("printer-existing");
    existing.connection = Some(ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: "conflict.local".to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: None,
    });
    repository.create(existing).unwrap();

    // Mirrors create_printer_with's Step 4, items 1-3, directly: mint a
    // reference, enqueue it as provisional, write the secret.
    let reference = "farm3d/credential/6ba7b810-9dad-4f83-a131-2a6f44cbbf89".to_string();
    repository
        .enqueue_credential_cleanup(&reference, None, "provisional")
        .unwrap();
    credentials.set(&reference, "s3cret-FIXTURE").unwrap();

    let mut conflicting = a_stored_printer("printer-conflicting");
    conflicting.connection = Some(ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: "conflict.local".to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: Some(reference.clone()),
    });

    let result = repository.create_in(conflicting, Some(&reference));

    assert!(
        matches!(result, Err(RepositoryError::DuplicateHost { .. })),
        "the conflicting insert must roll back: {result:?}"
    );
    assert_eq!(
        pending_cleanup_count(&storage),
        1,
        "the provisional row must survive the rolled-back insert"
    );
    assert_eq!(
        credentials.get(&reference).unwrap().as_deref(),
        Some("s3cret-FIXTURE"),
        "the secret must still be in the store, unreferenced by any Printer"
    );

    // create_printer_with's error path: retry cleanup right away rather than
    // waiting for the next startup.
    farm3d_lib::connections::commands::retry_pending_credential_cleanup(&storage, &credentials)
        .unwrap();

    assert_eq!(
        pending_cleanup_count(&storage),
        0,
        "the reclaimed provisional row must be gone"
    );
    assert_eq!(
        credentials.get(&reference).unwrap(),
        None,
        "the orphaned secret must be gone from the store"
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

// --- 9b. `set_printer_connection` to another active Printer's host --------

fn a_connected_printer(id: &str, host: &str) -> farm3d_lib::printers::StoredPrinter {
    let mut printer = a_stored_printer(id);
    printer.connection = Some(ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: host.to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: None,
    });
    printer
}

fn assert_duplicate_host_rejected_before_probe_and_secret(existing_host: Option<&str>) {
    let (_temp, _lease, storage) = storage();
    let credentials_dir = tempfile::tempdir().unwrap();
    let (factory, calls) = recording_factory(Arc::clone(&storage));
    let (_app, webview, _manager, _services) = runtime(
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials_dir.path().to_path_buf(),
        factory,
    );
    let repository = PrinterRepository::new(Arc::clone(&storage));
    let owner = repository
        .create(a_connected_printer("printer-owner", "ok.local"))
        .unwrap();
    let target = match existing_host {
        Some(host) => repository.create(a_connected_printer("printer-b", host)),
        None => repository.create(a_stored_printer("printer-b")),
    }
    .unwrap();
    let before_credentials = credentials_dir_snapshot(credentials_dir.path());
    let before_pending = pending_cleanup_count(&storage);

    let error = invoke(
        &webview,
        "set_printer_connection",
        json!({
            "contractVersion": 1,
            "id": target.id,
            "expectedRevision": target.revision,
            "submission": {
                "kind": "moonraker",
                "host": "OK.local",
                "port": 7125,
                "useTls": false,
                "credential": "s3cret-FIXTURE",
            },
        }),
    )
    .unwrap_err();

    assert_eq!(error["code"], "DUPLICATE_HOST");
    assert_eq!(error["details"]["conflictingPrinterId"], json!(owner.id));
    assert!(
        calls.recv_timeout(Duration::from_millis(300)).is_err(),
        "a duplicate host must be rejected before any probe"
    );
    assert_eq!(
        credentials_dir_snapshot(credentials_dir.path()),
        before_credentials,
        "no credential may be written for a rejected duplicate host"
    );
    assert_eq!(pending_cleanup_count(&storage), before_pending);
    assert_eq!(repository.get(&target.id).unwrap().unwrap(), target);
}

#[test]
fn first_time_set_printer_connection_to_a_taken_host_is_duplicate_host_without_writing_a_secret() {
    assert_duplicate_host_rejected_before_probe_and_secret(None);
}

#[test]
fn replacing_a_connection_with_a_taken_host_is_duplicate_host_without_probing() {
    assert_duplicate_host_rejected_before_probe_and_secret(Some("other.local"));
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
