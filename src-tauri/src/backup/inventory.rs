//! D5: what goes into a backup, read from the database copy (or, for
//! `backup_inventory` and the size estimate, the live Farm): the content
//! blobs, the camera snapshots a media choice selects, the media pre-pass,
//! and the sanitization of the copy.

use std::collections::{BTreeMap, BTreeSet};
use std::fs::{self, File};
use std::io;
use std::path::Path;

use rusqlite::{params, Connection};

use super::archive::{is_media_rel_path, Sha256Reader};
use super::manifest::ManifestMigration;
use super::{
    BackupContentTotals, BackupExcludedClass, BackupInventory, BackupMediaChoice,
    BackupMediaTotals, TableCount,
};
use crate::persistence::integrity::quote_identifier;

/// One `content_blobs` row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct BlobItem {
    pub sha256: String,
    pub bytes: u64,
    /// A 3MF (a Model source or a plate), already compressed: stored, not
    /// deflated (D2).
    pub stored: bool,
}

/// One selected, unpruned `camera_snapshots` row.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct MediaItem {
    pub id: String,
    pub rel_path: String,
    pub bytes: u64,
    pub sha256: String,
}

/// The selection condition for `choice` (unpruned rows only).
fn media_filter(choice: BackupMediaChoice) -> &'static str {
    match choice {
        BackupMediaChoice::None => "0",
        BackupMediaChoice::Pinned => "pruned_at IS NULL AND pinned_at IS NOT NULL",
        BackupMediaChoice::All => "pruned_at IS NULL",
    }
}

/// Every table's exact `count(*)` (SQLite's own tables excluded), by name.
pub fn table_counts(connection: &Connection) -> rusqlite::Result<BTreeMap<String, i64>> {
    let names: Vec<String> = connection
        .prepare(
            "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut counts = BTreeMap::new();
    for name in names {
        let rows: i64 = connection.query_row(
            &format!("SELECT count(*) FROM {}", quote_identifier(&name)),
            [],
            |row| row.get(0),
        )?;
        counts.insert(name, rows);
    }
    Ok(counts)
}

/// `page_count × page_size`.
pub fn database_bytes(connection: &Connection) -> rusqlite::Result<i64> {
    let pages: i64 = connection.query_row("PRAGMA page_count", [], |row| row.get(0))?;
    let page_size: i64 = connection.query_row("PRAGMA page_size", [], |row| row.get(0))?;
    Ok(pages * page_size)
}

/// Distinct `$.credentialRef` values in `printers.connection_json`.
pub fn credential_ref_count(connection: &Connection) -> rusqlite::Result<i64> {
    connection.query_row(
        "SELECT count(DISTINCT json_extract(connection_json, '$.credentialRef'))
           FROM printers WHERE connection_json IS NOT NULL",
        [],
        |row| row.get(0),
    )
}

fn media_totals(
    connection: &Connection,
    choice: BackupMediaChoice,
) -> rusqlite::Result<(i64, i64)> {
    connection.query_row(
        &format!(
            "SELECT count(*), COALESCE(sum(byte_len), 0) FROM camera_snapshots WHERE {}",
            media_filter(choice)
        ),
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

fn content_totals(connection: &Connection) -> rusqlite::Result<(i64, i64)> {
    connection.query_row(
        "SELECT count(*), COALESCE(sum(size_bytes), 0) FROM content_blobs",
        [],
        |row| Ok((row.get(0)?, row.get(1)?)),
    )
}

/// `backup_inventory` over the live Farm (call it in one read
/// transaction).
pub fn live_inventory(connection: &Connection) -> rusqlite::Result<BackupInventory> {
    let (content_count, content_bytes) = content_totals(connection)?;
    let media = BackupMediaChoice::ALL
        .into_iter()
        .map(|choice| {
            media_totals(connection, choice).map(|(count, bytes)| BackupMediaTotals {
                choice,
                count,
                bytes,
            })
        })
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let counts = table_counts(connection)?
        .into_iter()
        .map(|(table, rows)| TableCount { table, rows })
        .collect();
    let active_job_count = connection.query_row(
        "SELECT count(*) FROM jobs WHERE state NOT IN ('completed', 'failed', 'cancelled')",
        [],
        |row| row.get(0),
    )?;
    Ok(BackupInventory {
        database_bytes: database_bytes(connection)?,
        content: BackupContentTotals {
            count: content_count,
            bytes: content_bytes,
        },
        media,
        counts,
        credential_ref_count: credential_ref_count(connection)?,
        active_job_count,
        excluded: BackupExcludedClass::ALL.to_vec(),
    })
}

/// D5 step 1's estimate: the database, every blob, and the selected
/// snapshots, in bytes.
pub fn estimate(connection: &Connection, choice: BackupMediaChoice) -> rusqlite::Result<u64> {
    let (_, content) = content_totals(connection)?;
    let (_, media) = media_totals(connection, choice)?;
    let total = database_bytes(connection)?
        .saturating_add(content)
        .saturating_add(media);
    Ok(u64::try_from(total).unwrap_or(0))
}

/// Every `content_blobs` row, sorted by hash.
pub fn blobs(connection: &Connection) -> rusqlite::Result<Vec<BlobItem>> {
    let stored: BTreeSet<String> = connection
        .prepare(
            "SELECT content_sha256 FROM model_source_revisions WHERE format = '3mf'
             UNION SELECT sha256 FROM slice_revision_blobs WHERE role = 'plate3mf'",
        )?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    connection
        .prepare("SELECT sha256, size_bytes FROM content_blobs ORDER BY sha256")?
        .query_map([], |row| {
            let sha256: String = row.get(0)?;
            let bytes: i64 = row.get(1)?;
            Ok(BlobItem {
                stored: stored.contains(&sha256),
                sha256,
                bytes: u64::try_from(bytes).unwrap_or(0),
            })
        })?
        .collect()
}

/// D5 step 3: the unpruned snapshot rows `choice` selects, sorted by path.
pub fn selected_media(
    connection: &Connection,
    choice: BackupMediaChoice,
) -> rusqlite::Result<Vec<MediaItem>> {
    connection
        .prepare(&format!(
            "SELECT id, rel_path, byte_len, sha256 FROM camera_snapshots WHERE {}
              ORDER BY rel_path",
            media_filter(choice)
        ))?
        .query_map([], |row| {
            Ok(MediaItem {
                id: row.get(0)?,
                rel_path: row.get(1)?,
                bytes: u64::try_from(row.get::<_, i64>(2)?).unwrap_or(0),
                sha256: row.get(3)?,
            })
        })?
        .collect()
}

/// The streamed length and SHA-256 of the regular file at `path`, or
/// `None` when it is missing, not a regular file, or unreadable.
pub fn hash_file(path: &Path) -> Option<(u64, String)> {
    let metadata = fs::symlink_metadata(path).ok()?;
    if !metadata.is_file() {
        return None;
    }
    let mut reader = Sha256Reader::new(File::open(path).ok()?);
    io::copy(&mut reader, &mut io::sink()).ok()?;
    Some(reader.finish())
}

/// D5 step 4, the media pre-pass: splits `selected` into the files that
/// are present with their recorded length and SHA-256, and the ids of the
/// ones that are missing (gone, another length, or another hash).
pub fn media_prepass(media_root: &Path, selected: Vec<MediaItem>) -> (Vec<MediaItem>, Vec<String>) {
    let mut present = Vec::new();
    let mut missing = Vec::new();
    for item in selected {
        let intact = is_media_rel_path(&item.rel_path)
            && hash_file(&media_root.join(&item.rel_path))
                .is_some_and(|(bytes, sha256)| bytes == item.bytes && sha256 == item.sha256);
        if intact {
            present.push(item);
        } else {
            missing.push(item.id);
        }
    }
    (present, missing)
}

/// What [`sanitize`] marked.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct Sanitized {
    pub not_in_backup: i64,
    pub missing_file: i64,
}

/// D5 step 5, on the copy, in one transaction (the caller `VACUUM`s
/// after): nulls the Slicer runtime paths, empties the telemetry cache and
/// both pending-cleanup tables, and prunes every unpruned snapshot that
/// isn't `included` (`notInBackup`, or `missingFile` for the `missing`
/// ones) at `created_at`. No Incident timeline row is written.
pub fn sanitize(
    connection: &mut Connection,
    created_at: &str,
    included: &BTreeSet<String>,
    missing: &BTreeSet<String>,
) -> rusqlite::Result<Sanitized> {
    let transaction = connection.transaction()?;
    transaction.execute_batch(
        "UPDATE slicer_runtime_config SET engine_path = NULL, preset_source_path = NULL;
         DELETE FROM printer_status_snapshots;
         DELETE FROM pending_credential_cleanup;
         DELETE FROM pending_blob_cleanup;",
    )?;
    let unpruned: Vec<String> = transaction
        .prepare("SELECT id FROM camera_snapshots WHERE pruned_at IS NULL ORDER BY id")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    let mut sanitized = Sanitized::default();
    {
        let mut prune = transaction.prepare(
            "UPDATE camera_snapshots
                SET pruned_at = ?1, prune_reason = ?2, revision = revision + 1
              WHERE id = ?3",
        )?;
        for id in unpruned {
            if included.contains(&id) {
                continue;
            }
            let reason = if missing.contains(&id) {
                sanitized.missing_file += 1;
                "missingFile"
            } else {
                sanitized.not_in_backup += 1;
                "notInBackup"
            };
            prune.execute(params![created_at, reason, id])?;
        }
    }
    transaction.commit()?;
    Ok(sanitized)
}

/// The copy's `schema_migrations`, ascending.
pub fn migrations(connection: &Connection) -> rusqlite::Result<Vec<ManifestMigration>> {
    connection
        .prepare("SELECT version, name, checksum FROM schema_migrations ORDER BY version")?
        .query_map([], |row| {
            Ok(ManifestMigration {
                version: row.get(0)?,
                name: row.get(1)?,
                checksum: row.get(2)?,
            })
        })?
        .collect()
}
