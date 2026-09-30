//! P9 Task 11 (spec D17, decision 20, acceptance criterion 11): the final
//! cross-domain reference matrix. Every archive, delete, prune, cleanup,
//! reset, and restore path in v1 is one row, run against its own
//! every-domain Farm (`p9_farm`) through the real commands where one
//! exists. After each row:
//!
//! - the result is the expected success, or the expected refusal (for a
//!   lifecycle refusal, exactly the expected blocker codes, and no table's
//!   row count changed);
//! - `integrity::check` (with the file roots) has no violation, and its
//!   tolerated findings are exactly the row's expected ones;
//! - decision 20's named allowed loss holds: a Printer delete loses exactly
//!   the Spool movements through that Printer's slots, every other live
//!   row loses none, a restore holds exactly the backup's movements, and a
//!   Farm reset holds none.

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::fs;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use tauri::test::MockRuntime;

use farm3d_lib::backup::dialogs::PortabilityDialogs;
use farm3d_lib::backup::installer::InstallOutcome;
use farm3d_lib::backup::lease::{BackupLease, LeaseActivity};
use farm3d_lib::backup::restart::RecordingRestarter;
use farm3d_lib::backup::writer::{write_backup, BackupRequest, WriterHooks};
use farm3d_lib::backup::{BackupMediaChoice, BackupOrigin};
use farm3d_lib::cameras::media;
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::persistence::integrity::{self, IntegrityRoots, IntegrityRule};
use farm3d_lib::persistence::{RepositoryError, SnapshotKind, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::RuntimeServices;

use p9_farm::{ids, snapshot_rel_path, Farm};

const CREATED_AT: &str = "2026-09-29T12:00:00.000Z";

// --- the command rig ----------------------------------------------------------------

struct Dialogs {
    open: Mutex<Option<PathBuf>>,
}

impl PortabilityDialogs for Dialogs {
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.open.lock().unwrap().clone())
    }
    fn save_backup(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_diagnostics(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
}

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

struct Rig {
    app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    restarter: Arc<RecordingRestarter>,
}

impl Rig {
    fn new(farm: &Farm, backup: Option<PathBuf>) -> Rig {
        let dialogs: Arc<dyn PortabilityDialogs> = Arc::new(Dialogs {
            open: Mutex::new(backup),
        });
        let restarter = Arc::new(RecordingRestarter::default());
        let injected = Arc::clone(&restarter);
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::printers::commands::archive_printer,
                farm3d_lib::printers::commands::unarchive_printer,
                farm3d_lib::printers::commands::delete_printer,
                farm3d_lib::printers::commands::set_material_slot_layout,
                farm3d_lib::connections::commands::clear_printer_connection,
                farm3d_lib::cameras::commands::clear_printer_camera,
                farm3d_lib::slicing::commands::delete_preparation,
                farm3d_lib::spools::commands::set_spool_lifecycle,
                farm3d_lib::spools::commands::delete_tare,
                farm3d_lib::library::commands::delete_project,
                farm3d_lib::library::commands::delete_model,
                farm3d_lib::slicing::commands::delete_slice_revision,
                farm3d_lib::queue::commands::remove_queue_entry,
                farm3d_lib::cameras::commands::set_snapshot_pinned,
                farm3d_lib::diagnostics::commands::clear_storage,
                farm3d_lib::diagnostics::commands::reset_farm,
                farm3d_lib::backup::commands::preview_restore,
                farm3d_lib::backup::commands::apply_restore,
            ],
            Arc::clone(&farm.storage),
            Arc::new(common::a_catalog()),
            farm.credentials_dir.clone(),
            no_connection,
            move |services| {
                let backup = services
                    .backup
                    .with_dialogs(dialogs)
                    .with_restarter(injected);
                services.backup = Arc::new(backup);
            },
        );
        Rig {
            app,
            webview,
            services,
            restarter,
        }
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|envelope| envelope["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command}: {error}"))
    }
}

// --- reading the Farm ---------------------------------------------------------------

fn sql(farm: &Farm, statements: &str) {
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            tx.execute_batch(statements)
                .unwrap_or_else(|error| panic!("fixture SQL failed: {error}\n{statements}"));
            Ok(())
        })
        .expect("fixture write");
}

fn revision(farm: &Farm, table: &str, id: &str) -> i64 {
    farm.storage
        .read(|connection| {
            connection.query_row(
                &format!("SELECT revision FROM {table} WHERE id = ?1"),
                [id],
                |row| row.get(0),
            )
        })
        .unwrap_or_else(|error| panic!("{table} {id}: {error:?}"))
}

fn ids_of(connection: &Connection, statement: &str) -> BTreeSet<String> {
    connection
        .prepare(statement)
        .unwrap()
        .query_map([], |row| row.get::<_, String>(0))
        .unwrap()
        .map(Result::unwrap)
        .collect()
}

fn movements(connection: &Connection) -> BTreeSet<String> {
    ids_of(connection, "SELECT id FROM spool_movements")
}

/// Decision 20's named loss for deleting `printer_id`: the movements into
/// or out of its slots.
fn movements_through(connection: &Connection, printer_id: &str) -> BTreeSet<String> {
    ids_of(
        connection,
        &format!(
            "SELECT id FROM spool_movements
              WHERE from_slot_id IN (SELECT id FROM material_slots WHERE printer_id = '{printer_id}')
                 OR to_slot_id IN (SELECT id FROM material_slots WHERE printer_id = '{printer_id}')"
        ),
    )
}

fn open(farm: &Farm) -> Connection {
    Connection::open(farm.paths().database()).unwrap()
}

// --- a row's run --------------------------------------------------------------------

/// Where the row's Farm ended up: the live Farm (most rows), or the Farm
/// the startup installer produced after a restore or a Farm reset.
struct Site {
    storage: Arc<Storage>,
    roots: IntegrityRoots,
    /// Keeps the temp directory, and the lease, for the checks.
    _keep: Box<dyn std::any::Any>,
}

/// What the row's own checks compare against.
enum Movements {
    /// The live Farm: exactly these ids are gone (decision 20), none is.
    Lost(BTreeSet<String>),
    /// A replaced Farm: exactly these ids exist.
    Exactly(BTreeSet<String>),
}

struct Done {
    result: Result<Value, Value>,
    counts_before: BTreeMap<String, i64>,
    movements_before: BTreeSet<String>,
    movements: Movements,
    site: Site,
}

/// The state a live row compares against, taken right before its action
/// (after any setup).
struct Before {
    counts: BTreeMap<String, i64>,
    movements: BTreeSet<String>,
    lost: BTreeSet<String>,
}

fn before(farm: &Farm, deleted_printer: Option<&str>) -> Before {
    let connection = open(farm);
    Before {
        counts: farm.counts(),
        movements: movements(&connection),
        lost: deleted_printer.map_or_else(BTreeSet::new, |printer| {
            movements_through(&connection, printer)
        }),
    }
}

/// A live row: `act` runs against `farm` through a fresh rig; the checks
/// read the same Farm afterwards.
fn live(
    farm: Farm,
    deleted_printer: Option<&str>,
    setup: impl FnOnce(&Farm, &Rig),
    act: impl FnOnce(&Farm, &Rig) -> Result<Value, Value>,
) -> Done {
    live_with(farm, None, deleted_printer, setup, act)
}

/// [`live`], with the rig's open-backup dialog returning `backup`.
fn live_with(
    farm: Farm,
    backup: Option<PathBuf>,
    deleted_printer: Option<&str>,
    setup: impl FnOnce(&Farm, &Rig),
    act: impl FnOnce(&Farm, &Rig) -> Result<Value, Value>,
) -> Done {
    let rig = Rig::new(&farm, backup);
    setup(&farm, &rig);
    let taken = before(&farm, deleted_printer);
    let result = act(&farm, &rig);
    drop(rig);
    let roots = IntegrityRoots::from_paths(farm.paths());
    Done {
        result,
        counts_before: taken.counts,
        movements_before: taken.movements,
        movements: Movements::Lost(taken.lost),
        site: Site {
            storage: Arc::clone(&farm.storage),
            roots,
            _keep: Box::new(farm),
        },
    }
}

/// Drops the rig and the Farm's `Storage` handle, then starts the Farm the
/// way the app does after its restart (`open_storage`: the restore
/// installer, then `Storage::open`), and returns the started Farm.
///
/// The mock runtime's background tasks (the camera janitor, the Job
/// driver, the attention task) keep their own `Storage` handles for the
/// life of the test process, where the app's restart would end them. The
/// command that wrote the journal holds the backup lease, so they make no
/// deletions, and the installer moves the database files they hold aside.
fn restart(farm: Farm, rig: Rig) -> Site {
    assert!(
        rig.restarter.wait_for(1, Duration::from_secs(10)),
        "the restart was requested"
    );
    let Rig {
        app,
        webview,
        services,
        restarter,
    } = rig;
    drop(webview);
    drop(services);
    drop(restarter);
    drop(app);
    let Farm {
        temp,
        metadata_lease,
        storage,
        credentials_dir,
    } = farm;
    let paths = storage.paths().clone();
    drop(storage);
    let slot = Mutex::new(Some(metadata_lease));
    let (storage, report) = farm3d_lib::open_storage(paths.clone(), &slot, || {
        CredentialStore::file_backed(credentials_dir.clone())
    })
    .map_err(|_| "startup failed")
    .unwrap();
    assert_eq!(report.outcome, InstallOutcome::Installed, "{report:?}");
    Site {
        storage,
        roots: IntegrityRoots::from_paths(&paths),
        _keep: Box::new((temp, slot)),
    }
}

// --- the actions --------------------------------------------------------------------

fn archive(rig: &Rig, farm: &Farm, printer: &str, dispositions: Value) -> Result<Value, Value> {
    rig.call(
        "archive_printer",
        json!({
            "operationId": format!("op-archive-{printer}"),
            "id": printer,
            "expectedRevision": revision(farm, "printers", printer),
            "spoolDispositions": dispositions,
        }),
    )
}

/// Printer B's archive disposition: its loaded Spool back to storage.
fn b_dispositions(farm: &Farm) -> Value {
    json!([{
        "spoolId": ids::SPOOL_LOADED,
        "expectedSpoolRevision": revision(farm, "spools", ids::SPOOL_LOADED),
        "disposition": { "kind": "storage", "storageLabel": "Shelf 2" },
    }])
}

/// Printer B's open Queue Entry is pinned to it; removing it first leaves
/// only the loaded Spool between Printer B and its archive.
fn remove_open_entry(farm: &Farm, rig: &Rig) {
    remove_entry(farm, rig, ids::QUEUE_OPEN).expect("remove the pinned entry");
}

fn remove_entry(farm: &Farm, rig: &Rig, entry: &str) -> Result<Value, Value> {
    rig.call(
        "remove_queue_entry",
        json!({
            "operationId": format!("op-remove-{entry}"),
            "entryId": entry,
            "expectedRevision": revision(farm, "queue_entries", entry),
        }),
    )
}

/// Printers import's repository step (`import_printers` reads a document
/// first), replacing every Printer with none.
fn import_nothing(farm: &Farm) -> Result<Value, Value> {
    let printers = PrinterRepository::new(Arc::clone(&farm.storage));
    let mut current: Vec<(String, i64)> = printers
        .list()
        .unwrap()
        .into_iter()
        .map(|printer| (printer.id, printer.revision))
        .collect();
    current.sort_by(|a, b| a.0.as_bytes().cmp(b.0.as_bytes()));
    printers
        .replace_all(&current, Vec::new(), &HashMap::new())
        .map(|_| Value::Null)
        .map_err(|error| serde_json::to_value(CommandError::from_repository(error)).unwrap())
}

fn unarchive(rig: &Rig, farm: &Farm, printer: &str) -> Result<Value, Value> {
    rig.call(
        "unarchive_printer",
        json!({ "id": printer, "expectedRevision": revision(farm, "printers", printer) }),
    )
}

fn delete_printer(rig: &Rig, farm: &Farm, printer: &str) -> Result<Value, Value> {
    rig.call(
        "delete_printer",
        json!({ "id": printer, "expectedRevision": revision(farm, "printers", printer) }),
    )
}

fn spool_lifecycle(
    rig: &Rig,
    farm: &Farm,
    spool: &str,
    action: &str,
    storage_label: Option<&str>,
) -> Result<Value, Value> {
    rig.call(
        "set_spool_lifecycle",
        json!({
            "operationId": format!("op-{action}-{spool}"),
            "id": spool,
            "expectedRevision": revision(farm, "spools", spool),
            "action": action,
            "storageLabel": storage_label,
        }),
    )
}

fn delete_slice_revision(rig: &Rig, revision_id: &str) -> Result<Value, Value> {
    rig.call(
        "delete_slice_revision",
        json!({ "sliceRevisionId": revision_id }),
    )
}

fn delete_model(rig: &Rig, farm: &Farm, model: &str) -> Result<Value, Value> {
    rig.call(
        "delete_model",
        json!({ "id": model, "expectedRevision": revision(farm, "library_models", model) }),
    )
}

fn pin(rig: &Rig, snapshot: &str, pinned: bool) -> Result<Value, Value> {
    rig.call(
        "set_snapshot_pinned",
        json!({
            "operationId": format!("op-pin-{snapshot}-{pinned}"),
            "snapshotId": snapshot,
            "pinned": pinned,
        }),
    )
}

fn clear(rig: &Rig, target: &str) -> Result<Value, Value> {
    rig.call(
        "clear_storage",
        json!({ "operationId": format!("op-clear-{target}"), "target": target }),
    )
}

fn reset(rig: &Rig, request: Value, confirmation: &str) -> Result<Value, Value> {
    rig.call(
        "reset_farm",
        json!({
            "operationId": "op-reset",
            "request": request,
            "confirmation": confirmation,
        }),
    )
}

/// `2026-03-01`: two months after the Farm's snapshots were captured, past
/// the default 30-day retention.
fn later() -> DateTime<Utc> {
    "2026-03-01T00:00:00Z".parse().unwrap()
}

/// Resolves the Farm's only restore blocker (`hop-a`, `dispatching`).
fn resolve_blocker(farm: &Farm) {
    sql(
        farm,
        &format!(
            "UPDATE host_operations SET state = 'failed', resolved_at = '{now}',
                    failure_json = '{{\"code\":\"neverSent\",\"message\":\"x\"}}'
              WHERE id = '{id}';",
            now = ids::NOW,
            id = ids::HOST_OPERATION_UNRESOLVED,
        ),
    );
}

fn write_archive(farm: &Farm) -> PathBuf {
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    let directory = farm.temp.path().join("exports");
    fs::create_dir_all(&directory).unwrap();
    let path = directory.join(format!("backup-{}.farm3d-backup", uuid::Uuid::new_v4()));
    write_backup(
        &farm.storage,
        &guard,
        &path,
        &BackupRequest {
            media: BackupMediaChoice::All,
            origin: BackupOrigin::Operator,
            created_at: Some(CREATED_AT.parse().unwrap()),
            app_version: "0.1.0".to_string(),
        },
        &WriterHooks::default(),
    )
    .expect("backup");
    path
}

/// `preview_restore` then `apply_restore` of `archive` on `farm`, then a
/// restart. The Farm's movements must afterwards be exactly `expected`.
fn restore(farm: Farm, archive: PathBuf, expected: BTreeSet<String>) -> Done {
    let counts_before = farm.counts();
    let movements_before = movements(&open(&farm));
    let rig = Rig::new(&farm, Some(archive));
    let preview = rig.ok("preview_restore", json!({ "source": { "kind": "file" } }));
    assert_eq!(preview["status"], "previewed", "{preview}");
    let staging_id = preview["preview"]["stagingId"]
        .as_str()
        .unwrap()
        .to_string();
    let result = rig.call(
        "apply_restore",
        json!({
            "operationId": "op-apply",
            "stagingId": staging_id,
            "confirmation": "restore",
        }),
    );
    assert_eq!(result.as_ref().unwrap()["status"], "restarting");
    let site = restart(farm, rig);
    Done {
        result,
        counts_before,
        movements_before,
        movements: Movements::Exactly(expected),
        site,
    }
}

// --- the rows -----------------------------------------------------------------------

enum Expect {
    Ok,
    /// `LIFECYCLE_BLOCKED` with exactly these blocker codes; no table's
    /// row count changes.
    Blocked(&'static [&'static str]),
    /// Refused with this error code; no table's row count changes.
    Refused(&'static str),
}

struct Row {
    name: &'static str,
    act: fn(Farm) -> Done,
    expect: Expect,
    /// Exactly the tolerated findings afterwards: `(rule, sample)`.
    tolerated: &'static [(IntegrityRule, &'static [&'static str])],
    /// The row's own effect, checked on the Farm afterwards (proof the
    /// action did what the row names); `None` for a row with nothing of its
    /// own to check (a refusal, whose unchanged Farm is checked instead).
    effect: Option<fn(&Connection, &Site, &Value)>,
}

fn one<T: rusqlite::types::FromSql>(connection: &Connection, statement: &str) -> T {
    connection
        .query_row(statement, [], |row| row.get(0))
        .unwrap_or_else(|error| panic!("{statement}: {error}"))
}

fn exists(connection: &Connection, table: &str, id: &str) -> bool {
    one::<i64>(
        connection,
        &format!("SELECT count(*) FROM {table} WHERE id = '{id}'"),
    ) == 1
}

/// The pre-import settings snapshot databases (not their `-wal`/`-shm`).
fn pre_import_snapshots(paths: &StoragePaths) -> Vec<String> {
    let Ok(entries) = fs::read_dir(paths.snapshot_root()) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.starts_with(".farm3d-pre-import-settings-") && name.ends_with(".sqlite3")
        })
        .collect();
    names.sort();
    names
}

fn blob_file(site: &Site, sha256: &str) -> PathBuf {
    site.roots
        .content_root
        .join("blobs/sha256")
        .join(&sha256[..2])
        .join(sha256)
}

/// A deleted Printer: gone with its setup rows and slots; its open Events
/// resolved `sourceRemoved`, every Event of it keeps its row with
/// `printer_id` NULL; its Spools stay, in no slot.
fn printer_deleted(connection: &Connection, printer: &str) {
    assert!(!exists(connection, "printers", printer));
    for table in [
        "material_slots",
        "printer_cameras",
        "printer_alert_defaults",
    ] {
        assert_eq!(
            one::<i64>(
                connection,
                &format!("SELECT count(*) FROM {table} WHERE printer_id = '{printer}'")
            ),
            0,
            "{table}"
        );
    }
    assert_eq!(
        one::<i64>(
            connection,
            &format!(
                "SELECT count(*) FROM attention_events
                  WHERE source_kind = 'printer' AND source_id = '{printer}'
                    AND (resolved_at IS NULL OR printer_id IS NOT NULL)"
            )
        ),
        0
    );
}

fn rows() -> Vec<Row> {
    use IntegrityRule::*;
    vec![
        // --- Printers ---------------------------------------------------------------
        Row {
            name: "printer archive, blocked by an unresolved Host Operation",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| archive(rig, farm, ids::PRINTER_A, json!([])),
                )
            },
            expect: Expect::Blocked(&["HOST_OPERATION_UNRESOLVED"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "printer archive, blocked by a loaded Spool",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| archive(rig, farm, ids::PRINTER_B, json!([])),
                )
            },
            expect: Expect::Blocked(&["SPOOLS_LOADED"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "printer archive, allowed with a disposition",
            act: |farm| {
                live(farm, None, remove_open_entry, |farm, rig| {
                    archive(rig, farm, ids::PRINTER_B, b_dispositions(farm))
                })
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert!(one::<Option<String>>(
                    connection,
                    &format!(
                        "SELECT archived_at FROM printers WHERE id = '{}'",
                        ids::PRINTER_B
                    )
                )
                .is_some());
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT slot_id FROM spools WHERE id = '{}'",
                            ids::SPOOL_LOADED
                        )
                    ),
                    None
                );
            }),
        },
        Row {
            name: "printer unarchive, allowed",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| unarchive(rig, farm, ids::PRINTER_ARCHIVED),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT archived_at FROM printers WHERE id = '{}'",
                            ids::PRINTER_ARCHIVED
                        )
                    ),
                    None
                );
            }),
        },
        Row {
            name: "printer unarchive, blocked (not archived)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| unarchive(rig, farm, ids::PRINTER_B),
                )
            },
            expect: Expect::Blocked(&["NOT_ARCHIVED"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "printer delete, allowed (the archived Printer; decision 20's loss)",
            act: |farm| {
                live(
                    farm,
                    Some(ids::PRINTER_ARCHIVED),
                    |_, _| {},
                    |farm, rig| delete_printer(rig, farm, ids::PRINTER_ARCHIVED),
                )
            },
            expect: Expect::Ok,
            tolerated: &[(SliceTargetPrinter, &[ids::SLICE_REVISION_FARM3D])],
            effect: Some(|connection, _, _| {
                printer_deleted(connection, ids::PRINTER_ARCHIVED);
                assert!(exists(connection, "spools", ids::SPOOL_ARCHIVED));
            }),
        },
        Row {
            name: "printer delete, allowed after archive (Printer B; decision 20's loss)",
            act: |farm| {
                live(
                    farm,
                    Some(ids::PRINTER_B),
                    |farm, rig| {
                        remove_open_entry(farm, rig);
                        archive(rig, farm, ids::PRINTER_B, b_dispositions(farm))
                            .expect("archive Printer B");
                    },
                    |farm, rig| delete_printer(rig, farm, ids::PRINTER_B),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                printer_deleted(connection, ids::PRINTER_B);
                // The Event resolved before the delete keeps its resolution.
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT resolution FROM attention_events WHERE id = '{}'",
                            ids::ATTENTION_RESOLVED
                        )
                    ),
                    "conditionCleared"
                );
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT resolution FROM attention_events WHERE id = '{}'",
                            ids::ATTENTION_RECURRENCE
                        )
                    ),
                    "sourceRemoved"
                );
                // Its terminal upload went with it.
                assert!(!exists(
                    connection,
                    "host_operations",
                    ids::HOST_OPERATION_FAILED
                ));
            }),
        },
        Row {
            name: "printer delete, blocked (an open Queue Entry is pinned to it)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, rig| {
                        archive(rig, farm, ids::PRINTER_B, b_dispositions(farm))
                            .expect("archive Printer B");
                    },
                    |farm, rig| delete_printer(rig, farm, ids::PRINTER_B),
                )
            },
            expect: Expect::Blocked(&["QUEUE_ENTRY_PINNED"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "printer delete, blocked (active, with history)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| delete_printer(rig, farm, ids::PRINTER_A),
                )
            },
            expect: Expect::Blocked(&[
                "NOT_ARCHIVED",
                "HOST_OPERATION_UNRESOLVED",
                "JOB_HISTORY_EXISTS",
                "INCIDENT_HISTORY_EXISTS",
                "PINNED_EVIDENCE_EXISTS",
            ]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "printers import (replace all), refused while a Spool is loaded",
            act: |farm| live(farm, None, |_, _| {}, |farm, _| import_nothing(farm)),
            expect: Expect::Refused("VALIDATION"),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "printers import (replace all), refused while Jobs exist",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, rig| {
                        spool_lifecycle(rig, farm, ids::SPOOL_LOADED, "markEmpty", Some("Bin"))
                            .expect("unload");
                    },
                    |farm, _| import_nothing(farm),
                )
            },
            expect: Expect::Refused("JOBS_EXIST"),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "printer camera clear",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| {
                        rig.call(
                        "clear_printer_camera",
                        json!({ "operationId": "op-clear-camera", "printerId": ids::PRINTER_B }),
                    )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<i64>(
                        connection,
                        &format!(
                            "SELECT count(*) FROM printer_cameras WHERE printer_id = '{}'",
                            ids::PRINTER_B
                        )
                    ),
                    0
                );
            }),
        },
        Row {
            name: "printer connection clear (its ref is queued for cleanup)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| {
                        rig.call(
                            "clear_printer_connection",
                            json!({
                                "id": ids::PRINTER_B,
                                "expectedRevision": revision(farm, "printers", ids::PRINTER_B),
                            }),
                        )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT connection_json FROM printers WHERE id = '{}'",
                            ids::PRINTER_B
                        )
                    ),
                    None
                );
            }),
        },
        Row {
            name: "material slot removal (an empty slot)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| {
                        rig.call(
                            "set_material_slot_layout",
                            json!({
                                "printerId": ids::PRINTER_B,
                                "expectedRevision": revision(farm, "printers", ids::PRINTER_B),
                                "slots": [{ "id": ids::SLOT_B1, "name": "A1" }],
                            }),
                        )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                // Soft-removed: the row stays for the movement history.
                assert!(one::<Option<String>>(
                    connection,
                    &format!(
                        "SELECT removed_at FROM material_slots WHERE id = '{}'",
                        ids::SLOT_B2
                    )
                )
                .is_some());
            }),
        },
        // --- Spools and tares -------------------------------------------------------
        Row {
            name: "spool archive, blocked (loaded)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| spool_lifecycle(rig, farm, ids::SPOOL_LOADED, "archive", None),
                )
            },
            expect: Expect::Blocked(&["SPOOLS_LOADED"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "spool archive, blocked (reserved)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| spool_lifecycle(rig, farm, ids::SPOOL_A, "archive", None),
                )
            },
            expect: Expect::Blocked(&["SPOOL_RESERVED"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "spool archive, allowed (emptied into storage first)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, rig| {
                        spool_lifecycle(rig, farm, ids::SPOOL_LOADED, "markEmpty", Some("Bin"))
                            .expect("mark empty");
                    },
                    |farm, rig| spool_lifecycle(rig, farm, ids::SPOOL_LOADED, "archive", None),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT lifecycle FROM spools WHERE id = '{}'",
                            ids::SPOOL_LOADED
                        )
                    ),
                    "archived"
                );
            }),
        },
        Row {
            name: "spool mark empty (unloads it)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| {
                        spool_lifecycle(rig, farm, ids::SPOOL_LOADED, "markEmpty", Some("Bin"))
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT lifecycle FROM spools WHERE id = '{}'",
                            ids::SPOOL_LOADED
                        )
                    ),
                    "empty"
                );
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT slot_id FROM spools WHERE id = '{}'",
                            ids::SPOOL_LOADED
                        )
                    ),
                    None
                );
            }),
        },
        Row {
            name: "spool reactivate",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, rig| {
                        spool_lifecycle(rig, farm, ids::SPOOL_LOADED, "markEmpty", Some("Bin"))
                            .expect("mark empty");
                    },
                    |farm, rig| spool_lifecycle(rig, farm, ids::SPOOL_LOADED, "reactivate", None),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT lifecycle FROM spools WHERE id = '{}'",
                            ids::SPOOL_LOADED
                        )
                    ),
                    "active"
                );
            }),
        },
        Row {
            name: "spool unarchive",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| spool_lifecycle(rig, farm, ids::SPOOL_ARCHIVED, "unarchive", None),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT lifecycle FROM spools WHERE id = '{}'",
                            ids::SPOOL_ARCHIVED
                        )
                    ),
                    "active"
                );
            }),
        },
        Row {
            name: "tare delete (clears the Spool's tare)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| {
                        rig.call(
                            "delete_tare",
                            json!({
                                "id": ids::TARE,
                                "expectedRevision": revision(farm, "spool_tares", ids::TARE),
                            }),
                        )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert!(!exists(connection, "spool_tares", ids::TARE));
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT tare_id FROM spools WHERE id = '{}'",
                            ids::SPOOL_ARCHIVED
                        )
                    ),
                    None
                );
            }),
        },
        // --- Library ----------------------------------------------------------------
        Row {
            name: "project delete",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| {
                        rig.call(
                        "delete_project",
                        json!({
                            "id": ids::PROJECT,
                            "expectedRevision": revision(farm, "library_projects", ids::PROJECT),
                        }),
                    )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert!(!exists(connection, "library_projects", ids::PROJECT));
                assert_eq!(
                    one::<i64>(connection, "SELECT count(*) FROM project_models"),
                    0
                );
                assert!(exists(connection, "library_models", ids::MODEL_GCODE));
            }),
        },
        Row {
            name: "model delete, allowed (its blobs are released)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| delete_model(rig, farm, ids::MODEL_3MF),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, site, _| {
                assert!(!exists(connection, "library_models", ids::MODEL_3MF));
                assert_eq!(
                    one::<i64>(
                        connection,
                        &format!(
                            "SELECT count(*) FROM model_source_revisions WHERE model_id = '{}'",
                            ids::MODEL_3MF
                        )
                    ),
                    0
                );
                let source = p9_farm::sha256_hex(&p9_farm::three_mf_bytes("source"));
                let thumbnail = p9_farm::sha256_hex(&p9_farm::thumbnail_bytes());
                for sha256 in [source, thumbnail] {
                    assert_eq!(
                        one::<i64>(
                            connection,
                            &format!(
                                "SELECT count(*) FROM content_blobs WHERE sha256 = '{sha256}'"
                            )
                        ),
                        0
                    );
                    assert!(!blob_file(site, &sha256).exists(), "{sha256} unlinked");
                }
            }),
        },
        Row {
            name: "model delete, blocked (its Slice Revisions)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| delete_model(rig, farm, ids::MODEL_GCODE),
                )
            },
            expect: Expect::Blocked(&["SLICE_REVISIONS_EXIST"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "model delete, blocked (the linked Model's farm3d Slice Revision)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| delete_model(rig, farm, ids::MODEL_LINKED),
                )
            },
            expect: Expect::Blocked(&["SLICE_REVISIONS_EXIST"]),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "preparation delete",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| {
                        rig.call(
                        "delete_preparation",
                        json!({
                            "preparationId": ids::PREPARATION,
                            "expectedRevision": revision(farm, "slice_preparations", ids::PREPARATION),
                        }),
                    )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert!(!exists(connection, "slice_preparations", ids::PREPARATION));
            }),
        },
        // --- Slice Revisions --------------------------------------------------------
        Row {
            name: "slice revision delete, allowed (farm3d)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| delete_slice_revision(rig, ids::SLICE_REVISION_FARM3D),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert!(!exists(
                    connection,
                    "slice_revisions",
                    ids::SLICE_REVISION_FARM3D
                ));
                // The shared G-code blob stays: other revisions hold it.
                assert!(
                    one::<i64>(
                        connection,
                        &format!(
                            "SELECT count(*) FROM content_blobs WHERE sha256 = '{}'",
                            p9_farm::farm_seed::GCODE_HASH
                        )
                    ) == 1
                );
            }),
        },
        Row {
            name: "slice revision delete, allowed (external, a terminal upload of it)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| delete_slice_revision(rig, ids::SLICE_REVISION_TARGETED),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert!(!exists(
                    connection,
                    "slice_revisions",
                    ids::SLICE_REVISION_TARGETED
                ));
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT slice_revision_id FROM host_operations WHERE id = '{}'",
                            ids::HOST_OPERATION_FAILED
                        )
                    ),
                    None
                );
            }),
        },
        Row {
            name: "slice revision delete, blocked (Queue Entries and Jobs)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| delete_slice_revision(rig, ids::SLICE_REVISION),
                )
            },
            expect: Expect::Blocked(&["QUEUE_REFERENCES_REVISION"]),
            tolerated: &[],
            effect: None,
        },
        // --- Queue ------------------------------------------------------------------
        Row {
            name: "queue entry remove (open)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| remove_entry(farm, rig, ids::QUEUE_OPEN),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT close_reason FROM queue_entries WHERE id = '{}'",
                            ids::QUEUE_OPEN
                        )
                    ),
                    "removed"
                );
            }),
        },
        Row {
            name: "queue entry remove (a retry successor)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| remove_entry(farm, rig, ids::QUEUE_RETRY),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!(
                            "SELECT close_reason FROM queue_entries WHERE id = '{}'",
                            ids::QUEUE_RETRY
                        )
                    ),
                    "removed"
                );
                assert!(exists(connection, "queue_entries", ids::QUEUE_FAILED));
            }),
        },
        // --- Camera media -----------------------------------------------------------
        Row {
            name: "media prune (the retention pass)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |farm, rig| {
                        let policy = farm
                            .storage
                            .read(|connection| media::read_policy(connection))
                            .unwrap();
                        let changes = tauri::async_runtime::block_on(media::prune_pass(
                            &farm.storage,
                            rig.services.cameras.janitor(),
                            policy,
                            later(),
                        ))
                        .map_err(|error| json!(format!("{error:?}")))?;
                        Ok(json!(changes.snapshots.len()))
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, site, _| {
                for id in [ids::SNAPSHOT_INCIDENT, ids::SNAPSHOT_MANUAL] {
                    assert_eq!(
                        one::<Option<String>>(
                            connection,
                            &format!("SELECT prune_reason FROM camera_snapshots WHERE id = '{id}'")
                        )
                        .as_deref(),
                        Some("age"),
                        "{id}"
                    );
                    assert!(!site.roots.media_root.join(snapshot_rel_path(id)).exists());
                }
                // The pinned one stays; the Incident's timeline records it.
                assert!(site
                    .roots
                    .media_root
                    .join(snapshot_rel_path(ids::SNAPSHOT_PINNED))
                    .is_file());
                assert_eq!(
                    one::<i64>(
                        connection,
                        "SELECT count(*) FROM incident_events WHERE kind = 'evidencePruned'"
                    ),
                    1
                );
            }),
        },
        Row {
            name: "media pin",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| pin(rig, ids::SNAPSHOT_MANUAL, true),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert!(one::<Option<String>>(
                    connection,
                    &format!(
                        "SELECT pinned_at FROM camera_snapshots WHERE id = '{}'",
                        ids::SNAPSHOT_MANUAL
                    )
                )
                .is_some());
            }),
        },
        Row {
            name: "media unpin",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| pin(rig, ids::SNAPSHOT_PINNED, false),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT pinned_at FROM camera_snapshots WHERE id = '{}'",
                            ids::SNAPSHOT_PINNED
                        )
                    ),
                    None
                );
            }),
        },
        Row {
            name: "retention sweep (a missing file and an orphan file)",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, _| {
                        fs::remove_file(farm.media_file(&snapshot_rel_path(ids::SNAPSHOT_MANUAL)))
                            .unwrap();
                        let orphan = farm.media_file(&snapshot_rel_path("snp-orphan"));
                        fs::write(orphan, b"orphan").unwrap();
                    },
                    |farm, rig| {
                        let changes = tauri::async_runtime::block_on(media::sweep_under_lock(
                            &farm.storage,
                            rig.services.cameras.janitor(),
                            later(),
                        ))
                        .map_err(|error| json!(format!("{error:?}")))?;
                        Ok(json!(changes.snapshots.len()))
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, site, _| {
                assert_eq!(
                    one::<Option<String>>(
                        connection,
                        &format!(
                            "SELECT prune_reason FROM camera_snapshots WHERE id = '{}'",
                            ids::SNAPSHOT_MANUAL
                        )
                    )
                    .as_deref(),
                    Some("missingFile")
                );
                assert!(!site
                    .roots
                    .media_root
                    .join(snapshot_rel_path("snp-orphan"))
                    .exists());
            }),
        },
        // --- Storage cleanup --------------------------------------------------------
        Row {
            name: "clear storage: unreferenced content",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| clear(rig, "unreferencedContent"),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, site, _| {
                assert_eq!(
                    one::<i64>(connection, "SELECT count(*) FROM pending_blob_cleanup"),
                    0
                );
                assert!(!blob_file(site, ids::PENDING_BLOB).exists());
            }),
        },
        Row {
            name: "clear storage: pre-import snapshots",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, _| {
                        // Two, a few milliseconds apart, so the newer one's
                        // name (which leads with its time) sorts last.
                        for _ in 0..2 {
                            farm.storage
                                .create_snapshot(SnapshotKind::Settings)
                                .unwrap();
                            std::thread::sleep(Duration::from_millis(5));
                        }
                        assert_eq!(pre_import_snapshots(farm.paths()).len(), 2);
                    },
                    |farm, rig| {
                        let newest = pre_import_snapshots(farm.paths()).pop().unwrap();
                        clear(rig, "preImportSnapshots").map(|mut outcome| {
                            outcome["newest"] = json!(newest);
                            outcome
                        })
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|_, site, result| {
                assert_eq!(result["removedCount"], 1, "{result}");
                // The newest valid snapshot stays.
                let left = pre_import_snapshots(site.storage.paths());
                assert_eq!(left.len(), 1, "{left:?}");
                assert_eq!(result["newest"], left[0].as_str(), "the newest stays");
            }),
        },
        Row {
            name: "clear storage: rotated logs",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, _| {
                        let logs = farm.paths().log_root();
                        fs::create_dir_all(logs).unwrap();
                        fs::write(logs.join("farm3d.log"), b"{}\n").unwrap();
                        fs::write(logs.join("farm3d.1.log"), b"{}\n").unwrap();
                    },
                    |_, rig| clear(rig, "rotatedLogs"),
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|_, site, result| {
                assert_eq!(result["removedCount"], 1, "{result}");
                let logs = site.storage.paths().log_root();
                assert!(
                    !logs.join("farm3d.1.log").exists(),
                    "the rotated log is gone"
                );
                assert!(logs.join("farm3d.log").is_file(), "the active log stays");
            }),
        },
        Row {
            name: "clear storage: OrcaSlicer profile cache",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, rig| {
                        let cache = rig.services.slicing.cache_dir().join("orca-profiles");
                        fs::create_dir_all(&cache).unwrap();
                        fs::write(cache.join("profile.json"), b"{}").unwrap();
                    },
                    |_, rig| {
                        let cache = rig.services.slicing.cache_dir().join("orca-profiles");
                        clear(rig, "orcaCache").map(|mut outcome| {
                            // The cache lives outside the Farm's roots; report
                            // whether it survived for the effect check.
                            outcome["cacheLeft"] = json!(cache.exists());
                            outcome
                        })
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|_, _, result| {
                assert_eq!(result["removedCount"], 1, "{result}");
                assert_eq!(result["freedBytes"], 2, "{result}");
                assert_eq!(result["cacheLeft"], false, "the cache is gone");
            }),
        },
        // --- Resets -----------------------------------------------------------------
        Row {
            name: "reset (a): settings",
            act: |farm| {
                live(
                    farm,
                    None,
                    |farm, _| {
                        sql(
                            farm,
                            "UPDATE settings SET revision = revision + 1,
                                    theme_mode = 'farm3d-dark', snapshot_retention_days = 7;",
                        );
                    },
                    |farm, rig| {
                        let revision: i64 = farm
                            .storage
                            .read(|c| {
                                c.query_row("SELECT revision FROM settings", [], |r| r.get(0))
                            })
                            .unwrap();
                        reset(
                            rig,
                            json!({ "tier": "settings", "expectedRevision": revision }),
                            "reset settings",
                        )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                // The settings are the defaults again.
                assert_eq!(
                    one::<String>(connection, "SELECT theme_mode FROM settings"),
                    "system"
                );
                assert_eq!(
                    one::<i64>(connection, "SELECT snapshot_retention_days FROM settings"),
                    30
                );
                // Tier (a) leaves the Slicer runtime and alert defaults alone.
                assert!(one::<Option<String>>(
                    connection,
                    "SELECT engine_path FROM slicer_runtime_config"
                )
                .is_some());
                assert_eq!(
                    connection
                        .query_row(
                            &format!(
                                "SELECT offline_after_minutes, notifications
                                   FROM printer_alert_defaults WHERE printer_id = '{}'",
                                ids::PRINTER_B
                            ),
                            [],
                            |row| Ok((row.get::<_, i64>(0)?, row.get::<_, String>(1)?)),
                        )
                        .unwrap(),
                    (5, "follow".to_string())
                );
                assert_eq!(
                    one::<i64>(connection, "SELECT count(*) FROM printer_alert_defaults"),
                    2
                );
            }),
        },
        Row {
            name: "reset (b): camera media, unpinned",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| {
                        reset(
                            rig,
                            json!({ "tier": "cameraMedia", "scope": "unpinned" }),
                            "reset media",
                        )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<i64>(
                        connection,
                        "SELECT count(*) FROM camera_snapshots
                          WHERE pinned_at IS NULL AND pruned_at IS NULL"
                    ),
                    0
                );
                assert_eq!(
                    one::<i64>(connection, "SELECT count(*) FROM camera_snapshots"),
                    4
                );
            }),
        },
        Row {
            name: "reset (b): camera media, all",
            act: |farm| {
                live(
                    farm,
                    None,
                    |_, _| {},
                    |_, rig| {
                        reset(
                            rig,
                            json!({ "tier": "cameraMedia", "scope": "all" }),
                            "reset media",
                        )
                    },
                )
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                assert_eq!(
                    one::<i64>(
                        connection,
                        "SELECT count(*) FROM camera_snapshots WHERE pruned_at IS NULL"
                    ),
                    0
                );
                assert_eq!(
                    one::<i64>(connection, "SELECT count(*) FROM camera_snapshots"),
                    4
                );
            }),
        },
        Row {
            name: "reset (c): the Farm, through the startup installer",
            act: |farm| {
                let counts_before = farm.counts();
                let movements_before = movements(&open(&farm));
                let rig = Rig::new(&farm, None);
                let result = reset(
                    &rig,
                    json!({ "tier": "farm", "safetyBackup": true, "deleteSafetyBackups": false }),
                    "reset farm",
                );
                let site = restart(farm, rig);
                Done {
                    result,
                    counts_before,
                    movements_before,
                    movements: Movements::Exactly(BTreeSet::new()),
                    site,
                }
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                for table in [
                    "printers",
                    "spools",
                    "library_models",
                    "slice_revisions",
                    "queue_entries",
                    "jobs",
                    "attention_events",
                    "incidents",
                    "camera_snapshots",
                    "host_operations",
                    "content_blobs",
                ] {
                    assert_eq!(
                        one::<i64>(connection, &format!("SELECT count(*) FROM {table}")),
                        0,
                        "{table}"
                    );
                }
            }),
        },
        // --- Restores ---------------------------------------------------------------
        Row {
            name: "restore, refused while a Host Operation is unresolved",
            act: |farm| {
                let archive = write_archive(&farm);
                // The preview (the setup) releases the Farm's pending blob
                // when its lease drops; the refusal is measured after it.
                let staging = std::cell::RefCell::new(Value::Null);
                live_with(
                    farm,
                    Some(archive),
                    None,
                    |_, rig| {
                        let preview =
                            rig.ok("preview_restore", json!({ "source": { "kind": "file" } }));
                        assert_eq!(preview["status"], "previewed", "{preview}");
                        *staging.borrow_mut() = preview["preview"]["stagingId"].clone();
                    },
                    |_, rig| {
                        rig.call(
                            "apply_restore",
                            json!({
                                "operationId": "op-apply",
                                "stagingId": staging.borrow().clone(),
                                "confirmation": "restore",
                            }),
                        )
                    },
                )
            },
            expect: Expect::Refused("RESTORE_BLOCKED"),
            tolerated: &[],
            effect: None,
        },
        Row {
            name: "restore of the Farm's own backup",
            act: |farm| {
                resolve_blocker(&farm);
                let archive = write_archive(&farm);
                let expected = movements(&open(&farm));
                restore(farm, archive, expected)
            },
            expect: Expect::Ok,
            tolerated: &[],
            effect: Some(|connection, _, _| {
                for (table, count) in [
                    ("printers", 3),
                    ("jobs", 3),
                    ("spool_movements", 3),
                    ("camera_snapshots", 4),
                    ("content_blobs", 6),
                    ("attention_events", 5),
                ] {
                    assert_eq!(
                        one::<i64>(connection, &format!("SELECT count(*) FROM {table}")),
                        count,
                        "{table}"
                    );
                }
            }),
        },
        Row {
            name: "restore of a conflicting backup (replace-only)",
            act: |farm| {
                // The backup's Farm: Printer B renamed, the archived
                // Printer and its movements gone, a Project of its own.
                let remote = Farm::with_every_domain();
                sql(
                    &remote,
                    &format!(
                        "UPDATE printers SET name = 'Bravo (remote)', revision = revision + 1
                           WHERE id = '{b}';
                         INSERT INTO library_projects(id, revision, name, created_at, updated_at)
                           VALUES ('prj-remote', 1, 'Remote only', '{now}', '{now}');",
                        b = ids::PRINTER_B,
                        now = ids::NOW,
                    ),
                );
                let rig = Rig::new(&remote, None);
                resolve_blocker(&remote);
                delete_printer(&rig, &remote, ids::PRINTER_ARCHIVED).expect("remote delete");
                drop(rig);
                let archive = write_archive(&remote);
                let expected = movements(&open(&remote));
                resolve_blocker(&farm);
                sql(
                    &farm,
                    &format!(
                        "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
                           VALUES ('prj-local', 1, 'Local only', '{now}', '{now}');",
                        now = ids::NOW,
                    ),
                );
                let done = restore(farm, archive, expected);
                drop(remote);
                done
            },
            expect: Expect::Ok,
            tolerated: &[(SliceTargetPrinter, &[ids::SLICE_REVISION_FARM3D])],
            effect: Some(|connection, _, _| {
                assert!(exists(connection, "library_projects", "prj-remote"));
                assert!(!exists(connection, "library_projects", "prj-local"));
                assert!(!exists(connection, "printers", ids::PRINTER_ARCHIVED));
                assert_eq!(
                    one::<String>(
                        connection,
                        &format!("SELECT name FROM printers WHERE id = '{}'", ids::PRINTER_B)
                    ),
                    "Bravo (remote)"
                );
            }),
        },
    ]
}

fn check_row(row: &Row, done: &Done) {
    let name = row.name;
    let connection = Connection::open(done.site.storage.paths().database()).unwrap();
    match row.expect {
        Expect::Ok => {
            if let Err(error) = &done.result {
                panic!("{name}: expected success, got {error}");
            }
        }
        Expect::Blocked(codes) => {
            let error = done
                .result
                .as_ref()
                .expect_err(&format!("{name}: expected LIFECYCLE_BLOCKED"));
            assert_eq!(error["code"], "LIFECYCLE_BLOCKED", "{name}: {error}");
            let actual: BTreeSet<&str> = error["details"]["blockers"]
                .as_array()
                .unwrap_or_else(|| panic!("{name}: blockers in {error}"))
                .iter()
                .map(|blocker| blocker["code"].as_str().unwrap())
                .collect();
            let expected: BTreeSet<&str> = codes.iter().copied().collect();
            assert_eq!(actual, expected, "{name}: {error}");
            assert_eq!(
                p9_farm::table_counts(&connection),
                done.counts_before,
                "{name}: a refusal changes nothing"
            );
        }
        Expect::Refused(code) => {
            let error = done
                .result
                .as_ref()
                .expect_err(&format!("{name}: expected {code}"));
            assert_eq!(error["code"], code, "{name}: {error}");
            assert_eq!(
                p9_farm::table_counts(&connection),
                done.counts_before,
                "{name}: a refusal changes nothing"
            );
        }
    }

    let report = integrity::check(&connection, Some(&done.site.roots)).unwrap();
    assert!(
        report.violations().is_empty(),
        "{name}: violations {:#?}",
        report.violations()
    );
    let tolerated: Vec<(IntegrityRule, Vec<String>)> = report
        .tolerated()
        .into_iter()
        .map(|finding| (finding.rule, finding.sample.clone()))
        .collect();
    let expected: Vec<(IntegrityRule, Vec<String>)> = row
        .tolerated
        .iter()
        .map(|(rule, sample)| (*rule, sample.iter().map(|id| id.to_string()).collect()))
        .collect();
    assert_eq!(tolerated, expected, "{name}: tolerated findings");

    if let Some(effect) = row.effect {
        effect(
            &connection,
            &done.site,
            done.result.as_ref().unwrap_or(&Value::Null),
        );
    }

    // Decision 20: the named allowed loss, and no other.
    let after = movements(&connection);
    match &done.movements {
        Movements::Lost(lost) => {
            let gone: BTreeSet<String> =
                done.movements_before.difference(&after).cloned().collect();
            assert_eq!(&gone, lost, "{name}: movements lost");
        }
        Movements::Exactly(expected) => assert_eq!(&after, expected, "{name}: movements"),
    }
}

#[test]
fn no_archive_delete_prune_cleanup_reset_or_restore_leaves_a_dangling_reference() {
    // The Farm itself is clean before any row runs.
    let farm = Farm::with_every_domain();
    let report = integrity::check(
        &open(&farm),
        Some(&IntegrityRoots::from_paths(farm.paths())),
    )
    .unwrap();
    assert!(report.findings.is_empty(), "{report:#?}");
    drop(farm);

    // Every row runs; the failures are reported together.
    // `P9_MATRIX_ROW=<part of a name>` runs only the matching rows.
    let only = std::env::var("P9_MATRIX_ROW").ok();
    let mut failed = Vec::new();
    for row in rows() {
        if only.as_deref().is_some_and(|part| !row.name.contains(part)) {
            continue;
        }
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            if matches!(row.expect, Expect::Ok) {
                assert!(
                    row.effect.is_some(),
                    "{}: a success row checks its own effect",
                    row.name
                );
            }
            let done = (row.act)(Farm::with_every_domain());
            check_row(&row, &done);
        }));
        if outcome.is_err() {
            failed.push(row.name);
        }
    }
    assert!(failed.is_empty(), "failing rows: {failed:#?}");
}
