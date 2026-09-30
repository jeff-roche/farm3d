//! P9 Task 10 (spec D14, D18): storage usage by class and cleanup of
//! reclaimable data.
//!
//! - `storage_usage` reports every class in the spec's order, and each
//!   class equals the apparent bytes on disk for a quiescent Farm
//!   (tolerance zero).
//! - `clear_storage` per target: `unreferencedContent`,
//!   `preImportSnapshots` (keeps the newest), `rotatedLogs` (never the
//!   active file), `orcaCache` (refused with `STORAGE_IN_USE` while a
//!   slice operation is queued or running). Each leaves `integrity::check`
//!   clean and never touches referenced data.
//! - D5/D18: the lease (`BACKUP_IN_PROGRESS`, activity `storageCleanup`)
//!   and the process-local ledger (replay, `VALIDATION` on a reuse).

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use tauri::test::MockRuntime;

use farm3d_lib::backup::lease::LeaseActivity;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::persistence::integrity::{self, IntegrityRoots};
use farm3d_lib::persistence::RepositoryError;
use farm3d_lib::RuntimeServices;

use p9_farm::{ids, sha256_hex, Farm};

const CLASSES: [&str; 13] = [
    "database",
    "contentModelSources",
    "contentThumbnails",
    "contentGcode",
    "contentSliceArtifacts",
    "contentSlicerLogs",
    "contentUnreferenced",
    "cameraMediaPinned",
    "cameraMediaUnpinned",
    "safetyBackups",
    "preImportSnapshots",
    "logs",
    "slicerProfileCache",
];

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

struct Rig {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
}

impl Rig {
    fn new(farm: &Farm) -> Rig {
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::diagnostics::commands::storage_usage,
                farm3d_lib::diagnostics::commands::clear_storage,
            ],
            Arc::clone(&farm.storage),
            Arc::new(common::a_catalog()),
            farm.credentials_dir.clone(),
            no_connection,
            |_| {},
        );
        Rig {
            _app: app,
            webview,
            services,
        }
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|envelope| envelope["data"].clone())
    }

    fn usage(&self) -> Value {
        self.call("storage_usage", json!({})).unwrap()
    }

    fn clear(&self, operation_id: &str, target: &str) -> Result<Value, Value> {
        self.call(
            "clear_storage",
            json!({ "operationId": operation_id, "target": target }),
        )
    }

    fn cache_dir(&self) -> PathBuf {
        self.services.slicing.cache_dir().join("orca-profiles")
    }
}

fn class(usage: &Value, name: &str) -> (u64, u64) {
    let row = usage["classes"]
        .as_array()
        .unwrap()
        .iter()
        .find(|row| row["class"] == name)
        .unwrap_or_else(|| panic!("class {name} in {usage}"));
    (
        row["bytes"].as_u64().unwrap(),
        row["count"].as_u64().unwrap(),
    )
}

fn bytes(usage: &Value, name: &str) -> u64 {
    class(usage, name).0
}

fn sql(farm: &Farm, statements: &str) {
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            tx.execute_batch(statements)
                .unwrap_or_else(|error| panic!("fixture SQL failed: {error}\n{statements}"));
            Ok(())
        })
        .expect("fixture write");
}

fn count(farm: &Farm, statement: &str) -> i64 {
    farm.storage
        .read(|connection| connection.query_row(statement, [], |row| row.get(0)))
        .unwrap()
}

fn tree_bytes(root: &Path) -> u64 {
    let mut total = 0;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).into_iter().flatten().flatten() {
            let metadata = fs::symlink_metadata(entry.path()).unwrap();
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                total += metadata.len();
            }
        }
    }
    total
}

fn write_file(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
}

fn assert_integrity_clean(farm: &Farm) {
    let connection = rusqlite::Connection::open(farm.paths().database()).unwrap();
    let report =
        integrity::check(&connection, Some(&IntegrityRoots::from_paths(farm.paths()))).unwrap();
    assert!(
        report.violations().is_empty(),
        "integrity violations: {:?}",
        report.violations()
    );
}

fn snapshot_name(timestamp: u128, tag: &str) -> String {
    format!(".farm3d-pre-import-settings-{timestamp:020}-{tag}.sqlite3")
}

fn names_in(directory: &Path) -> Vec<String> {
    let mut names: Vec<String> = fs::read_dir(directory)
        .into_iter()
        .flatten()
        .flatten()
        .map(|entry| entry.file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

/// The bytes of a real, valid pre-import snapshot database.
fn valid_snapshot_bytes(farm: &Farm) -> Vec<u8> {
    let snapshot = farm
        .storage
        .create_snapshot(farm3d_lib::persistence::SnapshotKind::Settings)
        .unwrap();
    let bytes = fs::read(snapshot.path()).unwrap();
    fs::remove_file(snapshot.path()).unwrap();
    bytes
}

/// Rows and files for every class: an unreferenced blob row, a pending
/// blob, a slicer log, a safety backup, three pre-import snapshots, the
/// log files, and an Orca cache.
fn stocked_farm() -> (Farm, Rig) {
    let farm = Farm::with_every_domain();
    // A slicer log, held by the seeded Slice Revision.
    let log = farm.write_blob(b"slicer log bytes");
    // A blob held by nothing (its row survives), and a pending blob.
    let unreferenced = farm.write_blob(b"an unreferenced blob");
    let pending = farm.write_blob(b"a pending blob, more bytes than the others");
    // A file no row names (an orphan).
    farm.write_blob(b"an orphan blob file");
    sql(
        &farm,
        &format!(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at) VALUES
               ('{log}', {}, '{now}'), ('{unreferenced}', {}, '{now}');
             INSERT INTO slice_revision_blobs(revision_id, role, sha256)
               VALUES ('{slice}', 'log', '{log}');
             INSERT INTO pending_blob_cleanup(sha256, created_at) VALUES ('{pending}', '{now}');",
            b"slicer log bytes".len(),
            b"an unreferenced blob".len(),
            now = ids::NOW,
            slice = ids::SLICE_REVISION,
        ),
    );
    let paths = farm.paths();
    write_file(
        &paths.backup_root().join("safety/sfb-one.farm3d-backup"),
        &[1; 300],
    );
    write_file(
        &paths.backup_root().join("safety/sfb-two.farm3d-backup"),
        &[2; 150],
    );
    let valid = valid_snapshot_bytes(&farm);
    for timestamp in [100, 200, 300] {
        write_file(
            &paths
                .snapshot_root()
                .join(snapshot_name(timestamp, &format!("t{timestamp}"))),
            &valid,
        );
    }
    write_file(&paths.log_root().join("farm3d.log"), &[b'a'; 500]);
    for (index, length) in [(1, 400), (2, 300), (3, 200), (4, 100)] {
        write_file(
            &paths.log_root().join(format!("farm3d.{index}.log")),
            &vec![b'r'; length],
        );
    }
    let rig = Rig::new(&farm);
    write_file(&rig.cache_dir().join("abc123/machine.json"), &[3; 70]);
    write_file(
        &rig.cache_dir().join("abc123/nested/process.json"),
        &[4; 30],
    );
    (farm, rig)
}

// --- usage -----------------------------------------------------------------------------

#[test]
fn usage_reports_every_class_in_order_and_equals_the_bytes_on_disk() {
    let (farm, rig) = stocked_farm();
    let paths = farm.paths();
    let usage = rig.usage();
    let names: Vec<&str> = usage["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["class"].as_str().unwrap())
        .collect();
    assert_eq!(names, CLASSES);

    // Database: the main file, the WAL, and the shared-memory file.
    let database = paths.database();
    let sibling = |suffix: &str| {
        let mut name = database.file_name().unwrap().to_os_string();
        name.push(suffix);
        database.with_file_name(name)
    };
    let expected_database: u64 = [database.to_path_buf(), sibling("-wal"), sibling("-shm")]
        .iter()
        .filter_map(|path| fs::metadata(path).ok())
        .map(|metadata| metadata.len())
        .sum();
    assert_eq!(bytes(&usage, "database"), expected_database);

    // Content: the seeded blobs, each counted once in its first class.
    let (source, thumbnail, plate) = farm.blob_hashes();
    let length = |sha: &str| fs::metadata(farm.blob_path(sha)).unwrap().len();
    // The G-code blob is a Model Source Revision's, so it is a model source
    // even though a Slice Revision holds it too.
    assert_eq!(
        class(&usage, "contentModelSources"),
        (length(p9_farm::farm_seed::GCODE_HASH) + length(&source), 2)
    );
    assert_eq!(class(&usage, "contentThumbnails"), (length(&thumbnail), 1));
    assert_eq!(class(&usage, "contentGcode"), (0, 0));
    assert_eq!(class(&usage, "contentSliceArtifacts"), (length(&plate), 1));
    assert_eq!(
        class(&usage, "contentSlicerLogs"),
        (b"slicer log bytes".len() as u64, 1)
    );
    // The unreferenced row, plus the pending blob (its file's length); the
    // seeded pending hash has no file.
    let pending = sha256_hex(b"a pending blob, more bytes than the others");
    assert_eq!(
        class(&usage, "contentUnreferenced"),
        (b"an unreferenced blob".len() as u64 + length(&pending), 3)
    );
    // Every row-backed blob's recorded size is its file's length, so the
    // classes sum to the blob bytes on disk less the one orphan file.
    let content_sum: u64 = [
        "contentModelSources",
        "contentThumbnails",
        "contentGcode",
        "contentSliceArtifacts",
        "contentSlicerLogs",
        "contentUnreferenced",
    ]
    .iter()
    .map(|name| bytes(&usage, name))
    .sum();
    assert_eq!(
        content_sum + b"an orphan blob file".len() as u64,
        tree_bytes(&paths.content_root().join("blobs"))
    );

    // Camera media: unpruned rows, pinned versus not.
    let file = |id: &str| {
        fs::metadata(farm.media_file(&p9_farm::snapshot_rel_path(id)))
            .unwrap()
            .len()
    };
    assert_eq!(
        class(&usage, "cameraMediaPinned"),
        (file(ids::SNAPSHOT_PINNED), 1)
    );
    assert_eq!(
        class(&usage, "cameraMediaUnpinned"),
        (file(ids::SNAPSHOT_INCIDENT) + file(ids::SNAPSHOT_MANUAL), 2)
    );

    assert_eq!(class(&usage, "safetyBackups"), (450, 2));
    let snapshot_len = valid_snapshot_bytes(&farm).len() as u64;
    assert_eq!(class(&usage, "preImportSnapshots"), (3 * snapshot_len, 3));
    assert_eq!(class(&usage, "logs"), (1500, 5));
    assert_eq!(class(&usage, "slicerProfileCache"), (100, 2));

    let total: u64 = usage["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["bytes"].as_u64().unwrap())
        .sum();
    assert_eq!(usage["totalBytes"].as_u64().unwrap(), total);
    assert!(usage["measuredAt"].as_str().unwrap().ends_with('Z'));
}

#[test]
fn usage_of_a_bare_farm_reports_zero_for_the_absent_classes() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm);
    let usage = rig.usage();
    for name in [
        "safetyBackups",
        "preImportSnapshots",
        "slicerProfileCache",
        "contentSlicerLogs",
    ] {
        assert_eq!(class(&usage, name), (0, 0), "{name}");
    }
    assert!(bytes(&usage, "database") > 0);
}

// --- unreferencedContent ------------------------------------------------------------------

#[test]
fn clearing_unreferenced_content_frees_only_unreferenced_blobs() {
    let (farm, rig) = stocked_farm();
    let referenced_before = farm.content_hashes();
    let before = rig.usage();
    let pending = sha256_hex(b"a pending blob, more bytes than the others");
    let unreferenced = sha256_hex(b"an unreferenced blob");
    let orphan = sha256_hex(b"an orphan blob file");

    let result = rig.clear("op-content", "unreferencedContent").unwrap();
    assert_eq!(result["target"], "unreferencedContent");
    // The pending blob, the unreferenced row's blob, and the orphan file.
    assert_eq!(result["removedCount"], 3);
    let freed = bytes(&before, "contentUnreferenced") + b"an orphan blob file".len() as u64;
    assert_eq!(result["freedBytes"].as_u64().unwrap(), freed);
    for sha in [&pending, &unreferenced, &orphan] {
        assert!(!farm.blob_path(sha).exists(), "{sha} unlinked");
    }
    // Referenced blobs (and their rows) are untouched.
    for sha in &referenced_before {
        assert!(farm.blob_path(sha).exists(), "{sha} kept");
    }
    assert_eq!(
        count(&farm, "SELECT count(*) FROM content_blobs WHERE sha256 IN (SELECT sha256 FROM slice_revision_blobs)"),
        2
    );
    assert_eq!(count(&farm, "SELECT count(*) FROM pending_blob_cleanup"), 0);
    // The reported usage is the new usage.
    assert_eq!(result["usage"]["classes"], rig.usage()["classes"]);
    assert_eq!(class(&result["usage"], "contentUnreferenced"), (0, 0));
    assert_eq!(rig.services.backup.lease.holder(), None);
    assert_integrity_clean(&farm);
}

// --- preImportSnapshots -----------------------------------------------------------------------

#[test]
fn clearing_pre_import_snapshots_keeps_the_newest_one() {
    let (farm, rig) = stocked_farm();
    let root = farm.paths().snapshot_root().to_path_buf();
    // A file that isn't an accepted snapshot stays.
    write_file(&root.join("notes.txt"), b"keep");
    let result = rig.clear("op-snapshots", "preImportSnapshots").unwrap();
    assert_eq!(result["removedCount"], 2);
    let snapshot_len = valid_snapshot_bytes(&farm).len() as u64;
    assert_eq!(result["freedBytes"].as_u64().unwrap(), 2 * snapshot_len);
    assert_eq!(
        names_in(&root)
            .into_iter()
            .filter(|name| name.ends_with(".sqlite3") || name == "notes.txt")
            .collect::<Vec<_>>(),
        vec![snapshot_name(300, "t300"), "notes.txt".to_string()]
    );
    assert_eq!(
        class(&result["usage"], "preImportSnapshots"),
        (snapshot_len, 1)
    );
    // Nothing more to remove.
    let again = rig.clear("op-snapshots-2", "preImportSnapshots").unwrap();
    assert_eq!(again["removedCount"], 0);
    assert_eq!(again["freedBytes"], 0);
    assert_integrity_clean(&farm);
}

#[test]
fn a_damaged_newest_snapshot_is_not_taken_for_the_newest_valid_one() {
    let (farm, rig) = stocked_farm();
    let root = farm.paths().snapshot_root().to_path_buf();
    // Newer than every valid snapshot, but truncated; and one whose name
    // has no readable timestamp. Neither is a restore source.
    write_file(&root.join(snapshot_name(400, "t400")), b"truncated");
    write_file(
        &root.join(".farm3d-pre-import-settings-nope.sqlite3"),
        b"unnamed",
    );
    let result = rig.clear("op-damaged", "preImportSnapshots").unwrap();
    assert_eq!(result["removedCount"], 2);
    let kept: Vec<String> = names_in(&root)
        .into_iter()
        .filter(|name| name.starts_with(".farm3d-pre-import-") && name.ends_with(".sqlite3"))
        .collect();
    assert_eq!(
        kept,
        vec![
            snapshot_name(300, "t300"),
            snapshot_name(400, "t400"),
            ".farm3d-pre-import-settings-nope.sqlite3".to_string(),
        ]
    );
    assert_integrity_clean(&farm);
}

// --- rotatedLogs ----------------------------------------------------------------------------

#[test]
fn clearing_rotated_logs_never_removes_the_active_file() {
    let (farm, rig) = stocked_farm();
    let log_root = farm.paths().log_root().to_path_buf();
    write_file(&log_root.join("other.txt"), b"keep");
    let result = rig.clear("op-logs", "rotatedLogs").unwrap();
    assert_eq!(result["removedCount"], 4);
    assert_eq!(result["freedBytes"], 400 + 300 + 200 + 100);
    assert_eq!(names_in(&log_root), vec!["farm3d.log", "other.txt"]);
    assert_eq!(
        fs::metadata(log_root.join("farm3d.log")).unwrap().len(),
        500
    );
    assert_eq!(class(&result["usage"], "logs"), (500, 1));
    assert_integrity_clean(&farm);
}

// --- orcaCache ---------------------------------------------------------------------------------

fn slice_operation(farm: &Farm, id: &str, state: &str) {
    let failure = if state == "failed" {
        "'{\"code\":\"x\"}'"
    } else {
        "NULL"
    };
    let finished = if matches!(state, "queued" | "running") {
        "NULL"
    } else {
        "'2026-01-01T00:00:00.000Z'"
    };
    let connection = rusqlite::Connection::open(farm.paths().database()).unwrap();
    // The fixture row needs no preparation or source revision.
    connection
        .execute_batch("PRAGMA foreign_keys = OFF;")
        .unwrap();
    connection
        .execute_batch(&format!(
            "INSERT INTO slice_operations(id, preparation_id, source_revision_id, plate_key,
               plate_snapshot_json, state, failure_json, queued_at, finished_at)
             VALUES ('{id}', 'prep-x', '{revision}', 'plate_1', '{{}}', '{state}', {failure},
               '2026-01-01T00:00:00.000Z', {finished});",
            revision = ids::REVISION_3MF,
        ))
        .unwrap();
}

#[test]
fn the_orca_cache_is_deleted_when_no_slice_operation_is_active() {
    let (farm, rig) = stocked_farm();
    slice_operation(&farm, "sop-done", "succeeded");
    slice_operation(&farm, "sop-failed", "failed");
    let result = rig.clear("op-orca", "orcaCache").unwrap();
    assert_eq!(result["removedCount"], 2);
    assert_eq!(result["freedBytes"], 100);
    assert!(!rig.cache_dir().exists());
    assert_eq!(class(&result["usage"], "slicerProfileCache"), (0, 0));
    // The fixture operations have no preparation; the cleanup left them.
    assert_eq!(count(&farm, "SELECT count(*) FROM slice_operations"), 2);
    sql(&farm, "DELETE FROM slice_operations");
    assert_integrity_clean(&farm);
}

#[test]
fn the_orca_cache_is_refused_while_a_slice_operation_is_queued_or_running() {
    for state in ["queued", "running"] {
        let (farm, rig) = stocked_farm();
        slice_operation(&farm, "sop-active", state);
        let error = rig.clear("op-orca", "orcaCache").unwrap_err();
        assert_eq!(error["code"], "STORAGE_IN_USE", "{state}");
        assert_eq!(error["details"]["target"], "orcaCache");
        assert_eq!(error["details"]["reason"], "sliceRunning");
        assert_eq!(error["retryable"], true);
        assert!(rig.cache_dir().join("abc123/machine.json").exists());
        // Other targets aren't affected by the running slice.
        rig.clear("op-logs", "rotatedLogs").unwrap();
        assert_eq!(rig.services.backup.lease.holder(), None);
    }
}

// --- the lease and the ledger -------------------------------------------------------------------

#[test]
fn a_held_lease_refuses_every_target_and_cleanup_holds_the_lease_while_it_runs() {
    let (farm, rig) = stocked_farm();
    let held = rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::Backup)
        .unwrap();
    for target in [
        "unreferencedContent",
        "preImportSnapshots",
        "rotatedLogs",
        "orcaCache",
    ] {
        let error = rig.clear(&format!("op-{target}"), target).unwrap_err();
        assert_eq!(error["code"], "BACKUP_IN_PROGRESS", "{target}");
        assert_eq!(error["details"]["activity"], "backup");
    }
    // Nothing was deleted.
    assert_eq!(names_in(farm.paths().log_root()).len(), 5);
    assert!(farm.blob_path(&sha256_hex(b"an orphan blob file")).exists());
    drop(held);
    // The lease is released after a cleanup, whatever its outcome.
    rig.clear("op-after", "rotatedLogs").unwrap();
    assert_eq!(rig.services.backup.lease.holder(), None);
}

#[test]
fn a_replay_returns_the_cached_result_and_a_reuse_is_a_validation_error() {
    let (farm, rig) = stocked_farm();
    let first = rig.clear("op-replay", "rotatedLogs").unwrap();
    assert_eq!(first["removedCount"], 4);
    // Files added since don't matter: the replay is the recorded result.
    write_file(&farm.paths().log_root().join("farm3d.2.log"), &[1; 10]);
    let second = rig.clear("op-replay", "rotatedLogs").unwrap();
    assert_eq!(second, first);
    assert!(farm.paths().log_root().join("farm3d.2.log").exists());
    // The same id with another target.
    let error = rig.clear("op-replay", "preImportSnapshots").unwrap_err();
    assert_eq!(error["code"], "VALIDATION");
    assert_eq!(error["details"]["fieldPath"], "operationId");
    // A blank id.
    let error = rig.clear("  ", "rotatedLogs").unwrap_err();
    assert_eq!(error["code"], "VALIDATION");
}

#[test]
fn an_unknown_target_is_rejected() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm);
    assert!(rig.clear("op-bad", "everything").is_err());
}
