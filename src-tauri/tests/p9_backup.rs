//! P9 Task 5 (spec D2, D3, D5, D9, D18): the backup writer, the lease, the
//! safety backups, and the four backup commands, over the every-domain
//! Farm (`tests/p9_farm`).
//!
//! - Writer: for each media choice the archive holds exactly the expected
//!   entries; the database copy is sanitized, `VACUUM`ed, and passes the
//!   integrity checks; its counts equal the source's; no backup-forbidden
//!   secret reaches the archive, any decompressed entry, or the copy's
//!   free pages; a missing or damaged blob fails `BACKUP_SOURCE_DAMAGED`
//!   and leaves no file.
//! - Consistency: a blob deleted during a backup is still archived and is
//!   cleaned up when the lease drops; media pruning waits for the lease; a
//!   capture's prune queues its unlinks; a second lease holder gets
//!   `BACKUP_IN_PROGRESS`; a missing or altered media file is marked
//!   `missingFile`; an `operationId` replay returns the cached result.
//! - Safety backups: verification, retention (newest 3), listing, delete.

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::BTreeMap;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use tauri::test::MockRuntime;

use farm3d_lib::backup::archive;
use farm3d_lib::backup::dialogs::PortabilityDialogs;
use farm3d_lib::backup::lease::{BackupLease, LeaseActivity};
use farm3d_lib::backup::manifest::Manifest;
use farm3d_lib::backup::safety;
use farm3d_lib::backup::writer::{write_backup, BackupRequest, BackupWriteError, WriterHooks};
use farm3d_lib::backup::{BackupMediaChoice, BackupOrigin};
use farm3d_lib::cameras::fetch::Frame;
use farm3d_lib::cameras::media::{self, CaptureLink, NewSnapshot, StoreOutcome};
use farm3d_lib::cameras::retention::{MediaJanitor, RetentionPolicy};
use farm3d_lib::cameras::CameraContentType;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::persistence::integrity;
use farm3d_lib::RuntimeServices;

use p9_farm::farm_seed::GCODE_HASH;
use p9_farm::{ids, snapshot_rel_path, Farm};

const CREATED_AT: &str = "2026-09-29T12:00:00.000Z";

fn created_at() -> DateTime<Utc> {
    CREATED_AT.parse().unwrap()
}

fn request(media: BackupMediaChoice) -> BackupRequest {
    BackupRequest {
        media,
        origin: BackupOrigin::Operator,
        created_at: Some(created_at()),
        app_version: "0.1.0".to_string(),
    }
}

/// A destination directory beside the Farm (as a native save dialog
/// would choose), and a path in it.
fn destination(farm: &Farm, name: &str) -> PathBuf {
    let directory = farm.temp.path().join("exports");
    fs::create_dir_all(&directory).unwrap();
    directory.join(name)
}

/// Writes a backup of `farm` under a lease of its own.
fn backup(farm: &Farm, media: BackupMediaChoice) -> (PathBuf, Manifest) {
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    let path = destination(farm, &format!("backup-{}.farm3d-backup", media.as_str()));
    let written = write_backup(
        &farm.storage,
        &guard,
        &path,
        &request(media),
        &WriterHooks::default(),
    )
    .expect("backup");
    (path, written.manifest)
}

fn entry_names(path: &Path) -> Vec<String> {
    let zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    zip.file_names().map(str::to_string).collect()
}

fn entry_bytes(path: &Path, name: &str) -> Vec<u8> {
    let mut zip = zip::ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    let mut entry = zip.by_name(name).unwrap();
    let mut bytes = Vec::new();
    entry.read_to_end(&mut bytes).unwrap();
    bytes
}

/// The archive's database entry, extracted to a file beside it.
fn extracted_database(path: &Path) -> PathBuf {
    let target = path.with_extension("sqlite3");
    fs::write(&target, entry_bytes(path, archive::DATABASE_PATH)).unwrap();
    target
}

fn open_read_only(path: &Path) -> Connection {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).unwrap()
}

fn media_entry(id: &str) -> String {
    archive::media_path(&snapshot_rel_path(id))
}

fn leftovers(directory: &Path) -> Vec<String> {
    fs::read_dir(directory)
        .map(|entries| {
            entries
                .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
                .collect()
        })
        .unwrap_or_default()
}

// --- Step 2: the writer ---------------------------------------------------------------

#[test]
fn for_each_media_choice_the_archive_holds_exactly_the_expected_entries() {
    let farm = Farm::with_every_domain();
    let (source_3mf, _, plate_3mf) = farm.blob_hashes();
    let (linked_3mf, _) = farm.linked_blob_hashes();
    let stored_blobs = [source_3mf.clone(), plate_3mf.clone(), linked_3mf];
    for (choice, snapshots, counts) in [
        (BackupMediaChoice::None, vec![], (0, 3, 0)),
        (
            BackupMediaChoice::Pinned,
            vec![ids::SNAPSHOT_PINNED],
            (1, 2, 0),
        ),
        (
            BackupMediaChoice::All,
            vec![
                ids::SNAPSHOT_INCIDENT,
                ids::SNAPSHOT_MANUAL,
                ids::SNAPSHOT_PINNED,
            ],
            (3, 0, 0),
        ),
    ] {
        let (path, manifest) = backup(&farm, choice);
        let mut expected = vec![
            archive::MANIFEST_PATH.to_string(),
            archive::DATABASE_PATH.to_string(),
        ];
        expected.extend(
            farm.content_hashes()
                .iter()
                .map(|hash| archive::content_path(hash)),
        );
        expected.extend(snapshots.iter().map(|id| media_entry(id)));
        assert_eq!(entry_names(&path), expected, "{choice:?}");

        // The manifest lists every entry but itself, in archive order.
        let listed: Vec<&str> = manifest
            .entries
            .iter()
            .map(|entry| entry.path.as_str())
            .collect();
        assert_eq!(
            listed,
            expected[1..].iter().map(String::as_str).collect::<Vec<_>>()
        );
        assert_eq!(manifest.contents.media, choice);
        assert_eq!(
            (
                manifest.media.included,
                manifest.media.not_in_backup,
                manifest.media.missing_file
            ),
            counts,
            "{choice:?}"
        );
        assert_eq!(manifest.origin, BackupOrigin::Operator);
        assert_eq!(manifest.created_at, CREATED_AT);
        assert_eq!(manifest.credential_ref_count, 2);

        // The archive reads back under D3's rules, and its manifest entry
        // is the one returned.
        assert_eq!(archive::verify(&path).expect("verifies"), manifest);
        assert_eq!(
            Manifest::parse(&entry_bytes(&path, archive::MANIFEST_PATH)).unwrap(),
            manifest
        );

        // D2's compression column.
        let mut zip = zip::ZipArchive::new(fs::File::open(&path).unwrap()).unwrap();
        for index in 0..zip.len() {
            let entry = zip.by_index_raw(index).unwrap();
            let name = entry.name().to_string();
            let stored = name.starts_with("media/")
                || stored_blobs
                    .iter()
                    .any(|hash| name.ends_with(hash.as_str()));
            let expected_method = if stored {
                zip::CompressionMethod::Stored
            } else {
                zip::CompressionMethod::Deflated
            };
            assert_eq!(entry.compression(), expected_method, "{name}");
            assert!(entry.is_file() && !entry.encrypted(), "{name}");
        }
    }
}

#[test]
fn the_database_copy_is_sanitized_vacuumed_and_passes_the_integrity_checks() {
    let farm = Farm::with_every_domain();
    let (path, _) = backup(&farm, BackupMediaChoice::Pinned);
    let copy_path = extracted_database(&path);
    let copy = open_read_only(&copy_path);

    let (engine, presets): (Option<String>, Option<String>) = copy
        .query_row(
            "SELECT engine_path, preset_source_path FROM slicer_runtime_config",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!((engine, presets), (None, None));
    for table in [
        "printer_status_snapshots",
        "pending_credential_cleanup",
        "pending_blob_cleanup",
    ] {
        let rows: i64 = copy
            .query_row(&format!("SELECT count(*) FROM {table}"), [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(rows, 0, "{table}");
    }

    // Not selected: pruned `notInBackup` at `createdAt`, revision bumped.
    // Selected and present: untouched. Already pruned: untouched.
    let snapshot = |id: &str| -> (i64, Option<String>, Option<String>) {
        copy.query_row(
            "SELECT revision, pruned_at, prune_reason FROM camera_snapshots WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .unwrap()
    };
    for id in [ids::SNAPSHOT_INCIDENT, ids::SNAPSHOT_MANUAL] {
        assert_eq!(
            snapshot(id),
            (
                2,
                Some(CREATED_AT.to_string()),
                Some("notInBackup".to_string())
            ),
            "{id}"
        );
    }
    assert_eq!(snapshot(ids::SNAPSHOT_PINNED), (1, None, None));
    assert_eq!(
        snapshot(ids::SNAPSHOT_PRUNED),
        (1, Some(ids::NOW.to_string()), Some("age".to_string()))
    );
    // No Incident timeline entry was appended.
    let incident_events: i64 = copy
        .query_row("SELECT count(*) FROM incident_events", [], |row| row.get(0))
        .unwrap();
    assert_eq!(incident_events, farm.counts()["incident_events"]);

    // `VACUUM`ed, self-contained, and valid.
    let freelist: i64 = copy
        .query_row("PRAGMA freelist_count", [], |row| row.get(0))
        .unwrap();
    assert_eq!(freelist, 0);
    let journal_mode: String = copy
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(journal_mode, "delete");
    let check: String = copy
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .unwrap();
    assert_eq!(check, "ok");
    let report = integrity::check(&copy, None).unwrap();
    assert!(report.violations().is_empty(), "{:?}", report.findings);
    let user_version: i64 = copy
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(
        user_version,
        farm3d_lib::persistence::CURRENT_SCHEMA_VERSION
    );
}

#[test]
fn the_counts_equal_the_source_and_the_copy() {
    let farm = Farm::with_every_domain();
    let source = farm.counts();
    let (path, manifest) = backup(&farm, BackupMediaChoice::All);

    let mut expected: BTreeMap<String, i64> = source.clone();
    for emptied in [
        "printer_status_snapshots",
        "pending_credential_cleanup",
        "pending_blob_cleanup",
    ] {
        assert!(source[emptied] > 0, "{emptied} is seeded");
        expected.insert(emptied.to_string(), 0);
    }
    assert_eq!(manifest.counts, expected);
    let copy = open_read_only(&extracted_database(&path));
    assert_eq!(p9_farm::table_counts(&copy), manifest.counts);

    // The manifest's migrations are the copy's `schema_migrations`.
    let migrations: Vec<(i64, String, String)> = copy
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")
        .unwrap()
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))
        .unwrap()
        .collect::<Result<_, _>>()
        .unwrap();
    let listed: Vec<(i64, String, String)> = manifest
        .migrations
        .iter()
        .map(|row| (row.version, row.name.clone(), row.checksum.clone()))
        .collect();
    assert_eq!(listed, migrations);
    assert_eq!(
        manifest.schema_version,
        farm3d_lib::persistence::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(manifest.platform.os, std::env::consts::OS);
    assert_eq!(manifest.platform.arch, std::env::consts::ARCH);
}

#[test]
fn no_backup_forbidden_secret_reaches_the_archive_its_entries_or_the_copys_free_pages() {
    let farm = Farm::with_every_domain();
    // Preconditions: the live database holds the corpus the backup must
    // drop: a header in the telemetry cache, and a userinfo URL in a
    // freed page. The credential values live in the store beside it.
    let live = farm.paths().database();
    assert!(p9_farm::file_contains(live, secrets::HEADER_LINE));
    assert!(p9_farm::file_contains(live, secrets::USERINFO_URL));
    // The seeded cache row is a well-formed snapshot, so hydrating it logs
    // no `cacheHydrateFailed` noise.
    farm3d_lib::connections::status_repository::StatusRepository::new(Arc::clone(&farm.storage))
        .list()
        .expect("the seeded telemetry cache decodes");
    let store = secrets::find_any(
        secrets::BACKUP_FORBIDDEN,
        &fs::read(farm3d_lib::connections::credentials::credentials_file_path(
            &farm.credentials_dir,
        ))
        .unwrap(),
        "credentials",
    );
    assert!(!store.is_empty(), "the store holds the credential values");

    for choice in BackupMediaChoice::ALL {
        let (path, _) = backup(&farm, choice);
        let bytes = fs::read(&path).unwrap();
        // The archive, every entry name, and every compressed and
        // decompressed entry (the database copy, free pages included, and
        // the 3MF blobs, recursively).
        secrets::assert_no_backup_forbidden(&bytes, "backup");
        let copy_path = extracted_database(&path);
        secrets::assert_no_backup_forbidden(&fs::read(&copy_path).unwrap(), "database copy");
        let freelist: i64 = open_read_only(&copy_path)
            .query_row("PRAGMA freelist_count", [], |row| row.get(0))
            .unwrap();
        assert_eq!(freelist, 0, "no free page can keep an old value");
        // Farm data a whole-Farm backup legitimately holds (D10).
        assert!(p9_farm::file_contains(&copy_path, secrets::PRINTER_NAME));
        assert!(p9_farm::file_contains(
            &copy_path,
            secrets::STORED_CAMERA_URL
        ));
    }
}

#[test]
fn a_missing_or_damaged_blob_fails_source_damaged_and_leaves_no_file() {
    let farm = Farm::with_every_domain();
    let blob = farm.blob_path(GCODE_HASH);
    let original = fs::read(&blob).unwrap();
    let lease = BackupLease::new();
    let path = destination(&farm, "damaged.farm3d-backup");
    let expected_entry = archive::content_path(GCODE_HASH);

    let mut damaged = original.clone();
    damaged[0] ^= 0xff;
    for (label, bytes) in [("missing", None), ("damaged", Some(damaged))] {
        match &bytes {
            None => fs::remove_file(&blob).unwrap(),
            Some(bytes) => fs::write(&blob, bytes).unwrap(),
        }
        let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
        let error = write_backup(
            &farm.storage,
            &guard,
            &path,
            &request(BackupMediaChoice::All),
            &WriterHooks::default(),
        )
        .err()
        .unwrap_or_else(|| panic!("{label}: the backup succeeded"));
        assert_eq!(
            error,
            BackupWriteError::SourceDamaged {
                entry: expected_entry.clone()
            },
            "{label}"
        );
        assert!(!path.exists(), "{label}: a destination was left");
        assert_eq!(
            leftovers(path.parent().unwrap()),
            Vec::<String>::new(),
            "{label}"
        );
        assert_eq!(
            leftovers(&farm.paths().backup_root().join("tmp")),
            Vec::<String>::new(),
            "{label}: the working directory was left"
        );
        let command_error = CommandError::from(error);
        assert_eq!(
            serde_json::to_value(&command_error).unwrap()["code"],
            "BACKUP_SOURCE_DAMAGED"
        );
        assert_eq!(
            serde_json::to_value(&command_error).unwrap()["details"],
            json!({ "entry": expected_entry })
        );
    }
    fs::write(&blob, &original).unwrap();
}

#[test]
fn a_destination_without_room_is_insufficient_space() {
    let farm = Farm::with_every_domain();
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    let path = destination(&farm, "full.farm3d-backup");
    let hooks = WriterHooks {
        available_bytes: Some(1024),
        ..WriterHooks::default()
    };
    let error = write_backup(
        &farm.storage,
        &guard,
        &path,
        &request(BackupMediaChoice::All),
        &hooks,
    )
    .expect_err("refused");
    let BackupWriteError::InsufficientSpace {
        required,
        available,
    } = error
    else {
        panic!("{error:?}");
    };
    assert_eq!(available, 1024);
    assert!(required > 1024);
    assert!(!path.exists());
}

// --- Step 3: consistency ---------------------------------------------------------------

/// What runs while the (fake) save dialog is open.
type DialogHook = Box<dyn FnOnce() + Send>;

struct FakeDialogs {
    save_to: Option<PathBuf>,
    saves: AtomicUsize,
    while_open: Mutex<Option<DialogHook>>,
}

impl PortabilityDialogs for FakeDialogs {
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_backup(&self, suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        assert!(suggested_name.starts_with("farm3d-"), "{suggested_name}");
        assert!(
            suggested_name.ends_with(".farm3d-backup"),
            "{suggested_name}"
        );
        self.saves.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = self.while_open.lock().unwrap().take() {
            hook();
        }
        Ok(self.save_to.clone())
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
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    dialogs: Arc<FakeDialogs>,
}

impl Rig {
    fn new(farm: &Farm, save_to: Option<PathBuf>) -> Rig {
        let dialogs = Arc::new(FakeDialogs {
            save_to,
            saves: AtomicUsize::new(0),
            while_open: Mutex::new(None),
        });
        let injected: Arc<dyn PortabilityDialogs> = dialogs.clone();
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::backup::commands::backup_inventory,
                farm3d_lib::backup::commands::create_backup,
                farm3d_lib::backup::commands::list_backups,
                farm3d_lib::backup::commands::delete_backup,
                farm3d_lib::library::commands::delete_model,
            ],
            Arc::clone(&farm.storage),
            Arc::new(common::a_catalog()),
            farm.credentials_dir.clone(),
            no_connection,
            move |services| services.backup = Arc::new(services.backup.with_dialogs(injected)),
        );
        Rig {
            _app: app,
            webview,
            services,
            dialogs,
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

    fn err(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .err()
            .unwrap_or_else(|| panic!("{command} succeeded"))
    }
}

#[test]
fn a_blob_deleted_during_a_backup_is_archived_and_released_after_the_lease_drops() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm, None);
    let (source_3mf, thumbnail, _) = farm.blob_hashes();
    let guard = rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::Backup)
        .unwrap();
    let deleted = Arc::new(Mutex::new(None));
    let hooks = {
        let webview = rig.webview.clone();
        let deleted = Arc::clone(&deleted);
        WriterHooks {
            after_database_copy: Some(Box::new(move || {
                // After the copy, before any blob is read.
                let result = common::invoke(
                    &webview,
                    "delete_model",
                    json!({ "contractVersion": 1, "id": ids::MODEL_3MF, "expectedRevision": 1 }),
                );
                *deleted.lock().unwrap() = Some(result.is_ok());
            })),
            ..WriterHooks::default()
        }
    };
    let path = destination(&farm, "during.farm3d-backup");
    write_backup(
        &farm.storage,
        &guard,
        &path,
        &request(BackupMediaChoice::None),
        &hooks,
    )
    .expect("backup");
    assert_eq!(*deleted.lock().unwrap(), Some(true), "delete_model ran");

    // The copy predates the delete, so the archive holds the Model and its
    // blobs; the files survive while the lease is held.
    let names = entry_names(&path);
    for hash in [&source_3mf, &thumbnail] {
        assert!(
            names.contains(&archive::content_path(hash)),
            "{hash} archived"
        );
        assert!(
            farm.blob_path(hash).is_file(),
            "{hash} kept under the lease"
        );
    }
    let pending: i64 = farm
        .storage
        .read(|c| {
            c.query_row("SELECT count(*) FROM pending_blob_cleanup", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(
        pending, 3,
        "the seeded row plus the two released blobs wait"
    );

    // Dropping the lease runs the release once.
    drop(guard);
    for hash in [&source_3mf, &thumbnail] {
        assert!(
            !farm.blob_path(hash).exists(),
            "{hash} unlinked after the lease"
        );
    }
    let pending: i64 = farm
        .storage
        .read(|c| {
            c.query_row("SELECT count(*) FROM pending_blob_cleanup", [], |r| {
                r.get(0)
            })
        })
        .unwrap();
    assert_eq!(pending, 0);
}

/// `2026-01-02`: a day after the Farm's snapshots were captured.
fn soon() -> DateTime<Utc> {
    "2026-01-02T00:00:00Z".parse().unwrap()
}

fn prune_reason(farm: &Farm, id: &str) -> Option<String> {
    farm.storage
        .read(|c| {
            c.query_row(
                "SELECT prune_reason FROM camera_snapshots WHERE id = ?1",
                [id],
                |row| row.get(0),
            )
        })
        .unwrap()
}

#[tokio::test]
async fn media_pruning_waits_for_the_lease() {
    let farm = Farm::with_every_domain();
    farm.pin_linked_snapshots();
    let (old_a, old_b) = ("snp-old-a", "snp-old-b");
    farm.add_unlinked_snapshot(old_a);
    farm.add_unlinked_snapshot(old_b);
    let lease = BackupLease::new();
    let janitor = MediaJanitor::with_backup_lease(lease.clone());
    // One day of retention: both unpinned snapshots are past it.
    let policy = RetentionPolicy {
        retention_days: 1,
        cap_bytes: 1 << 30,
    };
    let now: DateTime<Utc> = "2026-02-01T00:00:00Z".parse().unwrap();
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    let pokes = janitor.pokes();

    let changes = media::prune_pass(&farm.storage, &janitor, policy, now)
        .await
        .unwrap();
    assert!(changes.is_empty(), "the pass is skipped under the lease");
    for id in [old_a, old_b] {
        assert_eq!(prune_reason(&farm, id), None, "{id}");
        assert!(farm.media_file(&snapshot_rel_path(id)).is_file(), "{id}");
    }

    drop(guard);
    assert_eq!(
        janitor.pokes(),
        pokes + 1,
        "dropping the lease pokes the janitor"
    );
    media::prune_pass(&farm.storage, &janitor, policy, now)
        .await
        .unwrap();
    for id in [old_a, old_b] {
        assert_eq!(prune_reason(&farm, id).as_deref(), Some("age"), "{id}");
        assert!(!farm.media_file(&snapshot_rel_path(id)).exists(), "{id}");
    }
}

#[tokio::test]
async fn a_capture_that_must_prune_under_the_lease_queues_its_unlinks() {
    let farm = Farm::with_every_domain();
    farm.pin_linked_snapshots();
    let evicted_id = "snp-evicted";
    farm.add_unlinked_snapshot(evicted_id);
    let lease = BackupLease::new();
    let janitor = MediaJanitor::with_backup_lease(lease.clone());
    let used: i64 = farm
        .storage
        .read(|c| {
            c.query_row(
                "SELECT sum(byte_len) FROM camera_snapshots WHERE pruned_at IS NULL",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    // Room for exactly what is stored: a new frame must evict the only
    // unpinned snapshot.
    let policy = RetentionPolicy {
        retention_days: 365,
        cap_bytes: used,
    };
    let evicted = p9_farm::snapshot_bytes(evicted_id).len();
    let mut bytes = vec![0x5A; evicted];
    bytes[..3].copy_from_slice(&[0xFF, 0xD8, 0xFF]);
    let frame = Frame {
        content_type: CameraContentType::Jpeg,
        bytes,
        captured_at: soon(),
    };
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    let outcome = media::store_frame(
        &farm.storage,
        &janitor,
        policy,
        soon(),
        NewSnapshot {
            printer_id: ids::PRINTER_A,
            link: CaptureLink::Manual {
                operation_id: "op-capture-under-lease",
                digest: "digest",
            },
            frame: &frame,
        },
    )
    .await
    .unwrap();
    assert!(
        matches!(outcome, StoreOutcome::Stored { .. }),
        "{outcome:?}"
    );
    // The row is pruned in the capture's transaction, exactly as P8 does,
    // but its file waits for the lease.
    assert_eq!(prune_reason(&farm, evicted_id).as_deref(), Some("diskCap"));
    let file = farm.media_file(&snapshot_rel_path(evicted_id));
    assert!(
        file.is_file(),
        "the unlink is queued while the lease is held"
    );
    drop(guard);
    assert!(!file.exists(), "dropping the lease unlinks the queued file");
}

#[test]
fn a_second_lease_holder_gets_backup_in_progress() {
    let farm = Farm::with_every_domain();
    let save_to = destination(&farm, "second.farm3d-backup");
    let rig = Rig::new(&farm, Some(save_to.clone()));
    let lease = &rig.services.backup.lease;
    let held = lease.try_acquire(LeaseActivity::RestorePreview).unwrap();
    assert_eq!(
        lease.try_acquire(LeaseActivity::Backup).err(),
        Some(LeaseActivity::RestorePreview)
    );

    for (command, body) in [
        (
            "create_backup",
            json!({ "operationId": "op-1", "media": "all" }),
        ),
        (
            "delete_backup",
            json!({ "operationId": "op-2", "backupId": "sfb-x" }),
        ),
    ] {
        let error = rig.err(command, body);
        assert_eq!(error["code"], "BACKUP_IN_PROGRESS", "{command}: {error}");
        assert_eq!(error["details"], json!({ "activity": "restorePreview" }));
        assert_eq!(error["recovery"], json!(["RETRY"]));
        assert_eq!(error["retryable"], true);
        assert_eq!(
            error["message"],
            "Another backup, restore, reset, or cleanup is running."
        );
    }
    assert_eq!(
        rig.dialogs.saves.load(Ordering::SeqCst),
        0,
        "no dialog opened"
    );
    assert!(!save_to.exists());

    drop(held);
    let result = rig.ok(
        "create_backup",
        json!({ "operationId": "op-1", "media": "all" }),
    );
    assert_eq!(result["status"], "exported", "{result}");
}

#[test]
fn a_missing_or_altered_media_file_is_marked_missing_file_and_counted() {
    let farm = Farm::with_every_domain();
    fs::remove_file(farm.media_file(&snapshot_rel_path(ids::SNAPSHOT_INCIDENT))).unwrap();
    let altered = farm.media_file(&snapshot_rel_path(ids::SNAPSHOT_MANUAL));
    let mut bytes = fs::read(&altered).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0xff;
    fs::write(&altered, bytes).unwrap();

    let (path, manifest) = backup(&farm, BackupMediaChoice::All);
    assert_eq!(
        (
            manifest.media.included,
            manifest.media.not_in_backup,
            manifest.media.missing_file
        ),
        (1, 0, 2)
    );
    let media: Vec<String> = entry_names(&path)
        .into_iter()
        .filter(|name| name.starts_with("media/"))
        .collect();
    assert_eq!(media, vec![media_entry(ids::SNAPSHOT_PINNED)]);
    let copy = open_read_only(&extracted_database(&path));
    for id in [ids::SNAPSHOT_INCIDENT, ids::SNAPSHOT_MANUAL] {
        let (reason, pruned_at): (Option<String>, Option<String>) = copy
            .query_row(
                "SELECT prune_reason, pruned_at FROM camera_snapshots WHERE id = ?1",
                [id],
                |row| Ok((row.get(0)?, row.get(1)?)),
            )
            .unwrap();
        assert_eq!(
            (reason.as_deref(), pruned_at.as_deref()),
            (Some("missingFile"), Some(CREATED_AT)),
            "{id}"
        );
    }
    // The live Farm is untouched: the pre-pass only marks the copy.
    assert_eq!(prune_reason(&farm, ids::SNAPSHOT_INCIDENT), None);
}

#[test]
fn create_backup_replays_its_operation_id_in_the_same_process() {
    let farm = Farm::with_every_domain();
    let save_to = destination(&farm, "replay.farm3d-backup");
    let rig = Rig::new(&farm, Some(save_to.clone()));
    let body = json!({ "operationId": "op-replay", "media": "pinned" });
    let first = rig.ok("create_backup", body.clone());
    assert_eq!(first["status"], "exported");
    assert_eq!(first["fileName"], "replay.farm3d-backup");
    assert_eq!(first["media"], "pinned");
    assert_eq!(first["mediaNotInBackup"], 2);
    assert_eq!(first["mediaMissingFile"], 0);
    assert_eq!(first["bytes"], fs::metadata(&save_to).unwrap().len());
    assert!(
        !first
            .to_string()
            .contains(farm.temp.path().to_str().unwrap()),
        "a result never carries a full path: {first}"
    );
    let written = fs::read(&save_to).unwrap();
    fs::remove_file(&save_to).unwrap();

    let replay = rig.ok("create_backup", body);
    assert_eq!(replay, first, "the cached result");
    assert!(!save_to.exists(), "a replay writes nothing");
    assert_eq!(
        rig.dialogs.saves.load(Ordering::SeqCst),
        1,
        "no second dialog"
    );
    assert!(!written.is_empty());

    let reused = rig.err(
        "create_backup",
        json!({ "operationId": "op-replay", "media": "all" }),
    );
    assert_eq!(reused["code"], "VALIDATION", "{reused}");
    assert_eq!(reused["details"]["fieldPath"], "operationId");
}

// --- the commands ---------------------------------------------------------------------

#[test]
fn a_cancelled_dialog_writes_nothing() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm, None);
    let result = rig.ok(
        "create_backup",
        json!({ "operationId": "op-c", "media": "none" }),
    );
    assert_eq!(result, json!({ "status": "cancelled" }));
    assert_eq!(rig.dialogs.saves.load(Ordering::SeqCst), 1);
    assert!(rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::Backup)
        .is_ok());
}

#[test]
fn backup_inventory_describes_the_live_farm() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm, None);
    let inventory = rig.ok("backup_inventory", json!({}));
    let content_bytes = p9_farm::farm_seed::GCODE_BYTES.len()
        + p9_farm::three_mf_bytes("source").len()
        + p9_farm::thumbnail_bytes().len()
        + p9_farm::three_mf_bytes("plate").len()
        + p9_farm::three_mf_bytes("linked").len()
        + p9_farm::linked_thumbnail_bytes().len();
    assert_eq!(
        inventory["content"],
        json!({ "count": 6, "bytes": content_bytes })
    );
    let snapshot_len = |id: &str| p9_farm::snapshot_bytes(id).len();
    let pinned = snapshot_len(ids::SNAPSHOT_PINNED);
    let all = snapshot_len(ids::SNAPSHOT_INCIDENT) + snapshot_len(ids::SNAPSHOT_MANUAL) + pinned;
    assert_eq!(
        inventory["media"],
        json!([
            { "choice": "none", "count": 0, "bytes": 0 },
            { "choice": "pinned", "count": 1, "bytes": pinned },
            { "choice": "all", "count": 3, "bytes": all },
        ])
    );
    let counts: BTreeMap<String, i64> = inventory["counts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| {
            (
                row["table"].as_str().unwrap().to_string(),
                row["rows"].as_i64().unwrap(),
            )
        })
        .collect();
    assert_eq!(counts, farm.counts());
    let tables: Vec<&str> = inventory["counts"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["table"].as_str().unwrap())
        .collect();
    let mut sorted = tables.clone();
    sorted.sort();
    assert_eq!(tables, sorted);
    assert_eq!(inventory["credentialRefCount"], 2);
    assert_eq!(inventory["activeJobCount"], 0);
    assert_eq!(
        inventory["excluded"],
        json!([
            "credentials",
            "slicerRuntimePaths",
            "printerStatusCache",
            "pendingCredentialCleanup",
            "pendingBlobCleanup",
            "logs"
        ])
    );
    let (pages, page_size): (i64, i64) = farm
        .storage
        .read(|c| {
            Ok((
                c.query_row("PRAGMA page_count", [], |row| row.get(0))?,
                c.query_row("PRAGMA page_size", [], |row| row.get(0))?,
            ))
        })
        .unwrap();
    assert_eq!(inventory["databaseBytes"], pages * page_size);
}

fn safety_backup(rig: &Rig, farm: &Farm, created_at: &str) -> String {
    let guard = rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::RestoreApply)
        .unwrap();
    safety::write_safety_backup(
        &farm.storage,
        &guard,
        BackupOrigin::BeforeRestore,
        created_at.parse().unwrap(),
        "0.1.0",
        &WriterHooks::default(),
    )
    .expect("safety backup")
    .backup_id
}

#[test]
fn safety_backups_are_verified_kept_three_deep_listed_and_deletable() {
    let farm = Farm::with_every_domain();
    let rig = Rig::new(&farm, None);
    let safety_root = farm.paths().backup_root().join("safety");
    fs::create_dir_all(&safety_root).unwrap();
    // A file that doesn't parse: listed as invalid, never retained away.
    fs::write(safety_root.join("sfb-broken.farm3d-backup"), b"not a zip").unwrap();

    let ids: Vec<String> = [
        "2026-09-29T10:00:00.000Z",
        "2026-09-29T11:00:00.000Z",
        "2026-09-29T12:00:00.000Z",
        "2026-09-29T13:00:00.000Z",
    ]
    .iter()
    .map(|at| safety_backup(&rig, &farm, at))
    .collect();
    for id in &ids {
        assert!(id.starts_with("sfb-"), "{id}");
    }
    assert!(
        !safety_root
            .join(format!("{}.farm3d-backup", ids[0]))
            .exists(),
        "oldest removed"
    );

    let listed = rig.ok("list_backups", json!({}));
    let listed = listed.as_array().unwrap();
    let listed_ids: Vec<&str> = listed
        .iter()
        .map(|row| row["backupId"].as_str().unwrap())
        .collect();
    assert_eq!(
        listed_ids,
        vec![
            ids[3].as_str(),
            ids[2].as_str(),
            ids[1].as_str(),
            "sfb-broken"
        ]
    );
    assert_eq!(
        listed[0],
        json!({
            "backupId": ids[3],
            "origin": "beforeRestore",
            "createdAt": "2026-09-29T13:00:00.000Z",
            "bytes": fs::metadata(safety_root.join(format!("{}.farm3d-backup", ids[3]))).unwrap().len(),
            "appVersion": "0.1.0",
            "schemaVersion": farm3d_lib::persistence::CURRENT_SCHEMA_VERSION,
            "media": "all",
            "valid": true,
        })
    );
    assert_eq!(listed[3]["valid"], false);
    assert_eq!(listed[3]["createdAt"], Value::Null);

    // A safety backup carries every snapshot (`media: all`).
    let manifest = archive::verify(&safety_root.join(format!("{}.farm3d-backup", ids[3]))).unwrap();
    assert_eq!(manifest.contents.media, BackupMediaChoice::All);
    assert_eq!(manifest.origin, BackupOrigin::BeforeRestore);

    let deleted = rig.ok(
        "delete_backup",
        json!({ "operationId": "op-del", "backupId": ids[1] }),
    );
    assert_eq!(deleted, json!({ "backupId": ids[1], "deleted": true }));
    assert!(!safety_root
        .join(format!("{}.farm3d-backup", ids[1]))
        .exists());
    // A replay answers from the ledger.
    assert_eq!(
        rig.ok(
            "delete_backup",
            json!({ "operationId": "op-del", "backupId": ids[1] })
        ),
        deleted
    );
    // The broken file can be deleted too.
    rig.ok(
        "delete_backup",
        json!({ "operationId": "op-del-broken", "backupId": "sfb-broken" }),
    );
    for unknown in ["sfb-nope", "../metadata/farm3d", "", "sfb-a/../../x"] {
        let error = rig.err(
            "delete_backup",
            json!({ "operationId": format!("op-{unknown}"), "backupId": unknown }),
        );
        assert_eq!(error["code"], "NOT_FOUND", "{unknown}: {error}");
    }
}

#[test]
fn a_safety_backup_that_fails_verification_is_deleted() {
    let farm = Farm::with_every_domain();
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::Reset).unwrap();
    let hooks = WriterHooks {
        before_verify: Some(Box::new(|path: &Path| {
            // Damage the database entry's bytes on disk after the write.
            let mut bytes = fs::read(path).unwrap();
            let at = bytes.len() / 3;
            bytes[at] ^= 0xff;
            fs::write(path, bytes).unwrap();
        })),
        ..WriterHooks::default()
    };
    let error = safety::write_safety_backup(
        &farm.storage,
        &guard,
        BackupOrigin::BeforeReset,
        created_at(),
        "0.1.0",
        &hooks,
    )
    .expect_err("verification fails");
    assert!(
        matches!(error, BackupWriteError::SourceDamaged { .. }),
        "{error:?}"
    );
    let safety_root = farm.paths().backup_root().join("safety");
    assert_eq!(leftovers(&safety_root), Vec::<String>::new());
}

/// The backup is the Farm at the instant of its copy, which happens after
/// the save dialog closes: `createdAt` (and the `pruned_at` of every
/// snapshot left out) is never earlier than anything captured while the
/// dialog was open.
#[test]
fn created_at_is_stamped_at_the_copy_not_before_the_dialog() {
    let farm = Farm::with_every_domain();
    let save_to = destination(&farm, "late.farm3d-backup");
    let rig = Rig::new(&farm, Some(save_to.clone()));
    let opened_at = Arc::new(Mutex::new(None::<DateTime<Utc>>));
    {
        let storage = Arc::clone(&farm.storage);
        let opened_at = Arc::clone(&opened_at);
        *rig.dialogs.while_open.lock().unwrap() = Some(Box::new(move || {
            // The operator takes a while; a snapshot is captured meanwhile.
            std::thread::sleep(std::time::Duration::from_millis(30));
            let captured = Utc::now();
            *opened_at.lock().unwrap() = Some(captured);
            storage
                .write(|tx| {
                    p9_farm::farm_seed::seed_manual_snapshot(
                        tx,
                        "snp-during-dialog",
                        ids::PRINTER_A,
                        &snapshot_rel_path("snp-during-dialog"),
                    );
                    tx.execute(
                        "UPDATE camera_snapshots SET captured_at = ?1 WHERE id = 'snp-during-dialog'",
                        [captured.to_rfc3339_opts(chrono::SecondsFormat::Millis, true)],
                    )?;
                    Ok(())
                })
                .unwrap();
            std::thread::sleep(std::time::Duration::from_millis(5));
        }));
    }
    let result = rig.ok(
        "create_backup",
        json!({ "operationId": "op-late", "media": "none" }),
    );
    assert_eq!(result["status"], "exported", "{result}");
    let captured = opened_at.lock().unwrap().expect("the dialog ran");

    let manifest = archive::verify(&save_to).unwrap();
    let created_at: DateTime<Utc> = manifest.created_at.parse().unwrap();
    assert!(created_at >= captured, "{created_at} < {captured}");
    let exported_at: DateTime<Utc> = result["exportedAt"].as_str().unwrap().parse().unwrap();
    assert_eq!(exported_at, created_at);

    let copy = open_read_only(&extracted_database(&save_to));
    let inverted: i64 = copy
        .query_row(
            "SELECT count(*) FROM camera_snapshots
              WHERE pruned_at IS NOT NULL AND pruned_at < captured_at",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(inverted, 0, "a snapshot pruned before it was captured");
    let reason: String = copy
        .query_row(
            "SELECT prune_reason FROM camera_snapshots WHERE id = 'snp-during-dialog'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(reason, "notInBackup");
}

/// The janitor's runtime retry of the startup sweep deletes orphan image
/// files, so it waits for the lease too.
#[tokio::test]
async fn the_runtime_media_sweep_waits_for_the_lease() {
    let farm = Farm::with_every_domain();
    let lease = BackupLease::new();
    let janitor = MediaJanitor::with_backup_lease(lease.clone());
    let orphan = farm.media_file("snapshots/2026/01/snp-orphan.jpg");
    fs::write(&orphan, b"no row").unwrap();
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    assert!(media::sweep_under_lock(&farm.storage, &janitor, soon())
        .await
        .is_err());
    assert!(orphan.is_file(), "kept under the lease");
    drop(guard);
    media::sweep_under_lock(&farm.storage, &janitor, soon())
        .await
        .unwrap();
    assert!(!orphan.exists(), "swept after the lease");
}
