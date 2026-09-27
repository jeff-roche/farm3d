//! D14: the SQL for the slicer runtime configuration, Preparations, slice
//! operations, and Slice Revisions. Every function takes the caller's
//! `&Transaction`/`&Connection` and returns `RepositoryError` (P3/P4's
//! pattern), so callers compose them inside one `Storage::write_repo`.
//!
//! JSON columns hold serde JSON of the typed wire structs in
//! [`super`]. A stored value that no longer decodes is `CORRUPT_DATA`.

use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::library::content::mark_unreferenced_blobs;
use crate::library::formats::Producer;
use crate::library::ModelFormat;
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;
use crate::spools::{decode_enum, encode_enum};

use super::blockers::{evaluate_slice_revision_deletion, SliceRevisionDeletionBlocker};
use super::facts::{ExternalFacts, Farm3dFacts, SliceFacts};
use super::{
    ClaimedEstimates, PlateSnapshot, PreparationDocument, PreparationRecord, SliceEstimates,
    SliceFailure, SliceOperationRecord, SliceOperationState, SlicePlateRef, SliceRevisionBlob,
    SliceRevisionBlobRole, SliceRevisionKind, SliceRevisionRecord, SliceRevisionSummary,
    SliceRevisionTarget, SliceRuntimeInfo,
};

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("slicing wire types always serialize")
}

/// Decodes a JSON column; a value that doesn't decode is corrupt data.
fn from_json<T: DeserializeOwned>(index: usize, text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn decode_text_enum<T: DeserializeOwned>(index: usize, text: &str) -> rusqlite::Result<T> {
    decode_enum(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn positive_expected_revision(expected_revision: i64) -> Result<(), RepositoryError> {
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Slicer runtime configuration (D2)
// ---------------------------------------------------------------------------

/// The Conflict `entityId` for the slicer runtime configuration.
pub const RUNTIME_CONFIG_ENTITY_ID: &str = "slicerRuntime";

/// D2: this machine's slicer runtime choices. Holds full paths, so it never
/// crosses to the UI as-is. `None` means auto-discover (engine) or use the
/// engine (preset source). Before anything is saved there is no row, and
/// the configuration is the unconfigured default at `revision` 1.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct SlicerRuntimeConfig {
    pub revision: i64,
    pub engine_path: Option<String>,
    pub preset_source_path: Option<String>,
    /// `None` until the first save.
    pub updated_at: Option<String>,
}

impl SlicerRuntimeConfig {
    fn unconfigured() -> Self {
        Self {
            revision: 1,
            engine_path: None,
            preset_source_path: None,
            updated_at: None,
        }
    }
}

pub fn load_runtime_config(connection: &Connection) -> Result<SlicerRuntimeConfig, StorageError> {
    let stored = connection
        .query_row(
            "SELECT revision, engine_path, preset_source_path, updated_at
             FROM slicer_runtime_config WHERE singleton_id = 1",
            [],
            |row| {
                Ok(SlicerRuntimeConfig {
                    revision: row.get(0)?,
                    engine_path: row.get(1)?,
                    preset_source_path: row.get(2)?,
                    updated_at: Some(row.get(3)?),
                })
            },
        )
        .optional()?;
    Ok(stored.unwrap_or_else(SlicerRuntimeConfig::unconfigured))
}

/// Replaces both paths after the `expected_revision` check. An empty path
/// is `VALIDATION`; `None` clears it.
pub fn save_runtime_config(
    tx: &Transaction<'_>,
    expected_revision: i64,
    engine_path: Option<&str>,
    preset_source_path: Option<&str>,
) -> Result<SlicerRuntimeConfig, RepositoryError> {
    positive_expected_revision(expected_revision)?;
    if engine_path.is_some_and(str::is_empty) {
        return Err(RepositoryError::Validation {
            field_path: "enginePath",
        });
    }
    if preset_source_path.is_some_and(str::is_empty) {
        return Err(RepositoryError::Validation {
            field_path: "presetSourcePath",
        });
    }
    let current = load_runtime_config(tx)?;
    if current.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: RUNTIME_CONFIG_ENTITY_ID.to_string(),
            expected_revision,
            current_revision: current.revision,
        });
    }
    tx.execute(
        "INSERT INTO slicer_runtime_config(singleton_id, revision, engine_path,
                                           preset_source_path, updated_at)
         VALUES (1, ?1, ?2, ?3, ?4)
         ON CONFLICT(singleton_id) DO UPDATE SET
           revision = excluded.revision,
           engine_path = excluded.engine_path,
           preset_source_path = excluded.preset_source_path,
           updated_at = excluded.updated_at",
        params![
            current.revision + 1,
            engine_path,
            preset_source_path,
            now_rfc3339()
        ],
    )?;
    Ok(load_runtime_config(tx)?)
}

// ---------------------------------------------------------------------------
// Preparations (D5)
// ---------------------------------------------------------------------------

/// `stale` is derived here, never stored: the pinned revision is not the
/// Model's highest-`sequence` revision.
const PREPARATION_SELECT: &str = "
    SELECT p.id, p.model_id, p.source_revision_id, p.revision, p.document_json,
           p.created_at, p.updated_at,
           p.source_revision_id IS NOT (
             SELECT r.id FROM model_source_revisions r WHERE r.model_id = p.model_id
             ORDER BY r.sequence DESC LIMIT 1
           )
    FROM slice_preparations p";

fn decode_preparation(row: &rusqlite::Row<'_>) -> rusqlite::Result<PreparationRecord> {
    Ok(PreparationRecord {
        id: row.get(0)?,
        model_id: row.get(1)?,
        source_revision_id: row.get(2)?,
        revision: row.get(3)?,
        document: from_json(4, &row.get::<_, String>(4)?)?,
        created_at: row.get(5)?,
        updated_at: row.get(6)?,
        stale: row.get(7)?,
    })
}

pub fn load_preparation(
    connection: &Connection,
    id: &str,
) -> Result<Option<PreparationRecord>, StorageError> {
    Ok(connection
        .query_row(
            &format!("{PREPARATION_SELECT} WHERE p.id = ?1"),
            [id],
            decode_preparation,
        )
        .optional()?)
}

/// D5: a Model has at most one Preparation.
pub fn load_preparation_for_model(
    connection: &Connection,
    model_id: &str,
) -> Result<Option<PreparationRecord>, StorageError> {
    Ok(connection
        .query_row(
            &format!("{PREPARATION_SELECT} WHERE p.model_id = ?1"),
            [model_id],
            decode_preparation,
        )
        .optional()?)
}

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
}

/// The Model and format of Model Source Revision `id`, or `None`.
fn source_revision(
    connection: &Connection,
    id: &str,
) -> Result<Option<(String, ModelFormat, String, i64)>, StorageError> {
    Ok(connection
        .query_row(
            "SELECT model_id, format, content_sha256, size_bytes
             FROM model_source_revisions WHERE id = ?1",
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    decode_text_enum(1, &row.get::<_, String>(1)?)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, i64>(3)?,
                ))
            },
        )
        .optional()?)
}

/// D5: creates Model `model_id`'s Preparation at `revision` 1, pinned to
/// `source_revision_id`, which must be one of that Model's revisions. A
/// Model that already has a Preparation is `VALIDATION` on `modelId`.
pub fn insert_preparation(
    tx: &Transaction<'_>,
    id: &str,
    model_id: &str,
    source_revision_id: &str,
    document: &PreparationDocument,
) -> Result<PreparationRecord, RepositoryError> {
    let model_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM library_models WHERE id = ?1)",
        [model_id],
        |row| row.get(0),
    )?;
    if !model_exists {
        return Err(not_found(model_id));
    }
    match source_revision(tx, source_revision_id)? {
        Some((owner, _, _, _)) if owner == model_id => {}
        _ => {
            return Err(RepositoryError::Validation {
                field_path: "sourceRevisionId",
            })
        }
    }
    if load_preparation_for_model(tx, model_id)?.is_some() {
        return Err(RepositoryError::Validation {
            field_path: "modelId",
        });
    }
    tx.execute(
        "INSERT INTO slice_preparations(id, model_id, source_revision_id, revision, document_json,
                                        created_at, updated_at)
         VALUES (?1, ?2, ?3, 1, ?4, ?5, ?5)",
        params![
            id,
            model_id,
            source_revision_id,
            to_json(document),
            now_rfc3339()
        ],
    )?;
    load_preparation(tx, id)?.ok_or_else(|| not_found(id))
}

/// The current Preparation `id` after the `expected_revision` check (P4's
/// `rename_project` `CONFLICT` shape).
fn checked_preparation(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
) -> Result<PreparationRecord, RepositoryError> {
    positive_expected_revision(expected_revision)?;
    let current = load_preparation(tx, id)?.ok_or_else(|| not_found(id))?;
    if current.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: current.revision,
        });
    }
    Ok(current)
}

/// D5: replaces Preparation `id`'s whole document and bumps its revision.
pub fn update_preparation(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    document: &PreparationDocument,
) -> Result<PreparationRecord, RepositoryError> {
    checked_preparation(tx, id, expected_revision)?;
    tx.execute(
        "UPDATE slice_preparations
         SET document_json = ?2, revision = revision + 1, updated_at = ?3
         WHERE id = ?1",
        params![id, to_json(document), now_rfc3339()],
    )?;
    load_preparation(tx, id)?.ok_or_else(|| not_found(id))
}

/// D5 reload: pins Preparation `id` to `source_revision_id` (one of its
/// Model's revisions) with `document`, and bumps its revision.
pub fn rebase_preparation(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    source_revision_id: &str,
    document: &PreparationDocument,
) -> Result<PreparationRecord, RepositoryError> {
    let current = checked_preparation(tx, id, expected_revision)?;
    match source_revision(tx, source_revision_id)? {
        Some((owner, _, _, _)) if owner == current.model_id => {}
        _ => {
            return Err(RepositoryError::Validation {
                field_path: "sourceRevisionId",
            })
        }
    }
    tx.execute(
        "UPDATE slice_preparations
         SET source_revision_id = ?2, document_json = ?3, revision = revision + 1, updated_at = ?4
         WHERE id = ?1",
        params![id, source_revision_id, to_json(document), now_rfc3339()],
    )?;
    load_preparation(tx, id)?.ok_or_else(|| not_found(id))
}

/// Every Preparation, by Model id.
pub fn list_preparations(connection: &Connection) -> Result<Vec<PreparationRecord>, StorageError> {
    let mut statement =
        connection.prepare(&format!("{PREPARATION_SELECT} ORDER BY p.model_id, p.id"))?;
    let rows = statement
        .query_map([], decode_preparation)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Deletes Preparation `id`. Its operations cascade, and their logs are
/// marked for cleanup when nothing else refers to them.
pub fn delete_preparation(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
) -> Result<(), RepositoryError> {
    checked_preparation(tx, id, expected_revision)?;
    let logs = {
        let mut statement = tx.prepare(
            "SELECT DISTINCT log_sha256 FROM slice_operations
             WHERE preparation_id = ?1 AND log_sha256 IS NOT NULL",
        )?;
        let hashes = statement
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        hashes
    };
    tx.execute("DELETE FROM slice_preparations WHERE id = ?1", [id])?;
    mark_unreferenced_blobs(tx, &logs)?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Slice operations (D10)
// ---------------------------------------------------------------------------

/// A new operation, queued for one plate of a Preparation.
#[derive(Clone, Debug)]
pub struct NewSliceOperation {
    pub id: String,
    pub preparation_id: String,
    pub source_revision_id: String,
    pub plate: PlateSnapshot,
}

/// D10: a move out of an operation's current state.
#[derive(Clone, Debug)]
pub enum OperationTransition {
    /// `queued → running`, once OrcaSlicer is spawned. `pid_started_at` is
    /// the process's start time (Linux `/proc/<pid>/stat` field 22).
    Start { pid: i64, pid_started_at: i64 },
    /// `running → succeeded`, with the published revision.
    Succeed { slice_revision_id: String },
    /// `queued|running → failed`, keeping the log blob when there is one.
    /// A spawn failure (`spawnFailed`) fails an operation that never ran.
    Fail {
        failure: SliceFailure,
        log_sha256: Option<String>,
    },
    /// `queued|running → cancelled`, keeping the log blob when there is one.
    Cancel { log_sha256: Option<String> },
    /// `queued|running → interrupted`, when the app stopped under it.
    Interrupt,
}

impl OperationTransition {
    fn target(&self) -> SliceOperationState {
        match self {
            Self::Start { .. } => SliceOperationState::Running,
            Self::Succeed { .. } => SliceOperationState::Succeeded,
            Self::Fail { .. } => SliceOperationState::Failed,
            Self::Cancel { .. } => SliceOperationState::Cancelled,
            Self::Interrupt => SliceOperationState::Interrupted,
        }
    }

    /// Whether D10 allows this move out of `from`.
    fn allowed_from(&self, from: SliceOperationState) -> bool {
        use SliceOperationState::{Queued, Running};
        match self {
            Self::Start { .. } => from == Queued,
            Self::Succeed { .. } => from == Running,
            Self::Fail { .. } | Self::Cancel { .. } | Self::Interrupt => {
                matches!(from, Queued | Running)
            }
        }
    }
}

const OPERATION_SELECT: &str = "
    SELECT id, preparation_id, source_revision_id, plate_key, plate_snapshot_json, state,
           failure_json, slice_revision_id, queued_at, started_at, finished_at
    FROM slice_operations";

fn decode_operation(row: &rusqlite::Row<'_>) -> rusqlite::Result<SliceOperationRecord> {
    let snapshot: PlateSnapshot = from_json(4, &row.get::<_, String>(4)?)?;
    let failure = row
        .get::<_, Option<String>>(6)?
        .map(|text| from_json(6, &text))
        .transpose()?;
    Ok(SliceOperationRecord {
        id: row.get(0)?,
        preparation_id: row.get(1)?,
        source_revision_id: row.get(2)?,
        plate_key: row.get(3)?,
        plate_name: snapshot.plate.name,
        plate_index: snapshot.plate_index,
        state: decode_text_enum(5, &row.get::<_, String>(5)?)?,
        failure,
        slice_revision_id: row.get(7)?,
        queued_at: row.get(8)?,
        started_at: row.get(9)?,
        finished_at: row.get(10)?,
    })
}

pub fn load_operation(
    connection: &Connection,
    id: &str,
) -> Result<Option<SliceOperationRecord>, StorageError> {
    Ok(connection
        .query_row(
            &format!("{OPERATION_SELECT} WHERE id = ?1"),
            [id],
            decode_operation,
        )
        .optional()?)
}

/// D10: writes a `queued` operation.
pub fn insert_operation(
    tx: &Transaction<'_>,
    operation: &NewSliceOperation,
) -> Result<SliceOperationRecord, RepositoryError> {
    if operation.plate.plate_index == 0 {
        return Err(RepositoryError::Validation {
            field_path: "plateIndex",
        });
    }
    tx.execute(
        "INSERT INTO slice_operations(id, preparation_id, source_revision_id, plate_key,
                                      plate_snapshot_json, state, queued_at)
         VALUES (?1, ?2, ?3, ?4, ?5, 'queued', ?6)",
        params![
            operation.id,
            operation.preparation_id,
            operation.source_revision_id,
            operation.plate.plate.plate_key,
            to_json(&operation.plate),
            now_rfc3339(),
        ],
    )?;
    load_operation(tx, &operation.id)?.ok_or_else(|| not_found(&operation.id))
}

/// D10: moves operation `id` along `transition`. A move D10 doesn't allow
/// from the current state is [`RepositoryError::IllegalSliceTransition`]
/// and writes nothing. A move to `failed` or `cancelled` also prunes its
/// Preparation's older unpublished logs ([`prune_unpublished_logs`]); the
/// caller releases the blobs after commit.
pub fn transition_operation(
    tx: &Transaction<'_>,
    id: &str,
    transition: OperationTransition,
) -> Result<SliceOperationRecord, RepositoryError> {
    let current = load_operation(tx, id)?.ok_or_else(|| not_found(id))?;
    let to = transition.target();
    if !transition.allowed_from(current.state) {
        return Err(RepositoryError::IllegalSliceTransition {
            operation_id: id.to_string(),
            from: current.state,
            to,
        });
    }
    let now = now_rfc3339();
    let state = encode_enum(to);
    match transition {
        OperationTransition::Start {
            pid,
            pid_started_at,
        } => tx.execute(
            "UPDATE slice_operations
             SET state = ?2, pid = ?3, pid_started_at = ?4, started_at = ?5
             WHERE id = ?1",
            params![id, state, pid, pid_started_at, now],
        )?,
        OperationTransition::Succeed { slice_revision_id } => tx.execute(
            "UPDATE slice_operations
             SET state = ?2, slice_revision_id = ?3, finished_at = ?4
             WHERE id = ?1",
            params![id, state, slice_revision_id, now],
        )?,
        OperationTransition::Fail {
            failure,
            log_sha256,
        } => tx.execute(
            "UPDATE slice_operations
             SET state = ?2, failure_json = ?3, log_sha256 = ?4, finished_at = ?5
             WHERE id = ?1",
            params![id, state, to_json(&failure), log_sha256, now],
        )?,
        OperationTransition::Cancel { log_sha256 } => tx.execute(
            "UPDATE slice_operations
             SET state = ?2, log_sha256 = ?3, finished_at = ?4
             WHERE id = ?1",
            params![id, state, log_sha256, now],
        )?,
        OperationTransition::Interrupt => tx.execute(
            "UPDATE slice_operations SET state = ?2, finished_at = ?3 WHERE id = ?1",
            params![id, state, now],
        )?,
    };
    if matches!(
        to,
        SliceOperationState::Failed | SliceOperationState::Cancelled
    ) {
        prune_unpublished_logs(tx, Some(&current.preparation_id))?;
    }
    load_operation(tx, id)?.ok_or_else(|| not_found(id))
}

/// How many logs a Preparation's failed or cancelled operations keep: those
/// of the most recent operations that have one, by end time and then id.
/// An operation with no log never takes a place.
pub const KEPT_UNPUBLISHED_LOGS: u32 = 5;

/// Drops the logs of every failed or cancelled operation of Preparation
/// `preparation_id` (of every Preparation, when `None`) beyond the
/// [`KEPT_UNPUBLISHED_LOGS`] most recent that have a log. The rows stay, with no log, and
/// each dropped blob is marked for cleanup when nothing else refers to it;
/// after commit, [`ContentStore::release_unreferenced`] unlinks it.
/// Succeeded, queued, running, and interrupted operations, and Slice
/// Revision logs, are never touched. Returns how many logs were dropped.
///
/// [`ContentStore::release_unreferenced`]: crate::library::content::ContentStore::release_unreferenced
pub fn prune_unpublished_logs(
    tx: &Transaction<'_>,
    preparation_id: Option<&str>,
) -> Result<usize, StorageError> {
    let pruned = {
        let mut statement = tx.prepare(
            "SELECT id, log_sha256 FROM (
                 SELECT id, log_sha256,
                        ROW_NUMBER() OVER (
                            PARTITION BY preparation_id ORDER BY finished_at DESC, id DESC
                        ) AS recency
                 FROM slice_operations
                 WHERE state IN ('failed', 'cancelled')
                   AND log_sha256 IS NOT NULL
                   AND (?1 IS NULL OR preparation_id = ?1)
             )
             WHERE recency > ?2",
        )?;
        let rows = statement
            .query_map(params![preparation_id, KEPT_UNPUBLISHED_LOGS], |row| {
                Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    for (id, _) in &pruned {
        tx.execute(
            "UPDATE slice_operations SET log_sha256 = NULL WHERE id = ?1",
            [id],
        )?;
    }
    let hashes: Vec<String> = pruned.iter().map(|(_, sha256)| sha256.clone()).collect();
    mark_unreferenced_blobs(tx, &hashes)?;
    Ok(pruned.len())
}

/// D17's backfill: every `queued` or `running` operation, in queue order,
/// then the `recent` most recently finished ones, newest first.
pub fn list_active_and_recent_operations(
    connection: &Connection,
    recent: u32,
) -> Result<Vec<SliceOperationRecord>, StorageError> {
    let mut rows = Vec::new();
    for (filter, limit) in [
        (
            "WHERE state IN ('queued', 'running') ORDER BY queued_at, id",
            None,
        ),
        (
            "WHERE state NOT IN ('queued', 'running')
             ORDER BY finished_at DESC, queued_at DESC, id DESC LIMIT ?1",
            Some(recent),
        ),
    ] {
        let mut statement = connection.prepare(&format!("{OPERATION_SELECT} {filter}"))?;
        let found = match limit {
            Some(limit) => statement
                .query_map([limit], decode_operation)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
            None => statement
                .query_map([], decode_operation)?
                .collect::<rusqlite::Result<Vec<_>>>()?,
        };
        rows.extend(found);
    }
    Ok(rows)
}

/// An operation [`mark_active_interrupted`] stopped. A formerly `running`
/// one carries its process identity, so recovery can check whether that
/// process is still alive.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct InterruptedOperation {
    pub id: String,
    pub was_running: bool,
    pub pid: Option<i64>,
    pub pid_started_at: Option<i64>,
}

/// D10 startup recovery step 1: every `queued` or `running` operation
/// becomes `interrupted`, with `finished_at = now`.
pub fn mark_active_interrupted(
    tx: &Transaction<'_>,
) -> Result<Vec<InterruptedOperation>, StorageError> {
    let active = {
        let mut statement = tx.prepare(
            "SELECT id, state = 'running', pid, pid_started_at FROM slice_operations
             WHERE state IN ('queued', 'running') ORDER BY queued_at, id",
        )?;
        let rows = statement
            .query_map([], |row| {
                Ok(InterruptedOperation {
                    id: row.get(0)?,
                    was_running: row.get(1)?,
                    pid: row.get(2)?,
                    pid_started_at: row.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        rows
    };
    tx.execute(
        "UPDATE slice_operations SET state = 'interrupted', finished_at = ?1
         WHERE state IN ('queued', 'running')",
        [now_rfc3339()],
    )?;
    Ok(active)
}

// ---------------------------------------------------------------------------
// Slice Revisions (D1, D12-D16)
// ---------------------------------------------------------------------------

/// A farm3d Slice Revision to publish: one plate of one STL or 3MF Model
/// Source Revision. `gcode_sha256` and every blob must already have
/// `content_blobs` rows.
#[derive(Clone, Debug)]
pub struct NewFarm3dRevision {
    pub id: String,
    pub source_revision_id: String,
    pub plate: SlicePlateRef,
    pub gcode_sha256: String,
    pub gcode_size: i64,
    pub target: SliceRevisionTarget,
    pub facts: Farm3dFacts,
    pub estimates: SliceEstimates,
    pub runtime: SliceRuntimeInfo,
    pub blobs: Vec<(SliceRevisionBlobRole, String)>,
}

/// D16: an external Slice Revision over a G-code Model Source Revision. It
/// reuses that revision's content as its G-code, with no copy and no
/// `slice_revision_blobs` rows.
#[derive(Clone, Debug)]
pub struct NewExternalRevision {
    pub id: String,
    pub source_revision_id: String,
    pub facts: ExternalFacts,
    pub claimed_estimates: ClaimedEstimates,
    pub producer: Option<Producer>,
}

/// `slice_revisions.estimates_json`: the D12 estimates, plus an external
/// revision's own claims and producer (D16), which have no column of
/// their own.
#[derive(Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
struct StoredEstimates {
    /// `null` for an external revision.
    estimates: Option<SliceEstimates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    claimed_estimates: Option<ClaimedEstimates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    producer: Option<Producer>,
}

/// A validated source revision for a new Slice Revision: its Model, its
/// content hash, and its size.
fn source_for(
    tx: &Transaction<'_>,
    source_revision_id: &str,
    kind: SliceRevisionKind,
) -> Result<(String, String, i64), RepositoryError> {
    let (model_id, format, sha256, size) =
        source_revision(tx, source_revision_id)?.ok_or_else(|| not_found(source_revision_id))?;
    let accepted = match kind {
        SliceRevisionKind::Farm3d => matches!(format, ModelFormat::Stl | ModelFormat::ThreeMf),
        SliceRevisionKind::External => format == ModelFormat::Gcode,
    };
    if !accepted {
        return Err(RepositoryError::Validation {
            field_path: "sourceRevisionId",
        });
    }
    Ok((model_id, sha256, size))
}

/// D13: writes a farm3d revision and its `slice_revision_blobs` rows.
pub fn insert_farm3d_revision(
    tx: &Transaction<'_>,
    revision: &NewFarm3dRevision,
) -> Result<SliceRevisionRecord, RepositoryError> {
    if revision.plate.plate_index == 0 {
        return Err(RepositoryError::Validation {
            field_path: "plateIndex",
        });
    }
    let (model_id, _, _) = source_for(tx, &revision.source_revision_id, SliceRevisionKind::Farm3d)?;
    let facts = revision.facts.facts();
    let estimates = StoredEstimates {
        estimates: Some(revision.estimates.clone()),
        claimed_estimates: None,
        producer: None,
    };
    tx.execute(
        "INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, plate_key, plate_index,
                                     plate_name, gcode_sha256, gcode_size, target_json, facts_json,
                                     requires_manual_printer_selection, estimates_json,
                                     runtime_json, created_at)
         VALUES (?1, 'farm3d', ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14)",
        params![
            revision.id,
            model_id,
            revision.source_revision_id,
            revision.plate.plate_key,
            revision.plate.plate_index,
            revision.plate.plate_name,
            revision.gcode_sha256,
            revision.gcode_size,
            to_json(&revision.target),
            to_json(facts),
            facts.requires_manual_printer_selection(),
            to_json(&estimates),
            to_json(&revision.runtime),
            now_rfc3339(),
        ],
    )?;
    for (role, sha256) in &revision.blobs {
        tx.execute(
            "INSERT INTO slice_revision_blobs(revision_id, role, sha256) VALUES (?1, ?2, ?3)",
            params![revision.id, encode_enum(role), sha256],
        )?;
    }
    load_revision(tx, &revision.id)?.ok_or_else(|| not_found(&revision.id))
}

/// D16: writes an external revision over its G-code source revision.
pub fn insert_external_revision(
    tx: &Transaction<'_>,
    revision: &NewExternalRevision,
) -> Result<SliceRevisionRecord, RepositoryError> {
    let (model_id, sha256, size) = source_for(
        tx,
        &revision.source_revision_id,
        SliceRevisionKind::External,
    )?;
    let facts = revision.facts.facts();
    let estimates = StoredEstimates {
        estimates: None,
        claimed_estimates: Some(revision.claimed_estimates.clone()),
        producer: revision.producer.clone(),
    };
    tx.execute(
        "INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
                                     gcode_size, target_json, facts_json,
                                     requires_manual_printer_selection, estimates_json,
                                     created_at)
         VALUES (?1, 'external', ?2, ?3, ?4, ?5, 'null', ?6, ?7, ?8, ?9)",
        params![
            revision.id,
            model_id,
            revision.source_revision_id,
            sha256,
            size,
            to_json(facts),
            facts.requires_manual_printer_selection(),
            to_json(&estimates),
            now_rfc3339(),
        ],
    )?;
    load_revision(tx, &revision.id)?.ok_or_else(|| not_found(&revision.id))
}

const REVISION_SELECT: &str = "
    SELECT s.id, s.kind, s.model_id, s.source_revision_id, r.sequence, s.plate_key,
           s.plate_index, s.plate_name, s.target_json, s.facts_json,
           s.requires_manual_printer_selection, s.estimates_json, s.runtime_json, s.created_at,
           (SELECT p.name FROM printers p
            WHERE p.id = json_extract(s.target_json, '$.target.printerId'))
    FROM slice_revisions s
    JOIN model_source_revisions r ON r.id = s.source_revision_id";

/// One `REVISION_SELECT` row, decoded.
struct RevisionRow {
    summary: SliceRevisionSummary,
    target: Option<SliceRevisionTarget>,
    claimed_estimates: Option<ClaimedEstimates>,
    producer: Option<Producer>,
}

/// The label lists show for what a revision was sliced for: the target
/// Printer's current name, else the profile's catalog variant.
fn target_label(
    printer_name: Option<String>,
    target: Option<&SliceRevisionTarget>,
    facts: &SliceFacts,
) -> String {
    printer_name
        .or_else(|| target.map(|target| target.profile.catalog_ref.variant.clone()))
        .or_else(|| {
            facts
                .printer_profile
                .value()
                .map(|profile| profile.catalog_ref.variant.clone())
        })
        .filter(|label| !label.is_empty())
        .unwrap_or_else(|| "No printer profile".to_string())
}

fn decode_revision(row: &rusqlite::Row<'_>) -> rusqlite::Result<RevisionRow> {
    let kind: SliceRevisionKind = decode_text_enum(1, &row.get::<_, String>(1)?)?;
    let plate = match row.get::<_, Option<String>>(5)? {
        Some(plate_key) => Some(SlicePlateRef {
            plate_key,
            plate_index: row.get(6)?,
            plate_name: row.get(7)?,
        }),
        None => None,
    };
    let target: Option<SliceRevisionTarget> = from_json(8, &row.get::<_, String>(8)?)?;
    let facts: SliceFacts = from_json(9, &row.get::<_, String>(9)?)?;
    let stored: StoredEstimates = from_json(11, &row.get::<_, String>(11)?)?;
    let runtime = row
        .get::<_, Option<String>>(12)?
        .map(|text| from_json(12, &text))
        .transpose()?;
    let printer_name: Option<String> = row.get(14)?;
    Ok(RevisionRow {
        summary: SliceRevisionSummary {
            id: row.get(0)?,
            kind,
            model_id: row.get(2)?,
            source_revision_id: row.get(3)?,
            source_revision_sequence: row.get(4)?,
            plate,
            target_label: target_label(printer_name, target.as_ref(), &facts),
            estimates: stored.estimates,
            facts,
            requires_manual_printer_selection: row.get(10)?,
            runtime,
            created_at: row.get(13)?,
        },
        target,
        claimed_estimates: stored.claimed_estimates,
        producer: stored.producer,
    })
}

/// Model `model_id`'s Slice Revisions, newest first.
pub fn list_revision_summaries(
    connection: &Connection,
    model_id: &str,
) -> Result<Vec<SliceRevisionSummary>, StorageError> {
    let mut statement = connection.prepare(&format!(
        "{REVISION_SELECT} WHERE s.model_id = ?1 ORDER BY s.created_at DESC, s.id DESC"
    ))?;
    let rows = statement
        .query_map([model_id], decode_revision)?
        .map(|row| row.map(|row| row.summary))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every Slice Revision, newest first.
pub fn list_all_revision_summaries(
    connection: &Connection,
) -> Result<Vec<SliceRevisionSummary>, StorageError> {
    let mut statement = connection.prepare(&format!(
        "{REVISION_SELECT} ORDER BY s.created_at DESC, s.id DESC"
    ))?;
    let rows = statement
        .query_map([], decode_revision)?
        .map(|row| row.map(|row| row.summary))
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Slice Revision `id` in full, or `None`.
pub fn load_revision(
    connection: &Connection,
    id: &str,
) -> Result<Option<SliceRevisionRecord>, StorageError> {
    let Some(row) = connection
        .query_row(
            &format!("{REVISION_SELECT} WHERE s.id = ?1"),
            [id],
            decode_revision,
        )
        .optional()?
    else {
        return Ok(None);
    };
    let mut blobs = {
        let mut statement = connection.prepare(
            "SELECT b.role, c.size_bytes FROM slice_revision_blobs b
             JOIN content_blobs c ON c.sha256 = b.sha256
             WHERE b.revision_id = ?1",
        )?;
        let blobs = statement
            .query_map([id], |row| {
                Ok(SliceRevisionBlob {
                    role: decode_text_enum(0, &row.get::<_, String>(0)?)?,
                    size_bytes: row.get(1)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        blobs
    };
    blobs.sort_by_key(|blob| blob.role);
    Ok(Some(SliceRevisionRecord {
        summary: row.summary,
        target: row.target,
        claimed_estimates: row.claimed_estimates,
        producer: row.producer,
        blobs,
    }))
}

/// What [`delete_slice_revision`] removed.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct DeletedSliceRevision {
    pub id: String,
    pub model_id: String,
}

/// D14: deletes Slice Revision `id`, unless a registered
/// [`SliceRevisionDeletionBlocker`] objects (`LIFECYCLE_BLOCKED`). Its
/// blob rows cascade, an operation that produced it keeps its state and
/// loses the link, and every blob nothing else refers to is marked for
/// cleanup in the same transaction. Production passes
/// [`super::blockers::slice_revision_blocker_sources`].
pub fn delete_slice_revision(
    tx: &Transaction<'_>,
    id: &str,
    sources: &[&dyn SliceRevisionDeletionBlocker],
) -> Result<DeletedSliceRevision, RepositoryError> {
    let (model_id, gcode_sha256): (String, String) = tx
        .query_row(
            "SELECT model_id, gcode_sha256 FROM slice_revisions WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| not_found(id))?;
    let blocked = evaluate_slice_revision_deletion(id, tx, sources)?;
    if !blocked.is_empty() {
        return Err(RepositoryError::LifecycleBlocked(blocked));
    }
    let mut hashes = {
        let mut statement =
            tx.prepare("SELECT sha256 FROM slice_revision_blobs WHERE revision_id = ?1")?;
        let hashes = statement
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        hashes
    };
    hashes.push(gcode_sha256);
    hashes.sort();
    hashes.dedup();
    tx.execute("DELETE FROM slice_revisions WHERE id = ?1", [id])?;
    mark_unreferenced_blobs(tx, &hashes)?;
    Ok(DeletedSliceRevision {
        id: id.to_string(),
        model_id,
    })
}

/// Test fixtures shared by this module's tests and the library's.
#[cfg(test)]
pub(crate) mod fixtures {
    use super::*;
    use crate::slicing::facts::tests::a_profile_snapshot;
    use crate::slicing::{
        ClaimedEstimateSource, InstanceDoc, InstanceTransform, PlateDoc, RuntimeChannel,
        SliceControls, SliceTarget,
    };
    use crate::spools::MaterialFamily;

    pub(crate) const NOW: &str = "2026-01-01T00:00:00.000Z";
    pub(crate) const STL_HASH: &str =
        "1111111111111111111111111111111111111111111111111111111111111111";
    pub(crate) const STL_HASH_2: &str =
        "2222222222222222222222222222222222222222222222222222222222222222";
    pub(crate) const GCODE_SOURCE_HASH: &str =
        "3333333333333333333333333333333333333333333333333333333333333333";
    pub(crate) const OUTPUT_HASH: &str =
        "4444444444444444444444444444444444444444444444444444444444444444";
    pub(crate) const LOG_HASH: &str =
        "5555555555555555555555555555555555555555555555555555555555555555";
    pub(crate) const MANIFEST_HASH: &str =
        "6666666666666666666666666666666666666666666666666666666666666666";

    pub(crate) fn insert_blob(tx: &Transaction<'_>, sha256: &str, size: i64) {
        tx.execute(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at) VALUES (?1, ?2, ?3)",
            params![sha256, size, NOW],
        )
        .expect("blob");
    }

    pub(crate) fn insert_model(tx: &Transaction<'_>, id: &str, format: &str) {
        tx.execute(
            "INSERT INTO library_models(id, revision, name, format, storage_mode, created_at,
                                        updated_at)
             VALUES (?1, 1, 'Bracket', ?2, 'managed', ?3, ?3)",
            params![id, format, NOW],
        )
        .expect("model");
    }

    pub(crate) fn insert_source_revision(
        tx: &Transaction<'_>,
        id: &str,
        model_id: &str,
        sequence: i64,
        format: &str,
        sha256: &str,
    ) {
        tx.execute(
            "INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256, size_bytes,
               format, origin, source_file_name, source_path, captured_at, inspector_version,
               inspection_json)
             VALUES (?1, ?2, ?3, ?4, 100, ?5, 'import', 'part', '/src/part', ?6, 1, '{}')",
            params![id, model_id, sequence, sha256, format, NOW],
        )
        .expect("source revision");
    }

    /// STL Model `mdl-stl` (revision `msr-stl-1`) and G-code Model
    /// `mdl-gcode` (revision `msr-gcode-1`), plus the output, log, and
    /// manifest blobs a farm3d revision refers to.
    pub(crate) fn seed(tx: &Transaction<'_>) {
        for (sha256, size) in [
            (STL_HASH, 100),
            (GCODE_SOURCE_HASH, 300),
            (OUTPUT_HASH, 400),
            (LOG_HASH, 10),
            (MANIFEST_HASH, 20),
        ] {
            insert_blob(tx, sha256, size);
        }
        insert_model(tx, "mdl-stl", "stl");
        insert_source_revision(tx, "msr-stl-1", "mdl-stl", 1, "stl", STL_HASH);
        insert_model(tx, "mdl-gcode", "gcode");
        insert_source_revision(
            tx,
            "msr-gcode-1",
            "mdl-gcode",
            1,
            "gcode",
            GCODE_SOURCE_HASH,
        );
    }

    pub(crate) fn a_plate(key: &str, name: Option<&str>) -> PlateDoc {
        PlateDoc {
            plate_key: key.to_string(),
            name: name.map(str::to_string),
            instances: vec![InstanceDoc {
                instance_key: format!("{key}-instance"),
                object_key: 1,
                transform: InstanceTransform {
                    translate_mm: [128.0, 128.0],
                    rotate_deg: [0.0, 0.0, 90.0],
                    scale: [1.0, 1.0, 1.0],
                },
            }],
        }
    }

    pub(crate) fn a_document() -> PreparationDocument {
        PreparationDocument {
            plates: vec![a_plate("plate-a", Some("Left")), a_plate("plate-b", None)],
            target: SliceTarget::Printer {
                printer_id: "prn-a".to_string(),
            },
            process_preset: Some("0.20mm Standard".to_string()),
            filament_preset: None,
            controls: SliceControls {
                wall_loops: Some(3),
                ..SliceControls::default()
            },
        }
    }

    pub(crate) fn a_farm3d_revision(id: &str, plate_index: u32) -> NewFarm3dRevision {
        NewFarm3dRevision {
            id: id.to_string(),
            source_revision_id: "msr-stl-1".to_string(),
            plate: SlicePlateRef {
                plate_key: format!("plate-{plate_index}"),
                plate_index,
                plate_name: Some("Left".to_string()),
            },
            gcode_sha256: OUTPUT_HASH.to_string(),
            gcode_size: 400,
            target: SliceRevisionTarget {
                target: SliceTarget::Profile {
                    catalog_ref: a_profile_snapshot().catalog_ref,
                },
                profile: a_profile_snapshot(),
                machine_preset: "Elegoo Centauri Carbon 0.4 nozzle".to_string(),
                process_preset: "0.20mm Standard".to_string(),
                filament_preset: "Elegoo PLA @ECC".to_string(),
                controls: SliceControls::default(),
            },
            facts: Farm3dFacts::new(a_profile_snapshot(), 0.4, MaterialFamily::Pla, None, 1.75),
            estimates: SliceEstimates {
                print_seconds: Some(3723),
                filament_grams: Some(12.5),
                filament_mm: Some(4100.0),
                layer_count: Some(100),
                max_z_mm: Some(20.0),
                ..SliceEstimates::none()
            },
            runtime: SliceRuntimeInfo {
                engine_version: "2.4.2".to_string(),
                engine_channel: RuntimeChannel::Release,
                preset_source_version: "2.4.2".to_string(),
                preset_source_channel: RuntimeChannel::Release,
            },
            blobs: vec![
                (SliceRevisionBlobRole::Manifest, MANIFEST_HASH.to_string()),
                (SliceRevisionBlobRole::Log, LOG_HASH.to_string()),
            ],
        }
    }

    pub(crate) fn an_external_revision(id: &str) -> NewExternalRevision {
        use crate::slicing::facts::{ConfirmedFact, ConfirmedFacts};
        NewExternalRevision {
            id: id.to_string(),
            source_revision_id: "msr-gcode-1".to_string(),
            facts: ExternalFacts::new(ConfirmedFacts {
                printer_profile: ConfirmedFact::Confirmed(a_profile_snapshot()),
                nozzle_diameter_mm: ConfirmedFact::Confirmed(0.4),
                material_family: ConfirmedFact::Absent,
                material_other: None,
                filament_diameter_mm: ConfirmedFact::Absent,
            }),
            claimed_estimates: ClaimedEstimates {
                print_seconds: Some(60),
                filament_grams: None,
                filament_mm: None,
                layer_count: Some(5),
                max_z_mm: None,
                source: ClaimedEstimateSource::FileClaim,
                trusted: false,
            },
            producer: Some(Producer {
                name: "PrusaSlicer".to_string(),
                version: "2.8.0".to_string(),
            }),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::fixtures::*;
    use super::*;
    use crate::persistence::Storage;
    use crate::printers::lifecycle::{LifecycleAction, LifecycleBlocker, LifecycleBlockerCode};
    use crate::slicing::{SliceFailureCode, SliceOperationState as State, SliceTarget};

    fn seeded() -> (
        tempfile::TempDir,
        crate::persistence::MetadataRootLease,
        std::sync::Arc<Storage>,
    ) {
        let (temp, lease, storage) = crate::test_storage();
        storage
            .write(|tx| {
                seed(tx);
                Ok(())
            })
            .expect("seed");
        (temp, lease, storage)
    }

    fn count(storage: &Storage, sql: &str) -> i64 {
        storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .expect("count")
    }

    fn with_preparation(storage: &Storage) -> PreparationRecord {
        storage
            .write_repo(|tx| insert_preparation(tx, "prp-a", "mdl-stl", "msr-stl-1", &a_document()))
            .expect("preparation")
    }

    fn queue(storage: &Storage, id: &str) -> SliceOperationRecord {
        storage
            .write_repo(|tx| {
                insert_operation(
                    tx,
                    &NewSliceOperation {
                        id: id.to_string(),
                        preparation_id: "prp-a".to_string(),
                        source_revision_id: "msr-stl-1".to_string(),
                        plate: PlateSnapshot {
                            plate_index: 2,
                            plate: a_plate("plate-b", Some("Right")),
                        },
                    },
                )
            })
            .expect("operation")
    }

    fn transition(
        storage: &Storage,
        id: &str,
        transition: OperationTransition,
    ) -> Result<SliceOperationRecord, RepositoryError> {
        storage.write_repo(|tx| transition_operation(tx, id, transition))
    }

    fn a_failure() -> SliceFailure {
        SliceFailure {
            code: SliceFailureCode::ObjectsOutsidePlate,
            message: "An object is outside the printable area.".to_string(),
        }
    }

    // --- runtime configuration ---

    fn config(storage: &Storage) -> SlicerRuntimeConfig {
        storage
            .read(|connection| Ok(load_runtime_config(connection)))
            .expect("read")
            .expect("config")
    }

    #[test]
    fn runtime_config_starts_unconfigured_and_saves_with_expected_revision() {
        let (_temp, _lease, storage) = crate::test_storage();

        let initial = config(&storage);
        assert_eq!(initial, SlicerRuntimeConfig::unconfigured());

        let saved = storage
            .write_repo(|tx| save_runtime_config(tx, 1, Some("/opt/orca/orca-slicer"), None))
            .expect("save");
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.engine_path.as_deref(), Some("/opt/orca/orca-slicer"));
        assert_eq!(saved.preset_source_path, None);
        assert!(saved.updated_at.is_some());
        assert_eq!(config(&storage), saved);

        let cleared = storage
            .write_repo(|tx| save_runtime_config(tx, 2, None, Some("/opt/presets")))
            .expect("save again");
        assert_eq!(cleared.revision, 3);
        assert_eq!(cleared.engine_path, None);
        assert_eq!(cleared.preset_source_path.as_deref(), Some("/opt/presets"));
    }

    #[test]
    fn runtime_config_rejects_a_stale_revision_and_an_empty_path() {
        let (_temp, _lease, storage) = crate::test_storage();

        let stale = storage
            .write_repo(|tx| save_runtime_config(tx, 5, None, None))
            .expect_err("stale revision");
        assert!(matches!(
            stale,
            RepositoryError::Conflict {
                expected_revision: 5,
                current_revision: 1,
                ..
            }
        ));
        let empty = storage
            .write_repo(|tx| save_runtime_config(tx, 1, Some(""), None))
            .expect_err("empty path");
        assert!(matches!(
            empty,
            RepositoryError::Validation {
                field_path: "enginePath"
            }
        ));
        assert_eq!(config(&storage).revision, 1);
    }

    // --- preparations ---

    #[test]
    fn a_preparation_round_trips_and_loads_by_id_or_model() {
        let (_temp, _lease, storage) = seeded();

        let created = with_preparation(&storage);

        assert_eq!(created.id, "prp-a");
        assert_eq!(created.model_id, "mdl-stl");
        assert_eq!(created.source_revision_id, "msr-stl-1");
        assert_eq!(created.revision, 1);
        assert!(!created.stale);
        assert_eq!(created.document, a_document());
        let by_id = storage
            .read(|c| Ok(load_preparation(c, "prp-a")))
            .expect("read")
            .expect("load");
        let by_model = storage
            .read(|c| Ok(load_preparation_for_model(c, "mdl-stl")))
            .expect("read")
            .expect("load");
        assert_eq!(by_id, Some(created.clone()));
        assert_eq!(by_model, Some(created));
    }

    #[test]
    fn a_preparation_needs_its_own_models_revision_and_is_one_per_model() {
        let (_temp, _lease, storage) = seeded();

        let foreign = storage
            .write_repo(|tx| {
                insert_preparation(tx, "prp-a", "mdl-stl", "msr-gcode-1", &a_document())
            })
            .expect_err("another Model's revision");
        assert!(matches!(
            foreign,
            RepositoryError::Validation {
                field_path: "sourceRevisionId"
            }
        ));
        let missing = storage
            .write_repo(|tx| {
                insert_preparation(tx, "prp-a", "mdl-none", "msr-stl-1", &a_document())
            })
            .expect_err("unknown Model");
        assert!(matches!(missing, RepositoryError::NotFound { .. }));
        with_preparation(&storage);
        let second = storage
            .write_repo(|tx| insert_preparation(tx, "prp-b", "mdl-stl", "msr-stl-1", &a_document()))
            .expect_err("second Preparation");
        assert!(matches!(
            second,
            RepositoryError::Validation {
                field_path: "modelId"
            }
        ));
    }

    #[test]
    fn updating_a_preparation_checks_the_expected_revision() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        let mut next = a_document();
        next.plates.truncate(1);
        next.filament_preset = Some("Elegoo PLA @ECC".to_string());

        let updated = storage
            .write_repo(|tx| update_preparation(tx, "prp-a", 1, &next))
            .expect("update");
        assert_eq!(updated.revision, 2);
        assert_eq!(updated.document, next);

        let conflict = storage
            .write_repo(|tx| update_preparation(tx, "prp-a", 1, &a_document()))
            .expect_err("stale revision");
        assert!(matches!(
            conflict,
            RepositoryError::Conflict {
                ref entity_id,
                expected_revision: 1,
                current_revision: 2,
            } if entity_id == "prp-a"
        ));
        let reloaded = storage
            .read(|c| Ok(load_preparation(c, "prp-a")))
            .expect("read")
            .expect("load")
            .expect("present");
        assert_eq!(reloaded, updated, "the conflicting write changed nothing");
        let missing = storage
            .write_repo(|tx| update_preparation(tx, "prp-none", 1, &next))
            .expect_err("unknown Preparation");
        assert!(matches!(missing, RepositoryError::NotFound { .. }));
    }

    #[test]
    fn a_preparation_is_stale_once_its_model_has_a_newer_revision() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);

        storage
            .write(|tx| {
                insert_blob(tx, STL_HASH_2, 100);
                insert_source_revision(tx, "msr-stl-2", "mdl-stl", 2, "stl", STL_HASH_2);
                Ok(())
            })
            .expect("new revision");

        let reloaded = storage
            .read(|c| Ok(load_preparation(c, "prp-a")))
            .expect("read")
            .expect("load")
            .expect("present");
        assert!(reloaded.stale);
        assert_eq!(reloaded.source_revision_id, "msr-stl-1");
        assert_eq!(reloaded.revision, 1, "staleness is not a write");
    }

    #[test]
    fn deleting_a_preparation_checks_its_revision_and_cascades_its_operations() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-a");
        transition(
            &storage,
            "sop-a",
            OperationTransition::Cancel {
                log_sha256: Some(LOG_HASH.to_string()),
            },
        )
        .expect("cancel");

        let conflict = storage
            .write_repo(|tx| delete_preparation(tx, "prp-a", 7))
            .expect_err("stale revision");
        assert!(matches!(conflict, RepositoryError::Conflict { .. }));
        storage
            .write_repo(|tx| delete_preparation(tx, "prp-a", 1))
            .expect("delete");

        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM slice_preparations"),
            0
        );
        assert_eq!(count(&storage, "SELECT COUNT(*) FROM slice_operations"), 0);
        assert_eq!(
            count(
                &storage,
                &format!("SELECT COUNT(*) FROM pending_blob_cleanup WHERE sha256 = '{LOG_HASH}'")
            ),
            1,
            "the cancelled operation's log is released"
        );
    }

    // --- operations ---

    #[test]
    fn an_operation_is_queued_with_its_plate_snapshot() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);

        let queued = queue(&storage, "sop-a");

        assert_eq!(queued.state, State::Queued);
        assert_eq!(queued.plate_key, "plate-b");
        assert_eq!(queued.plate_index, 2);
        assert_eq!(queued.plate_name.as_deref(), Some("Right"));
        assert_eq!(queued.started_at, None);
        assert_eq!(queued.finished_at, None);
        assert_eq!(queued.failure, None);
    }

    #[test]
    fn an_operation_runs_and_succeeds_with_its_revision() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-a");

        let running = transition(
            &storage,
            "sop-a",
            OperationTransition::Start {
                pid: 4242,
                pid_started_at: 99,
            },
        )
        .expect("start");
        assert_eq!(running.state, State::Running);
        assert!(running.started_at.is_some());
        let pid: (i64, i64) = storage
            .read(|c| {
                c.query_row(
                    "SELECT pid, pid_started_at FROM slice_operations WHERE id = 'sop-a'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .expect("pid");
        assert_eq!(pid, (4242, 99));

        let succeeded = storage
            .write_repo(|tx| {
                insert_farm3d_revision(tx, &a_farm3d_revision("slr-a", 2))?;
                transition_operation(
                    tx,
                    "sop-a",
                    OperationTransition::Succeed {
                        slice_revision_id: "slr-a".to_string(),
                    },
                )
            })
            .expect("succeed");
        assert_eq!(succeeded.state, State::Succeeded);
        assert_eq!(succeeded.slice_revision_id.as_deref(), Some("slr-a"));
        assert!(succeeded.finished_at.is_some());
    }

    #[test]
    fn an_operation_fails_with_its_failure_and_log() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-a");
        transition(
            &storage,
            "sop-a",
            OperationTransition::Start {
                pid: 1,
                pid_started_at: 1,
            },
        )
        .expect("start");

        let failed = transition(
            &storage,
            "sop-a",
            OperationTransition::Fail {
                failure: a_failure(),
                log_sha256: Some(LOG_HASH.to_string()),
            },
        )
        .expect("fail");

        assert_eq!(failed.state, State::Failed);
        assert_eq!(failed.failure, Some(a_failure()));
        assert!(failed.finished_at.is_some());
        assert_eq!(
            count(
                &storage,
                &format!("SELECT COUNT(*) FROM slice_operations WHERE log_sha256 = '{LOG_HASH}'")
            ),
            1
        );
    }

    #[test]
    fn a_queued_operation_can_be_cancelled_or_fail_to_spawn() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-a");
        queue(&storage, "sop-b");

        let cancelled = transition(
            &storage,
            "sop-a",
            OperationTransition::Cancel { log_sha256: None },
        )
        .expect("cancel");
        let spawn_failed = transition(
            &storage,
            "sop-b",
            OperationTransition::Fail {
                failure: SliceFailure {
                    code: SliceFailureCode::SpawnFailed,
                    message: "farm3d couldn't start OrcaSlicer.".to_string(),
                },
                log_sha256: None,
            },
        )
        .expect("spawn failure");

        assert_eq!(cancelled.state, State::Cancelled);
        assert_eq!(cancelled.started_at, None);
        assert_eq!(spawn_failed.state, State::Failed);
    }

    #[test]
    fn illegal_transitions_are_rejected_and_write_nothing() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-a");

        let succeed_from_queued = transition(
            &storage,
            "sop-a",
            OperationTransition::Succeed {
                slice_revision_id: "slr-none".to_string(),
            },
        )
        .expect_err("queued can't succeed");
        assert!(matches!(
            succeed_from_queued,
            RepositoryError::IllegalSliceTransition {
                from: State::Queued,
                to: State::Succeeded,
                ..
            }
        ));

        transition(
            &storage,
            "sop-a",
            OperationTransition::Cancel { log_sha256: None },
        )
        .expect("cancel");
        for next in [
            OperationTransition::Start {
                pid: 1,
                pid_started_at: 1,
            },
            OperationTransition::Cancel { log_sha256: None },
            OperationTransition::Interrupt,
            OperationTransition::Fail {
                failure: a_failure(),
                log_sha256: None,
            },
        ] {
            let error = transition(&storage, "sop-a", next).expect_err("cancelled is terminal");
            assert!(matches!(
                error,
                RepositoryError::IllegalSliceTransition {
                    from: State::Cancelled,
                    ..
                }
            ));
        }
        queue(&storage, "sop-b");
        transition(
            &storage,
            "sop-b",
            OperationTransition::Start {
                pid: 1,
                pid_started_at: 1,
            },
        )
        .expect("start");
        let restart = transition(
            &storage,
            "sop-b",
            OperationTransition::Start {
                pid: 2,
                pid_started_at: 2,
            },
        )
        .expect_err("running can't start again");
        assert!(matches!(
            restart,
            RepositoryError::IllegalSliceTransition {
                from: State::Running,
                to: State::Running,
                ..
            }
        ));
        let missing = transition(&storage, "sop-none", OperationTransition::Interrupt)
            .expect_err("unknown operation");
        assert!(matches!(missing, RepositoryError::NotFound { .. }));
    }

    #[test]
    fn startup_recovery_interrupts_every_active_operation_only() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-queued");
        queue(&storage, "sop-running");
        queue(&storage, "sop-done");
        transition(
            &storage,
            "sop-running",
            OperationTransition::Start {
                pid: 77,
                pid_started_at: 123,
            },
        )
        .expect("start");
        transition(
            &storage,
            "sop-done",
            OperationTransition::Cancel { log_sha256: None },
        )
        .expect("cancel");

        let mut interrupted = storage.write(mark_active_interrupted).expect("recovery");
        interrupted.sort_by(|a, b| a.id.cmp(&b.id));

        assert_eq!(
            interrupted,
            vec![
                InterruptedOperation {
                    id: "sop-queued".to_string(),
                    was_running: false,
                    pid: None,
                    pid_started_at: None,
                },
                InterruptedOperation {
                    id: "sop-running".to_string(),
                    was_running: true,
                    pid: Some(77),
                    pid_started_at: Some(123),
                },
            ]
        );
        for id in ["sop-queued", "sop-running"] {
            let record = storage
                .read(|c| Ok(load_operation(c, id)))
                .expect("read")
                .expect("load")
                .expect("present");
            assert_eq!(record.state, State::Interrupted);
            assert!(record.finished_at.is_some());
        }
        let done = storage
            .read(|c| Ok(load_operation(c, "sop-done")))
            .expect("read")
            .expect("load")
            .expect("present");
        assert_eq!(done.state, State::Cancelled);
        assert!(storage
            .write(mark_active_interrupted)
            .expect("again")
            .is_empty());
    }

    // --- revisions ---

    #[test]
    fn a_farm3d_revision_round_trips_with_its_blobs() {
        let (_temp, _lease, storage) = seeded();
        let new = a_farm3d_revision("slr-a", 1);

        let record = storage
            .write_repo(|tx| insert_farm3d_revision(tx, &new))
            .expect("insert");

        let summary = &record.summary;
        assert_eq!(summary.kind, SliceRevisionKind::Farm3d);
        assert_eq!(summary.model_id, "mdl-stl");
        assert_eq!(summary.source_revision_id, "msr-stl-1");
        assert_eq!(summary.source_revision_sequence, 1);
        assert_eq!(summary.plate, Some(new.plate.clone()));
        assert_eq!(summary.target_label, "Elegoo Centauri Carbon 0.4 nozzle");
        assert_eq!(summary.estimates, Some(new.estimates.clone()));
        assert_eq!(&summary.facts, new.facts.facts());
        assert!(!summary.requires_manual_printer_selection);
        assert_eq!(summary.runtime, Some(new.runtime.clone()));
        assert_eq!(record.target, Some(new.target.clone()));
        assert_eq!(record.claimed_estimates, None);
        assert_eq!(record.producer, None);
        assert_eq!(
            record.blobs,
            vec![
                SliceRevisionBlob {
                    role: SliceRevisionBlobRole::Manifest,
                    size_bytes: 20,
                },
                SliceRevisionBlob {
                    role: SliceRevisionBlobRole::Log,
                    size_bytes: 10,
                },
            ]
        );
        let gcode: (String, i64) = storage
            .read(|c| {
                c.query_row(
                    "SELECT gcode_sha256, gcode_size FROM slice_revisions WHERE id = 'slr-a'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .expect("gcode");
        assert_eq!(gcode, (OUTPUT_HASH.to_string(), 400));
    }

    #[test]
    fn a_farm3d_revision_names_its_target_printer() {
        let (_temp, _lease, storage) = seeded();
        storage
            .write(|tx| {
                tx.execute(
                    "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                       catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                       overrides_json, created_at, updated_at)
                     VALUES ('prn-a', 1, 'Left Carbon', '', '', '', '', '', '', '{}', ?1, ?1)",
                    [NOW],
                )?;
                Ok(())
            })
            .expect("printer");
        let mut new = a_farm3d_revision("slr-a", 1);
        new.target.target = SliceTarget::Printer {
            printer_id: "prn-a".to_string(),
        };

        let record = storage
            .write_repo(|tx| insert_farm3d_revision(tx, &new))
            .expect("insert");

        assert_eq!(record.summary.target_label, "Left Carbon");
    }

    #[test]
    fn an_external_revision_reuses_its_source_content_and_keeps_claims_apart() {
        let (_temp, _lease, storage) = seeded();
        let new = an_external_revision("slr-ext");

        let record = storage
            .write_repo(|tx| insert_external_revision(tx, &new))
            .expect("insert");

        let summary = &record.summary;
        assert_eq!(summary.kind, SliceRevisionKind::External);
        assert_eq!(summary.model_id, "mdl-gcode");
        assert_eq!(summary.plate, None);
        assert_eq!(summary.runtime, None);
        assert_eq!(summary.estimates, None, "claims are never estimates");
        assert!(summary.requires_manual_printer_selection);
        assert_eq!(&summary.facts, new.facts.facts());
        assert_eq!(summary.target_label, "Elegoo Centauri Carbon 0.4 nozzle");
        assert_eq!(record.target, None);
        assert_eq!(
            record.claimed_estimates,
            Some(new.claimed_estimates.clone())
        );
        assert_eq!(record.producer, new.producer);
        assert!(record.blobs.is_empty());
        let gcode: (String, i64) = storage
            .read(|c| {
                c.query_row(
                    "SELECT gcode_sha256, gcode_size FROM slice_revisions WHERE id = 'slr-ext'",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .expect("gcode");
        assert_eq!(gcode, (GCODE_SOURCE_HASH.to_string(), 100));
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM slice_revision_blobs"),
            0
        );
    }

    #[test]
    fn each_revision_kind_needs_the_matching_source_format() {
        let (_temp, _lease, storage) = seeded();
        let mut farm3d_over_gcode = a_farm3d_revision("slr-a", 1);
        farm3d_over_gcode.source_revision_id = "msr-gcode-1".to_string();
        let mut external_over_stl = an_external_revision("slr-b");
        external_over_stl.source_revision_id = "msr-stl-1".to_string();

        for error in [
            storage
                .write_repo(|tx| insert_farm3d_revision(tx, &farm3d_over_gcode))
                .expect_err("farm3d over G-code"),
            storage
                .write_repo(|tx| insert_external_revision(tx, &external_over_stl))
                .expect_err("external over STL"),
        ] {
            assert!(matches!(
                error,
                RepositoryError::Validation {
                    field_path: "sourceRevisionId"
                }
            ));
        }
        assert_eq!(count(&storage, "SELECT COUNT(*) FROM slice_revisions"), 0);
    }

    #[test]
    fn revisions_list_newest_first_per_model() {
        let (_temp, _lease, storage) = seeded();
        storage
            .write_repo(|tx| {
                insert_farm3d_revision(tx, &a_farm3d_revision("slr-a", 1))?;
                insert_farm3d_revision(tx, &a_farm3d_revision("slr-b", 2))?;
                insert_external_revision(tx, &an_external_revision("slr-ext"))?;
                Ok(())
            })
            .expect("insert");

        let stl = storage
            .read(|c| Ok(list_revision_summaries(c, "mdl-stl")))
            .expect("read")
            .expect("list");
        let gcode = storage
            .read(|c| Ok(list_revision_summaries(c, "mdl-gcode")))
            .expect("read")
            .expect("list");

        // Newest first; equal timestamps fall back to the higher id.
        assert_eq!(
            stl.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["slr-b", "slr-a"]
        );
        assert_eq!(
            gcode.iter().map(|s| s.id.as_str()).collect::<Vec<_>>(),
            ["slr-ext"]
        );
        assert_eq!(
            gcode[0].estimates, None,
            "an external revision has no estimates"
        );
        assert!(stl.iter().all(|summary| summary.estimates.is_some()));
        let missing = storage
            .read(|c| Ok(load_revision(c, "slr-none")))
            .expect("read")
            .expect("load");
        assert_eq!(missing, None);
    }

    struct Queued;

    impl SliceRevisionDeletionBlocker for Queued {
        fn blockers(
            &self,
            slice_revision_id: &str,
            _tx: &Transaction<'_>,
        ) -> Result<Vec<LifecycleBlocker>, StorageError> {
            Ok(vec![LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::SliceRevisionsExist,
                message: format!("{slice_revision_id} is queued."),
            }])
        }
    }

    #[test]
    fn a_blocked_revision_delete_is_lifecycle_blocked_and_changes_nothing() {
        let (_temp, _lease, storage) = seeded();
        storage
            .write_repo(|tx| insert_farm3d_revision(tx, &a_farm3d_revision("slr-a", 1)))
            .expect("insert");

        let error = storage
            .write_repo(|tx| delete_slice_revision(tx, "slr-a", &[&Queued]))
            .expect_err("blocked");

        let RepositoryError::LifecycleBlocked(blockers) = error else {
            panic!("expected LifecycleBlocked, got {error:?}");
        };
        assert_eq!(blockers[0].message, "slr-a is queued.");
        assert_eq!(count(&storage, "SELECT COUNT(*) FROM slice_revisions"), 1);
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM slice_revision_blobs"),
            2
        );
    }

    #[test]
    fn deleting_a_farm3d_revision_releases_only_its_unreferenced_blobs() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-a");
        storage
            .write_repo(|tx| {
                transition_operation(
                    tx,
                    "sop-a",
                    OperationTransition::Start {
                        pid: 1,
                        pid_started_at: 1,
                    },
                )?;
                insert_farm3d_revision(tx, &a_farm3d_revision("slr-a", 1))?;
                transition_operation(
                    tx,
                    "sop-a",
                    OperationTransition::Succeed {
                        slice_revision_id: "slr-a".to_string(),
                    },
                )
            })
            .expect("publish");
        // A second revision shares the log blob, which must stay.
        let mut sharing = a_farm3d_revision("slr-b", 2);
        sharing.blobs = vec![(SliceRevisionBlobRole::Log, LOG_HASH.to_string())];
        storage
            .write_repo(|tx| insert_farm3d_revision(tx, &sharing))
            .expect("second");

        let deleted = storage
            .write_repo(|tx| {
                delete_slice_revision(
                    tx,
                    "slr-a",
                    crate::slicing::blockers::slice_revision_blocker_sources(),
                )
            })
            .expect("delete");

        assert_eq!(
            deleted,
            DeletedSliceRevision {
                id: "slr-a".to_string(),
                model_id: "mdl-stl".to_string(),
            }
        );
        let pending = |sha256: &str| {
            count(
                &storage,
                &format!("SELECT COUNT(*) FROM pending_blob_cleanup WHERE sha256 = '{sha256}'"),
            )
        };
        assert_eq!(pending(MANIFEST_HASH), 1, "only slr-a used the manifest");
        assert_eq!(pending(LOG_HASH), 0, "slr-b still uses the log");
        assert_eq!(pending(OUTPUT_HASH), 0, "slr-b still uses the G-code");
        let operation = storage
            .read(|c| Ok(load_operation(c, "sop-a")))
            .expect("read")
            .expect("load")
            .expect("present");
        assert_eq!(operation.state, State::Succeeded);
        assert_eq!(operation.slice_revision_id, None);
        let missing = storage
            .write_repo(|tx| delete_slice_revision(tx, "slr-a", &[]))
            .expect_err("already deleted");
        assert!(matches!(missing, RepositoryError::NotFound { .. }));
    }

    /// D13: a blob a Slice Revision's G-code, a revision input blob, or an
    /// operation log refers to stays referenced; once nothing does, it is
    /// released.
    #[test]
    fn blob_cleanup_keeps_blobs_slicing_rows_still_reference() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        queue(&storage, "sop-a");
        storage
            .write_repo(|tx| {
                insert_blob(tx, STL_HASH_2, 5);
                transition_operation(
                    tx,
                    "sop-a",
                    OperationTransition::Cancel {
                        log_sha256: Some(STL_HASH_2.to_string()),
                    },
                )?;
                insert_farm3d_revision(tx, &a_farm3d_revision("slr-a", 1))
            })
            .expect("references");
        let candidates = [OUTPUT_HASH, MANIFEST_HASH, LOG_HASH, STL_HASH_2].map(str::to_string);

        storage
            .write(|tx| mark_unreferenced_blobs(tx, &candidates))
            .expect("mark");

        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM pending_blob_cleanup"),
            0
        );
        for sha256 in &candidates {
            assert_eq!(
                count(
                    &storage,
                    &format!("SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{sha256}'")
                ),
                1,
                "{sha256} is still referenced"
            );
        }

        storage
            .write_repo(|tx| {
                delete_slice_revision(tx, "slr-a", &[])?;
                delete_preparation(tx, "prp-a", 1)
            })
            .expect("remove the references");
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM pending_blob_cleanup"),
            4
        );
    }

    // --- unpublished-log retention ---

    /// A distinct fake log hash for `n`.
    fn log_hash(n: u32) -> String {
        format!("{n:0>64}")
    }

    fn log_of(storage: &Storage, id: &str) -> Option<String> {
        storage
            .read(|c| {
                c.query_row(
                    "SELECT log_sha256 FROM slice_operations WHERE id = ?1",
                    [id],
                    |row| row.get(0),
                )
            })
            .expect("log")
    }

    fn pending(storage: &Storage, sha256: &str) -> bool {
        count(
            storage,
            &format!("SELECT COUNT(*) FROM pending_blob_cleanup WHERE sha256 = '{sha256}'"),
        ) == 1
    }

    /// Queues `id` on `preparation_id` and fails it with log `log_hash(n)`.
    fn fail_with_log(storage: &Storage, preparation_id: &str, id: &str, n: u32) {
        storage
            .write_repo(|tx| {
                insert_blob(tx, &log_hash(n), 10);
                let source_revision_id = load_preparation(tx, preparation_id)?
                    .expect("preparation")
                    .source_revision_id;
                insert_operation(
                    tx,
                    &NewSliceOperation {
                        id: id.to_string(),
                        preparation_id: preparation_id.to_string(),
                        source_revision_id,
                        plate: PlateSnapshot {
                            plate_index: 1,
                            plate: a_plate("plate-a", None),
                        },
                    },
                )?;
                transition_operation(
                    tx,
                    id,
                    OperationTransition::Fail {
                        failure: a_failure(),
                        log_sha256: Some(log_hash(n)),
                    },
                )
            })
            .expect("fail");
    }

    #[test]
    fn a_sixth_unpublished_log_drops_the_oldest_and_marks_its_blob() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        storage
            .write_repo(|tx| {
                insert_blob(tx, STL_HASH_2, 100);
                insert_model(tx, "mdl-stl-b", "stl");
                insert_source_revision(tx, "msr-stl-b", "mdl-stl-b", 1, "stl", STL_HASH_2);
                insert_preparation(tx, "prp-b", "mdl-stl-b", "msr-stl-b", &a_document())
            })
            .expect("second preparation");
        // prp-b's one log is older than all of prp-a's.
        fail_with_log(&storage, "prp-b", "sop-other", 90);
        for n in 1..=5 {
            fail_with_log(&storage, "prp-a", &format!("sop-{n}"), n);
        }
        // Ending times win over ids: sop-3 ended first.
        storage
            .write(|tx| {
                tx.execute(
                    "UPDATE slice_operations SET finished_at = '2020-01-01T00:00:00.000Z'
                     WHERE id IN ('sop-3', 'sop-other')",
                    [],
                )?;
                Ok(())
            })
            .expect("backdate");
        for n in 1..=5 {
            assert_eq!(
                log_of(&storage, &format!("sop-{n}")),
                Some(log_hash(n)),
                "five logs are all kept"
            );
        }

        fail_with_log(&storage, "prp-a", "sop-6", 6);

        assert_eq!(log_of(&storage, "sop-3"), None, "the oldest is pruned");
        for n in [1, 2, 4, 5, 6] {
            assert_eq!(log_of(&storage, &format!("sop-{n}")), Some(log_hash(n)));
        }
        assert!(pending(&storage, &log_hash(3)), "its blob is marked");
        assert_eq!(
            count(
                &storage,
                &format!(
                    "SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{}'",
                    log_hash(3)
                )
            ),
            0
        );
        let pruned = storage
            .read(|c| Ok(load_operation(c, "sop-3")))
            .expect("read")
            .expect("load")
            .expect("the row stays");
        assert_eq!(pruned.state, State::Failed);
        assert_eq!(pruned.failure, Some(a_failure()));
        assert_eq!(
            log_of(&storage, "sop-other"),
            Some(log_hash(90)),
            "another Preparation's log is untouched"
        );
    }

    #[test]
    fn an_operation_without_a_log_never_evicts_one() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        for n in 1..=5 {
            fail_with_log(&storage, "prp-a", &format!("sop-{n}"), n);
        }
        queue(&storage, "sop-cancel");

        transition(
            &storage,
            "sop-cancel",
            OperationTransition::Cancel { log_sha256: None },
        )
        .expect("cancel");

        for n in 1..=5 {
            assert_eq!(log_of(&storage, &format!("sop-{n}")), Some(log_hash(n)));
        }
        assert_eq!(
            count(&storage, "SELECT COUNT(*) FROM pending_blob_cleanup"),
            0
        );
    }

    #[test]
    fn a_tie_in_ending_time_prunes_the_lower_id_first() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        // Six logged cancellations sharing one end time, written straight to the
        // rows so no transition prunes on the way in. The lowest id is
        // written last, so only the id can put it first.
        let ids = ["sop-b", "sop-c", "sop-d", "sop-e", "sop-f", "sop-a"];
        storage
            .write_repo(|tx| {
                for (n, id) in (1..).zip(ids) {
                    insert_blob(tx, &log_hash(n), 10);
                    insert_operation(
                        tx,
                        &NewSliceOperation {
                            id: id.to_string(),
                            preparation_id: "prp-a".to_string(),
                            source_revision_id: "msr-stl-1".to_string(),
                            plate: PlateSnapshot {
                                plate_index: 1,
                                plate: a_plate("plate-a", None),
                            },
                        },
                    )?;
                    tx.execute(
                        "UPDATE slice_operations
                         SET state = 'cancelled', log_sha256 = ?2,
                             finished_at = '2020-01-01T00:00:00.000Z'
                         WHERE id = ?1",
                        params![id, log_hash(n)],
                    )?;
                }
                Ok(())
            })
            .expect("seed");

        let pruned = storage
            .write(|tx| prune_unpublished_logs(tx, Some("prp-a")))
            .expect("prune");
        let again = storage
            .write(|tx| prune_unpublished_logs(tx, Some("prp-a")))
            .expect("prune again");

        assert_eq!(pruned, 1);
        assert_eq!(again, 0, "pruning is idempotent");
        assert_eq!(log_of(&storage, "sop-a"), None, "the lower id goes");
        assert!(pending(&storage, &log_hash(6)));
        for (n, id) in (1..).zip(&ids[..5]) {
            assert_eq!(log_of(&storage, id), Some(log_hash(n)), "{id}");
        }
    }

    #[test]
    fn pruning_leaves_succeeded_active_and_revision_logs_alone() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        // A succeeded operation whose revision's log is LOG_HASH.
        queue(&storage, "sop-ok");
        transition(
            &storage,
            "sop-ok",
            OperationTransition::Start {
                pid: 1,
                pid_started_at: 1,
            },
        )
        .expect("start");
        storage
            .write_repo(|tx| {
                insert_farm3d_revision(tx, &a_farm3d_revision("slr-a", 2))?;
                transition_operation(
                    tx,
                    "sop-ok",
                    OperationTransition::Succeed {
                        slice_revision_id: "slr-a".to_string(),
                    },
                )
            })
            .expect("succeed");
        queue(&storage, "sop-queued");
        queue(&storage, "sop-running");
        transition(
            &storage,
            "sop-running",
            OperationTransition::Start {
                pid: 2,
                pid_started_at: 2,
            },
        )
        .expect("start");
        // The oldest failure shares its log with the revision.
        storage
            .write_repo(|tx| {
                insert_operation(
                    tx,
                    &NewSliceOperation {
                        id: "sop-0".to_string(),
                        preparation_id: "prp-a".to_string(),
                        source_revision_id: "msr-stl-1".to_string(),
                        plate: PlateSnapshot {
                            plate_index: 1,
                            plate: a_plate("plate-a", None),
                        },
                    },
                )?;
                transition_operation(
                    tx,
                    "sop-0",
                    OperationTransition::Fail {
                        failure: a_failure(),
                        log_sha256: Some(LOG_HASH.to_string()),
                    },
                )
            })
            .expect("fail");
        for n in 1..=5 {
            fail_with_log(&storage, "prp-a", &format!("sop-{n}"), n);
        }

        assert_eq!(log_of(&storage, "sop-0"), None, "the oldest is pruned");
        assert!(
            !pending(&storage, LOG_HASH),
            "the revision still refers to its log"
        );
        assert_eq!(
            count(
                &storage,
                &format!("SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{LOG_HASH}'")
            ),
            1
        );
        assert_eq!(
            count(
                &storage,
                &format!(
                    "SELECT COUNT(*) FROM slice_revision_blobs
                     WHERE revision_id = 'slr-a' AND role = 'log' AND sha256 = '{LOG_HASH}'"
                )
            ),
            1
        );
        let state = |id: &str| {
            storage
                .read(|c| Ok(load_operation(c, id)))
                .expect("read")
                .expect("load")
                .expect("present")
                .state
        };
        assert_eq!(state("sop-ok"), State::Succeeded);
        assert_eq!(state("sop-queued"), State::Queued);
        assert_eq!(state("sop-running"), State::Running);
    }

    #[test]
    fn a_log_a_kept_operation_shares_is_not_released() {
        let (_temp, _lease, storage) = seeded();
        with_preparation(&storage);
        // The oldest and the newest failures logged identical text, so
        // they share one hash.
        fail_with_log(&storage, "prp-a", "sop-1", 1);
        for n in 2..=5 {
            fail_with_log(&storage, "prp-a", &format!("sop-{n}"), n);
        }
        storage
            .write_repo(|tx| {
                insert_operation(
                    tx,
                    &NewSliceOperation {
                        id: "sop-6".to_string(),
                        preparation_id: "prp-a".to_string(),
                        source_revision_id: "msr-stl-1".to_string(),
                        plate: PlateSnapshot {
                            plate_index: 1,
                            plate: a_plate("plate-a", None),
                        },
                    },
                )?;
                transition_operation(
                    tx,
                    "sop-6",
                    OperationTransition::Fail {
                        failure: a_failure(),
                        log_sha256: Some(log_hash(1)),
                    },
                )
            })
            .expect("fail");

        assert_eq!(log_of(&storage, "sop-1"), None, "the oldest is pruned");
        assert_eq!(log_of(&storage, "sop-6"), Some(log_hash(1)));
        assert!(
            !pending(&storage, &log_hash(1)),
            "a kept operation still refers to the log"
        );
        assert_eq!(
            count(
                &storage,
                &format!(
                    "SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{}'",
                    log_hash(1)
                )
            ),
            1
        );
    }

    #[test]
    fn deleting_an_external_revision_keeps_its_source_content() {
        let (_temp, _lease, storage) = seeded();
        storage
            .write_repo(|tx| insert_external_revision(tx, &an_external_revision("slr-ext")))
            .expect("insert");

        storage
            .write_repo(|tx| delete_slice_revision(tx, "slr-ext", &[]))
            .expect("delete");

        assert_eq!(count(&storage, "SELECT COUNT(*) FROM slice_revisions"), 0);
        assert_eq!(
            count(
                &storage,
                &format!("SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{GCODE_SOURCE_HASH}'")
            ),
            1,
            "the G-code Model still owns its content"
        );
    }
}
