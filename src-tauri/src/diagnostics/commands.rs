//! The diagnostics, storage, reset, and About commands (spec "Commands",
//! D13, D14, D15, D18): `diagnostics_preview`, `export_diagnostics`,
//! `storage_usage`, `clear_storage`, `reset_preview`, `reset_farm`, and
//! `about_farm3d`.
//!
//! `clear_storage` holds the backup lease (activity `storageCleanup`) for
//! its whole run and claims its `operationId` in the process-local ledger
//! (digest `{ target }`).
//!
//! `export_diagnostics` owns its native save dialog (the frontend never
//! passes a path; the result carries the basename), builds the bundle in
//! memory, and writes it with `document_io::atomic_write` only after the
//! egress scan passes. It claims its `operationId` in the process-local
//! ledger (D18, digest `{ sections }` sorted).
//!
//! `reset_farm` checks the exact phrase first, then, for every tier, the
//! journal (`RESTART_PENDING` while a restore or reset waits for the
//! restart) before it takes the lease (`BACKUP_IN_PROGRESS`, activity
//! `reset`) and before any pre-reset snapshot. Every tier holds the lease
//! for its whole run; tier (c) keeps it until the process exits for the
//! restart. Tiers (a) and (b) claim their `operationId` in the database;
//! tier (c) uses the process-local ledger.

use std::sync::Arc;

use std::collections::BTreeSet;

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::backup::lease::{LeaseActivity, LeaseGuard};
use crate::backup::process_ops::ProcessOperationKind;
use crate::backup::{apply, restart, safety, BackupOrigin, RestartingStatus};
use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::RuntimeServices;

use super::about::{self, AboutInfo};
use super::bundle::{
    self, BundleError, BundleInputs, DiagnosticsPreview, DiagnosticsSection,
    ExportDiagnosticsOutcome,
};
use super::collect::CollectContext;
use super::reset::{self, ResetPreview, ResetRequest, ResetResult, ResetTier};
use super::storage::{self, ClearStorageOutcome, StorageCleanupTarget, StorageUsage};
use crate::persistence::integrity::IntegrityRoots;
use crate::persistence::RepositoryError;
use crate::printers::StoredPrinter;
use crate::slicing::runtime::{EngineState, PresetSourceState};

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

// --- diagnostics (D13) and About --------------------------------------------------------

/// Builds the bundle inputs from the live services and runs `work` on them.
fn with_bundle_inputs<R: tauri::Runtime, T>(
    services: &RuntimeServices<R>,
    app_version: &str,
    now: DateTime<Utc>,
    work: impl FnOnce(&BundleInputs<'_>) -> T,
) -> T {
    let slicer_status = services.slicing.runtime_status().ok();
    let about = about::about(
        app_version,
        &services.catalog,
        slicer_status.as_ref(),
        &services.credentials,
    );
    let statuses = services.manager.statuses();
    let ids: Vec<String> = statuses.keys().cloned().collect();
    let error_causes = services.manager.error_causes(&ids);
    let host_ops = Arc::clone(&services.host_ops);
    let capabilities = move |printer: &StoredPrinter| host_ops.capabilities(printer);
    // Paths farm3d knows only at runtime: the profile cache and whatever
    // engine and preset source discovery resolved.
    let mut runtime_paths = vec![services.slicing.cache_dir().to_path_buf()];
    if let Some(status) = &slicer_status {
        if let EngineState::Available { path, .. } = &status.engine {
            runtime_paths.push(path.into());
        }
        if let PresetSourceState::Available { path, .. } = &status.preset_source {
            runtime_paths.push(path.into());
        }
        runtime_paths.extend(
            status
                .engine_candidates
                .iter()
                .map(|candidate| candidate.path.clone().into()),
        );
    }
    let mut home_dirs = services.diagnostics.home_dirs();
    home_dirs.extend(services.slicing.discovery_env().home);
    let paths = services.storage.paths();
    let inputs = BundleInputs {
        storage: &services.storage,
        credentials: &services.credentials,
        context: CollectContext {
            about,
            catalog: &services.catalog,
            statuses,
            error_causes,
            capabilities: &capabilities,
            integrity_roots: IntegrityRoots::from_paths(paths),
            log_root: paths.log_root(),
            storage_paths: paths,
            slicer_cache: services.slicing.cache_dir(),
            now,
        },
        home_dirs,
        runtime_paths,
        app_version,
        created_at: now,
        entry_hook: services.diagnostics.entry_hook(),
    };
    work(&inputs)
}

fn bundle_error(error: BundleError) -> CommandError {
    match error {
        BundleError::RedactionFailed(section) => {
            crate::f3d_log!(warn, "diagnostics.redactionFailed", section = section);
            CommandError::diagnostics_redaction_failed(section)
        }
        BundleError::Repository(error) => CommandError::from_repository(error),
        BundleError::Internal => CommandError::internal(),
    }
}

/// `diagnostics_preview`: every section with its estimated size. Reads
/// only; opens no dialog and writes nothing.
#[tauri::command]
pub async fn diagnostics_preview<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<DiagnosticsPreview>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let app_version = app.package_info().version.to_string();
    let sections = blocking(move || {
        with_bundle_inputs(&services, &app_version, Utc::now(), bundle::preview)
            .map_err(bundle_error)
    })
    .await?;
    Ok(CommandSuccess::new(DiagnosticsPreview { sections }))
}

/// The process-local ledger digest, `{ sections }` sorted.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ExportDiagnosticsDigest {
    sections: Vec<DiagnosticsSection>,
}

/// `export_diagnostics`: asks for a destination, builds the bundle of
/// `sections` (1–6, distinct), and writes it only if the egress scan
/// passes. A closed dialog is `cancelled` with no side effect.
#[tauri::command]
pub async fn export_diagnostics<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    sections: Vec<DiagnosticsSection>,
) -> Result<CommandSuccess<ExportDiagnosticsOutcome>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let selected: BTreeSet<DiagnosticsSection> = sections.iter().copied().collect();
    if sections.is_empty()
        || sections.len() > DiagnosticsSection::ALL.len()
        || selected.len() != sections.len()
    {
        return Err(CommandError::from_repository(RepositoryError::Validation {
            field_path: "sections",
        }));
    }
    let digest = ExportDiagnosticsDigest {
        sections: selected.iter().copied().collect(),
    };
    let ledger = &services.backup.operations;
    if let Some(cached) = ledger.replay(
        &operation_id,
        ProcessOperationKind::ExportDiagnostics,
        &digest,
    )? {
        return Ok(CommandSuccess::new(cached));
    }
    let app_version = app.package_info().version.to_string();
    let outcome = {
        let services = Arc::clone(&services);
        blocking(move || run_export(&services, &selected, &app_version)).await?
    };
    if matches!(outcome, ExportDiagnosticsOutcome::Exported { .. }) {
        ledger.record(
            &operation_id,
            ProcessOperationKind::ExportDiagnostics,
            &digest,
            &outcome,
        );
    }
    Ok(CommandSuccess::new(outcome))
}

fn run_export<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    sections: &BTreeSet<DiagnosticsSection>,
    app_version: &str,
) -> Result<ExportDiagnosticsOutcome, CommandError> {
    let suggested = bundle::suggested_file_name(Utc::now());
    let Some(destination) = services.backup.dialogs.save_diagnostics(&suggested)? else {
        return Ok(ExportDiagnosticsOutcome::Cancelled);
    };
    let file_name = destination
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .ok_or_else(|| CommandError::validation("Select a file name for the diagnostics."))?;
    // Stamped after the dialog closes, at the collection.
    let created_at = Utc::now();
    let bytes = with_bundle_inputs(services, app_version, created_at, |inputs| {
        bundle::build(inputs, sections)
    })
    .map_err(bundle_error)?;
    crate::document_io::atomic_write(&destination, &bytes)?;
    crate::f3d_log!(
        info,
        "diagnostics.exported",
        bytes = bytes.len() as u64,
        sections = sections.len() as u64,
    );
    Ok(ExportDiagnosticsOutcome::Exported {
        exported_at: created_at.to_rfc3339_opts(SecondsFormat::Millis, true),
        file_name,
        bytes: bytes.len() as u64,
        sections: sections.iter().copied().collect(),
    })
}

/// `about_farm3d`: the version (from `app.package_info()`), the schema and
/// backup format versions, the platform, the catalog, the OrcaSlicer
/// version (never its path), and the credential tier.
#[tauri::command]
pub async fn about_farm3d<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<AboutInfo>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let app_version = app.package_info().version.to_string();
    let info = blocking(move || {
        let slicer_status = services.slicing.runtime_status().ok();
        Ok(about::about(
            &app_version,
            &services.catalog,
            slicer_status.as_ref(),
            &services.credentials,
        ))
    })
    .await?;
    Ok(CommandSuccess::new(info))
}

// --- storage (D14) ---------------------------------------------------------------------

/// `storage_usage`: every class's bytes and count. Reads only.
#[tauri::command]
pub async fn storage_usage<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<StorageUsage>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let usage = blocking(move || {
        storage::usage(&services.storage, services.slicing.cache_dir(), Utc::now())
            .map_err(|error| CommandError::from_repository(RepositoryError::Storage(error)))
    })
    .await?;
    Ok(CommandSuccess::new(usage))
}

/// The process-local ledger digest, `{ target }`.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ClearStorageDigest {
    target: StorageCleanupTarget,
}

/// `clear_storage`: one cleanup target, under the lease. Returns what it
/// freed and the new usage.
#[tauri::command]
pub async fn clear_storage<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    target: StorageCleanupTarget,
) -> Result<CommandSuccess<ClearStorageOutcome>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = ClearStorageDigest { target };
    let ledger = &services.backup.operations;
    if let Some(cached) =
        ledger.replay(&operation_id, ProcessOperationKind::ClearStorage, &digest)?
    {
        return Ok(CommandSuccess::new(cached));
    }
    let result = {
        let services = Arc::clone(&services);
        blocking(move || run_clear_storage(&services, target)).await?
    };
    ledger.record(
        &operation_id,
        ProcessOperationKind::ClearStorage,
        &digest,
        &result,
    );
    Ok(CommandSuccess::new(result))
}

fn run_clear_storage<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    target: StorageCleanupTarget,
) -> Result<ClearStorageOutcome, CommandError> {
    let guard = services
        .backup
        .lease
        .try_acquire(LeaseActivity::StorageCleanup)
        .map_err(|holder| CommandError::backup_in_progress(holder.as_str()))?;
    let cleared = storage::clear(
        &services.storage,
        &services.library.content,
        services.slicing.cache_dir(),
        &guard,
        target,
    )?;
    let usage = storage::usage(&services.storage, services.slicing.cache_dir(), Utc::now())
        .map_err(|error| CommandError::from_repository(RepositoryError::Storage(error)))?;
    drop(guard);
    Ok(ClearStorageOutcome {
        target,
        removed_count: cleared.removed_count,
        freed_bytes: cleared.freed_bytes,
        usage,
    })
}
