//! P9 Task 17 (spec D2, D3, D4, D20): the versioned backup compatibility
//! net.
//!
//! `tests/fixtures/backup/v1/farm-v1-schema10.farm3d-backup` is a real
//! backup of the seeded every-domain Farm (format 1, schema 10), committed
//! and never regenerated (see the README beside it). Every later binary must
//! still restore it. Tampered siblings are built here, in a temp directory,
//! from the committed file; none is committed.
//!
//! `regenerate_backup_fixtures` writes the fixture. It is ignored by
//! default and runs through `just gen-backup-fixtures`. Its output is
//! byte-deterministic (fixed `createdAt`, fixed ids and content).

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::BTreeMap;
use std::fs;
use std::io::{Cursor, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use chrono::{DateTime, Utc};
use rusqlite::Connection;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

use farm3d_lib::backup::archive::{self, ArchiveError, DATABASE_PATH, MANIFEST_PATH};
use farm3d_lib::backup::lease::{BackupLease, LeaseActivity};
use farm3d_lib::backup::manifest::Manifest;
use farm3d_lib::backup::preview;
use farm3d_lib::backup::staging::{self, RestoreError, StagingOptions};
use farm3d_lib::backup::writer::{write_backup, BackupRequest, WriterHooks};
use farm3d_lib::backup::{apply, installer};
use farm3d_lib::backup::{BackupInvalidReason, BackupMediaChoice, BackupOrigin};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::contracts::command::{CommandError, ErrorCode};
use farm3d_lib::persistence::integrity::{self, IntegrityRoots};
use farm3d_lib::persistence::{RepositoryError, Storage};

use p9_farm::{ids, Farm};

const FIXTURE_NAME: &str = "farm-v1-schema10.farm3d-backup";
const CREATED_AT: &str = "2026-09-29T12:00:00.000Z";
/// The fixture must stay small enough to commit without thought.
const MAX_FIXTURE_BYTES: u64 = 1024 * 1024;

fn fixture_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/backup/v1")
}

fn fixture() -> PathBuf {
    fixture_dir().join(FIXTURE_NAME)
}

fn sql(farm: &Farm, statements: &str) {
    farm.storage
        .write_repo(|tx| -> Result<(), RepositoryError> {
            tx.execute_batch(statements).expect("fixture SQL");
            Ok(())
        })
        .expect("fixture write");
}

/// The seeded Farm with every timestamp fixed.
fn seeded_farm() -> Farm {
    let farm = Farm::with_every_domain();
    sql(
        &farm,
        &format!("UPDATE settings SET updated_at = '{}';", ids::NOW),
    );
    farm
}

// --- the generator ----------------------------------------------------------------------

/// Writes the v1 fixture. Ignored by default; run `just gen-backup-fixtures`.
/// Never run it to change a released version's fixture: add a new
/// `tests/fixtures/backup/vN/` instead (README).
#[test]
#[ignore = "run via `just gen-backup-fixtures`"]
fn regenerate_backup_fixtures() {
    let farm = seeded_farm();
    // The one wall-clock column the seeds leave: when each migration ran.
    sql(
        &farm,
        &format!("UPDATE schema_migrations SET applied_at = '{}';", ids::NOW),
    );
    fs::create_dir_all(fixture_dir()).unwrap();
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::Backup).unwrap();
    let path = fixture();
    let _ = fs::remove_file(&path);
    write_backup(
        &farm.storage,
        &guard,
        &path,
        &BackupRequest {
            media: BackupMediaChoice::All,
            origin: BackupOrigin::Operator,
            created_at: Some(CREATED_AT.parse::<DateTime<Utc>>().unwrap()),
            app_version: "0.1.0".to_string(),
        },
        &WriterHooks::default(),
    )
    .expect("backup");
}

// --- helpers ------------------------------------------------------------------------------

fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// The archive's entries, in order.
fn read_entries(path: &Path) -> Vec<(String, Vec<u8>)> {
    let mut zip = ZipArchive::new(fs::File::open(path).unwrap()).unwrap();
    (0..zip.len())
        .map(|index| {
            let mut entry = zip.by_index(index).unwrap();
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            (entry.name().to_string(), bytes)
        })
        .collect()
}

fn write_entries(path: &Path, entries: &[(String, Vec<u8>)]) {
    let mut zip = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default()
        .compression_method(CompressionMethod::Deflated)
        .last_modified_time(zip::DateTime::from_date_and_time(2026, 9, 29, 12, 0, 0).unwrap())
        .unix_permissions(0o644);
    for (name, bytes) in entries {
        zip.start_file(name.as_str(), options).unwrap();
        zip.write_all(bytes).unwrap();
    }
    fs::write(path, zip.finish().unwrap().into_inner()).unwrap();
}

fn manifest_json(entries: &[(String, Vec<u8>)]) -> Value {
    let bytes = &entries.iter().find(|(n, _)| n == MANIFEST_PATH).unwrap().1;
    serde_json::from_slice(bytes).unwrap()
}

fn replace(entries: &mut [(String, Vec<u8>)], name: &str, bytes: Vec<u8>) {
    entries.iter_mut().find(|(n, _)| n == name).unwrap().1 = bytes;
}

/// Points the manifest's entry for `name` at `bytes` (length and SHA-256).
fn relist(manifest: &mut Value, name: &str, bytes: &[u8]) {
    let entry = manifest["entries"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .find(|entry| entry["path"] == name)
        .unwrap();
    entry["bytes"] = json!(bytes.len());
    entry["sha256"] = json!(sha256_hex(bytes));
}

fn put_manifest(entries: &mut [(String, Vec<u8>)], manifest: &Value) {
    replace(
        entries,
        MANIFEST_PATH,
        serde_json::to_vec_pretty(manifest).unwrap(),
    );
}

/// A tampered sibling of the fixture in `dir`.
struct Variants {
    dir: tempfile::TempDir,
}

impl Variants {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.path().join(format!("{name}.farm3d-backup"))
    }

    /// Rewrites the fixture's entries through `edit`.
    fn rewritten(&self, name: &str, edit: impl FnOnce(&mut Vec<(String, Vec<u8>)>)) -> PathBuf {
        let mut entries = read_entries(&fixture());
        edit(&mut entries);
        let path = self.path(name);
        write_entries(&path, &entries);
        path
    }
}

/// What `stage` (the real preview path: rules 1-11) returns for `archive`.
fn refusal(archive: &Path) -> RestoreError {
    let scratch = Farm::with_every_domain();
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::RestorePreview).unwrap();
    match staging::stage(scratch.paths(), &guard, archive, &StagingOptions::default()) {
        Ok(_) => panic!("{} was accepted", archive.display()),
        Err(error) => error,
    }
}

fn invalid(error: &RestoreError) -> BackupInvalidReason {
    match error {
        RestoreError::Archive(ArchiveError::Invalid { reason, .. }) => *reason,
        other => panic!("expected BACKUP_INVALID, got {other:?}"),
    }
}

fn code(error: RestoreError) -> ErrorCode {
    CommandError::from(error).code
}

// --- the committed fixture ------------------------------------------------------------------

#[test]
fn the_fixture_is_a_small_v1_schema_10_backup_and_verifies() {
    let size = fs::metadata(fixture())
        .expect("the committed fixture (run `just gen-backup-fixtures`)")
        .len();
    assert!(size < MAX_FIXTURE_BYTES, "{size} bytes");
    let manifest = archive::verify(&fixture()).expect("verifies");
    assert_eq!(manifest.format_version, 1);
    assert_eq!(manifest.schema_version, 10);
    assert_eq!(manifest.created_at, CREATED_AT);
    assert_eq!(manifest.contents.media, BackupMediaChoice::All);
    for table in [
        "printers",
        "library_projects",
        "library_models",
        "jobs",
        "spools",
        "settings",
    ] {
        assert!(
            manifest.counts.get(table).copied().unwrap_or(0) > 0,
            "{table} has rows in the fixture: {:?}",
            manifest.counts
        );
    }
}

#[test]
fn no_real_network_detail_is_in_the_fixture() {
    let database = read_entries(&fixture())
        .into_iter()
        .find(|(name, _)| name == DATABASE_PATH)
        .unwrap()
        .1;
    let text = String::from_utf8_lossy(&database);
    for line in text.split(|c: char| !(c.is_ascii_digit() || c == '.')) {
        let parts: Vec<&str> = line.split('.').collect();
        if parts.len() == 4 && parts.iter().all(|p| p.parse::<u8>().is_ok()) {
            assert!(
                line.starts_with("192.0.2.")
                    || line.starts_with("198.51.100.")
                    || line.starts_with("203.0.113.")
                    || line == "127.0.0.1"
                    || line == "0.0.0.0",
                "{line} is not a documentation address"
            );
        }
    }
}

#[test]
fn the_fixture_previews_applies_and_installs() {
    // A local Farm with no restore blocker.
    let local = seeded_farm();
    sql(
        &local,
        &format!(
            "UPDATE host_operations SET state = 'failed', resolved_at = '{}',
                    failure_json = '{{\"code\":\"neverSent\"}}' WHERE id = 'hop-a';
             DELETE FROM library_projects WHERE id = '{}';",
            ids::NOW,
            ids::PROJECT
        ),
    );

    // Preview.
    let lease = BackupLease::new();
    let guard = lease.try_acquire(LeaseActivity::RestorePreview).unwrap();
    let candidate = staging::stage(
        local.paths(),
        &guard,
        &fixture(),
        &StagingOptions::default(),
    )
    .expect("staged");
    drop(guard);
    let store = CredentialStore::file_backed(local.credentials_dir.clone());
    let preview = preview::compute(
        &local.storage,
        &candidate,
        farm3d_lib::backup::RestorePreviewSource::File {
            file_name: FIXTURE_NAME.to_string(),
        },
        &store,
    )
    .expect("preview");
    assert_eq!(preview.backup.format_version, 1);
    assert_eq!(preview.backup.schema_version, 10);
    let projects = preview
        .counts
        .iter()
        .find(|count| count.table == "library_projects")
        .unwrap();
    assert_eq!((projects.local, projects.backup), (0, 1));

    // Apply (the pending journal), then the next-start install.
    apply::write_pending_journal(
        &local.storage,
        &candidate,
        "sfb-fixture",
        CREATED_AT.parse().unwrap(),
    )
    .expect("journal");
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
    let report = installer::run_with_faults(
        &paths,
        &metadata_lease,
        || panic!("a restore never opens the credential store"),
        &installer::Faults::new(vec![]),
    )
    .expect("the installer ran");
    assert_eq!(report.outcome, installer::InstallOutcome::Installed);

    // Installed: integrity clean, counts equal the manifest's.
    let manifest = archive::verify(&fixture()).unwrap();
    let connection = Connection::open(paths.database()).unwrap();
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .unwrap();
    let integrity =
        integrity::check(&connection, Some(&IntegrityRoots::from_paths(&paths))).unwrap();
    assert!(integrity.violations().is_empty(), "{integrity:?}");
    for (table, rows) in &integrity.counts {
        if table == "pending_credential_cleanup" || table == "slicer_runtime_config" {
            continue; // D8's two carried exceptions
        }
        assert_eq!(Some(rows), manifest.counts.get(table), "{table}");
    }
    let restored: i64 = connection
        .query_row(
            "SELECT count(*) FROM library_projects WHERE id = ?1",
            [ids::PROJECT],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(restored, 1);
    drop(connection);
    drop(temp);
}

// --- tampered variants ---------------------------------------------------------------------

/// Flips one byte in the middle of `name`'s stored bytes and returns the
/// path of the damaged copy.
fn with_flipped_byte(variants: &Variants, variant: &str, name: &str) -> PathBuf {
    let mut bytes = fs::read(fixture()).unwrap();
    let mut zip = ZipArchive::new(Cursor::new(fs::read(fixture()).unwrap())).unwrap();
    let (start, size) = {
        let entry = zip.by_name(name).unwrap();
        (entry.data_start().unwrap(), entry.compressed_size())
    };
    bytes[(start + size / 2) as usize] ^= 0xff;
    let path = variants.path(variant);
    fs::write(&path, bytes).unwrap();
    path
}

#[test]
fn a_flipped_byte_is_refused_checksum_mismatch() {
    let variants = Variants::new();
    // A stored entry keeps its length, so only the checksum can catch it.
    let path = with_flipped_byte(&variants, "flipped", "media/snapshots/2026/01/snp-a.jpg");
    assert_eq!(
        invalid(&refusal(&path)),
        BackupInvalidReason::ChecksumMismatch
    );
    assert!(archive::verify(&path).is_err());
}

#[test]
fn a_flipped_byte_in_a_deflated_entry_is_refused() {
    let variants = Variants::new();
    // Damaged deflate data may change the inflated length or the hash;
    // either way the entry is refused, never restored.
    let path = with_flipped_byte(&variants, "flipped-db", DATABASE_PATH);
    let reason = invalid(&refusal(&path));
    assert!(
        matches!(
            reason,
            BackupInvalidReason::ChecksumMismatch | BackupInvalidReason::SizeMismatch
        ),
        "{reason:?}"
    );
}

#[test]
fn a_truncated_archive_is_refused_not_a_zip() {
    let variants = Variants::new();
    let bytes = fs::read(fixture()).unwrap();
    let path = variants.path("truncated");
    fs::write(&path, &bytes[..bytes.len() / 2]).unwrap();
    assert_eq!(invalid(&refusal(&path)), BackupInvalidReason::NotAZip);
}

#[test]
fn an_extra_unlisted_entry_is_refused_entry_unlisted() {
    let variants = Variants::new();
    let extra = b"extra".to_vec();
    let name = archive::content_path(&sha256_hex(&extra));
    let path = variants.rewritten("extra", |entries| entries.push((name, extra)));
    assert_eq!(invalid(&refusal(&path)), BackupInvalidReason::EntryUnlisted);
}

#[test]
fn a_traversal_path_is_refused_unsafe_path() {
    let variants = Variants::new();
    let path = variants.rewritten("traversal", |entries| {
        entries.push(("media/../../evil.jpg".to_string(), b"x".to_vec()));
    });
    assert_eq!(invalid(&refusal(&path)), BackupInvalidReason::UnsafePath);
}

#[test]
fn a_bomb_is_refused_size_mismatch_without_being_inflated() {
    let variants = Variants::new();
    // The database entry inflates to 64 MiB of zeros; the manifest lists
    // its real, small length.
    let path = variants.rewritten("bomb", |entries| {
        replace(entries, DATABASE_PATH, vec![0_u8; 64 * 1024 * 1024]);
    });
    assert!(fs::metadata(&path).unwrap().len() < 1024 * 1024);
    assert_eq!(invalid(&refusal(&path)), BackupInvalidReason::SizeMismatch);
}

#[test]
fn format_version_2_is_refused_unsupported_backup_format() {
    let variants = Variants::new();
    let path = variants.rewritten("format2", |entries| {
        let mut manifest = manifest_json(entries);
        manifest["formatVersion"] = json!(2);
        put_manifest(entries, &manifest);
    });
    let error = refusal(&path);
    assert_eq!(
        error,
        RestoreError::Archive(ArchiveError::UnsupportedFormat { received: 2 })
    );
    assert_eq!(code(error), ErrorCode::UnsupportedBackupFormat);
}

#[test]
fn schema_99_is_refused_unsupported_schema_version() {
    let variants = Variants::new();
    let path = variants.rewritten("schema99", |entries| {
        let mut manifest = manifest_json(entries);
        manifest["schemaVersion"] = json!(99);
        put_manifest(entries, &manifest);
    });
    let error = refusal(&path);
    assert_eq!(
        error,
        RestoreError::Archive(ArchiveError::UnsupportedSchema { received: 99 })
    );
    assert_eq!(code(error), ErrorCode::UnsupportedSchemaVersion);
}

#[test]
fn a_manifest_migration_checksum_mismatch_is_refused_migration_mismatch() {
    let variants = Variants::new();
    let path = variants.rewritten("manifest-checksum", |entries| {
        let mut manifest = manifest_json(entries);
        manifest["migrations"][0]["checksum"] = json!("0".repeat(64));
        put_manifest(entries, &manifest);
    });
    assert_eq!(
        invalid(&refusal(&path)),
        BackupInvalidReason::MigrationMismatch
    );
}

#[test]
fn a_database_schema_migrations_checksum_mismatch_is_refused_migration_mismatch() {
    let variants = Variants::new();
    // The database copy's own schema_migrations row is altered and the
    // manifest relisted, so every archive rule passes and only D4's
    // migration check can catch it.
    let path = variants.rewritten("database-checksum", |entries| {
        let scratch = tempfile::tempdir().unwrap();
        let database = scratch.path().join("db.sqlite3");
        let bytes = entries
            .iter()
            .find(|(n, _)| n == DATABASE_PATH)
            .unwrap()
            .1
            .clone();
        fs::write(&database, bytes).unwrap();
        {
            let connection = Connection::open(&database).unwrap();
            connection
                .pragma_update(None, "journal_mode", "DELETE")
                .unwrap();
            let changed = connection
                .execute(
                    "UPDATE schema_migrations SET checksum = ?1 WHERE version = 1",
                    ["0".repeat(64)],
                )
                .unwrap();
            assert_eq!(changed, 1);
        }
        let altered = fs::read(&database).unwrap();
        let mut manifest = manifest_json(entries);
        relist(&mut manifest, DATABASE_PATH, &altered);
        replace(entries, DATABASE_PATH, altered);
        put_manifest(entries, &manifest);
    });
    assert_eq!(
        invalid(&refusal(&path)),
        BackupInvalidReason::MigrationMismatch
    );
}

/// Silences the unused-import lint for helpers only some tests use.
#[allow(dead_code)]
fn _uses(_: &Manifest, _: &BTreeMap<String, i64>, _: &Storage) {}
