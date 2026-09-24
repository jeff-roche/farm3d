//! The Library's Tauri commands. Each validates the contract version and
//! waits for bootstrap before any side effect (opening a dialog, reading a
//! file, writing). None accepts a filesystem path (D7): files are named by
//! `selectionId` and `fileIndex`.
//!
//! The Project and Model commands (D1, D18) each run ONE
//! `Storage::write_repo`, then any post-commit work (unlinking released
//! blobs, unwatching a deleted link), then publish their `library.*` events
//! (D17). Nothing is emitted inside a transaction or for a failed write.
//! Records a command returns are read inside its own transaction, so the
//! result and the events carry exactly the committed state.

use std::io::Read;

use base64::Engine;
use rusqlite::Transaction;
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use ts_rs::TS;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::contracts::event::JsSafeInteger;
use crate::persistence::{RepositoryError, StorageError};
use crate::RuntimeServices;

use super::blockers::{self, ModelDeletionBlocker};
use super::content::{self, ContentError};
use super::events::{self, LibraryEventSpec};
use super::import::{self, ImportItemRequest, ImportModelsResult, MAX_PROJECT_IDS};
use super::inspection::{self, ImportInspection};
use super::links;
use super::repository;
use super::selection::{CancelImportSelectionData, ImportSelectionSummary, SelectionPurpose};
use super::{
    duplicate_name_warning, new_id, validate_model_name, validate_project_name, ImportWarning,
    ModelRecord, ModelSourceRevisionRecord, ProjectRecord, StorageMode, StoredModel,
};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// D17's backfill: the Library stream's identity, the sequence the
/// snapshot covers, and every Project and Model.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/LibrarySnapshot.ts")]
pub struct LibrarySnapshot {
    pub stream_id: String,
    pub snapshot_sequence: JsSafeInteger,
    pub projects: Vec<ProjectRecord>,
    pub models: Vec<ModelRecord>,
}

/// `create_project`/`rename_project`.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/ProjectMutationResult.ts"
)]
pub struct ProjectMutationResult {
    pub project: ProjectRecord,
}

/// `update_model`/`set_model_projects`. `warnings` carries D1's
/// `DUPLICATE_NAME`.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ModelMutationResult.ts")]
pub struct ModelMutationResult {
    pub model: ModelRecord,
    pub warnings: Vec<ImportWarning>,
}

/// `update_model`'s patch. An absent field is left unchanged.
#[derive(Deserialize, Serialize, Clone, Debug, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ModelPatch.ts")]
pub struct ModelPatch {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
}

/// D18 `delete_project`: the Models that lost the membership, and which of
/// them are now Unfiled, each ordered by Model name. Named
/// `DeleteProjectData` in TypeScript, because `DeleteProjectResult` there is
/// the command's `CommandSuccess` wrapper (as with `DeletePrinterData`).
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename = "DeleteProjectData",
    rename_all = "camelCase",
    export_to = "command/DeleteProjectData.ts"
)]
pub struct DeleteProjectResult {
    pub deleted_id: String,
    pub affected_model_ids: Vec<String>,
    pub now_unfiled_model_ids: Vec<String>,
}

/// D18 `delete_model`. Named `DeleteModelData` in TypeScript, as with
/// [`DeleteProjectResult`].
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename = "DeleteModelData",
    rename_all = "camelCase",
    export_to = "command/DeleteModelData.ts"
)]
pub struct DeleteModelResult {
    pub deleted_id: String,
    pub warnings: Vec<ImportWarning>,
}

/// D12: a revision's embedded thumbnail, for a `data:` URL.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/RevisionThumbnail.ts")]
pub struct RevisionThumbnail {
    pub media_type: String,
    pub width: u32,
    pub height: u32,
    pub data_base64: String,
}

/// The content store's totals, for the Library status line.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/LibraryContentInfo.ts")]
pub struct LibraryContentInfo {
    #[ts(type = "number")]
    pub blob_count: i64,
    #[ts(type = "number")]
    pub total_bytes: i64,
    #[ts(type = "number")]
    pub pending_cleanup_count: i64,
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// Reads through a read-only transaction.
fn read<R: tauri::Runtime, T>(
    services: &RuntimeServices<R>,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, RepositoryError>,
) -> Result<T, CommandError> {
    services
        .storage
        .read_transaction(|tx| Ok(operation(tx)))
        .map_err(storage_error)?
        .map_err(CommandError::from_repository)
}

/// Runs `operation` as the command's one write transaction.
fn write<R: tauri::Runtime, T>(
    services: &RuntimeServices<R>,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, RepositoryError>,
) -> Result<T, CommandError> {
    services
        .storage
        .write_repo(operation)
        .map_err(CommandError::from_repository)
}

/// The committed `ModelRecord` for `id`, read in the write's transaction.
fn committed_record<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    tx: &Transaction<'_>,
    id: &str,
) -> Result<ModelRecord, RepositoryError> {
    services
        .library
        .record_for(tx, id)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity_id: id.to_string(),
        })
}

/// The committed records of Projects `ids`, by name (the order
/// `library.project.changed` events go out in).
fn project_records(
    tx: &Transaction<'_>,
    ids: &[String],
) -> Result<Vec<ProjectRecord>, StorageError> {
    Ok(repository::list_projects(tx)?
        .into_iter()
        .filter(|project| ids.contains(&project.id))
        .collect())
}

/// A Project write rejected only for its name, after the command already
/// checked the name's length, is a case-insensitive duplicate (D1).
fn project_write_error(error: RepositoryError) -> CommandError {
    match error {
        RepositoryError::Validation { field_path: "name" } => {
            CommandError::validation_at("name", "A Project with this name already exists.")
        }
        other => CommandError::from_repository(other),
    }
}

/// D13 step 1: opens the native picker and registers the chosen files.
/// A cancelled dialog (or an empty choice) returns `null` and registers
/// nothing.
#[tauri::command]
pub async fn pick_model_files<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    purpose: SelectionPurpose,
) -> Result<CommandSuccess<Option<ImportSelectionSummary>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let file_io = std::sync::Arc::clone(&services.library.file_io);
    let picked = tauri::async_runtime::spawn_blocking(move || file_io.pick_files(purpose))
        .await
        .map_err(|_| CommandError::internal())??;
    let summary = picked
        .filter(|paths| !paths.is_empty())
        .map(|paths| services.library.selections.register(purpose, paths));
    Ok(CommandSuccess::new(summary))
}

/// D13 step 2: stages, hashes, and inspects every file of the selection,
/// emitting `library.import.progress`. A repeat call returns the stored
/// result. An unknown or expired selection is `SELECTION_EXPIRED`; a Locate
/// selection is `VALIDATION`.
#[tauri::command]
pub async fn inspect_import_selection<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    selection_id: String,
) -> Result<CommandSuccess<ImportInspection>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let entry = services.library.selections.get(&selection_id)?;
    import::require_import_purpose(&entry)?;
    inspection::inspect_selection(&app, &services.storage, &services.library, entry)
        .await
        .map(CommandSuccess::new)
}

/// D13 step 3: commits the requested `ready` items of an inspected
/// selection, each in its own transaction, and returns one outcome per
/// item. Repeating an `operationId` replays its recorded outcomes; another
/// operation while one is importing is `CONFLICT`.
#[tauri::command]
pub async fn import_models<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    selection_id: String,
    operation_id: String,
    items: Vec<ImportItemRequest>,
) -> Result<CommandSuccess<ImportModelsResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let entry = services.library.selections.get(&selection_id)?;
    import::validate_request(&entry, &operation_id, &items)?;
    let inspected = entry.inspection.lock().await.clone().ok_or_else(|| {
        CommandError::validation_at("selectionId", "Inspect these files before importing them.")
    })?;
    let result = tauri::async_runtime::spawn_blocking(move || {
        import::import_selection(
            &app,
            &services.storage,
            &services.library,
            &entry,
            &inspected,
            &operation_id,
            &items,
        )
    })
    .await
    .map_err(|_| CommandError::internal())??;
    Ok(CommandSuccess::new(result))
}

/// D13: stops in-flight work on the selection at its next check, deletes
/// its staging, and discards it. Unknown selections are a no-op.
#[tauri::command]
pub fn cancel_import_selection<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    selection_id: String,
) -> Result<CommandSuccess<CancelImportSelectionData>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    services.library.selections.discard(&selection_id);
    Ok(CommandSuccess::new(CancelImportSelectionData {}))
}

/// D17: the backfill. The sequence is read BEFORE the rows, so a change this
/// snapshot misses always has a larger sequence than `snapshotSequence`.
#[tauri::command]
pub fn list_library<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<LibrarySnapshot>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let snapshot_sequence = services.library.stream.snapshot_sequence();
    let (projects, models) = read(&services, |tx| {
        Ok((
            repository::list_projects(tx)?,
            services.library.records_for(tx, None)?,
        ))
    })?;
    Ok(CommandSuccess::new(LibrarySnapshot {
        stream_id: services.library.stream.stream_id().to_string(),
        snapshot_sequence,
        projects,
        models,
    }))
}

/// D1: a new, empty Project.
#[tauri::command]
pub fn create_project<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    name: String,
) -> Result<CommandSuccess<ProjectMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let name = validate_project_name(&name)?;
    let project = services
        .storage
        .write_repo(|tx| repository::insert_project(tx, &new_id("prj"), &name))
        .map_err(project_write_error)?;
    services
        .library
        .stream
        .publish(&app, vec![events::project_changed(&project)]);
    Ok(CommandSuccess::new(ProjectMutationResult { project }))
}

/// D1: renames a Project.
#[tauri::command]
pub fn rename_project<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
    name: String,
) -> Result<CommandSuccess<ProjectMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let name = validate_project_name(&name)?;
    let project = services
        .storage
        .write_repo(|tx| repository::rename_project(tx, &id, expected_revision, &name))
        .map_err(project_write_error)?;
    services
        .library
        .stream
        .publish(&app, vec![events::project_changed(&project)]);
    Ok(CommandSuccess::new(ProjectMutationResult { project }))
}

/// D18: deletes a Project and its memberships only. Each affected Model's
/// revision is bumped; no Model, revision, or blob is deleted.
#[tauri::command]
pub fn delete_project<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
) -> Result<CommandSuccess<DeleteProjectResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let models = write(&services, |tx| {
        let affected = repository::delete_project(tx, &id, expected_revision)?;
        Ok(services.library.records_for(tx, Some(&affected))?)
    })?;
    let mut published: Vec<LibraryEventSpec> = models.iter().map(events::model_changed).collect();
    published.push(events::project_removed(&id));
    services.library.stream.publish(&app, published);
    Ok(CommandSuccess::new(DeleteProjectResult {
        deleted_id: id,
        affected_model_ids: models.iter().map(|model| model.id.clone()).collect(),
        now_unfiled_model_ids: models
            .iter()
            .filter(|model| model.project_ids.is_empty())
            .map(|model| model.id.clone())
            .collect(),
    }))
}

/// D1: edits a Model's own fields (its name, in P4). A name another Model
/// in a shared Project (or, both Unfiled) already has is a
/// `DUPLICATE_NAME` warning, not an error. A patch that changes nothing
/// bumps nothing and emits nothing.
#[tauri::command]
pub fn update_model<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
    patch: ModelPatch,
) -> Result<CommandSuccess<ModelMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let name = patch.name.as_deref().map(validate_model_name).transpose()?;
    let (model, changed, duplicate_name) = write(&services, |tx| {
        let stored = repository::checked_model(tx, &id, expected_revision)?;
        let changed = name.as_ref().is_some_and(|name| *name != stored.name);
        if let Some(name) = name.as_deref().filter(|_| changed) {
            repository::rename_model(tx, &id, name)?;
        }
        let duplicate_name = repository::has_same_name_model(tx, &id)?;
        Ok((
            committed_record(&services, tx, &id)?,
            changed,
            duplicate_name,
        ))
    })?;
    if changed {
        services
            .library
            .stream
            .publish(&app, vec![events::model_changed(&model)]);
    }
    let warnings = if duplicate_name {
        vec![duplicate_name_warning()]
    } else {
        Vec::new()
    };
    Ok(CommandSuccess::new(ModelMutationResult { model, warnings }))
}

/// Spec §Commands: the one membership command. Adds and removes are a set
/// edit in one transaction; the Model's revision is bumped only when the
/// membership actually changed, and a call that changes nothing emits
/// nothing.
#[tauri::command]
pub fn set_model_projects<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    model_id: String,
    expected_revision: i64,
    add: Vec<String>,
    remove: Vec<String>,
) -> Result<CommandSuccess<ModelMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let (add, remove) = membership_request(add, remove)?;
    let (model, changed, touched) = write(&services, |tx| {
        repository::checked_model(tx, &model_id, expected_revision)?;
        for project_id in &remove {
            if !repository::project_exists(tx, project_id)? {
                return Err(RepositoryError::NotFound {
                    entity_id: project_id.clone(),
                });
            }
        }
        let change = repository::apply_membership(tx, &model_id, &add, &remove)?;
        if change.changed {
            repository::bump_model_revision(tx, &model_id)?;
        }
        Ok((
            committed_record(&services, tx, &model_id)?,
            change.changed,
            project_records(tx, &change.touched_projects)?,
        ))
    })?;
    if changed {
        let mut published = vec![events::model_changed(&model)];
        published.extend(touched.iter().map(events::project_changed));
        services.library.stream.publish(&app, published);
    }
    Ok(CommandSuccess::new(ModelMutationResult {
        model,
        warnings: Vec::new(),
    }))
}

/// `set_model_projects`' request checks: at most [`MAX_PROJECT_IDS`] ids
/// per list, repeats collapsed, and no id in both lists.
fn membership_request(
    add: Vec<String>,
    remove: Vec<String>,
) -> Result<(Vec<String>, Vec<String>), CommandError> {
    fn distinct(ids: Vec<String>, field: &str) -> Result<Vec<String>, CommandError> {
        if ids.len() > MAX_PROJECT_IDS {
            return Err(CommandError::validation_at(
                field,
                "At most 64 Projects can change at once.",
            ));
        }
        let mut seen = std::collections::HashSet::new();
        Ok(ids
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect())
    }
    let add = distinct(add, "add")?;
    let remove = distinct(remove, "remove")?;
    if remove.iter().any(|id| add.contains(id)) {
        return Err(CommandError::validation_at(
            "remove",
            "A Project can't be both added and removed.",
        ));
    }
    Ok((add, remove))
}

/// What [`delete_model_in`] removed, for the post-commit work and events.
struct DeletedModel {
    model: StoredModel,
    /// The Projects it belonged to, as committed, by name.
    projects: Vec<ProjectRecord>,
}

/// D18 steps 1-3, inside the delete's transaction: the revision check, the
/// deletion blockers `sources` report, then the Model row (its revisions,
/// thumbnail rows, and memberships cascade), then D4's in-transaction
/// cleanup of every blob nothing else refers to. Production passes
/// [`blockers::blocker_sources`].
fn delete_model_in(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    sources: &[&dyn ModelDeletionBlocker],
) -> Result<DeletedModel, RepositoryError> {
    let model = repository::checked_model(tx, id, expected_revision)?;
    let blocked = blockers::evaluate(&model, tx, sources)?;
    if !blocked.is_empty() {
        return Err(RepositoryError::LifecycleBlocked(blocked));
    }
    let hashes = repository::model_content_hashes(tx, id)?;
    let project_ids = repository::project_ids_for(tx, id)?;
    repository::delete_model_row(tx, id)?;
    content::mark_unreferenced_blobs(tx, &hashes)?;
    Ok(DeletedModel {
        model,
        projects: project_records(tx, &project_ids)?,
    })
}

/// D18: deletes a Model with all its revisions. After commit, blobs nothing
/// else refers to are unlinked, a linked source stops being followed, and
/// `library.model.removed` goes out, then `library.project.changed` for
/// each Project it belonged to. A linked Model's source file is never
/// touched.
#[tauri::command]
pub fn delete_model<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
) -> Result<CommandSuccess<DeleteModelResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let deleted = write(&services, |tx| {
        delete_model_in(tx, &id, expected_revision, blockers::blocker_sources())
    })?;
    // The delete has committed; a blob that can't be unlinked now keeps its
    // `pending_blob_cleanup` row for the startup sweep to retry.
    let _ = services
        .library
        .content
        .release_unreferenced(&services.storage);
    if deleted.model.storage_mode == StorageMode::Linked {
        services.library.unfollow_link(&id);
    }
    let mut published = vec![events::model_removed(&id)];
    published.extend(deleted.projects.iter().map(events::project_changed));
    services.library.stream.publish(&app, published);
    Ok(CommandSuccess::new(DeleteModelResult {
        deleted_id: id,
        warnings: Vec::new(),
    }))
}

/// Every revision of a Model in full, newest first.
#[tauri::command]
pub fn list_model_revisions<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    model_id: String,
) -> Result<CommandSuccess<Vec<ModelSourceRevisionRecord>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let revisions = read(&services, |tx| {
        if repository::load_model(tx, &model_id)?.is_none() {
            return Err(RepositoryError::NotFound {
                entity_id: model_id.clone(),
            });
        }
        Ok(repository::list_revision_records(tx, &model_id)?)
    })?;
    Ok(CommandSuccess::new(revisions))
}

/// D12: a revision's embedded thumbnail as base64, or `null` when it has
/// none (every STL revision). The blob is verified against its hash as it
/// is read.
#[tauri::command]
pub fn get_revision_thumbnail<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    revision_id: String,
) -> Result<CommandSuccess<Option<RevisionThumbnail>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let row = read(&services, |tx| {
        repository::load_thumbnail(tx, &revision_id)?.ok_or_else(|| RepositoryError::NotFound {
            entity_id: revision_id.clone(),
        })
    })?;
    let Some(row) = row else {
        return Ok(CommandSuccess::new(None));
    };
    let mut bytes = Vec::new();
    services
        .library
        .content
        .open_verified(&row.content_sha256)?
        .read_to_end(&mut bytes)
        .map_err(ContentError::from)?;
    Ok(CommandSuccess::new(Some(RevisionThumbnail {
        media_type: row.media_type,
        width: row.width,
        height: row.height,
        data_base64: base64::engine::general_purpose::STANDARD.encode(bytes),
    })))
}

/// The content store's totals (`Stored copies: 1.2 GB`).
#[tauri::command]
pub fn library_content_info<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<LibraryContentInfo>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let info = services
        .library
        .content
        .info(&services.storage)
        .map_err(storage_error)?;
    Ok(CommandSuccess::new(LibraryContentInfo {
        blob_count: info.blob_count,
        total_bytes: info.total_bytes,
        pending_cleanup_count: info.pending_cleanup_count,
    }))
}

/// D15 trigger 3: checks `modelIds` (every linked Model when absent) now
/// and returns the records of those that changed. The frontend calls it
/// when the Library becomes visible or regains focus, and from **Check
/// sources**. Unknown and managed Models are skipped.
#[tauri::command]
pub async fn check_linked_sources<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    model_ids: Option<Vec<String>>,
) -> Result<CommandSuccess<Vec<ModelRecord>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let links = services
        .library
        .links
        .get()
        .cloned()
        .ok_or_else(CommandError::internal)?;
    Ok(CommandSuccess::new(links.check(model_ids).await?))
}

/// D16: relinks a linked Model to the file chosen through
/// `pick_model_files { purpose: "locate" }`. The file must have the Model's
/// format. The same content relinks without a revision; different content
/// is `SOURCE_CONTENT_DIFFERS` unless `acceptDifferentContent`, which adds a
/// `relocate` revision. The selection is discarded once the Model is
/// relinked, and the new path is followed from then on.
#[tauri::command]
#[allow(clippy::too_many_arguments)]
pub async fn locate_linked_source<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    model_id: String,
    expected_revision: i64,
    selection_id: String,
    file_index: u32,
    accept_different_content: bool,
) -> Result<CommandSuccess<ModelMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let entry = services.library.selections.get(&selection_id)?;
    if entry.purpose != SelectionPurpose::Locate {
        return Err(CommandError::validation_at(
            "selectionId",
            "This selection is for importing, not for locating a source.",
        ));
    }
    let file = entry
        .files
        .get(file_index as usize)
        .cloned()
        .ok_or_else(|| {
            CommandError::validation_at("fileIndex", "The selection has no file at this index.")
        })?;
    let located = {
        let services = std::sync::Arc::clone(&services);
        let file = file.clone();
        tauri::async_runtime::spawn_blocking(move || {
            links::locate_source(
                &services.library,
                &services.storage,
                &model_id,
                expected_revision,
                &file,
                accept_different_content,
            )
        })
        .await
        .map_err(|_| CommandError::internal())??
    };
    services.library.selections.discard(&selection_id);
    let mut model = located.record;
    let warnings: Vec<ImportWarning> = services
        .library
        .follow_link(&model.id, &file.path)
        .into_iter()
        .collect();
    services.library.apply_watch_mode(&mut model);
    let mut published = vec![events::model_changed(&model)];
    published.extend(located.revision.as_ref().map(events::revision_created));
    services.library.stream.publish(&app, published);
    Ok(CommandSuccess::new(ModelMutationResult { model, warnings }))
}

/// D5: makes a linked Model managed and stops following its source. Every
/// revision already holds its bytes, so nothing is copied, and it works in
/// any source state, `missing` included.
#[tauri::command]
pub fn convert_model_to_managed<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    model_id: String,
    expected_revision: i64,
) -> Result<CommandSuccess<ModelMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let model = services
        .storage
        .write_repo(|tx| {
            let stored = repository::checked_model(tx, &model_id, expected_revision)?;
            if stored.storage_mode != StorageMode::Linked {
                return Err(RepositoryError::Validation {
                    field_path: "modelId",
                });
            }
            repository::convert_to_managed(tx, &model_id)?;
            committed_record(&services, tx, &model_id)
        })
        .map_err(|error| match error {
            RepositoryError::Validation {
                field_path: "modelId",
            } => CommandError::validation_at("modelId", "This Model is already managed."),
            other => CommandError::from_repository(other),
        })?;
    services.library.unfollow_link(&model_id);
    services
        .library
        .stream
        .publish(&app, vec![events::model_changed(&model)]);
    Ok(CommandSuccess::new(ModelMutationResult {
        model,
        warnings: Vec::new(),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::command::ErrorCode;
    use crate::persistence::Storage;
    use crate::printers::lifecycle::{LifecycleAction, LifecycleBlocker, LifecycleBlockerCode};

    const SHA: &str = "aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa";

    /// A test-only deletion blocker (P17): production registers none.
    struct InUse;

    impl ModelDeletionBlocker for InUse {
        fn blockers(
            &self,
            model: &StoredModel,
            _tx: &Transaction<'_>,
        ) -> Result<Vec<LifecycleBlocker>, StorageError> {
            Ok(vec![LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::NotArchived,
                message: format!("{} is still in use.", model.name),
            }])
        }
    }

    /// One managed Model with one revision over one blob, in one Project.
    fn seed(storage: &Storage) {
        storage
            .write(|tx| {
                let now = "2026-01-01T00:00:00.000Z";
                tx.execute(
                    "INSERT INTO content_blobs (sha256, size_bytes, created_at) VALUES (?1, 3, ?2)",
                    [SHA, now],
                )?;
                tx.execute(
                    "INSERT INTO library_models (id, revision, name, format, storage_mode,
                                                 created_at, updated_at)
                     VALUES ('mdl-a', 1, 'Bracket', 'stl', 'managed', ?1, ?1)",
                    [now],
                )?;
                tx.execute(
                    "INSERT INTO model_source_revisions (
                        id, model_id, sequence, content_sha256, size_bytes, format, origin,
                        source_file_name, source_path, captured_at, inspector_version,
                        inspection_json
                     ) VALUES ('msr-a', 'mdl-a', 1, ?1, 3, 'stl', 'import', 'bracket.stl',
                               '/src/bracket.stl', ?2, 1, '{}')",
                    [SHA, now],
                )?;
                tx.execute(
                    "INSERT INTO library_projects (id, revision, name, created_at, updated_at)
                     VALUES ('prj-a', 1, 'Parts', ?1, ?1)",
                    [now],
                )?;
                tx.execute(
                    "INSERT INTO project_models (project_id, model_id, added_at)
                     VALUES ('prj-a', 'mdl-a', ?1)",
                    [now],
                )?;
                Ok(())
            })
            .expect("seed");
    }

    fn counts(storage: &Storage) -> [i64; 5] {
        storage
            .read(|connection| {
                let count = |table: &str| {
                    connection.query_row(&format!("SELECT COUNT(*) FROM {table}"), [], |row| {
                        row.get::<_, i64>(0)
                    })
                };
                Ok([
                    count("library_models")?,
                    count("model_source_revisions")?,
                    count("project_models")?,
                    count("content_blobs")?,
                    count("pending_blob_cleanup")?,
                ])
            })
            .expect("counts")
    }

    #[test]
    fn a_blocked_delete_is_lifecycle_blocked_and_changes_nothing() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed(&storage);

        let error = storage
            .write_repo(|tx| delete_model_in(tx, "mdl-a", 1, &[&InUse]))
            .err()
            .expect("the blocker must block the delete");

        let RepositoryError::LifecycleBlocked(blockers) = &error else {
            panic!("expected LifecycleBlocked, got {error:?}");
        };
        assert_eq!(blockers.len(), 1);
        assert_eq!(blockers[0].action, LifecycleAction::Delete);
        assert_eq!(blockers[0].message, "Bracket is still in use.");
        let command = CommandError::from_repository(error);
        assert_eq!(command.code, ErrorCode::LifecycleBlocked);
        assert!(command.details.unwrap().contains_key("blockers"));
        assert_eq!(counts(&storage), [1, 1, 1, 1, 0], "nothing changed");
    }

    #[test]
    fn an_unblocked_delete_cascades_and_marks_the_unreferenced_blob() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed(&storage);
        assert!(blockers::blocker_sources().is_empty(), "P4 registers none");

        let deleted = storage
            .write_repo(|tx| delete_model_in(tx, "mdl-a", 1, blockers::blocker_sources()))
            .expect("delete");

        assert_eq!(deleted.model.id, "mdl-a");
        assert_eq!(deleted.projects.len(), 1);
        assert_eq!(deleted.projects[0].model_count, 0);
        assert_eq!(counts(&storage), [0, 0, 0, 0, 1]);
    }
}
