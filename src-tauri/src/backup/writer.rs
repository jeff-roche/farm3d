//! D5's writer, used by `create_backup` and by every safety backup (D9).
//!
//! 1. The caller holds the lease ([`LeaseGuard`]). The estimate (the live
//!    database's pages, every blob, the selected snapshots) plus 10 % must
//!    fit the destination's free space (`INSUFFICIENT_SPACE`).
//! 2. The live database is copied with the online backup API into a
//!    private working directory, `<backup_root>/tmp/<uuid>/`.
//! 3. The copy's blob rows and selected snapshot rows are read, and
//! 4. the media pre-pass hashes each selected file.
//! 5. The copy is sanitized in one transaction, then `VACUUM`ed.
//! 6. The copy is validated (`integrity_check`, `foreign_key_check`,
//!    `integrity::check` with the live roots). A violation means the local
//!    Farm is damaged: `BACKUP_SOURCE_DAMAGED`.
//! 7. The manifest is built, then the zip is streamed into a random
//!    temporary file beside the destination: `manifest.json`, the
//!    database, each blob (hashed as it is copied), each included image.
//! 8. The file is `fsync`ed and atomically replaces the destination; the
//!    directory is `fsync`ed and the working directory removed. Any
//!    failure removes the temporary file and the working directory, so the
//!    destination is never left half-written.

use std::collections::BTreeSet;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufWriter};
use std::path::{Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;

use super::archive::{content_path, media_path, ArchiveWriter, EntryCompression, DATABASE_PATH};
use super::inventory::{self, BlobItem, MediaItem};
use super::lease::LeaseGuard;
use super::manifest::{
    Manifest, ManifestContents, ManifestEntry, ManifestMediaCounts, ManifestPlatform, FORMAT,
    FORMAT_VERSION,
};
use super::{BackupExcludedClass, BackupMediaChoice, BackupOrigin};
use crate::contracts::command::CommandError;
use crate::persistence::integrity::{self, IntegrityRoots, IntegrityRule};
use crate::persistence::{create_contained_directory, Storage, StorageError, StoragePaths};

/// A hook run with no argument.
pub type Hook = Box<dyn Fn() + Send + Sync>;
/// A hook run on a file.
pub type PathHook = Box<dyn Fn(&Path) + Send + Sync>;

/// Test hooks. Production leaves every one empty.
#[derive(Default)]
pub struct WriterHooks {
    /// Runs after the database copy (step 2), before any file is read.
    pub after_database_copy: Option<Hook>,
    /// Replaces the destination's measured free space.
    pub available_bytes: Option<u64>,
    /// Runs on a safety backup's file after it is written, before it is
    /// verified (D9).
    pub before_verify: Option<PathHook>,
}

/// What to write.
#[derive(Clone, Debug)]
pub struct BackupRequest {
    pub media: BackupMediaChoice,
    pub origin: BackupOrigin,
    /// The manifest's `createdAt`, every entry's modification time, and
    /// the `pruned_at` of every snapshot the backup leaves out. `None`
    /// stamps it at the database copy (the backup's point in time), which
    /// is what an operator backup wants: anything captured before the copy
    /// is then never "pruned" before it was captured. Tests and fixtures
    /// pass a fixed time.
    pub created_at: Option<DateTime<Utc>>,
    pub app_version: String,
}

/// A written, `fsync`ed backup.
#[derive(Clone, Debug)]
pub struct WrittenBackup {
    pub manifest: Manifest,
    /// The archive's length on disk.
    pub bytes: u64,
}

#[derive(Clone, PartialEq, Eq, Debug)]
pub enum BackupWriteError {
    /// The estimate plus 10 % doesn't fit.
    InsufficientSpace { required: u64, available: u64 },
    /// A file the Farm uses is missing or damaged: `entry` is `database` or
    /// the archive path of the first bad file.
    SourceDamaged { entry: String },
    /// Reading the Farm or writing the destination failed.
    Io,
}

impl BackupWriteError {
    /// The command error, with `INSUFFICIENT_SPACE`'s `target`.
    pub fn into_command_error(self, target: &str) -> CommandError {
        match self {
            BackupWriteError::InsufficientSpace {
                required,
                available,
            } => CommandError::insufficient_space(required, available, target),
            BackupWriteError::SourceDamaged { entry } => {
                CommandError::backup_source_damaged(&entry)
            }
            BackupWriteError::Io => CommandError::persistence_unavailable(),
        }
    }
}

/// `create_backup`'s mapping (`target: backupDestination`).
impl From<BackupWriteError> for CommandError {
    fn from(error: BackupWriteError) -> Self {
        error.into_command_error("backupDestination")
    }
}

fn io_error<E>(_: E) -> BackupWriteError {
    BackupWriteError::Io
}

fn database_damaged() -> BackupWriteError {
    BackupWriteError::SourceDamaged {
        entry: "database".to_string(),
    }
}

/// The free space available to this user under `path`.
pub fn available_bytes(path: &Path) -> io::Result<u64> {
    #[cfg(unix)]
    {
        let stat = rustix::fs::statvfs(path)?;
        Ok(stat.f_bavail.saturating_mul(stat.f_frsize))
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let mut available: u64 = 0;
        // SAFETY: `wide` is NUL-terminated and outlives the call; the
        // output pointer is valid; the two optional outputs are null.
        let ok = unsafe {
            GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut available,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            Err(io::Error::last_os_error())
        } else {
            Ok(available)
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = path;
        Ok(u64::MAX)
    }
}

/// D9: removes what a crashed backup left in farm3d's own directories:
/// every working directory under `<backup_root>/tmp/`, and every
/// temporary archive in `<backup_root>/safety/`. Startup runs it before
/// any command is served (the installer, Task 7, runs before it). Best
/// effort: anything left is retried at the next start.
pub fn clear_working_files(paths: &StoragePaths) {
    let backup_root = paths.backup_root();
    if let Ok(tmp) = create_contained_directory(backup_root, Path::new("tmp")) {
        for entry in fs::read_dir(&tmp).into_iter().flatten().flatten() {
            let path = entry.path();
            let _ = match entry.file_type() {
                Ok(file_type) if file_type.is_dir() => fs::remove_dir_all(&path),
                _ => fs::remove_file(&path),
            };
        }
    }
    if let Ok(safety) = create_contained_directory(backup_root, Path::new("safety")) {
        for entry in fs::read_dir(&safety).into_iter().flatten().flatten() {
            let is_temporary = entry.file_name().to_str().is_some_and(|name| {
                name.starts_with(TEMPORARY_PREFIX) && name.ends_with(TEMPORARY_SUFFIX)
            });
            if is_temporary && entry.file_type().is_ok_and(|file_type| file_type.is_file()) {
                let _ = fs::remove_file(entry.path());
            }
        }
    }
}

const TEMPORARY_PREFIX: &str = ".farm3d-backup-";
const TEMPORARY_SUFFIX: &str = ".tmp";

fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

/// Writes a backup of `storage`'s Farm to `destination` (D5). The caller
/// holds `_lease` for the whole call.
pub fn write_backup(
    storage: &Storage,
    _lease: &LeaseGuard,
    destination: &Path,
    request: &BackupRequest,
    hooks: &WriterHooks,
) -> Result<WrittenBackup, BackupWriteError> {
    let directory = destination.parent().ok_or(BackupWriteError::Io)?;

    // Step 1: the free-space check.
    let estimate = storage
        .read(|connection| inventory::estimate(connection, request.media))
        .map_err(io_error)?;
    let required = estimate.saturating_add(estimate.div_ceil(10));
    let available = match hooks.available_bytes {
        Some(bytes) => bytes,
        None => available_bytes(directory).map_err(io_error)?,
    };
    if required > available {
        return Err(BackupWriteError::InsufficientSpace {
            required,
            available,
        });
    }

    // Step 2's private working directory.
    let tmp_root = create_contained_directory(storage.paths().backup_root(), Path::new("tmp"))
        .map_err(io_error)?;
    let work = create_contained_directory(&tmp_root, Path::new(&uuid::Uuid::new_v4().to_string()))
        .map_err(io_error)?;
    // Removed on every exit, a panic included.
    let _work = RemoveOnDrop::directory(work.clone());
    write_in(storage, &work, destination, directory, request, hooks)
}

/// Removes a path when dropped (error returns and panics alike) unless
/// [`disarm`](Self::disarm)ed.
struct RemoveOnDrop {
    path: Option<PathBuf>,
    directory: bool,
}

impl RemoveOnDrop {
    fn directory(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            directory: true,
        }
    }

    fn file(path: PathBuf) -> Self {
        Self {
            path: Some(path),
            directory: false,
        }
    }

    fn disarm(&mut self) {
        self.path = None;
    }
}

impl Drop for RemoveOnDrop {
    fn drop(&mut self) {
        if let Some(path) = self.path.take() {
            let _ = if self.directory {
                fs::remove_dir_all(path)
            } else {
                fs::remove_file(path)
            };
        }
    }
}

/// Everything the copy tells the manifest.
struct Copied {
    copy: PathBuf,
    created_at: DateTime<Utc>,
    database: ManifestEntry,
    blobs: Vec<BlobItem>,
    media: Vec<MediaItem>,
    manifest: Manifest,
}

fn write_in(
    storage: &Storage,
    work: &Path,
    destination: &Path,
    directory: &Path,
    request: &BackupRequest,
    hooks: &WriterHooks,
) -> Result<WrittenBackup, BackupWriteError> {
    let copied = copy_and_describe(storage, work, request, hooks)?;
    let temporary = directory.join(format!(
        "{TEMPORARY_PREFIX}{}{TEMPORARY_SUFFIX}",
        uuid::Uuid::new_v4()
    ));
    let file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&temporary)
        .map_err(io_error)?;
    // Removed on an error or a panic until the rename has happened.
    let mut cleanup = RemoveOnDrop::file(temporary.clone());
    stream_archive(storage, &copied, file)?;
    crate::document_io::atomic_replace(&temporary, destination).map_err(io_error)?;
    cleanup.disarm();
    sync_directory(directory).map_err(io_error)?;
    let bytes = fs::metadata(destination).map_err(io_error)?.len();
    Ok(WrittenBackup {
        manifest: copied.manifest,
        bytes,
    })
}

/// Steps 2–6, then the manifest (step 7's first half).
fn copy_and_describe(
    storage: &Storage,
    work: &Path,
    request: &BackupRequest,
    hooks: &WriterHooks,
) -> Result<Copied, BackupWriteError> {
    let copy = work.join("farm3d.sqlite3");
    storage
        .create_snapshot_to(&copy)
        .map_err(|error| match error {
            // The live database itself fails validation.
            StorageError::InvalidSnapshot | StorageError::MigrationFailed => database_damaged(),
            _ => BackupWriteError::Io,
        })?;
    // The copy is the backup's point in time.
    let created_at_time = request.created_at.unwrap_or_else(Utc::now);
    if let Some(hook) = &hooks.after_database_copy {
        hook();
    }

    let created_at = created_at_time.to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut connection = Connection::open(&copy).map_err(io_error)?;
    // A self-contained file: no `-wal` beside it.
    connection
        .query_row("PRAGMA journal_mode = DELETE", [], |_| Ok(()))
        .map_err(io_error)?;
    connection
        .pragma_update(None, "foreign_keys", "ON")
        .map_err(io_error)?;

    // Steps 3 and 4.
    let blobs = inventory::blobs(&connection).map_err(io_error)?;
    let selected = inventory::selected_media(&connection, request.media).map_err(io_error)?;
    let (media, missing) = inventory::media_prepass(storage.paths().media_root(), selected);

    // Step 5.
    let included: BTreeSet<String> = media.iter().map(|item| item.id.clone()).collect();
    let missing: BTreeSet<String> = missing.into_iter().collect();
    let sanitized =
        inventory::sanitize(&mut connection, &created_at, &included, &missing).map_err(io_error)?;
    connection.execute_batch("VACUUM").map_err(io_error)?;

    // Step 6.
    let check: String = connection
        .query_row("PRAGMA integrity_check", [], |row| row.get(0))
        .map_err(io_error)?;
    if check != "ok" {
        return Err(database_damaged());
    }
    let roots = IntegrityRoots::from_paths(storage.paths());
    let report = integrity::check(&connection, Some(&roots)).map_err(io_error)?;
    if let Some(violation) = report.violations().first() {
        let sample = violation.sample.first().cloned().unwrap_or_default();
        let entry = match violation.rule {
            IntegrityRule::BlobFile => content_path(&sample),
            IntegrityRule::MediaFile => connection
                .query_row(
                    "SELECT rel_path FROM camera_snapshots WHERE id = ?1",
                    [&sample],
                    |row| row.get::<_, String>(0),
                )
                .map(|rel_path| media_path(&rel_path))
                .unwrap_or_else(|_| "database".to_string()),
            _ => "database".to_string(),
        };
        return Err(BackupWriteError::SourceDamaged { entry });
    }
    let schema_version: i64 = connection
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .map_err(io_error)?;
    let migrations = inventory::migrations(&connection).map_err(io_error)?;
    let credential_ref_count = inventory::credential_ref_count(&connection).map_err(io_error)?;
    connection.close().map_err(io_error)?;
    let (database_bytes, database_sha256) =
        inventory::hash_file(&copy).ok_or(BackupWriteError::Io)?;

    let database = ManifestEntry {
        path: DATABASE_PATH.to_string(),
        bytes: database_bytes,
        sha256: database_sha256,
    };
    let mut entries = vec![database.clone()];
    entries.extend(blobs.iter().map(|blob| ManifestEntry {
        path: content_path(&blob.sha256),
        bytes: blob.bytes,
        sha256: blob.sha256.clone(),
    }));
    entries.extend(media.iter().map(|item| ManifestEntry {
        path: media_path(&item.rel_path),
        bytes: item.bytes,
        sha256: item.sha256.clone(),
    }));
    let manifest = Manifest {
        format: FORMAT.to_string(),
        format_version: FORMAT_VERSION,
        created_at,
        app_version: request.app_version.clone(),
        schema_version,
        migrations,
        platform: ManifestPlatform::current(),
        origin: request.origin,
        contents: ManifestContents {
            media: request.media,
        },
        counts: report.counts,
        excluded: BackupExcludedClass::ALL.to_vec(),
        credential_ref_count,
        media: ManifestMediaCounts {
            included: media.len() as i64,
            not_in_backup: sanitized.not_in_backup,
            missing_file: sanitized.missing_file,
        },
        entries,
    };
    Ok(Copied {
        copy,
        created_at: created_at_time,
        database,
        blobs,
        media,
        manifest,
    })
}

/// Opens a live file the manifest names; a missing or non-regular file is
/// `BACKUP_SOURCE_DAMAGED` at `entry`.
fn open_source(path: &Path, entry: &str) -> Result<File, BackupWriteError> {
    let damaged = || BackupWriteError::SourceDamaged {
        entry: entry.to_string(),
    };
    let metadata = fs::symlink_metadata(path).map_err(|_| damaged())?;
    if !metadata.is_file() {
        return Err(damaged());
    }
    File::open(path).map_err(|_| damaged())
}

/// Step 7's second half and step 8's `fsync`: streams every entry, and
/// checks each written length and hash against the manifest.
fn stream_archive(storage: &Storage, copied: &Copied, file: File) -> Result<(), BackupWriteError> {
    let mut writer =
        ArchiveWriter::new(BufWriter::new(file), copied.created_at).map_err(io_error)?;
    writer.write_manifest(&copied.manifest).map_err(io_error)?;

    let database = File::open(&copied.copy).map_err(io_error)?;
    let written = writer
        .write_entry(
            DATABASE_PATH,
            database,
            EntryCompression::Deflated,
            copied.database.bytes,
        )
        .map_err(io_error)?;
    if written != (copied.database.bytes, copied.database.sha256.clone()) {
        return Err(BackupWriteError::Io);
    }

    let blobs_root = storage.paths().content_root().join("blobs/sha256");
    for blob in &copied.blobs {
        let entry = content_path(&blob.sha256);
        let source = open_source(
            &blobs_root.join(&blob.sha256[..2]).join(&blob.sha256),
            &entry,
        )?;
        let compression = if blob.stored {
            EntryCompression::Stored
        } else {
            EntryCompression::Deflated
        };
        let written = writer
            .write_entry(&entry, source, compression, blob.bytes)
            .map_err(io_error)?;
        if written != (blob.bytes, blob.sha256.clone()) {
            return Err(BackupWriteError::SourceDamaged { entry });
        }
    }

    let media_root = storage.paths().media_root();
    for item in &copied.media {
        let entry = media_path(&item.rel_path);
        let source = open_source(&media_root.join(&item.rel_path), &entry)?;
        let written = writer
            .write_entry(&entry, source, EntryCompression::Stored, item.bytes)
            .map_err(io_error)?;
        if written != (item.bytes, item.sha256.clone()) {
            return Err(BackupWriteError::SourceDamaged { entry });
        }
    }

    let file = writer
        .finish()
        .map_err(io_error)?
        .into_inner()
        .map_err(io_error)?;
    file.sync_all().map_err(io_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_panic_mid_backup_leaves_no_working_directory() {
        let (temp, _lease, storage) = crate::test_storage();
        let lease = crate::backup::lease::BackupLease::new();
        let guard = lease
            .try_acquire(crate::backup::lease::LeaseActivity::Backup)
            .unwrap();
        let destination = temp.path().join("out.farm3d-backup");
        let hooks = WriterHooks {
            after_database_copy: Some(Box::new(|| panic!("injected"))),
            ..WriterHooks::default()
        };
        let request = BackupRequest {
            media: BackupMediaChoice::All,
            origin: BackupOrigin::Operator,
            created_at: None,
            app_version: "0.1.0".to_string(),
        };
        let outcome = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            write_backup(&storage, &guard, &destination, &request, &hooks)
        }));
        assert!(outcome.is_err());
        let tmp = storage.paths().backup_root().join("tmp");
        assert_eq!(
            fs::read_dir(&tmp).unwrap().count(),
            0,
            "working directory left"
        );
        assert!(!destination.exists());
    }

    #[test]
    fn startup_clears_working_directories_and_temporary_safety_archives() {
        let (_temp, _lease, storage) = crate::test_storage();
        let paths = storage.paths();
        let work = paths.backup_root().join("tmp").join("crashed");
        fs::create_dir_all(&work).unwrap();
        fs::write(work.join("farm3d.sqlite3"), b"copy").unwrap();
        let safety = paths.backup_root().join("safety");
        fs::create_dir_all(&safety).unwrap();
        let temporary = safety.join(".farm3d-backup-1234.tmp");
        fs::write(&temporary, b"half").unwrap();
        let kept = safety.join("sfb-1.farm3d-backup");
        fs::write(&kept, b"a backup").unwrap();

        clear_working_files(paths);

        assert!(!work.exists());
        assert!(!temporary.exists());
        assert!(kept.exists(), "a finished safety backup stays");
    }
}
