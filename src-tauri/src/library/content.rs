//! P4 content-addressed store (spec D2, D3, D4). Every Model Source
//! Revision's exact bytes, and every thumbnail, live once per SHA-256 at
//! `<content_root>/blobs/sha256/<first 2 hex>/<64 hex>`.
//!
//! The write protocol:
//! 1. [`ContentStore::stage_from_path`] streams the source into
//!    `staging/<key>/<index>.part` in 1 MiB chunks, hashing as it copies,
//!    then `fsync`s the copy.
//! 2. It re-stats the source and discards the copy if the size or
//!    modification time moved while it was being read.
//! 3. The caller inspects the staged bytes, never the live file.
//! 4. [`ContentStore::place_and_commit`] takes the placement lock. It
//!    renames each staged file to its blob path, or deletes the staged file
//!    if a blob of the same size is already there.
//! 5. In the same critical section, it commits one write transaction that
//!    inserts the `content_blobs` rows and whatever the caller's closure
//!    writes.
//!
//! A blob file can exist without a row (a crash between steps 4 and 5),
//! but a row never exists without its file. [`ContentStore::startup_sweep`]
//! removes the leftovers before any command is served.

use std::collections::HashSet;
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU8, Ordering};
use std::sync::{Arc, Barrier, Mutex};
use std::time::SystemTime;

use rusqlite::{params, Transaction};
use sha2::{Digest, Sha256};

use crate::contracts::command::CommandError;
use crate::persistence::{create_contained_directory, RepositoryError, Storage, StorageError};
use crate::printers::now_rfc3339;

/// D4: the largest source file farm3d copies into the store. Larger files
/// are rejected with `TOO_LARGE` before any byte is read.
pub const MAX_SOURCE_BYTES: u64 = 1024 * 1024 * 1024;

const CHUNK_BYTES: usize = 1024 * 1024;
const BLOB_DIRECTORY: &str = "blobs/sha256";
const STAGING_DIRECTORY: &str = "staging";
const PART_SUFFIX: &str = ".part";
const MAX_STAGING_COMPONENT: usize = 128;

/// A cooperative cancellation signal for long-running file work (staging
/// here, inspection in `library::formats`). The owner of the matching
/// `watch::Sender` flips it to `true`.
#[derive(Clone, Debug)]
pub struct CancelFlag(tokio::sync::watch::Receiver<bool>);

impl CancelFlag {
    pub fn new(receiver: tokio::sync::watch::Receiver<bool>) -> Self {
        Self(receiver)
    }

    /// A flag that is never cancelled, for work with no cancel control
    /// (for example a linked-source capture).
    pub fn never() -> Self {
        Self(tokio::sync::watch::channel(false).1)
    }

    pub fn is_cancelled(&self) -> bool {
        *self.0.borrow()
    }
}

/// One file copied into staging: its path under `staging/`, the lowercase
/// hex SHA-256 of the bytes, and their length. `source` is the source file's
/// stat as the copy saw it, for a file staged from a path; in-memory bytes
/// (a thumbnail) have none.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedFile {
    pub path: PathBuf,
    pub sha256: String,
    pub size: u64,
    pub source: Option<SourceStat>,
}

/// D6's change-detection identity for a source file: its size, its
/// modification time, and, on Unix, `dev:ino` of the resolved file. On
/// other platforms the file id is omitted until they are verified.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SourceStat {
    pub size: u64,
    pub modified: Option<SystemTime>,
    pub file_id: Option<String>,
}

impl SourceStat {
    pub fn of(metadata: &fs::Metadata) -> Self {
        #[cfg(unix)]
        let file_id = {
            use std::os::unix::fs::MetadataExt;
            Some(format!("{}:{}", metadata.dev(), metadata.ino()))
        };
        #[cfg(not(unix))]
        let file_id = None;
        Self {
            size: metadata.len(),
            modified: modified(metadata),
            file_id,
        }
    }

    /// The modification time in nanoseconds since the Unix epoch, when it
    /// is known and fits.
    pub fn modified_ns(&self) -> Option<i64> {
        let since_epoch = self.modified?.duration_since(SystemTime::UNIX_EPOCH).ok()?;
        i64::try_from(since_epoch.as_nanos()).ok()
    }

    /// The modification time as RFC 3339 UTC, when it is known.
    pub fn modified_rfc3339(&self) -> Option<String> {
        self.modified.map(|modified| {
            chrono::DateTime::<chrono::Utc>::from(modified)
                .to_rfc3339_opts(chrono::SecondsFormat::Nanos, true)
        })
    }
}

#[derive(Debug)]
pub enum ContentError {
    /// The source is larger than the store's limit ([`MAX_SOURCE_BYTES`]).
    TooLarge,
    /// The source is a directory, FIFO, socket, or device.
    NotAFile,
    /// The source could not be opened or read.
    Unreadable(io::ErrorKind),
    /// The source's size or modification time changed while it was copied.
    ChangedDuringRead,
    Cancelled,
    /// A stored blob's bytes no longer match its hash, or an existing blob's
    /// size disagrees with bytes that hash to the same value. `CORRUPT_DATA`.
    HashMismatch,
    /// farm3d's own staging or blob files could not be written, or a caller
    /// passed a staging key or name that isn't a single safe path segment.
    Io,
    /// The store's directories, or the database outside the commit.
    Storage(StorageError),
    /// The commit transaction, including the caller's closure (P7).
    Repository(RepositoryError),
}

impl fmt::Display for ContentError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TooLarge => formatter.write_str("the file is larger than the import limit"),
            Self::NotAFile => formatter.write_str("the path is not a regular file"),
            Self::Unreadable(kind) => write!(formatter, "the file could not be read ({kind})"),
            Self::ChangedDuringRead => formatter.write_str("the file changed while it was read"),
            Self::Cancelled => formatter.write_str("the operation was cancelled"),
            Self::HashMismatch => formatter.write_str("a stored copy is damaged"),
            Self::Io => formatter.write_str("the content store could not be written"),
            Self::Storage(error) => fmt::Display::fmt(error, formatter),
            Self::Repository(_) => formatter.write_str("the library write failed"),
        }
    }
}

impl std::error::Error for ContentError {}

/// Recovers [`ContentError::HashMismatch`] from a [`VerifiedReader`]'s
/// `io::Error`. Any other I/O failure reading a blob is `Unreadable`.
impl From<io::Error> for ContentError {
    fn from(error: io::Error) -> Self {
        if error
            .get_ref()
            .is_some_and(|inner| inner.is::<BlobHashMismatch>())
        {
            Self::HashMismatch
        } else {
            Self::Unreadable(error.kind())
        }
    }
}

/// The command-level mapping, for failures that end a whole command (a
/// verified read or a commit). Per-file staging failures (`TooLarge`,
/// `NotAFile`, `Unreadable`, `ChangedDuringRead`, `Cancelled`) are import
/// item outcomes that the import layer maps to its own codes, so reaching a
/// command error with one is a bug and maps to `INTERNAL`.
impl From<ContentError> for CommandError {
    fn from(error: ContentError) -> Self {
        match error {
            ContentError::HashMismatch => CommandError::content_corrupt(),
            ContentError::Repository(error) => CommandError::from_repository(error),
            ContentError::Storage(error) => {
                CommandError::from_repository(RepositoryError::Storage(error))
            }
            ContentError::Io => CommandError::persistence_unavailable(),
            ContentError::TooLarge
            | ContentError::NotAFile
            | ContentError::Unreadable(_)
            | ContentError::ChangedDuringRead
            | ContentError::Cancelled => CommandError::internal(),
        }
    }
}

/// What [`ContentStore::startup_sweep`] removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Top-level entries (one per staging key) deleted from `staging/`.
    pub staging_removed: usize,
    /// Top-level `staging/` entries that could not be deleted (left for
    /// the next sweep).
    pub staging_failed: usize,
    /// `pending_blob_cleanup` blobs unlinked.
    pub pending_released: usize,
    /// Blob files deleted because no `content_blobs` row names them.
    pub orphans_removed: usize,
    /// Orphan blob files that could not be deleted (left for the next
    /// sweep).
    pub orphans_failed: usize,
}

/// The store's totals, for the Library status line (`library_content_info`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentInfo {
    pub blob_count: i64,
    pub total_bytes: i64,
    pub pending_cleanup_count: i64,
}

/// Test-only failure injection (S3). Each point fires once, the next time
/// the store reaches it.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum ContentFailurePoint {
    /// `place_and_commit` fails after renaming blobs into place and before
    /// its transaction starts, as a crash there would.
    AfterPlacementBeforeCommit = 1,
    /// `release_unreferenced` treats its next unlink as failed.
    BeforeUnlink = 2,
    /// `stage_from_path` truncates its next staged copy to half its length
    /// after hashing it, so inspection sees damaged bytes.
    TruncateStagedOnce = 4,
}

type SourceHook = Box<dyn FnOnce(&Path) + Send>;

/// Test-only barrier (S3, S5, S6): holds one `stage_from_path` call after
/// its first chunk is written, or one `place_and_commit_unless_cancelled`
/// call before its cancel check, until the test releases it. A source with
/// no bytes has no first chunk and never pauses.
#[doc(hidden)]
pub struct StagePause {
    reached: Barrier,
    released: Barrier,
}

impl StagePause {
    /// Blocks until the paused copy has written its first chunk.
    pub fn wait_until_reached(&self) {
        self.reached.wait();
    }

    /// Lets the paused copy continue.
    pub fn release(&self) {
        self.released.wait();
    }
}

pub struct ContentStore {
    blobs: PathBuf,
    staging: PathBuf,
    placement: Mutex<()>,
    max_bytes: u64,
    failures: AtomicU8,
    before_open: Mutex<Option<SourceHook>>,
    before_restat: Mutex<Option<SourceHook>>,
    pause: Mutex<Option<Arc<StagePause>>>,
    placement_pause: Mutex<Option<Arc<StagePause>>>,
}

impl ContentStore {
    /// Opens the store under `content_root` (which must already be
    /// canonical, as `StoragePaths::content_root` is), creating
    /// `blobs/sha256` and `staging` with the contained-directory checks. A
    /// symlinked `blobs` or `staging` is `PathCollision`.
    pub fn open(content_root: &Path) -> Result<Self, StorageError> {
        let blobs = create_contained_directory(content_root, Path::new(BLOB_DIRECTORY))?;
        let staging = create_contained_directory(content_root, Path::new(STAGING_DIRECTORY))?;
        Ok(Self {
            blobs,
            staging,
            placement: Mutex::new(()),
            max_bytes: MAX_SOURCE_BYTES,
            failures: AtomicU8::new(0),
            before_open: Mutex::new(None),
            before_restat: Mutex::new(None),
            pause: Mutex::new(None),
            placement_pause: Mutex::new(None),
        })
    }

    /// Test hook (S3): lowers the source size limit.
    #[doc(hidden)]
    pub fn with_max_bytes(mut self, max_bytes: u64) -> Self {
        self.max_bytes = max_bytes;
        self
    }

    /// Test hook (S3): arms `point` to fire once.
    #[doc(hidden)]
    pub fn inject_failure_once(&self, point: ContentFailurePoint) {
        self.failures.fetch_or(point as u8, Ordering::SeqCst);
    }

    /// Test hook (S3): runs `hook` with the source path once, in the next
    /// `stage_from_path`, between the copy and the re-stat.
    #[doc(hidden)]
    pub fn before_restat_once(&self, hook: SourceHook) {
        *lock_ignoring_poison(&self.before_restat) = Some(hook);
    }

    /// Test hook (S3): runs `hook` with the source path once, in the next
    /// `stage_from_path`, after its first stat and before it opens the
    /// source, where a swap can land.
    #[doc(hidden)]
    pub fn before_open_once(&self, hook: SourceHook) {
        *lock_ignoring_poison(&self.before_open) = Some(hook);
    }

    /// Test hook (S3, S5, S6): pauses the next `stage_from_path` after its
    /// first chunk. See [`StagePause`].
    #[doc(hidden)]
    pub fn pause_after_first_chunk_once(&self) -> Arc<StagePause> {
        arm_pause(&self.pause)
    }

    /// Test hook (S6): pauses the next `place_and_commit_unless_cancelled`
    /// before its cancel check and the placement lock, so a test can land a
    /// cancel or a second import while one import is between staging and
    /// placement. See [`StagePause`].
    #[doc(hidden)]
    pub fn pause_before_placement_once(&self) -> Arc<StagePause> {
        arm_pause(&self.placement_pause)
    }

    fn take_failure(&self, point: ContentFailurePoint) -> bool {
        let bit = point as u8;
        self.failures.fetch_and(!bit, Ordering::SeqCst) & bit != 0
    }

    /// D4 steps 1–2: copies `source` (following symlinks) into
    /// `staging/<staging_key>/<file_index>.part`, hashing as it goes.
    /// `progress(copied, total)` runs after every chunk, and `cancel` is
    /// checked before each one. Any failure removes the partial copy.
    pub fn stage_from_path(
        &self,
        source: &Path,
        staging_key: &str,
        file_index: usize,
        cancel: &CancelFlag,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<StagedFile, ContentError> {
        // The path is checked first so a device or FIFO is never opened in
        // the usual case, then the handle is checked, since the path can be
        // swapped between the two. `before` is the opened file's own stat.
        let unreadable = |error: io::Error| ContentError::Unreadable(error.kind());
        let first = fs::metadata(source).map_err(unreadable)?;
        check_source(&first, self.max_bytes)?;
        if let Some(hook) = lock_ignoring_poison(&self.before_open).take() {
            hook(source);
        }
        let mut reader = open_source(source).map_err(unreadable)?;
        let before = reader.metadata().map_err(unreadable)?;
        check_source(&before, self.max_bytes)?;
        let path = self
            .staging_directory(staging_key)?
            .join(format!("{file_index}{PART_SUFFIX}"));
        let result = self.copy_into_staging(&mut reader, source, &path, &before, cancel, progress);
        if result.is_err() {
            let _ = fs::remove_file(&path);
        }
        result
    }

    fn copy_into_staging(
        &self,
        reader: &mut File,
        source: &Path,
        path: &Path,
        before: &fs::Metadata,
        cancel: &CancelFlag,
        progress: &mut dyn FnMut(u64, u64),
    ) -> Result<StagedFile, ContentError> {
        let expected = before.len();
        let mut staged = create_staged_file(path)?;
        let mut hasher = Sha256::new();
        let mut buffer = vec![0_u8; CHUNK_BYTES];
        // One byte past the expected length is enough to notice growth.
        let mut limited = reader.take(expected + 1);
        let mut copied = 0_u64;
        loop {
            if cancel.is_cancelled() {
                return Err(ContentError::Cancelled);
            }
            let read = fill(&mut limited, &mut buffer)
                .map_err(|error| ContentError::Unreadable(error.kind()))?;
            if read == 0 {
                break;
            }
            let chunk = &buffer[..read];
            hasher.update(chunk);
            staged.write_all(chunk).map_err(|_| ContentError::Io)?;
            let first_chunk = copied == 0;
            copied += read as u64;
            if copied > expected {
                return Err(ContentError::ChangedDuringRead);
            }
            progress(copied, expected);
            if first_chunk {
                self.pause_if_armed();
            }
        }
        if copied != expected {
            return Err(ContentError::ChangedDuringRead);
        }
        staged.sync_all().map_err(|_| ContentError::Io)?;
        drop(staged);

        if let Some(hook) = lock_ignoring_poison(&self.before_restat).take() {
            hook(source);
        }
        let after = fs::metadata(source).map_err(|_| ContentError::ChangedDuringRead)?;
        if after.len() != before.len() || modified(&after) != modified(before) {
            return Err(ContentError::ChangedDuringRead);
        }

        if self.take_failure(ContentFailurePoint::TruncateStagedOnce) {
            OpenOptions::new()
                .write(true)
                .open(path)
                .and_then(|file| file.set_len(expected / 2))
                .map_err(|_| ContentError::Io)?;
        }
        Ok(StagedFile {
            path: path.to_path_buf(),
            sha256: format!("{:x}", hasher.finalize()),
            size: copied,
            source: Some(SourceStat::of(before)),
        })
    }

    fn pause_if_armed(&self) {
        wait_if_armed(&self.pause);
    }

    /// Stages in-memory bytes (a thumbnail) as `staging/<staging_key>/<name>`.
    /// `name` must not end in `.part`, which is reserved for source copies
    /// (P9), so a count of `.part` files counts sources only.
    pub fn stage_bytes(
        &self,
        bytes: &[u8],
        staging_key: &str,
        name: &str,
    ) -> Result<StagedFile, ContentError> {
        if !is_safe_component(name) || name.ends_with(PART_SUFFIX) {
            return Err(ContentError::Io);
        }
        if bytes.len() as u64 > self.max_bytes {
            return Err(ContentError::TooLarge);
        }
        let path = self.staging_directory(staging_key)?.join(name);
        let result = create_staged_file(&path).and_then(|mut file| {
            file.write_all(bytes)
                .and_then(|()| file.sync_all())
                .map_err(|_| ContentError::Io)
        });
        if result.is_err() {
            let _ = fs::remove_file(&path);
        }
        result?;
        Ok(StagedFile {
            path,
            sha256: format!("{:x}", Sha256::digest(bytes)),
            size: bytes.len() as u64,
            source: None,
        })
    }

    fn staging_directory(&self, staging_key: &str) -> Result<PathBuf, ContentError> {
        if !is_safe_component(staging_key) {
            return Err(ContentError::Io);
        }
        create_contained_directory(&self.staging, Path::new(staging_key))
            .map_err(|_| ContentError::Io)
    }

    /// D4 steps 4–5. Under the placement lock, moves every staged file to
    /// its blob path (or drops it if an equal-size blob already exists),
    /// then commits one transaction that inserts any missing
    /// `content_blobs` rows for `staged` and runs `commit`. The closure
    /// need not insert blob rows itself. If `commit` fails, nothing is
    /// written; any newly placed blob stays as an orphan for the startup
    /// sweep.
    pub fn place_and_commit<T>(
        &self,
        storage: &Storage,
        staged: &[&StagedFile],
        commit: impl FnOnce(&Transaction<'_>) -> Result<T, RepositoryError>,
    ) -> Result<T, ContentError> {
        self.place_and_commit_unless_cancelled(storage, staged, &CancelFlag::never(), commit)
    }

    /// [`Self::place_and_commit`] with D13's last cancel check: once the
    /// placement lock is held, a raised `cancel` stops the item with
    /// [`ContentError::Cancelled`] before anything is placed or written.
    pub fn place_and_commit_unless_cancelled<T>(
        &self,
        storage: &Storage,
        staged: &[&StagedFile],
        cancel: &CancelFlag,
        commit: impl FnOnce(&Transaction<'_>) -> Result<T, RepositoryError>,
    ) -> Result<T, ContentError> {
        wait_if_armed(&self.placement_pause);
        let _placement = self
            .placement
            .lock()
            .map_err(|_| ContentError::Storage(StorageError::PersistenceUnavailable))?;
        if cancel.is_cancelled() {
            return Err(ContentError::Cancelled);
        }
        for file in staged {
            self.place(file)?;
        }
        if self.take_failure(ContentFailurePoint::AfterPlacementBeforeCommit) {
            return Err(ContentError::Io);
        }
        let created_at = now_rfc3339();
        storage
            .write_repo(|transaction| {
                for file in staged {
                    let size =
                        i64::try_from(file.size).map_err(|_| RepositoryError::Validation {
                            field_path: "sizeBytes",
                        })?;
                    transaction.execute(
                        "INSERT INTO content_blobs (sha256, size_bytes, created_at)
                         VALUES (?1, ?2, ?3)
                         ON CONFLICT(sha256) DO NOTHING",
                        params![file.sha256, size, created_at],
                    )?;
                }
                commit(transaction)
            })
            .map_err(ContentError::Repository)
    }

    fn place(&self, file: &StagedFile) -> Result<(), ContentError> {
        if !is_sha256_hex(&file.sha256) || !file.path.starts_with(&self.staging) {
            return Err(ContentError::Io);
        }
        let directory = create_contained_directory(&self.blobs, Path::new(&file.sha256[..2]))
            .map_err(ContentError::Storage)?;
        let blob = directory.join(&file.sha256);
        match fs::symlink_metadata(&blob) {
            Ok(existing) if existing.is_file() => {
                if existing.len() != file.size {
                    return Err(ContentError::HashMismatch);
                }
                match fs::remove_file(&file.path) {
                    Ok(()) => Ok(()),
                    Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
                    Err(_) => Err(ContentError::Io),
                }
            }
            Ok(_) => Err(ContentError::Storage(StorageError::PathCollision)),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                set_blob_permissions(&file.path).map_err(|_| ContentError::Io)?;
                fs::rename(&file.path, &blob).map_err(|_| ContentError::Io)?;
                sync_directory(&directory).map_err(|_| ContentError::Io)?;
                sync_directory(&self.blobs).map_err(|_| ContentError::Io)
            }
            Err(_) => Err(ContentError::Io),
        }
    }

    /// D4 cleanup, after a commit that called [`mark_unreferenced_blobs`]:
    /// under the placement lock, unlinks each `pending_blob_cleanup` blob
    /// and deletes its row. A blob that has regained a `content_blobs` row
    /// (re-imported since it was marked) is kept and only its pending row
    /// is dropped. A failed unlink leaves the row with `attempt_count`
    /// bumped for the startup sweep to retry. Returns the number of blobs
    /// unlinked.
    pub fn release_unreferenced(&self, storage: &Storage) -> Result<usize, StorageError> {
        let _placement = self
            .placement
            .lock()
            .map_err(|_| StorageError::PersistenceUnavailable)?;
        let pending = storage.read(|connection| {
            let mut statement =
                connection.prepare("SELECT sha256 FROM pending_blob_cleanup ORDER BY sha256")?;
            let hashes = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok(hashes)
        })?;
        let mut released = 0;
        for sha256 in pending {
            let referenced = storage.read(|connection| {
                connection.query_row(
                    "SELECT EXISTS(SELECT 1 FROM content_blobs WHERE sha256 = ?1)",
                    [&sha256],
                    |row| row.get::<_, bool>(0),
                )
            })?;
            if !referenced {
                if self.unlink_blob(&sha256).is_err() {
                    storage.write(|transaction| {
                        transaction.execute(
                            "UPDATE pending_blob_cleanup
                             SET attempt_count = attempt_count + 1,
                                 last_error_code = 'PERSISTENCE_UNAVAILABLE',
                                 last_attempt_at = ?2
                             WHERE sha256 = ?1",
                            params![sha256, now_rfc3339()],
                        )?;
                        Ok(())
                    })?;
                    continue;
                }
                released += 1;
            }
            storage.write(|transaction| {
                transaction.execute(
                    "DELETE FROM pending_blob_cleanup WHERE sha256 = ?1",
                    [&sha256],
                )?;
                Ok(())
            })?;
        }
        Ok(released)
    }

    fn unlink_blob(&self, sha256: &str) -> io::Result<()> {
        if self.take_failure(ContentFailurePoint::BeforeUnlink) {
            return Err(io::Error::other("injected unlink failure"));
        }
        let blob = self.blobs.join(&sha256[..2]).join(sha256);
        match remove_blob_file(&blob) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            result => result,
        }
    }

    /// D4 startup reconciliation, run before commands are served (the
    /// metadata-root lease guarantees no import is in flight): empties
    /// `staging/`, retries `pending_blob_cleanup`, and deletes blob files
    /// that no `content_blobs` row names.
    ///
    /// Deleting is best effort: a file that can't be removed is counted in
    /// the report, logged by basename or hash, and left for the next sweep,
    /// so one stuck file never stops the app from serving. Only a
    /// containment violation (a symlinked blob prefix directory) or a
    /// database failure is an error.
    pub fn startup_sweep(&self, storage: &Storage) -> Result<SweepReport, StorageError> {
        let (staging_removed, staging_failed) = self.clear_staging();
        let pending_released = self.release_unreferenced(storage)?;
        let (orphans_removed, orphans_failed) = self.remove_orphans(storage)?;
        Ok(SweepReport {
            staging_removed,
            staging_failed,
            pending_released,
            orphans_removed,
            orphans_failed,
        })
    }

    /// Returns `(removed, failed)` top-level `staging/` entries.
    fn clear_staging(&self) -> (usize, usize) {
        let entries = match fs::read_dir(&self.staging) {
            Ok(entries) => entries,
            Err(error) => {
                eprintln!("farm3d: content sweep could not list staging: {error}");
                return (0, 1);
            }
        };
        let (mut removed, mut failed) = (0, 0);
        for entry in entries {
            let outcome = entry.and_then(|entry| {
                let path = entry.path();
                let result = if entry.file_type()?.is_dir() {
                    fs::remove_dir_all(&path)
                } else {
                    fs::remove_file(&path)
                };
                result.map_err(|error| {
                    eprintln!(
                        "farm3d: content sweep could not remove staging entry {:?}: {error}",
                        entry.file_name()
                    );
                    error
                })
            });
            match outcome {
                Ok(()) => removed += 1,
                Err(_) => failed += 1,
            }
        }
        (removed, failed)
    }

    /// Returns `(removed, failed)` orphan blob files. A hash with a
    /// `pending_blob_cleanup` row is left to `release_unreferenced`, which
    /// already tried it and recorded the attempt.
    fn remove_orphans(&self, storage: &Storage) -> Result<(usize, usize), StorageError> {
        let (known, pending) = storage.read(|connection| {
            let hashes = |sql: &str| -> rusqlite::Result<HashSet<String>> {
                let mut statement = connection.prepare(sql)?;
                let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
                rows.collect()
            };
            Ok((
                hashes("SELECT sha256 FROM content_blobs")?,
                hashes("SELECT sha256 FROM pending_blob_cleanup")?,
            ))
        })?;
        let (mut removed, mut failed) = (0, 0);
        let prefixes = match fs::read_dir(&self.blobs) {
            Ok(prefixes) => prefixes,
            Err(error) => {
                eprintln!("farm3d: content sweep could not list blobs: {error}");
                return Ok((0, 1));
            }
        };
        for prefix in prefixes.flatten() {
            let Ok(file_type) = prefix.file_type() else {
                failed += 1;
                continue;
            };
            if file_type.is_symlink() {
                return Err(StorageError::PathCollision);
            }
            if !file_type.is_dir() {
                continue;
            }
            let prefix_name = prefix.file_name();
            let blobs = match fs::read_dir(prefix.path()) {
                Ok(blobs) => blobs,
                Err(error) => {
                    eprintln!(
                        "farm3d: content sweep could not list blob prefix {prefix_name:?}: {error}"
                    );
                    failed += 1;
                    continue;
                }
            };
            for blob in blobs.flatten() {
                if blob.file_type().is_ok_and(|file_type| file_type.is_dir()) {
                    continue;
                }
                let name = blob.file_name();
                let keep = name
                    .to_str()
                    .zip(prefix_name.to_str())
                    .is_some_and(|(name, prefix)| {
                        name.starts_with(prefix) && (known.contains(name) || pending.contains(name))
                    });
                if keep {
                    continue;
                }
                match remove_blob_file(&blob.path()) {
                    Ok(()) => removed += 1,
                    Err(error) => {
                        eprintln!("farm3d: content sweep could not remove blob {name:?}: {error}");
                        failed += 1;
                    }
                }
            }
        }
        Ok((removed, failed))
    }

    /// D2: opens the blob for `sha256` for reading. The returned reader
    /// hashes as it streams and fails with [`ContentError::HashMismatch`]
    /// (as an `io::Error`; convert with `ContentError::from`) at the end if
    /// the bytes no longer match. Bytes returned before the end are not yet
    /// verified.
    pub fn open_verified(&self, sha256: &str) -> Result<VerifiedReader, ContentError> {
        if !is_sha256_hex(sha256) {
            return Err(ContentError::Unreadable(io::ErrorKind::InvalidInput));
        }
        let path = self.blobs.join(&sha256[..2]).join(sha256);
        let file = open_no_follow(&path).map_err(|error| ContentError::Unreadable(error.kind()))?;
        let metadata = file
            .metadata()
            .map_err(|error| ContentError::Unreadable(error.kind()))?;
        if !metadata.is_file() {
            return Err(ContentError::NotAFile);
        }
        Ok(VerifiedReader {
            file,
            hasher: Sha256::new(),
            expected: sha256.to_string(),
            state: ReadState::Reading,
        })
    }

    /// Opens the blob for `sha256` as a plain, seekable file, without
    /// verifying it. For readers that need to seek (a 3MF's ZIP directory,
    /// P5 D6); pair it with [`ContentStore::verify`] before trusting the
    /// bytes. A blob is never buffered whole.
    pub fn open_unverified(&self, sha256: &str) -> Result<File, ContentError> {
        if !is_sha256_hex(sha256) {
            return Err(ContentError::Unreadable(io::ErrorKind::InvalidInput));
        }
        let path = self.blobs.join(&sha256[..2]).join(sha256);
        let file = open_no_follow(&path).map_err(|error| ContentError::Unreadable(error.kind()))?;
        let metadata = file
            .metadata()
            .map_err(|error| ContentError::Unreadable(error.kind()))?;
        if !metadata.is_file() {
            return Err(ContentError::NotAFile);
        }
        Ok(file)
    }

    /// Streams the blob for `sha256` through its hash, in one pass with a
    /// fixed buffer. [`ContentError::HashMismatch`] when the bytes no
    /// longer match.
    pub fn verify(&self, sha256: &str) -> Result<(), ContentError> {
        let mut reader = self.open_verified(sha256)?;
        io::copy(&mut reader, &mut io::sink())?;
        Ok(())
    }

    /// Deletes `staging/<staging_key>` and everything in it. Best effort:
    /// anything left behind goes at the next startup sweep.
    pub fn discard_staging(&self, staging_key: &str) {
        if !is_safe_component(staging_key) {
            return;
        }
        let path = self.staging.join(staging_key);
        match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => {
                let _ = fs::remove_dir_all(&path);
            }
            Ok(_) => {
                let _ = fs::remove_file(&path);
            }
            Err(_) => {}
        }
    }

    pub fn info(&self, storage: &Storage) -> Result<ContentInfo, StorageError> {
        storage.read(|connection| {
            let (blob_count, total_bytes) = connection.query_row(
                "SELECT COUNT(*), COALESCE(SUM(size_bytes), 0) FROM content_blobs",
                [],
                |row| Ok((row.get::<_, i64>(0)?, row.get::<_, i64>(1)?)),
            )?;
            let pending_cleanup_count =
                connection.query_row("SELECT COUNT(*) FROM pending_blob_cleanup", [], |row| {
                    row.get::<_, i64>(0)
                })?;
            Ok(ContentInfo {
                blob_count,
                total_bytes,
                pending_cleanup_count,
            })
        })
    }
}

/// The in-transaction half of D4 cleanup, called by whatever deletes the
/// rows that referenced `candidates`. Each candidate no longer referenced
/// by any Model Source Revision, thumbnail, Slice Revision (its G-code or
/// an input blob), or slice operation log (P5 D13) loses its
/// `content_blobs` row and gains a
/// `pending_blob_cleanup` row. After commit, call
/// [`ContentStore::release_unreferenced`] to unlink the files.
pub fn mark_unreferenced_blobs(
    transaction: &Transaction<'_>,
    candidates: &[String],
) -> Result<(), StorageError> {
    let now = now_rfc3339();
    for sha256 in candidates {
        let referenced: bool = transaction.query_row(
            "SELECT EXISTS(SELECT 1 FROM model_source_revisions WHERE content_sha256 = ?1)
                 OR EXISTS(SELECT 1 FROM model_revision_thumbnails WHERE content_sha256 = ?1)
                 OR EXISTS(SELECT 1 FROM slice_revisions WHERE gcode_sha256 = ?1)
                 OR EXISTS(SELECT 1 FROM slice_revision_blobs WHERE sha256 = ?1)
                 OR EXISTS(SELECT 1 FROM slice_operations WHERE log_sha256 = ?1)",
            [sha256],
            |row| row.get(0),
        )?;
        if referenced {
            continue;
        }
        let deleted =
            transaction.execute("DELETE FROM content_blobs WHERE sha256 = ?1", [sha256])?;
        if deleted > 0 {
            transaction.execute(
                "INSERT INTO pending_blob_cleanup (sha256, created_at) VALUES (?1, ?2)
                 ON CONFLICT(sha256) DO NOTHING",
                params![sha256, now],
            )?;
        }
    }
    Ok(())
}

/// A blob reader that verifies the SHA-256 when it reaches the end (D2).
pub struct VerifiedReader {
    file: File,
    hasher: Sha256,
    expected: String,
    state: ReadState,
}

enum ReadState {
    Reading,
    Verified,
    Mismatch,
}

/// The `io::Error` payload a [`VerifiedReader`] fails with;
/// `ContentError::from` turns it back into `HashMismatch`.
#[derive(Debug)]
struct BlobHashMismatch;

impl fmt::Display for BlobHashMismatch {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("stored content does not match its hash")
    }
}

impl std::error::Error for BlobHashMismatch {}

impl Read for VerifiedReader {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        match self.state {
            ReadState::Mismatch => {
                return Err(io::Error::new(io::ErrorKind::InvalidData, BlobHashMismatch))
            }
            ReadState::Verified => return Ok(0),
            ReadState::Reading => {}
        }
        if buffer.is_empty() {
            return Ok(0);
        }
        let read = self.file.read(buffer)?;
        if read > 0 {
            self.hasher.update(&buffer[..read]);
            return Ok(read);
        }
        let actual = format!("{:x}", self.hasher.finalize_reset());
        if actual == self.expected {
            self.state = ReadState::Verified;
            Ok(0)
        } else {
            self.state = ReadState::Mismatch;
            Err(io::Error::new(io::ErrorKind::InvalidData, BlobHashMismatch))
        }
    }
}

fn arm_pause(slot: &Mutex<Option<Arc<StagePause>>>) -> Arc<StagePause> {
    let pause = Arc::new(StagePause {
        reached: Barrier::new(2),
        released: Barrier::new(2),
    });
    *lock_ignoring_poison(slot) = Some(Arc::clone(&pause));
    pause
}

fn wait_if_armed(slot: &Mutex<Option<Arc<StagePause>>>) {
    let pause = lock_ignoring_poison(slot).take();
    if let Some(pause) = pause {
        pause.reached.wait();
        pause.released.wait();
    }
}

fn lock_ignoring_poison<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Reads until `buffer` is full or the reader is exhausted, so every chunk
/// but the last is exactly [`CHUNK_BYTES`].
fn fill(reader: &mut impl Read, buffer: &mut [u8]) -> io::Result<usize> {
    let mut filled = 0;
    while filled < buffer.len() {
        match reader.read(&mut buffer[filled..]) {
            Ok(0) => break,
            Ok(read) => filled += read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
    Ok(filled)
}

fn modified(metadata: &fs::Metadata) -> Option<SystemTime> {
    metadata.modified().ok()
}

/// A staging key or staged file name: one path segment of ASCII letters,
/// digits, `-`, `_`, and `.`, not starting with `.`.
fn is_safe_component(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= MAX_STAGING_COMPONENT
        && !value.starts_with('.')
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.'))
}

fn is_sha256_hex(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| matches!(byte, b'0'..=b'9' | b'a'..=b'f'))
}

fn create_staged_file(path: &Path) -> Result<File, ContentError> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options.open(path).map_err(|_| ContentError::Io)
}

/// `NotAFile` unless `metadata` is a regular file; `TooLarge` past
/// `max_bytes`.
fn check_source(metadata: &fs::Metadata, max_bytes: u64) -> Result<(), ContentError> {
    if !metadata.is_file() {
        return Err(ContentError::NotAFile);
    }
    if metadata.len() > max_bytes {
        return Err(ContentError::TooLarge);
    }
    Ok(())
}

/// Opens a source (following symlinks) without waiting: on Unix with
/// `O_NONBLOCK`, so a path swapped for a FIFO opens at once, to be refused
/// as not a regular file, instead of blocking until a writer appears.
/// `O_NONBLOCK` does not change reads of a regular file.
fn open_source(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NONBLOCK.bits() as i32);
    }
    options.open(path)
}

fn open_no_follow(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.read(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options.open(path)
}

/// D4: blob files are `0400` on Unix and read-only elsewhere.
fn set_blob_permissions(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o400))
    }
    #[cfg(not(unix))]
    {
        let mut permissions = fs::metadata(path)?.permissions();
        permissions.set_readonly(true);
        fs::set_permissions(path, permissions)
    }
}

/// Removes a blob file. Unix only needs write access to the directory; on
/// other platforms the read-only attribute must be cleared first.
fn remove_blob_file(path: &Path) -> io::Result<()> {
    #[cfg(not(unix))]
    {
        if let Ok(metadata) = fs::symlink_metadata(path) {
            let mut permissions = metadata.permissions();
            #[allow(clippy::permissions_set_readonly_false)]
            permissions.set_readonly(false);
            let _ = fs::set_permissions(path, permissions);
        }
    }
    fs::remove_file(path)
}

/// Makes a rename or new entry in `directory` durable (Unix). Other
/// platforms have no directory handle to flush.
fn sync_directory(directory: &Path) -> io::Result<()> {
    #[cfg(unix)]
    {
        File::open(directory)?.sync_all()
    }
    #[cfg(not(unix))]
    {
        let _ = directory;
        Ok(())
    }
}
