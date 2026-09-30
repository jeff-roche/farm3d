//! P9 D14: storage usage by class and cleanup of reclaimable data, plus
//! the free-space helper the backup writer and restore staging use.
//!
//! [`usage_in`] reads the same bases P4 and P8 already use: the recorded
//! `content_blobs.size_bytes` (P4's `library_content_info`), the stored
//! `camera_snapshots.byte_len` (P8's `media_usage`), and apparent file
//! lengths (never allocated blocks) for the files themselves, so a
//! quiescent Farm's usage equals the bytes on disk exactly.
//!
//! [`clear`] runs one [`StorageCleanupTarget`] while the caller holds the
//! backup lease (`clear_storage`, activity `storageCleanup`). No target
//! touches referenced data, and none touches a database row a reference
//! check reads.

use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::log::MAX_FILES;
use super::reset::tree_usage;
use crate::backup::lease::LeaseGuard;
use crate::contracts::command::CommandError;
use crate::library::content::ContentStore;
use crate::persistence::snapshot::accepted_snapshots;
use crate::persistence::{RepositoryError, Storage, StorageError, StoragePaths};
use crate::slicing::runtime::PROFILE_CACHE_DIR;

/// A class of stored data, in the order `storage_usage` lists them.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StorageClass.ts")]
pub enum StorageClass {
    Database,
    ContentModelSources,
    ContentThumbnails,
    ContentGcode,
    ContentSliceArtifacts,
    ContentSlicerLogs,
    ContentUnreferenced,
    CameraMediaPinned,
    CameraMediaUnpinned,
    SafetyBackups,
    PreImportSnapshots,
    Logs,
    SlicerProfileCache,
}

impl StorageClass {
    /// Every class, in the order `storage_usage` lists them.
    pub const ALL: [StorageClass; 13] = [
        StorageClass::Database,
        StorageClass::ContentModelSources,
        StorageClass::ContentThumbnails,
        StorageClass::ContentGcode,
        StorageClass::ContentSliceArtifacts,
        StorageClass::ContentSlicerLogs,
        StorageClass::ContentUnreferenced,
        StorageClass::CameraMediaPinned,
        StorageClass::CameraMediaUnpinned,
        StorageClass::SafetyBackups,
        StorageClass::PreImportSnapshots,
        StorageClass::Logs,
        StorageClass::SlicerProfileCache,
    ];
}

/// One class's bytes and the number of records or files they come from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StorageClassUsage.ts")]
pub struct StorageClassUsage {
    pub class: StorageClass,
    #[ts(type = "number")]
    pub bytes: u64,
    #[ts(type = "number")]
    pub count: u64,
}

/// `storage_usage`'s result.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StorageUsage.ts")]
pub struct StorageUsage {
    /// Every class, in order.
    pub classes: Vec<StorageClassUsage>,
    #[ts(type = "number")]
    pub total_bytes: u64,
    pub measured_at: String,
}

/// What `clear_storage` can remove.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StorageCleanupTarget.ts")]
pub enum StorageCleanupTarget {
    UnreferencedContent,
    PreImportSnapshots,
    RotatedLogs,
    OrcaCache,
}

impl StorageCleanupTarget {
    pub fn as_str(self) -> &'static str {
        match self {
            StorageCleanupTarget::UnreferencedContent => "unreferencedContent",
            StorageCleanupTarget::PreImportSnapshots => "preImportSnapshots",
            StorageCleanupTarget::RotatedLogs => "rotatedLogs",
            StorageCleanupTarget::OrcaCache => "orcaCache",
        }
    }
}

/// `clear_storage`'s result.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ClearStorageOutcome.ts")]
pub struct ClearStorageOutcome {
    pub target: StorageCleanupTarget,
    #[ts(type = "number")]
    pub removed_count: u64,
    #[ts(type = "number")]
    pub freed_bytes: u64,
    pub usage: StorageUsage,
}

// --- usage -------------------------------------------------------------------------------

const BLOB_DIRECTORY: &str = "blobs/sha256";

fn to_u64(value: i64) -> u64 {
    u64::try_from(value).unwrap_or(0)
}

fn file_len(path: &Path) -> Option<u64> {
    match fs::symlink_metadata(path) {
        Ok(metadata) if metadata.is_file() => Some(metadata.len()),
        _ => None,
    }
}

/// `(count, bytes)` of the files directly in `directory` that `select`
/// accepts by name (subdirectories and links are not counted).
fn files_in(directory: &Path, select: impl Fn(&str) -> bool) -> (u64, u64) {
    let mut total = (0, 0);
    for entry in fs::read_dir(directory).into_iter().flatten().flatten() {
        let accepted = entry.file_name().to_str().is_some_and(&select);
        if let (true, Some(length)) = (accepted, file_len(&entry.path())) {
            total.0 += 1;
            total.1 += length;
        }
    }
    total
}

fn tree(root: &Path) -> (u64, u64) {
    let (count, bytes) = tree_usage(root);
    (to_u64(count), to_u64(bytes))
}

/// The pending blob's file length, when its hash is a well-formed SHA-256
/// (the name a blob file has) and the file exists.
fn blob_len(paths: &StoragePaths, sha256: &str) -> u64 {
    let well_formed = sha256.len() == 64
        && sha256
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte));
    if !well_formed {
        return 0;
    }
    file_len(
        &paths
            .content_root()
            .join(BLOB_DIRECTORY)
            .join(&sha256[..2])
            .join(sha256),
    )
    .unwrap_or(0)
}

/// Every class's usage, read inside `connection`'s current transaction.
/// `slicer_cache` is the app cache directory the Slicer keeps
/// `orca-profiles/` under.
pub fn usage_in(
    connection: &Connection,
    paths: &StoragePaths,
    slicer_cache: &Path,
    now: DateTime<Utc>,
) -> rusqlite::Result<StorageUsage> {
    let mut totals = [(0_u64, 0_u64); 13];
    let mut set = |class: StorageClass, value: (u64, u64)| {
        let slot = StorageClass::ALL
            .iter()
            .position(|candidate| *candidate == class)
            .unwrap_or(0);
        totals[slot].0 += value.1;
        totals[slot].1 += value.0;
    };

    // The database: the main file, the WAL, and the shared-memory file.
    let database = paths.database();
    let mut database_files = (0, 0);
    for suffix in ["", "-wal", "-shm"] {
        let mut name = database.file_name().unwrap_or_default().to_os_string();
        name.push(suffix);
        if let Some(length) = file_len(&database.with_file_name(name)) {
            database_files.0 += 1;
            database_files.1 += length;
        }
    }
    set(StorageClass::Database, database_files);

    // Content: each blob once, in the first class that references it.
    let mut statement = connection.prepare(
        "SELECT CASE
                  WHEN EXISTS(SELECT 1 FROM model_source_revisions WHERE content_sha256 = b.sha256) THEN 0
                  WHEN EXISTS(SELECT 1 FROM model_revision_thumbnails WHERE content_sha256 = b.sha256) THEN 1
                  WHEN EXISTS(SELECT 1 FROM slice_revisions WHERE gcode_sha256 = b.sha256) THEN 2
                  WHEN EXISTS(SELECT 1 FROM slice_revision_blobs WHERE sha256 = b.sha256 AND role <> 'log') THEN 3
                  WHEN EXISTS(SELECT 1 FROM slice_revision_blobs WHERE sha256 = b.sha256)
                    OR EXISTS(SELECT 1 FROM slice_operations WHERE log_sha256 = b.sha256) THEN 4
                  ELSE 5
                END AS class, COUNT(*), COALESCE(SUM(b.size_bytes), 0)
         FROM content_blobs b GROUP BY class",
    )?;
    let content_classes = [
        StorageClass::ContentModelSources,
        StorageClass::ContentThumbnails,
        StorageClass::ContentGcode,
        StorageClass::ContentSliceArtifacts,
        StorageClass::ContentSlicerLogs,
        StorageClass::ContentUnreferenced,
    ];
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for (class, count, bytes) in rows {
        if let Some(class) = usize::try_from(class)
            .ok()
            .and_then(|index| content_classes.get(index))
        {
            set(*class, (to_u64(count), to_u64(bytes)));
        }
    }
    // Pending cleanup blobs no row names any longer: the file's length.
    let mut statement = connection.prepare(
        "SELECT sha256 FROM pending_blob_cleanup
         WHERE sha256 NOT IN (SELECT sha256 FROM content_blobs)",
    )?;
    let pending = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    for sha256 in pending {
        set(
            StorageClass::ContentUnreferenced,
            (1, blob_len(paths, &sha256)),
        );
    }

    // Camera media: unpruned rows, pinned or not.
    let (pinned_count, pinned_bytes, unpinned_count, unpinned_bytes) = connection.query_row(
        "SELECT COUNT(CASE WHEN pinned_at IS NOT NULL THEN 1 END),
                COALESCE(SUM(CASE WHEN pinned_at IS NOT NULL THEN byte_len END), 0),
                COUNT(CASE WHEN pinned_at IS NULL THEN 1 END),
                COALESCE(SUM(CASE WHEN pinned_at IS NULL THEN byte_len END), 0)
         FROM camera_snapshots WHERE pruned_at IS NULL",
        [],
        |row| {
            Ok((
                row.get::<_, i64>(0)?,
                row.get::<_, i64>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, i64>(3)?,
            ))
        },
    )?;
    set(
        StorageClass::CameraMediaPinned,
        (to_u64(pinned_count), to_u64(pinned_bytes)),
    );
    set(
        StorageClass::CameraMediaUnpinned,
        (to_u64(unpinned_count), to_u64(unpinned_bytes)),
    );

    // Files.
    set(
        StorageClass::SafetyBackups,
        tree(&paths.backup_root().join("safety")),
    );
    set(
        StorageClass::PreImportSnapshots,
        files_in(paths.snapshot_root(), |name| {
            name.starts_with(".farm3d-pre-import-") && name.ends_with(".sqlite3")
        }),
    );
    set(
        StorageClass::Logs,
        files_in(paths.log_root(), |name| {
            name.starts_with("farm3d") && name.ends_with(".log")
        }),
    );
    set(
        StorageClass::SlicerProfileCache,
        tree(&slicer_cache.join(PROFILE_CACHE_DIR)),
    );

    let classes: Vec<StorageClassUsage> = StorageClass::ALL
        .iter()
        .zip(totals)
        .map(|(class, (bytes, count))| StorageClassUsage {
            class: *class,
            bytes,
            count,
        })
        .collect();
    Ok(StorageUsage {
        total_bytes: classes.iter().map(|class| class.bytes).sum(),
        classes,
        measured_at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
    })
}

/// [`usage_in`] in its own read transaction.
pub fn usage(
    storage: &Storage,
    slicer_cache: &Path,
    now: DateTime<Utc>,
) -> Result<StorageUsage, StorageError> {
    storage.read(|connection| usage_in(connection, storage.paths(), slicer_cache, now))
}

// --- cleanup -----------------------------------------------------------------------------

/// What one cleanup removed.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Cleared {
    pub removed_count: u64,
    pub freed_bytes: u64,
}

fn repository(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

fn io_error(_: io::Error) -> CommandError {
    CommandError::persistence_unavailable()
}

/// Runs `target`. `lease` proves the caller holds the backup lease
/// (`clear_storage`, D5): nothing else deletes a blob or an image while it
/// runs.
pub fn clear(
    storage: &Storage,
    content: &ContentStore,
    slicer_cache: &Path,
    lease: &LeaseGuard,
    target: StorageCleanupTarget,
) -> Result<Cleared, CommandError> {
    let paths = storage.paths();
    let cleared = match target {
        StorageCleanupTarget::UnreferencedContent => {
            let blobs = paths.content_root().join(BLOB_DIRECTORY);
            let before = tree(&blobs);
            let done = content
                .clear_unreferenced(storage, lease)
                .map_err(repository)?;
            let after = tree(&blobs);
            crate::f3d_log!(
                info,
                "storage.contentCleared",
                released = done.released as u64,
                orphans = done.orphans_removed as u64,
                failed = done.orphans_failed as u64,
            );
            Cleared {
                removed_count: before.0.saturating_sub(after.0),
                freed_bytes: before.1.saturating_sub(after.1),
            }
        }
        StorageCleanupTarget::PreImportSnapshots => {
            let mut snapshots = accepted_snapshots(paths.snapshot_root());
            // The newest one stays.
            snapshots.pop();
            remove_files(&snapshots)
        }
        StorageCleanupTarget::RotatedLogs => {
            let rotated: Vec<_> = (1..MAX_FILES)
                .map(|index| paths.log_root().join(format!("farm3d.{index}.log")))
                .collect();
            remove_files(&rotated)
        }
        StorageCleanupTarget::OrcaCache => {
            let running: bool = storage
                .read(|connection| {
                    connection.query_row(
                        "SELECT EXISTS(SELECT 1 FROM slice_operations
                                       WHERE state IN ('queued', 'running'))",
                        [],
                        |row| row.get(0),
                    )
                })
                .map_err(repository)?;
            if running {
                return Err(CommandError::storage_in_use(target.as_str()));
            }
            let root = slicer_cache.join(PROFILE_CACHE_DIR);
            let before = tree(&root);
            match fs::remove_dir_all(&root) {
                Ok(()) => {}
                Err(error) if error.kind() == io::ErrorKind::NotFound => {}
                Err(error) => return Err(io_error(error)),
            }
            Cleared {
                removed_count: before.0,
                freed_bytes: before.1,
            }
        }
    };
    crate::f3d_log!(
        info,
        "storage.cleared",
        removed = cleared.removed_count,
        freed = cleared.freed_bytes,
    );
    Ok(cleared)
}

/// Removes each regular file in `paths` (best effort: one that can't be
/// removed stays and isn't counted).
fn remove_files(paths: &[std::path::PathBuf]) -> Cleared {
    let mut cleared = Cleared::default();
    for path in paths {
        let Some(length) = file_len(path) else {
            continue;
        };
        if fs::remove_file(path).is_ok() {
            cleared.removed_count += 1;
            cleared.freed_bytes += length;
        }
    }
    cleared
}

// --- free space ---------------------------------------------------------------------------

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
