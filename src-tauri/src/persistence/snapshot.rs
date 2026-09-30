use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rusqlite::backup::Backup;
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use uuid::Uuid;

use super::database::{create_private_directory, create_private_file, Storage, StoragePaths};
use super::error::StorageError;
use super::migrations;

const SNAPSHOT_PREFIX: &str = ".farm3d-pre-import-";
const RESTORE_STAGING_DIRECTORY: &str = crate::backup::staging::DATABASE_STAGING_DIRECTORY;
const RETAINED_SNAPSHOTS: usize = 5;

#[derive(Clone, Copy)]
pub enum SnapshotKind {
    Settings,
    Printers,
}

impl SnapshotKind {
    fn name(self) -> &'static str {
        match self {
            Self::Settings => "settings",
            Self::Printers => "printers",
        }
    }
}

pub struct Snapshot {
    path: PathBuf,
}

impl Snapshot {
    pub fn path(&self) -> &Path {
        &self.path
    }
}

#[derive(Debug, PartialEq, Eq)]
pub struct ValidationSummary {
    pub(crate) schema_version: i64,
    pub(crate) integrity_ok: bool,
    pub(crate) foreign_keys_ok: bool,
}

impl ValidationSummary {
    pub(crate) const CURRENT: Self = Self {
        schema_version: migrations::CURRENT_SCHEMA_VERSION,
        integrity_ok: true,
        foreign_keys_ok: true,
    };
}

pub struct StagedRestore {
    staging_id: String,
    validation: ValidationSummary,
}

impl StagedRestore {
    pub fn staging_id(&self) -> &str {
        &self.staging_id
    }

    pub fn validation(&self) -> &ValidationSummary {
        &self.validation
    }
}

impl Storage {
    pub fn create_snapshot(&self, kind: SnapshotKind) -> Result<Snapshot, StorageError> {
        if self.take_failure(super::database::FailurePoint::Snapshot) {
            return Err(StorageError::Filesystem);
        }
        let _snapshot_guard = self.lock_snapshot()?;
        cleanup_partial_snapshots(self.paths.snapshot_root())?;
        let timestamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_err(|_| StorageError::Filesystem)?
            .as_nanos();
        let stem = format!(
            "{SNAPSHOT_PREFIX}{}-{timestamp:020}-{}",
            kind.name(),
            Uuid::new_v4()
        );
        let partial = self
            .paths
            .snapshot_root()
            .join(format!("{stem}.sqlite3.partial"));
        let accepted = self.paths.snapshot_root().join(format!("{stem}.sqlite3"));

        self.create_snapshot_to(&partial)?;
        fs::rename(&partial, &accepted)?;
        sync_directory(self.paths.snapshot_root())?;
        if retain_newest_snapshots(self.paths.snapshot_root(), &accepted, |path| {
            self.remove_retained_snapshot(path)
        })
        .is_err()
        {
            let _ = self.write(|transaction| {
                transaction.execute(
                    "INSERT INTO migration_warnings(
                        id, code, source_name, source_sha256, message, details_json, created_at
                     ) VALUES (
                        ?1, 'SNAPSHOT_RETENTION_FAILED', NULL, NULL,
                        'An old safety snapshot could not be removed', '{}',
                        strftime('%Y-%m-%dT%H:%M:%fZ', 'now')
                     ) ON CONFLICT DO NOTHING",
                    [Uuid::new_v4().to_string()],
                )?;
                Ok(())
            });
        }
        Ok(Snapshot { path: accepted })
    }

    /// Copies the live database to `path` with the online backup API
    /// (ADR-0008: never a file copy of the database and its WAL), in one
    /// step, so the copy is the Farm at one instant. Then validates the copy
    /// (schema, `integrity_check`, `foreign_key_check`) and `fsync`s it.
    /// `path` must not name an existing non-file or a symlink. Returns the
    /// validated copy; on failure the caller removes `path`.
    pub fn create_snapshot_to(&self, path: &Path) -> Result<Snapshot, StorageError> {
        let source = self.open_reader()?;
        backup_database(&source, path)?;
        drop(source);
        validate_database(path)?;
        File::open(path)?.sync_all()?;
        Ok(Snapshot {
            path: path.to_path_buf(),
        })
    }

    pub fn stage_restore(&self, selected_snapshot: &Path) -> Result<StagedRestore, StorageError> {
        let _snapshot_guard = self.lock_snapshot()?;
        let selected_snapshot =
            accepted_snapshot_path(self.paths.snapshot_root(), selected_snapshot)?;
        validate_database(&selected_snapshot)?;

        let staging_id = crate::backup::staging::new_staging_id();
        let staging_directory = self
            .paths
            .snapshot_root()
            .join(RESTORE_STAGING_DIRECTORY)
            .join(&staging_id);
        let staging_root = self.paths.snapshot_root().join(RESTORE_STAGING_DIRECTORY);
        create_private_directory(&staging_directory)?;
        let candidate_path = staging_directory.join("candidate.sqlite3");
        let result = (|| {
            let source = open_read_only(&selected_snapshot)?;
            backup_database(&source, &candidate_path)?;
            let validation = validate_database(&candidate_path)?;
            File::open(&candidate_path)?.sync_all()?;
            sync_directory(&staging_directory)?;
            sync_directory(&staging_root)?;
            Ok(StagedRestore {
                staging_id,
                validation,
            })
        })();

        if result.is_err() {
            let _ = fs::remove_dir_all(&staging_directory);
        }
        result
    }
}

/// Startup: removes partial pre-import snapshots, then every restore
/// staging directory (all three kinds, P9 D8) except the one a `pending` or
/// `installing` restore journal names. A staged candidate is never kept
/// only for being valid: the process that staged it is gone.
pub(super) fn cleanup_restore_staging(paths: &StoragePaths) -> Result<(), StorageError> {
    cleanup_partial_snapshots(paths.snapshot_root())?;
    crate::backup::staging::cleanup(paths)
}

fn backup_database(source: &Connection, destination: &Path) -> Result<(), StorageError> {
    create_private_file(destination)?;
    let mut destination_connection = Connection::open(destination)?;
    let backup = Backup::new(source, &mut destination_connection)?;
    // Every page in one step: a step that spans a source write restarts,
    // and one step reads a single snapshot of the source.
    backup.run_to_completion(i32::MAX, Duration::from_millis(1), None)?;
    drop(backup);
    destination_connection
        .close()
        .map_err(|_| StorageError::Database)?;
    Ok(())
}

pub(super) fn validate_database(path: &Path) -> Result<ValidationSummary, StorageError> {
    validate_database_with_schema_hook(path, || Ok(()))
}

fn validate_database_with_schema_hook(
    path: &Path,
    after_schema: impl FnOnce() -> Result<(), StorageError>,
) -> Result<ValidationSummary, StorageError> {
    let mut connection = open_read_only(path).map_err(|_| StorageError::InvalidSnapshot)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Deferred)
        .map_err(|_| StorageError::InvalidSnapshot)?;
    migrations::validate(&transaction)?;
    after_schema()?;
    let integrity: String = transaction
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(|_| StorageError::InvalidSnapshot)?;
    if integrity != "ok" {
        return Err(StorageError::InvalidSnapshot);
    }
    if migrations::has_foreign_key_violation(&transaction)
        .map_err(|_| StorageError::InvalidSnapshot)?
    {
        return Err(StorageError::InvalidSnapshot);
    }
    transaction
        .commit()
        .map_err(|_| StorageError::InvalidSnapshot)?;
    Ok(ValidationSummary::CURRENT)
}

#[cfg(test)]
pub(super) fn validate_database_during_test(
    path: &Path,
    after_schema: impl FnOnce() -> Result<(), StorageError>,
) -> Result<ValidationSummary, StorageError> {
    validate_database_with_schema_hook(path, after_schema)
}

fn open_read_only(path: &Path) -> Result<Connection, StorageError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    connection.busy_timeout(Duration::from_secs(5))?;
    Ok(connection)
}

fn accepted_snapshot_path(root: &Path, selected: &Path) -> Result<PathBuf, StorageError> {
    let root = root
        .canonicalize()
        .map_err(|_| StorageError::InvalidSnapshot)?;
    let selected = selected
        .canonicalize()
        .map_err(|_| StorageError::InvalidSnapshot)?;
    let name = selected
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or(StorageError::InvalidSnapshot)?;
    if selected.parent() != Some(root.as_path())
        || !name.starts_with(SNAPSHOT_PREFIX)
        || !name.ends_with(".sqlite3")
    {
        return Err(StorageError::InvalidSnapshot);
    }
    Ok(selected)
}

fn cleanup_partial_snapshots(root: &Path) -> Result<(), StorageError> {
    for entry in fs::read_dir(root)? {
        let entry = entry?;
        let path = entry.path();
        if path.is_file()
            && path
                .file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(".partial"))
        {
            fs::remove_file(path)?;
        }
    }
    Ok(())
}

fn retain_newest_snapshots(
    root: &Path,
    protected: &Path,
    mut remove: impl FnMut(&Path) -> Result<(), StorageError>,
) -> Result<(), StorageError> {
    let mut snapshots = Vec::new();
    for entry in fs::read_dir(root)? {
        let path = entry?.path();
        let has_snapshot_name = path
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(".sqlite3"));
        if has_snapshot_name && validate_database(&path).is_ok() {
            let timestamp = snapshot_timestamp(&path).ok_or(StorageError::InvalidSnapshot)?;
            snapshots.push((timestamp, path));
        }
    }
    snapshots.sort_unstable();
    let remove_count = snapshots.len().saturating_sub(RETAINED_SNAPSHOTS);
    let mut removed = 0;
    for (_, obsolete) in snapshots {
        if removed == remove_count {
            break;
        }
        if obsolete != protected {
            remove(&obsolete)?;
            removed += 1;
        }
    }
    Ok(())
}

/// P9 D14: every accepted pre-import snapshot in `root` (a regular file
/// named `.farm3d-pre-import-*.sqlite3`), oldest first. A name whose
/// timestamp can't be read sorts first, so it is never taken for the
/// newest.
pub fn accepted_snapshots(root: &Path) -> Vec<PathBuf> {
    let mut snapshots: Vec<(u128, PathBuf)> = fs::read_dir(root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|entry| entry.file_type().is_ok_and(|kind| kind.is_file()))
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .is_some_and(|name| {
                    name.starts_with(SNAPSHOT_PREFIX) && name.ends_with(".sqlite3")
                })
        })
        .map(|path| (snapshot_timestamp(&path).unwrap_or(0), path))
        .collect();
    snapshots.sort_unstable();
    snapshots.into_iter().map(|(_, path)| path).collect()
}

fn snapshot_timestamp(path: &Path) -> Option<u128> {
    let name = path.file_name()?.to_str()?;
    let remainder = ["settings", "printers"]
        .iter()
        .find_map(|domain| name.strip_prefix(&format!("{SNAPSHOT_PREFIX}{domain}-")))?;
    let (timestamp, _) = remainder.split_once('-')?;
    timestamp.parse().ok()
}

fn sync_directory(path: &Path) -> Result<(), StorageError> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}
