//! The backup commands (spec "Commands"): `backup_inventory`,
//! `create_backup`, `list_backups`, and `delete_backup`. Restore staging,
//! apply, and status are Tasks 6 and 7.
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
    inventory, safety, BackupInventory, BackupMediaChoice, BackupOrigin, BackupSummary,
    CreateBackupOutcome, DeleteBackupOutcome,
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
