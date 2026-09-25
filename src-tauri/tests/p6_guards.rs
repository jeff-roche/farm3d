//! P6 Task 7 (spec D7): while a Printer has an unresolved Host Operation
//! (`dispatching`, `uncertain`, `reconciling`), every mutation that would
//! orphan it is refused inside its own transaction — driven here through
//! the Tauri IPC path (`archive_printer`, `delete_printer`,
//! `set_printer_connection`, `clear_printer_connection`, `import_printers`,
//! `printer_lifecycle_eligibility`). Each is allowed again once the row is
//! `succeeded`, `failed`, or `abandoned`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Barrier, Mutex};

use common::{a_catalog, a_stored_printer, invoke, FakeConnection};
use farm3d_lib::connections::capabilities::{HostOperationFailureCode, InconclusiveReason};
use farm3d_lib::connections::commands::retry_pending_credential_cleanup;
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection, MOONRAKER_KIND};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::document_io::{DocumentIo, DocumentKind};
use farm3d_lib::host_ops::repository::{self as host_ops, NewHostOperation, Outcome};
use farm3d_lib::host_ops::{
    HostOperationEndpoint, HostOperationFailure, HostOperationKind, HostOperationResolution,
};
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::spools::operations::OperationKind;
use serde_json::{json, Value};

const PRINTER: &str = "printer-a";
const OTHER_PRINTER: &str = "printer-b";
const HOST: &str = "ok.local";
const PORT: u16 = 7125;
const CREDENTIAL_REF: &str = "farm3d/credential/original";
const SECRET: &str = "ORIGINAL-SECRET";
/// The wire spelling of `LifecycleBlockerCode::HostOperationUnresolved`
/// (the enum is `SCREAMING_SNAKE_CASE` on the wire, like every other
/// blocker code).
const BLOCKER_CODE: &str = "HOST_OPERATION_UNRESOLVED";

/// Where a fixture Host Operation row is left.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RowState {
    Dispatching,
    Uncertain,
    Reconciling,
    Succeeded,
    Failed,
    Abandoned,
}

const UNRESOLVED: [RowState; 3] = [
    RowState::Dispatching,
    RowState::Uncertain,
    RowState::Reconciling,
];
const TERMINAL: [RowState; 3] = [RowState::Succeeded, RowState::Failed, RowState::Abandoned];

fn endpoint() -> HostOperationEndpoint {
    HostOperationEndpoint {
        kind: MOONRAKER_KIND.to_string(),
        host: HOST.to_string(),
        port: PORT,
    }
}

fn new_upload(operation_id: &str, printer_id: &str) -> NewHostOperation {
    NewHostOperation {
        operation_id: operation_id.to_string(),
        operation_kind: OperationKind::StageSliceRevision,
        request_digest: format!("digest-{operation_id}"),
        printer_id: printer_id.to_string(),
        kind: HostOperationKind::Upload,
        slice_revision_id: None,
        source_host_operation_id: None,
        gcode_sha256: Some("a".repeat(64)),
        gcode_size: Some(100),
        host_path: "farm3d/slr-a.gcode".to_string(),
        history_mark: None,
        endpoint: endpoint(),
    }
}

fn uncertain() -> Outcome {
    Outcome::Uncertain {
        reason: InconclusiveReason::ResponseLost,
        no_longer_pending: false,
    }
}

/// Inserts an `upload` row for `printer_id` and drives it to `state`
/// through the repository's legal D3 edges. Returns its `hop-*` id.
fn seed_row(storage: &Storage, printer_id: &str, operation_id: &str, state: RowState) -> String {
    let id = storage
        .write_repo(|tx| host_ops::insert_dispatching(tx, &new_upload(operation_id, printer_id)))
        .expect("insert dispatching")
        .id;
    let steps: Vec<Outcome> = match state {
        RowState::Dispatching => vec![],
        RowState::Uncertain => vec![uncertain()],
        RowState::Reconciling => vec![uncertain(), Outcome::Reconciling],
        RowState::Succeeded => vec![Outcome::Succeeded {
            resolution: HostOperationResolution::ArtifactVerified { reconciled: false },
        }],
        RowState::Failed => vec![Outcome::Failed {
            failure: HostOperationFailure::for_code(HostOperationFailureCode::HostUnreachable),
        }],
        RowState::Abandoned => vec![uncertain(), Outcome::Abandoned { note: None }],
    };
    for step in steps {
        storage
            .write_repo(|tx| host_ops::transition(tx, &id, step.clone()))
            .expect("transition");
    }
    id
}

/// A `succeeded` `start` row that starts the (terminal) upload `source`.
fn seed_succeeded_start(storage: &Storage, printer_id: &str, operation_id: &str, source: &str) {
    let start = NewHostOperation {
        operation_kind: OperationKind::StartStagedArtifact,
        kind: HostOperationKind::Start,
        source_host_operation_id: Some(source.to_string()),
        history_mark: Some(0),
        ..new_upload(operation_id, printer_id)
    };
    let id = storage
        .write_repo(|tx| host_ops::insert_dispatching(tx, &start))
        .expect("insert start")
        .id;
    storage
        .write_repo(|tx| {
            host_ops::transition(
                tx,
                &id,
                Outcome::Succeeded {
                    resolution: HostOperationResolution::StartAccepted,
                },
            )
        })
        .expect("start accepted");
}

/// Moves an unresolved fixture row to a terminal state.
fn resolve_row(storage: &Storage, id: &str) {
    let current = storage
        .read(|connection| {
            connection.query_row(
                "SELECT state FROM host_operations WHERE id = ?1",
                [id],
                |row| row.get::<_, String>(0),
            )
        })
        .unwrap();
    if current == "dispatching" {
        storage
            .write_repo(|tx| host_ops::transition(tx, id, uncertain()))
            .unwrap();
    }
    if current == "reconciling" {
        storage
            .write_repo(|tx| host_ops::record_attempt(tx, id, InconclusiveReason::ResponseLost))
            .unwrap();
    }
    storage
        .write_repo(|tx| host_ops::transition(tx, id, Outcome::Abandoned { note: None }))
        .unwrap();
}

fn count(storage: &Storage, sql: &str) -> i64 {
    storage
        .read(|connection| connection.query_row(sql, [], |row| row.get::<_, i64>(0)))
        .unwrap()
}

fn host_operation_rows(storage: &Storage, printer_id: &str) -> i64 {
    count(
        storage,
        &format!("SELECT COUNT(*) FROM host_operations WHERE printer_id = '{printer_id}'"),
    )
}

fn a_connected_printer(id: &str) -> StoredPrinter {
    StoredPrinter {
        connection: Some(ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: HOST.to_string(),
            port: PORT,
            use_tls: false,
            credential_ref: Some(CREDENTIAL_REF.to_string()),
        }),
        ..a_stored_printer(id)
    }
}

/// A `DocumentIo` whose open dialog always picks one Printers document.
struct ImportDocument(Vec<u8>);

impl DocumentIo for ImportDocument {
    fn open_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(Some(PathBuf::from("printers.json")))
    }
    fn save_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn read(&self, _path: &Path) -> Result<Vec<u8>, CommandError> {
        Ok(self.0.clone())
    }
    fn atomic_write(&self, _path: &Path, _bytes: &[u8]) -> Result<(), CommandError> {
        Err(CommandError::internal())
    }
}

/// A v1 Printers document holding `printer-a` under a new name.
fn import_document() -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schemaVersion": 1,
        "exportedAt": "2026-09-25T00:00:00Z",
        "printers": [{
            "id": PRINTER,
            "revision": 1,
            "name": "Imported",
            "catalogRef": common::a_ref_json(),
            "notes": "",
            "overrides": {},
            "lastKnownGood": null,
            "connection": {"kind": MOONRAKER_KIND, "host": HOST, "port": PORT, "useTls": false}
        }]
    }))
    .unwrap()
}

struct Harness {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Arc<Storage>,
    credentials_dir: tempfile::TempDir,
    _app: tauri::App<tauri::test::MockRuntime>,
    webview: tauri::WebviewWindow<tauri::test::MockRuntime>,
    /// Every host the connection factory built a connection for (a probe
    /// or a supervision start).
    probed: Arc<Mutex<Vec<(String, u16)>>>,
}

impl Harness {
    /// `printer-a`, connected to `ok.local:7125` with a stored credential.
    fn new() -> Self {
        let (temp, lease, storage, _database) = common::storage();
        let credentials_dir = tempfile::tempdir().unwrap();
        CredentialStore::file_backed(credentials_dir.path().to_path_buf())
            .set(CREDENTIAL_REF, SECRET)
            .unwrap();
        PrinterRepository::new(Arc::clone(&storage))
            .create(a_connected_printer(PRINTER))
            .unwrap();
        let probed = Arc::new(Mutex::new(Vec::new()));
        let recorded = Arc::clone(&probed);
        let factory = move |config: &ConnectionConfig,
                            _key: Option<zeroize::Zeroizing<String>>|
              -> Option<Box<dyn PrinterConnection>> {
            recorded
                .lock()
                .unwrap()
                .push((config.host.clone(), config.port));
            Some(Box::new(FakeConnection {
                host: config.host.clone(),
            }))
        };
        let (app, webview, _manager, _services) = common::runtime_with_documents(
            tauri::generate_handler![
                farm3d_lib::printers::commands::archive_printer,
                farm3d_lib::printers::commands::delete_printer,
                farm3d_lib::printers::commands::import_printers,
                farm3d_lib::printers::commands::printer_lifecycle_eligibility,
                farm3d_lib::connections::commands::set_printer_connection,
                farm3d_lib::connections::commands::clear_printer_connection,
            ],
            Arc::clone(&storage),
            Arc::new(a_catalog()),
            credentials_dir.path().to_path_buf(),
            factory,
            Arc::new(ImportDocument(import_document())),
        );
        Self {
            _temp: temp,
            _lease: lease,
            storage,
            credentials_dir,
            _app: app,
            webview,
            probed,
        }
    }

    fn printer(&self) -> Option<StoredPrinter> {
        PrinterRepository::new(Arc::clone(&self.storage))
            .get(PRINTER)
            .unwrap()
    }

    fn revision(&self) -> i64 {
        self.printer().unwrap().revision
    }

    fn credentials(&self) -> CredentialStore {
        CredentialStore::file_backed(self.credentials_dir.path().to_path_buf())
    }

    fn invoke(&self, command: &str, body: Value) -> Result<Value, Value> {
        invoke(&self.webview, command, body)
    }

    fn archive(&self) -> Result<Value, Value> {
        self.invoke(
            "archive_printer",
            json!({
                "contractVersion": 1,
                "id": PRINTER,
                "expectedRevision": self.revision(),
                "operationId": format!("op-archive-{}", self.revision()),
                "spoolDispositions": [],
            }),
        )
    }

    fn delete(&self) -> Result<Value, Value> {
        self.invoke(
            "delete_printer",
            json!({"contractVersion": 1, "id": PRINTER, "expectedRevision": self.revision()}),
        )
    }

    fn set_connection(&self, submission: Value) -> Result<Value, Value> {
        self.invoke(
            "set_printer_connection",
            json!({
                "contractVersion": 1,
                "id": PRINTER,
                "expectedRevision": self.revision(),
                "submission": submission,
            }),
        )
    }

    fn clear_connection(&self) -> Result<Value, Value> {
        self.invoke(
            "clear_printer_connection",
            json!({"contractVersion": 1, "id": PRINTER, "expectedRevision": self.revision()}),
        )
    }

    fn import(&self) -> Result<Value, Value> {
        self.invoke(
            "import_printers",
            json!({
                "contractVersion": 1,
                "expectedRevisions": [{"id": PRINTER, "revision": self.revision()}],
            }),
        )
    }

    fn eligibility(&self) -> Value {
        self.invoke(
            "printer_lifecycle_eligibility",
            json!({"contractVersion": 1, "id": PRINTER}),
        )
        .unwrap()["data"]
            .clone()
    }
}

fn assert_lifecycle_blocked(error: &Value, action: &str, message: &str) {
    assert_eq!(error["code"], "LIFECYCLE_BLOCKED", "{error}");
    assert_eq!(
        error["details"]["blockers"],
        json!([{"action": action, "code": BLOCKER_CODE, "message": message}]),
        "{error}"
    );
}

fn assert_connection_in_use(error: &Value, host_operation_id: &str) {
    assert_eq!(error["code"], "CONNECTION_IN_USE", "{error}");
    assert_eq!(
        error["message"],
        "Finish or abandon the pending printer operation before changing this Connection."
    );
    assert_eq!(error["recovery"], json!(["OPEN_PRINTER_JOB"]));
    assert_eq!(error["retryable"], false);
    assert_eq!(
        error["details"],
        json!({"printerId": PRINTER, "hostOperationId": host_operation_id})
    );
}

// --- Archive ---------------------------------------------------------------

#[test]
fn archive_is_blocked_while_a_host_operation_is_unresolved() {
    for state in UNRESOLVED {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);
        let revision = harness.revision();

        let error = harness.archive().unwrap_err();

        assert_lifecycle_blocked(
            &error,
            "archive",
            "Finish or abandon the pending printer operation before archiving.",
        );
        let printer = harness.printer().unwrap();
        assert!(printer.archived_at.is_none(), "{state:?}");
        assert_eq!(printer.revision, revision, "{state:?}");
    }
}

#[test]
fn archive_is_allowed_after_a_terminal_host_operation() {
    for state in TERMINAL {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);

        let archived = harness.archive().unwrap();

        assert!(
            archived["data"]["printer"]["archivedAt"].is_string(),
            "{state:?}"
        );
    }
}

// --- Delete ----------------------------------------------------------------

#[test]
fn delete_is_blocked_while_a_host_operation_is_unresolved() {
    for state in UNRESOLVED {
        let harness = Harness::new();
        harness.archive().unwrap();
        let id = seed_row(&harness.storage, PRINTER, "op-1", state);

        let error = harness.delete().unwrap_err();

        assert_lifecycle_blocked(
            &error,
            "delete",
            "Finish or abandon the pending printer operation before deleting.",
        );
        assert!(harness.printer().is_some(), "{state:?}");
        assert_eq!(host_operation_rows(&harness.storage, PRINTER), 1);

        // Resolving the row releases the guard (D8: abandon).
        resolve_row(&harness.storage, &id);
        harness.delete().unwrap();
        assert!(harness.printer().is_none(), "{state:?}");
    }
}

#[test]
fn delete_after_terminal_rows_removes_only_that_printers_terminal_rows() {
    for state in TERMINAL {
        let harness = Harness::new();
        PrinterRepository::new(Arc::clone(&harness.storage))
            .create(StoredPrinter {
                connection: None,
                ..a_stored_printer(OTHER_PRINTER)
            })
            .unwrap();
        seed_row(&harness.storage, OTHER_PRINTER, "op-other", state);
        harness.archive().unwrap();
        seed_row(&harness.storage, PRINTER, "op-1", state);
        let staged = seed_row(&harness.storage, PRINTER, "op-2", RowState::Succeeded);
        // A terminal start linked to that upload: deleting both in one
        // statement unlinks the start first (`ON DELETE SET NULL`).
        seed_succeeded_start(&harness.storage, PRINTER, "op-3", &staged);

        harness.delete().unwrap();

        assert!(harness.printer().is_none(), "{state:?}");
        assert_eq!(host_operation_rows(&harness.storage, PRINTER), 0);
        assert_eq!(host_operation_rows(&harness.storage, OTHER_PRINTER), 1);
    }
}

// --- Eligibility -----------------------------------------------------------

#[test]
fn eligibility_reports_the_unresolved_host_operation_blocker() {
    for state in UNRESOLVED {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);

        let eligibility = harness.eligibility();

        assert_eq!(eligibility["canArchive"], false, "{state:?}");
        let blockers = eligibility["blockers"].as_array().unwrap();
        assert!(blockers.contains(&json!({
            "action": "archive",
            "code": BLOCKER_CODE,
            "message": "Finish or abandon the pending printer operation before archiving.",
        })));
        assert!(blockers.contains(&json!({
            "action": "delete",
            "code": BLOCKER_CODE,
            "message": "Finish or abandon the pending printer operation before deleting.",
        })));
    }
    for state in TERMINAL {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);

        let eligibility = harness.eligibility();

        assert_eq!(eligibility["canArchive"], true, "{state:?}");
        assert!(!eligibility["blockers"]
            .as_array()
            .unwrap()
            .iter()
            .any(|blocker| blocker["code"] == BLOCKER_CODE));
    }
}

// --- Import ----------------------------------------------------------------

#[test]
fn import_is_rejected_whole_while_any_host_operation_is_unresolved() {
    for state in UNRESOLVED {
        let harness = Harness::new();
        let id = seed_row(&harness.storage, PRINTER, "op-1", state);
        let before = harness.printer().unwrap();

        let error = harness.import().unwrap_err();

        assert_eq!(error["code"], "HOST_OPERATION_PENDING", "{error}");
        assert_eq!(
            error["message"],
            "This printer has a pending operation. Finish or abandon it first."
        );
        assert_eq!(error["recovery"], json!(["OPEN_PRINTER_JOB"]));
        assert_eq!(error["retryable"], false);
        assert_eq!(
            error["details"],
            json!({"printerIds": [PRINTER], "hostOperationIds": [id]})
        );
        let after = harness.printer().unwrap();
        assert_eq!(after.name, before.name, "nothing written ({state:?})");
        assert_eq!(after.revision, before.revision);
        assert_eq!(host_operation_rows(&harness.storage, PRINTER), 1);
    }
}

#[test]
fn import_is_allowed_after_a_terminal_host_operation() {
    for state in TERMINAL {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);

        let imported = harness.import().unwrap();

        assert_eq!(imported["data"]["status"], "applied", "{state:?}");
        assert_eq!(harness.printer().unwrap().name, "Imported");
    }
}

// --- Connection: endpoint change --------------------------------------------

#[test]
fn an_endpoint_change_is_connection_in_use_while_unresolved() {
    let changes = [
        json!({"kind": MOONRAKER_KIND, "host": HOST, "port": 7126, "useTls": false}),
        json!({"kind": MOONRAKER_KIND, "host": "other.local", "port": PORT, "useTls": false}),
        json!({"kind": "octoprint", "host": HOST, "port": PORT, "useTls": false}),
    ];
    for state in UNRESOLVED {
        for change in &changes {
            let harness = Harness::new();
            let id = seed_row(&harness.storage, PRINTER, "op-1", state);
            let before = harness.printer().unwrap();

            let error = harness
                .invoke(
                    "set_printer_connection",
                    json!({
                        "contractVersion": 1,
                        "id": PRINTER,
                        "expectedRevision": before.revision,
                        "submission": change,
                        "acceptUnverified": true,
                    }),
                )
                .unwrap_err();

            assert_connection_in_use(&error, &id);
            let after = harness.printer().unwrap();
            assert_eq!(after.connection, before.connection, "{state:?} {change}");
            assert_eq!(after.revision, before.revision);
        }
    }
}

#[test]
fn an_endpoint_change_is_allowed_after_a_terminal_host_operation() {
    for state in TERMINAL {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);

        harness
            .set_connection(
                json!({"kind": MOONRAKER_KIND, "host": HOST, "port": 7126, "useTls": false}),
            )
            .unwrap();

        assert_eq!(harness.printer().unwrap().connection.unwrap().port, 7126);
    }
}

// --- Connection: clear -------------------------------------------------------

#[test]
fn clearing_the_connection_is_connection_in_use_while_unresolved() {
    for state in UNRESOLVED {
        let harness = Harness::new();
        let id = seed_row(&harness.storage, PRINTER, "op-1", state);

        let error = harness.clear_connection().unwrap_err();

        assert_connection_in_use(&error, &id);
        assert!(harness.printer().unwrap().connection.is_some(), "{state:?}");
        assert_eq!(
            harness
                .credentials()
                .get(CREDENTIAL_REF)
                .unwrap()
                .as_deref(),
            Some(SECRET)
        );
    }
}

#[test]
fn clearing_the_connection_is_allowed_after_a_terminal_host_operation() {
    for state in TERMINAL {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);

        harness.clear_connection().unwrap();

        assert!(harness.printer().unwrap().connection.is_none(), "{state:?}");
    }
}

// --- Connection: credential clear and replacement ---------------------------

#[test]
fn clearing_the_credential_is_connection_in_use_while_unresolved() {
    for state in UNRESOLVED {
        let harness = Harness::new();
        let id = seed_row(&harness.storage, PRINTER, "op-1", state);

        let error = harness
            .set_connection(json!({
                "kind": MOONRAKER_KIND, "host": HOST, "port": PORT, "useTls": false,
                "credential": "",
            }))
            .unwrap_err();

        assert_connection_in_use(&error, &id);
        let connection = harness.printer().unwrap().connection.unwrap();
        assert_eq!(connection.credential_ref.as_deref(), Some(CREDENTIAL_REF));
        assert_eq!(
            harness
                .credentials()
                .get(CREDENTIAL_REF)
                .unwrap()
                .as_deref(),
            Some(SECRET)
        );
    }
}

#[test]
fn clearing_the_credential_is_allowed_after_a_terminal_host_operation() {
    for state in TERMINAL {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);

        harness
            .set_connection(json!({
                "kind": MOONRAKER_KIND, "host": HOST, "port": PORT, "useTls": false,
                "credential": "",
            }))
            .unwrap();

        let connection = harness.printer().unwrap().connection.unwrap();
        assert_eq!(connection.credential_ref, None, "{state:?}");
    }
}

#[test]
fn replacing_the_credential_on_the_same_endpoint_is_allowed_and_probed_while_unresolved() {
    for state in UNRESOLVED {
        let harness = Harness::new();
        seed_row(&harness.storage, PRINTER, "op-1", state);
        harness.probed.lock().unwrap().clear();

        let updated = harness
            .set_connection(json!({
                "kind": MOONRAKER_KIND, "host": HOST, "port": PORT, "useTls": false,
                "credential": "NEW-SECRET",
            }))
            .unwrap();

        let connection = harness.printer().unwrap().connection.unwrap();
        let new_reference = connection.credential_ref.clone().unwrap();
        assert_ne!(new_reference, CREDENTIAL_REF, "{state:?}");
        assert_eq!((connection.host.as_str(), connection.port), (HOST, PORT));
        assert_eq!(
            harness
                .credentials()
                .get(&new_reference)
                .unwrap()
                .as_deref(),
            Some("NEW-SECRET")
        );
        assert!(
            harness
                .probed
                .lock()
                .unwrap()
                .contains(&(HOST.to_string(), PORT)),
            "the replacement is still probed ({state:?})"
        );
        assert!(!updated.to_string().contains("NEW-SECRET"));
    }
}

// --- Credential cleanup -----------------------------------------------------

#[test]
fn credential_cleanup_never_deletes_a_credential_of_a_printer_with_an_unresolved_row() {
    let harness = Harness::new();
    let id = seed_row(&harness.storage, PRINTER, "op-1", RowState::Uncertain);
    // A cleanup row naming the Printer's current credential (whatever
    // queued it) must not take the secret the pending operation relies on.
    harness
        .storage
        .write(|tx| {
            tx.execute(
                "INSERT INTO pending_credential_cleanup(credential_ref, printer_id, reason, created_at)
                 VALUES (?1, ?2, 'replaced', '2026-09-25T00:00:00Z')",
                [CREDENTIAL_REF, PRINTER],
            )?;
            Ok(())
        })
        .unwrap();
    // And the credential a same-endpoint replacement just displaced.
    harness
        .set_connection(json!({
            "kind": MOONRAKER_KIND, "host": HOST, "port": PORT, "useTls": false,
            "credential": "NEW-SECRET",
        }))
        .unwrap();

    retry_pending_credential_cleanup(&harness.storage, &harness.credentials()).unwrap();

    assert_eq!(
        harness
            .credentials()
            .get(CREDENTIAL_REF)
            .unwrap()
            .as_deref(),
        Some(SECRET),
        "kept while the row is unresolved"
    );

    resolve_row(&harness.storage, &id);
    retry_pending_credential_cleanup(&harness.storage, &harness.credentials()).unwrap();

    assert_eq!(
        harness.credentials().get(CREDENTIAL_REF).unwrap(),
        None,
        "released once the row is terminal"
    );
    let current = harness
        .printer()
        .unwrap()
        .connection
        .unwrap()
        .credential_ref
        .unwrap();
    assert_eq!(
        harness.credentials().get(&current).unwrap().as_deref(),
        Some("NEW-SECRET")
    );
}

// --- Race -------------------------------------------------------------------

/// A write-ahead insert and a permanent delete of the same (archived)
/// Printer, from two threads at once: the writes serialize, exactly one
/// wins, and no `host_operations` row is ever left without its Printer.
/// Rounds alternate which thread is nudged to go second, so both orders
/// are exercised: an insert first blocks the delete (`LIFECYCLE_BLOCKED`);
/// a delete first fails the insert's foreign key.
#[test]
fn a_concurrent_insert_and_delete_leave_no_orphan_row() {
    let nudge = |round: usize, thread: usize| {
        if round % 3 == thread {
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    };
    let mut insert_wins = 0;
    let mut delete_wins = 0;
    for round in 0..24 {
        let (_temp, _lease, storage, _database) = common::storage();
        let repository = PrinterRepository::new(Arc::clone(&storage));
        repository.create(a_connected_printer(PRINTER)).unwrap();
        let archived = repository
            .archive(PRINTER, 1, &format!("op-archive-{round}"), &[])
            .unwrap();
        let barrier = Arc::new(Barrier::new(2));

        let inserter = {
            let storage = Arc::clone(&storage);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                nudge(round, 1);
                storage
                    .write_repo(|tx| host_ops::insert_dispatching(tx, &new_upload("op-1", PRINTER)))
            })
        };
        let deleter = {
            let storage = Arc::clone(&storage);
            let barrier = Arc::clone(&barrier);
            std::thread::spawn(move || {
                barrier.wait();
                nudge(round, 2);
                PrinterRepository::new(storage).delete(PRINTER, archived.revision)
            })
        };
        let inserted = inserter.join().unwrap();
        let deleted = deleter.join().unwrap();

        match (&inserted, &deleted) {
            (Ok(_), Err(RepositoryError::LifecycleBlocked(blockers))) => {
                insert_wins += 1;
                assert_eq!(blockers.len(), 1);
                assert_eq!(repository.list().unwrap().len(), 1);
                assert_eq!(host_operation_rows(&storage, PRINTER), 1);
            }
            (Err(_), Ok(_)) => {
                delete_wins += 1;
                assert!(repository.list().unwrap().is_empty());
                assert_eq!(host_operation_rows(&storage, PRINTER), 0);
            }
            other => panic!("exactly one must win, got {other:?}"),
        }
        assert_eq!(
            count(
                &storage,
                "SELECT COUNT(*) FROM host_operations
                 WHERE printer_id NOT IN (SELECT id FROM printers)"
            ),
            0,
            "no orphan row"
        );
    }
    assert_eq!(insert_wins + delete_wins, 24);
    assert!(
        insert_wins > 0,
        "the insert-first order was never exercised"
    );
    assert!(
        delete_wins > 0,
        "the delete-first order was never exercised"
    );
}
