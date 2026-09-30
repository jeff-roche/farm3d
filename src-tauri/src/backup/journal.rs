//! The restore journal (spec D8 "The journal"; ADR-0016).
//!
//! `<metadata_root>/restore/journal.json` is the one journal. It drives the
//! startup installer ([`super::installer`]) through a restore or a tier (c)
//! reset, and it is the only record of how far an install got, so it is
//! written only atomically ([`write`]: a temporary file, `fsync`, rename,
//! `fsync` the directory). It holds refs and local paths, never a
//! credential value, and never leaves the machine.
//!
//! - [`read`] refuses a journal it can't parse ([`JournalError::Unreadable`])
//!   or one whose `journalVersion` isn't 1 ([`JournalError::Version`]); the
//!   installer then stops startup with `RESTORE_FAILED` and touches nothing.
//! - [`RestoreJournal::set_phase`] allows only D8's legal transitions.
//! - [`remove`] deletes a finished journal and its `restore/<id>/`
//!   directory (`acknowledge_restore_status`, and `apply_restore` before a
//!   new restore).

use std::collections::BTreeMap;
use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::{InstallerStep, RestoreJournalKind, RestoreStatus};
use crate::contracts::command::ErrorCode;
use crate::persistence::{create_contained_directory, StoragePaths};

/// The only `journalVersion` this binary reads or writes.
pub const JOURNAL_VERSION: i64 = 1;
const RESTORE_DIRECTORY: &str = "restore";
const JOURNAL_FILE: &str = "journal.json";
const JOURNAL_TEMP_FILE: &str = ".journal.json.tmp";
const PREVIOUS_DIRECTORY: &str = "previous";

/// `<metadata_root>/restore/`.
pub fn restore_root(paths: &StoragePaths) -> PathBuf {
    paths.metadata_root().join(RESTORE_DIRECTORY)
}

/// `<metadata_root>/restore/journal.json`.
pub fn journal_path(paths: &StoragePaths) -> PathBuf {
    restore_root(paths).join(JOURNAL_FILE)
}

/// `<metadata_root>/restore/<journalId>/`.
pub fn journal_dir(paths: &StoragePaths, journal_id: &str) -> PathBuf {
    restore_root(paths).join(journal_id)
}

/// `<metadata_root>/restore/<journalId>/previous/`: the live database set
/// moved aside during an install.
pub fn previous_dir(paths: &StoragePaths, journal_id: &str) -> PathBuf {
    journal_dir(paths, journal_id).join(PREVIOUS_DIRECTORY)
}

/// A new restore journal id, `rst-<uuid v4>`.
pub fn new_restore_id() -> String {
    format!("rst-{}", uuid::Uuid::new_v4())
}

/// A new reset journal id, `rsf-<uuid v4>`.
pub fn new_reset_id() -> String {
    format!("rsf-{}", uuid::Uuid::new_v4())
}

/// Whether `id` is a journal id (`rst-` or `rsf-` and a hyphenated UUID):
/// the only ids ever joined onto a path or echoed in an error.
pub fn is_journal_id(id: &str) -> bool {
    id.strip_prefix("rst-")
        .or_else(|| id.strip_prefix("rsf-"))
        .is_some_and(|rest| {
            rest.len() == 36
                && uuid::Uuid::try_parse(rest)
                    .is_ok_and(|uuid| uuid.hyphenated().to_string() == rest)
        })
}

/// D8's journal phases.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum JournalPhase {
    Pending,
    Installing,
    Installed,
    Done,
    Failed,
}

impl JournalPhase {
    /// `done` and `failed` are final.
    pub fn is_finished(self) -> bool {
        matches!(self, JournalPhase::Done | JournalPhase::Failed)
    }
}

/// The local Slicer runtime paths carried into the restored database.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CarriedSlicerRuntime {
    pub engine_path: Option<String>,
    pub preset_source_path: Option<String>,
}

/// One local `pending_credential_cleanup` row, carried verbatim.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct CarriedCredentialCleanup {
    pub credential_ref: String,
    pub printer_id: Option<String>,
    pub reason: String,
    pub attempt_count: i64,
    pub last_error_code: Option<String>,
    pub created_at: String,
    pub last_attempt_at: Option<String>,
}

/// What `carryLocalState` writes into the placed database (restore only).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Carry {
    pub slicer_runtime: Option<CarriedSlicerRuntime>,
    pub pending_credential_cleanup: Vec<CarriedCredentialCleanup>,
}

/// Written at the start of `swapMedia`, before any rename.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SwapMedia {
    /// Whether `<media_root>/snapshots` existed (P8 creates it lazily).
    pub live_existed: bool,
}

/// The two-phase rollback's progress, written before a rollback touches
/// anything (D8 "Rollback and retry").
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RollbackMarker {
    /// The step that crashed or failed.
    pub from: InstallerStep,
    /// Rollback phase (a) is durable.
    pub media_restored: bool,
    /// Rollback phase (b) is durable.
    pub candidate_cleared: bool,
}

/// A tier (c) reset's options (Task 8).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct ResetOptions {
    pub delete_safety_backups: bool,
}

/// Set with phase `done`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Outcome {
    pub finished_at: String,
}

/// Set with phase `failed`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct Failure {
    /// `RESTORE_BLOCKED`, `RESTORE_STAGING_EXPIRED`, `BACKUP_INVALID`,
    /// `INSUFFICIENT_SPACE`, or `RESTORE_FAILED`.
    pub code: ErrorCode,
    pub step: Option<InstallerStep>,
    pub finished_at: String,
}

/// D8's `RestoreJournal`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct RestoreJournal {
    pub journal_version: i64,
    /// `rst-…` (restore) or `rsf-…` (reset).
    pub id: String,
    pub kind: RestoreJournalKind,
    pub created_at: String,
    pub phase: JournalPhase,
    /// The step being run; written before it starts.
    pub step: Option<InstallerStep>,
    /// Restore: 0 until the first `markInstalling`; at most 2.
    pub attempts: u32,
    /// Restore only.
    pub staging_id: Option<String>,
    /// `None` only for a reset without a safety backup.
    pub safety_backup_id: Option<String>,
    /// Restore only: the candidate's per-table counts.
    pub expected_counts: Option<BTreeMap<String, i64>>,
    /// Restore only.
    pub carry: Option<Carry>,
    /// Restore: queued as `import_orphan`; reset: deleted from the store.
    pub orphan_credential_refs: Vec<String>,
    pub swap_media: Option<SwapMedia>,
    pub rollback: Option<RollbackMarker>,
    /// Reset only.
    pub reset: Option<ResetOptions>,
    pub outcome: Option<Outcome>,
    pub failure: Option<Failure>,
}

/// Why a journal can't be used: startup stops with `RESTORE_FAILED` and
/// touches nothing.
#[derive(Debug)]
pub enum JournalError {
    /// The file can't be read or parsed (`journalUnreadable`).
    Unreadable,
    /// Its `journalVersion` isn't 1 (`journalVersion`).
    Version,
}

impl JournalError {
    /// `RESTORE_FAILED`'s `details.reason`.
    pub fn reason(&self) -> &'static str {
        match self {
            JournalError::Unreadable => "journalUnreadable",
            JournalError::Version => "journalVersion",
        }
    }
}

/// A phase change D8 doesn't allow.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IllegalTransition {
    pub from: JournalPhase,
    pub to: JournalPhase,
}

impl RestoreJournal {
    /// A new `pending` restore journal.
    #[allow(clippy::too_many_arguments)]
    pub fn new_restore(
        id: String,
        created_at: String,
        staging_id: String,
        safety_backup_id: String,
        expected_counts: BTreeMap<String, i64>,
        carry: Carry,
        orphan_credential_refs: Vec<String>,
    ) -> Self {
        Self {
            journal_version: JOURNAL_VERSION,
            id,
            kind: RestoreJournalKind::Restore,
            created_at,
            phase: JournalPhase::Pending,
            step: None,
            attempts: 0,
            staging_id: Some(staging_id),
            safety_backup_id: Some(safety_backup_id),
            expected_counts: Some(expected_counts),
            carry: Some(carry),
            orphan_credential_refs,
            swap_media: None,
            rollback: None,
            reset: None,
            outcome: None,
            failure: None,
        }
    }

    /// Whether D8 allows `from → to` for this journal's kind.
    pub fn is_legal(kind: RestoreJournalKind, from: JournalPhase, to: JournalPhase) -> bool {
        use JournalPhase::*;
        match kind {
            RestoreJournalKind::Restore => matches!(
                (from, to),
                (Pending, Installing)
                    | (Pending, Failed)
                    | (Installing, Pending)
                    | (Installing, Failed)
                    | (Installing, Installed)
                    | (Installed, Done)
            ),
            RestoreJournalKind::Reset => {
                matches!((from, to), (Pending, Installing) | (Installing, Done))
            }
        }
    }

    /// Moves to `to`, if D8 allows it.
    pub fn set_phase(&mut self, to: JournalPhase) -> Result<(), IllegalTransition> {
        if !Self::is_legal(self.kind, self.phase, to) {
            return Err(IllegalTransition {
                from: self.phase,
                to,
            });
        }
        self.phase = to;
        Ok(())
    }

    /// `restore_status` for this journal: `done` or `failed` once it is
    /// finished, `none` before.
    pub fn status(&self) -> RestoreStatus {
        match self.phase {
            JournalPhase::Done => RestoreStatus::Done {
                journal_id: self.id.clone(),
                kind: self.kind,
                finished_at: self
                    .outcome
                    .as_ref()
                    .map(|outcome| outcome.finished_at.clone())
                    .unwrap_or_else(|| self.created_at.clone()),
                safety_backup_id: self.safety_backup_id.clone(),
            },
            JournalPhase::Failed => {
                let failure = self.failure.clone().unwrap_or(Failure {
                    code: ErrorCode::RestoreFailed,
                    step: self.step,
                    finished_at: self.created_at.clone(),
                });
                RestoreStatus::Failed {
                    journal_id: self.id.clone(),
                    kind: self.kind,
                    finished_at: failure.finished_at,
                    code: failure.code,
                    failed_step: failure.step,
                    safety_backup_id: self.safety_backup_id.clone(),
                }
            }
            _ => RestoreStatus::None,
        }
    }
}

/// The journal, if one exists.
pub fn read(paths: &StoragePaths) -> Result<Option<RestoreJournal>, JournalError> {
    let bytes = match fs::read(journal_path(paths)) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(JournalError::Unreadable),
    };
    parse(&bytes).map(Some)
}

/// Parses a journal: the version first, then the closed schema.
pub fn parse(bytes: &[u8]) -> Result<RestoreJournal, JournalError> {
    let value: serde_json::Value =
        serde_json::from_slice(bytes).map_err(|_| JournalError::Unreadable)?;
    match value
        .get("journalVersion")
        .and_then(serde_json::Value::as_i64)
    {
        Some(JOURNAL_VERSION) => {}
        Some(_) => return Err(JournalError::Version),
        None => return Err(JournalError::Unreadable),
    }
    let journal: RestoreJournal =
        serde_json::from_value(value).map_err(|_| JournalError::Unreadable)?;
    if !is_journal_id(&journal.id) {
        return Err(JournalError::Unreadable);
    }
    if let Some(staging_id) = &journal.staging_id {
        if !super::staging::is_staging_id(staging_id) {
            return Err(JournalError::Unreadable);
        }
    }
    Ok(journal)
}

/// Writes `journal` atomically: a temporary file, `fsync`, rename over
/// `journal.json`, `fsync` the directory. Creating `restore/` on the first
/// write also syncs the metadata root, so the directory itself is durable.
pub fn write(paths: &StoragePaths, journal: &RestoreJournal) -> io::Result<()> {
    write_reporting(paths, journal).map(|_| ())
}

/// [`write`], returning the parent directories it synced for a directory it
/// created.
fn write_reporting(paths: &StoragePaths, journal: &RestoreJournal) -> io::Result<Vec<PathBuf>> {
    let (directory, synced) =
        create_synced_dir(paths.metadata_root(), Path::new(RESTORE_DIRECTORY))?;
    let bytes = serde_json::to_vec_pretty(journal).map_err(io::Error::other)?;
    let temporary = directory.join(JOURNAL_TEMP_FILE);
    let _ = fs::remove_file(&temporary);
    let mut file = File::options()
        .write(true)
        .create_new(true)
        .open(&temporary)?;
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, directory.join(JOURNAL_FILE))?;
    sync_directory(&directory)?;
    Ok(synced)
}

/// Writes a new journal with `write` (production: [`write`]). A write that
/// fails after its rename (on the directory `fsync`) has still left the
/// journal the next start acts on, so an on-disk journal with the same id
/// counts as written: the caller must not report a failure the next start
/// contradicts.
pub fn write_confirmed(
    paths: &StoragePaths,
    journal: &RestoreJournal,
    write: impl FnOnce(&StoragePaths, &RestoreJournal) -> io::Result<()>,
) -> io::Result<()> {
    match write(paths, journal) {
        Ok(()) => Ok(()),
        Err(error) => match read(paths) {
            Ok(Some(on_disk)) if on_disk.id == journal.id => Ok(()),
            _ => Err(error),
        },
    }
}

/// Creates `base/relative` (contained, private), making each directory it
/// creates durable with an `fsync` of its parent. Returns the directory
/// and the parents it synced. I/O errors keep their kind (out of space
/// stays `StorageFull`).
pub(crate) fn create_synced_dir(
    base: &Path,
    relative: &Path,
) -> io::Result<(PathBuf, Vec<PathBuf>)> {
    let mut current = base.to_path_buf();
    let mut synced = Vec::new();
    for component in relative.components() {
        let parent = current.clone();
        current.push(component);
        match fs::symlink_metadata(&current) {
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                match fs::create_dir(&current) {
                    Ok(()) => {
                        sync_directory(&parent)?;
                        synced.push(parent);
                    }
                    Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {}
                    Err(error) => return Err(error),
                }
            }
            Err(error) => return Err(error),
        }
    }
    // The containment, symlink, and permission checks.
    let directory = create_contained_directory(base, relative)
        .map_err(|_| io::Error::other("contained directory"))?;
    Ok((directory, synced))
}

/// Deletes the journal `journal_id`'s directory, then the journal file.
pub fn remove(paths: &StoragePaths, journal_id: &str) -> io::Result<()> {
    if is_journal_id(journal_id) {
        match fs::remove_dir_all(journal_dir(paths, journal_id)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
    }
    match fs::remove_file(journal_path(paths)) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    sync_directory(&restore_root(paths)).or_else(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            Ok(())
        } else {
            Err(error)
        }
    })
}

pub(crate) fn sync_directory(path: &Path) -> io::Result<()> {
    #[cfg(unix)]
    File::open(path)?.sync_all()?;
    #[cfg(not(unix))]
    let _ = path;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every directory created is made durable by an `fsync` of its parent;
    /// one that already existed is not synced again.
    #[test]
    fn created_directories_sync_their_parents() {
        let temp = tempfile::tempdir().unwrap();
        let base = temp.path().canonicalize().unwrap();
        let (created, synced) = create_synced_dir(&base, Path::new("a/b/c")).unwrap();
        assert_eq!(created, base.join("a/b/c"));
        assert_eq!(synced, vec![base.clone(), base.join("a"), base.join("a/b")]);
        let (_, synced) = create_synced_dir(&base, Path::new("a/b/d")).unwrap();
        assert_eq!(synced, vec![base.join("a/b")]);
        let (_, synced) = create_synced_dir(&base, Path::new("a/b/d")).unwrap();
        assert!(synced.is_empty());
    }

    /// The journal's own directory is durable once the journal is written.
    #[test]
    fn the_first_journal_write_syncs_the_metadata_root() {
        let temp = tempfile::tempdir().unwrap();
        let paths = StoragePaths::new(temp.path().join("m"), temp.path().join("d")).unwrap();
        let synced = write_reporting(&paths, &sample()).unwrap();
        assert!(synced.contains(&paths.metadata_root().to_path_buf()));
        let synced = write_reporting(&paths, &sample()).unwrap();
        assert!(!synced.contains(&paths.metadata_root().to_path_buf()));
    }

    fn sample() -> RestoreJournal {
        RestoreJournal::new_restore(
            new_restore_id(),
            "2026-09-29T12:00:00.000Z".to_string(),
            "stg-00000000-0000-4000-8000-000000000000".to_string(),
            "sfb-test".to_string(),
            BTreeMap::new(),
            Carry {
                slicer_runtime: None,
                pending_credential_cleanup: vec![],
            },
            vec![],
        )
    }
}
