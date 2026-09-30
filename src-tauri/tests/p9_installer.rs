//! P9 Task 7 (spec D8, D9, D10, D18; ADR-0016): the restore journal, the
//! startup installer, and `apply_restore`, `restore_status`, and
//! `acknowledge_restore_status`.
//!
//! - The journal: legal phase transitions only, the atomic write, a
//!   corrupt journal or an unknown `journalVersion` refused without
//!   touching anything.
//! - The installer: the spec's "Installer fault points" restore rows, one
//!   test per row (below, copied verbatim). Each injects a fault
//!   (`installer::Fault`: a crash drops every handle and returns, an error
//!   is handled in-run), then runs the installer again with no fault. The
//!   outcome is either **installed** (counts equal the manifest's with D8's
//!   two carried exceptions, `integrity::check` has no violation, the
//!   orphan refs are queued, the journal is `done`) or **rolled back**
//!   (byte-exact as D8 defines it against the files before the first run,
//!   and the journal is `failed`). Never mixed.
//! - `apply_restore`: the confirmation, the ledger, `RESTART_PENDING`
//!   before the lease (f25), the staging, the lease, the blocker re-check,
//!   the safety backup before the journal, the carry, and the injected
//!   restart.
//! - The startup order: installer, then `Storage::open`, then the sweeps.
//!
//! **Restore** (spec "Installer fault points", verbatim)
//!
//! | # | Fault point | Journal at the fault | Disk at the fault | Next start | Outcome |
//! |---|---|---|---|---|---|
//! | f1 | after `apply_restore` writes the journal, before restart | `pending`, attempts 0 | nothing moved | runs from `recheckBlockers` | installed |
//! | f2 | `recheckBlockers` finds an active Job (seeded after the journal was written) | `pending` | nothing moved | — | `failed` `RESTORE_BLOCKED`; nothing moved; staging removed |
//! | f3 | the staging directory is deleted before start | `pending` | nothing moved | — | `failed` `RESTORE_STAGING_EXPIRED`; nothing moved |
//! | f4 | after `markInstalling`, before any move | `installing`/`moveDatabaseAside`, 1 | nothing moved | rollback (no-op), retry | installed, attempts 2 |
//! | f5 | `moveDatabaseAside` after moving the main file only, with a live `-wal` holding uncheckpointed frames | `installing`/`moveDatabaseAside`, 1 | main in `previous/`, `-wal` and `-shm` in root | rollback moves main back, retry | installed; and the pre-restore database, read before the retry, still shows the WAL's rows |
//! | f6 | after `moveDatabaseAside` completes | `installing`/`moveDatabaseAside`, 1 | no database in root | rollback, retry | installed |
//! | f7 | `placeCandidate` with the `.partial` copy half-written | `installing`/`placeCandidate`, 1 | `.partial` in root | rollback deletes it, moves the set back, retry | installed |
//! | f8 | after `placeCandidate` completes | `installing`/`placeCandidate`, 1 | candidate in root | rollback, retry | installed |
//! | f9 | `carryLocalState` before its commit | `installing`/`carryLocalState`, 1 | candidate plus `-wal` in root | rollback deletes the candidate set, retry | installed |
//! | f10 | `extractContent` after the first blob is renamed | `installing`/`extractContent`, 1 | one blob added | rollback, retry (keeps the added blob) | installed |
//! | f11 | `extractContent` fails with no space (twice) | — (in-run errors) | some blobs added | rollback, retry, same error | `failed` `INSUFFICIENT_SPACE`; rolled back; the added blobs are removed by P4's sweep at that same start |
//! | f12 | `swapMedia` between its two renames | `installing`/`swapMedia`, 1, `liveExisted` true | no `<media_root>/snapshots` | rollback renames `previous-snapshots` back, retry | installed |
//! | f12a | `swapMedia` on a Farm with no `<media_root>/snapshots` (never captured), after the staged tree is renamed in | `installing`/`swapMedia`, 1, `liveExisted` false | staged tree at `<media_root>/snapshots` | rollback moves it back to staging, leaving no `snapshots`, retry | installed |
//! | f12b | as f12a, but both attempts crash there | `installing`/`swapMedia`, 2, `liveExisted` false | staged tree in place | rollback, no retry | `failed` `RESTORE_FAILED`; rolled back, `<media_root>/snapshots` still absent |
//! | f12c | after `swapMedia` records `liveExisted`, before its first rename | `installing`/`swapMedia`, 1 | untouched | rollback (media: nothing to undo), retry | installed |
//! | f13 | after `swapMedia` completes | `installing`/`swapMedia`, 1 | staged media in place | rollback swaps both back, retry | installed |
//! | f14 | a staged blob altered after preview | — (in-run, at `extractContent`) | — | rollback, no retry | `failed` `BACKUP_INVALID` (`checksumMismatch`); rolled back |
//! | f15 | `validate` fails (candidate corrupted after staging) | — (in-run) | candidate in place | rollback, no retry | `failed` `BACKUP_INVALID` (`databaseInvalid`); rolled back |
//! | f16 | crash during `validate` | `installing`/`validate`, 1 | candidate in place | rollback, retry | installed |
//! | f17 | two consecutive crashes (f8, then f8 again on the retry) | `installing`/`placeCandidate`, 2 | candidate in root | rollback, no retry | `failed` `RESTORE_FAILED`; rolled back |
//! | f18 | after `markInstalled` | `installed` | `previous/` present | `removePrevious`, `markDone` | installed |
//! | f19 | `removePrevious` half done | `installed`/`removePrevious` | `previous/` partly deleted | `removePrevious`, `markDone` | installed |
//! | f20 | after `markDone` | `done` | clean | nothing | installed; `restore_status` is `done` until acknowledged |
//! | f21 | rollback fails once (an injected I/O error during f8's rollback, phase b) | `installing`/`placeCandidate`, 1, `rollback` written | candidate in root | startup `RESTORE_FAILED` (`rollbackFailed`); the retry resumes the rollback, then retries the install | installed |
//! | f21a | crash in f13's rollback after `rollback` is written, before phase (a) | `rollback { from: swapMedia, false, false }` | staged media in place, candidate in root | rollback resumes at phase (a) | installed |
//! | f21b | crash in f13's rollback after `mediaRestored` is written | `rollback { …, true, false }` | original media back, candidate in root | rollback resumes at phase (b) | installed |
//! | f21c | crash in f8's rollback after `candidateCleared` is written, before any move-back | `rollback { from: placeCandidate, true, true }` | no database in root; the original set in `previous/` | phase (c) moves all three back | installed |
//! | f21d | crash in f8's rollback after `farm3d.sqlite3` is moved back, before `-wal` (with a live `-wal` holding uncheckpointed frames) | `rollback { …, true, true }` | original main in root, original `-wal` and `-shm` in `previous/` | phase (c) moves `-wal` and `-shm` back; it never deletes the main file | installed; and, read before the retry, the pre-restore database shows the WAL's rows |
//! | f21e | crash in f8's rollback after `-wal` is moved back, before `-shm` | `rollback { …, true, true }` | main and `-wal` in root, `-shm` in `previous/` | phase (c) moves `-shm` back | installed |
//! | f21f | f21d, with `attempts` 2 | `rollback { …, true, true }`, attempts 2 | as f21d | phase (c), then no retry | `failed` `RESTORE_FAILED`; rolled back byte-exact, the WAL's rows readable |
//! | f21g | crash in f5's rollback (`from: moveDatabaseAside`) after `candidateCleared` is written | `rollback { from: moveDatabaseAside, true, true }` | main in `previous/`, `-wal` and `-shm` in root | phase (c) moves main back; nothing is deleted | installed |
//! | f22 | the journal file is corrupt | — | untouched | startup `RESTORE_FAILED` (`journalUnreadable`) | nothing touched |
//! | f23 | `journalVersion` is 2 | — | untouched | startup `RESTORE_FAILED` (`journalVersion`) | nothing touched |
//! | f24 | the restarting process still holds the lease for 2 s | `pending` | — | lease retried for up to 10 s | installed |
//! | f25 | a second `apply_restore` (or any `reset_farm`) while the journal is `pending`, before the restart | `pending` | — | — | refused `RESTART_PENDING`; the journal is unchanged; the next start installs the first restore |
//!
//! Every row seeds the WAL the same way where it says so: a connection that
//! never checkpoints writes one Project (`prj-wal`) after the Farm closed,
//! so `farm3d.sqlite3-wal` holds uncommitted-to-main frames at `pending`.

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use rusqlite::{Connection, OpenFlags};
use serde_json::{json, Value};
use tauri::test::MockRuntime;

use farm3d_lib::backup::apply;
use farm3d_lib::backup::dialogs::PortabilityDialogs;
use farm3d_lib::backup::installer::{
    self, Fault, FaultEffect, FaultPoint, Faults, InstallOutcome, InstallReport, InstallerError,
    RollbackPoint,
};
use farm3d_lib::backup::journal::{self, JournalPhase, RestoreJournal, RollbackMarker};
use farm3d_lib::backup::lease::{BackupLease, LeaseActivity};
use farm3d_lib::backup::restart::RecordingRestarter;
use farm3d_lib::backup::staging::{self, StagedCandidate, StagingOptions};
use farm3d_lib::backup::writer::{write_backup, BackupRequest, WriterHooks};
use farm3d_lib::backup::{BackupInvalidReason, BackupMediaChoice, BackupOrigin, InstallerStep};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::contracts::command::{CommandError, ErrorCode};
use farm3d_lib::library::content::ContentStore;
use farm3d_lib::persistence::integrity::{self, IntegrityRoots};
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::RuntimeServices;

use p9_farm::{farm_seed, ids, sha256_hex, Farm};

const CREATED_AT: &str = "2026-09-29T12:00:00.000Z";
const NOW: &str = ids::NOW;
const REF_LOCAL: &str = "farm3d/printer/prn-local/apikey";
const REF_GONE: &str = "farm3d/printer/prn-gone/apikey";
/// `farm_seed`'s pending cleanup row (reason `cleared`), in both Farms.
const REF_SEEDED: &str = "cred-a";
const LOCAL_ENGINE: &str = "/home/example/local/orca-slicer";
const LOCAL_PRESETS: &str = "/home/example/local/presets";
const DATABASE: &str = "farm3d.sqlite3";
const WAL: &str = "farm3d.sqlite3-wal";
const SHM: &str = "farm3d.sqlite3-shm";
const PARTIAL: &str = ".farm3d-restore-candidate.partial";

// --- fixtures -------------------------------------------------------------------------

fn created_at() -> DateTime<Utc> {
    CREATED_AT.parse().unwrap()
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

fn project(id: &str, name: &str) -> String {
    format!(
        "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
           VALUES ('{id}', 1, '{name}', '{NOW}', '{NOW}');"
    )
}

fn farm() -> Farm {
    let farm = Farm::with_every_domain();
    sql(&farm, &format!("UPDATE settings SET updated_at = '{NOW}';"));
    farm
}

fn remote_blob_bytes(label: &str) -> Vec<u8> {
    format!("p9 remote-only blob {label}").into_bytes()
}

/// The backup's Farm: the every-domain Farm plus a Project, a snapshot,
/// and two blobs the local Farm doesn't have.
fn remote_farm() -> Farm {
    let remote = farm();
    sql(&remote, &project("prj-remote", "Remote only"));
    remote.add_unlinked_snapshot("snp-remote");
    for label in ["one", "two"] {
        let bytes = remote_blob_bytes(label);
        let sha256 = remote.write_blob(&bytes);
        sql(
            &remote,
            &format!(
                "INSERT INTO content_blobs(sha256, size_bytes, created_at)
                   VALUES ('{sha256}', {}, '{NOW}');",
                bytes.len()
            ),
        );
    }
    remote
}

fn remote_blob_hashes() -> Vec<String> {
    let mut hashes: Vec<String> = ["one", "two"]
        .iter()
        .map(|label| sha256_hex(&remote_blob_bytes(label)))
        .collect();
    hashes.sort();
    hashes
}

/// The local Farm: no restore blocker (its upload failed), a Project, a
/// Printer with its own credential, pending cleanup rows (one whose ref the
/// backup's Printers use again), this machine's Slicer runtime paths, and a
/// blob the backup lacks.
fn local_farm() -> Farm {
    let local = farm();
    let connection = json!({
        "kind": "moonraker",
        "host": "192.0.2.40",
        "port": 7125,
        "useTls": false,
        "credentialRef": REF_LOCAL,
    })
    .to_string();
    local
        .storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            farm_seed::seed_printer(tx, "prn-local");
            tx.execute(
                "UPDATE printers SET connection_json = ?1 WHERE id = 'prn-local'",
                [&connection],
            )
            .unwrap();
            Ok(())
        })
        .unwrap();
    sql(
        &local,
        &format!(
            "UPDATE host_operations SET state = 'failed', resolved_at = '{NOW}',
                    failure_json = '{{\"code\":\"neverSent\"}}' WHERE id = 'hop-a';
             {project}
             INSERT INTO pending_credential_cleanup(credential_ref, printer_id, reason,
               attempt_count, last_error_code, created_at, last_attempt_at)
               VALUES ('{REF_GONE}', 'prn-gone', 'printer_deleted', 2, 'CREDENTIAL_UNAVAILABLE',
                       '{NOW}', '{NOW}'),
                      ('{b}', NULL, 'cleared', 0, NULL, '{NOW}', NULL);
             UPDATE slicer_runtime_config SET engine_path = '{LOCAL_ENGINE}',
                    preset_source_path = '{LOCAL_PRESETS}';",
            project = project("prj-local", "Local only"),
            b = ids::CREDENTIAL_REF_B,
        ),
    );
    let bytes = b"p9 local-only blob".to_vec();
    let sha256 = local.write_blob(&bytes);
    sql(
        &local,
        &format!(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at)
               VALUES ('{sha256}', {}, '{NOW}');",
            bytes.len()
        ),
    );
    CredentialStore::file_backed(local.credentials_dir.clone())
        .set(REF_LOCAL, "local-secret-value")
        .unwrap();
    local
}

fn write_archive(farm: &Farm, media: BackupMediaChoice) -> PathBuf {
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
            media,
            origin: BackupOrigin::Operator,
            created_at: Some(created_at()),
            app_version: "0.1.0".to_string(),
        },
        &WriterHooks::default(),
    )
    .expect("backup");
    path
}

fn stage(paths: &StoragePaths, archive: &Path) -> StagedCandidate {
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::RestorePreview).unwrap();
    staging::stage(paths, &guard, archive, &StagingOptions::default()).expect("staged")
}

/// A local Farm with a `pending` restore journal, closed as the process
/// that wrote the journal would be before the restart.
struct Site {
    temp: tempfile::TempDir,
    paths: StoragePaths,
    lease: MetadataRootLease,
    candidate: StagedCandidate,
    journal_id: String,
}

fn site() -> Site {
    site_with(|_| {})
}

/// [`site`], with `before_close` run on the local Farm after the journal
/// is written.
fn site_with(before_close: impl FnOnce(&Farm)) -> Site {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let candidate = stage(local.paths(), &archive);
    let journal =
        apply::write_pending_journal(&local.storage, &candidate, "sfb-test", created_at())
            .expect("journal");
    before_close(&local);
    drop(remote);
    let Farm {
        temp,
        metadata_lease,
        storage,
        ..
    } = local;
    let paths = storage.paths().clone();
    drop(
        Arc::try_unwrap(storage)
            .ok()
            .expect("the only Storage handle"),
    );
    Site {
        temp,
        paths,
        lease: metadata_lease,
        candidate,
        journal_id: journal.id,
    }
}

impl Site {
    fn root(&self, name: &str) -> PathBuf {
        self.paths.metadata_root().join(name)
    }

    fn previous(&self, name: &str) -> PathBuf {
        journal::previous_dir(&self.paths, &self.journal_id).join(name)
    }

    fn journal(&self) -> RestoreJournal {
        journal::read(&self.paths).unwrap().expect("a journal")
    }

    fn snapshots(&self) -> PathBuf {
        self.paths.media_root().join("snapshots")
    }

    fn run(&self, faults: Vec<Fault>) -> Result<InstallReport, InstallerError> {
        installer::run_with_faults(
            &self.paths,
            &self.lease,
            || panic!("a restore never opens the credential store"),
            &Faults::new(faults),
        )
    }

    /// Runs with `faults` and expects the injected crash.
    fn crash(&self, faults: Vec<Fault>) {
        match self.run(faults) {
            Err(InstallerError::Crashed) => {}
            other => panic!("expected the injected crash, got {other:?}"),
        }
    }

    fn run_clean(&self) -> InstallReport {
        self.run(vec![]).expect("the installer ran")
    }
}

fn crash_at(step: InstallerStep, point: FaultPoint) -> Fault {
    Fault {
        step,
        point,
        effect: FaultEffect::Crash,
        times: 1,
    }
}

fn rollback_crash(from: InstallerStep, point: RollbackPoint) -> Fault {
    crash_at(from, FaultPoint::Rollback(point))
}

/// Adds uncheckpointed frames to the closed local database: one Project,
/// `prj-wal`, lives only in `-wal`.
fn add_wal_frames(paths: &StoragePaths) {
    let connection = Connection::open(paths.database()).unwrap();
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
    connection
        .pragma_update(None, "wal_autocheckpoint", 0)
        .unwrap();
    let mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .unwrap();
    assert_eq!(mode, "wal");
    connection
        .execute_batch(&project("prj-wal", "Only in the WAL"))
        .unwrap();
    drop(connection);
    let wal = paths.metadata_root().join(WAL);
    assert!(
        fs::metadata(&wal).unwrap().len() > 0,
        "the -wal holds frames"
    );
}

/// Whether the database in the metadata root shows `prj-wal`, read without
/// a checkpoint.
fn wal_rows_visible(paths: &StoragePaths) -> bool {
    let connection = Connection::open_with_flags(
        paths.database(),
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .unwrap();
    connection
        .set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )
        .unwrap();
    connection
        .query_row(
            "SELECT count(*) FROM library_projects WHERE id = 'prj-wal'",
            [],
            |row| row.get::<_, i64>(0),
        )
        .unwrap()
        == 1
}

fn files_under(root: &Path) -> Option<BTreeMap<String, Vec<u8>>> {
    if !root.exists() {
        return None;
    }
    let mut files = BTreeMap::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory).unwrap() {
            let entry = entry.unwrap();
            let path = entry.path();
            if entry.file_type().unwrap().is_dir() {
                pending.push(path);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .into_owned();
                files.insert(relative, fs::read(&path).unwrap());
            }
        }
    }
    Some(files)
}

/// D8's byte-exact baseline, taken before the installer's first run.
struct Baseline {
    database: Vec<u8>,
    /// `Some` when the `-wal` existed and wasn't empty.
    wal: Option<Vec<u8>>,
    blobs: BTreeMap<String, Vec<u8>>,
    media: Option<BTreeMap<String, Vec<u8>>>,
}

impl Baseline {
    fn capture(site: &Site) -> Baseline {
        Baseline {
            database: fs::read(site.root(DATABASE)).unwrap(),
            wal: fs::read(site.root(WAL))
                .ok()
                .filter(|bytes| !bytes.is_empty()),
            blobs: files_under(&site.paths.content_root().join("blobs")).unwrap_or_default(),
            media: files_under(&site.snapshots()),
        }
    }

    fn assert_byte_exact(&self, site: &Site) {
        assert!(
            fs::read(site.root(DATABASE)).unwrap() == self.database,
            "farm3d.sqlite3 is byte-identical"
        );
        match &self.wal {
            Some(bytes) => assert!(
                fs::read(site.root(WAL)).ok().as_ref() == Some(bytes),
                "the non-empty -wal is byte-identical"
            ),
            None => assert!(
                fs::read(site.root(WAL)).map_or(true, |bytes| bytes.is_empty()),
                "no -wal, or an empty one"
            ),
        }
        let blobs = files_under(&site.paths.content_root().join("blobs")).unwrap_or_default();
        for (name, bytes) in &self.blobs {
            assert!(blobs.get(name) == Some(bytes), "blob {name} unchanged");
        }
        assert!(
            files_under(&site.snapshots()) == self.media,
            "the media tree is identical (or still absent)"
        );
        for leftover in [PARTIAL, "farm3d.sqlite3-journal"] {
            assert!(!site.root(leftover).exists(), "{leftover} removed");
        }
    }
}

fn assert_failed(site: &Site, baseline: &Baseline, code: ErrorCode, step: Option<InstallerStep>) {
    baseline.assert_byte_exact(site);
    let journal = site.journal();
    assert_eq!(journal.phase, JournalPhase::Failed, "{journal:?}");
    assert_eq!(journal.rollback, None);
    let failure = journal.failure.expect("a failure");
    assert_eq!(failure.code, code);
    if step.is_some() {
        assert_eq!(failure.step, step);
    }
    assert_eq!(journal.outcome, None);
}

/// "Installed": counts equal `expectedCounts` (with D8's two carried
/// exceptions), `integrity::check` has no violation, orphan refs are
/// queued, and the journal is `done`, with nothing left behind.
fn assert_installed(site: &Site) {
    let journal = site.journal();
    assert_eq!(journal.phase, JournalPhase::Done, "{journal:?}");
    assert!(journal.outcome.is_some());
    assert_eq!(journal.rollback, None);
    assert!(!site.root(WAL).exists() || fs::metadata(site.root(WAL)).unwrap().len() == 0);

    let connection = Connection::open(site.paths.database()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let report =
        integrity::check(&connection, Some(&IntegrityRoots::from_paths(&site.paths))).unwrap();
    assert!(report.violations().is_empty(), "{report:?}");
    let expected = journal.expected_counts.clone().unwrap();
    for (table, rows) in &report.counts {
        if table == "pending_credential_cleanup" || table == "slicer_runtime_config" {
            continue;
        }
        assert_eq!(Some(rows), expected.get(table), "{table}");
    }
    assert_eq!(
        report.counts.keys().collect::<Vec<_>>(),
        expected.keys().collect::<Vec<_>>()
    );

    // The backup's Farm, not the local one.
    let projects: BTreeSet<String> = connection
        .prepare("SELECT id FROM library_projects")
        .unwrap()
        .query_map([], |row| row.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert!(projects.contains("prj-remote"));
    assert!(!projects.contains("prj-local"));
    assert!(!projects.contains("prj-wal"));

    // The carried local state: this machine's Slicer runtime paths, the
    // carried cleanup row verbatim, and the orphan queued `import_orphan`.
    let slicer: (Option<String>, Option<String>) = connection
        .query_row(
            "SELECT engine_path, preset_source_path FROM slicer_runtime_config",
            [],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .unwrap();
    assert_eq!(
        slicer,
        (
            Some(LOCAL_ENGINE.to_string()),
            Some(LOCAL_PRESETS.to_string())
        )
    );
    let pending: Vec<PendingRow> = connection
        .prepare(
            "SELECT credential_ref, printer_id, reason, attempt_count, last_error_code
               FROM pending_credential_cleanup ORDER BY credential_ref",
        )
        .unwrap()
        .query_map([], |row| {
            Ok((
                row.get(0)?,
                row.get(1)?,
                row.get(2)?,
                row.get(3)?,
                row.get(4)?,
            ))
        })
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        pending,
        vec![
            (REF_SEEDED.to_string(), None, "cleared".to_string(), 0, None,),
            (
                REF_GONE.to_string(),
                Some("prn-gone".to_string()),
                "printer_deleted".to_string(),
                2,
                Some("CREDENTIAL_UNAVAILABLE".to_string()),
            ),
            (
                REF_LOCAL.to_string(),
                None,
                "import_orphan".to_string(),
                0,
                None,
            ),
        ]
    );
    drop(connection);

    // The files: the backup's blobs and media are in place.
    for hash in remote_blob_hashes() {
        let path = site
            .paths
            .content_root()
            .join("blobs/sha256")
            .join(&hash[..2])
            .join(&hash);
        assert!(path.is_file(), "restored blob {hash}");
    }
    assert!(site.snapshots().join("2026/01/snp-remote.jpg").is_file());

    // Nothing left behind.
    assert!(!journal::previous_dir(&site.paths, &site.journal_id).exists());
    for directory in [
        &site.candidate.layout.database_dir,
        &site.candidate.layout.content_dir,
        &site.candidate.layout.media_dir,
    ] {
        assert!(!directory.exists(), "{}", directory.display());
    }
    assert!(!site.root(PARTIAL).exists());
}

/// `(credential_ref, printer_id, reason, attempt_count, last_error_code)`.
type PendingRow = (String, Option<String>, String, i64, Option<String>);

fn installed_report(report: &InstallReport) {
    assert_eq!(report.outcome, InstallOutcome::Installed, "{report:?}");
}

// --- the journal ------------------------------------------------------------------------

fn sample_journal(site: &Site) -> RestoreJournal {
    site.journal()
}

#[test]
fn only_d8_transitions_are_legal() {
    use farm3d_lib::backup::RestoreJournalKind::{Reset, Restore};
    use JournalPhase::*;
    let all = [Pending, Installing, Installed, Done, Failed];
    let legal_restore = [
        (Pending, Installing),
        (Pending, Failed),
        (Installing, Pending),
        (Installing, Failed),
        (Installing, Installed),
        (Installed, Done),
    ];
    let legal_reset = [(Pending, Installing), (Installing, Done)];
    for from in all {
        for to in all {
            assert_eq!(
                RestoreJournal::is_legal(Restore, from, to),
                legal_restore.contains(&(from, to)),
                "restore {from:?} -> {to:?}"
            );
            assert_eq!(
                RestoreJournal::is_legal(Reset, from, to),
                legal_reset.contains(&(from, to)),
                "reset {from:?} -> {to:?}"
            );
        }
    }
    let site = site();
    let mut journal = sample_journal(&site);
    assert!(journal.set_phase(Done).is_err());
    assert_eq!(journal.phase, Pending, "an illegal change leaves the phase");
    journal.set_phase(Installing).unwrap();
    journal.set_phase(Installed).unwrap();
    assert!(journal.set_phase(Pending).is_err());
    journal.set_phase(Done).unwrap();
    for to in all {
        assert!(journal.set_phase(to).is_err(), "done is final");
    }
}

#[test]
fn the_journal_is_written_atomically_and_holds_no_credential_value() {
    let site = site();
    let path = journal::journal_path(&site.paths);
    let before = fs::read(&path).unwrap();
    let text = String::from_utf8(before.clone()).unwrap();
    assert!(!text.contains(secrets::CREDENTIAL_VALUE));
    assert!(!text.contains("local-secret-value"));
    let value: Value = serde_json::from_slice(&before).unwrap();
    assert_eq!(value["journalVersion"], 1);
    assert_eq!(value["kind"], "restore");
    assert_eq!(value["phase"], "pending");
    assert_eq!(value["attempts"], 0);
    assert_eq!(value["stagingId"], json!(site.candidate.staging_id));
    assert_eq!(value["safetyBackupId"], "sfb-test");
    assert_eq!(
        value["orphanCredentialRefs"],
        json!([REF_SEEDED, REF_GONE, REF_LOCAL]),
        "the local refs the backup's Printers don't use"
    );
    assert_eq!(
        value["carry"]["pendingCredentialCleanup"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["credentialRef"].clone())
            .collect::<Vec<_>>(),
        vec![json!(REF_SEEDED), json!(REF_GONE)],
        "the row whose ref the backup's Printers use is dropped"
    );
    assert_eq!(
        value["carry"]["slicerRuntime"],
        json!({ "enginePath": LOCAL_ENGINE, "presetSourcePath": LOCAL_PRESETS })
    );

    // A rewrite replaces the file whole; no temporary file stays.
    let mut journal = site.journal();
    journal.set_phase(JournalPhase::Installing).unwrap();
    journal::write(&site.paths, &journal).unwrap();
    assert_eq!(site.journal(), journal);
    let names: Vec<String> = fs::read_dir(journal::restore_root(&site.paths))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| !name.starts_with("rst-"))
        .collect();
    assert_eq!(names, vec!["journal.json".to_string()]);
}

/// f22: the journal file is corrupt.
/// A journal write that fails after its rename (on the directory `fsync`)
/// has still left the `pending` journal the next start installs, so
/// `apply_restore` must not report a failure: the on-disk journal with the
/// same id counts as written. A failure before the rename leaves no
/// journal and is reported.
#[test]
fn a_journal_write_that_fails_after_its_rename_counts_as_written() {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let candidate = stage(local.paths(), &archive);
    let written = apply::write_pending_journal_with(
        &local.storage,
        &candidate,
        "sfb-test",
        created_at(),
        |paths, pending| {
            journal::write(paths, pending)?;
            Err(std::io::Error::other("the directory fsync failed"))
        },
    )
    .expect("the journal on disk is the one written");
    assert_eq!(journal::read(local.paths()).unwrap(), Some(written));

    let path = journal::journal_path(local.paths());
    fs::remove_file(&path).unwrap();
    let error = apply::write_pending_journal_with(
        &local.storage,
        &candidate,
        "sfb-test",
        created_at(),
        |_, _| Err(std::io::Error::from(std::io::ErrorKind::StorageFull)),
    )
    .unwrap_err();
    assert_eq!(
        serde_json::to_value(error).unwrap()["code"],
        "PERSISTENCE_UNAVAILABLE"
    );
    assert!(!path.exists());
}

#[test]
fn f22_a_corrupt_journal_stops_startup_and_touches_nothing() {
    let site = site();
    let baseline = Baseline::capture(&site);
    let path = journal::journal_path(&site.paths);
    for corrupt in [&b"{ not json"[..], b"[]", b"{\"journalVersion\":1}"] {
        fs::write(&path, corrupt).unwrap();
        let error = site.run(vec![]).unwrap_err();
        let error = serde_json::to_value(error.to_command_error()).unwrap();
        assert_eq!(error["code"], "RESTORE_FAILED");
        assert_eq!(error["details"]["reason"], "journalUnreadable");
        assert_eq!(error["retryable"], false);
        assert_eq!(fs::read(&path).unwrap(), corrupt, "the journal untouched");
        baseline.assert_byte_exact(&site);
        assert!(site.candidate.layout.candidate_path().is_file());
    }
}

/// f23: `journalVersion` is 2.
#[test]
fn f23_an_unknown_journal_version_is_refused() {
    let site = site();
    let baseline = Baseline::capture(&site);
    let path = journal::journal_path(&site.paths);
    let mut value: Value = serde_json::from_slice(&fs::read(&path).unwrap()).unwrap();
    value["journalVersion"] = json!(2);
    let bytes = serde_json::to_vec(&value).unwrap();
    fs::write(&path, &bytes).unwrap();
    let error = site.run(vec![]).unwrap_err();
    let error = serde_json::to_value(error.to_command_error()).unwrap();
    assert_eq!(error["code"], "RESTORE_FAILED");
    assert_eq!(error["details"]["reason"], "journalVersion");
    assert_eq!(fs::read(&path).unwrap(), bytes);
    baseline.assert_byte_exact(&site);
    assert!(site.candidate.layout.candidate_path().is_file());
}

#[test]
fn no_journal_is_a_no_op() {
    let site = site();
    fs::remove_file(journal::journal_path(&site.paths)).unwrap();
    let baseline = Baseline::capture(&site);
    assert_eq!(site.run_clean().outcome, InstallOutcome::NoJournal);
    baseline.assert_byte_exact(&site);
}

// --- the fault matrix -------------------------------------------------------------------

/// f1: after `apply_restore` writes the journal, before restart.
#[test]
fn f1_a_pending_journal_installs_from_recheck_blockers() {
    let site = site();
    let journal = site.journal();
    assert_eq!(
        (journal.phase, journal.attempts),
        (JournalPhase::Pending, 0)
    );
    let report = site.run_clean();
    installed_report(&report);
    assert_eq!(report.attempts, 1);
    assert_installed(&site);
    // A second start finds the finished journal and does nothing.
    assert_eq!(site.run_clean().outcome, InstallOutcome::AlreadyFinished);
    assert_installed(&site);
}

/// f2: `recheckBlockers` finds an active Job seeded after the journal.
#[test]
fn f2_a_blocker_found_at_startup_fails_and_moves_nothing() {
    let site = site_with(|local| {
        sql(
            local,
            &format!(
                "INSERT INTO host_operations(id, operation_id, printer_id, kind, host_path,
                   endpoint_json, state, created_at)
                   VALUES ('hop-late', 'hop-late-op', 'prn-local', 'pause', 'part.gcode', '{{}}',
                           'dispatching', '{NOW}');"
            ),
        );
    });
    let baseline = Baseline::capture(&site);
    let report = site.run_clean();
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::RestoreBlocked,
            step: Some(InstallerStep::RecheckBlockers)
        }
    );
    assert_failed(
        &site,
        &baseline,
        ErrorCode::RestoreBlocked,
        Some(InstallerStep::RecheckBlockers),
    );
    assert_eq!(site.journal().attempts, 0);
    for directory in [
        &site.candidate.layout.database_dir,
        &site.candidate.layout.content_dir,
        &site.candidate.layout.media_dir,
    ] {
        assert!(!directory.exists(), "staging removed");
    }
}

/// f3: the staging directory is deleted before start.
#[test]
fn f3_a_missing_staging_fails_expired() {
    let site = site();
    fs::remove_dir_all(&site.candidate.layout.database_dir).unwrap();
    let baseline = Baseline::capture(&site);
    let report = site.run_clean();
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::RestoreStagingExpired,
            step: Some(InstallerStep::RecheckBlockers)
        }
    );
    assert_failed(
        &site,
        &baseline,
        ErrorCode::RestoreStagingExpired,
        Some(InstallerStep::RecheckBlockers),
    );
}

/// f4: after `markInstalling`, before any move.
#[test]
fn f4_a_crash_before_any_move_retries_with_attempts_2() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::MoveDatabaseAside,
        FaultPoint::Start,
    )]);
    let journal = site.journal();
    assert_eq!(
        (journal.phase, journal.step, journal.attempts),
        (
            JournalPhase::Installing,
            Some(InstallerStep::MoveDatabaseAside),
            1
        )
    );
    assert!(site.root(DATABASE).is_file(), "nothing moved");
    let report = site.run_clean();
    installed_report(&report);
    assert_eq!(report.attempts, 2);
    assert_eq!(site.journal().attempts, 2);
    assert_installed(&site);
}

/// f5: `moveDatabaseAside` after the main file only, with a live `-wal`.
#[test]
fn f5_a_partial_move_aside_keeps_the_wal_with_its_database() {
    let site = site();
    add_wal_frames(&site.paths);
    let baseline = Baseline::capture(&site);
    site.crash(vec![crash_at(
        InstallerStep::MoveDatabaseAside,
        FaultPoint::Within(0),
    )]);
    assert!(site.previous(DATABASE).is_file());
    assert!(!site.root(DATABASE).exists());
    assert!(site.root(WAL).is_file() && site.root(SHM).is_file());
    // Rollback moves the main file back; the retry then stops (crash) at
    // its first step, so the pre-restore database can be read.
    site.crash(vec![crash_at(
        InstallerStep::RecheckBlockers,
        FaultPoint::Start,
    )]);
    baseline.assert_byte_exact(&site);
    assert!(wal_rows_visible(&site.paths), "the WAL's rows");
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f6: after `moveDatabaseAside` completes.
#[test]
fn f6_after_move_aside_rolls_back_and_retries() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::MoveDatabaseAside,
        FaultPoint::End,
    )]);
    assert!(!site.root(DATABASE).exists());
    assert!(site.previous(DATABASE).is_file());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f7: `placeCandidate` with the `.partial` copy half-written.
#[test]
fn f7_a_half_written_partial_is_deleted() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::Within(0),
    )]);
    assert!(site.root(PARTIAL).is_file());
    assert!(!site.root(DATABASE).exists());
    let journal = site.journal();
    assert_eq!(journal.step, Some(InstallerStep::PlaceCandidate));
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f8: after `placeCandidate` completes.
#[test]
fn f8_after_place_candidate_rolls_back_and_retries() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    assert!(site.root(DATABASE).is_file(), "the candidate in root");
    assert!(site.previous(DATABASE).is_file());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f9: `carryLocalState` before its commit.
#[test]
fn f9_a_crash_before_the_carry_commits_deletes_the_candidate_set() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::CarryLocalState,
        FaultPoint::Within(0),
    )]);
    assert!(site.root(DATABASE).is_file());
    assert!(site.root(WAL).exists(), "candidate plus -wal in root");
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f10: `extractContent` after the first blob is renamed.
#[test]
fn f10_a_crash_after_one_blob_keeps_it_on_retry() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::ExtractContent,
        FaultPoint::Within(0),
    )]);
    let present = remote_blob_hashes()
        .into_iter()
        .filter(|hash| {
            site.paths
                .content_root()
                .join("blobs/sha256")
                .join(&hash[..2])
                .join(hash)
                .is_file()
        })
        .count();
    assert_eq!(present, 1, "one blob added");
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f11: `extractContent` fails with no space, twice.
#[test]
fn f11_out_of_space_twice_fails_insufficient_space_and_the_sweep_removes_the_blobs() {
    let site = site();
    let baseline = Baseline::capture(&site);
    let report = site
        .run(vec![Fault {
            step: InstallerStep::ExtractContent,
            point: FaultPoint::Within(0),
            effect: FaultEffect::Error(std::io::ErrorKind::StorageFull),
            times: 2,
        }])
        .expect("an in-run failure is a journal outcome");
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::InsufficientSpace,
            step: Some(InstallerStep::ExtractContent)
        }
    );
    assert!(report.rolled_back);
    assert_eq!(report.attempts, 2);
    assert_failed(
        &site,
        &baseline,
        ErrorCode::InsufficientSpace,
        Some(InstallerStep::ExtractContent),
    );
    let blob = |hash: &str| {
        site.paths
            .content_root()
            .join("blobs/sha256")
            .join(&hash[..2])
            .join(hash)
    };
    assert!(
        remote_blob_hashes().iter().any(|hash| blob(hash).is_file()),
        "added"
    );
    // The same start continues: Storage::open, then P4's sweep.
    let storage = Storage::open(site.paths.clone(), &site.lease).unwrap();
    ContentStore::open(site.paths.content_root())
        .unwrap()
        .startup_sweep(&storage)
        .unwrap();
    for hash in remote_blob_hashes() {
        assert!(!blob(&hash).exists(), "{hash} removed by the sweep");
    }
    drop(storage);
    assert_eq!(
        files_under(&site.paths.content_root().join("blobs")).unwrap_or_default(),
        baseline.blobs,
        "only the pre-existing blobs remain"
    );
}

/// f12: `swapMedia` between its two renames.
#[test]
fn f12_a_crash_between_the_media_renames_renames_back() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::SwapMedia,
        FaultPoint::Within(1),
    )]);
    let journal = site.journal();
    assert_eq!(journal.step, Some(InstallerStep::SwapMedia));
    assert_eq!(journal.swap_media.map(|swap| swap.live_existed), Some(true));
    assert!(!site.snapshots().exists());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// A Farm that never captured: no `<media_root>/snapshots`.
fn site_without_media() -> Site {
    let site = site();
    fs::remove_dir_all(site.snapshots()).unwrap();
    site
}

/// f12a: `swapMedia` with no live `snapshots`, after the staged tree is in.
#[test]
fn f12a_a_farm_that_never_captured_rolls_the_staged_tree_back() {
    let site = site_without_media();
    site.crash(vec![crash_at(InstallerStep::SwapMedia, FaultPoint::End)]);
    let journal = site.journal();
    assert_eq!(
        journal.swap_media.map(|swap| swap.live_existed),
        Some(false)
    );
    assert!(site.snapshots().exists(), "the staged tree in place");
    // Rollback, then the retry stops at its first step: no `snapshots`.
    site.crash(vec![crash_at(
        InstallerStep::RecheckBlockers,
        FaultPoint::Start,
    )]);
    assert!(!site.snapshots().exists(), "no snapshots, as before");
    assert!(site.candidate.layout.media_snapshots_dir().is_dir());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f12b: as f12a, but both attempts crash there.
#[test]
fn f12b_two_crashes_in_swap_media_fail_with_snapshots_still_absent() {
    let site = site_without_media();
    let baseline = Baseline::capture(&site);
    site.crash(vec![crash_at(InstallerStep::SwapMedia, FaultPoint::End)]);
    site.crash(vec![crash_at(InstallerStep::SwapMedia, FaultPoint::End)]);
    let journal = site.journal();
    assert_eq!(
        (journal.phase, journal.step, journal.attempts),
        (JournalPhase::Installing, Some(InstallerStep::SwapMedia), 2)
    );
    let report = site.run_clean();
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::RestoreFailed,
            step: Some(InstallerStep::SwapMedia)
        }
    );
    assert_failed(
        &site,
        &baseline,
        ErrorCode::RestoreFailed,
        Some(InstallerStep::SwapMedia),
    );
    assert!(!site.snapshots().exists());
}

/// f12c: after `swapMedia` records `liveExisted`, before its first rename.
#[test]
fn f12c_a_crash_after_live_existed_has_no_media_to_undo() {
    let site = site();
    let media = files_under(&site.snapshots());
    site.crash(vec![crash_at(
        InstallerStep::SwapMedia,
        FaultPoint::Within(0),
    )]);
    assert_eq!(files_under(&site.snapshots()), media, "untouched");
    assert!(site.journal().swap_media.is_some());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f13: after `swapMedia` completes.
#[test]
fn f13_after_swap_media_both_trees_swap_back() {
    let site = site();
    let media = files_under(&site.snapshots());
    site.crash(vec![crash_at(InstallerStep::SwapMedia, FaultPoint::End)]);
    assert_ne!(
        files_under(&site.snapshots()),
        media,
        "staged media in place"
    );
    site.crash(vec![crash_at(
        InstallerStep::RecheckBlockers,
        FaultPoint::Start,
    )]);
    assert_eq!(files_under(&site.snapshots()), media, "swapped back");
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f14: a staged blob altered after preview.
#[test]
fn f14_an_altered_staged_blob_fails_backup_invalid_without_retry() {
    let site = site();
    let hash = &remote_blob_hashes()[1];
    let staged = site
        .candidate
        .layout
        .content_dir
        .join(&hash[..2])
        .join(hash);
    let mut bytes = fs::read(&staged).unwrap();
    bytes[0] ^= 0xff;
    fs::write(&staged, &bytes).unwrap();
    let baseline = Baseline::capture(&site);
    let report = site.run_clean();
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::BackupInvalid,
            step: Some(InstallerStep::ExtractContent)
        }
    );
    assert_eq!(report.attempts, 1, "no retry");
    assert_failed(
        &site,
        &baseline,
        ErrorCode::BackupInvalid,
        Some(InstallerStep::ExtractContent),
    );
    assert_eq!(
        report.invalid_reason,
        Some(BackupInvalidReason::ChecksumMismatch)
    );
}

/// f15: `validate` fails (candidate corrupted after staging).
#[test]
fn f15_a_corrupted_candidate_fails_validation_without_retry() {
    let site = site();
    {
        let connection = Connection::open(site.candidate.layout.candidate_path()).unwrap();
        connection
            .execute_batch(
                "PRAGMA foreign_keys = OFF;
                 UPDATE project_models SET project_id = 'prj-missing';",
            )
            .unwrap();
    }
    let baseline = Baseline::capture(&site);
    let report = site.run_clean();
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::BackupInvalid,
            step: Some(InstallerStep::Validate)
        }
    );
    assert_eq!(report.attempts, 1, "no retry");
    assert_eq!(
        report.invalid_reason,
        Some(BackupInvalidReason::DatabaseInvalid)
    );
    assert_failed(
        &site,
        &baseline,
        ErrorCode::BackupInvalid,
        Some(InstallerStep::Validate),
    );
}

/// f16: crash during `validate`.
#[test]
fn f16_a_crash_during_validate_rolls_back_and_retries() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::Validate,
        FaultPoint::Within(0),
    )]);
    assert_eq!(site.journal().step, Some(InstallerStep::Validate));
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f17: two consecutive crashes (f8, then f8 again on the retry).
#[test]
fn f17_two_consecutive_crashes_fail_restore_failed() {
    let site = site();
    let baseline = Baseline::capture(&site);
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    let journal = site.journal();
    assert_eq!(
        (journal.phase, journal.step, journal.attempts),
        (
            JournalPhase::Installing,
            Some(InstallerStep::PlaceCandidate),
            2
        )
    );
    let report = site.run_clean();
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::RestoreFailed,
            step: Some(InstallerStep::PlaceCandidate)
        }
    );
    assert_failed(
        &site,
        &baseline,
        ErrorCode::RestoreFailed,
        Some(InstallerStep::PlaceCandidate),
    );
}

/// f18: after `markInstalled`.
#[test]
fn f18_after_mark_installed_rolls_forward() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::MarkInstalled,
        FaultPoint::End,
    )]);
    assert_eq!(site.journal().phase, JournalPhase::Installed);
    assert!(journal::previous_dir(&site.paths, &site.journal_id).exists());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f19: `removePrevious` half done.
#[test]
fn f19_a_half_done_remove_previous_finishes() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::RemovePrevious,
        FaultPoint::Within(0),
    )]);
    let journal = site.journal();
    assert_eq!(
        (journal.phase, journal.step),
        (JournalPhase::Installed, Some(InstallerStep::RemovePrevious))
    );
    assert!(
        site.candidate.layout.database_dir.exists(),
        "partly deleted"
    );
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f20: after `markDone`.
#[test]
fn f20_after_mark_done_nothing_runs() {
    let site = site();
    site.crash(vec![crash_at(InstallerStep::MarkDone, FaultPoint::End)]);
    assert_installed(&site);
    let status = site.journal().status();
    assert!(matches!(
        status,
        farm3d_lib::backup::RestoreStatus::Done { .. }
    ));
    assert_eq!(site.run_clean().outcome, InstallOutcome::AlreadyFinished);
    assert_installed(&site);
}

/// f21: rollback fails once (an I/O error during f8's rollback, phase b).
#[test]
fn f21_a_failed_rollback_stops_startup_and_the_retry_resumes_it() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    let error = site
        .run(vec![Fault {
            step: InstallerStep::PlaceCandidate,
            point: FaultPoint::Rollback(RollbackPoint::BeforeClear),
            effect: FaultEffect::Error(std::io::ErrorKind::Other),
            times: 1,
        }])
        .unwrap_err();
    let error = serde_json::to_value(error.to_command_error()).unwrap();
    assert_eq!(error["code"], "RESTORE_FAILED");
    assert_eq!(error["details"]["reason"], "rollbackFailed");
    assert_eq!(error["details"]["step"], "placeCandidate");
    assert_eq!(error["retryable"], true);
    assert_eq!(error["recovery"], json!(["RETRY"]));
    assert_eq!(
        site.journal().rollback,
        Some(RollbackMarker {
            from: InstallerStep::PlaceCandidate,
            media_restored: true,
            candidate_cleared: false,
        })
    );
    assert!(site.root(DATABASE).is_file(), "candidate in root");
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f21a: crash in f13's rollback after `rollback` is written.
#[test]
fn f21a_a_crash_before_rollback_phase_a_resumes_there() {
    let site = site();
    site.crash(vec![crash_at(InstallerStep::SwapMedia, FaultPoint::End)]);
    site.crash(vec![rollback_crash(
        InstallerStep::SwapMedia,
        RollbackPoint::MarkerWritten,
    )]);
    assert_eq!(
        site.journal().rollback,
        Some(RollbackMarker {
            from: InstallerStep::SwapMedia,
            media_restored: false,
            candidate_cleared: false,
        })
    );
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f21b: crash in f13's rollback after `mediaRestored` is written.
#[test]
fn f21b_a_crash_after_media_restored_resumes_at_phase_b() {
    let site = site();
    let media = files_under(&site.snapshots());
    site.crash(vec![crash_at(InstallerStep::SwapMedia, FaultPoint::End)]);
    site.crash(vec![rollback_crash(
        InstallerStep::SwapMedia,
        RollbackPoint::MediaRestored,
    )]);
    let journal = site.journal();
    assert_eq!(
        journal.rollback,
        Some(RollbackMarker {
            from: InstallerStep::SwapMedia,
            media_restored: true,
            candidate_cleared: false,
        })
    );
    assert_eq!(journal.swap_media, None);
    assert_eq!(files_under(&site.snapshots()), media, "original media back");
    assert!(site.root(DATABASE).is_file(), "candidate in root");
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f21c: crash in f8's rollback after `candidateCleared`, before any
/// move-back.
#[test]
fn f21c_a_crash_after_candidate_cleared_moves_all_three_back() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    site.crash(vec![rollback_crash(
        InstallerStep::PlaceCandidate,
        RollbackPoint::CandidateCleared,
    )]);
    assert_eq!(
        site.journal().rollback,
        Some(RollbackMarker {
            from: InstallerStep::PlaceCandidate,
            media_restored: true,
            candidate_cleared: true,
        })
    );
    assert!(!site.root(DATABASE).exists(), "no database in root");
    assert!(site.previous(DATABASE).is_file());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f21d: crash in f8's rollback after the main file is moved back, before
/// `-wal`, with a live `-wal`.
#[test]
fn f21d_a_crash_between_move_backs_never_deletes_the_main_file() {
    let site = site();
    add_wal_frames(&site.paths);
    let baseline = Baseline::capture(&site);
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    site.crash(vec![rollback_crash(
        InstallerStep::PlaceCandidate,
        RollbackPoint::MovedBack(1),
    )]);
    assert!(site.root(DATABASE).is_file(), "original main in root");
    assert!(site.previous(WAL).is_file() && site.previous(SHM).is_file());
    site.crash(vec![crash_at(
        InstallerStep::RecheckBlockers,
        FaultPoint::Start,
    )]);
    baseline.assert_byte_exact(&site);
    assert!(wal_rows_visible(&site.paths), "the WAL's rows");
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f21e: crash in f8's rollback after `-wal` is moved back, before `-shm`.
#[test]
fn f21e_a_crash_before_the_shm_move_back_finishes_it() {
    let site = site();
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    site.crash(vec![rollback_crash(
        InstallerStep::PlaceCandidate,
        RollbackPoint::MovedBack(2),
    )]);
    assert!(site.root(DATABASE).is_file() && site.root(WAL).is_file());
    assert!(site.previous(SHM).is_file());
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f21f: f21d, with `attempts` 2.
#[test]
fn f21f_the_last_attempt_rolls_back_byte_exact_and_fails() {
    let site = site();
    add_wal_frames(&site.paths);
    let baseline = Baseline::capture(&site);
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    site.crash(vec![crash_at(
        InstallerStep::PlaceCandidate,
        FaultPoint::End,
    )]);
    assert_eq!(site.journal().attempts, 2);
    site.crash(vec![rollback_crash(
        InstallerStep::PlaceCandidate,
        RollbackPoint::MovedBack(1),
    )]);
    let report = site.run_clean();
    assert_eq!(
        report.outcome,
        InstallOutcome::Failed {
            code: ErrorCode::RestoreFailed,
            step: Some(InstallerStep::PlaceCandidate)
        }
    );
    assert_failed(
        &site,
        &baseline,
        ErrorCode::RestoreFailed,
        Some(InstallerStep::PlaceCandidate),
    );
    assert!(wal_rows_visible(&site.paths), "the WAL's rows readable");
}

/// f21g: crash in f5's rollback after `candidateCleared` is written.
#[test]
fn f21g_a_move_aside_rollback_deletes_nothing() {
    let site = site();
    add_wal_frames(&site.paths);
    let baseline = Baseline::capture(&site);
    site.crash(vec![crash_at(
        InstallerStep::MoveDatabaseAside,
        FaultPoint::Within(0),
    )]);
    site.crash(vec![rollback_crash(
        InstallerStep::MoveDatabaseAside,
        RollbackPoint::CandidateCleared,
    )]);
    assert_eq!(
        site.journal().rollback,
        Some(RollbackMarker {
            from: InstallerStep::MoveDatabaseAside,
            media_restored: true,
            candidate_cleared: true,
        })
    );
    assert!(site.previous(DATABASE).is_file());
    assert!(
        site.root(WAL).is_file() && site.root(SHM).is_file(),
        "nothing deleted"
    );
    site.crash(vec![crash_at(
        InstallerStep::RecheckBlockers,
        FaultPoint::Start,
    )]);
    baseline.assert_byte_exact(&site);
    assert!(wal_rows_visible(&site.paths));
    installed_report(&site.run_clean());
    assert_installed(&site);
}

/// f24: the restarting process still holds the lease for 2 s.
#[test]
fn f24_a_busy_lease_is_retried_while_a_journal_exists() {
    let site = site();
    let Site {
        temp: _temp,
        paths,
        lease,
        ..
    } = site;
    // The exiting process holds the lease for 2 s more.
    let started = Instant::now();
    let holder = std::thread::spawn(move || {
        std::thread::sleep(Duration::from_secs(2));
        drop(lease);
    });
    let lease = installer::acquire_lease(&paths).expect("the lease, once released");
    let waited = started.elapsed();
    assert!(waited >= Duration::from_millis(1500), "{waited:?}");
    assert!(waited < Duration::from_secs(10), "{waited:?}");
    holder.join().unwrap();
    let report = installer::run(&paths, &lease, || {
        panic!("no credential store for a restore")
    })
    .unwrap();
    installed_report(&report);

    // Without a journal, a busy lease fails at once (F1).
    fs::remove_file(journal::journal_path(&paths)).unwrap();
    let started = Instant::now();
    assert!(installer::acquire_lease(&paths).is_err());
    assert!(started.elapsed() < Duration::from_secs(1));
}

// --- the startup order ------------------------------------------------------------------

/// The installer consumes the staged content before P4's sweep empties
/// `content_root/staging/`: installer, `Storage::open`, then the sweeps.
#[test]
fn startup_installs_before_storage_opens_and_the_sweeps_run() {
    let site = site();
    let staged_blob = site
        .candidate
        .layout
        .content_dir
        .join(&remote_blob_hashes()[0][..2])
        .join(&remote_blob_hashes()[0]);
    assert!(staged_blob.is_file(), "staged under content_root/staging/");
    let Site {
        temp: _temp,
        paths,
        lease,
        ..
    } = site;
    drop(lease);
    let slot = Mutex::new(None);
    let (storage, report) = farm3d_lib::open_storage(paths.clone(), &slot, || {
        panic!("a restore never opens the credential store")
    })
    .map_err(|_| "startup failed")
    .unwrap();
    installed_report(&report);
    ContentStore::open(paths.content_root())
        .unwrap()
        .startup_sweep(&storage)
        .unwrap();
    farm3d_lib::cameras::media::startup_sweep(&storage, Utc::now()).unwrap();
    for hash in remote_blob_hashes() {
        assert!(paths
            .content_root()
            .join("blobs/sha256")
            .join(&hash[..2])
            .join(&hash)
            .is_file());
    }
    assert!(paths
        .media_root()
        .join("snapshots/2026/01/snp-remote.jpg")
        .is_file());
    let projects: i64 = storage
        .read(|connection| {
            connection.query_row(
                "SELECT count(*) FROM library_projects WHERE id = 'prj-remote'",
                [],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(projects, 1);
}

/// A journal the installer can't use enters the bootstrap `Failed` state
/// (`RESTORE_FAILED`) before `Storage::open`; the retry, with the same
/// retained lease, re-runs the installer.
#[test]
fn startup_fails_restore_failed_on_a_corrupt_journal_and_the_retry_reruns_the_installer() {
    let site = site();
    let baseline = Baseline::capture(&site);
    let path = journal::journal_path(&site.paths);
    let valid = fs::read(&path).unwrap();
    fs::write(&path, b"{ corrupt").unwrap();
    let Site {
        temp: _temp,
        paths,
        lease,
        candidate,
        ..
    } = site;
    drop(lease);
    let slot = Mutex::new(None);
    match farm3d_lib::open_storage(paths.clone(), &slot, || panic!("unused")) {
        Err(farm3d_lib::StartupFailure::Recoverable(error)) => {
            let error = serde_json::to_value(error).unwrap();
            assert_eq!(error["code"], "RESTORE_FAILED");
            assert_eq!(error["details"]["reason"], "journalUnreadable");
        }
        Err(farm3d_lib::StartupFailure::Fatal) => panic!("fatal"),
        Ok(_) => panic!("startup continued past a corrupt journal"),
    }
    assert!(
        slot.lock().unwrap().is_some(),
        "the lease is kept for the retry"
    );
    assert!(fs::read(paths.database()).unwrap() == baseline.database);
    assert!(
        candidate.layout.candidate_path().is_file(),
        "the staging is kept"
    );

    // The operator's retry, once the journal is readable again.
    fs::write(&path, &valid).unwrap();
    let (_storage, report) = farm3d_lib::open_storage(paths, &slot, || panic!("unused"))
        .map_err(|_| "startup failed")
        .unwrap();
    installed_report(&report);
}

// --- the commands -----------------------------------------------------------------------

struct FakeDialogs {
    open: Mutex<Option<PathBuf>>,
}

impl PortabilityDialogs for FakeDialogs {
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
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    restarter: Arc<RecordingRestarter>,
}

impl Rig {
    fn new(farm: &Farm, archive: Option<PathBuf>) -> Rig {
        Rig::with_hooks(farm, archive, WriterHooks::default())
    }

    fn with_hooks(farm: &Farm, archive: Option<PathBuf>, hooks: WriterHooks) -> Rig {
        let dialogs: Arc<dyn PortabilityDialogs> = Arc::new(FakeDialogs {
            open: Mutex::new(archive),
        });
        let restarter = Arc::new(RecordingRestarter::default());
        let injected = Arc::clone(&restarter);
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::backup::commands::preview_restore,
                farm3d_lib::backup::commands::apply_restore,
                farm3d_lib::backup::commands::restore_status,
                farm3d_lib::backup::commands::acknowledge_restore_status,
            ],
            Arc::clone(&farm.storage),
            Arc::new(common::a_catalog()),
            farm.credentials_dir.clone(),
            no_connection,
            move |services| {
                let mut backup = services
                    .backup
                    .with_dialogs(dialogs)
                    .with_restarter(injected);
                backup.writer_hooks = hooks;
                services.backup = Arc::new(backup);
            },
        );
        Rig {
            _app: app,
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

    fn preview(&self) -> String {
        let outcome = self.ok("preview_restore", json!({ "source": { "kind": "file" } }));
        assert_eq!(outcome["status"], "previewed", "{outcome}");
        outcome["preview"]["stagingId"]
            .as_str()
            .unwrap()
            .to_string()
    }

    fn apply(
        &self,
        operation_id: &str,
        staging_id: &str,
        confirmation: &str,
    ) -> Result<Value, Value> {
        self.call(
            "apply_restore",
            json!({
                "operationId": operation_id,
                "stagingId": staging_id,
                "confirmation": confirmation,
            }),
        )
    }
}

fn safety_files(paths: &StoragePaths) -> Vec<String> {
    let root = paths.backup_root().join("safety");
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    names.sort();
    names
}

#[test]
fn apply_restore_writes_the_safety_backup_then_the_journal_and_restarts() {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let journal_path = journal::journal_path(local.paths());
    let seen_journal = Arc::new(Mutex::new(None));
    let seen = Arc::clone(&seen_journal);
    let checked = journal_path.clone();
    let hooks = WriterHooks {
        before_verify: Some(Box::new(move |_| {
            *seen.lock().unwrap() = Some(checked.exists());
        })),
        ..WriterHooks::default()
    };
    let rig = Rig::with_hooks(&local, Some(archive), hooks);
    let staging_id = rig.preview();
    let result = rig.apply("op-apply", &staging_id, "restore").unwrap();
    assert_eq!(result["status"], "restarting");
    let safety_backup_id = result["safetyBackupId"].as_str().unwrap().to_string();
    assert_eq!(
        *seen_journal.lock().unwrap(),
        Some(false),
        "the safety backup was written before the journal"
    );
    assert_eq!(
        safety_files(local.paths()),
        vec![format!("{safety_backup_id}.farm3d-backup")]
    );
    // The restart is requested through the injected Restarter, after the
    // response.
    assert_eq!(rig.restarter.requests(), 0, "not before the response");
    assert!(rig.restarter.wait_for(1, Duration::from_secs(5)));

    let journal = journal::read(local.paths()).unwrap().unwrap();
    assert_eq!(journal.phase, JournalPhase::Pending);
    assert_eq!(journal.staging_id.as_deref(), Some(staging_id.as_str()));
    assert_eq!(
        journal.safety_backup_id.as_deref(),
        Some(safety_backup_id.as_str())
    );
    let carried: Vec<&str> = journal
        .carry
        .as_ref()
        .unwrap()
        .pending_credential_cleanup
        .iter()
        .map(|row| row.credential_ref.as_str())
        .collect();
    assert_eq!(
        carried,
        vec![REF_SEEDED, REF_GONE],
        "REF_B's row is dropped"
    );
    assert_eq!(
        journal.orphan_credential_refs,
        vec![REF_SEEDED, REF_GONE, REF_LOCAL]
    );

    // The staging is out of reach: it can't be discarded or replaced.
    assert_eq!(rig.services.backup.stagings.current_id(), None);
    let layout = staging::StagingLayout::new(local.paths(), &staging_id);
    assert!(
        layout.candidate_path().is_file(),
        "the staged files stay for the installer"
    );
    assert!(layout.content_dir.is_dir() && layout.media_snapshots_dir().is_dir());
    // The lease stays held until the restart.
    assert_eq!(
        rig.services.backup.lease.holder(),
        Some(LeaseActivity::RestoreApply)
    );

    // A replay in the same process returns the cached result.
    assert_eq!(
        rig.apply("op-apply", &staging_id, "restore").unwrap(),
        result
    );
    assert_eq!(safety_files(local.paths()).len(), 1);
    // The same id for another staging is VALIDATION.
    let reused = rig
        .apply(
            "op-apply",
            "stg-00000000-0000-4000-8000-000000000000",
            "restore",
        )
        .unwrap_err();
    assert_eq!(reused["code"], "VALIDATION");
    assert_eq!(reused["details"]["fieldPath"], "operationId");
}

/// f25: a second `apply_restore` while the journal is `pending`.
#[test]
fn f25_a_second_apply_while_pending_is_restart_pending_before_the_lease() {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let rig = Rig::new(&local, Some(archive));
    let staging_id = rig.preview();
    let first = rig.apply("op-first", &staging_id, "restore").unwrap();
    let journal_path = journal::journal_path(local.paths());
    let journal_bytes = fs::read(&journal_path).unwrap();
    let journal_id = journal::read(local.paths()).unwrap().unwrap().id;
    // The first apply keeps the lease; a second apply still gets
    // RESTART_PENDING, because the journal is checked before the lease.
    let error = rig.apply("op-second", &staging_id, "restore").unwrap_err();
    assert_eq!(error["code"], "RESTART_PENDING");
    assert_eq!(
        error["details"],
        json!({ "journalId": journal_id, "kind": "restore" })
    );
    assert_eq!(error["recovery"], json!(["RESTART_APPLICATION"]));
    assert_eq!(fs::read(&journal_path).unwrap(), journal_bytes, "unchanged");
    assert_eq!(safety_files(local.paths()).len(), 1);
    assert_eq!(
        rig.restore_status(),
        json!({ "state": "none" }),
        "pending is not a status yet"
    );
    let _ = first;

    // The next start installs the first restore.
    drop(rig);
    let Farm {
        temp: _temp,
        metadata_lease,
        storage,
        ..
    } = local;
    let paths = storage.paths().clone();
    drop(storage);
    let report = installer::run(&paths, &metadata_lease, || panic!("unused")).unwrap();
    installed_report(&report);
    assert_eq!(report.journal_id.as_deref(), Some(journal_id.as_str()));
}

impl Rig {
    fn restore_status(&self) -> Value {
        self.ok("restore_status", json!({}))
    }
}

#[test]
fn apply_restore_refuses_a_mismatched_confirmation_and_an_expired_staging() {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let rig = Rig::new(&local, Some(archive));
    let staging_id = rig.preview();
    for confirmation in ["Restore", " restore", "restore ", "RESTORE", ""] {
        let error = rig.apply("op-c", &staging_id, confirmation).unwrap_err();
        assert_eq!(error["code"], "CONFIRMATION_MISMATCH", "{confirmation:?}");
        assert_eq!(error["details"], json!({ "expected": "restore" }));
        assert_eq!(error["recovery"], json!(["EDIT_FIELDS"]));
    }
    for unknown in ["stg-00000000-0000-4000-8000-000000000000", "../x"] {
        let error = rig.apply("op-e", unknown, "restore").unwrap_err();
        assert_eq!(error["code"], "RESTORE_STAGING_EXPIRED", "{unknown}");
        assert!(!error.to_string().contains("../x"));
    }
    assert!(!journal::journal_path(local.paths()).exists());
    assert!(safety_files(local.paths()).is_empty());
    assert_eq!(rig.restarter.requests(), 0);
    assert_eq!(
        rig.services.backup.stagings.current_id().as_deref(),
        Some(staging_id.as_str()),
        "a refusal leaves the staging"
    );
    assert_eq!(rig.services.backup.lease.holder(), None);
}

#[test]
fn apply_restore_rechecks_the_blockers_under_the_lease() {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let rig = Rig::new(&local, Some(archive));
    let staging_id = rig.preview();
    // Work starts after the preview.
    sql(
        &local,
        &format!(
            "INSERT INTO host_operations(id, operation_id, printer_id, kind, host_path,
               endpoint_json, state, created_at)
               VALUES ('hop-late', 'hop-late-op', 'prn-local', 'pause', 'part.gcode', '{{}}',
                       'dispatching', '{NOW}');"
        ),
    );
    let error = rig.apply("op-b", &staging_id, "restore").unwrap_err();
    assert_eq!(error["code"], "RESTORE_BLOCKED");
    assert_eq!(
        error["details"],
        json!({ "blockers": [{ "kind": "hostOperation", "id": "hop-late" }], "blockerTotal": 1 })
    );
    assert_eq!(error["recovery"], json!(["OPEN_QUEUE"]));
    assert!(!journal::journal_path(local.paths()).exists());
    assert!(safety_files(local.paths()).is_empty());
    assert_eq!(rig.services.backup.lease.holder(), None);
    assert_eq!(
        rig.services.backup.stagings.current_id().as_deref(),
        Some(staging_id.as_str())
    );

    // The lease held by another operation is BACKUP_IN_PROGRESS.
    let guard = rig
        .services
        .backup
        .lease
        .try_acquire(LeaseActivity::Backup)
        .unwrap();
    let error = rig.apply("op-l", &staging_id, "restore").unwrap_err();
    assert_eq!(error["code"], "BACKUP_IN_PROGRESS");
    drop(guard);
}

#[test]
fn a_failed_safety_backup_stops_before_the_journal() {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let hooks = WriterHooks {
        before_verify: Some(Box::new(|path: &Path| {
            let bytes = fs::read(path).unwrap();
            fs::write(path, &bytes[..bytes.len() / 2]).unwrap();
        })),
        ..WriterHooks::default()
    };
    let rig = Rig::with_hooks(&local, Some(archive), hooks);
    let staging_id = rig.preview();
    let error = rig.apply("op-s", &staging_id, "restore").unwrap_err();
    assert_eq!(error["code"], "BACKUP_SOURCE_DAMAGED");
    assert!(!journal::journal_path(local.paths()).exists());
    assert!(safety_files(local.paths()).is_empty());
    assert_eq!(rig.restarter.requests(), 0);
    assert_eq!(rig.services.backup.lease.holder(), None);
    assert_eq!(
        rig.services.backup.stagings.current_id().as_deref(),
        Some(staging_id.as_str())
    );
}

#[test]
fn restore_status_reports_the_finished_journal_until_acknowledged() {
    let remote = remote_farm();
    let archive = write_archive(&remote, BackupMediaChoice::All);
    let local = local_farm();
    let rig = Rig::new(&local, Some(archive));
    let staging_id = rig.preview();
    let result = rig.apply("op-status", &staging_id, "restore").unwrap();
    let safety_backup_id = result["safetyBackupId"].clone();
    assert!(rig.restarter.wait_for(1, Duration::from_secs(5)));
    drop(rig);

    // The restart: the installer, then the restored Farm is served.
    let Farm {
        temp: _temp,
        metadata_lease,
        storage,
        credentials_dir,
    } = local;
    let paths = storage.paths().clone();
    drop(storage);
    let report = installer::run(&paths, &metadata_lease, || panic!("unused")).unwrap();
    installed_report(&report);
    let journal_id = report.journal_id.clone().unwrap();
    let storage = Arc::new(Storage::open(paths.clone(), &metadata_lease).unwrap());
    let restored = Farm {
        temp: _temp,
        metadata_lease,
        storage,
        credentials_dir,
    };
    let rig = Rig::new(&restored, None);
    let status = rig.restore_status();
    assert_eq!(status["state"], "done");
    assert_eq!(status["journalId"], json!(journal_id));
    assert_eq!(status["kind"], "restore");
    assert_eq!(status["safetyBackupId"], safety_backup_id);
    assert!(status["finishedAt"].as_str().is_some());
    assert_eq!(rig.restore_status(), status, "until acknowledged");

    let wrong = rig
        .call(
            "acknowledge_restore_status",
            json!({ "journalId": "rst-00000000-0000-4000-8000-000000000000" }),
        )
        .unwrap_err();
    assert_eq!(wrong["code"], "NOT_FOUND");
    assert_eq!(
        rig.ok(
            "acknowledge_restore_status",
            json!({ "journalId": journal_id })
        ),
        json!({ "state": "none" })
    );
    assert!(!journal::journal_path(&paths).exists());
    assert!(!journal::journal_dir(&paths, &journal_id).exists());
    assert_eq!(rig.restore_status(), json!({ "state": "none" }));
    let again = rig
        .call(
            "acknowledge_restore_status",
            json!({ "journalId": journal_id }),
        )
        .unwrap_err();
    assert_eq!(again["code"], "NOT_FOUND");
}

#[test]
fn restore_status_reports_a_failed_restore() {
    let site = site();
    fs::remove_dir_all(&site.candidate.layout.database_dir).unwrap();
    site.run_clean();
    let status = serde_json::to_value(site.journal().status()).unwrap();
    assert_eq!(status["state"], "failed");
    assert_eq!(status["code"], "RESTORE_STAGING_EXPIRED");
    assert_eq!(status["failedStep"], "recheckBlockers");
    assert_eq!(status["safetyBackupId"], "sfb-test");
    assert_eq!(status["journalId"], json!(site.journal_id));
}
