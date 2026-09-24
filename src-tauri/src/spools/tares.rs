//! D3: reusable, named empty-spool tares (`spool_tares`). Names are unique
//! case-insensitively (`spool_tares_name`, migration 0004); deleting a tare
//! sets `spools.tare_id = NULL` for every Spool that referenced it
//! (`ON DELETE SET NULL`) without touching any past ledger row's `tareMg`
//! snapshot.
//!
//! Every function here takes the caller's `&Transaction`, matching
//! `repository`/`ledger` — a higher-level write (e.g. a future
//! `TareManagerDialog` command) opens the transaction and composes these.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;

use super::weight;

/// One reusable tare (D3).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Tare.ts")]
pub struct Tare {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub name: String,
    #[ts(type = "number")]
    pub weight_mg: i64,
    pub created_at: String,
    pub updated_at: String,
}

const COLUMNS: &str = "id, revision, name, weight_mg, created_at, updated_at";

pub fn generate_id() -> String {
    format!("tar-{}", uuid::Uuid::new_v4())
}

pub fn get(tx: &Transaction<'_>, id: &str) -> Result<Option<Tare>, StorageError> {
    tx.query_row(
        &format!("SELECT {COLUMNS} FROM spool_tares WHERE id = ?1"),
        [id],
        decode,
    )
    .optional()
    .map_err(StorageError::from)
}

/// Every tare, case-insensitive name order — the order a tare picker shows
/// them in.
pub fn list(tx: &Transaction<'_>) -> Result<Vec<Tare>, StorageError> {
    let mut statement = tx.prepare(&format!(
        "SELECT {COLUMNS} FROM spool_tares ORDER BY lower(name)"
    ))?;
    let rows = statement
        .query_map([], decode)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn create(tx: &Transaction<'_>, name: &str, weight_mg: i64) -> Result<Tare, RepositoryError> {
    let name = validated_name(name)?;
    validate_weight(weight_mg)?;
    if name_taken(tx, &name, None)? {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    let now = now_rfc3339();
    let tare = Tare {
        id: generate_id(),
        revision: 1,
        name,
        weight_mg,
        created_at: now.clone(),
        updated_at: now,
    };
    tx.execute(
        "INSERT INTO spool_tares(id, revision, name, weight_mg, created_at, updated_at)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
        params![
            tare.id,
            tare.revision,
            tare.name,
            tare.weight_mg,
            tare.created_at,
            tare.updated_at
        ],
    )?;
    Ok(tare)
}

pub fn update(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    name: &str,
    weight_mg: i64,
) -> Result<Tare, RepositoryError> {
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    let name = validated_name(name)?;
    validate_weight(weight_mg)?;
    let mut tare = get(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })?;
    if tare.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: tare.revision,
        });
    }
    if name_taken(tx, &name, Some(id))? {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    tare.name = name;
    tare.weight_mg = weight_mg;
    tare.revision += 1;
    tare.updated_at = now_rfc3339();
    tx.execute(
        "UPDATE spool_tares SET name = ?2, weight_mg = ?3, revision = ?4, updated_at = ?5
         WHERE id = ?1",
        params![
            tare.id,
            tare.name,
            tare.weight_mg,
            tare.revision,
            tare.updated_at
        ],
    )?;
    Ok(tare)
}

/// Deletes tare `id` after the revision check. Returns the deleted tare
/// and the ids of the Spools whose default tare it was. Those Spools lose
/// their `tareId`, so each gets a revision bump in this transaction, like
/// any other field change.
pub fn delete(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
) -> Result<(Tare, Vec<String>), RepositoryError> {
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    let tare = get(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: id.to_string(),
    })?;
    if tare.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: tare.revision,
        });
    }
    let cleared_spool_ids = {
        let mut statement =
            tx.prepare("SELECT id FROM spools WHERE tare_id = ?1 ORDER BY spool_number")?;
        let ids = statement
            .query_map([id], |row| row.get::<_, String>(0))?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        ids
    };
    // `spools.tare_id REFERENCES spool_tares(id) ON DELETE SET NULL`
    // (migration 0004) clears `tareId` on every Spool that referenced this
    // tare as part of this same statement/transaction.
    tx.execute("DELETE FROM spool_tares WHERE id = ?1", [id])?;
    let now = now_rfc3339();
    for spool_id in &cleared_spool_ids {
        tx.execute(
            "UPDATE spools SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
            params![spool_id, now],
        )?;
    }
    Ok((tare, cleared_spool_ids))
}

/// D3: "Names are unique, compared case-insensitively" — 1-64 chars
/// trimmed (matching the migration's CHECK), and not already used by
/// another tare (backstopped by `spool_tares_name`, the migration's
/// `lower(name)` unique index).
fn validated_name(name: &str) -> Result<String, RepositoryError> {
    let trimmed = name.trim().to_string();
    let len = trimmed.chars().count();
    if !(1..=64).contains(&len) {
        return Err(RepositoryError::Validation { field_path: "name" });
    }
    Ok(trimmed)
}

fn validate_weight(weight_mg: i64) -> Result<(), RepositoryError> {
    if weight::TARE_MG_RANGE.contains(&weight_mg) {
        Ok(())
    } else {
        Err(RepositoryError::Validation {
            field_path: "weightMg",
        })
    }
}

fn name_taken(
    tx: &Transaction<'_>,
    name: &str,
    excluding: Option<&str>,
) -> Result<bool, StorageError> {
    let count: i64 = tx.query_row(
        "SELECT COUNT(*) FROM spool_tares WHERE lower(name) = lower(?1) AND (?2 IS NULL OR id != ?2)",
        params![name, excluding],
        |row| row.get(0),
    )?;
    Ok(count > 0)
}

fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<Tare> {
    Ok(Tare {
        id: row.get(0)?,
        revision: row.get(1)?,
        name: row.get(2)?,
        weight_mg: row.get(3)?,
        created_at: row.get(4)?,
        updated_at: row.get(5)?,
    })
}
