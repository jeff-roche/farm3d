//! D1/D2: the Library repository — `library_projects` reads/writes, the
//! `project_models` membership primitives, and the Model, revision, and
//! thumbnail rows import (Task 6) and linking (Task 8) write. Every
//! function here takes the caller's `&Transaction`/`&Connection` rather
//! than opening its own, matching `spools::repository`/`tares` (P3's
//! pattern for library-style writes: free functions over `&Transaction`
//! that return `RepositoryError`, run through `Storage::write_repo`).

use std::collections::HashMap;
use std::path::Path;

use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};

use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;
use crate::spools::{decode_enum, encode_enum};

use super::content::{SourceStat, StagedFile};
use super::formats::{InspectOutcome, Inspection, INSPECTOR_VERSION};
use super::inspection::StagedThumbnail;
use super::{
    new_id, ImportWarning, ModelFormat, ModelLink, ModelRecord, ModelSourceRevisionSummary,
    ProjectRecord, RevisionOrigin, SourceState, StorageMode, StoredModel, StoredRevision,
    WatchMode,
};

/// D1: a Project name is unique case-insensitively. 1-128 characters,
/// trimmed — the same rule [`super::validate_project_name`] enforces at the
/// command layer, checked again here so a caller that reaches the
/// repository directly (every test in this module, and any later task that
/// doesn't route through a command) can't write an invalid name.
fn validated_name(name: &str) -> Result<String, RepositoryError> {
    let trimmed = name.trim().to_string();
    let len = trimmed.chars().count();
    if !(1..=128).contains(&len) {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    Ok(trimmed)
}

/// D1: "unique, compared case-insensitively" with full Unicode case
/// folding (`str::to_lowercase`), not SQLite's ASCII-only `lower()`. The
/// migration's `library_projects_name` unique index on `lower(name)` is
/// only a backstop against a race between this check and the write (it
/// only catches ASCII-fold collisions on its own).
pub fn project_name_taken(
    tx: &Transaction<'_>,
    name: &str,
    excluding: Option<&str>,
) -> Result<bool, StorageError> {
    let folded = name.to_lowercase();
    let mut statement =
        tx.prepare("SELECT name FROM library_projects WHERE (?1 IS NULL OR id != ?1)")?;
    let names = statement
        .query_map(params![excluding], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(names.iter().any(|other| other.to_lowercase() == folded))
}

fn decode_project_record(row: &rusqlite::Row<'_>) -> rusqlite::Result<ProjectRecord> {
    Ok(ProjectRecord {
        id: row.get(0)?,
        revision: row.get(1)?,
        name: row.get(2)?,
        model_count: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}

/// The one derivation query behind [`list_projects`] and the
/// insert/rename read-back below: `filter` is a `WHERE` clause over `p`
/// (`library_projects`), and `model_count` is a correlated `COUNT` over
/// `project_models` — the whole of Project membership (D1), so there is no
/// separate membership table to join and de-duplicate.
fn query_project_records<P: rusqlite::Params>(
    connection: &Connection,
    filter: &str,
    params: P,
) -> Result<Vec<ProjectRecord>, StorageError> {
    let query = format!(
        "SELECT p.id, p.revision, p.name,
                (SELECT COUNT(*) FROM project_models m WHERE m.project_id = p.id),
                p.created_at, p.updated_at
         FROM library_projects p
         WHERE {filter}
         ORDER BY lower(p.name), p.id"
    );
    let mut statement = connection.prepare(&query)?;
    let rows = statement
        .query_map(params, decode_project_record)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// D1's Project list, case-insensitive name order, each with its
/// membership count.
pub fn list_projects(connection: &Connection) -> Result<Vec<ProjectRecord>, StorageError> {
    query_project_records(connection, "1 = 1", [])
}

fn load_project_record(
    connection: &Connection,
    id: &str,
) -> Result<Option<ProjectRecord>, StorageError> {
    Ok(query_project_records(connection, "p.id = ?1", [id])?
        .into_iter()
        .next())
}

/// D1: creates a Project. `id` is the caller's (`new_id("prj")` in normal
/// use; a fixed value in tests), so this never allocates one itself — that
/// keeps id generation testable and lets a future import path pick a
/// stable id. Starts at `revision` 1 with no membership.
pub fn insert_project(
    tx: &Transaction<'_>,
    id: &str,
    name: &str,
) -> Result<ProjectRecord, RepositoryError> {
    let name = validated_name(name)?;
    if project_name_taken(tx, &name, None)? {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    let now = now_rfc3339();
    tx.execute(
        "INSERT INTO library_projects(id, revision, name, created_at, updated_at)
         VALUES (?1, 1, ?2, ?3, ?3)",
        params![id, name, now],
    )?;
    load_project_record(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })
}

/// D1: renames Project `id`, after the optimistic-concurrency check against
/// `expected_revision` (P2's `CONFLICT` shape) and the case-insensitive
/// uniqueness check (excluding this Project itself).
pub fn rename_project(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    name: &str,
) -> Result<ProjectRecord, RepositoryError> {
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    let name = validated_name(name)?;
    let current = load_project_record(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })?;
    if current.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: current.revision,
        });
    }
    if project_name_taken(tx, &name, Some(id))? {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    let now = now_rfc3339();
    tx.execute(
        "UPDATE library_projects SET name = ?2, revision = revision + 1, updated_at = ?3
         WHERE id = ?1",
        params![id, name, now],
    )?;
    load_project_record(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })
}

/// D1: every Project id a Model belongs to, ordered by Project name
/// (case-insensitive) then id — the order `ModelRecord.projectIds` uses.
pub fn project_ids_for(
    connection: &Connection,
    model_id: &str,
) -> Result<Vec<String>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT p.id FROM project_models m
         JOIN library_projects p ON p.id = m.project_id
         WHERE m.model_id = ?1
         ORDER BY lower(p.name), p.id",
    )?;
    let ids = statement
        .query_map([model_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// [`project_ids_for`], for every Model in one query — the list-read
/// version, so a Model list doesn't run one membership query per row
/// (N+1).
pub fn project_ids_by_model(
    connection: &Connection,
) -> Result<HashMap<String, Vec<String>>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT m.model_id, p.id FROM project_models m
         JOIN library_projects p ON p.id = m.project_id
         ORDER BY m.model_id, lower(p.name), p.id",
    )?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let mut by_model: HashMap<String, Vec<String>> = HashMap::new();
    for (model_id, project_id) in rows {
        by_model.entry(model_id).or_default().push(project_id);
    }
    Ok(by_model)
}

/// D1: whether Project `id` exists, checked up front by [`apply_membership`]
/// so an unknown `add` id fails as [`RepositoryError::NotFound`] rather than
/// as an opaque foreign-key error, and so the whole call — including any
/// other, valid, adds/removes — writes nothing (the caller's
/// `Storage::write_repo` rolls the transaction back on `Err`).
pub(crate) fn project_exists(connection: &Connection, id: &str) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM library_projects WHERE id = ?1)",
            [id],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

/// D1: the result of [`apply_membership`] — whether any `project_models`
/// row actually changed, and which Project ids were touched (added to or
/// removed from), for a caller that needs to emit `library.project.changed`
/// per gained/lost Project.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct MembershipChange {
    pub changed: bool,
    pub touched_projects: Vec<String>,
}

/// D1: applies a set of Project-membership adds and removes for one Model
/// in a single call. Membership is a set (`INSERT OR IGNORE`): adding to a
/// Project the Model already belongs to is a successful no-op. Every `add`
/// id must name an existing Project, checked before any write — an unknown
/// one is [`RepositoryError::NotFound`] and the whole call writes nothing.
/// Removing from a Project the Model doesn't belong to (or that doesn't
/// exist) is also a no-op, since `DELETE` on a missing row is harmless.
pub fn apply_membership(
    tx: &Transaction<'_>,
    model_id: &str,
    add: &[String],
    remove: &[String],
) -> Result<MembershipChange, RepositoryError> {
    for project_id in add {
        if !project_exists(tx, project_id)? {
            return Err(RepositoryError::NotFound {
                entity_id: project_id.clone(),
            });
        }
    }
    let now = now_rfc3339();
    let mut changed = false;
    let mut touched_projects = Vec::new();
    for project_id in add {
        let inserted = tx.execute(
            "INSERT OR IGNORE INTO project_models(project_id, model_id, added_at)
             VALUES (?1, ?2, ?3)",
            params![project_id, model_id, now],
        )?;
        if inserted > 0 {
            changed = true;
            touched_projects.push(project_id.clone());
        }
    }
    for project_id in remove {
        let deleted = tx.execute(
            "DELETE FROM project_models WHERE project_id = ?1 AND model_id = ?2",
            params![project_id, model_id],
        )?;
        if deleted > 0 {
            changed = true;
            touched_projects.push(project_id.clone());
        }
    }
    Ok(MembershipChange {
        changed,
        touched_projects,
    })
}

/// What `model_source_revisions.inspection_json` holds: the revision's
/// [`Inspection`] (tagged by `format`, its fields at the top level) plus the
/// inspection warnings, which `ModelSourceRevisionRecord.warnings` reports.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct StoredInspection {
    #[serde(flatten)]
    pub inspection: Inspection,
    pub warnings: Vec<ImportWarning>,
}

/// A linked Model's source as import observed it (D5, D6).
pub(crate) struct LinkObservation<'a> {
    pub path: &'a str,
    pub stat: Option<&'a SourceStat>,
}

/// A Model row to create. It starts at `revision` 1; its first revision is
/// inserted with [`insert_revision`] in the same transaction.
pub(crate) struct NewModel<'a> {
    pub id: &'a str,
    pub name: &'a str,
    pub format: ModelFormat,
    pub link: Option<LinkObservation<'a>>,
}

pub(crate) fn insert_model(
    tx: &Transaction<'_>,
    model: &NewModel<'_>,
) -> Result<(), RepositoryError> {
    let now = now_rfc3339();
    let (storage_mode, link_state, checked_at) = match model.link {
        Some(_) => (
            StorageMode::Linked,
            Some(encode_enum(SourceState::Ok)),
            Some(now.as_str()),
        ),
        None => (StorageMode::Managed, None, None),
    };
    let path = model.link.as_ref().map(|link| link.path);
    let stat = model.link.as_ref().and_then(|link| link.stat);
    let size = stat
        .map(|stat| i64::try_from(stat.size))
        .transpose()
        .map_err(|_| RepositoryError::Validation {
            field_path: "sizeBytes",
        })?;
    tx.execute(
        "INSERT INTO library_models (
            id, revision, name, format, storage_mode, linked_path, link_state, link_checked_at,
            link_observed_size, link_observed_mtime_ns, link_observed_file_id,
            created_at, updated_at
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
        params![
            model.id,
            model.name,
            encode_enum(model.format),
            encode_enum(storage_mode),
            path,
            link_state,
            checked_at,
            size,
            stat.and_then(SourceStat::modified_ns),
            stat.and_then(|stat| stat.file_id.as_deref()),
            now,
        ],
    )?;
    Ok(())
}

/// D1: bumps a Model's `revision` after a write that changed it (a new
/// revision, or membership).
pub(crate) fn bump_model_revision(
    tx: &Transaction<'_>,
    model_id: &str,
) -> Result<(), RepositoryError> {
    let updated = tx.execute(
        "UPDATE library_models SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
        params![model_id, now_rfc3339()],
    )?;
    if updated == 0 {
        return Err(RepositoryError::NotFound {
            entity_id: model_id.to_string(),
        });
    }
    Ok(())
}

/// D2: appends a Model Source Revision for `staged` with the next
/// `sequence` (`max(sequence) + 1`, so 1 for a new Model). The content blob
/// row must exist in this transaction already (`place_and_commit` inserts
/// it). `source_path` is the absolute path read, and its basename becomes
/// `source_file_name`.
pub(crate) fn insert_revision(
    tx: &Transaction<'_>,
    model_id: &str,
    staged: &StagedFile,
    outcome: &InspectOutcome,
    origin: RevisionOrigin,
    source_path: &str,
    source_mtime: Option<&str>,
) -> Result<StoredRevision, RepositoryError> {
    let sequence: i64 = tx.query_row(
        "SELECT COALESCE(MAX(sequence), 0) + 1 FROM model_source_revisions WHERE model_id = ?1",
        [model_id],
        |row| row.get(0),
    )?;
    let size_bytes = i64::try_from(staged.size).map_err(|_| RepositoryError::Validation {
        field_path: "sizeBytes",
    })?;
    let source_file_name = Path::new(source_path)
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| source_path.to_string());
    let inspection_json = serde_json::to_string(&StoredInspection {
        inspection: outcome.inspection.clone(),
        warnings: outcome.warnings.clone(),
    })
    .map_err(|_| RepositoryError::Validation {
        field_path: "inspection",
    })?;
    let revision = StoredRevision {
        id: new_id("msr"),
        model_id: model_id.to_string(),
        sequence,
        content_sha256: staged.sha256.clone(),
        size_bytes,
        format: inspection_format(&outcome.inspection),
        origin,
        source_file_name,
        source_path: source_path.to_string(),
        source_mtime: source_mtime.map(str::to_string),
        captured_at: now_rfc3339(),
        inspector_version: INSPECTOR_VERSION,
        inspection_json,
    };
    tx.execute(
        "INSERT INTO model_source_revisions (
            id, model_id, sequence, content_sha256, size_bytes, format, origin,
            source_file_name, source_path, source_mtime, captured_at, inspector_version,
            inspection_json
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13)",
        params![
            revision.id,
            revision.model_id,
            revision.sequence,
            revision.content_sha256,
            revision.size_bytes,
            encode_enum(revision.format),
            encode_enum(revision.origin),
            revision.source_file_name,
            revision.source_path,
            revision.source_mtime,
            revision.captured_at,
            revision.inspector_version,
            revision.inspection_json,
        ],
    )?;
    Ok(revision)
}

fn inspection_format(inspection: &Inspection) -> ModelFormat {
    match inspection {
        Inspection::Stl(_) => ModelFormat::Stl,
        Inspection::ThreeMf(_) => ModelFormat::ThreeMf,
        Inspection::Gcode(_) => ModelFormat::Gcode,
    }
}

/// D12: records a revision's embedded thumbnail. Its blob row must exist in
/// this transaction already.
pub(crate) fn insert_thumbnail(
    tx: &Transaction<'_>,
    revision_id: &str,
    thumbnail: &StagedThumbnail,
) -> Result<(), RepositoryError> {
    tx.execute(
        "INSERT INTO model_revision_thumbnails (
            revision_id, source, origin_part, media_type, width, height, content_sha256
         ) VALUES (?1, 'embedded', ?2, 'image/png', ?3, ?4, ?5)",
        params![
            revision_id,
            thumbnail.origin_part,
            thumbnail.width,
            thumbnail.height,
            thumbnail.staged.sha256,
        ],
    )?;
    Ok(())
}

/// D14: whether any Model has a revision with this content.
pub(crate) fn hash_in_library(connection: &Connection, sha256: &str) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM model_source_revisions WHERE content_sha256 = ?1)",
            [sha256],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

/// D14: whether Model `model_id` has any revision with this content.
pub(crate) fn model_holds_hash(
    connection: &Connection,
    model_id: &str,
    sha256: &str,
) -> Result<bool, StorageError> {
    connection
        .query_row(
            "SELECT EXISTS(SELECT 1 FROM model_source_revisions
                           WHERE model_id = ?1 AND content_sha256 = ?2)",
            [model_id, sha256],
            |row| row.get(0),
        )
        .map_err(StorageError::from)
}

/// The content hash of Model `model_id`'s current (highest-sequence)
/// revision.
pub(crate) fn current_revision_sha256(
    connection: &Connection,
    model_id: &str,
) -> Result<Option<String>, StorageError> {
    connection
        .query_row(
            "SELECT content_sha256 FROM model_source_revisions
             WHERE model_id = ?1 ORDER BY sequence DESC LIMIT 1",
            [model_id],
            |row| row.get(0),
        )
        .optional()
        .map_err(StorageError::from)
}

const MODEL_COLUMNS: &str = "m.id, m.revision, m.name, m.format, m.storage_mode, m.linked_path,
     m.link_state, m.link_checked_at, m.link_observed_size, m.link_observed_mtime_ns,
     m.link_observed_file_id, m.created_at, m.updated_at";

/// Decodes an enum stored as its serde string, as a column conversion error
/// when the text is not a known value.
fn decode_column<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<T> {
    let text: String = row.get(index)?;
    decode_enum(&text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn decode_optional_column<T: serde::de::DeserializeOwned>(
    row: &rusqlite::Row<'_>,
    index: usize,
) -> rusqlite::Result<Option<T>> {
    match row.get::<_, Option<String>>(index)? {
        None => Ok(None),
        Some(text) => decode_enum(&text).map(Some).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
        }),
    }
}

fn decode_model(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredModel> {
    Ok(StoredModel {
        id: row.get(0)?,
        revision: row.get(1)?,
        name: row.get(2)?,
        format: decode_column(row, 3)?,
        storage_mode: decode_column(row, 4)?,
        linked_path: row.get(5)?,
        link_state: decode_optional_column(row, 6)?,
        link_checked_at: row.get(7)?,
        link_observed_size: row.get(8)?,
        link_observed_mtime_ns: row.get(9)?,
        link_observed_file_id: row.get(10)?,
        created_at: row.get(11)?,
        updated_at: row.get(12)?,
    })
}

/// The persisted `library_models` row for `model_id`.
pub fn load_model(
    connection: &Connection,
    model_id: &str,
) -> Result<Option<StoredModel>, StorageError> {
    connection
        .query_row(
            &format!("SELECT {MODEL_COLUMNS} FROM library_models m WHERE m.id = ?1"),
            [model_id],
            decode_model,
        )
        .optional()
        .map_err(StorageError::from)
}

/// A Model row with its current revision and revision count: everything
/// in a [`ModelRecord`] except membership and runtime watch state.
pub(crate) struct ModelView {
    pub model: StoredModel,
    pub current_revision: ModelSourceRevisionSummary,
    pub revision_count: i64,
}

impl ModelView {
    /// The record, with `link.watchMode` `notWatched` until the caller
    /// merges the supervisor's state.
    pub(crate) fn into_record(self, project_ids: Vec<String>) -> ModelRecord {
        let model = self.model;
        let link = model
            .linked_path
            .zip(model.link_state)
            .map(|(path, state)| ModelLink {
                path,
                state,
                checked_at: model.link_checked_at,
                watch_mode: WatchMode::NotWatched,
            });
        ModelRecord {
            id: model.id,
            revision: model.revision,
            name: model.name,
            project_ids,
            format: model.format,
            storage_mode: model.storage_mode,
            link,
            current_revision: self.current_revision,
            revision_count: self.revision_count,
            created_at: model.created_at,
            updated_at: model.updated_at,
        }
    }
}

/// [`ModelView`]s for `model_ids` (every Model when `None`) in one query,
/// ordered by name (case-insensitive), then id.
pub(crate) fn load_model_views(
    connection: &Connection,
    model_ids: Option<&[String]>,
) -> Result<Vec<ModelView>, StorageError> {
    let filter =
        model_ids.map(|ids| serde_json::to_string(ids).expect("a string list always serializes"));
    let mut statement = connection.prepare(&format!(
        "SELECT {MODEL_COLUMNS},
                r.id, r.sequence, r.content_sha256, r.size_bytes, r.format, r.origin,
                r.source_file_name, r.captured_at, r.inspection_json,
                EXISTS(SELECT 1 FROM model_revision_thumbnails t WHERE t.revision_id = r.id),
                (SELECT COUNT(*) FROM model_source_revisions c WHERE c.model_id = m.id)
         FROM library_models m
         JOIN model_source_revisions r ON r.model_id = m.id
          AND r.sequence = (SELECT MAX(s.sequence) FROM model_source_revisions s
                            WHERE s.model_id = m.id)
         WHERE ?1 IS NULL OR m.id IN (SELECT value FROM json_each(?1))
         ORDER BY lower(m.name), m.id"
    ))?;
    let views = statement
        .query_map([filter], |row| {
            let model = decode_model(row)?;
            let inspection_json: String = row.get(21)?;
            let stored: StoredInspection =
                serde_json::from_str(&inspection_json).map_err(|error| {
                    rusqlite::Error::FromSqlConversionFailure(21, Type::Text, Box::new(error))
                })?;
            let current_revision = ModelSourceRevisionSummary {
                id: row.get(13)?,
                model_id: model.id.clone(),
                sequence: row.get(14)?,
                sha256: row.get(15)?,
                size_bytes: row.get(16)?,
                format: decode_column(row, 17)?,
                origin: decode_column(row, 18)?,
                source_file_name: row.get(19)?,
                captured_at: row.get(20)?,
                has_thumbnail: row.get(22)?,
                summary: stored.inspection.summary(),
            };
            Ok(ModelView {
                model,
                current_revision,
                revision_count: row.get(23)?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(views)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn insert_project(
        storage: &crate::persistence::Storage,
        id: &str,
        name: &str,
    ) -> ProjectRecord {
        storage
            .write_repo(|tx| super::insert_project(tx, id, name))
            .expect("insert project")
    }

    /// `Storage::read`'s closure must return a plain `rusqlite::Result<T>`,
    /// but every read helper above returns `Result<T, StorageError>` (the
    /// brief's `list_projects(&Connection) -> Result<_, StorageError>`
    /// shape). Nests the call and flattens it, matching
    /// `printers::repository::list`/`get`'s precedent for calling a
    /// `StorageError`-returning free function through `Storage::read`.
    fn read<T>(
        storage: &crate::persistence::Storage,
        operation: impl FnOnce(&Connection) -> Result<T, StorageError>,
    ) -> Result<T, StorageError> {
        storage
            .read(|connection| Ok(operation(connection)))
            .and_then(|inner| inner)
    }

    /// Seeds a `library_models` row directly with raw SQL — Task 2 has no
    /// Model repository yet (that's later tasks' job), and
    /// `apply_membership` needs a real Model to reference.
    fn seed_model(storage: &crate::persistence::Storage, id: &str) {
        storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO library_models(
                        id, revision, name, format, storage_mode, created_at, updated_at
                     ) VALUES (?1, 1, 'Model', 'stl', 'managed', '2026-01-01T00:00:00.000Z', '2026-01-01T00:00:00.000Z')",
                    [id],
                )?;
                Ok(())
            })
            .expect("seed model");
    }

    #[test]
    fn insert_project_then_list_projects_returns_model_count_zero() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");

        let projects = read(&storage, super::list_projects).expect("list projects");

        assert_eq!(projects.len(), 1);
        assert_eq!(projects[0].id, "prj-a");
        assert_eq!(projects[0].name, "Miniatures");
        assert_eq!(projects[0].model_count, 0);
        assert_eq!(projects[0].revision, 1);
    }

    #[test]
    fn rename_project_with_a_stale_revision_is_a_conflict() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");

        let error = storage
            .write_repo(|tx| super::rename_project(tx, "prj-a", 7, "Terrain"))
            .expect_err("stale revision must conflict");

        assert!(matches!(
            error,
            RepositoryError::Conflict {
                entity_id,
                expected_revision: 7,
                current_revision: 1,
            } if entity_id == "prj-a"
        ));
    }

    #[test]
    fn rename_project_succeeds_and_bumps_the_revision() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");

        let renamed = storage
            .write_repo(|tx| super::rename_project(tx, "prj-a", 1, "Terrain"))
            .expect("rename");

        assert_eq!(renamed.name, "Terrain");
        assert_eq!(renamed.revision, 2);
    }

    #[test]
    fn a_unicode_case_fold_duplicate_name_is_rejected() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Ärger");

        let error = storage
            .write_repo(|tx| super::insert_project(tx, "prj-b", "ärger"))
            .expect_err("unicode case-fold duplicate must be rejected");

        assert!(matches!(
            error,
            RepositoryError::Validation { field_path: "name" }
        ));
    }

    #[test]
    fn insert_project_trims_the_name_before_writing_it() {
        let (_temp, _lease, storage) = crate::test_storage();

        let created = insert_project(&storage, "prj-a", " x ");

        assert_eq!(created.name, "x");
    }

    #[test]
    fn insert_project_rejects_a_name_that_is_blank_after_trimming() {
        let (_temp, _lease, storage) = crate::test_storage();

        let error = storage
            .write_repo(|tx| super::insert_project(tx, "prj-a", "   "))
            .expect_err("a blank name must be rejected");

        assert!(matches!(
            error,
            RepositoryError::Validation { field_path: "name" }
        ));
    }

    #[test]
    fn apply_membership_adding_an_existing_membership_is_a_no_op() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "Miniatures");
        seed_model(&storage, "mdl-a");
        storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("first add");

        let change = storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("second add is a no-op");

        assert_eq!(
            change,
            MembershipChange {
                changed: false,
                touched_projects: vec![],
            }
        );
    }

    #[test]
    fn apply_membership_add_and_remove_in_one_call_applies_both() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "A");
        insert_project(&storage, "prj-b", "B");
        seed_model(&storage, "mdl-a");
        storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("seed membership in prj-a");

        let change = storage
            .write_repo(|tx| {
                super::apply_membership(tx, "mdl-a", &["prj-b".to_string()], &["prj-a".to_string()])
            })
            .expect("add and remove");

        assert!(change.changed);
        let mut touched = change.touched_projects.clone();
        touched.sort();
        assert_eq!(touched, vec!["prj-a".to_string(), "prj-b".to_string()]);

        let ids =
            read(&storage, |conn| super::project_ids_for(conn, "mdl-a")).expect("project ids");
        assert_eq!(ids, vec!["prj-b".to_string()]);
    }

    #[test]
    fn apply_membership_with_an_unknown_project_id_writes_nothing() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_model(&storage, "mdl-a");

        let error = storage
            .write_repo(|tx| {
                super::apply_membership(tx, "mdl-a", &["prj-missing".to_string()], &[])
            })
            .expect_err("unknown project must be rejected");

        assert!(matches!(
            error,
            RepositoryError::NotFound { entity_id } if entity_id == "prj-missing"
        ));
        let ids =
            read(&storage, |conn| super::project_ids_for(conn, "mdl-a")).expect("project ids");
        assert!(ids.is_empty(), "nothing must have been written");
    }

    #[test]
    fn project_ids_for_orders_by_project_name() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-z", "Zeta");
        insert_project(&storage, "prj-a", "Alpha");
        insert_project(&storage, "prj-m", "Mid");
        seed_model(&storage, "mdl-a");
        storage
            .write_repo(|tx| {
                super::apply_membership(
                    tx,
                    "mdl-a",
                    &[
                        "prj-z".to_string(),
                        "prj-a".to_string(),
                        "prj-m".to_string(),
                    ],
                    &[],
                )
            })
            .expect("add all three");

        let ids =
            read(&storage, |conn| super::project_ids_for(conn, "mdl-a")).expect("project ids");

        assert_eq!(
            ids,
            vec![
                "prj-a".to_string(),
                "prj-m".to_string(),
                "prj-z".to_string()
            ]
        );
    }

    #[test]
    fn project_ids_by_model_matches_project_ids_for_with_one_query() {
        let (_temp, _lease, storage) = crate::test_storage();
        insert_project(&storage, "prj-a", "A");
        seed_model(&storage, "mdl-a");
        seed_model(&storage, "mdl-b");
        storage
            .write_repo(|tx| super::apply_membership(tx, "mdl-a", &["prj-a".to_string()], &[]))
            .expect("membership for mdl-a only");

        let by_model = read(&storage, super::project_ids_by_model).expect("project ids by model");

        assert_eq!(by_model.get("mdl-a"), Some(&vec!["prj-a".to_string()]));
        assert_eq!(by_model.get("mdl-b"), None);
    }

    #[test]
    fn a_stored_inspection_keeps_the_inspection_fields_at_the_top_level() {
        let stored = StoredInspection {
            inspection: Inspection::Gcode(crate::library::formats::GcodeInspection {
                producer: None,
                claims: vec![crate::library::formats::GcodeClaim {
                    key: "printer_model".to_string(),
                    value: "MK4S".to_string(),
                    line: 7,
                }],
                trusted: false,
                line_count: 12,
                command_count: 10,
                tools_used: vec![],
                relative_positioning_seen: false,
                relative_extrusion_seen: false,
                observed_bounds_mm: None,
                thumbnails: vec![],
            }),
            warnings: vec![ImportWarning::new(
                crate::library::ImportWarningCode::LongLine,
                "long",
            )],
        };

        let json = serde_json::to_value(&stored).unwrap();

        assert_eq!(json["format"], "gcode");
        assert_eq!(json["trusted"], false);
        assert_eq!(json["claims"][0]["value"], "MK4S");
        assert_eq!(json["warnings"][0]["code"], "LONG_LINE");
        let back: StoredInspection = serde_json::from_value(json).unwrap();
        assert_eq!(back, stored);
    }

    #[test]
    fn new_id_prefixed_project_id_round_trips_through_insert() {
        let (_temp, _lease, storage) = crate::test_storage();
        let id = crate::library::new_id("prj");

        let created = insert_project(&storage, &id, "Generated Id");

        assert_eq!(created.id, id);
    }
}
