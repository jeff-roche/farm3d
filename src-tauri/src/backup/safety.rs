//! D9: safety backups. An ordinary backup with `media: all`, written by
//! farm3d before a restore or a tier (c) reset to
//! `<backup_root>/safety/<sfb-id>.farm3d-backup`, then verified by reading
//! it back (D3 rules 1–8 and 10: every entry streamed and hashed, nothing
//! extracted). One that fails verification is deleted, and the operation
//! fails `BACKUP_SOURCE_DAMAGED`.
//!
//! Retention: only after the new one verifies, safety backups beyond the
//! newest three (by their manifests' `createdAt`) are deleted. A deletion
//! failure is ignored and retried after the next one. A file that doesn't
//! parse is listed as `valid: false` and never counted or deleted by
//! retention.

use std::fs;
use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};

use super::archive::{self, ArchiveError};
use super::dialogs::BACKUP_EXTENSION;
use super::lease::LeaseGuard;
use super::manifest::Manifest;
use super::writer::{write_backup, BackupRequest, BackupWriteError, WriterHooks};
use super::{BackupMediaChoice, BackupOrigin, BackupSummary};
use crate::persistence::{create_contained_directory, Storage, StorageError, StoragePaths};

/// The safety backups retention keeps.
pub const RETAINED: usize = 3;
const SAFETY_DIRECTORY: &str = "safety";
const ID_PREFIX: &str = "sfb";

/// A written and verified safety backup.
#[derive(Clone, Debug)]
pub struct SafetyBackup {
    pub backup_id: String,
    pub bytes: u64,
    pub manifest: Manifest,
}

/// `<backup_root>/safety/`, created (contained, private) on first use.
pub fn safety_root(paths: &StoragePaths) -> Result<PathBuf, StorageError> {
    create_contained_directory(paths.backup_root(), Path::new(SAFETY_DIRECTORY))
}

fn file_name(backup_id: &str) -> String {
    format!("{backup_id}.{BACKUP_EXTENSION}")
}

/// Whether `backup_id` can name a file directly in `safety/` (the file
/// stem `list` reports): no separator, no dot segment, short.
pub fn is_safe_backup_id(backup_id: &str) -> bool {
    !backup_id.is_empty()
        && backup_id.len() <= 128
        && backup_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
}

/// Writes, verifies, and retains a safety backup (D9). The caller holds
/// the lease (`apply_restore` or `reset_farm`).
pub fn write_safety_backup(
    storage: &Storage,
    lease: &LeaseGuard,
    origin: BackupOrigin,
    created_at: DateTime<Utc>,
    app_version: &str,
    hooks: &WriterHooks,
) -> Result<SafetyBackup, BackupWriteError> {
    let root = safety_root(storage.paths()).map_err(|_| BackupWriteError::Io)?;
    let backup_id = crate::library::new_id(ID_PREFIX);
    let path = root.join(file_name(&backup_id));
    let written = write_backup(
        storage,
        lease,
        &path,
        &BackupRequest {
            media: BackupMediaChoice::All,
            origin,
            created_at: Some(created_at),
            app_version: app_version.to_string(),
        },
        hooks,
    )?;
    if let Some(hook) = &hooks.before_verify {
        hook(&path);
    }
    if let Err(error) = archive::verify(&path) {
        let _ = fs::remove_file(&path);
        return Err(BackupWriteError::SourceDamaged {
            entry: failed_entry(&error, &written.manifest),
        });
    }
    retain(&root, &backup_id);
    Ok(SafetyBackup {
        backup_id,
        bytes: written.bytes,
        manifest: written.manifest,
    })
}

/// The archive path of the entry a verification failure names, or
/// `manifest.json` when it names the archive as a whole.
fn failed_entry(error: &ArchiveError, manifest: &Manifest) -> String {
    if let ArchiveError::Invalid { field_path, .. } = error {
        let index = field_path
            .strip_prefix("manifest.entries[")
            .and_then(|rest| rest.split(']').next())
            .and_then(|index| index.parse::<usize>().ok());
        if let Some(entry) = index.and_then(|index| manifest.entries.get(index)) {
            return entry.path.clone();
        }
    }
    archive::MANIFEST_PATH.to_string()
}

/// One file in `safety/`: its id, length, and manifest when it parses.
struct Listed {
    backup_id: String,
    bytes: u64,
    manifest: Option<Manifest>,
}

fn listed(root: &Path) -> Vec<Listed> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let suffix = format!(".{BACKUP_EXTENSION}");
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let Some(backup_id) = name.strip_suffix(&suffix) else {
            continue;
        };
        // Temporary files start with a dot.
        if !is_safe_backup_id(backup_id) {
            continue;
        }
        let path = entry.path();
        let Ok(metadata) = fs::symlink_metadata(&path) else {
            continue;
        };
        if !metadata.is_file() {
            continue;
        }
        found.push(Listed {
            backup_id: backup_id.to_string(),
            bytes: metadata.len(),
            manifest: archive::open(&path)
                .ok()
                .map(|reader| reader.manifest().clone()),
        });
    }
    // Valid ones newest first (then by id), then the ones that don't parse.
    found.sort_by(|left, right| match (&left.manifest, &right.manifest) {
        (Some(a), Some(b)) => b
            .created_at
            .cmp(&a.created_at)
            .then_with(|| left.backup_id.cmp(&right.backup_id)),
        (Some(_), None) => std::cmp::Ordering::Less,
        (None, Some(_)) => std::cmp::Ordering::Greater,
        (None, None) => left.backup_id.cmp(&right.backup_id),
    });
    found
}

/// Deletes the valid safety backups beyond the newest [`RETAINED`], never
/// `keep`. Failures are ignored (retried after the next safety backup).
fn retain(root: &Path, keep: &str) {
    let valid: Vec<Listed> = listed(root)
        .into_iter()
        .filter(|listed| listed.manifest.is_some())
        .collect();
    let mut kept = 0;
    for listed in valid {
        if listed.backup_id == keep || kept < RETAINED {
            kept += 1;
            continue;
        }
        let _ = fs::remove_file(root.join(file_name(&listed.backup_id)));
    }
}

/// `list_backups`: every safety backup, newest first.
pub fn list(paths: &StoragePaths) -> Result<Vec<BackupSummary>, StorageError> {
    let root = safety_root(paths)?;
    Ok(listed(&root)
        .into_iter()
        .map(|listed| match listed.manifest {
            Some(manifest) => BackupSummary {
                backup_id: listed.backup_id,
                origin: manifest.origin,
                created_at: Some(manifest.created_at),
                bytes: listed.bytes,
                app_version: Some(manifest.app_version),
                schema_version: Some(manifest.schema_version),
                media: Some(manifest.contents.media),
                valid: true,
            },
            // The wire type has no "unknown" origin; every file in
            // `safety/` was written before a restore or a reset, and an
            // unreadable one is offered for deletion only.
            None => BackupSummary {
                backup_id: listed.backup_id,
                origin: BackupOrigin::BeforeRestore,
                created_at: None,
                bytes: listed.bytes,
                app_version: None,
                schema_version: None,
                media: None,
                valid: false,
            },
        })
        .collect())
}

/// `delete_backup`: removes `safety/<backup_id>.farm3d-backup`. `false`
/// when there is no such file.
pub fn delete(paths: &StoragePaths, backup_id: &str) -> Result<bool, StorageError> {
    if !is_safe_backup_id(backup_id) {
        return Ok(false);
    }
    let path = safety_root(paths)?.join(file_name(backup_id));
    match fs::symlink_metadata(&path) {
        Ok(metadata) if metadata.is_file() => {
            fs::remove_file(&path)?;
            Ok(true)
        }
        Ok(_) => Ok(false),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(false),
        Err(error) => Err(error.into()),
    }
}
