//! P8 D5 "The media store and retention": the media root, the
//! `camera_snapshots` repository, the capture write, the prune pass, and
//! the startup sweep.
//!
//! - **Root.** `StoragePaths::media_root()` (`<app data>/farm3d-media/v1`).
//!   Images live at `snapshots/<yyyy>/<mm>/<snp-id>.<jpg|png>` (UTC capture
//!   time); in-progress writes go to `tmp/`. `rel_path` is stored relative
//!   to the root and never leaves Rust (global constraint 3).
//! - **Capture order** ([`store_frame`]): the frame is already in memory →
//!   take the janitor lock → `plan_prune` for the incoming size → write
//!   `tmp/<snp-id>.part`, fsync, rename into place → one transaction: mark
//!   the planned rows pruned, insert the new row, and write the timeline
//!   row, the Event's evidence, or (for `capture_snapshot`) claim the
//!   `operationId` → commit → release the lock → unlink the pruned files. A
//!   failure before commit unlinks the renamed file.
//! - **Pruning** ([`prune_pass`]): `plan_prune` with nothing incoming,
//!   marked in one transaction, unlinked after commit. A pruned row stays,
//!   with `pruned_at` and `prune_reason`; an Incident-linked one gets
//!   `evidencePruned`.
//! - **Startup sweep** ([`startup_sweep`]): empty `tmp/`; mark every
//!   unpruned row whose file is missing `missingFile` (pinned or not); then
//!   delete every image file with no unpruned row (an orphan from a crash
//!   after the rename, or a pruned file whose unlink never ran).
//!
//! Usage is `SUM(byte_len)` over unpruned rows, never a filesystem walk.

use std::collections::{BTreeSet, HashSet};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Component, Path, PathBuf};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::attention::{repository as attention_repository, AttentionEvent, EvidenceOutcome};
use crate::incidents::{repository as incidents_repository, Incident, IncidentEntryDetail};
use crate::persistence::{create_contained_directory, RepositoryError, Storage, StorageError};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::{decode_enum, encode_enum};

use super::fetch::Frame;
use super::retention::{
    plan_prune, CaptureFault, MediaJanitor, PruneAction, RetainedSnapshot, RetentionPolicy,
};
use super::{
    CameraContentType, CameraErrorKind, CameraSnapshot, EvidenceSkipReason, MediaUsage,
    PruneReason, SnapshotPage, SnapshotTrigger,
};

const SNAPSHOTS_DIRECTORY: &str = "snapshots";
const TMP_DIRECTORY: &str = "tmp";
const SNAPSHOT_ID_PREFIX: &str = "snp";

/// A new `snp-<uuid v4>` id.
pub fn new_snapshot_id() -> String {
    crate::library::new_id(SNAPSHOT_ID_PREFIX)
}

fn timestamp(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn parse_timestamp(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

// --- the media root -----------------------------------------------------------------

/// The farm3d-owned media root. Cheap to build: a path, whose directories
/// are created (contained, private) when a write needs them.
#[derive(Clone, Debug)]
pub struct MediaStore {
    root: PathBuf,
}

impl MediaStore {
    /// `root` must be canonical (as `StoragePaths::media_root()` is).
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// The store under `storage`'s media root.
    pub fn for_storage(storage: &Storage) -> Self {
        Self::new(storage.paths().media_root())
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// `snapshots/<yyyy>/<mm>/<snp-id>.<jpg|png>`, by UTC capture time.
    pub fn rel_path(
        snapshot_id: &str,
        captured_at: DateTime<Utc>,
        content_type: CameraContentType,
    ) -> String {
        let extension = match content_type {
            CameraContentType::Jpeg => "jpg",
            CameraContentType::Png => "png",
        };
        format!(
            "{SNAPSHOTS_DIRECTORY}/{}/{snapshot_id}.{extension}",
            captured_at.format("%Y/%m")
        )
    }

    /// The absolute path of a stored `rel_path`, or `None` for one that
    /// isn't a plain relative path under `snapshots/`.
    fn path_of(&self, rel_path: &str) -> Option<PathBuf> {
        let relative = Path::new(rel_path);
        let mut components = relative.components();
        if components.next() != Some(Component::Normal(SNAPSHOTS_DIRECTORY.as_ref())) {
            return None;
        }
        if !components.all(|component| matches!(component, Component::Normal(_))) {
            return None;
        }
        Some(self.root.join(relative))
    }

    /// Writes one image: `tmp/<snp-id>.part`, fsync, rename to `rel_path`,
    /// fsync the directory. On failure nothing is left behind.
    pub fn write_image(
        &self,
        snapshot_id: &str,
        rel_path: &str,
        bytes: &[u8],
    ) -> Result<(), StorageError> {
        let part = self.write_part(snapshot_id, bytes)?;
        self.rename_part(&part, rel_path)
    }

    /// The capture write's first half: `tmp/<snp-id>.part`, written and
    /// fsynced. On failure nothing is left behind.
    fn write_part(&self, snapshot_id: &str, bytes: &[u8]) -> Result<PathBuf, StorageError> {
        let tmp = create_contained_directory(&self.root, Path::new(TMP_DIRECTORY))?;
        let part = tmp.join(format!("{snapshot_id}.part"));
        let written = (|| -> Result<(), StorageError> {
            let mut file = create_private_file(&part)?;
            file.write_all(bytes)?;
            file.sync_all()?;
            Ok(())
        })();
        match written {
            Ok(()) => Ok(part),
            Err(error) => {
                let _ = fs::remove_file(&part);
                Err(error)
            }
        }
    }

    /// The second half: the part renamed to `rel_path`, and its directory
    /// fsynced. On failure the part is removed.
    fn rename_part(&self, part: &Path, rel_path: &str) -> Result<(), StorageError> {
        let renamed = (|| -> Result<(), StorageError> {
            let target = self.path_of(rel_path).ok_or(StorageError::PathCollision)?;
            let relative_parent = Path::new(rel_path)
                .parent()
                .ok_or(StorageError::PathCollision)?;
            let directory = create_contained_directory(&self.root, relative_parent)?;
            let file_name = target.file_name().ok_or(StorageError::PathCollision)?;
            fs::rename(part, directory.join(file_name))?;
            sync_directory(&directory)?;
            Ok(())
        })();
        if renamed.is_err() {
            let _ = fs::remove_file(part);
        }
        renamed
    }

    /// A stored image, or `None` when its file is gone.
    pub fn read_image(&self, rel_path: &str) -> Result<Option<Vec<u8>>, StorageError> {
        let path = self.path_of(rel_path).ok_or(StorageError::PathCollision)?;
        match fs::read(&path) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error.into()),
        }
    }

    /// Whether `rel_path`'s file is there.
    pub fn exists(&self, rel_path: &str) -> bool {
        self.path_of(rel_path)
            .and_then(|path| fs::symlink_metadata(path).ok())
            .is_some_and(|metadata| metadata.is_file())
    }

    /// Removes `rel_path`'s file, if there. Best-effort: a file that can't
    /// be removed now is an orphan the next startup sweep deletes.
    pub fn unlink(&self, rel_path: &str) {
        if let Some(path) = self.path_of(rel_path) {
            let _ = fs::remove_file(path);
        }
    }

    fn unlink_all(&self, rel_paths: &[String]) {
        for rel_path in rel_paths {
            self.unlink(rel_path);
        }
    }

    /// P9 D5: unlinks `rel_paths` after a commit, or, while the backup
    /// lease is held, queues them until it drops.
    fn unlink_after_commit(&self, janitor: &MediaJanitor, rel_paths: &[String]) {
        let paths = rel_paths
            .iter()
            .filter_map(|rel_path| self.path_of(rel_path))
            .collect();
        janitor.backup_lease().unlink_or_defer(paths);
    }

    /// Every entry of `tmp/`, removed.
    fn clear_tmp(&self) -> Result<(), StorageError> {
        let tmp = self.root.join(TMP_DIRECTORY);
        let entries = match fs::read_dir(&tmp) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        for entry in entries {
            let entry = entry?;
            let file_type = entry.file_type()?;
            if file_type.is_dir() {
                fs::remove_dir_all(entry.path())?;
            } else {
                fs::remove_file(entry.path())?;
            }
        }
        Ok(())
    }

    /// Every non-directory under `snapshots/`, as a `/`-separated path
    /// relative to the root.
    fn stored_files(&self) -> Result<Vec<String>, StorageError> {
        let mut found = Vec::new();
        let mut pending = vec![PathBuf::from(SNAPSHOTS_DIRECTORY)];
        while let Some(relative) = pending.pop() {
            let entries = match fs::read_dir(self.root.join(&relative)) {
                Ok(entries) => entries,
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = entry?;
                let child = relative.join(entry.file_name());
                if entry.file_type()?.is_dir() {
                    pending.push(child);
                } else {
                    let text = child
                        .components()
                        .map(|component| component.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/");
                    found.push(text);
                }
            }
        }
        found.sort();
        Ok(found)
    }

    fn remove_relative(&self, relative: &str) {
        let _ = fs::remove_file(self.root.join(relative));
    }
}

fn create_private_file(path: &Path) -> std::io::Result<fs::File> {
    let mut options = OpenOptions::new();
    options.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options.open(path)
}

#[cfg(unix)]
fn sync_directory(path: &Path) -> Result<(), StorageError> {
    fs::File::open(path)?.sync_all()?;
    Ok(())
}

#[cfg(not(unix))]
fn sync_directory(_: &Path) -> Result<(), StorageError> {
    Ok(())
}

// --- the camera_snapshots repository ------------------------------------------------

const SNAPSHOT_COLUMNS: &str =
    "id, revision, printer_id, incident_id, job_id, trigger, captured_at, \
     content_type, byte_len, sha256, pinned_at, pruned_at, prune_reason";

fn decode_text_enum<T: serde::de::DeserializeOwned>(
    index: usize,
    text: &str,
) -> rusqlite::Result<T> {
    decode_enum(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn decode_snapshot(row: &rusqlite::Row<'_>) -> rusqlite::Result<CameraSnapshot> {
    let trigger: String = row.get(5)?;
    let content_type: String = row.get(7)?;
    let prune_reason: Option<String> = row.get(12)?;
    Ok(CameraSnapshot {
        id: row.get(0)?,
        revision: row.get(1)?,
        printer_id: row.get(2)?,
        incident_id: row.get(3)?,
        job_id: row.get(4)?,
        trigger: decode_text_enum(5, &trigger)?,
        captured_at: row.get(6)?,
        content_type: decode_text_enum(7, &content_type)?,
        byte_len: row.get(8)?,
        sha256: row.get(9)?,
        pinned_at: row.get(10)?,
        pruned_at: row.get(11)?,
        prune_reason: prune_reason
            .map(|text| decode_text_enum(12, &text))
            .transpose()?,
    })
}

/// One snapshot row (never its `rel_path`).
pub fn load_snapshot(conn: &Connection, id: &str) -> Result<Option<CameraSnapshot>, StorageError> {
    Ok(conn
        .query_row(
            &format!("SELECT {SNAPSHOT_COLUMNS} FROM camera_snapshots WHERE id = ?1"),
            [id],
            decode_snapshot,
        )
        .optional()?)
}

/// The `manual` snapshot `capture_snapshot` stored under `operation_id`.
pub fn snapshot_for_operation(
    conn: &Connection,
    operation_id: &str,
) -> Result<Option<CameraSnapshot>, StorageError> {
    Ok(conn
        .query_row(
            &format!("SELECT {SNAPSHOT_COLUMNS} FROM camera_snapshots WHERE operation_id = ?1"),
            [operation_id],
            decode_snapshot,
        )
        .optional()?)
}

/// A snapshot's stored image path (Rust-only).
pub fn rel_path_of(conn: &Connection, id: &str) -> Result<Option<String>, StorageError> {
    Ok(conn
        .query_row(
            "SELECT rel_path FROM camera_snapshots WHERE id = ?1",
            [id],
            |row| row.get(0),
        )
        .optional()?)
}

/// `list_snapshots`' filters.
#[derive(Clone, Copy, Debug, Default)]
pub struct SnapshotFilter<'a> {
    pub printer_id: Option<&'a str>,
    pub incident_id: Option<&'a str>,
    pub job_id: Option<&'a str>,
    pub include_pruned: bool,
}

fn encode_list_cursor(captured_at: &str, id: &str) -> String {
    format!("{captured_at}|{id}")
}

/// Whether `cursor` has the shape [`list`] mints (`capturedAt|id`).
pub fn is_well_formed_cursor(cursor: &str) -> bool {
    cursor
        .split_once('|')
        .is_some_and(|(captured_at, id)| !captured_at.is_empty() && !id.is_empty())
}

/// `list_snapshots`: `capturedAt` descending, then id descending, from
/// after the exclusive `before` cursor.
pub fn list(
    conn: &Connection,
    filter: SnapshotFilter<'_>,
    before: Option<&str>,
    limit: i64,
) -> Result<SnapshotPage, StorageError> {
    let (captured_before, id_before) = match before.and_then(|cursor| cursor.split_once('|')) {
        Some((captured_at, id)) => (Some(captured_at), Some(id)),
        None => (None, None),
    };
    let mut statement = conn.prepare(&format!(
        "SELECT {SNAPSHOT_COLUMNS} FROM camera_snapshots
         WHERE (?1 IS NULL OR printer_id = ?1)
           AND (?2 IS NULL OR incident_id = ?2)
           AND (?3 IS NULL OR job_id = ?3)
           AND (?4 OR pruned_at IS NULL)
           AND (?5 IS NULL OR captured_at < ?5 OR (captured_at = ?5 AND id < ?6))
         ORDER BY captured_at DESC, id DESC
         LIMIT ?7"
    ))?;
    let mut snapshots = statement
        .query_map(
            params![
                filter.printer_id,
                filter.incident_id,
                filter.job_id,
                filter.include_pruned,
                captured_before,
                id_before,
                limit + 1
            ],
            decode_snapshot,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = snapshots.len() as i64 > limit;
    if has_more {
        snapshots.truncate(limit as usize);
    }
    let next_cursor = has_more.then(|| {
        let last = snapshots.last().expect("has_more implies a row");
        encode_list_cursor(&last.captured_at, &last.id)
    });
    Ok(SnapshotPage {
        snapshots,
        next_cursor,
    })
}

/// The retention settings (the migration's defaults when there is no
/// settings row yet).
pub fn read_policy(conn: &Connection) -> rusqlite::Result<RetentionPolicy> {
    Ok(conn
        .query_row(
            "SELECT snapshot_retention_days, snapshot_disk_cap_mb FROM settings WHERE singleton_id = 1",
            [],
            |row| Ok(RetentionPolicy::from_settings(row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .unwrap_or(RetentionPolicy::DEFAULT))
}

/// `media_usage`: the stored totals. `snapshotCount` counts every row,
/// `pinnedCount` the pinned rows with an image, `prunedCount` the pruned
/// rows; `usedBytes` and `pinnedBytes` sum the unpruned rows.
pub fn usage(conn: &Connection) -> rusqlite::Result<MediaUsage> {
    let policy = read_policy(conn)?;
    conn.query_row(
        "SELECT
             COALESCE(SUM(CASE WHEN pruned_at IS NULL THEN byte_len END), 0),
             COALESCE(SUM(CASE WHEN pruned_at IS NULL AND pinned_at IS NOT NULL THEN byte_len END), 0),
             COUNT(*),
             COUNT(CASE WHEN pruned_at IS NULL AND pinned_at IS NOT NULL THEN 1 END),
             COUNT(CASE WHEN pruned_at IS NOT NULL THEN 1 END)
         FROM camera_snapshots",
        [],
        |row| {
            Ok(MediaUsage {
                used_bytes: row.get(0)?,
                pinned_bytes: row.get(1)?,
                cap_bytes: policy.cap_bytes,
                retention_days: policy.retention_days,
                snapshot_count: row.get(2)?,
                pinned_count: row.get(3)?,
                pruned_count: row.get(4)?,
            })
        },
    )
}

/// The planner's input: every unpruned row.
pub fn retained_rows(conn: &Connection) -> rusqlite::Result<Vec<RetainedSnapshot>> {
    let mut statement = conn.prepare(
        "SELECT id, captured_at, byte_len, pinned_at IS NOT NULL FROM camera_snapshots
         WHERE pruned_at IS NULL",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, i64>(2)?,
                row.get::<_, bool>(3)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows
        .into_iter()
        .map(|(id, captured_at, byte_len, pinned)| RetainedSnapshot {
            id,
            // An unreadable time sorts first: pruned before anything it
            // could be compared with.
            captured_at: parse_timestamp(&captured_at).unwrap_or(DateTime::<Utc>::MIN_UTC),
            byte_len,
            pinned,
        })
        .collect())
}

/// Whether the Incident already recorded its capture's outcome.
pub fn incident_has_evidence(conn: &Connection, incident_id: &str) -> Result<bool, StorageError> {
    Ok(conn.query_row(
        "SELECT EXISTS(SELECT 1 FROM incident_events WHERE incident_id = ?1
                         AND kind IN ('evidenceCaptured', 'evidenceSkipped'))",
        [incident_id],
        |row| row.get(0),
    )?)
}

/// P8 D8, Printer delete step 2, inside the delete's transaction (after
/// its guards): deletes the Printer's `manual` snapshot rows with no
/// Incident and no Job that are unpinned or pruned, and returns the
/// `rel_path`s of the unpruned ones, whose files the caller unlinks after
/// commit ([`MediaStore::unlink`]). A pinned, unpruned row is left in
/// place: `PINNED_EVIDENCE_EXISTS` has already blocked the delete, and
/// `ON DELETE RESTRICT` backs that up. A crash between the commit and the
/// unlink leaves files with no row, which the startup sweep deletes.
pub fn remove_unattached_manual(
    tx: &Transaction<'_>,
    printer_id: &str,
) -> Result<Vec<String>, StorageError> {
    const UNATTACHED: &str = "printer_id = ?1 AND trigger = 'manual'
         AND incident_id IS NULL AND job_id IS NULL
         AND (pinned_at IS NULL OR pruned_at IS NOT NULL)";
    let rel_paths = {
        let mut statement = tx.prepare(&format!(
            "SELECT rel_path FROM camera_snapshots WHERE {UNATTACHED} AND pruned_at IS NULL
             ORDER BY id"
        ))?;
        let paths = statement
            .query_map([printer_id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        paths
    };
    tx.execute(
        &format!("DELETE FROM camera_snapshots WHERE {UNATTACHED}"),
        [printer_id],
    )?;
    Ok(rel_paths)
}

// --- what a media transaction changed ----------------------------------------------

/// A committed media transaction's rows, to publish on the `attention`
/// stream (Events, then Incidents, then snapshots), each once with its
/// final row.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MediaChanges {
    pub events: Vec<AttentionEvent>,
    pub incidents: Vec<Incident>,
    pub snapshots: Vec<CameraSnapshot>,
}

impl MediaChanges {
    pub fn is_empty(&self) -> bool {
        self.events.is_empty() && self.incidents.is_empty() && self.snapshots.is_empty()
    }

    pub fn extend(&mut self, other: MediaChanges) {
        self.events.extend(other.events);
        self.incidents.extend(other.incidents);
        self.snapshots.extend(other.snapshots);
    }
}

/// Ids a transaction touched, loaded as final rows at its end.
#[derive(Default)]
struct Touched {
    events: BTreeSet<String>,
    incidents: BTreeSet<String>,
    snapshots: Vec<String>,
}

impl Touched {
    fn snapshot(&mut self, id: &str) {
        if !self.snapshots.iter().any(|seen| seen == id) {
            self.snapshots.push(id.to_string());
        }
    }

    fn load(self, tx: &Transaction<'_>) -> Result<MediaChanges, RepositoryError> {
        let mut changes = MediaChanges::default();
        for id in &self.events {
            if let Some(event) = attention_repository::load_event(tx, id)? {
                changes.events.push(event);
            }
        }
        for id in &self.incidents {
            if let Some(incident) = incidents_repository::load_incident(tx, id)? {
                changes.incidents.push(incident);
            }
        }
        for id in &self.snapshots {
            if let Some(snapshot) = load_snapshot(tx, id)? {
                changes.snapshots.push(snapshot);
            }
        }
        Ok(changes)
    }
}

/// Marks `actions` pruned (skipping a row already pruned, or pinned since
/// it was planned — except for `missingFile`, which a pinned row can get),
/// writes `evidencePruned` for each Incident-linked one, and returns the
/// files to unlink after commit.
fn mark_pruned(
    tx: &Transaction<'_>,
    actions: &[PruneAction],
    now: DateTime<Utc>,
    touched: &mut Touched,
) -> Result<Vec<String>, RepositoryError> {
    let mut rel_paths = Vec::new();
    for action in actions {
        let reason = encode_enum(action.reason);
        let changed = tx.execute(
            "UPDATE camera_snapshots
                SET pruned_at = ?2, prune_reason = ?3, revision = revision + 1
              WHERE id = ?1 AND pruned_at IS NULL
                AND (pinned_at IS NULL OR ?3 = 'missingFile')",
            params![action.id, timestamp(now), reason],
        )?;
        if changed == 0 {
            continue;
        }
        let (rel_path, incident_id): (String, Option<String>) = tx.query_row(
            "SELECT rel_path, incident_id FROM camera_snapshots WHERE id = ?1",
            [&action.id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )?;
        if let Some(incident_id) = incident_id {
            incidents_repository::append_entry(
                tx,
                &incident_id,
                &IncidentEntryDetail::EvidencePruned {
                    snapshot_id: action.id.clone(),
                    reason: action.reason,
                },
                None,
                now,
            )?;
            touched.incidents.insert(incident_id);
        }
        touched.snapshot(&action.id);
        rel_paths.push(rel_path);
    }
    Ok(rel_paths)
}

// --- the capture write ---------------------------------------------------------------

/// What a stored frame is evidence of.
#[derive(Clone, Copy, Debug)]
pub enum CaptureLink<'a> {
    /// An `incident` capture: linked to the Incident (and its Job), with an
    /// `evidenceCaptured` timeline row.
    Incident {
        incident_id: &'a str,
        job_id: Option<&'a str>,
    },
    /// A `completion` capture: linked to the Job, with the outcome in the
    /// `job.completed` Event's `evidence`.
    Completion { event_id: &'a str, job_id: &'a str },
    /// `capture_snapshot`: linked to the Printer's active Job, if any; its
    /// `operationId` is claimed in the same transaction.
    Manual {
        operation_id: &'a str,
        digest: &'a str,
    },
}

impl CaptureLink<'_> {
    fn trigger(&self) -> SnapshotTrigger {
        match self {
            CaptureLink::Incident { .. } => SnapshotTrigger::Incident,
            CaptureLink::Completion { .. } => SnapshotTrigger::Completion,
            CaptureLink::Manual { .. } => SnapshotTrigger::Manual,
        }
    }
}

/// One frame to store.
pub struct NewSnapshot<'a> {
    pub printer_id: &'a str,
    pub link: CaptureLink<'a>,
    pub frame: &'a Frame,
}

/// `SNAPSHOT_DISK_CAP`'s totals.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DiskCapTotals {
    pub used_bytes: i64,
    pub cap_bytes: i64,
    pub pinned_bytes: i64,
}

/// What [`store_frame`] did.
#[derive(Debug)]
pub enum StoreOutcome {
    /// The frame is stored as `snapshot`; `changes` is everything the
    /// transaction changed (the new row included).
    Stored {
        snapshot: CameraSnapshot,
        changes: MediaChanges,
    },
    /// The cap can't be met with only pinned rows left: nothing stored.
    /// For an `incident` or `completion` capture, `evidenceSkipped
    /// { reason: diskCap }` was recorded; age prunes still applied.
    DiskCap {
        totals: DiskCapTotals,
        changes: MediaChanges,
    },
    /// The outcome was already recorded (another attempt for the same
    /// Incident or Event got there first), or — `capture_snapshot` — the
    /// same `operationId` committed meanwhile (its row, if still there).
    /// Nothing was written.
    AlreadyRecorded(Option<CameraSnapshot>),
}

enum Written {
    Stored(CameraSnapshot, MediaChanges, Vec<String>),
    Already(Option<CameraSnapshot>),
}

fn already_recorded(
    tx: &Transaction<'_>,
    link: &CaptureLink<'_>,
) -> Result<Option<Option<CameraSnapshot>>, RepositoryError> {
    Ok(match link {
        CaptureLink::Incident { incident_id, .. } => {
            incident_has_evidence(tx, incident_id)?.then_some(None)
        }
        CaptureLink::Completion { event_id, .. } => attention_repository::load_event(tx, event_id)?
            .ok_or_else(|| RepositoryError::NotFound {
                entity_id: event_id.to_string(),
            })?
            .evidence
            .is_some()
            .then_some(None),
        CaptureLink::Manual { .. } => None,
    })
}

/// Records a capture that was attempted and failed, or skipped for the
/// cap: `evidenceSkipped` on the Incident, or the `job.completed` Event's
/// `evidence`. Nothing for a manual capture. A second outcome for the same
/// Incident or Event is a no-op.
fn record_skip_in(
    tx: &Transaction<'_>,
    link: &CaptureLink<'_>,
    reason: EvidenceSkipReason,
    error_kind: Option<CameraErrorKind>,
    now: DateTime<Utc>,
    touched: &mut Touched,
) -> Result<(), RepositoryError> {
    match link {
        CaptureLink::Incident { incident_id, .. } => {
            if !incident_has_evidence(tx, incident_id)? {
                incidents_repository::append_entry(
                    tx,
                    incident_id,
                    &IncidentEntryDetail::EvidenceSkipped { reason, error_kind },
                    None,
                    now,
                )?;
                touched.incidents.insert(incident_id.to_string());
            }
        }
        CaptureLink::Completion { event_id, .. } => {
            let (_, changed) = attention_repository::record_evidence(
                tx,
                event_id,
                &EvidenceOutcome::Skipped { reason, error_kind },
            )?;
            if changed {
                touched.events.insert(event_id.to_string());
            }
        }
        CaptureLink::Manual { .. } => {}
    }
    Ok(())
}

/// D4 "Recorded outcome" for a failed fetch: `evidenceSkipped { reason:
/// cameraError, errorKind }` (Incident), or the same in the Event's
/// `evidence` (completion). Returns what changed (empty when the outcome
/// was already recorded).
pub fn record_camera_error(
    storage: &Storage,
    link: &CaptureLink<'_>,
    error_kind: CameraErrorKind,
    now: DateTime<Utc>,
) -> Result<MediaChanges, RepositoryError> {
    record_skip(
        storage,
        link,
        EvidenceSkipReason::CameraError,
        Some(error_kind),
        now,
    )
}

/// Records a capture that was attempted and skipped for `reason`
/// (`evidenceSkipped` on the Incident, or the Event's `evidence`). A no-op
/// when an outcome is already recorded, and for a manual capture.
pub fn record_skip(
    storage: &Storage,
    link: &CaptureLink<'_>,
    reason: EvidenceSkipReason,
    error_kind: Option<CameraErrorKind>,
    now: DateTime<Utc>,
) -> Result<MediaChanges, RepositoryError> {
    storage.write_repo(|tx| {
        let mut touched = Touched::default();
        record_skip_in(tx, link, reason, error_kind, now, &mut touched)?;
        touched.load(tx)
    })
}

fn insert_row(
    tx: &Transaction<'_>,
    snapshot_id: &str,
    request: &NewSnapshot<'_>,
    rel_path: &str,
) -> Result<CameraSnapshot, RepositoryError> {
    let (incident_id, job_id, operation_id) = match request.link {
        CaptureLink::Incident {
            incident_id,
            job_id,
        } => (
            Some(incident_id.to_string()),
            job_id.map(str::to_string),
            None,
        ),
        CaptureLink::Completion { job_id, .. } => (None, Some(job_id.to_string()), None),
        CaptureLink::Manual { operation_id, .. } => (
            None,
            crate::jobs::repository::active_job_for_printer(tx, request.printer_id)?
                .map(|job| job.id),
            Some(operation_id.to_string()),
        ),
    };
    let frame = request.frame;
    tx.execute(
        "INSERT INTO camera_snapshots(
             id, revision, printer_id, incident_id, job_id, trigger, operation_id, captured_at,
             content_type, byte_len, sha256, rel_path
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
        params![
            snapshot_id,
            request.printer_id,
            incident_id,
            job_id,
            encode_enum(request.link.trigger()),
            operation_id,
            timestamp(frame.captured_at),
            encode_enum(frame.content_type),
            frame.bytes.len() as i64,
            format!("{:x}", Sha256::digest(&frame.bytes)),
            rel_path,
        ],
    )?;
    load_snapshot(tx, snapshot_id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: snapshot_id.to_string(),
    })
}

fn totals(rows: &[RetainedSnapshot], policy: RetentionPolicy) -> DiskCapTotals {
    DiskCapTotals {
        used_bytes: rows.iter().map(|row| row.byte_len).sum(),
        cap_bytes: policy.cap_bytes,
        pinned_bytes: rows
            .iter()
            .filter(|row| row.pinned)
            .map(|row| row.byte_len)
            .sum(),
    }
}

/// D5 "Capture order": stores `request`'s frame under the janitor lock.
/// See the module doc for the order. `now` is the prune planner's and the
/// timeline's time; the row's `capturedAt` is the frame's own.
pub async fn store_frame(
    storage: &Storage,
    janitor: &MediaJanitor,
    policy: RetentionPolicy,
    now: DateTime<Utc>,
    request: NewSnapshot<'_>,
) -> Result<StoreOutcome, RepositoryError> {
    let store = MediaStore::for_storage(storage);
    let guard = janitor.lock().await;
    let incoming = request.frame.bytes.len() as i64;
    let rows = storage.read(retained_rows)?;
    let plan = plan_prune(&rows, policy, now, incoming);

    if !plan.fits {
        let totals = totals(&rows, policy);
        let (changes, rel_paths) = storage.write_repo(|tx| {
            let mut touched = Touched::default();
            if already_recorded(tx, &request.link)?.is_none() {
                record_skip_in(
                    tx,
                    &request.link,
                    EvidenceSkipReason::DiskCap,
                    None,
                    now,
                    &mut touched,
                )?;
            }
            let rel_paths = mark_pruned(tx, &plan.prune, now, &mut touched)?;
            Ok((touched.load(tx)?, rel_paths))
        })?;
        drop(guard);
        store.unlink_after_commit(janitor, &rel_paths);
        return Ok(StoreOutcome::DiskCap { totals, changes });
    }

    let snapshot_id = new_snapshot_id();
    let rel_path = MediaStore::rel_path(
        &snapshot_id,
        request.frame.captured_at,
        request.frame.content_type,
    );
    let part = store.write_part(&snapshot_id, &request.frame.bytes)?;
    if janitor.take_fault(CaptureFault::BeforeRename) {
        // A crash here leaves the fsynced part in `tmp/`, nothing else.
        return Err(RepositoryError::Storage(StorageError::OperationFailed));
    }
    store.rename_part(&part, &rel_path)?;
    if janitor.take_fault(CaptureFault::AfterRename) {
        // A crash here leaves the image in place with no row.
        return Err(RepositoryError::Storage(StorageError::OperationFailed));
    }
    let written = storage.write_repo(|tx| {
        if let Some(existing) = already_recorded(tx, &request.link)? {
            return Ok(Written::Already(existing));
        }
        if let CaptureLink::Manual {
            operation_id,
            digest,
        } = request.link
        {
            if operations::claim(tx, operation_id, OperationKind::CaptureSnapshot, digest)?
                == Claim::Replay
            {
                return Ok(Written::Already(snapshot_for_operation(tx, operation_id)?));
            }
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM printers WHERE id = ?1)",
                [request.printer_id],
                |row| row.get(0),
            )?;
            if !exists {
                return Err(RepositoryError::NotFound {
                    entity_id: request.printer_id.to_string(),
                });
            }
        }
        let mut touched = Touched::default();
        let rel_paths = mark_pruned(tx, &plan.prune, now, &mut touched)?;
        let snapshot = insert_row(tx, &snapshot_id, &request, &rel_path)?;
        match request.link {
            CaptureLink::Incident { incident_id, .. } => {
                incidents_repository::append_entry(
                    tx,
                    incident_id,
                    &IncidentEntryDetail::EvidenceCaptured {
                        snapshot_id: snapshot_id.clone(),
                        trigger: SnapshotTrigger::Incident,
                    },
                    None,
                    now,
                )?;
                touched.incidents.insert(incident_id.to_string());
            }
            CaptureLink::Completion { event_id, .. } => {
                attention_repository::record_evidence(
                    tx,
                    event_id,
                    &EvidenceOutcome::Captured {
                        snapshot_id: snapshot_id.clone(),
                    },
                )?;
                touched.events.insert(event_id.to_string());
            }
            CaptureLink::Manual { .. } => {}
        }
        touched.snapshot(&snapshot_id);
        Ok(Written::Stored(snapshot, touched.load(tx)?, rel_paths))
    });
    match written {
        Ok(Written::Stored(snapshot, changes, rel_paths)) => {
            drop(guard);
            store.unlink_after_commit(janitor, &rel_paths);
            Ok(StoreOutcome::Stored { snapshot, changes })
        }
        Ok(Written::Already(existing)) => {
            store.unlink(&rel_path);
            Ok(StoreOutcome::AlreadyRecorded(existing))
        }
        Err(error) => {
            store.unlink(&rel_path);
            Err(error)
        }
    }
}

// --- pruning -------------------------------------------------------------------------

/// A committed prune: its rows, and the files still to unlink.
#[derive(Debug, Default)]
pub struct PruneCommit {
    pub changes: MediaChanges,
    pub rel_paths: Vec<String>,
}

/// The transactional half of a prune pass: plans with nothing incoming and
/// marks the planned rows pruned (with `evidencePruned` if linked), in one
/// transaction. The files are **not** unlinked; the caller does that after
/// commit ([`prune_pass`]), and a crash in between leaves them for the
/// startup sweep.
pub fn mark_prunable(
    storage: &Storage,
    policy: RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<PruneCommit, RepositoryError> {
    storage.write_repo(|tx| {
        let rows = retained_rows(tx)?;
        let plan = plan_prune(&rows, policy, now, 0);
        let mut touched = Touched::default();
        let rel_paths = mark_pruned(tx, &plan.prune, now, &mut touched)?;
        Ok(PruneCommit {
            changes: touched.load(tx)?,
            rel_paths,
        })
    })
}

/// D5 "`MediaJanitor`": one prune pass under the janitor lock; the files
/// are unlinked after commit and after the lock is released. P9 D5: while
/// the backup lease is held the pass is skipped (nothing marked, nothing
/// unlinked); dropping the lease pokes the janitor for another.
pub async fn prune_pass(
    storage: &Storage,
    janitor: &MediaJanitor,
    policy: RetentionPolicy,
    now: DateTime<Utc>,
) -> Result<MediaChanges, RepositoryError> {
    let guard = janitor.lock().await;
    let Some(permit) = janitor.backup_lease().deletion_permit() else {
        return Ok(MediaChanges::default());
    };
    let commit = mark_prunable(storage, policy, now)?;
    drop(guard);
    MediaStore::for_storage(storage).unlink_all(&commit.rel_paths);
    drop(permit);
    Ok(commit.changes)
}

/// A missing image found outside the sweep (`snapshot_image`): the row
/// becomes `pruned: missingFile`, with `evidencePruned` if linked.
pub fn mark_missing(
    storage: &Storage,
    snapshot_id: &str,
    now: DateTime<Utc>,
) -> Result<MediaChanges, RepositoryError> {
    storage.write_repo(|tx| {
        let mut touched = Touched::default();
        mark_pruned(
            tx,
            &[PruneAction {
                id: snapshot_id.to_string(),
                reason: PruneReason::MissingFile,
            }],
            now,
            &mut touched,
        )?;
        touched.load(tx)
    })
}

// --- pinning -------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PinDigest<'a> {
    snapshot_id: &'a str,
    pinned: bool,
}

/// `set_snapshot_pinned`'s digest.
pub fn pin_digest(snapshot_id: &str, pinned: bool) -> String {
    operations::digest(&PinDigest {
        snapshot_id,
        pinned,
    })
}

/// D5 "Pinning": pins or unpins, claiming `operation_id`. The same state is
/// a no-op (nothing changes or publishes). Pinning a pruned row is
/// `EVIDENCE_PRUNED`; unpinning one is allowed. A change bumps `revision`
/// and, for an Incident-linked row, writes `evidencePinned` /
/// `evidenceUnpinned`. Returns the row and what changed.
pub fn set_pinned(
    storage: &Storage,
    operation_id: &str,
    snapshot_id: &str,
    pinned: bool,
    now: DateTime<Utc>,
) -> Result<(CameraSnapshot, MediaChanges), RepositoryError> {
    let digest = pin_digest(snapshot_id, pinned);
    storage.write_repo(|tx| {
        let not_found = || RepositoryError::NotFound {
            entity_id: snapshot_id.to_string(),
        };
        let replay =
            operations::claim(tx, operation_id, OperationKind::SetSnapshotPinned, &digest)?
                == Claim::Replay;
        let current = load_snapshot(tx, snapshot_id)?.ok_or_else(not_found)?;
        if replay {
            return Ok((current, MediaChanges::default()));
        }
        if pinned {
            // Any reason, a pinned `missingFile` row included: there is no
            // image left to protect.
            if let Some(reason) = current.prune_reason {
                return Err(RepositoryError::EvidencePruned {
                    snapshot_id: snapshot_id.to_string(),
                    reason,
                });
            }
        }
        if current.pinned_at.is_some() == pinned {
            return Ok((current, MediaChanges::default()));
        }
        tx.execute(
            "UPDATE camera_snapshots SET pinned_at = ?2, revision = revision + 1 WHERE id = ?1",
            params![snapshot_id, pinned.then(|| timestamp(now))],
        )?;
        let mut touched = Touched::default();
        if let Some(incident_id) = &current.incident_id {
            let detail = if pinned {
                IncidentEntryDetail::EvidencePinned {
                    snapshot_id: snapshot_id.to_string(),
                }
            } else {
                IncidentEntryDetail::EvidenceUnpinned {
                    snapshot_id: snapshot_id.to_string(),
                }
            };
            incidents_repository::append_entry(tx, incident_id, &detail, Some(operation_id), now)?;
            touched.incidents.insert(incident_id.clone());
        }
        touched.snapshot(snapshot_id);
        let changes = touched.load(tx)?;
        let snapshot = load_snapshot(tx, snapshot_id)?.ok_or_else(not_found)?;
        Ok((snapshot, changes))
    })
}

// --- the startup sweep ---------------------------------------------------------------

/// The startup sweep again, at runtime, under the janitor lock (so no
/// capture is between its file write and its commit): the janitor's retry
/// while the media store is unavailable. P9 D5: the sweep deletes orphan
/// files, so while the backup lease is held it doesn't run and fails
/// `PersistenceUnavailable` (the store stays unavailable; dropping the
/// lease pokes the janitor, which retries).
pub async fn sweep_under_lock(
    storage: &Storage,
    janitor: &MediaJanitor,
    now: DateTime<Utc>,
) -> Result<MediaChanges, RepositoryError> {
    let _serialized = janitor.lock().await;
    let Some(_permit) = janitor.backup_lease().deletion_permit() else {
        return Err(RepositoryError::Storage(
            StorageError::PersistenceUnavailable,
        ));
    };
    startup_sweep(storage, now)
}

/// D5 "Startup sweep", before any command is served: empties `tmp/`, marks
/// every unpruned row whose file is missing `missingFile` (pinned or not,
/// with `evidencePruned` if linked), then deletes every image file with no
/// unpruned row. Returns the changed rows, published once the camera
/// runtime starts.
pub fn startup_sweep(
    storage: &Storage,
    now: DateTime<Utc>,
) -> Result<MediaChanges, RepositoryError> {
    let store = MediaStore::for_storage(storage);
    store.clear_tmp()?;
    let changes = storage.write_repo(|tx| {
        let mut statement = tx.prepare(
            "SELECT id, rel_path FROM camera_snapshots WHERE pruned_at IS NULL ORDER BY id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        drop(statement);
        let missing: Vec<PruneAction> = rows
            .into_iter()
            .filter(|(_, rel_path)| !store.exists(rel_path))
            .map(|(id, _)| PruneAction {
                id,
                reason: PruneReason::MissingFile,
            })
            .collect();
        let mut touched = Touched::default();
        mark_pruned(tx, &missing, now, &mut touched)?;
        touched.load(tx)
    })?;
    let keep: HashSet<String> = storage.read(|conn| {
        let mut statement =
            conn.prepare("SELECT rel_path FROM camera_snapshots WHERE pruned_at IS NULL")?;
        let paths = statement
            .query_map([], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<HashSet<_>>>()?;
        Ok(paths)
    })?;
    for relative in store.stored_files()? {
        if !keep.contains(&relative) {
            store.remove_relative(&relative);
        }
    }
    Ok(changes)
}
