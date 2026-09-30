//! The reset commands (spec "Commands", D15, D18): `reset_preview` and
//! `reset_farm`.
//!
//! `reset_farm` checks the exact phrase first, then, for every tier, the
//! journal (`RESTART_PENDING` while a restore or reset waits for the
//! restart) before it takes the lease (`BACKUP_IN_PROGRESS`, activity
//! `reset`) and before any pre-reset snapshot. Every tier holds the lease
//! for its whole run; tier (c) keeps it until the process exits for the
//! restart. Tiers (a) and (b) claim their `operationId` in the database;
//! tier (c) uses the process-local ledger.

use std::sync::Arc;

use chrono::Utc;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::backup::lease::{LeaseActivity, LeaseGuard};
use crate::backup::process_ops::ProcessOperationKind;
use crate::backup::{apply, restart, safety, BackupOrigin, RestartingStatus};
use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::RuntimeServices;

use super::reset::{self, ResetPreview, ResetRequest, ResetResult, ResetTier};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

fn ready<R: tauri::Runtime>(
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<RuntimeServices<R>>, CommandError> {
    contract_version.validate()?;
    bootstrap.ready()
}

/// Runs blocking file and database work off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| CommandError::internal())?
}

/// `RESTART_PENDING` first, then the lease (activity `reset`).
fn begin<R: tauri::Runtime>(services: &RuntimeServices<R>) -> Result<LeaseGuard, CommandError> {
    apply::refuse_if_restart_pending(services.storage.paths())?;
    services
        .backup
        .lease
        .try_acquire(LeaseActivity::Reset)
        .map_err(|holder| CommandError::backup_in_progress(holder.as_str()))
}

/// `reset_preview`: every class the tier affects or keeps, with counts and
/// bytes, and the warnings. Reads only.
#[tauri::command]
pub async fn reset_preview<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    tier: ResetTier,
) -> Result<CommandSuccess<ResetPreview>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let preview = blocking(move || {
        reset::preview(
            &services.storage,
            tier,
            &services.credentials,
            services.slicing.cache_dir(),
        )
    })
    .await?;
    Ok(CommandSuccess::new(preview))
}

/// Tier (c)'s process-local ledger digest, `{ request }`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct FarmDigest {
    request: ResetRequest,
}

/// What a tier (c) replay needs from the recorded result.
#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct FarmReplay {
    safety_backup_id: Option<String>,
}

/// `reset_farm`: one of the three tiers, behind its exact phrase.
#[tauri::command]
pub async fn reset_farm<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    request: ResetRequest,
    confirmation: String,
) -> Result<CommandSuccess<ResetResult>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let phrase = request.tier().phrase();
    if confirmation != phrase {
        return Err(CommandError::confirmation_mismatch(phrase));
    }
    let result = match request {
        ResetRequest::Settings { expected_revision } => {
            let services = Arc::clone(&services);
            blocking(move || reset_settings(&services, &operation_id, expected_revision)).await?
        }
        ResetRequest::CameraMedia { scope } => {
            let guard = {
                let services = Arc::clone(&services);
                blocking(move || begin(&services)).await?
            };
            // P8's prune order: under the janitor lock, one transaction,
            // then the files, queued until the lease drops.
            let serialized = services.cameras.janitor().lock().await;
            let pruned = {
                let services = Arc::clone(&services);
                blocking(move || {
                    reset::reset_camera_media(&services.storage, &operation_id, scope, Utc::now())
                        .map_err(CommandError::from_repository)
                })
                .await?
            };
            crate::cameras::media::queue_unlinks(
                &services.storage,
                services.cameras.janitor(),
                &pruned.rel_paths,
            );
            drop(serialized);
            // Dropping the lease unlinks the queued files.
            drop(guard);
            crate::cameras::capture::publish(&services, &app, &pruned.changes);
            crate::f3d_log!(
                info,
                "reset.cameraMedia",
                pruned = pruned.pruned_count as u64,
            );
            ResetResult::CameraMedia {
                pruned_count: pruned.pruned_count,
                freed_bytes: pruned.freed_bytes,
            }
        }
        ResetRequest::Farm { .. } => {
            let digest = FarmDigest { request };
            let ledger = &services.backup.operations;
            if let Some(cached) = ledger.replay::<FarmReplay>(
                &operation_id,
                ProcessOperationKind::ResetFarm,
                &digest,
            )? {
                return Ok(CommandSuccess::new(ResetResult::Farm {
                    status: RestartingStatus::Restarting,
                    safety_backup_id: cached.safety_backup_id,
                }));
            }
            let app_version = app.package_info().version.to_string();
            let result = {
                let services = Arc::clone(&services);
                blocking(move || run_farm_reset(&services, request, &app_version)).await?
            };
            ledger.record(
                &operation_id,
                ProcessOperationKind::ResetFarm,
                &digest,
                &result,
            );
            // After the response reaches the frontend.
            restart::request_restart(Arc::clone(&services.backup.restarter));
            result
        }
    };
    Ok(CommandSuccess::new(result))
}

/// Tier (a) under the lease.
fn reset_settings<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    operation_id: &str,
    expected_revision: i64,
) -> Result<ResetResult, CommandError> {
    let guard = begin(services)?;
    let saved = reset::reset_settings(&services.storage, operation_id, expected_revision)
        .map_err(CommandError::from_repository)?;
    drop(guard);
    if saved.retention_changed {
        services.cameras.janitor().poke();
    }
    crate::f3d_log!(info, "reset.settings");
    Ok(ResetResult::Settings {
        settings: saved.record.into(),
    })
}

/// Tier (c): the optional safety backup first, then the `pending` reset
/// journal; the lease is kept until the restart.
fn run_farm_reset<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    request: ResetRequest,
    app_version: &str,
) -> Result<ResetResult, CommandError> {
    let ResetRequest::Farm {
        safety_backup,
        delete_safety_backups,
    } = request
    else {
        return Err(CommandError::internal());
    };
    let now = Utc::now();
    let paths = services.storage.paths();
    let guard = begin(services)?;
    // A finished journal is acknowledged before a new one is written.
    apply::acknowledge_finished(paths)?;
    let safety_backup_id = if safety_backup {
        let written = safety::write_safety_backup(
            &services.storage,
            &guard,
            BackupOrigin::BeforeReset,
            now,
            app_version,
            &services.backup.writer_hooks,
        )
        .map_err(|error| error.into_command_error("safetyBackup"))?;
        Some(written.backup_id)
    } else {
        None
    };
    let written = reset::write_farm_journal(
        &services.storage,
        safety_backup_id.clone(),
        delete_safety_backups,
        now,
    )?;
    // Keep the lease until the restart.
    *services
        .backup
        .held_until_restart
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(guard);
    crate::f3d_log!(
        info,
        "reset.journalWritten",
        credential_refs = written.orphan_credential_refs.len() as u64,
        safety_backup = safety_backup_id.is_some(),
        delete_safety_backups = delete_safety_backups,
    );
    Ok(ResetResult::Farm {
        status: RestartingStatus::Restarting,
        safety_backup_id,
    })
}
