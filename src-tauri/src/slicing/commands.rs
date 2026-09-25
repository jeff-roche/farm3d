//! P5's Tauri commands (spec §Commands). Each validates the contract
//! version and waits for bootstrap before any side effect. None accepts a
//! filesystem path: the engine and preset source arrive only through the
//! Rust-owned pickers (D2). Work that probes OrcaSlicer, reads geometry, or
//! builds the preset index runs on a blocking thread.
//!
//! Events go out after commit only (D17); a command attaches the app handle
//! to the slicing services so the scheduler can emit too.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::ipc::Response;
use tauri::AppHandle;
use ts_rs::TS;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::contracts::event::JsSafeInteger;
use crate::library::content::ContentError;
use crate::persistence::{RepositoryError, StorageError};
use crate::RuntimeServices;

use super::blockers::slice_revision_blocker_sources;
use super::events;
use super::external::{self, CreateExternalSliceRevisionFacts};
use super::operations::{self, StartSliceRequest, RECENT_OPERATIONS};
use super::preparation::{self, ReloadPreparationData};
use super::presets::list_slice_options as build_slice_options;
use super::repository;
use super::runtime::{self as slicer_runtime, PresetSourcePickKind, RuntimePick};
use super::{
    PreparationDocument, PreparationRecord, RevisionGeometry, SliceOperationRecord, SliceOptions,
    SliceRevisionRecord, SliceRevisionSummary, SliceTarget, SlicerRuntimeStatus,
};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// Runs blocking `work` off the async runtime.
async fn blocking<T: Send + 'static>(
    work: impl FnOnce() -> Result<T, CommandError> + Send + 'static,
) -> Result<T, CommandError> {
    tauri::async_runtime::spawn_blocking(work)
        .await
        .map_err(|_| CommandError::internal())?
}

fn ready<R: tauri::Runtime>(
    app: &AppHandle<R>,
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<RuntimeServices<R>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    services.slicing.attach(app);
    Ok(services)
}

// ---------------------------------------------------------------------------
// Wire types
// ---------------------------------------------------------------------------

/// D17's backfill.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SlicingSnapshot.ts")]
pub struct SlicingSnapshot {
    pub stream_id: String,
    pub snapshot_sequence: JsSafeInteger,
    pub runtime: SlicerRuntimeStatus,
    pub preparations: Vec<PreparationRecord>,
    /// Every queued or running operation, then the 50 most recently
    /// finished.
    pub active_and_recent_operations: Vec<SliceOperationRecord>,
    pub revisions: Vec<SliceRevisionSummary>,
}

/// D2 `pick_preset_source`'s `kind`: a file (an executable or AppImage) or
/// a folder (an install or resources directory).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/PresetSourceKind.ts")]
pub enum PresetSourceKind {
    File,
    Folder,
}

/// `start_slice`'s result. Named `StartSliceData` because
/// `StartSliceResult` is the command's `CommandSuccess` wrapper.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/StartSliceData.ts")]
pub struct StartSliceData {
    pub operations: Vec<SliceOperationRecord>,
}

/// `get_slice_operation_log`: the stored log (D9, D13), whether its middle
/// was dropped, and the 1-based numbers of its known-noise lines (D9's
/// "unable to open display"), which the UI dims.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/SliceOperationLog.ts")]
pub struct SliceOperationLog {
    pub text: String,
    pub truncated: bool,
    pub noise_lines: Vec<u32>,
}

/// `get_slice_revision_log`: a Slice Revision's own stored log (D13, D21).
/// `log` is `null` for an external revision, which farm3d never sliced and
/// so has no log.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/SliceRevisionLog.ts")]
pub struct SliceRevisionLog {
    pub log: Option<SliceOperationLog>,
}

/// `{}` for `delete_preparation` and `delete_slice_revision`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[ts(export_to = "command/SlicingDeleted.ts")]
pub struct SlicingDeleted {}

// ---------------------------------------------------------------------------
// D2 and D22: the runtime
// ---------------------------------------------------------------------------

/// D2: the last resolved runtime status (resolved now if there is none).
#[tauri::command]
pub async fn get_slicer_runtime<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<SlicerRuntimeStatus>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || services.slicing.runtime_status())
        .await
        .map(CommandSuccess::new)
}

/// D22 **Check again**: re-probes, ignoring the 60 s cache.
#[tauri::command]
pub async fn check_slicer_runtime<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<SlicerRuntimeStatus>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        services
            .slicing
            .resolve_runtime(true)
            .map(|runtime| runtime.status)
    })
    .await
    .map(CommandSuccess::new)
}

/// The status after a configuration change, or `null` for a cancelled
/// picker.
fn after_pick<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    pick: RuntimePick,
) -> Result<Option<SlicerRuntimeStatus>, CommandError> {
    match pick {
        RuntimePick::Cancelled => Ok(None),
        RuntimePick::Saved(_) => services
            .slicing
            .resolve_runtime(true)
            .map(|runtime| Some(runtime.status)),
    }
}

/// D2 **Choose engine…**: the native picker, then a probe; only a
/// supported OrcaSlicer is saved. `null` when the picker is cancelled.
#[tauri::command]
pub async fn pick_slicer_engine<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
) -> Result<CommandSuccess<Option<SlicerRuntimeStatus>>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        let io = services.slicing.file_io();
        let env = services.slicing.discovery_env();
        let pick = slicer_runtime::pick_slicer_engine(
            io.as_ref(),
            &services.storage,
            expected_revision,
            &env,
        )?;
        after_pick(&services, pick)
    })
    .await
    .map(CommandSuccess::new)
}

/// D2 **Choose preset source…**: a file or a folder, resolved before it is
/// saved. `null` when the picker is cancelled.
#[tauri::command]
pub async fn pick_preset_source<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    kind: PresetSourceKind,
) -> Result<CommandSuccess<Option<SlicerRuntimeStatus>>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        let io = services.slicing.file_io();
        let env = services.slicing.discovery_env();
        let kind = match kind {
            PresetSourceKind::File => PresetSourcePickKind::File,
            PresetSourceKind::Folder => PresetSourcePickKind::Folder,
        };
        let pick = slicer_runtime::pick_preset_source(
            io.as_ref(),
            &services.storage,
            expected_revision,
            kind,
            &env,
            services.slicing.cache_dir(),
            &services.slicing.caches,
        )?;
        after_pick(&services, pick)
    })
    .await
    .map(CommandSuccess::new)
}

/// D2 **Use automatic discovery** / **Use the engine's presets**.
#[tauri::command]
pub async fn reset_slicer_runtime<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    engine: bool,
    preset_source: bool,
) -> Result<CommandSuccess<SlicerRuntimeStatus>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        slicer_runtime::reset_slicer_runtime(
            &services.storage,
            expected_revision,
            engine,
            preset_source,
        )?;
        services
            .slicing
            .resolve_runtime(true)
            .map(|runtime| runtime.status)
    })
    .await
    .map(CommandSuccess::new)
}

/// D3: the presets offered for `target`, the defaults, the profile
/// snapshot, and the matching Printers.
#[tauri::command]
pub async fn list_slice_options<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    target: SliceTarget,
) -> Result<CommandSuccess<SliceOptions>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        let (_, source) = services.slicing.usable_runtime()?;
        let index = services.slicing.preset_index(&source)?;
        build_slice_options(&services.storage, &services.catalog, &index, &target)
    })
    .await
    .map(CommandSuccess::new)
}

// ---------------------------------------------------------------------------
// D6: geometry
// ---------------------------------------------------------------------------

/// D6: a Model Source Revision's objects, lay-flat faces, and build items.
#[tauri::command]
pub async fn get_revision_geometry<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    revision_id: String,
) -> Result<CommandSuccess<RevisionGeometry>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        services
            .slicing
            .revision_geometry(&revision_id)
            .map(|revision| revision.geometry().clone())
    })
    .await
    .map(CommandSuccess::new)
}

/// D6: one object's mesh as the binary `F3DM` buffer, returned raw (not in
/// the JSON envelope). An unknown object is `NOT_FOUND`.
#[tauri::command]
pub async fn get_revision_mesh<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    revision_id: String,
    object_key: u32,
) -> Result<Response, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    let buffer = blocking(move || {
        services
            .slicing
            .revision_geometry(&revision_id)?
            .mesh_buffer(object_key)
            .ok_or_else(|| CommandError::not_found(format!("{revision_id}/{object_key}")))
    })
    .await?;
    Ok(Response::new(buffer))
}

// ---------------------------------------------------------------------------
// D17: the backfill
// ---------------------------------------------------------------------------

/// D17: the backfill. The sequence is read BEFORE the rows, so a change
/// this snapshot misses always has a larger sequence.
#[tauri::command]
pub async fn list_slicing<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<SlicingSnapshot>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        let slicing = &services.slicing;
        let snapshot_sequence = slicing.stream.snapshot_sequence();
        let (preparations, operations, revisions) = services
            .storage
            .read(|connection| {
                let wrap = |error: StorageError| {
                    rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                        error.to_string(),
                    )))
                };
                Ok((
                    repository::list_preparations(connection).map_err(wrap)?,
                    repository::list_active_and_recent_operations(connection, RECENT_OPERATIONS)
                        .map_err(wrap)?,
                    repository::list_all_revision_summaries(connection).map_err(wrap)?,
                ))
            })
            .map_err(storage_error)?;
        // The status after the rows: a runtime change emitted meanwhile
        // has a larger sequence and replays over it.
        let runtime = slicing.runtime_status()?;
        Ok(SlicingSnapshot {
            stream_id: slicing.stream.stream_id().to_string(),
            snapshot_sequence,
            runtime,
            preparations,
            active_and_recent_operations: operations,
            revisions,
        })
    })
    .await
    .map(CommandSuccess::new)
}

// ---------------------------------------------------------------------------
// D5: Preparations
// ---------------------------------------------------------------------------

/// D5: the Model's Preparation, seeded from its current revision; an
/// existing one is returned as is. G-code Models are `VALIDATION`.
#[tauri::command]
pub async fn create_preparation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    model_id: String,
    target: Option<SliceTarget>,
) -> Result<CommandSuccess<PreparationRecord>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || preparation::create_preparation(&services.slicing, &model_id, target))
        .await
        .map(CommandSuccess::new)
}

/// D5: replaces the whole document (`CONFLICT` on a stale revision).
#[tauri::command]
pub fn update_preparation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    preparation_id: String,
    expected_revision: i64,
    document: PreparationDocument,
) -> Result<CommandSuccess<PreparationRecord>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    preparation::update_preparation(
        &services.slicing,
        &preparation_id,
        expected_revision,
        &document,
    )
    .map(CommandSuccess::new)
}

/// D5: re-bases the Preparation onto its Model's current revision.
#[tauri::command]
pub async fn reload_preparation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    preparation_id: String,
    expected_revision: i64,
) -> Result<CommandSuccess<ReloadPreparationData>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        preparation::reload_preparation(&services.slicing, &preparation_id, expected_revision)
    })
    .await
    .map(CommandSuccess::new)
}

/// D5: deletes the Preparation and its operations' history.
#[tauri::command]
pub fn delete_preparation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    preparation_id: String,
    expected_revision: i64,
) -> Result<CommandSuccess<SlicingDeleted>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    preparation::delete_preparation(&services.slicing, &preparation_id, expected_revision)?;
    Ok(CommandSuccess::new(SlicingDeleted {}))
}

// ---------------------------------------------------------------------------
// D10: operations
// ---------------------------------------------------------------------------

/// D10: queues one operation per plate. Idempotent by `operationId`.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn start_slice<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    preparation_id: String,
    expected_revision: i64,
    plate_keys: Vec<String>,
    continue_with_source_revision: Option<String>,
) -> Result<CommandSuccess<StartSliceData>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    let request = StartSliceRequest {
        operation_id,
        preparation_id,
        expected_revision,
        plate_keys,
        continue_with_source_revision,
    };
    blocking(move || operations::start_slice(&services.slicing, &request))
        .await
        .map(|operations| CommandSuccess::new(StartSliceData { operations }))
}

/// D9/D10: cancels a queued or running operation.
#[tauri::command]
pub async fn cancel_slice_operation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    slice_operation_id: String,
) -> Result<CommandSuccess<SliceOperationRecord>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || operations::cancel_slice_operation(&services.slicing, &slice_operation_id))
        .await
        .map(CommandSuccess::new)
}

/// D9/D13: the operation's stored log: its revision's `log` blob when it
/// succeeded, else `slice_operations.log_sha256`. Empty while it has none
/// (it never ran). A succeeded operation's log is its revision's, so it
/// goes with a deleted revision: that is `NOT_FOUND`
/// ([`CommandError::slice_log_deleted`]), not an empty log.
#[tauri::command]
pub async fn get_slice_operation_log<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    slice_operation_id: String,
) -> Result<CommandSuccess<SliceOperationLog>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        let (state, sha256) = services
            .storage
            .read(|connection| {
                let state: Option<String> =
                    rusqlite::OptionalExtension::optional(connection.query_row(
                        "SELECT state FROM slice_operations WHERE id = ?1",
                        [&slice_operation_id],
                        |row| row.get(0),
                    ))?;
                let sha256 =
                    operations::log_sha256(connection, &slice_operation_id).map_err(|error| {
                        rusqlite::Error::ToSqlConversionFailure(Box::new(std::io::Error::other(
                            error.to_string(),
                        )))
                    })?;
                Ok((state, sha256))
            })
            .map_err(storage_error)?;
        let Some(state) = state else {
            return Err(CommandError::not_found(slice_operation_id));
        };
        if sha256.is_none() && state == "succeeded" {
            return Err(CommandError::slice_log_deleted(&slice_operation_id));
        }
        let Some(sha256) = sha256 else {
            return Ok(SliceOperationLog {
                text: String::new(),
                truncated: false,
                noise_lines: Vec::new(),
            });
        };
        read_stored_log(&services.library.content, &sha256)
    })
    .await
    .map(CommandSuccess::new)
}

/// Reads a stored log blob, verified, as the UI shows it.
fn read_stored_log(
    content: &crate::library::content::ContentStore,
    sha256: &str,
) -> Result<SliceOperationLog, CommandError> {
    let mut bytes = Vec::new();
    std::io::Read::read_to_end(&mut content.open_verified(sha256)?, &mut bytes)
        .map_err(ContentError::from)?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    Ok(SliceOperationLog {
        truncated: operations::log_was_truncated(&text),
        noise_lines: operations::noise_lines(&text),
        text,
    })
}

/// D21: a Slice Revision's read-only log, by the revision's own id, so it
/// stays readable after its operation leaves the recent list or goes with
/// its Preparation. A farm3d revision reads its `log` blob (an empty log
/// if it has none); an external revision has no log, so `log` is `null`.
/// An unknown revision is `NOT_FOUND`.
#[tauri::command]
pub async fn get_slice_revision_log<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    slice_revision_id: String,
) -> Result<CommandSuccess<SliceRevisionLog>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        let (kind, sha256) = services
            .storage
            .read(|connection| {
                let kind: Option<String> =
                    rusqlite::OptionalExtension::optional(connection.query_row(
                        "SELECT kind FROM slice_revisions WHERE id = ?1",
                        [&slice_revision_id],
                        |row| row.get(0),
                    ))?;
                let sha256: Option<String> =
                    rusqlite::OptionalExtension::optional(connection.query_row(
                        "SELECT sha256 FROM slice_revision_blobs
                         WHERE revision_id = ?1 AND role = 'log'",
                        [&slice_revision_id],
                        |row| row.get(0),
                    ))?;
                Ok((kind, sha256))
            })
            .map_err(storage_error)?;
        let Some(kind) = kind else {
            return Err(CommandError::not_found(slice_revision_id));
        };
        if kind == "external" {
            return Ok(SliceRevisionLog { log: None });
        }
        let log = match sha256 {
            Some(sha256) => read_stored_log(&services.library.content, &sha256)?,
            None => SliceOperationLog {
                text: String::new(),
                truncated: false,
                noise_lines: Vec::new(),
            },
        };
        Ok(SliceRevisionLog { log: Some(log) })
    })
    .await
    .map(CommandSuccess::new)
}

// ---------------------------------------------------------------------------
// Slice Revisions
// ---------------------------------------------------------------------------

/// A Model's Slice Revisions, newest first.
#[tauri::command]
pub fn list_slice_revisions<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    model_id: String,
) -> Result<CommandSuccess<Vec<SliceRevisionSummary>>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    let summaries = services
        .storage
        .read(|connection| Ok(repository::list_revision_summaries(connection, &model_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?;
    Ok(CommandSuccess::new(summaries))
}

/// One Slice Revision in full.
#[tauri::command]
pub fn get_slice_revision<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    slice_revision_id: String,
) -> Result<CommandSuccess<SliceRevisionRecord>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    services
        .storage
        .read(|connection| Ok(repository::load_revision(connection, &slice_revision_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?
        .map(CommandSuccess::new)
        .ok_or_else(|| CommandError::not_found(slice_revision_id))
}

/// D16: creates an external Slice Revision over a G-code Model Source
/// Revision, from the operator's confirmed facts. The revision reuses the
/// source's content as its G-code; a non-G-code source is `VALIDATION` on
/// `sourceRevisionId`. Idempotent by `operationId`.
#[tauri::command]
pub async fn create_external_slice_revision<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    source_revision_id: String,
    facts: CreateExternalSliceRevisionFacts,
) -> Result<CommandSuccess<SliceRevisionRecord>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    blocking(move || {
        external::create_external_slice_revision(
            &services.slicing,
            &operation_id,
            &source_revision_id,
            &facts,
        )
    })
    .await
    .map(CommandSuccess::new)
}

/// D14: deletes a Slice Revision unless something references it
/// (`LIFECYCLE_BLOCKED`; nothing can before P7).
#[tauri::command]
pub fn delete_slice_revision<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    slice_revision_id: String,
) -> Result<CommandSuccess<SlicingDeleted>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    // The operations that made it lose their `sliceRevisionId` (and their
    // log, which was the revision's) in the same commit.
    let operations = services
        .storage
        .write_repo(|tx| {
            let ids = {
                let mut statement =
                    tx.prepare("SELECT id FROM slice_operations WHERE slice_revision_id = ?1")?;
                let ids = statement
                    .query_map([&slice_revision_id], |row| row.get::<_, String>(0))?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                ids
            };
            repository::delete_slice_revision(
                tx,
                &slice_revision_id,
                slice_revision_blocker_sources(),
            )?;
            let mut operations = Vec::new();
            for id in ids {
                operations.extend(repository::load_operation(tx, &id)?);
            }
            Ok(operations)
        })
        .map_err(CommandError::from_repository)?;
    // Committed; a blob that can't be unlinked now is retried at startup.
    let _ = services
        .library
        .content
        .release_unreferenced(&services.storage);
    let mut published = vec![events::revision_removed(&slice_revision_id)];
    published.extend(operations.iter().map(events::operation_changed));
    services.slicing.publish(published);
    Ok(CommandSuccess::new(SlicingDeleted {}))
}
