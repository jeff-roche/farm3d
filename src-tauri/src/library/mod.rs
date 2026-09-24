//! P4 library domain types (D1) — the Project/Model/Model Source Revision
//! vocabulary every later P4 task builds on: the Project repository (this
//! task), the content store (Task 3), format inspection (Task 4), import
//! (Tasks 5-6), commands (Tasks 7-8), and linking (Task 8).
//!
//! Rust remains persisted truth. This module holds the closed enums the
//! schema's `CHECK` constraints mirror, the persisted row shapes, the wire
//! `ModelRecord` and `ModelSourceRevisionSummary`, name validation (D1),
//! and [`LibraryServices`], the Library's runtime state in
//! `RuntimeServices`. [`LibraryServices::record_for`] and
//! [`LibraryServices::records_for`] are the one place a `ModelRecord` is
//! assembled (clarification 3, P15).

pub mod commands;
pub mod content;
pub mod events;
pub mod formats;
pub mod import;
pub mod inspection;
pub mod repository;
pub mod selection;

use std::marker::PhantomData;
use std::path::Path;
use std::sync::{Arc, OnceLock};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contracts::command::CommandError;
use crate::persistence::StorageError;

use content::ContentStore;
use events::LibraryStream;
use formats::InspectionSummary;
use selection::{ModelFileIo, SelectionRegistry};

/// The Library's runtime services, held in `RuntimeServices::library`.
/// Holds no `AppHandle`: whatever emits events passes its own (P3's
/// pattern).
pub struct LibraryServices<R: tauri::Runtime> {
    /// D4: the long-lived content store (staging, blobs, cleanup).
    pub content: Arc<ContentStore>,
    /// D17: the `library` event stream.
    pub stream: Arc<LibraryStream>,
    /// D7: live import and Locate selections.
    pub selections: Arc<SelectionRegistry>,
    /// D7: the native file picker.
    pub file_io: Arc<dyn ModelFileIo>,
    /// D15: the linked-source supervisor. Task 8 starts it; until then the
    /// slot stays empty and linked Models report `notWatched`.
    pub links: OnceLock<Arc<LinkSupervisor<R>>>,
}

impl<R: tauri::Runtime> LibraryServices<R> {
    pub fn new(content: Arc<ContentStore>, file_io: Arc<dyn ModelFileIo>) -> Self {
        Self {
            selections: Arc::new(SelectionRegistry::new(Arc::clone(&content))),
            content,
            stream: Arc::new(LibraryStream::default()),
            file_io,
            links: OnceLock::new(),
        }
    }

    /// The `ModelRecord` for `model_id`, or `None` when there is no such
    /// Model. See [`Self::records_for`].
    pub fn record_for(
        &self,
        connection: &Connection,
        model_id: &str,
    ) -> Result<Option<ModelRecord>, StorageError> {
        Ok(self
            .records_for(connection, Some(&[model_id.to_string()]))?
            .into_iter()
            .next())
    }

    /// The `ModelRecord`s for `model_ids` (every Model when `None`), ordered
    /// by name (case-insensitive), then id. Unknown ids are skipped. The
    /// stored rows come from `connection`; `link.watchMode` is the link
    /// supervisor's runtime state (clarification 3), `notWatched` while no
    /// supervisor is running.
    pub fn records_for(
        &self,
        connection: &Connection,
        model_ids: Option<&[String]>,
    ) -> Result<Vec<ModelRecord>, StorageError> {
        let views = repository::load_model_views(connection, model_ids)?;
        let mut memberships = match model_ids {
            None => repository::project_ids_by_model(connection)?,
            Some(_) => views
                .iter()
                .map(|view| {
                    repository::project_ids_for(connection, &view.model.id)
                        .map(|ids| (view.model.id.clone(), ids))
                })
                .collect::<Result<_, _>>()?,
        };
        Ok(views
            .into_iter()
            .map(|view| {
                let project_ids = memberships.remove(&view.model.id).unwrap_or_default();
                let mut record = view.into_record(project_ids);
                self.apply_watch_mode(&mut record);
                record
            })
            .collect())
    }

    /// Sets `record.link.watchMode` from the link supervisor.
    pub fn apply_watch_mode(&self, record: &mut ModelRecord) {
        if let Some(link) = record.link.as_mut() {
            link.watch_mode = self
                .links
                .get()
                .map_or(WatchMode::NotWatched, |links| links.watch_mode(&record.id));
        }
    }

    /// D13/D15: starts following a newly linked Model's source after its
    /// import commits. This is the one call site Task 8's supervisor fills
    /// in (P14); until the supervisor is running it does nothing.
    ///
    /// Returns the warning to append to the import result, if any.
    pub fn follow_link(&self, model_id: &str, linked_path: &Path) -> Option<ImportWarning> {
        let links = self.links.get()?;
        let _mode = links.register(model_id, linked_path);
        // TODO(Task 8, P14): return `WATCH_UNAVAILABLE` when registration
        // fell back to polling for this directory (but not when the global
        // policy is `PollOnly`).
        None
    }
}

/// Placeholder for D15's `LinkSupervisor`, which Task 8 implements. It
/// exists now so `LibraryServices` has its final shape, and its two
/// methods are the calls Task 6 already makes.
pub struct LinkSupervisor<R: tauri::Runtime> {
    _runtime: PhantomData<fn() -> R>,
}

impl<R: tauri::Runtime> LinkSupervisor<R> {
    /// Starts following `linked_path` for `model_id` (P14). Task 8.
    pub fn register(&self, _model_id: &str, _linked_path: &Path) -> WatchMode {
        WatchMode::NotWatched
    }

    /// How `model_id`'s source is being followed right now. Task 8.
    pub fn watch_mode(&self, _model_id: &str) -> WatchMode {
        WatchMode::NotWatched
    }
}

/// D1: a Model's format, fixed at creation. Matches
/// `library_models`/`model_source_revisions`'s `format` `CHECK`.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[ts(export_to = "domain/ModelFormat.ts")]
pub enum ModelFormat {
    #[serde(rename = "stl")]
    Stl,
    #[serde(rename = "3mf")]
    ThreeMf,
    #[serde(rename = "gcode")]
    Gcode,
}

/// D5: whether a Model's bytes live in farm3d's managed content store or
/// are followed from a linked file on disk.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StorageMode.ts")]
pub enum StorageMode {
    Managed,
    Linked,
}

/// D15/D16: a linked Model's last-observed source condition.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SourceState.ts")]
pub enum SourceState {
    Ok,
    Missing,
    Unreadable,
    NotAFile,
    InvalidContent,
    Changing,
}

/// D15: how a linked Model's source is followed right now. Runtime state
/// from the link supervisor, merged into every `ModelRecord`; never stored.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/WatchMode.ts")]
pub enum WatchMode {
    Watching,
    Polling,
    NotWatched,
}

/// D2: a Model Source Revision's provenance.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/RevisionOrigin.ts")]
pub enum RevisionOrigin {
    Import,
    LinkedChange,
    Relocate,
    AddedRevision,
}

/// A non-blocking import note: the file is still imported. Messages carry
/// basenames and package part names only, never a full path.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportWarning.ts")]
pub struct ImportWarning {
    pub code: ImportWarningCode,
    pub message: String,
}

impl ImportWarning {
    pub fn new(code: ImportWarningCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

/// Spec §Commands `ImportWarningCode`. Format inspection produces
/// `EXTENSION_MISMATCH`, `TRAILING_BYTES`, `LONG_LINE`, and
/// `THUMBNAIL_SKIPPED`; import and linking produce the others.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "command/ImportWarningCode.ts"
)]
pub enum ImportWarningCode {
    ExtensionMismatch,
    TrailingBytes,
    LongLine,
    DuplicateName,
    WatchUnavailable,
    ThumbnailSkipped,
}

/// D1: a Project as the UI lists it. `model_count` is the number of
/// `project_models` rows for this Project (membership is the whole of
/// Project membership — there is no stored list of Model ids here).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ProjectRecord.ts")]
pub struct ProjectRecord {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub name: String,
    #[ts(type = "number")]
    pub model_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// `ModelRecord.link`: a linked Model's source. `path` is the one full
/// path that crosses to the UI (D6), so the user can recognise and recover
/// the source.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ModelLink.ts")]
pub struct ModelLink {
    pub path: String,
    pub state: SourceState,
    pub checked_at: Option<String>,
    pub watch_mode: WatchMode,
}

/// Spec §Domain types: a Model Source Revision as lists show it.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/ModelSourceRevisionSummary.ts"
)]
pub struct ModelSourceRevisionSummary {
    pub id: String,
    pub model_id: String,
    #[ts(type = "number")]
    pub sequence: i64,
    pub sha256: String,
    #[ts(type = "number")]
    pub size_bytes: i64,
    pub format: ModelFormat,
    pub origin: RevisionOrigin,
    pub source_file_name: String,
    pub captured_at: String,
    pub has_thumbnail: bool,
    pub summary: InspectionSummary,
}

/// D1: a Model as the frontend sees it. `projectIds` is ordered by Project
/// name (`[]` is Unfiled), and `currentRevision` is the highest `sequence`.
/// Built only by [`LibraryServices::records_for`].
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ModelRecord.ts")]
pub struct ModelRecord {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub name: String,
    pub project_ids: Vec<String>,
    pub format: ModelFormat,
    pub storage_mode: StorageMode,
    pub link: Option<ModelLink>,
    pub current_revision: ModelSourceRevisionSummary,
    #[ts(type = "number")]
    pub revision_count: i64,
    pub created_at: String,
    pub updated_at: String,
}

/// D1/D5: the full persisted `library_models` row. Not wire-exported —
/// [`ModelRecord`] derives the frontend's shape from this plus
/// `project_ids_for`/`project_ids_by_model` and the link supervisor's
/// runtime state (clarification 3).
#[derive(Clone, Debug, PartialEq)]
pub struct StoredModel {
    pub id: String,
    pub revision: i64,
    pub name: String,
    pub format: ModelFormat,
    pub storage_mode: StorageMode,
    pub linked_path: Option<String>,
    pub link_state: Option<SourceState>,
    pub link_checked_at: Option<String>,
    pub link_observed_size: Option<i64>,
    pub link_observed_mtime_ns: Option<i64>,
    pub link_observed_file_id: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// D2: the full persisted `model_source_revisions` row (immutable once
/// written). Not wire-exported — [`ModelSourceRevisionSummary`] (and Task
/// 7's `ModelSourceRevisionRecord`) derive the frontend's shape from this.
#[derive(Clone, Debug, PartialEq)]
pub struct StoredRevision {
    pub id: String,
    pub model_id: String,
    pub sequence: i64,
    pub content_sha256: String,
    pub size_bytes: i64,
    pub format: ModelFormat,
    pub origin: RevisionOrigin,
    pub source_file_name: String,
    pub source_path: String,
    pub source_mtime: Option<String>,
    pub captured_at: String,
    pub inspector_version: i64,
    pub inspection_json: String,
}

/// Trimmed length check shared by [`validate_project_name`] and
/// [`validate_model_name`] — the same trim-and-character-count rule as the
/// migration's `name = trim(name) AND length(name) BETWEEN 1 AND <max>`
/// `CHECK`.
fn trimmed_within(name: &str, max: usize) -> Option<String> {
    let trimmed = name.trim();
    let len = trimmed.chars().count();
    (1..=max).contains(&len).then(|| trimmed.to_string())
}

/// D1: a Project name is trimmed, 1-128 characters. Command-layer
/// validation — `repository::insert_project`/`rename_project` enforce the
/// same rule independently as `RepositoryError::Validation` (P5), since
/// tests and other callers can reach the repository directly.
pub fn validate_project_name(name: &str) -> Result<String, CommandError> {
    trimmed_within(name, 128).ok_or_else(|| {
        CommandError::validation_at("name", "The Project name must be 1-128 characters.")
    })
}

/// D1: a Model name is trimmed, 1-255 characters.
pub fn validate_model_name(name: &str) -> Result<String, CommandError> {
    trimmed_within(name, 255).ok_or_else(|| {
        CommandError::validation_at("name", "The Model name must be 1-255 characters.")
    })
}

/// `"{prefix}-{uuid-v4}"` — the id shape every library table's `CHECK (id
/// GLOB '<prefix>-*' ...)` expects (`prj-`, `mdl-`, `msr-`).
pub fn new_id(prefix: &str) -> String {
    format!("{prefix}-{}", uuid::Uuid::new_v4())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn validate_project_name_trims_and_enforces_1_to_128_characters() {
        assert_eq!(validate_project_name("  Brackets  ").unwrap(), "Brackets");
        assert!(validate_project_name("   ").is_err());
        assert!(validate_project_name(&"x".repeat(128)).is_ok());
        assert!(validate_project_name(&"x".repeat(129)).is_err());
    }

    #[test]
    fn validate_model_name_trims_and_enforces_1_to_255_characters() {
        assert_eq!(validate_model_name("  Benchy  ").unwrap(), "Benchy");
        assert!(validate_model_name("").is_err());
        assert!(validate_model_name(&"x".repeat(255)).is_ok());
        assert!(validate_model_name(&"x".repeat(256)).is_err());
    }

    #[test]
    fn new_id_uses_the_given_prefix() {
        let id = new_id("prj");
        assert!(id.starts_with("prj-"));
        assert_eq!(id.len(), 40);
    }

    /// The migration's `CHECK` constraints spell each enum's wire value as
    /// a bare (unquoted, by the time it's bound as SQL `TEXT`) string —
    /// this pins that every variant's serde rename produces exactly that
    /// string, since a later task's repository code will bind these
    /// enums straight into `TEXT` columns via `serde_json`.
    fn wire_string<T: Serialize>(value: T) -> String {
        match serde_json::to_value(value).expect("enum always serializes to a string") {
            serde_json::Value::String(text) => text,
            other => panic!("expected a JSON string, got {other:?}"),
        }
    }

    #[test]
    fn model_format_encodes_to_the_check_constraint_strings() {
        assert_eq!(wire_string(ModelFormat::Stl), "stl");
        assert_eq!(wire_string(ModelFormat::ThreeMf), "3mf");
        assert_eq!(wire_string(ModelFormat::Gcode), "gcode");
    }

    #[test]
    fn source_state_encodes_to_the_check_constraint_strings() {
        assert_eq!(wire_string(SourceState::Ok), "ok");
        assert_eq!(wire_string(SourceState::NotAFile), "notAFile");
        assert_eq!(wire_string(SourceState::InvalidContent), "invalidContent");
    }

    #[test]
    fn revision_origin_encodes_to_the_check_constraint_strings() {
        assert_eq!(wire_string(RevisionOrigin::Import), "import");
        assert_eq!(wire_string(RevisionOrigin::LinkedChange), "linkedChange");
        assert_eq!(wire_string(RevisionOrigin::AddedRevision), "addedRevision");
    }
}
