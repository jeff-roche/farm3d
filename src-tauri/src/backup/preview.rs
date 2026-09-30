//! The restore preview (spec D6, D7, D10): what swapping the live Farm for
//! a staged candidate would change.
//!
//! [`compute`] reads the live Farm in one read transaction and the
//! candidate through a read-only connection; it writes nothing.
//!
//! - **Counts:** every table either side has, sorted, local versus backup.
//! - **Conflicts** (D6): for each domain, a local row whose id is in the
//!   backup is `changed` when any column of the root row differs (same
//!   SQLite type and value); a local row whose id isn't is `uniqueClash`
//!   when a backup row with another id has its natural key, else
//!   `onlyLocal`. Backup-only rows are only counted. Groups follow class,
//!   then domain order; each keeps its total and at most 200 items, by
//!   local id, then backup id.
//! - **Blockers** (D7): live work in flight, at most 50, by kind then id.
//! - **Notices** (D10): credentials to re-enter (checked with
//!   `CredentialStore::contains`, which never hands out the value),
//!   credentials orphaned, linked paths missing here, Jobs active at backup
//!   time, media left out, the Slicer runtime kept local, and a migrated
//!   schema. A count notice appears only when its count isn't zero;
//!   `slicerRuntimeKeptLocal` always appears.

use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

use chrono::SecondsFormat;
use rusqlite::types::Value;
use rusqlite::{Connection, OpenFlags};

use super::inventory::table_counts;
use super::staging::{RestoreError, StagedCandidate};
use super::{
    BackupPlatform, RestoreBackupInfo, RestoreBlocker, RestoreBlockerKind, RestoreConflict,
    RestoreConflictClass, RestoreConflictGroup, RestoreCount, RestoreDomain, RestoreNotice,
    RestorePreview, RestorePreviewSource,
};
use crate::connections::credentials::CredentialStore;
use crate::persistence::Storage;

/// The most items a conflict group carries.
pub const MAX_CONFLICT_ITEMS: usize = 200;
/// The most blockers a preview or a refusal lists.
pub const MAX_BLOCKERS: usize = 50;

/// One D6 domain: its root table, id, label, and natural key.
struct Domain {
    domain: RestoreDomain,
    table: &'static str,
    id: &'static str,
    label: &'static str,
    /// `(expression, condition)`: the natural key, where the condition
    /// holds (the partial unique index's `WHERE`).
    key: Option<(&'static str, &'static str)>,
}

/// D6's table, in its order.
const DOMAINS: [Domain; 12] = [
    Domain {
        domain: RestoreDomain::Settings,
        table: "settings",
        id: "'settings'",
        label: "'Settings'",
        key: None,
    },
    Domain {
        domain: RestoreDomain::Printer,
        table: "printers",
        id: "id",
        label: "name",
        key: Some((
            "host_identity",
            "archived_at IS NULL AND host_identity IS NOT NULL",
        )),
    },
    Domain {
        domain: RestoreDomain::Spool,
        table: "spools",
        id: "id",
        label: "'#' || spool_number",
        key: Some(("CAST(spool_number AS TEXT)", "1")),
    },
    Domain {
        domain: RestoreDomain::Tare,
        table: "spool_tares",
        id: "id",
        label: "name",
        // SQLite's `lower()` folds ASCII only, as the unique index does.
        key: Some(("lower(name)", "1")),
    },
    Domain {
        domain: RestoreDomain::Project,
        table: "library_projects",
        id: "id",
        label: "name",
        key: Some(("lower(name)", "1")),
    },
    Domain {
        domain: RestoreDomain::Model,
        table: "library_models",
        id: "id",
        label: "name",
        key: None,
    },
    Domain {
        domain: RestoreDomain::SliceRevision,
        table: "slice_revisions",
        id: "id",
        label: "id",
        key: None,
    },
    Domain {
        domain: RestoreDomain::QueueEntry,
        table: "queue_entries",
        id: "id",
        label: "id",
        key: None,
    },
    Domain {
        domain: RestoreDomain::Job,
        table: "jobs",
        id: "id",
        label: "id",
        key: None,
    },
    Domain {
        domain: RestoreDomain::Incident,
        table: "incidents",
        id: "id",
        label: "id",
        key: None,
    },
    Domain {
        domain: RestoreDomain::AttentionEvent,
        table: "attention_events",
        id: "id",
        label: "id",
        key: None,
    },
    Domain {
        domain: RestoreDomain::Snapshot,
        table: "camera_snapshots",
        id: "id",
        label: "id",
        key: None,
    },
];

const CLASSES: [RestoreConflictClass; 3] = [
    RestoreConflictClass::OnlyLocal,
    RestoreConflictClass::Changed,
    RestoreConflictClass::UniqueClash,
];

/// One root row: its label, natural key (where it has one), and every
/// column in order.
struct Row {
    label: String,
    key: Option<String>,
    values: Vec<Value>,
}

fn load(connection: &Connection, domain: &Domain) -> rusqlite::Result<BTreeMap<String, Row>> {
    let key = match domain.key {
        Some((expression, condition)) => format!("CASE WHEN {condition} THEN {expression} END"),
        None => "NULL".to_string(),
    };
    let mut statement = connection.prepare(&format!(
        "SELECT {id}, {label}, {key}, * FROM {table}",
        id = domain.id,
        label = domain.label,
        table = domain.table,
    ))?;
    let columns = statement.column_count();
    let rows = statement
        .query_map([], |row| {
            let id: String = row.get(0)?;
            let label: Option<String> = row.get(1)?;
            let values = (3..columns)
                .map(|index| row.get::<_, Value>(index))
                .collect::<rusqlite::Result<Vec<_>>>()?;
            Ok((
                id.clone(),
                Row {
                    label: label.unwrap_or(id),
                    key: row.get(2)?,
                    values,
                },
            ))
        })?
        .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
    Ok(rows)
}

/// D6 for one domain: the `onlyLocal`, `changed`, and `uniqueClash` items,
/// each by local id (`String` orders bytewise).
fn classify(
    local: &BTreeMap<String, Row>,
    backup: &BTreeMap<String, Row>,
) -> [Vec<RestoreConflict>; 3] {
    let backup_keys: BTreeMap<&str, &str> = backup
        .iter()
        .filter_map(|(id, row)| row.key.as_deref().map(|key| (key, id.as_str())))
        .collect();
    let mut only_local = Vec::new();
    let mut changed = Vec::new();
    let mut clashes = Vec::new();
    for (id, row) in local {
        let item = |backup_id: Option<&str>| RestoreConflict {
            local_id: Some(id.clone()),
            backup_id: backup_id.map(str::to_string),
            label: row.label.clone(),
        };
        if let Some(other) = backup.get(id) {
            if other.values != row.values {
                changed.push(item(Some(id)));
            }
        } else if let Some(backup_id) = row
            .key
            .as_deref()
            .and_then(|key| backup_keys.get(key).copied())
        {
            clashes.push(item(Some(backup_id)));
        } else {
            only_local.push(item(None));
        }
    }
    [only_local, changed, clashes]
}

/// Every non-empty conflict group, in D6's order.
pub fn conflicts(
    live: &Connection,
    candidate: &Connection,
) -> rusqlite::Result<Vec<RestoreConflictGroup>> {
    let mut by_domain = Vec::with_capacity(DOMAINS.len());
    for domain in &DOMAINS {
        let classified = classify(&load(live, domain)?, &load(candidate, domain)?);
        by_domain.push((domain.domain, classified));
    }
    let mut groups = Vec::new();
    for (class_index, class) in CLASSES.into_iter().enumerate() {
        for (domain, classified) in &by_domain {
            let items = &classified[class_index];
            if items.is_empty() {
                continue;
            }
            groups.push(RestoreConflictGroup {
                class,
                domain: *domain,
                total: items.len() as i64,
                items: items.iter().take(MAX_CONFLICT_ITEMS).cloned().collect(),
            });
        }
    }
    Ok(groups)
}

/// D7's blockers in `connection` (the live Farm): at most
/// [`MAX_BLOCKERS`], by kind then id, and the total.
pub fn blockers(connection: &Connection) -> rusqlite::Result<(Vec<RestoreBlocker>, i64)> {
    let mut statement = connection.prepare(
        "SELECT 0, id FROM jobs WHERE state NOT IN ('completed', 'failed', 'cancelled')
         UNION ALL
         SELECT 1, id FROM host_operations WHERE state IN ('dispatching', 'uncertain', 'reconciling')
         UNION ALL
         SELECT 2, id FROM slice_operations WHERE state IN ('queued', 'running')
         ORDER BY 1, 2",
    )?;
    let mut blockers = Vec::new();
    let mut total = 0_i64;
    let mut rows = statement.query([])?;
    while let Some(row) = rows.next()? {
        total += 1;
        if blockers.len() < MAX_BLOCKERS {
            let kind = match row.get::<_, i64>(0)? {
                0 => RestoreBlockerKind::ActiveJob,
                1 => RestoreBlockerKind::HostOperation,
                _ => RestoreBlockerKind::SliceOperation,
            };
            blockers.push(RestoreBlocker {
                kind,
                id: row.get(1)?,
            });
        }
    }
    Ok((blockers, total))
}

/// `(printer id, credentialRef)` of every Printer with a text ref.
pub(crate) fn printer_refs(connection: &Connection) -> rusqlite::Result<Vec<(String, String)>> {
    connection
        .prepare(
            "SELECT id, json_extract(connection_json, '$.credentialRef') FROM printers
              WHERE connection_json IS NOT NULL AND json_valid(connection_json)
                AND json_type(connection_json, '$.credentialRef') = 'text'
              ORDER BY id",
        )?
        .query_map([], |row| Ok((row.get(0)?, row.get(1)?)))?
        .collect()
}

/// Every credential ref the live Farm names: its Printers' and its pending
/// cleanup rows'.
pub(crate) fn local_refs(connection: &Connection) -> rusqlite::Result<BTreeSet<String>> {
    let mut refs: BTreeSet<String> = printer_refs(connection)?
        .into_iter()
        .map(|(_, reference)| reference)
        .collect();
    let pending: Vec<String> = connection
        .prepare("SELECT credential_ref FROM pending_credential_cleanup")?
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<_>>()?;
    refs.extend(pending);
    Ok(refs)
}

/// What the preview reads from the candidate besides the conflicts.
struct CandidateFacts {
    counts: BTreeMap<String, i64>,
    printer_refs: Vec<(String, String)>,
    linked_paths: Vec<String>,
    active_jobs: i64,
}

fn candidate_facts(candidate: &Connection) -> rusqlite::Result<CandidateFacts> {
    Ok(CandidateFacts {
        counts: table_counts(candidate)?,
        printer_refs: printer_refs(candidate)?,
        linked_paths: candidate
            .prepare(
                "SELECT linked_path FROM library_models
                  WHERE storage_mode = 'linked' AND linked_path IS NOT NULL",
            )?
            .query_map([], |row| row.get(0))?
            .collect::<rusqlite::Result<_>>()?,
        active_jobs: candidate.query_row(
            "SELECT count(*) FROM jobs WHERE state NOT IN ('completed', 'failed', 'cancelled')",
            [],
            |row| row.get(0),
        )?,
    })
}

/// Opens the staged candidate read-only.
pub(crate) fn open_candidate(candidate: &StagedCandidate) -> Result<Connection, RestoreError> {
    let connection = Connection::open_with_flags(
        candidate.layout.candidate_path(),
        OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX,
    )
    .map_err(|_| RestoreError::Io)?;
    connection
        .pragma_update(None, "query_only", "ON")
        .map_err(|_| RestoreError::Io)?;
    Ok(connection)
}

/// The preview of restoring `candidate` over `storage`'s Farm. Writes
/// nothing: the live Farm is read in one read transaction, the candidate
/// through a read-only connection.
pub fn compute(
    storage: &Storage,
    candidate: &StagedCandidate,
    source: RestorePreviewSource,
    credentials: &CredentialStore,
) -> Result<RestorePreview, RestoreError> {
    let backup = open_candidate(candidate)?;
    let facts = candidate_facts(&backup).map_err(|_| RestoreError::Io)?;
    let (local_counts, conflicts, (blockers, blocker_total), local_refs) = storage
        .read_transaction(|live| {
            Ok((
                table_counts(live)?,
                conflicts(live, &backup)?,
                blockers(live)?,
                local_refs(live)?,
            ))
        })
        .map_err(|_| RestoreError::Io)?;
    drop(backup);

    let tables: BTreeSet<&String> = local_counts.keys().chain(facts.counts.keys()).collect();
    let counts = tables
        .into_iter()
        .map(|table| RestoreCount {
            table: table.clone(),
            local: local_counts.get(table).copied().unwrap_or(0),
            backup: facts.counts.get(table).copied().unwrap_or(0),
        })
        .collect();

    let notices = notices(candidate, &facts, &local_refs, credentials);
    let manifest = &candidate.manifest;
    let timestamp =
        |time: chrono::DateTime<chrono::Utc>| time.to_rfc3339_opts(SecondsFormat::Millis, true);
    Ok(RestorePreview {
        staging_id: candidate.staging_id.clone(),
        created_at: timestamp(candidate.created_at),
        expires_at: timestamp(candidate.expires_at()),
        source,
        backup: RestoreBackupInfo {
            created_at: manifest.created_at.clone(),
            app_version: manifest.app_version.clone(),
            schema_version: manifest.schema_version,
            format_version: manifest.format_version,
            origin: manifest.origin,
            media: manifest.contents.media,
            platform: BackupPlatform {
                os: manifest.platform.os.clone(),
                arch: manifest.platform.arch.clone(),
            },
        },
        counts,
        conflicts,
        notices,
        blockers,
        blocker_total,
    })
}

/// D10's notices, in the order of the type.
fn notices(
    candidate: &StagedCandidate,
    facts: &CandidateFacts,
    local_refs: &BTreeSet<String>,
    credentials: &CredentialStore,
) -> Vec<RestoreNotice> {
    let mut notices = Vec::new();

    // A Printer whose ref the store lacks (or can't be read for) reports
    // `CREDENTIAL_REQUIRED` after the restore.
    let mut present: BTreeMap<&str, bool> = BTreeMap::new();
    let mut to_reenter = 0;
    for (_, reference) in &facts.printer_refs {
        let found = *present
            .entry(reference.as_str())
            .or_insert_with(|| credentials.contains(reference).unwrap_or(false));
        if !found {
            to_reenter += 1;
        }
    }
    if to_reenter > 0 {
        notices.push(RestoreNotice::CredentialsToReenter {
            printer_count: to_reenter,
        });
    }

    let restored_refs: BTreeSet<&str> = facts
        .printer_refs
        .iter()
        .map(|(_, reference)| reference.as_str())
        .collect();
    let orphaned = local_refs
        .iter()
        .filter(|reference| !restored_refs.contains(reference.as_str()))
        .count() as i64;
    if orphaned > 0 {
        notices.push(RestoreNotice::CredentialsOrphaned {
            ref_count: orphaned,
        });
    }

    let missing = facts
        .linked_paths
        .iter()
        .filter(|path| !matches!(Path::new(path).try_exists(), Ok(true)))
        .count() as i64;
    if missing > 0 {
        notices.push(RestoreNotice::LinkedPathsMissing {
            model_count: missing,
        });
    }

    if facts.active_jobs > 0 {
        notices.push(RestoreNotice::ActiveJobsAtBackup {
            job_count: facts.active_jobs,
        });
    }

    let media = candidate.manifest.media;
    if media.not_in_backup > 0 || media.missing_file > 0 {
        notices.push(RestoreNotice::MediaNotInBackup {
            snapshot_count: media.not_in_backup,
            missing_file_count: media.missing_file,
        });
    }

    notices.push(RestoreNotice::SlicerRuntimeKeptLocal);

    if let Some(from_schema_version) = candidate.migrated_from {
        notices.push(RestoreNotice::Migrated {
            from_schema_version,
        });
    }
    notices
}
