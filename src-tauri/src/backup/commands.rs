//! The backup commands (spec "Commands"): `backup_inventory`,
//! `create_backup`, `list_backups`, `delete_backup`, `preview_restore`, and
//! `discard_restore_preview`, plus `apply_restore`, `restore_status`, and
//! `acknowledge_restore_status` (D8).
//!
//! `create_backup` and `delete_backup` hold the lease for their whole run
//! (`BACKUP_IN_PROGRESS` names another holder) and claim their
//! `operationId` in the process-local ledger (D18). `create_backup` owns
//! its native save dialog: the frontend never passes a path and the
//! result carries only the destination's basename.

use std::sync::Arc;

use chrono::Utc;
use serde::Serialize;
use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::{RepositoryError, StorageError};
use crate::RuntimeServices;

use super::dialogs::BACKUP_EXTENSION;
use super::lease::{LeaseActivity, LeaseGuard};
use super::process_ops::ProcessOperationKind;
use super::writer::{write_backup, BackupRequest};
use super::{
    apply, inventory, journal, preview, restart, safety, staging, ApplyRestoreOutcome,
    BackupInventory, BackupMediaChoice, BackupOrigin, BackupSummary, CreateBackupOutcome,
    DeleteBackupOutcome, DiscardRestorePreviewOutcome, PreviewRestoreOutcome, RestartingStatus,
    RestorePreviewSource, RestoreSource, RestoreStatus,
};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

fn ready<R: tauri::Runtime>(
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<RuntimeServices<R>>, CommandError> {
    contract_version.validate()?;
    bootstrap.ready()
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

fn lease<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    activity: LeaseActivity,
) -> Result<LeaseGuard, CommandError> {
    services
        .backup
        .lease
        .try_acquire(activity)
        .map_err(|holder| CommandError::backup_in_progress(holder.as_str()))
}

/// Runs blocking file work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| CommandError::internal())?
}

/// `backup_inventory`: what a backup of the live Farm would hold, read in
/// one transaction.
#[tauri::command]
pub async fn backup_inventory<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<BackupInventory>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    services
        .storage
        .read_transaction(|tx| inventory::live_inventory(tx))
        .map(CommandSuccess::new)
        .map_err(storage_error)
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CreateBackupDigest {
    media: BackupMediaChoice,
}

/// `create_backup`: asks for a destination, then writes a backup there
/// under the lease. A closed dialog is `cancelled` with no side effect.
#[tauri::command]
pub async fn create_backup<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    media: BackupMediaChoice,
) -> Result<CommandSuccess<CreateBackupOutcome>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = CreateBackupDigest { media };
    let ledger = &services.backup.operations;
    if let Some(cached) =
        ledger.replay(&operation_id, ProcessOperationKind::CreateBackup, &digest)?
    {
        return Ok(CommandSuccess::new(cached));
    }
    let app_version = app.package_info().version.to_string();
    let outcome = {
        let services = Arc::clone(&services);
        blocking(move || run_create_backup(&services, media, app_version)).await?
    };
    if matches!(outcome, CreateBackupOutcome::Exported { .. }) {
        ledger.record(
            &operation_id,
            ProcessOperationKind::CreateBackup,
            &digest,
            &outcome,
        );
    }
    Ok(CommandSuccess::new(outcome))
}

fn run_create_backup<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    media: BackupMediaChoice,
    app_version: String,
) -> Result<CreateBackupOutcome, CommandError> {
    let guard = lease(services, LeaseActivity::Backup)?;
    // Only the suggested name uses the time the dialog opens; `createdAt`
    // is stamped at the database copy, after the dialog closes.
    let suggested = format!(
        "farm3d-{}.{BACKUP_EXTENSION}",
        Utc::now().format("%Y%m%dT%H%M%SZ")
    );
    let Some(destination) = services.backup.dialogs.save_backup(&suggested)? else {
        return Ok(CreateBackupOutcome::Cancelled);
    };
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| CommandError::validation("Select a file name for the backup."))?;
    let written = write_backup(
        &services.storage,
        &guard,
        &destination,
        &BackupRequest {
            media,
            origin: BackupOrigin::Operator,
            created_at: None,
            app_version,
        },
        &services.backup.writer_hooks,
    )?;
    drop(guard);
    crate::f3d_log!(
        info,
        "backup.created",
        bytes = written.bytes,
        entries = written.manifest.entries.len() as u64,
    );
    Ok(CreateBackupOutcome::Exported {
        exported_at: written.manifest.created_at.clone(),
        file_name,
        bytes: written.bytes,
        media,
        media_not_in_backup: written.manifest.media.not_in_backup,
        media_missing_file: written.manifest.media.missing_file,
    })
}

/// `list_backups`: the safety backups, newest first.
#[tauri::command]
pub async fn list_backups<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<Vec<BackupSummary>>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let summaries =
        blocking(move || safety::list(services.storage.paths()).map_err(storage_error)).await?;
    Ok(CommandSuccess::new(summaries))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeleteBackupDigest<'a> {
    backup_id: &'a str,
}

/// `delete_backup`: deletes one safety backup under the lease. An unknown
/// id is `NOT_FOUND`.
#[tauri::command]
pub async fn delete_backup<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    backup_id: String,
) -> Result<CommandSuccess<DeleteBackupOutcome>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let ledger = &services.backup.operations;
    let digest = DeleteBackupDigest {
        backup_id: &backup_id,
    };
    if let Some(cached) =
        ledger.replay(&operation_id, ProcessOperationKind::DeleteBackup, &digest)?
    {
        return Ok(CommandSuccess::new(cached));
    }
    let deleted = {
        let services = Arc::clone(&services);
        let backup_id = backup_id.clone();
        blocking(move || {
            let _guard = lease(&services, LeaseActivity::BackupDelete)?;
            safety::delete(services.storage.paths(), &backup_id).map_err(storage_error)
        })
        .await?
    };
    if !deleted {
        // Echo the id only when it could name a file (never a path).
        let entity = if safety::is_safe_backup_id(&backup_id) {
            backup_id.as_str()
        } else {
            "backupId"
        };
        return Err(CommandError::not_found(entity));
    }
    let result = DeleteBackupOutcome {
        backup_id: backup_id.clone(),
        deleted: true,
    };
    ledger.record(
        &operation_id,
        ProcessOperationKind::DeleteBackup,
        &digest,
        &result,
    );
    Ok(CommandSuccess::new(result))
}

/// `preview_restore`: stages the chosen backup (a file the native open
/// dialog picks, or a safety backup by id), verifies and migrates it, and
/// previews restoring it. A closed dialog is `cancelled` with no side
/// effect. Replaces any earlier staging.
#[tauri::command]
pub async fn preview_restore<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    source: RestoreSource,
) -> Result<CommandSuccess<PreviewRestoreOutcome>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let outcome = blocking(move || run_preview_restore(&services, source)).await?;
    Ok(CommandSuccess::new(outcome))
}

fn run_preview_restore<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    source: RestoreSource,
) -> Result<PreviewRestoreOutcome, CommandError> {
    let guard = lease(services, LeaseActivity::RestorePreview)?;
    let paths = services.storage.paths();
    let (archive, preview_source) = match source {
        RestoreSource::File => {
            let Some(path) = services.backup.dialogs.open_backup()? else {
                return Ok(PreviewRestoreOutcome::Cancelled);
            };
            let file_name = path
                .file_name()
                .map(|name| name.to_string_lossy().into_owned())
                .ok_or_else(|| CommandError::validation("Select a local farm3d backup."))?;
            (path, RestorePreviewSource::File { file_name })
        }
        RestoreSource::SafetyBackup { backup_id } => {
            let Some(path) = safety::existing_path(paths, &backup_id).map_err(storage_error)?
            else {
                let entity = if safety::is_safe_backup_id(&backup_id) {
                    backup_id.as_str()
                } else {
                    "backupId"
                };
                return Err(CommandError::not_found(entity));
            };
            (path, RestorePreviewSource::SafetyBackup { backup_id })
        }
    };

    // At most one staging per process: the earlier one goes first.
    services.backup.stagings.clear();
    let candidate = staging::stage(paths, &guard, &archive, &services.backup.staging_options)?;
    let preview = match preview::compute(
        &services.storage,
        &candidate,
        preview_source,
        &services.credentials,
    ) {
        Ok(preview) => preview,
        Err(error) => {
            candidate.layout.remove();
            return Err(error.into());
        }
    };
    services.backup.stagings.replace(candidate);
    drop(guard);
    crate::f3d_log!(
        info,
        "restore.previewed",
        conflict_groups = preview.conflicts.len() as u64,
        blockers = preview.blocker_total,
    );
    Ok(PreviewRestoreOutcome::Previewed { preview })
}

/// `discard_restore_preview`: removes the staging `staging_id` (all three
/// directories). `false` when nothing was staged under that id.
#[tauri::command]
pub async fn discard_restore_preview<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    staging_id: String,
) -> Result<CommandSuccess<DiscardRestorePreviewOutcome>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let discarded = blocking(move || Ok(services.backup.stagings.discard(&staging_id))).await?;
    Ok(CommandSuccess::new(DiscardRestorePreviewOutcome {
        discarded,
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ApplyRestoreDigest<'a> {
    staging_id: &'a str,
}

/// `apply_restore` (D8): writes a safety backup and a `pending` journal,
/// keeps the lease, and requests the restart through the injected
/// `Restarter` after the response. The live database is never touched; the
/// startup installer swaps the Farm.
#[tauri::command]
pub async fn apply_restore<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    staging_id: String,
    confirmation: String,
) -> Result<CommandSuccess<ApplyRestoreOutcome>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    // 1. The exact phrase (no trim, no case folding).
    if confirmation != apply::CONFIRMATION {
        return Err(CommandError::confirmation_mismatch(apply::CONFIRMATION));
    }
    // 2. The process-local ledger (D18).
    let digest = ApplyRestoreDigest {
        staging_id: &staging_id,
    };
    let ledger = &services.backup.operations;
    if let Some(cached) =
        ledger.replay(&operation_id, ProcessOperationKind::ApplyRestore, &digest)?
    {
        return Ok(CommandSuccess::new(cached));
    }
    let app_version = app.package_info().version.to_string();
    let result = {
        let services = Arc::clone(&services);
        let staging_id = staging_id.clone();
        blocking(move || run_apply_restore(&services, &staging_id, &app_version)).await?
    };
    ledger.record(
        &operation_id,
        ProcessOperationKind::ApplyRestore,
        &digest,
        &result,
    );
    // 11. After the response reaches the frontend.
    restart::request_restart(Arc::clone(&services.backup.restarter));
    Ok(CommandSuccess::new(result))
}

/// Puts the staging back if `apply_restore` stops before its journal names
/// it.
struct TakenStaging<'a> {
    stagings: &'a staging::Stagings,
    candidate: Option<staging::StagedCandidate>,
}

impl Drop for TakenStaging<'_> {
    fn drop(&mut self) {
        if let Some(candidate) = self.candidate.take() {
            self.stagings.put_back(candidate);
        }
    }
}

fn run_apply_restore<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    staging_id: &str,
    app_version: &str,
) -> Result<ApplyRestoreOutcome, CommandError> {
    let paths = services.storage.paths();
    let now = Utc::now();
    // 6, first: a restore or reset waiting for the restart refuses before
    // the lease, which that operation still holds (fault row f25).
    apply::refuse_if_restart_pending(paths)?;
    // 3. The staging must exist and not be expired.
    services.backup.stagings.resolve(staging_id, now)?;
    // 4. The lease.
    let guard = lease(services, LeaseActivity::RestoreApply)?;
    // Out of `Stagings`: once the journal names it, it can't be discarded
    // or replaced.
    let mut taken = TakenStaging {
        stagings: &services.backup.stagings,
        candidate: Some(services.backup.stagings.take(staging_id, now)?),
    };
    // 5. The blockers, again.
    let (blockers, blocker_total) = services
        .storage
        .read_transaction(|tx| preview::blockers(tx))
        .map_err(storage_error)?;
    if blocker_total > 0 {
        return Err(CommandError::restore_blocked(&blockers, blocker_total));
    }
    // 6. A finished journal is acknowledged.
    apply::acknowledge_finished(paths)?;
    // 7. The safety backup; any failure stops here.
    let safety = safety::write_safety_backup(
        &services.storage,
        &guard,
        BackupOrigin::BeforeRestore,
        now,
        app_version,
        &services.backup.writer_hooks,
    )
    .map_err(|error| error.into_command_error("safetyBackup"))?;
    // 8–10. The carry, the expected counts, and the journal.
    let candidate = taken.candidate.as_ref().expect("taken above");
    let written =
        apply::write_pending_journal(&services.storage, candidate, &safety.backup_id, now)?;
    taken.candidate = None;
    // 11. Keep the lease until the restart.
    *services
        .backup
        .held_until_restart
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(guard);
    crate::f3d_log!(
        info,
        "restore.journalWritten",
        expected_tables = written
            .expected_counts
            .as_ref()
            .map_or(0, |counts| counts.len()) as u64,
        orphan_refs = written.orphan_credential_refs.len() as u64,
    );
    Ok(ApplyRestoreOutcome {
        status: RestartingStatus::Restarting,
        safety_backup_id: safety.backup_id,
    })
}

fn journal_error(_: journal::JournalError) -> CommandError {
    CommandError::persistence_unavailable()
}

/// `restore_status`: the finished journal of the last restore or reset,
/// until it is acknowledged; `none` otherwise.
#[tauri::command]
pub async fn restore_status<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<RestoreStatus>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let status = blocking(move || {
        Ok(journal::read(services.storage.paths())
            .map_err(journal_error)?
            .map_or(RestoreStatus::None, |current| current.status()))
    })
    .await?;
    Ok(CommandSuccess::new(status))
}

/// `acknowledge_restore_status`: deletes the finished journal `journal_id`
/// and its directory. Any other id is `NOT_FOUND`.
#[tauri::command]
pub async fn acknowledge_restore_status<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    journal_id: String,
) -> Result<CommandSuccess<RestoreStatus>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let status = blocking(move || {
        let paths = services.storage.paths();
        match journal::read(paths).map_err(journal_error)? {
            Some(current) if current.id == journal_id && current.phase.is_finished() => {
                journal::remove(paths, &current.id)
                    .map_err(|_| CommandError::persistence_unavailable())?;
                Ok(RestoreStatus::None)
            }
            _ => {
                let entity = if journal::is_journal_id(&journal_id) {
                    journal_id.as_str()
                } else {
                    "journalId"
                };
                Err(CommandError::not_found(entity))
            }
        }
    })
    .await?;
    Ok(CommandSuccess::new(status))
}
