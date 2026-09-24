//! P4 library domain types (D1) — the Project/Model/Model Source Revision
//! vocabulary every later P4 task builds on: the Project repository (this
//! task), the content store (Task 3), format inspection (Task 4), import
//! (Tasks 5-6), commands (Tasks 7-8), and linking (Task 8).
//!
//! Rust remains persisted truth. `ModelRecord` (the full wire shape with
//! `projectIds`, `link`, and revision summaries) is a later task's
//! responsibility — this module only carries what Task 2's migration and
//! Project repository need: the closed enums the schema's `CHECK`
//! constraints mirror, the persisted `Project`/`Model`/`ModelSourceRevision`
//! row shapes, and name validation (D1).

pub mod repository;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contracts::command::CommandError;

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

/// D1/D5: the full persisted `library_models` row. Not wire-exported —
/// `ModelRecord` (a later task) derives the frontend's shape from this plus
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
/// written). Not wire-exported — `ModelSourceRevisionRecord`/`Summary` (a
/// later task) derive the frontend's shape from this.
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
