//! Restore staging (spec D3 rules 9–11, D4, D8 "Staging"; ADR-0016).
//!
//! [`stage`] streams a backup once into three directories, each beside the
//! root its files are installed into, so the startup installer (D8) only
//! renames within one filesystem:
//!
//! | Path | Holds |
//! |---|---|
//! | `<snapshot_root>/.restore-staging/<stagingId>/` | `candidate.sqlite3` and `manifest.json` |
//! | `<content_root>/staging/restore-<stagingId>/<hh>/<hex>` | the content blobs |
//! | `<media_root>/restore-<stagingId>/snapshots/…` | the camera media (always created) |
//!
//! Each entry goes through a same-directory `.part` file that is `fsync`ed
//! and renamed only once its length and SHA-256 match the manifest (rule
//! 10). The candidate is then checked against the manifest and migrated
//! forward on the staged copy (D4); the live database is never opened.
//! Any failure removes all three directories.
//!
//! A staging expires 24 hours after it was created. [`Stagings`] is the
//! process's one staging (D8: "at most one exists per process"); at startup
//! [`cleanup`] removes every staging directory except the one a `pending`
//! or `installing` journal names.
//!
//! This extends F1's `Storage::stage_restore` (a pre-import snapshot staged
//! under the same `.restore-staging/` root, with the same `stg-` ids).

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io::{self, BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use chrono::{DateTime, Duration, Utc};
use rusqlite::{Connection, OpenFlags};

use super::archive::{self, ArchiveError, DATABASE_PATH, MANIFEST_PATH};
use super::lease::LeaseGuard;
use super::manifest::{Manifest, MIN_SCHEMA_VERSION};
use crate::diagnostics::storage::available_bytes;
use super::BackupInvalidReason;
use crate::contracts::command::CommandError;
use crate::persistence::integrity;
use crate::persistence::{
    apply_migrations, create_contained_directory, embedded_migrations, has_foreign_key_violation,
    StorageError, StoragePaths, CURRENT_SCHEMA_VERSION,
};

/// `<snapshot_root>/.restore-staging/`.
pub const DATABASE_STAGING_DIRECTORY: &str = ".restore-staging";
/// The staged database, inside the database staging directory.
pub const CANDIDATE_FILE: &str = "candidate.sqlite3";
/// The prefix of the content and media staging directories.
const RESTORE_PREFIX: &str = "restore-";
const ID_PREFIX: &str = "stg-";
const PART_SUFFIX: &str = ".part";

/// How long a staging stays usable after it was created (D8).
pub fn staging_ttl() -> Duration {
    Duration::hours(24)
}

/// A new staging id, `stg-<uuid v4>`.
pub fn new_staging_id() -> String {
    format!("{ID_PREFIX}{}", uuid::Uuid::new_v4())
}

/// Whether `id` is a staging id (`stg-` and a hyphenated UUID): the only
/// ids ever joined onto a path or echoed in an error.
pub fn is_staging_id(id: &str) -> bool {
    id.strip_prefix(ID_PREFIX).is_some_and(|rest| {
        rest.len() == 36
            && uuid::Uuid::try_parse(rest).is_ok_and(|uuid| uuid.hyphenated().to_string() == rest)
    })
}

/// The three staging directories of one staging id.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StagingLayout {
    /// `<snapshot_root>/.restore-staging/<id>/`.
    pub database_dir: PathBuf,
    /// `<content_root>/staging/restore-<id>/`.
    pub content_dir: PathBuf,
    /// `<media_root>/restore-<id>/` (its `snapshots/` holds the media).
    pub media_dir: PathBuf,
}

impl StagingLayout {
    /// The layout of `staging_id`, which must be a staging id.
    pub fn new(paths: &StoragePaths, staging_id: &str) -> Self {
        debug_assert!(is_staging_id(staging_id));
        Self {
            database_dir: paths
                .snapshot_root()
                .join(DATABASE_STAGING_DIRECTORY)
                .join(staging_id),
            content_dir: paths
                .content_root()
                .join("staging")
                .join(format!("{RESTORE_PREFIX}{staging_id}")),
            media_dir: paths
                .media_root()
                .join(format!("{RESTORE_PREFIX}{staging_id}")),
        }
    }

    pub fn candidate_path(&self) -> PathBuf {
        self.database_dir.join(CANDIDATE_FILE)
    }

    pub fn manifest_path(&self) -> PathBuf {
        self.database_dir.join(MANIFEST_PATH)
    }

    /// `restore-<id>/snapshots/`: the staged media tree.
    pub fn media_snapshots_dir(&self) -> PathBuf {
        self.media_dir.join("snapshots")
    }

    /// Removes all three directories. Returns whether any existed.
    pub fn remove(&self) -> bool {
        let mut existed = false;
        for directory in [&self.database_dir, &self.content_dir, &self.media_dir] {
            if fs::symlink_metadata(directory).is_ok() {
                existed = true;
                let _ = remove_path(directory);
            }
        }
        existed
    }
}

/// A staged, verified, and migrated backup.
#[derive(Clone, Debug)]
pub struct StagedCandidate {
    pub staging_id: String,
    pub created_at: DateTime<Utc>,
    /// The backup's manifest (also written beside the candidate).
    pub manifest: Manifest,
    pub layout: StagingLayout,
    /// The backup's `schemaVersion` when the staged copy was migrated
    /// forward from it; `None` when it was already current.
    pub migrated_from: Option<i64>,
}

impl StagedCandidate {
    pub fn expires_at(&self) -> DateTime<Utc> {
        self.created_at + staging_ttl()
    }

    pub fn is_expired(&self, now: DateTime<Utc>) -> bool {
        now >= self.expires_at()
    }
}

/// Staging's settings. Production uses the defaults.
#[derive(Clone, Debug)]
pub struct StagingOptions {
    /// Replaces the measured free space of every staging root (tests).
    pub available_bytes: Option<u64>,
    /// The oldest `schemaVersion` accepted (D4). Always
    /// [`MIN_SCHEMA_VERSION`] in production; tests open it wider to stage
    /// an older schema, which this binary's window can't yet contain.
    pub min_schema_version: i64,
}

impl Default for StagingOptions {
    fn default() -> Self {
        Self {
            available_bytes: None,
            min_schema_version: MIN_SCHEMA_VERSION,
        }
    }
}

/// Why a restore couldn't be staged, previewed, or resolved.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum RestoreError {
    /// D3 and D4: `BACKUP_INVALID`, `UNSUPPORTED_BACKUP_FORMAT`, or
    /// `UNSUPPORTED_SCHEMA_VERSION`.
    Archive(ArchiveError),
    /// D3 rule 9 (`INSUFFICIENT_SPACE`, target `restoreStaging`).
    InsufficientSpace { required: u64, available: u64 },
    /// The staging id is unknown or expired (`RESTORE_STAGING_EXPIRED`).
    Expired { staging_id: String },
    /// A staging directory or the live Farm couldn't be read or written.
    Io,
}

impl RestoreError {
    fn invalid(reason: BackupInvalidReason, field_path: impl Into<String>) -> Self {
        RestoreError::Archive(ArchiveError::Invalid {
            reason,
            field_path: field_path.into(),
        })
    }
}

impl From<ArchiveError> for RestoreError {
    fn from(error: ArchiveError) -> Self {
        match error {
            ArchiveError::Sink(_) => RestoreError::Io,
            other => RestoreError::Archive(other),
        }
    }
}

impl From<RestoreError> for CommandError {
    fn from(error: RestoreError) -> Self {
        match error {
            RestoreError::Archive(ArchiveError::Invalid { reason, field_path }) => {
                CommandError::backup_invalid(reason, &field_path)
            }
            RestoreError::Archive(ArchiveError::UnsupportedFormat { received }) => {
                CommandError::unsupported_backup_format(received)
            }
            RestoreError::Archive(ArchiveError::UnsupportedSchema { received }) => {
                CommandError::unsupported_backup_schema(received)
            }
            RestoreError::Archive(ArchiveError::Sink(_)) | RestoreError::Io => {
                CommandError::persistence_unavailable()
            }
            RestoreError::InsufficientSpace {
                required,
                available,
            } => CommandError::insufficient_space(required, available, "restoreStaging"),
            RestoreError::Expired { staging_id } => {
                let echoed = if is_staging_id(&staging_id) {
                    staging_id.as_str()
                } else {
                    "stagingId"
                };
                CommandError::restore_staging_expired(echoed)
            }
        }
    }
}

fn io<E>(_: E) -> RestoreError {
    RestoreError::Io
}

fn database_invalid() -> RestoreError {
    RestoreError::invalid(BackupInvalidReason::DatabaseInvalid, "database")
}

fn migration_mismatch(field_path: impl Into<String>) -> RestoreError {
    RestoreError::invalid(BackupInvalidReason::MigrationMismatch, field_path)
}

/// Removes the three directories on drop (an error or a panic) unless
/// disarmed.
struct RemoveOnDrop(Option<StagingLayout>);

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(layout) = self.0.take() {
            layout.remove();
        }
    }
}

/// Stages the backup at `archive_path` under `paths` (D8 "Staging"). The
/// caller holds the lease for the whole call.
pub fn stage(
    paths: &StoragePaths,
    _lease: &LeaseGuard,
    archive_path: &Path,
    options: &StagingOptions,
) -> Result<StagedCandidate, RestoreError> {
    // Rules 1–8.
    let mut reader = archive::open_from_schema(archive_path, options.min_schema_version)?;
    let manifest = reader.manifest().clone();

    // Rule 9: what each root will hold, plus 10 %, must fit its free space.
    let mut per_root: BTreeMap<&Path, u64> = BTreeMap::new();
    for entry in &manifest.entries {
        let root = if entry.path == DATABASE_PATH {
            paths.snapshot_root()
        } else if entry.path.starts_with("content/") {
            paths.content_root()
        } else {
            paths.media_root()
        };
        let total = per_root.entry(root).or_default();
        *total = total.saturating_add(entry.bytes);
    }
    for (root, bytes) in per_root {
        let required = bytes.saturating_add(bytes.div_ceil(10));
        let available = match options.available_bytes {
            Some(available) => available,
            None => {
                fs::create_dir_all(root).map_err(io)?;
                available_bytes(root).map_err(io)?
            }
        };
        if required > available {
            return Err(RestoreError::InsufficientSpace {
                required,
                available,
            });
        }
    }

    let staging_id = new_staging_id();
    let layout = StagingLayout::new(paths, &staging_id);
    let mut cleanup = RemoveOnDrop(Some(layout.clone()));
    let database_dir = contained(
        paths.snapshot_root(),
        &Path::new(DATABASE_STAGING_DIRECTORY).join(&staging_id),
    )?;
    let content_dir = contained(
        paths.content_root(),
        &Path::new("staging").join(format!("{RESTORE_PREFIX}{staging_id}")),
    )?;
    let media_dir = contained(
        paths.media_root(),
        &Path::new(&format!("{RESTORE_PREFIX}{staging_id}")).join("snapshots"),
    )?
    .parent()
    .map(Path::to_path_buf)
    .ok_or(RestoreError::Io)?;

    // Rule 10: every entry streamed once, verified, then renamed in place.
    for (index, entry) in manifest.entries.iter().enumerate() {
        let (base, relative) = if entry.path == DATABASE_PATH {
            (&database_dir, PathBuf::from(CANDIDATE_FILE))
        } else if let Some(rest) = entry.path.strip_prefix("content/sha256/") {
            (&content_dir, PathBuf::from(rest))
        } else if let Some(rel_path) = entry.path.strip_prefix("media/") {
            (&media_dir, PathBuf::from(rel_path))
        } else {
            // Rule 7 admits no other shape.
            return Err(RestoreError::invalid(
                BackupInvalidReason::ManifestInvalid,
                format!("manifest.entries[{index}].path"),
            ));
        };
        let directory = match relative.parent() {
            Some(parent) if !parent.as_os_str().is_empty() => contained(base, parent)?,
            _ => base.clone(),
        };
        let file_name = relative.file_name().ok_or(RestoreError::Io)?;
        let target = directory.join(file_name);
        let part = directory.join(format!("{}{PART_SUFFIX}", file_name.to_string_lossy()));
        let file = File::options()
            .write(true)
            .create_new(true)
            .open(&part)
            .map_err(io)?;
        let mut sink = BufWriter::new(file);
        reader.copy_entry(index, &mut sink)?;
        let file = sink.into_inner().map_err(io)?;
        file.sync_all().map_err(io)?;
        drop(file);
        fs::rename(&part, &target).map_err(io)?;
        sync_directory(&directory).map_err(io)?;
    }
    drop(reader);

    // Rule 11 and D4, on the staged copy only.
    let migrated_from = validate_candidate(&layout.candidate_path(), &manifest)?;
    write_file_synced(&layout.manifest_path(), &manifest.to_json()).map_err(io)?;
    sync_directory(&layout.database_dir).map_err(io)?;

    cleanup.0 = None;
    Ok(StagedCandidate {
        staging_id,
        created_at: Utc::now(),
        manifest,
        layout,
        migrated_from,
    })
}

fn contained(base: &Path, relative: &Path) -> Result<PathBuf, RestoreError> {
    fs::create_dir_all(base).map_err(io)?;
    create_contained_directory(base, relative).map_err(io)
}

/// D4 on the staged candidate: its migrations and counts must match the
/// manifest, then it is migrated forward and must pass SQLite's checks, the
/// schema check, and the integrity catalogue. Returns the schema it was
/// migrated from, if any.
fn validate_candidate(path: &Path, manifest: &Manifest) -> Result<Option<i64>, RestoreError> {
    // Every manifest row is the binary's own migration of that version, and
    // the rows are exactly 1 through `schemaVersion`.
    let embedded = embedded_migrations();
    for (index, row) in manifest.migrations.iter().enumerate() {
        let matches = embedded
            .iter()
            .find(|(version, _, _)| *version == row.version)
            .is_some_and(|(_, name, checksum)| *name == row.name && *checksum == row.checksum);
        if !matches {
            return Err(migration_mismatch(format!("manifest.migrations[{index}]")));
        }
    }
    let versions: Vec<i64> = manifest.migrations.iter().map(|row| row.version).collect();
    if versions != (1..=manifest.schema_version).collect::<Vec<_>>() {
        return Err(migration_mismatch("manifest.migrations"));
    }

    let mut connection = open_candidate(path)?;
    let staged = super::inventory::migrations(&connection).map_err(|_| database_invalid())?;
    if staged != manifest.migrations {
        return Err(migration_mismatch("manifest.migrations"));
    }
    let user_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(|_| database_invalid())?;
    if user_version != manifest.schema_version {
        return Err(migration_mismatch("manifest.schemaVersion"));
    }

    // Before migrating: the same tables with the same counts.
    let counts = super::inventory::table_counts(&connection).map_err(|_| database_invalid())?;
    let tables: BTreeSet<&String> = counts.keys().chain(manifest.counts.keys()).collect();
    for table in tables {
        if counts.get(table) != manifest.counts.get(table) {
            let field_path = if is_plain_identifier(table) {
                format!("manifest.counts.{table}")
            } else {
                "manifest.counts".to_string()
            };
            return Err(RestoreError::invalid(
                BackupInvalidReason::CountMismatch,
                field_path,
            ));
        }
    }

    // Forward, with the live database's own migrations.
    let migrated_from =
        (manifest.schema_version < CURRENT_SCHEMA_VERSION).then_some(manifest.schema_version);
    apply_migrations(&mut connection).map_err(|_| database_invalid())?;

    // Nothing farm3d's schema doesn't have (a planted trigger or view).
    if schema_objects(&connection)? != fresh_schema_objects()? {
        return Err(database_invalid());
    }
    let check: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| database_invalid())?;
    if check != "ok" || has_foreign_key_violation(&connection).map_err(|_| database_invalid())? {
        return Err(database_invalid());
    }
    let report = integrity::check(&connection, None).map_err(|_| database_invalid())?;
    if !report.violations().is_empty() {
        return Err(database_invalid());
    }
    connection.close().map_err(|_| RestoreError::Io)?;
    File::open(path)
        .and_then(|file| file.sync_all())
        .map_err(io)?;
    Ok(migrated_from)
}

/// Opens the staged candidate read-write (it is migrated in place) as a
/// self-contained file: rollback journal, never WAL, and SQLite's defensive
/// mode, since its bytes came from outside.
fn open_candidate(path: &Path) -> Result<Connection, RestoreError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| database_invalid())?;
    connection
        .set_db_config(rusqlite::config::DbConfig::SQLITE_DBCONFIG_DEFENSIVE, true)
        .map_err(|_| database_invalid())?;
    let journal_mode: String = connection
        .query_row("PRAGMA journal_mode", [], |row| row.get(0))
        .map_err(|_| database_invalid())?;
    if !journal_mode.eq_ignore_ascii_case("delete") {
        return Err(database_invalid());
    }
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(|_| database_invalid())?;
    Ok(connection)
}

type SchemaObject = (String, String, String);

/// `(type, name, tbl_name)` of every schema object but the ones SQLite
/// creates itself (automatic indexes, `sqlite_sequence`, and `ANALYZE`'s
/// statistics).
fn schema_objects(connection: &Connection) -> Result<BTreeSet<SchemaObject>, RestoreError> {
    connection
        .prepare(
            "SELECT type, name, tbl_name FROM sqlite_schema
              WHERE name NOT LIKE 'sqlite\\_autoindex\\_%' ESCAPE '\\'
                AND name NOT IN ('sqlite_sequence', 'sqlite_stat1', 'sqlite_stat4')",
        )
        .and_then(|mut statement| {
            statement
                .query_map([], |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)))?
                .collect::<rusqlite::Result<BTreeSet<_>>>()
        })
        .map_err(|_| database_invalid())
}

/// The schema objects a freshly migrated database has.
fn fresh_schema_objects() -> Result<BTreeSet<SchemaObject>, RestoreError> {
    let mut connection = Connection::open_in_memory().map_err(io)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(io)?;
    apply_migrations(&mut connection).map_err(io)?;
    schema_objects(&connection)
}

fn is_plain_identifier(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= 64
        && name
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'_')
}

fn write_file_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let part = path.with_extension(format!(
        "{}{PART_SUFFIX}",
        path.extension()
            .map(|extension| extension.to_string_lossy().into_owned())
            .unwrap_or_default()
    ));
    let mut file = File::options().write(true).create_new(true).open(&part)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&part, path)
}

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

fn remove_path(path: &Path) -> io::Result<()> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_dir() => fs::remove_dir_all(path),
        Ok(_) => fs::remove_file(path),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

// --- the process's staging -------------------------------------------------------------

/// The process's one staging (D8: "at most one exists per process").
/// `preview_restore` replaces it, `discard_restore_preview` removes it, and
/// `apply_restore` resolves it by id.
#[derive(Default)]
pub struct Stagings {
    current: Mutex<Option<StagedCandidate>>,
}

impl Stagings {
    fn lock(&self) -> std::sync::MutexGuard<'_, Option<StagedCandidate>> {
        self.current
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// Makes `candidate` the staging, removing the one before it.
    pub fn replace(&self, candidate: StagedCandidate) {
        if let Some(previous) = self.lock().replace(candidate) {
            previous.layout.remove();
        }
    }

    /// Removes the staging, if any.
    pub fn clear(&self) {
        if let Some(previous) = self.lock().take() {
            previous.layout.remove();
        }
    }

    /// `discard_restore_preview`: removes the staging when it is
    /// `staging_id`. `false` when nothing was staged under that id.
    pub fn discard(&self, staging_id: &str) -> bool {
        let mut current = self.lock();
        match current.as_ref() {
            Some(candidate) if candidate.staging_id == staging_id => {
                let candidate = current.take().expect("matched above");
                candidate.layout.remove();
                true
            }
            _ => false,
        }
    }

    /// The staging `staging_id`, when it exists and hasn't expired at `now`
    /// (`RESTORE_STAGING_EXPIRED` otherwise).
    pub fn resolve(
        &self,
        staging_id: &str,
        now: DateTime<Utc>,
    ) -> Result<StagedCandidate, RestoreError> {
        let expired = || RestoreError::Expired {
            staging_id: staging_id.to_string(),
        };
        let current = self.lock();
        let candidate = current
            .as_ref()
            .filter(|candidate| candidate.staging_id == staging_id)
            .ok_or_else(expired)?;
        if candidate.is_expired(now) || !candidate.layout.candidate_path().is_file() {
            return Err(expired());
        }
        Ok(candidate.clone())
    }

    /// Takes the staging `staging_id` out (its directories stay), when it
    /// exists and hasn't expired at `now`: once a journal names it, it can
    /// no longer be discarded or replaced (`apply_restore`).
    pub fn take(
        &self,
        staging_id: &str,
        now: DateTime<Utc>,
    ) -> Result<StagedCandidate, RestoreError> {
        let mut current = self.lock();
        let valid = current.as_ref().is_some_and(|candidate| {
            candidate.staging_id == staging_id
                && !candidate.is_expired(now)
                && candidate.layout.candidate_path().is_file()
        });
        if !valid {
            return Err(RestoreError::Expired {
                staging_id: staging_id.to_string(),
            });
        }
        Ok(current.take().expect("checked above"))
    }

    /// Puts back a staging [`take`](Self::take) took out, when nothing
    /// replaced it meanwhile (`apply_restore` failed before its journal).
    pub fn put_back(&self, candidate: StagedCandidate) {
        let mut current = self.lock();
        if current.is_none() {
            *current = Some(candidate);
        }
    }

    /// The current staging's id.
    pub fn current_id(&self) -> Option<String> {
        self.lock()
            .as_ref()
            .map(|candidate| candidate.staging_id.clone())
    }
}

// --- startup ---------------------------------------------------------------------------

/// Which staging the restore journal protects at startup.
enum Protected {
    /// No journal, or a finished one: nothing.
    Nothing,
    /// A `pending` or `installing` journal names this staging.
    Staging(String),
    /// The journal can't be read: touch nothing (the installer refuses to
    /// start on it, D8).
    Everything,
}

/// `<metadata_root>/restore/journal.json`.
pub fn journal_path(paths: &StoragePaths) -> PathBuf {
    paths.metadata_root().join("restore").join("journal.json")
}

/// Reads only the journal's `phase` and `stagingId` (the installer, D8,
/// owns the rest of it).
fn protected(paths: &StoragePaths) -> Protected {
    let bytes = match fs::read(journal_path(paths)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Protected::Nothing,
        Err(_) => return Protected::Everything,
    };
    let Ok(journal) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return Protected::Everything;
    };
    if journal
        .get("journalVersion")
        .and_then(serde_json::Value::as_i64)
        != Some(1)
    {
        return Protected::Everything;
    }
    let phase = journal.get("phase").and_then(serde_json::Value::as_str);
    match (phase, journal.get("stagingId")) {
        (Some("pending" | "installing"), Some(serde_json::Value::String(id))) => {
            Protected::Staging(id.clone())
        }
        (Some("pending" | "installing"), Some(serde_json::Value::Null) | None) => {
            Protected::Nothing
        }
        (Some("pending" | "installing"), Some(_)) => Protected::Everything,
        (Some("installed" | "done" | "failed"), _) => Protected::Nothing,
        _ => Protected::Everything,
    }
}

/// D8 "Staging", at startup: removes every staging directory of all three
/// kinds, except the one a `pending` or `installing` journal names. Called
/// by F1's `cleanup_restore_staging` from `Storage::open`.
pub fn cleanup(paths: &StoragePaths) -> Result<(), StorageError> {
    let keep = match protected(paths) {
        Protected::Everything => return Ok(()),
        Protected::Nothing => None,
        Protected::Staging(id) => Some(id),
    };
    let kept = |name: &str| keep.as_deref() == Some(name);

    let database_root = paths.snapshot_root().join(DATABASE_STAGING_DIRECTORY);
    create_contained_directory(paths.snapshot_root(), Path::new(DATABASE_STAGING_DIRECTORY))?;
    for entry in fs::read_dir(&database_root)? {
        let entry = entry?;
        if !kept(&entry.file_name().to_string_lossy()) {
            remove_path(&entry.path())?;
        }
    }
    for directory in [
        paths.content_root().join("staging"),
        paths.media_root().to_path_buf(),
    ] {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let Some(id) = name.strip_prefix(RESTORE_PREFIX) else {
                continue;
            };
            if !kept(id) {
                remove_path(&entry.path())?;
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn staging_ids_are_stg_and_a_hyphenated_uuid() {
        let id = new_staging_id();
        assert!(is_staging_id(&id), "{id}");
        for bad in [
            "",
            "stg-",
            "stg-../../etc",
            "stg-00000000000040008000000000000000",
            "rst-00000000-0000-4000-8000-000000000000",
            "stg-00000000-0000-4000-8000-000000000000/x",
            "stg-00000000-0000-4000-8000-00000000000G",
        ] {
            assert!(!is_staging_id(bad), "{bad}");
        }
    }
}
