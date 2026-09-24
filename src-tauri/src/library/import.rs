//! D13 step 3 and D14: committing a selection's inspected files as Models
//! and Model Source Revisions.
//!
//! `import_models` walks its items in request order. Each `ready` item is
//! checked (the row, then against the database), then placed and committed
//! in its own transaction through `ContentStore::place_and_commit_unless_cancelled`,
//! whose closure re-checks everything against the database before writing.
//! Earlier items stay committed if a later one fails. A committed item's
//! outcome is recorded on the selection under its `operationId`, then its
//! events are published. Duplicate content is never imported silently: an
//! item whose bytes some Model already holds needs an explicit
//! `duplicateAction`.

use std::collections::{HashMap, HashSet};
use std::sync::{Condvar, Mutex, MutexGuard};

use rusqlite::{Connection, Transaction};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use ts_rs::TS;

use crate::contracts::command::CommandError;
use crate::persistence::{RepositoryError, Storage, StorageError};

use super::content::{CancelFlag, ContentError, SourceStat, StagedFile};
use super::events::{self, LibraryEventSpec};
use super::formats::Inspection;
use super::inspection::{ImportItemErrorCode, InspectedItem, ReadyItem};
use super::repository::{self, LinkObservation, NewModel, RevisionSource};
use super::selection::{SelectedFile, SelectionEntry, SelectionPurpose};
use super::{
    duplicate_name_warning, new_id, validate_model_name, ImportWarning, LibraryServices,
    ModelRecord, ModelSourceRevisionSummary, ProjectRecord, RevisionOrigin, StorageMode,
};

/// D13: the most Projects one import row may name.
pub const MAX_PROJECT_IDS: usize = 64;

/// D14: how to resolve an item whose content the Library already holds.
/// `addRevision` is available for every ready item, duplicate or not.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/DuplicateAction.ts")]
pub enum DuplicateAction {
    UseExisting,
    AddAnother,
    AddRevision,
}

/// D13: one file of the selection to commit. `projectIds: []` is Unfiled,
/// and repeated ids are ignored. `storageMode` applies to a new Model only.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportItemRequest.ts")]
pub struct ImportItemRequest {
    pub file_index: u32,
    pub name: String,
    pub project_ids: Vec<String>,
    pub storage_mode: StorageMode,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub duplicate_action: Option<DuplicateAction>,
    /// Required for `useExisting` and `addRevision`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target_model_id: Option<String>,
    /// Required for `addRevision`; checked for `useExisting` when given.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub target_expected_revision: Option<i64>,
    pub acknowledge_unsupported: bool,
}

/// D13's per-item outcome.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportOutcome.ts")]
pub enum ImportOutcome {
    Imported,
    RevisionAdded,
    ReusedExisting,
    Rejected,
    Cancelled,
}

/// Why one item was not committed. `fieldPath` names the request field for
/// `VALIDATION`; `entityId` names the unknown or changed Project or Model
/// for `NOT_FOUND` and `CONFLICT`. Messages carry no paths.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportItemError.ts")]
pub struct ImportItemError {
    pub code: ImportItemErrorCode,
    pub message: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub field_path: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub entity_id: Option<String>,
}

impl ImportItemError {
    fn new(code: ImportItemErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            field_path: None,
            entity_id: None,
        }
    }

    fn validation(field_path: &str, message: &str) -> Self {
        Self {
            field_path: Some(field_path.to_string()),
            ..Self::new(ImportItemErrorCode::Validation, message)
        }
    }

    fn not_found(entity_id: &str, message: &str) -> Self {
        Self {
            entity_id: Some(entity_id.to_string()),
            ..Self::new(ImportItemErrorCode::NotFound, message)
        }
    }

    fn conflict(entity_id: &str) -> Self {
        Self {
            entity_id: Some(entity_id.to_string()),
            ..Self::new(
                ImportItemErrorCode::Conflict,
                "The Model changed. Review it and try again.",
            )
        }
    }

    fn persistence() -> Self {
        Self::new(
            ImportItemErrorCode::PersistenceUnavailable,
            "farm3d couldn't save this file to the Library.",
        )
    }
}

/// One item of `import_models`' result. `model` is present for the three
/// committed outcomes. `revision` is the revision this import created, so
/// it is present for `imported` and `revisionAdded` only.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportItemResult.ts")]
pub struct ImportItemResult {
    pub file_index: u32,
    pub outcome: ImportOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub model: Option<ModelRecord>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub revision: Option<ModelSourceRevisionSummary>,
    pub errors: Vec<ImportItemError>,
    pub warnings: Vec<ImportWarning>,
}

impl ImportItemResult {
    fn rejected(file_index: u32, error: ImportItemError) -> Self {
        Self {
            file_index,
            outcome: ImportOutcome::Rejected,
            model: None,
            revision: None,
            errors: vec![error],
            warnings: Vec::new(),
        }
    }

    fn cancelled(file_index: u32) -> Self {
        Self {
            outcome: ImportOutcome::Cancelled,
            ..Self::rejected(
                file_index,
                ImportItemError::new(ImportItemErrorCode::Cancelled, "The import was cancelled."),
            )
        }
    }
}

/// `import_models`' result, one item per request item, in request order.
/// Named `ImportModelsData` in TypeScript, because `ImportModelsResult`
/// there is the command's `CommandSuccess` wrapper (as with `MoveSpoolData`).
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename = "ImportModelsData",
    rename_all = "camelCase",
    export_to = "command/ImportModelsData.ts"
)]
pub struct ImportModelsResult {
    pub items: Vec<ImportItemResult>,
}

/// D13's in-memory idempotency record for one selection (ruling §6): each
/// committed item's outcome under `(operationId, fileIndex)`, and which
/// operation is importing right now. It lives and dies with the selection.
#[derive(Default)]
pub struct ImportLedger {
    state: Mutex<LedgerState>,
    /// Signalled when a run ends, for a repeat of the same operation that
    /// waits for it.
    finished: Condvar,
}

#[derive(Default)]
struct LedgerState {
    /// The operation importing right now. Checked and claimed under one
    /// lock, so runs on one selection never interleave.
    running: Option<String>,
    outcomes: HashMap<(String, u32), ImportItemResult>,
    committed: HashSet<u32>,
}

impl ImportLedger {
    /// Starts a run for `operation_id`. Another operation still importing
    /// on this selection is `CONFLICT`; the same operation waits for it and
    /// then replays.
    fn begin(&self, operation_id: &str) -> Result<ImportRun<'_>, CommandError> {
        let mut state = lock(&self.state);
        loop {
            match state.running.as_deref() {
                None => {
                    state.running = Some(operation_id.to_string());
                    return Ok(ImportRun { ledger: self });
                }
                Some(running) if running == operation_id => {
                    state = self
                        .finished
                        .wait(state)
                        .unwrap_or_else(|poisoned| poisoned.into_inner());
                }
                Some(_) => {
                    return Err(CommandError::conflict(
                        "These files are already being imported. Wait for that import to finish.",
                    ))
                }
            }
        }
    }

    fn recorded(&self, operation_id: &str, file_index: u32) -> Option<ImportItemResult> {
        lock(&self.state)
            .outcomes
            .get(&(operation_id.to_string(), file_index))
            .cloned()
    }

    fn record(&self, operation_id: &str, result: &ImportItemResult) {
        let mut state = lock(&self.state);
        state.committed.insert(result.file_index);
        state.outcomes.insert(
            (operation_id.to_string(), result.file_index),
            result.clone(),
        );
    }

    fn all_committed(&self, file_indexes: impl IntoIterator<Item = u32>) -> bool {
        let state = lock(&self.state);
        file_indexes
            .into_iter()
            .all(|index| state.committed.contains(&index))
    }
}

/// The claim on a selection's ledger for one `import_models` run; dropping
/// it ends the run.
struct ImportRun<'a> {
    ledger: &'a ImportLedger,
}

impl Drop for ImportRun<'_> {
    fn drop(&mut self) {
        lock(&self.ledger.state).running = None;
        self.ledger.finished.notify_all();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // The ledger's maps hold no invariant a panicking holder could break.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The request-level checks, before any side effect: a non-blank
/// `operationId` (mirroring P3's `operations::claim`), an import selection,
/// and file indexes that each name one of its files once.
pub fn validate_request(
    entry: &SelectionEntry,
    operation_id: &str,
    requests: &[ImportItemRequest],
) -> Result<(), CommandError> {
    if operation_id.trim().is_empty() {
        return Err(CommandError::validation_at(
            "operationId",
            "operationId is required.",
        ));
    }
    if entry.purpose != SelectionPurpose::Import {
        return Err(CommandError::validation_at(
            "selectionId",
            "This selection is for locating a source, not for importing.",
        ));
    }
    let mut seen = HashSet::new();
    for (position, request) in requests.iter().enumerate() {
        let field = format!("items[{position}].fileIndex");
        if request.file_index as usize >= entry.files.len() {
            return Err(CommandError::validation_at(
                field,
                "The selection has no file at this index.",
            ));
        }
        if !seen.insert(request.file_index) {
            return Err(CommandError::validation_at(
                field,
                "Each file can appear once per import.",
            ));
        }
    }
    Ok(())
}

/// D13 step 3 for `entry`, whose inspection result is `inspected`. Blocking:
/// run it off the async runtime. The caller has run [`validate_request`].
pub fn import_selection<R: Runtime>(
    app: &AppHandle<R>,
    storage: &Storage,
    library: &LibraryServices<R>,
    entry: &SelectionEntry,
    inspected: &[InspectedItem],
    operation_id: &str,
    requests: &[ImportItemRequest],
) -> Result<ImportModelsResult, CommandError> {
    let _run = entry.imports.begin(operation_id)?;
    let importer = Importer {
        storage,
        library,
        cancel: entry.cancel_flag(),
    };
    let mut items = Vec::with_capacity(requests.len());
    for request in requests {
        let file_index = request.file_index;
        if let Some(recorded) = entry.imports.recorded(operation_id, file_index) {
            items.push(recorded);
            continue;
        }
        let ready = match &inspected[file_index as usize] {
            InspectedItem::Rejected { code, message } => {
                items.push(ImportItemResult::rejected(
                    file_index,
                    ImportItemError::new(*code, message.clone()),
                ));
                continue;
            }
            InspectedItem::Ready(ready) => ready,
        };
        let file = &entry.files[file_index as usize];
        match importer.commit(ready, file, request) {
            Ok(committed) => {
                let (result, events) = committed.finish(library, ready, file, file_index);
                entry.imports.record(operation_id, &result);
                library.stream.publish(app, events);
                items.push(result);
            }
            Err(ItemFailure::Cancelled) => items.push(ImportItemResult::cancelled(file_index)),
            Err(ItemFailure::Rejected(error)) => {
                items.push(ImportItemResult::rejected(file_index, error))
            }
        }
    }
    let ready_indexes = inspected
        .iter()
        .enumerate()
        .filter(|(_, item)| matches!(item, InspectedItem::Ready(_)))
        .map(|(index, _)| index as u32);
    if entry.imports.all_committed(ready_indexes) {
        library.content.discard_staging(&entry.id);
    }
    Ok(ImportModelsResult { items })
}

enum ItemFailure {
    Rejected(ImportItemError),
    Cancelled,
}

impl From<ImportItemError> for ItemFailure {
    fn from(error: ImportItemError) -> Self {
        Self::Rejected(error)
    }
}

/// An item's request after the row checks: a valid name, Projects without
/// repeats, and the target the action needs.
struct Row {
    name: String,
    project_ids: Vec<String>,
    action: Option<DuplicateAction>,
    target_model_id: Option<String>,
    target_expected_revision: Option<i64>,
    linked_path: Option<String>,
}

/// What committing an item will write. Decided before placement, then
/// decided again inside the transaction; the two must agree.
#[derive(Clone, Debug, PartialEq, Eq)]
enum Plan {
    NewModel,
    NewRevision { target_id: String },
    Reuse { target_id: String },
}

impl Plan {
    fn writes_revision(&self) -> bool {
        !matches!(self, Self::Reuse { .. })
    }
}

struct Importer<'a, R: Runtime> {
    storage: &'a Storage,
    library: &'a LibraryServices<R>,
    cancel: CancelFlag,
}

impl<R: Runtime> Importer<'_, R> {
    fn commit(
        &self,
        ready: &ReadyItem,
        file: &SelectedFile,
        request: &ImportItemRequest,
    ) -> Result<Committed, ItemFailure> {
        if self.cancel.is_cancelled() {
            return Err(ItemFailure::Cancelled);
        }
        let row = check_row(request, ready, file)?;
        // Checked once before placement, so a row the database rejects
        // leaves no orphan blob behind.
        let plan = self
            .storage
            .read(|connection| Ok(plan_item(connection, &row, ready)))
            .map_err(|_| ImportItemError::persistence())??;
        let staged: Vec<&StagedFile> = if plan.writes_revision() {
            std::iter::once(&ready.staged)
                .chain(ready.thumbnail.as_ref().map(|thumbnail| &thumbnail.staged))
                .collect()
        } else {
            Vec::new()
        };
        let mut rejection = None;
        let committed = self.library.content.place_and_commit_unless_cancelled(
            self.storage,
            &staged,
            &self.cancel,
            |tx| {
                let current = match plan_item(tx, &row, ready) {
                    Ok(current) => current,
                    Err(error) => return Err(roll_back(&mut rejection, error)),
                };
                if current != plan {
                    let target = row.target_model_id.as_deref().unwrap_or_default();
                    return Err(roll_back(&mut rejection, ImportItemError::conflict(target)));
                }
                self.write(tx, &plan, &row, ready, file)
            },
        );
        match committed {
            Ok(committed) => Ok(committed),
            Err(_) if self.cancel.is_cancelled() => Err(ItemFailure::Cancelled),
            Err(ContentError::Repository(error)) => Err(ItemFailure::Rejected(
                rejection.unwrap_or_else(|| repository_rejection(error)),
            )),
            Err(ContentError::Cancelled) => Err(ItemFailure::Cancelled),
            Err(_) => Err(ItemFailure::Rejected(ImportItemError::persistence())),
        }
    }

    fn write(
        &self,
        tx: &Transaction<'_>,
        plan: &Plan,
        row: &Row,
        ready: &ReadyItem,
        file: &SelectedFile,
    ) -> Result<Committed, RepositoryError> {
        let source_path = file.path.to_string_lossy();
        let source_mtime = ready
            .staged
            .source
            .as_ref()
            .and_then(SourceStat::modified_rfc3339);
        let source = RevisionSource {
            path: &source_path,
            file_name: &file.file_name,
            mtime: source_mtime.as_deref(),
        };
        let add_revision = |model_id: &str, origin| -> Result<(), RepositoryError> {
            let revision = repository::insert_revision(
                tx,
                model_id,
                &ready.staged,
                &ready.outcome,
                origin,
                &source,
            )?;
            if let Some(thumbnail) = &ready.thumbnail {
                repository::insert_thumbnail(tx, &revision.id, thumbnail)?;
            }
            Ok(())
        };
        let (model_id, outcome) = match plan {
            Plan::NewModel => {
                let model_id = new_id("mdl");
                repository::insert_model(
                    tx,
                    &NewModel {
                        id: &model_id,
                        name: &row.name,
                        format: ready.format,
                        link: row.linked_path.as_deref().map(|path| LinkObservation {
                            path,
                            stat: ready.staged.source.as_ref(),
                        }),
                    },
                )?;
                add_revision(&model_id, RevisionOrigin::Import)?;
                (model_id, ImportOutcome::Imported)
            }
            Plan::NewRevision { target_id } => {
                add_revision(target_id, RevisionOrigin::AddedRevision)?;
                (target_id.clone(), ImportOutcome::RevisionAdded)
            }
            Plan::Reuse { target_id } => (target_id.clone(), ImportOutcome::ReusedExisting),
        };
        // D13: an existing target only ever gains memberships.
        let membership = repository::apply_membership(tx, &model_id, &row.project_ids, &[])?;
        let is_new_model = matches!(plan, Plan::NewModel);
        if !is_new_model && (plan.writes_revision() || membership.changed) {
            repository::bump_model_revision(tx, &model_id)?;
        }
        let model =
            self.library
                .record_for(tx, &model_id)?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity_id: model_id.clone(),
                })?;
        let duplicate_name = repository::has_same_name_model(tx, &model_id)?;
        let revision = plan
            .writes_revision()
            .then(|| model.current_revision.clone());
        let gained_projects = repository::list_projects(tx)?
            .into_iter()
            .filter(|project| membership.touched_projects.contains(&project.id))
            .collect();
        Ok(Committed {
            outcome,
            model,
            revision,
            gained_projects,
            newly_linked: is_new_model && row.linked_path.is_some(),
            duplicate_name,
        })
    }
}

/// Keeps the item-level `error` for the caller and returns the error that
/// rolls the transaction back (its own value is never reported).
fn roll_back(slot: &mut Option<ImportItemError>, error: ImportItemError) -> RepositoryError {
    *slot = Some(error);
    RepositoryError::Validation {
        field_path: "items",
    }
}

/// A committed item, before its link is followed and its events go out.
struct Committed {
    outcome: ImportOutcome,
    model: ModelRecord,
    revision: Option<ModelSourceRevisionSummary>,
    gained_projects: Vec<ProjectRecord>,
    newly_linked: bool,
    /// D1: another Model shares this one's name and a Project (or both are
    /// Unfiled).
    duplicate_name: bool,
}

impl Committed {
    /// Starts following a new link (D15), then builds the item's result and
    /// its events: `library.model.changed`, `library.revision.created` when
    /// a revision was added, and `library.project.changed` for each Project
    /// the Model joined, by name (P12).
    fn finish<R: Runtime>(
        mut self,
        library: &LibraryServices<R>,
        ready: &ReadyItem,
        file: &SelectedFile,
        file_index: u32,
    ) -> (ImportItemResult, Vec<LibraryEventSpec>) {
        let mut warnings = ready.outcome.warnings.clone();
        if self.duplicate_name {
            warnings.push(duplicate_name_warning());
        }
        if self.newly_linked {
            warnings.extend(library.follow_link(&self.model.id, &file.path));
            library.apply_watch_mode(&mut self.model);
        }
        let mut events = vec![events::model_changed(&self.model)];
        events.extend(self.revision.as_ref().map(events::revision_created));
        events.extend(self.gained_projects.iter().map(events::project_changed));
        let result = ImportItemResult {
            file_index,
            outcome: self.outcome,
            model: Some(self.model),
            revision: self.revision,
            errors: Vec::new(),
            warnings,
        };
        (result, events)
    }
}

/// D13's row validation that needs no database.
fn check_row(
    request: &ImportItemRequest,
    ready: &ReadyItem,
    file: &SelectedFile,
) -> Result<Row, ImportItemError> {
    let name = validate_model_name(&request.name).map_err(|_| {
        ImportItemError::validation("name", "The Model name must be 1-255 characters.")
    })?;
    if request.project_ids.len() > MAX_PROJECT_IDS {
        return Err(ImportItemError::validation(
            "projectIds",
            "A file can be added to at most 64 Projects at once.",
        ));
    }
    let mut seen = HashSet::new();
    let project_ids: Vec<String> = request
        .project_ids
        .iter()
        .filter(|id| seen.insert(id.as_str()))
        .cloned()
        .collect();
    let has_unsupported = match &ready.outcome.inspection {
        Inspection::ThreeMf(model) => !model.unsupported.is_empty(),
        Inspection::Stl(_) | Inspection::Gcode(_) => false,
    };
    if has_unsupported && !request.acknowledge_unsupported {
        return Err(ImportItemError::new(
            ImportItemErrorCode::UnsupportedNotAcknowledged,
            "This 3MF has content farm3d keeps but doesn't use. Acknowledge it to import.",
        ));
    }
    let action = request.duplicate_action;
    let needs_target = matches!(
        action,
        Some(DuplicateAction::UseExisting | DuplicateAction::AddRevision)
    );
    if needs_target && request.target_model_id.is_none() {
        return Err(ImportItemError::validation(
            "targetModelId",
            "Choose the Model to use.",
        ));
    }
    if action == Some(DuplicateAction::AddRevision) && request.target_expected_revision.is_none() {
        return Err(ImportItemError::validation(
            "targetExpectedRevision",
            "targetExpectedRevision is required to add a revision.",
        ));
    }
    let creates_model = !needs_target;
    let linked_path = if creates_model && request.storage_mode == StorageMode::Linked {
        // A linked Model follows its path by name (D6), so the stored path
        // must round-trip exactly.
        let path = file.path.to_str().ok_or_else(|| {
            ImportItemError::validation(
                "storageMode",
                "farm3d can't link to this file's location. Import a copy instead.",
            )
        })?;
        Some(path.to_string())
    } else {
        None
    };
    Ok(Row {
        name,
        project_ids,
        action,
        target_model_id: request.target_model_id.clone(),
        target_expected_revision: request.target_expected_revision,
        linked_path,
    })
}

/// D13/D14's database checks, run before placement and again inside the
/// commit transaction.
fn plan_item(
    connection: &Connection,
    row: &Row,
    ready: &ReadyItem,
) -> Result<Plan, ImportItemError> {
    let persistence = |_: StorageError| ImportItemError::persistence();
    for project_id in &row.project_ids {
        if !repository::project_exists(connection, project_id).map_err(persistence)? {
            return Err(ImportItemError::not_found(
                project_id,
                "A chosen Project no longer exists.",
            ));
        }
    }
    let sha256 = &ready.staged.sha256;
    let target_id = match row.action {
        None => {
            if repository::hash_in_library(connection, sha256).map_err(persistence)? {
                return Err(ImportItemError::new(
                    ImportItemErrorCode::DuplicateDecisionRequired,
                    "The Library already has this file. Choose what to do with it.",
                ));
            }
            return Ok(Plan::NewModel);
        }
        Some(DuplicateAction::AddAnother) => return Ok(Plan::NewModel),
        Some(DuplicateAction::UseExisting | DuplicateAction::AddRevision) => {
            row.target_model_id.as_deref().unwrap_or_default()
        }
    };
    let target = repository::load_model(connection, target_id)
        .map_err(persistence)?
        .ok_or_else(|| {
            ImportItemError::not_found(target_id, "The chosen Model no longer exists.")
        })?;
    let stale = row
        .target_expected_revision
        .is_some_and(|expected| expected != target.revision);
    if row.action == Some(DuplicateAction::UseExisting) {
        if !repository::model_holds_hash(connection, target_id, sha256).map_err(persistence)? {
            return Err(ImportItemError::validation(
                "targetModelId",
                "The chosen Model doesn't hold this file's content.",
            ));
        }
        if stale {
            return Err(ImportItemError::conflict(target_id));
        }
        return Ok(Plan::Reuse {
            target_id: target_id.to_string(),
        });
    }
    // D14: `addRevision` needs a managed Model of the same format.
    if target.storage_mode == StorageMode::Linked {
        return Err(ImportItemError::validation(
            "targetModelId",
            "A linked Model takes new revisions only from its own source.",
        ));
    }
    if target.format != ready.format {
        return Err(ImportItemError::validation(
            "targetModelId",
            "A revision must have the same format as its Model.",
        ));
    }
    if stale {
        return Err(ImportItemError::conflict(target_id));
    }
    let current =
        repository::current_revision_sha256(connection, target_id).map_err(persistence)?;
    if current.as_deref() == Some(sha256.as_str()) {
        return Ok(Plan::Reuse {
            target_id: target_id.to_string(),
        });
    }
    Ok(Plan::NewRevision {
        target_id: target_id.to_string(),
    })
}

/// A repository error from the commit that the plan didn't already turn
/// into an item error (P5/P7: typed `NOT_FOUND`, `CONFLICT`, and
/// `VALIDATION`).
fn repository_rejection(error: RepositoryError) -> ImportItemError {
    match error {
        RepositoryError::NotFound { entity_id } => ImportItemError::not_found(
            &entity_id,
            "An item this import refers to no longer exists.",
        ),
        RepositoryError::Conflict { entity_id, .. } => ImportItemError::conflict(&entity_id),
        RepositoryError::Validation { field_path } => {
            ImportItemError::validation(field_path, "The submitted value is invalid.")
        }
        _ => ImportItemError::persistence(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcomes_and_actions_use_the_spec_spellings() {
        assert_eq!(
            serde_json::to_value([
                ImportOutcome::Imported,
                ImportOutcome::RevisionAdded,
                ImportOutcome::ReusedExisting,
                ImportOutcome::Rejected,
                ImportOutcome::Cancelled,
            ])
            .unwrap(),
            serde_json::json!([
                "imported",
                "revisionAdded",
                "reusedExisting",
                "rejected",
                "cancelled"
            ])
        );
        assert_eq!(
            serde_json::to_value([
                DuplicateAction::UseExisting,
                DuplicateAction::AddAnother,
                DuplicateAction::AddRevision,
            ])
            .unwrap(),
            serde_json::json!(["useExisting", "addAnother", "addRevision"])
        );
    }

    #[test]
    fn a_request_without_the_optional_fields_deserializes() {
        let request: ImportItemRequest = serde_json::from_value(serde_json::json!({
            "fileIndex": 0,
            "name": "Cube",
            "projectIds": [],
            "storageMode": "managed",
            "acknowledgeUnsupported": false,
        }))
        .unwrap();
        assert_eq!(request.duplicate_action, None);
        assert_eq!(request.target_model_id, None);
        assert_eq!(request.target_expected_revision, None);
    }

    /// Task 6 review M1: operations that start together must not both pass
    /// the "is another operation running?" check. Exactly one runs; every
    /// other one is `CONFLICT` at once rather than waiting and running
    /// after it.
    #[test]
    fn operations_that_start_together_run_one_and_conflict_the_rest() {
        use std::sync::mpsc;
        use std::sync::{Arc, Barrier};
        use std::time::Duration;

        const RACERS: usize = 8;
        for _ in 0..200 {
            let ledger = Arc::new(ImportLedger::default());
            let start = Arc::new(Barrier::new(RACERS));
            let (outcomes, results) = mpsc::channel();
            let (release, released) = mpsc::channel::<()>();
            let released = Arc::new(Mutex::new(released));
            let threads: Vec<_> = (0..RACERS)
                .map(|racer| {
                    let (ledger, start, outcomes, released) = (
                        Arc::clone(&ledger),
                        Arc::clone(&start),
                        outcomes.clone(),
                        Arc::clone(&released),
                    );
                    std::thread::spawn(move || {
                        start.wait();
                        match ledger.begin(&format!("op-{racer}")) {
                            Ok(run) => {
                                outcomes.send(true).unwrap();
                                // Hold the run until every racer reported.
                                let _ = released.lock().unwrap().recv();
                                drop(run);
                            }
                            Err(error) => {
                                assert_eq!(
                                    error.code,
                                    crate::contracts::command::ErrorCode::Conflict
                                );
                                outcomes.send(false).unwrap();
                            }
                        }
                    })
                })
                .collect();
            let reported: Vec<bool> = (0..RACERS)
                .map_while(|_| results.recv_timeout(Duration::from_secs(2)).ok())
                .collect();
            drop(release);
            for thread in threads {
                thread.join().unwrap();
            }
            assert_eq!(
                reported.len(),
                RACERS,
                "a racer waited for the running import instead of conflicting"
            );
            assert_eq!(reported.iter().filter(|ran| **ran).count(), 1);
        }
    }

    #[test]
    fn a_repeat_of_the_running_operation_waits_for_it_to_finish() {
        use std::sync::mpsc;
        use std::sync::Arc;
        use std::time::Duration;

        let ledger = Arc::new(ImportLedger::default());
        let run = ledger.begin("op-a").unwrap();
        let (started, finished) = mpsc::channel();
        let repeat = {
            let ledger = Arc::clone(&ledger);
            std::thread::spawn(move || {
                let _run = ledger.begin("op-a").unwrap();
                started.send(()).unwrap();
            })
        };
        assert!(
            finished.recv_timeout(Duration::from_millis(100)).is_err(),
            "the repeat ran alongside the first run"
        );
        drop(run);
        finished.recv_timeout(Duration::from_secs(2)).unwrap();
        repeat.join().unwrap();
    }

    #[test]
    fn another_operation_conflicts_while_one_runs_and_the_same_one_replays() {
        let ledger = ImportLedger::default();
        let run = ledger.begin("op-a").unwrap();
        let conflict = ledger.begin("op-b").map(|_| ()).unwrap_err();
        assert_eq!(
            conflict.code,
            crate::contracts::command::ErrorCode::Conflict
        );
        let result = ImportItemResult::cancelled(3);
        ledger.record("op-a", &result);
        drop(run);

        assert_eq!(ledger.recorded("op-a", 3), Some(result));
        assert_eq!(ledger.recorded("op-b", 3), None);
        assert!(ledger.begin("op-b").is_ok(), "the run ended");
        assert!(ledger.all_committed([3]));
        assert!(!ledger.all_committed([3, 4]));
    }
}
