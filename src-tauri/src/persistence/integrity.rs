//! P9 D17: the reference-integrity catalogue. One checker, [`check`], used
//! by the backup writer, restore staging and install, the diagnostics
//! storage section, `clear_storage`, and the reference matrix. Each rule is
//! a named function below; the catalogue order is the report order.
//!
//! Loose references (JSON paths and id columns with no foreign key) are the
//! reason this exists: `PRAGMA foreign_key_check` cannot see them. Every
//! `*_json` column and every `TEXT` column ending `_id` the Task 11 audit
//! finds that this catalogue misses gets a rule (or a named tolerance)
//! here, in the same commit.
//!
//! A finding's `sample` holds at most [`SAMPLE_LIMIT`] row ids or content
//! hashes and never a path.

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::error::StorageError;
use super::StoragePaths;

/// Most row ids or hashes a finding samples.
pub const SAMPLE_LIMIT: usize = 20;

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/IntegrityRule.ts")]
pub enum IntegrityRule {
    Fk,
    JobCorrectionEvent,
    AmountEventReservation,
    ReservationHolder,
    AttentionSource,
    CompletionEvidence,
    HostOperationGcode,
    BlobFile,
    MediaFile,
    SliceTargetPrinter,
    PreparationTargetPrinter,
    OrphanBlobFile,
    OrphanMediaFile,
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/IntegrityOutcome.ts")]
pub enum IntegrityOutcome {
    Violation,
    Tolerated,
}

impl IntegrityRule {
    /// The rule's outcome when it fires: violations break a Farm; the
    /// tolerated rules are by-design dangling references or files a
    /// startup sweep removes.
    pub fn outcome(self) -> IntegrityOutcome {
        match self {
            Self::SliceTargetPrinter
            | Self::PreparationTargetPrinter
            | Self::OrphanBlobFile
            | Self::OrphanMediaFile => IntegrityOutcome::Tolerated,
            _ => IntegrityOutcome::Violation,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityFinding {
    pub rule: IntegrityRule,
    pub outcome: IntegrityOutcome,
    pub count: i64,
    pub sample: Vec<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct IntegrityReport {
    /// One entry per table (SQLite's own `sqlite_*` tables excluded): the
    /// exact `count(*)`.
    pub counts: BTreeMap<String, i64>,
    pub findings: Vec<IntegrityFinding>,
}

impl IntegrityReport {
    pub fn violations(&self) -> Vec<&IntegrityFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.outcome == IntegrityOutcome::Violation)
            .collect()
    }

    pub fn tolerated(&self) -> Vec<&IntegrityFinding> {
        self.findings
            .iter()
            .filter(|finding| finding.outcome == IntegrityOutcome::Tolerated)
            .collect()
    }

    pub fn finding(&self, rule: IntegrityRule) -> Option<&IntegrityFinding> {
        self.findings.iter().find(|finding| finding.rule == rule)
    }
}

/// Where the file-backed rules look. Without roots those rules are skipped.
#[derive(Clone, Debug)]
pub struct IntegrityRoots {
    /// The Library content root (`blobs/sha256/<hh>/<hex>` lives under it).
    pub content_root: PathBuf,
    /// The camera media root (`snapshots/...` lives under it).
    pub media_root: PathBuf,
}

impl IntegrityRoots {
    pub fn from_paths(paths: &StoragePaths) -> Self {
        Self {
            content_root: paths.content_root().to_path_buf(),
            media_root: paths.media_root().to_path_buf(),
        }
    }
}

/// Runs the whole catalogue. Its reads run in one deferred transaction (so
/// they all see one WAL snapshot); on a connection already inside a
/// transaction they run in that one.
pub fn check(
    connection: &Connection,
    roots: Option<&IntegrityRoots>,
) -> Result<IntegrityReport, StorageError> {
    if connection.is_autocommit() {
        let transaction = connection.unchecked_transaction()?;
        let report = run(&transaction, roots)?;
        transaction.commit()?;
        Ok(report)
    } else {
        run(connection, roots)
    }
}

fn run(
    connection: &Connection,
    roots: Option<&IntegrityRoots>,
) -> Result<IntegrityReport, StorageError> {
    let mut report = IntegrityReport {
        counts: table_counts(connection)?,
        findings: Vec::new(),
    };
    let mut push = |finding: Option<IntegrityFinding>| report.findings.extend(finding);
    push(foreign_keys(connection)?);
    push(job_correction_event(connection)?);
    push(amount_event_reservation(connection)?);
    push(reservation_holder(connection)?);
    push(attention_source(connection)?);
    push(completion_evidence(connection)?);
    push(host_operation_gcode(connection)?);
    if let Some(roots) = roots {
        push(blob_file(connection, roots)?);
        push(media_file(connection, roots)?);
    }
    push(slice_target_printer(connection)?);
    push(preparation_target_printer(connection)?);
    if let Some(roots) = roots {
        push(orphan_blob_file(connection, roots)?);
        push(orphan_media_file(connection, roots)?);
    }
    Ok(report)
}

fn table_counts(connection: &Connection) -> Result<BTreeMap<String, i64>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT name FROM sqlite_master WHERE type = 'table' AND name NOT LIKE 'sqlite_%'",
    )?;
    let names = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    let mut counts = BTreeMap::new();
    for name in names {
        let rows: i64 =
            connection.query_row(&format!("SELECT count(*) FROM \"{name}\""), [], |row| {
                row.get(0)
            })?;
        counts.insert(name, rows);
    }
    Ok(counts)
}

fn finding(rule: IntegrityRule, count: i64, sample: Vec<String>) -> Option<IntegrityFinding> {
    (count > 0).then(|| IntegrityFinding {
        rule,
        outcome: rule.outcome(),
        count,
        sample,
    })
}

/// Runs `offenders`, a query selecting one text id per offending row, for
/// its exact count and its first [`SAMPLE_LIMIT`] ids (sorted, so the
/// sample is deterministic).
fn from_query(
    connection: &Connection,
    rule: IntegrityRule,
    offenders: &str,
) -> Result<Option<IntegrityFinding>, StorageError> {
    let count: i64 =
        connection.query_row(&format!("SELECT count(*) FROM ({offenders})"), [], |row| {
            row.get(0)
        })?;
    if count == 0 {
        return Ok(None);
    }
    let mut statement = connection.prepare(&format!(
        "SELECT * FROM ({offenders}) ORDER BY 1 LIMIT {SAMPLE_LIMIT}"
    ))?;
    let sample = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<Result<Vec<_>, _>>()?;
    Ok(finding(rule, count, sample))
}

/// Rule `fk`: `PRAGMA foreign_key_check`.
fn foreign_keys(connection: &Connection) -> Result<Option<IntegrityFinding>, StorageError> {
    let mut statement = connection.prepare("PRAGMA foreign_key_check")?;
    let mut rows = statement
        .query_map([], |row| {
            Ok(format!(
                "{}#{}",
                row.get::<_, String>(0)?,
                row.get::<_, Option<i64>>(1)?.unwrap_or_default()
            ))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let count = rows.len() as i64;
    rows.sort();
    rows.truncate(SAMPLE_LIMIT);
    Ok(finding(IntegrityRule::Fk, count, rows))
}

/// Rule `jobCorrectionEvent`: a Job's correction event is an amount event
/// of the Job's own Spool.
fn job_correction_event(connection: &Connection) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::JobCorrectionEvent,
        "SELECT j.id FROM jobs j
          WHERE j.correction_event_id IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM spool_amount_events e
                             WHERE e.id = j.correction_event_id AND e.spool_id = j.spool_id)",
    )
}

/// Rule `amountEventReservation`: an amount event's reservation exists.
fn amount_event_reservation(
    connection: &Connection,
) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::AmountEventReservation,
        "SELECT e.id FROM spool_amount_events e
          WHERE e.reservation_id IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM spool_reservations r WHERE r.id = e.reservation_id)",
    )
}

/// Rule `reservationHolder`: a reservation is held by a Job that exists.
fn reservation_holder(connection: &Connection) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::ReservationHolder,
        "SELECT r.id FROM spool_reservations r
          WHERE r.holder_kind <> 'job'
             OR NOT EXISTS (SELECT 1 FROM jobs j WHERE j.id = r.holder_id)",
    )
}

/// Rule `attentionSource`: an Attention Event's source row exists; a
/// Printer source may be missing only when the Event is resolved. Printer
/// delete resolves the Printer's open Events `sourceRemoved` (P8 D8); one
/// resolved before the delete keeps its own resolution.
fn attention_source(connection: &Connection) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::AttentionSource,
        "SELECT a.id FROM attention_events a
          WHERE NOT CASE a.source_kind
            WHEN 'printer' THEN EXISTS (SELECT 1 FROM printers p WHERE p.id = a.source_id)
                                OR a.resolved_at IS NOT NULL
            WHEN 'job' THEN EXISTS (SELECT 1 FROM jobs j WHERE j.id = a.source_id)
            WHEN 'reconciliationRequirement'
              THEN EXISTS (SELECT 1 FROM reconciliation_requirements r WHERE r.id = a.source_id)
            WHEN 'spool' THEN EXISTS (SELECT 1 FROM spools s WHERE s.id = a.source_id)
            ELSE 0 END",
    )
}

/// Rule `completionEvidence`: a `job.completed` Event whose evidence is
/// `captured` names a snapshot row.
fn completion_evidence(connection: &Connection) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::CompletionEvidence,
        "SELECT a.id FROM attention_events a
          WHERE a.condition = 'job.completed'
            AND a.evidence_json IS NOT NULL
            AND json_extract(a.evidence_json, '$.status') = 'captured'
            AND NOT EXISTS (SELECT 1 FROM camera_snapshots s
                             WHERE s.id = json_extract(a.evidence_json, '$.snapshotId'))",
    )
}

/// Rule `hostOperationGcode`: an unresolved upload or start Host Operation
/// still has its G-code blob row.
fn host_operation_gcode(connection: &Connection) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::HostOperationGcode,
        "SELECT h.id FROM host_operations h
          WHERE h.kind IN ('upload','start')
            AND h.state IN ('dispatching','uncertain','reconciling')
            AND NOT EXISTS (SELECT 1 FROM content_blobs b WHERE b.sha256 = h.gcode_sha256)",
    )
}

/// Rule `sliceTargetPrinter` (tolerated): a Slice Revision's target Printer
/// may be gone; the UI falls back to a label by design.
fn slice_target_printer(connection: &Connection) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::SliceTargetPrinter,
        "SELECT s.id FROM slice_revisions s
          WHERE json_extract(s.target_json, '$.target.printerId') IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM printers p
                             WHERE p.id = json_extract(s.target_json, '$.target.printerId'))",
    )
}

/// Rule `preparationTargetPrinter` (tolerated): as above, for a Slice
/// Preparation's document.
fn preparation_target_printer(
    connection: &Connection,
) -> Result<Option<IntegrityFinding>, StorageError> {
    from_query(
        connection,
        IntegrityRule::PreparationTargetPrinter,
        "SELECT s.id FROM slice_preparations s
          WHERE json_extract(s.document_json, '$.target.printerId') IS NOT NULL
            AND NOT EXISTS (SELECT 1 FROM printers p
                             WHERE p.id = json_extract(s.document_json, '$.target.printerId'))",
    )
}

fn blob_path(root: &IntegrityRoots, sha256: &str) -> PathBuf {
    root.content_root
        .join("blobs/sha256")
        .join(sha256.get(..2).unwrap_or("00"))
        .join(sha256)
}

/// Rule `blobFile`: every blob row has its file, of `size_bytes` length.
fn blob_file(
    connection: &Connection,
    roots: &IntegrityRoots,
) -> Result<Option<IntegrityFinding>, StorageError> {
    let mut statement = connection.prepare("SELECT sha256, size_bytes FROM content_blobs")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut bad: Vec<String> = rows
        .into_iter()
        .filter(|(sha256, size)| {
            fs::metadata(blob_path(roots, sha256)).map_or(true, |metadata| {
                !metadata.is_file() || metadata.len() as i64 != *size
            })
        })
        .map(|(sha256, _)| sha256)
        .collect();
    Ok(file_finding(IntegrityRule::BlobFile, &mut bad))
}

/// Rule `mediaFile`: every unpruned snapshot row has its file.
fn media_file(
    connection: &Connection,
    roots: &IntegrityRoots,
) -> Result<Option<IntegrityFinding>, StorageError> {
    let mut statement =
        connection.prepare("SELECT id, rel_path FROM camera_snapshots WHERE pruned_at IS NULL")?;
    let rows = statement
        .query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?
        .collect::<Result<Vec<_>, _>>()?;
    let mut bad: Vec<String> = rows
        .into_iter()
        .filter(|(_, rel_path)| !roots.media_root.join(rel_path).is_file())
        .map(|(id, _)| id)
        .collect();
    Ok(file_finding(IntegrityRule::MediaFile, &mut bad))
}

/// Rule `orphanBlobFile` (tolerated): a blob file with neither a
/// `content_blobs` nor a `pending_blob_cleanup` row; P4's startup sweep
/// removes it.
fn orphan_blob_file(
    connection: &Connection,
    roots: &IntegrityRoots,
) -> Result<Option<IntegrityFinding>, StorageError> {
    let known: BTreeSet<String> = {
        let mut statement = connection.prepare(
            "SELECT sha256 FROM content_blobs UNION SELECT sha256 FROM pending_blob_cleanup",
        )?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let mut orphans: Vec<String> = files_under(&roots.content_root.join("blobs/sha256"))?
        .into_iter()
        .filter_map(|path| {
            path.file_name()
                .map(|name| name.to_string_lossy().into_owned())
        })
        .filter(|name| !known.contains(name))
        .collect();
    Ok(file_finding(IntegrityRule::OrphanBlobFile, &mut orphans))
}

/// Rule `orphanMediaFile` (tolerated): an image file under `snapshots/`
/// with no unpruned row; P8's startup sweep removes it. The sample is the
/// file's stem (the snapshot id it was named for), never its path.
fn orphan_media_file(
    connection: &Connection,
    roots: &IntegrityRoots,
) -> Result<Option<IntegrityFinding>, StorageError> {
    let known: BTreeSet<String> = {
        let mut statement =
            connection.prepare("SELECT rel_path FROM camera_snapshots WHERE pruned_at IS NULL")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        rows.collect::<Result<_, _>>()?
    };
    let snapshots = roots.media_root.join("snapshots");
    let mut orphans: Vec<String> = files_under(&snapshots)?
        .into_iter()
        .filter(|path| {
            matches!(
                path.extension().and_then(|extension| extension.to_str()),
                Some("jpg" | "jpeg" | "png")
            )
        })
        .filter(|path| {
            let relative = path
                .strip_prefix(&roots.media_root)
                .map(|relative| {
                    relative
                        .components()
                        .map(|component| component.as_os_str().to_string_lossy().into_owned())
                        .collect::<Vec<_>>()
                        .join("/")
                })
                .unwrap_or_default();
            !known.contains(&relative)
        })
        .filter_map(|path| {
            path.file_stem()
                .map(|stem| stem.to_string_lossy().into_owned())
        })
        .collect();
    Ok(file_finding(IntegrityRule::OrphanMediaFile, &mut orphans))
}

fn file_finding(rule: IntegrityRule, ids: &mut Vec<String>) -> Option<IntegrityFinding> {
    ids.sort();
    let count = ids.len() as i64;
    ids.truncate(SAMPLE_LIMIT);
    finding(rule, count, std::mem::take(ids))
}

/// Every regular file under `root`, recursively. A missing directory is
/// empty.
fn files_under(root: &Path) -> Result<Vec<PathBuf>, StorageError> {
    let mut found = Vec::new();
    let mut pending = vec![root.to_path_buf()];
    while let Some(directory) = pending.pop() {
        let entries = match fs::read_dir(&directory) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => continue,
            Err(_) => return Err(StorageError::Filesystem),
        };
        for entry in entries {
            let entry = entry.map_err(|_| StorageError::Filesystem)?;
            let file_type = entry.file_type().map_err(|_| StorageError::Filesystem)?;
            if file_type.is_dir() {
                pending.push(entry.path());
            } else if file_type.is_file() {
                found.push(entry.path());
            }
        }
    }
    Ok(found)
}
