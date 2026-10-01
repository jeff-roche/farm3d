//! P9 D15: tiered reset. Three tiers, each with its own exact
//! confirmation phrase, each naming every affected and every kept data
//! class in [`preview`] first:
//!
//! - **(a) `settings`** ([`reset_settings`]): F1's pre-import snapshot
//!   (`SnapshotKind::Settings`), then one transaction that checks
//!   `expectedRevision` (`CONFLICT`), claims the `operationId`
//!   (`resetSettings`), and writes the defaults through the settings save
//!   path. The Slicer runtime and the per-Printer alert defaults stay.
//! - **(b) `cameraMedia`** ([`reset_camera_media`]): P8's prune order with
//!   reason `reset` (`cameras::media::mark_reset`): the rows and Incident
//!   timeline entries stay, the image files go once the lease drops.
//! - **(c) `farm`** ([`write_farm_journal`]): a `reset` journal that the
//!   startup installer rolls forward (`backup::installer`).
//!
//! The Tauri commands (`reset_preview`, `reset_farm`) are in
//! [`super::commands`]; they check the journal (`RESTART_PENDING`) before
//! the lease and before any pre-reset snapshot, and hold the lease for
//! every tier.

use std::collections::BTreeSet;
use std::fs;
use std::io;
use std::path::Path;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::{Connection, OptionalExtension};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::backup::journal::{self, RestoreJournal};
use crate::backup::{preview, RestartingStatus};
use crate::cameras::media::{self, MediaReset};
use crate::connections::credentials::CredentialStore;
use crate::contracts::command::CommandError;
use crate::persistence::{RepositoryError, SnapshotKind, Storage, StorageError, StoragePaths};
use crate::settings::commands::SettingsRecord;
use crate::settings::repository::{self as settings_repository, SavedSettings};
use crate::spools::operations::{self, Claim, OperationKind};

/// A reset tier.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ResetTier.ts")]
pub enum ResetTier {
    Settings,
    CameraMedia,
    Farm,
}

impl ResetTier {
    /// The exact confirmation phrase (no trim, no case folding).
    pub fn phrase(self) -> &'static str {
        match self {
            ResetTier::Settings => "reset settings",
            ResetTier::CameraMedia => "reset media",
            ResetTier::Farm => "reset farm",
        }
    }
}

/// Tier (b)'s scope: the unpinned images, or all of them.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ResetMediaScope.ts")]
pub enum ResetMediaScope {
    Unpinned,
    All,
}

/// `reset_farm`'s `request`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "tier",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "tier",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/ResetRequest.ts"
)]
pub enum ResetRequest {
    Settings {
        #[ts(type = "number")]
        expected_revision: i64,
    },
    CameraMedia {
        scope: ResetMediaScope,
    },
    Farm {
        safety_backup: bool,
        delete_safety_backups: bool,
    },
}

impl ResetRequest {
    pub fn tier(&self) -> ResetTier {
        match self {
            ResetRequest::Settings { .. } => ResetTier::Settings,
            ResetRequest::CameraMedia { .. } => ResetTier::CameraMedia,
            ResetRequest::Farm { .. } => ResetTier::Farm,
        }
    }
}

/// A data class a reset names in its preview.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ResetClass.ts")]
pub enum ResetClass {
    Settings,
    SlicerRuntime,
    PrinterAlertDefaults,
    UnpinnedSnapshots,
    PinnedSnapshots,
    SnapshotRecords,
    Printers,
    Spools,
    Library,
    SliceRevisions,
    QueueAndJobs,
    IncidentsAndAttention,
    Content,
    CameraMedia,
    Logs,
    Credentials,
    PreImportSnapshots,
    RestoreStaging,
    LegacyArchives,
    SafetyBackups,
    SlicerProfileCache,
}

/// What a reset does to a class.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ResetEffect.ts")]
pub enum ResetEffect {
    Reset,
    Pruned,
    Deleted,
    Kept,
}

/// One class in a [`ResetPreview`], with its row or file count and its
/// bytes where they apply (`null` otherwise).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ResetDataClass.ts")]
pub struct ResetDataClass {
    pub class: ResetClass,
    pub effect: ResetEffect,
    #[ts(type = "number | null")]
    pub count: Option<i64>,
    #[ts(type = "number | null")]
    pub bytes: Option<i64>,
}

/// What the operator should know before a reset.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/ResetWarning.ts"
)]
pub enum ResetWarning {
    /// Tier (c): D7's blockers. The reset is not refused for them; their
    /// prints continue on the Printers, unknown to the fresh Farm.
    ActiveWork {
        #[ts(type = "number")]
        active_jobs: i64,
        #[ts(type = "number")]
        host_operations: i64,
        #[ts(type = "number")]
        slice_operations: i64,
    },
    /// Tier (c): farm3d's own credentials may not be deletable; failures
    /// are queued for the next start.
    CredentialStoreUnavailable,
}

/// `reset_preview`'s result.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ResetPreview.ts")]
pub struct ResetPreview {
    pub tier: ResetTier,
    /// The exact confirmation phrase.
    pub phrase: String,
    pub classes: Vec<ResetDataClass>,
    pub warnings: Vec<ResetWarning>,
}

/// `reset_farm`'s result.
#[derive(Serialize, TS)]
#[serde(
    tag = "tier",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "tier",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/ResetResult.ts"
)]
pub enum ResetResult {
    Settings {
        settings: SettingsRecord,
    },
    CameraMedia {
        #[ts(type = "number")]
        pruned_count: i64,
        #[ts(type = "number")]
        freed_bytes: i64,
    },
    Farm {
        status: RestartingStatus,
        safety_backup_id: Option<String>,
    },
}

// --- tier (a) --------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDigest {
    tier: ResetTier,
    expected_revision: i64,
}

/// `resetSettings`' digest, `{ tier, expectedRevision }`.
pub fn settings_digest(expected_revision: i64) -> String {
    operations::digest(&SettingsDigest {
        tier: ResetTier::Settings,
        expected_revision,
    })
}

/// The `(kind, digest)` recorded for `operation_id`, if any.
fn recorded(
    connection: &Connection,
    operation_id: &str,
) -> rusqlite::Result<Option<(String, String)>> {
    connection
        .query_row(
            "SELECT kind, request_digest FROM operations WHERE id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

/// Tier (a). The caller has checked the journal and holds the lease. A
/// replay returns the current record with no side effect (no snapshot);
/// a stale `expected_revision` is `CONFLICT` before the snapshot is taken,
/// and again inside the transaction.
pub fn reset_settings(
    storage: &Storage,
    operation_id: &str,
    expected_revision: i64,
) -> Result<SavedSettings, RepositoryError> {
    if operation_id.trim().is_empty() {
        return Err(RepositoryError::Validation {
            field_path: "operationId",
        });
    }
    let digest = settings_digest(expected_revision);
    let (prior, current) = storage.read_transaction(|tx| {
        Ok((
            recorded(tx, operation_id)?,
            settings_repository::load_record(tx)?,
        ))
    })?;
    match prior {
        Some((kind, recorded_digest)) if kind == "resetSettings" && recorded_digest == digest => {
            return Ok(SavedSettings {
                record: current,
                retention_changed: false,
            });
        }
        Some(_) => return Err(RepositoryError::OperationIdReused),
        None => {}
    }
    if current.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: "settings".to_string(),
            expected_revision,
            current_revision: current.revision,
        });
    }
    // F1's pre-import snapshot: the settings as they were.
    storage
        .create_snapshot(SnapshotKind::Settings)
        .map_err(RepositoryError::Storage)?;
    storage.write_repo(|tx| {
        if operations::claim(tx, operation_id, OperationKind::ResetSettings, &digest)?
            == Claim::Replay
        {
            return Ok(SavedSettings {
                record: settings_repository::load_record(tx).map_err(StorageError::from)?,
                retention_changed: false,
            });
        }
        settings_repository::save_update_in(
            tx,
            expected_revision,
            &crate::settings::commands::default_update(),
        )
    })
}

// --- tier (b) --------------------------------------------------------------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CameraMediaDigest {
    tier: ResetTier,
    scope: ResetMediaScope,
}

/// `resetCameraMedia`'s digest, `{ tier, scope }`.
pub fn camera_media_digest(scope: ResetMediaScope) -> String {
    operations::digest(&CameraMediaDigest {
        tier: ResetTier::CameraMedia,
        scope,
    })
}

/// Tier (b)'s transaction: every selected unpruned row pruned `reset`
/// (pinned rows too for `all`, keeping `pinned_at`), `evidencePruned` for
/// each Incident-linked one, and the `operationId` claimed. The caller
/// holds the janitor lock and the lease, and queues the returned files
/// (`cameras::media::queue_unlinks`) so they are unlinked when the lease
/// drops. A replay prunes nothing.
pub fn reset_camera_media(
    storage: &Storage,
    operation_id: &str,
    scope: ResetMediaScope,
    now: DateTime<Utc>,
) -> Result<MediaReset, RepositoryError> {
    media::mark_reset(
        storage,
        operation_id,
        &camera_media_digest(scope),
        scope == ResetMediaScope::All,
        now,
    )
}

// --- tier (c) --------------------------------------------------------------------------

/// Tier (c)'s journal: every credential ref the live Farm names (its
/// Printers' and its pending cleanup rows'; never a value), the safety
/// backup to keep, and the operator's option, written `pending`
/// atomically. The caller has checked for a waiting journal, holds the
/// lease, and wrote the safety backup (if any).
pub fn write_farm_journal(
    storage: &Storage,
    safety_backup_id: Option<String>,
    delete_safety_backups: bool,
    now: DateTime<Utc>,
) -> Result<RestoreJournal, CommandError> {
    write_farm_journal_with(
        storage,
        safety_backup_id,
        delete_safety_backups,
        now,
        journal::write,
    )
}

/// [`write_farm_journal`] with the journal write injected (tests). See
/// `journal::write_confirmed`.
pub fn write_farm_journal_with(
    storage: &Storage,
    safety_backup_id: Option<String>,
    delete_safety_backups: bool,
    now: DateTime<Utc>,
    write: impl FnOnce(&StoragePaths, &RestoreJournal) -> io::Result<()>,
) -> Result<RestoreJournal, CommandError> {
    let refs: BTreeSet<String> = storage
        .read_transaction(|tx| preview::local_refs(tx))
        .map_err(|_| CommandError::persistence_unavailable())?;
    let reset = RestoreJournal::new_reset(
        journal::new_reset_id(),
        now.to_rfc3339_opts(SecondsFormat::Millis, true),
        safety_backup_id,
        refs.into_iter().collect(),
        delete_safety_backups,
    );
    journal::write_confirmed(storage.paths(), &reset, write)
        .map_err(|_| CommandError::persistence_unavailable())?;
    Ok(reset)
}

// --- the preview ------------------------------------------------------------------------

/// `(count, bytes)` of every regular file under `root` (0, 0 when absent).
pub(crate) fn tree_usage(root: &Path) -> (i64, i64) {
    let mut count = 0_i64;
    let mut bytes = 0_i64;
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let Ok(entries) = fs::read_dir(&directory) else {
            continue;
        };
        for entry in entries.flatten() {
            let Ok(metadata) = fs::symlink_metadata(entry.path()) else {
                continue;
            };
            if metadata.is_dir() {
                pending.push(entry.path());
            } else if metadata.is_file() {
                count += 1;
                bytes += i64::try_from(metadata.len()).unwrap_or(i64::MAX);
            }
        }
    }
    (count, bytes)
}

/// The sum of [`tree_usage`] over the entries of `directory` whose names
/// `select` accepts.
fn entries_usage(directory: &Path, select: impl Fn(&str) -> bool) -> (i64, i64) {
    let Ok(entries) = fs::read_dir(directory) else {
        return (0, 0);
    };
    let mut total = (0, 0);
    for entry in entries.flatten() {
        let name = entry.file_name();
        if !name.to_str().is_some_and(&select) {
            continue;
        }
        let path = entry.path();
        let (count, bytes) = match fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_dir() => tree_usage(&path),
            Ok(metadata) if metadata.is_file() => {
                (1, i64::try_from(metadata.len()).unwrap_or(i64::MAX))
            }
            _ => (0, 0),
        };
        total.0 += count;
        total.1 += bytes;
    }
    total
}

const STAGING_DIRECTORY: &str = ".restore-staging";

/// What the preview reads from the database, in one read transaction.
struct Facts {
    settings: i64,
    slicer_runtime: i64,
    alert_defaults: i64,
    unpinned: (i64, i64),
    pinned: (i64, i64),
    snapshot_rows: i64,
    printers: i64,
    spools: i64,
    library: i64,
    slice_revisions: i64,
    queue_and_jobs: i64,
    incidents_and_attention: i64,
    credential_refs: i64,
    active_jobs: i64,
    host_operations: i64,
    slice_operations: i64,
}

fn count(connection: &Connection, sql: &str) -> rusqlite::Result<i64> {
    connection.query_row(sql, [], |row| row.get(0))
}

fn count_bytes(connection: &Connection, sql: &str) -> rusqlite::Result<(i64, i64)> {
    connection.query_row(sql, [], |row| Ok((row.get(0)?, row.get(1)?)))
}

fn facts(connection: &Connection) -> rusqlite::Result<Facts> {
    Ok(Facts {
        settings: count(connection, "SELECT count(*) FROM settings")?,
        slicer_runtime: count(connection, "SELECT count(*) FROM slicer_runtime_config")?,
        alert_defaults: count(connection, "SELECT count(*) FROM printer_alert_defaults")?,
        unpinned: count_bytes(
            connection,
            "SELECT count(*), COALESCE(sum(byte_len), 0) FROM camera_snapshots
              WHERE pruned_at IS NULL AND pinned_at IS NULL",
        )?,
        pinned: count_bytes(
            connection,
            "SELECT count(*), COALESCE(sum(byte_len), 0) FROM camera_snapshots
              WHERE pruned_at IS NULL AND pinned_at IS NOT NULL",
        )?,
        snapshot_rows: count(connection, "SELECT count(*) FROM camera_snapshots")?,
        printers: count(connection, "SELECT count(*) FROM printers")?,
        spools: count(connection, "SELECT count(*) FROM spools")?,
        library: count(
            connection,
            "SELECT (SELECT count(*) FROM library_models) + (SELECT count(*) FROM library_projects)",
        )?,
        slice_revisions: count(connection, "SELECT count(*) FROM slice_revisions")?,
        queue_and_jobs: count(
            connection,
            "SELECT (SELECT count(*) FROM queue_entries) + (SELECT count(*) FROM jobs)",
        )?,
        incidents_and_attention: count(
            connection,
            "SELECT (SELECT count(*) FROM incidents) + (SELECT count(*) FROM attention_events)",
        )?,
        credential_refs: i64::try_from(preview::local_refs(connection)?.len()).unwrap_or(i64::MAX),
        // D7's blockers, by kind.
        active_jobs: count(
            connection,
            "SELECT count(*) FROM jobs WHERE state NOT IN ('completed', 'failed', 'cancelled')",
        )?,
        host_operations: count(
            connection,
            "SELECT count(*) FROM host_operations
              WHERE state IN ('dispatching', 'uncertain', 'reconciling')",
        )?,
        slice_operations: count(
            connection,
            "SELECT count(*) FROM slice_operations WHERE state IN ('queued', 'running')",
        )?,
    })
}

fn class(
    class: ResetClass,
    effect: ResetEffect,
    count: Option<i64>,
    bytes: Option<i64>,
) -> ResetDataClass {
    ResetDataClass {
        class,
        effect,
        count,
        bytes,
    }
}

fn files(
    class_name: ResetClass,
    effect: ResetEffect,
    (count, bytes): (i64, i64),
) -> ResetDataClass {
    class(class_name, effect, Some(count), Some(bytes))
}

fn rows(class_name: ResetClass, effect: ResetEffect, count: i64) -> ResetDataClass {
    class(class_name, effect, Some(count), None)
}

/// `reset_preview(tier)`: every class the tier affects and every class it
/// keeps, with counts and bytes, and the warnings. Rust decides the scope;
/// the frontend presents it. For `cameraMedia` the pinned images are shown
/// `kept` (the `unpinned` scope); the `all` scope prunes them too.
pub fn preview(
    storage: &Storage,
    tier: ResetTier,
    credentials: &CredentialStore,
    slicer_cache: &Path,
) -> Result<ResetPreview, CommandError> {
    use ResetClass as C;
    use ResetEffect as E;
    let facts = storage
        .read_transaction(|tx| facts(tx))
        .map_err(|_| CommandError::persistence_unavailable())?;
    let paths = storage.paths();
    let mut warnings = Vec::new();
    let classes = match tier {
        ResetTier::Settings => vec![
            rows(C::Settings, E::Reset, facts.settings),
            rows(C::SlicerRuntime, E::Kept, facts.slicer_runtime),
            rows(C::PrinterAlertDefaults, E::Kept, facts.alert_defaults),
        ],
        ResetTier::CameraMedia => vec![
            files(C::UnpinnedSnapshots, E::Pruned, facts.unpinned),
            files(C::PinnedSnapshots, E::Kept, facts.pinned),
            rows(C::SnapshotRecords, E::Kept, facts.snapshot_rows),
        ],
        ResetTier::Farm => {
            if facts.active_jobs + facts.host_operations + facts.slice_operations > 0 {
                warnings.push(ResetWarning::ActiveWork {
                    active_jobs: facts.active_jobs,
                    host_operations: facts.host_operations,
                    slice_operations: facts.slice_operations,
                });
            }
            if credentials.unavailable_reason_code().is_some() {
                warnings.push(ResetWarning::CredentialStoreUnavailable);
            }
            let staging_prefix = |name: &str| name.starts_with("restore-");
            let restore_staging = {
                let (a, b) = tree_usage(&paths.snapshot_root().join(STAGING_DIRECTORY));
                let (c, d) = entries_usage(&paths.content_root().join("staging"), staging_prefix);
                let (e, f) = entries_usage(paths.media_root(), staging_prefix);
                (a + c + e, b + d + f)
            };
            vec![
                rows(C::Settings, E::Reset, facts.settings),
                rows(C::SlicerRuntime, E::Deleted, facts.slicer_runtime),
                rows(C::PrinterAlertDefaults, E::Deleted, facts.alert_defaults),
                rows(C::Printers, E::Deleted, facts.printers),
                rows(C::Spools, E::Deleted, facts.spools),
                rows(C::Library, E::Deleted, facts.library),
                rows(C::SliceRevisions, E::Deleted, facts.slice_revisions),
                rows(C::QueueAndJobs, E::Deleted, facts.queue_and_jobs),
                rows(
                    C::IncidentsAndAttention,
                    E::Deleted,
                    facts.incidents_and_attention,
                ),
                rows(C::SnapshotRecords, E::Deleted, facts.snapshot_rows),
                files(
                    C::Content,
                    E::Deleted,
                    tree_usage(&paths.content_root().join("blobs")),
                ),
                files(
                    C::CameraMedia,
                    E::Deleted,
                    tree_usage(&paths.media_root().join("snapshots")),
                ),
                files(C::Logs, E::Deleted, tree_usage(paths.log_root())),
                rows(C::Credentials, E::Deleted, facts.credential_refs),
                files(
                    C::PreImportSnapshots,
                    E::Deleted,
                    entries_usage(paths.snapshot_root(), |name| name != STAGING_DIRECTORY),
                ),
                files(C::RestoreStaging, E::Deleted, restore_staging),
                files(
                    C::LegacyArchives,
                    E::Deleted,
                    tree_usage(paths.legacy_root()),
                ),
                files(
                    C::SafetyBackups,
                    E::Kept,
                    tree_usage(&paths.backup_root().join("safety")),
                ),
                files(C::SlicerProfileCache, E::Kept, tree_usage(slicer_cache)),
            ]
        }
    };
    Ok(ResetPreview {
        tier,
        phrase: tier.phrase().to_string(),
        classes,
        warnings,
    })
}
