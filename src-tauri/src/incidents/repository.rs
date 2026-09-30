//! P8 D3: the SQL for `incidents` and its append-only `incident_events`
//! timeline. Every write function takes the caller's `&Transaction` (P3+'s
//! pattern; see `jobs::repository`'s module doc), so the projector
//! (Task 6) composes these with `attention::repository` into one atomic
//! pass, and a command composes one of these into one atomic commit.
//!
//! [`open`] is the projector's Incident-rule step 1 for a fresh Incident:
//! idempotent for a Job (a second `open` for the same Job returns the
//! existing row, D3 "at most one Incident per Job"), always fresh for
//! `printer.hostFailed` (`job_id: None`). [`link_event`] is steps 1-2 for
//! an existing Incident, reopening it first (via [`reopen`]) when it was
//! closed. [`append_entry`] is the one general-purpose timeline write
//! (`eventAcknowledged`, `eventResolved`, every `evidence*` kind,
//! `noteAdded`): it always bumps `incidents.revision` by one, deriving
//! `kind` and the `attention_event_id`/`snapshot_id` FKs from `detail`
//! itself so the two can't disagree. [`close_if_settled`] is the
//! projector's close check (D2 "Apply" rule 3). `Incident`'s derived
//! fields (`linkedEventIds`, `openLinkedEventCount`, `snapshotCount`) are
//! computed at read time from `attention_events`/`camera_snapshots`,
//! never stored.

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::jobs::PrinterSnapshot;
use crate::library;
use crate::persistence::{RepositoryError, StorageError};
use crate::spools::{decode_enum, encode_enum};

use super::{
    Incident, IncidentDetail, IncidentEntry, IncidentEntryDetail, IncidentKind, IncidentPage,
    IncidentState, IncidentTimelineItem,
};

const INCIDENT_ID_PREFIX: &str = "inc";
const INCIDENT_EVENT_ID_PREFIX: &str = "iev";

fn new_incident_id() -> String {
    library::new_id(INCIDENT_ID_PREFIX)
}

fn new_entry_id() -> String {
    library::new_id(INCIDENT_EVENT_ID_PREFIX)
}

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
}

fn rfc3339(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn decode_text_enum<T: serde::de::DeserializeOwned>(
    index: usize,
    text: &str,
) -> rusqlite::Result<T> {
    decode_enum(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

fn to_json(value: &impl serde::Serialize) -> String {
    serde_json::to_string(value).expect("incident wire types always serialize")
}

fn from_json<T: serde::de::DeserializeOwned>(index: usize, text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

const INCIDENT_COLUMNS: &str =
    "id, revision, kind, printer_id, job_id, printer_snapshot_json, opened_at, closed_at";

/// Reads one `incidents` row and fills its three derived fields
/// (`linkedEventIds`, `openLinkedEventCount`, `snapshotCount`) from
/// `attention_events`/`camera_snapshots` — never stored on the row itself,
/// so they're always current for whatever this transaction has committed
/// so far.
pub fn load_incident(conn: &Connection, id: &str) -> Result<Option<Incident>, StorageError> {
    let base = conn
        .query_row(
            &format!("SELECT {INCIDENT_COLUMNS} FROM incidents WHERE id = ?1"),
            [id],
            |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, i64>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                    row.get::<_, Option<String>>(4)?,
                    row.get::<_, String>(5)?,
                    row.get::<_, String>(6)?,
                    row.get::<_, Option<String>>(7)?,
                ))
            },
        )
        .optional()?;
    let Some((id, revision, kind_text, printer_id, job_id, snapshot_json, opened_at, closed_at)) =
        base
    else {
        return Ok(None);
    };
    let kind: IncidentKind = decode_text_enum(2, &kind_text)?;
    let printer_snapshot: PrinterSnapshot = from_json(5, &snapshot_json)?;
    let state = if closed_at.is_some() {
        IncidentState::Closed
    } else {
        IncidentState::Open
    };

    let linked_event_ids: Vec<String> = {
        let mut statement = conn.prepare(
            "SELECT id FROM attention_events WHERE incident_id = ?1 ORDER BY first_observed_at, id",
        )?;
        let ids = statement
            .query_map([&id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        ids
    };
    let open_linked_event_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM attention_events WHERE incident_id = ?1 AND resolved_at IS NULL",
        [&id],
        |row| row.get(0),
    )?;
    let snapshot_count: i64 = conn.query_row(
        "SELECT COUNT(*) FROM camera_snapshots WHERE incident_id = ?1",
        [&id],
        |row| row.get(0),
    )?;

    Ok(Some(Incident {
        id,
        revision,
        kind,
        state,
        printer_id,
        job_id,
        printer_snapshot,
        opened_at,
        closed_at,
        linked_event_ids,
        open_linked_event_count,
        snapshot_count,
    }))
}

/// `get_incident`'s row lookup (the full `IncidentDetail` — timeline,
/// Events, snapshots — is Task 6's assembly).
pub fn get(conn: &Connection, incident_id: &str) -> Result<Option<Incident>, StorageError> {
    load_incident(conn, incident_id)
}

/// The Incident already open for `job_id`, if any.
pub fn for_job(conn: &Connection, job_id: &str) -> Result<Option<Incident>, StorageError> {
    let id: Option<String> = conn
        .query_row(
            "SELECT id FROM incidents WHERE job_id = ?1",
            [job_id],
            |row| row.get(0),
        )
        .optional()?;
    match id {
        Some(id) => load_incident(conn, &id),
        None => Ok(None),
    }
}

const ENTRY_COLUMNS: &str =
    "id, incident_id, sequence, kind, attention_event_id, snapshot_id, detail_json, operation_id, at";

fn decode_entry_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<IncidentEntry> {
    let kind_text: String = row.get(3)?;
    let detail_json: String = row.get(6)?;
    Ok(IncidentEntry {
        id: row.get(0)?,
        incident_id: row.get(1)?,
        sequence: row.get(2)?,
        kind: decode_text_enum(3, &kind_text)?,
        detail: from_json(6, &detail_json)?,
        operation_id: row.get(7)?,
        at: row.get(8)?,
    })
}

fn load_entry(conn: &Connection, id: &str) -> Result<Option<IncidentEntry>, StorageError> {
    Ok(conn
        .query_row(
            &format!("SELECT {ENTRY_COLUMNS} FROM incident_events WHERE id = ?1"),
            [id],
            decode_entry_row,
        )
        .optional()?)
}

fn next_sequence(tx: &Transaction<'_>, incident_id: &str) -> Result<i64, RepositoryError> {
    let max: Option<i64> = tx.query_row(
        "SELECT MAX(sequence) FROM incident_events WHERE incident_id = ?1",
        [incident_id],
        |row| row.get(0),
    )?;
    Ok(max.unwrap_or(0) + 1)
}

/// D3 "Timeline kinds"/"Revision and publishing": appends one
/// `incident_events` row (`kind` and the `attention_event_id`/
/// `snapshot_id` FKs derived from `detail`) and bumps `incidents.revision`
/// by one, in that order, both inside the caller's transaction. The one
/// general-purpose write for every kind that doesn't also change the
/// Incident's own open/closed state (`eventAcknowledged`,
/// `eventResolved`, every `evidence*` kind, `noteAdded`); [`open`],
/// [`reopen`], and [`close_if_settled`] call it too, after their own
/// `incidents` row update, so the whole "state change + its entry" still
/// costs exactly one bump.
pub fn append_entry(
    tx: &Transaction<'_>,
    incident_id: &str,
    detail: &IncidentEntryDetail,
    operation_id: Option<&str>,
    now: DateTime<Utc>,
) -> Result<IncidentEntry, RepositoryError> {
    let sequence = next_sequence(tx, incident_id)?;
    let id = new_entry_id();
    tx.execute(
        "INSERT INTO incident_events(
             id, incident_id, sequence, kind, attention_event_id, snapshot_id, detail_json,
             operation_id, at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            id,
            incident_id,
            sequence,
            encode_enum(detail.kind()),
            detail.event_id(),
            detail.snapshot_id(),
            to_json(detail),
            operation_id,
            rfc3339(now),
        ],
    )?;
    tx.execute(
        "UPDATE incidents SET revision = revision + 1 WHERE id = ?1",
        params![incident_id],
    )?;
    load_entry(tx, &id)?.ok_or_else(|| not_found(&id))
}

/// D2 "Apply" Incident rule 1: opens a new Incident and writes its
/// `opened { eventId }` entry (sequence 1, `revision` stays 1 — the
/// "opened" state, not a later bump). Idempotent for a Job: a second
/// `open` for the same `job_id` returns the existing Incident untouched
/// (D3 "at most one Incident per Job"); `job_id: None`
/// (`printer.hostFailed`) always opens a fresh one. Links `event_id` to
/// the returned Incident either way is only done for the fresh-open path;
/// an idempotent hit leaves the earlier link alone (the caller's own
/// Insert already named a different key).
pub fn open(
    tx: &Transaction<'_>,
    kind: IncidentKind,
    printer_id: &str,
    job_id: Option<&str>,
    printer_snapshot: &PrinterSnapshot,
    event_id: &str,
    now: DateTime<Utc>,
) -> Result<Incident, RepositoryError> {
    if let Some(job_id) = job_id {
        if let Some(existing) = for_job(tx, job_id)? {
            return Ok(existing);
        }
    }
    let id = new_incident_id();
    let now_text = rfc3339(now);
    tx.execute(
        "INSERT INTO incidents(
             id, revision, kind, printer_id, job_id, printer_snapshot_json, opened_at
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6)",
        params![
            id,
            encode_enum(kind),
            printer_id,
            job_id,
            to_json(printer_snapshot),
            now_text,
        ],
    )?;
    let detail = IncidentEntryDetail::Opened {
        event_id: event_id.to_string(),
    };
    let entry_id = new_entry_id();
    tx.execute(
        "INSERT INTO incident_events(id, incident_id, sequence, kind, attention_event_id, detail_json, at)
         VALUES (?1, ?2, 1, ?3, ?4, ?5, ?6)",
        params![
            entry_id,
            id,
            encode_enum(detail.kind()),
            event_id,
            to_json(&detail),
            now_text,
        ],
    )?;
    tx.execute(
        "UPDATE attention_events SET incident_id = ?2 WHERE id = ?1",
        params![event_id, id],
    )?;
    load_incident(tx, &id)?.ok_or_else(|| not_found(&id))
}

/// D3 "State": reopens a closed Incident (`closed_at` cleared) and writes
/// its `reopened { eventId }` entry — always immediately followed by
/// [`link_event`]'s own `eventLinked` entry, so "reopen and link" costs
/// exactly two revision bumps in total (D3 "however many entries the
/// transaction appended").
pub fn reopen(
    tx: &Transaction<'_>,
    incident_id: &str,
    event_id: &str,
    now: DateTime<Utc>,
) -> Result<Incident, RepositoryError> {
    tx.execute(
        "UPDATE incidents SET closed_at = NULL WHERE id = ?1",
        params![incident_id],
    )?;
    append_entry(
        tx,
        incident_id,
        &IncidentEntryDetail::Reopened {
            event_id: event_id.to_string(),
        },
        None,
        now,
    )?;
    load_incident(tx, incident_id)?.ok_or_else(|| not_found(incident_id))
}

/// D2 "Apply" Incident rules 1-2 for an *existing* Incident: reopens it
/// first (writing `reopened`) if it was closed, then links `event_id`
/// (`attention_events.incident_id` set, and an `eventLinked` entry
/// appended).
pub fn link_event(
    tx: &Transaction<'_>,
    incident_id: &str,
    event_id: &str,
    now: DateTime<Utc>,
) -> Result<Incident, RepositoryError> {
    let current = load_incident(tx, incident_id)?.ok_or_else(|| not_found(incident_id))?;
    if current.state == IncidentState::Closed {
        reopen(tx, incident_id, event_id, now)?;
    }
    append_entry(
        tx,
        incident_id,
        &IncidentEntryDetail::EventLinked {
            event_id: event_id.to_string(),
        },
        None,
        now,
    )?;
    tx.execute(
        "UPDATE attention_events SET incident_id = ?2 WHERE id = ?1",
        params![event_id, incident_id],
    )?;
    load_incident(tx, incident_id)?.ok_or_else(|| not_found(incident_id))
}

/// D2 "Apply" Incident rule 3: closes an open Incident when every linked
/// actionable (`requires_action`) Event is resolved, writing a `closed`
/// entry. A no-op (returns the current row unchanged) when the Incident is
/// already closed or still has an open actionable Event linked.
pub fn close_if_settled(
    tx: &Transaction<'_>,
    incident_id: &str,
    now: DateTime<Utc>,
) -> Result<Incident, RepositoryError> {
    let current = load_incident(tx, incident_id)?.ok_or_else(|| not_found(incident_id))?;
    if current.state != IncidentState::Open {
        return Ok(current);
    }
    let open_actionable: i64 = tx.query_row(
        "SELECT COUNT(*) FROM attention_events
         WHERE incident_id = ?1 AND resolved_at IS NULL AND requires_action = 1",
        [incident_id],
        |row| row.get(0),
    )?;
    if open_actionable > 0 {
        return Ok(current);
    }
    tx.execute(
        "UPDATE incidents SET closed_at = ?2 WHERE id = ?1",
        params![incident_id, rfc3339(now)],
    )?;
    append_entry(tx, incident_id, &IncidentEntryDetail::Closed, None, now)?;
    load_incident(tx, incident_id)?.ok_or_else(|| not_found(incident_id))
}

fn encode_list_cursor(opened_at: &str, id: &str) -> String {
    format!("{opened_at}|{id}")
}

fn decode_list_cursor(cursor: &str) -> Result<(String, String), StorageError> {
    cursor
        .split_once('|')
        .map(|(opened_at, id)| (opened_at.to_string(), id.to_string()))
        .ok_or(StorageError::CorruptData {
            source_name: "incident cursor",
            source_sha256: None,
        })
}

/// `list_incidents`: `openedAt` descending, then id descending, filtered
/// by `state` (`None` = "all") and `printerId`, paginated by `before`
/// (opaque, exclusive).
pub fn list(
    conn: &Connection,
    state: Option<IncidentState>,
    printer_id: Option<&str>,
    before: Option<&str>,
    limit: i64,
) -> Result<IncidentPage, StorageError> {
    let (opened_before, id_before) = match before {
        Some(cursor) => {
            let (opened_at, id) = decode_list_cursor(cursor)?;
            (Some(opened_at), Some(id))
        }
        None => (None, None),
    };
    let closed_filter: Option<i64> = state.map(|state| match state {
        IncidentState::Open => 0,
        IncidentState::Closed => 1,
    });
    let mut statement = conn.prepare(
        "SELECT id, opened_at FROM incidents
         WHERE (?1 IS NULL OR printer_id = ?1)
           AND (?2 IS NULL
                OR (?2 = 0 AND closed_at IS NULL)
                OR (?2 = 1 AND closed_at IS NOT NULL))
           AND (?3 IS NULL OR opened_at < ?3 OR (opened_at = ?3 AND id < ?4))
         ORDER BY opened_at DESC, id DESC
         LIMIT ?5",
    )?;
    let mut rows: Vec<(String, String)> = statement
        .query_map(
            params![
                printer_id,
                closed_filter,
                opened_before,
                id_before,
                limit + 1
            ],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.truncate(limit as usize);
    }
    let next_cursor = has_more.then(|| {
        let (id, opened_at) = rows.last().expect("has_more implies at least one row");
        encode_list_cursor(opened_at, id)
    });
    let mut incidents = Vec::with_capacity(rows.len());
    for (id, _) in rows {
        incidents.push(load_incident(conn, &id)?.expect("row just selected by id"));
    }
    Ok(IncidentPage {
        incidents,
        next_cursor,
    })
}

/// Whether `cursor` has the shape [`list`] mints (`openedAt|id`), so a
/// command can reject a hand-made one as `VALIDATION`.
pub fn is_well_formed_cursor(cursor: &str) -> bool {
    cursor
        .split_once('|')
        .is_some_and(|(opened_at, id)| !opened_at.is_empty() && !id.is_empty())
}

/// Every open Incident, `openedAt` descending then id descending
/// (`AttentionBackfill.openIncidents`).
pub fn list_open(conn: &Connection) -> Result<Vec<Incident>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT id FROM incidents WHERE closed_at IS NULL ORDER BY opened_at DESC, id DESC",
    )?;
    let ids = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.iter()
        .map(|id| load_incident(conn, id)?.ok_or(StorageError::OperationFailed))
        .collect()
}

/// The Incident's own timeline, in sequence order.
pub fn entries(conn: &Connection, incident_id: &str) -> Result<Vec<IncidentEntry>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM incident_events WHERE incident_id = ?1 ORDER BY sequence"
    ))?;
    let rows = statement
        .query_map([incident_id], decode_entry_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The Incident's camera evidence, `capturedAt` then id. Never `rel_path`.
pub fn snapshots_for_incident(
    conn: &Connection,
    incident_id: &str,
) -> Result<Vec<crate::cameras::CameraSnapshot>, StorageError> {
    snapshots_where(conn, "incident_id", incident_id)
}

/// The Job's camera evidence (`camera_snapshots.job_id`), pruned rows
/// included, `capturedAt` then id. Never `rel_path`.
pub fn snapshots_for_job(
    conn: &Connection,
    job_id: &str,
) -> Result<Vec<crate::cameras::CameraSnapshot>, StorageError> {
    snapshots_where(conn, "job_id", job_id)
}

/// `column` is one of this module's two literal column names, never input.
fn snapshots_where(
    conn: &Connection,
    column: &'static str,
    value: &str,
) -> Result<Vec<crate::cameras::CameraSnapshot>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT id, revision, printer_id, incident_id, job_id, trigger, captured_at, content_type,
                byte_len, sha256, pinned_at, pruned_at, prune_reason
         FROM camera_snapshots WHERE {column} = ?1 ORDER BY captured_at, id"
    ))?;
    let rows = statement
        .query_map([value], |row| {
            let trigger: String = row.get(5)?;
            let content_type: String = row.get(7)?;
            let prune_reason: Option<String> = row.get(12)?;
            Ok(crate::cameras::CameraSnapshot {
                id: row.get(0)?,
                revision: row.get(1)?,
                printer_id: row.get(2)?,
                incident_id: row.get(3)?,
                job_id: row.get(4)?,
                trigger: decode_text_enum(5, &trigger)?,
                captured_at: row.get(6)?,
                content_type: decode_text_enum(7, &content_type)?,
                byte_len: row.get(8)?,
                sha256: row.get(9)?,
                pinned_at: row.get(10)?,
                pruned_at: row.get(11)?,
                prune_reason: prune_reason
                    .map(|text| decode_text_enum(12, &text))
                    .transpose()?,
            })
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// D3: `get_incident`'s assembly. The timeline merges the Incident's own
/// entries with its Job's `job_events` at read time (never copied),
/// ordered by `at`, then Incident entries before Job events at the same
/// instant, then sequence. `None` when there is no such Incident.
pub fn detail(
    conn: &Connection,
    incident_id: &str,
) -> Result<Option<IncidentDetail>, StorageError> {
    let Some(incident) = load_incident(conn, incident_id)? else {
        return Ok(None);
    };
    let mut timeline: Vec<(Option<DateTime<Utc>>, String, u8, i64, IncidentTimelineItem)> =
        Vec::new();
    for entry in entries(conn, incident_id)? {
        timeline.push((
            instant(&entry.at),
            entry.at.clone(),
            0,
            entry.sequence,
            IncidentTimelineItem::Incident { entry },
        ));
    }
    if let Some(job_id) = &incident.job_id {
        for event in crate::jobs::repository::list_events(conn, job_id)? {
            timeline.push((
                instant(&event.at),
                event.at.clone(),
                1,
                event.sequence,
                IncidentTimelineItem::Job { event },
            ));
        }
    }
    timeline.sort_by(|left, right| {
        left.0
            .cmp(&right.0)
            // The text only orders what couldn't be parsed.
            .then_with(|| match left.0 {
                None => left.1.cmp(&right.1),
                Some(_) => std::cmp::Ordering::Equal,
            })
            .then_with(|| left.2.cmp(&right.2))
            .then_with(|| left.3.cmp(&right.3))
    });
    Ok(Some(IncidentDetail {
        events: crate::attention::repository::events_for_incident(conn, incident_id)?,
        snapshots: snapshots_for_incident(conn, incident_id)?,
        timeline: timeline.into_iter().map(|(.., item)| item).collect(),
        incident,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attention::{
        self, AttentionDetail, AttentionOrigin, AttentionResolution, AttentionSubject,
        ConditionKind,
    };
    use crate::persistence::{MetadataRootLease, Storage, StoragePaths};

    const NOW_TEXT: &str = "2026-09-28T12:00:00Z";

    fn now() -> DateTime<Utc> {
        NOW_TEXT.parse().unwrap()
    }

    fn storage() -> (tempfile::TempDir, Storage) {
        let temp = tempfile::tempdir().unwrap();
        let paths =
            StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
        let lease = MetadataRootLease::acquire(&paths).unwrap();
        let storage = Storage::open(paths, &lease).unwrap();
        (temp, storage)
    }

    fn seed_printer(tx: &Transaction<'_>, id: &str) {
        tx.execute_batch(&format!(
            "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES ('{id}', 1, 'Printer', '', '', '', '', '', '', '{{}}',
                     '{NOW_TEXT}', '{NOW_TEXT}');"
        ))
        .unwrap();
    }

    /// A Printer -> Slice Revision -> Spool -> Reservation -> Queue Entry
    /// -> Job chain, minimal enough for `jobs.printer_id`/`attention_events`
    /// FKs. Assumes the Printer (`printer_id`) already exists.
    fn seed_job(tx: &Transaction<'_>, job_id: &str, printer_id: &str) {
        let hash = "b".repeat(64);
        tx.execute_batch(&format!(
            "INSERT INTO content_blobs(sha256, size_bytes, created_at)
               VALUES ('{hash}', 1, '{NOW_TEXT}');
             INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
               VALUES ('mdl-{job_id}', 1, 'Model', 'gcode', 'managed', '{NOW_TEXT}', '{NOW_TEXT}');
             INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256, size_bytes,
               format, origin, source_file_name, source_path, captured_at, inspector_version,
               inspection_json)
               VALUES ('msr-{job_id}', 'mdl-{job_id}', 1, '{hash}', 1, 'gcode', 'import', 'p.gcode',
                       '/p.gcode', '{NOW_TEXT}', 1, '{{}}');
             INSERT INTO slice_revisions(id, kind, model_id, source_revision_id, gcode_sha256,
               gcode_size, target_json, facts_json, requires_manual_printer_selection,
               estimates_json, created_at)
               VALUES ('slr-{job_id}', 'external', 'mdl-{job_id}', 'msr-{job_id}', '{hash}', 1,
                       '{{}}', '{{}}', 1, '{{}}', '{NOW_TEXT}');
             INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
               color_name, diameter, nominal_mg, current_mg, confidence, lifecycle, created_at,
               updated_at)
               VALUES ('spl-{job_id}', 1, 1, 'Acme', 'PLA', 'Black', '1.75', 1000000, 1000000,
                       'measured', 'active', '{NOW_TEXT}', '{NOW_TEXT}');
             INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id, amount_mg, state,
               operation_id, created_at)
               VALUES ('rsv-{job_id}', 'spl-{job_id}', 'job', '{job_id}', 500000, 'active',
                       'rsv-{job_id}-op', '{NOW_TEXT}');
             INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id, copy_index,
               state, position, policy, preference, estimate_mg, estimate_source, created_at,
               updated_at)
               VALUES ('qen-{job_id}', 1, 'slr-{job_id}', 'qln-{job_id}', 1, 'queued', 1, 'manual',
                       'loadedFirst', 500000, 'operatorEntered', '{NOW_TEXT}', '{NOW_TEXT}');
             INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
               printer_snapshot_json, spool_id, reservation_id, estimate_mg, state, settlement,
               assigned_by, created_at, updated_at)
               VALUES ('{job_id}', 1, 'qen-{job_id}', 'slr-{job_id}', '{printer_id}', '{{}}',
                       'spl-{job_id}', 'rsv-{job_id}', 500000, 'assigned', 'open', 'operator',
                       '{NOW_TEXT}', '{NOW_TEXT}');"
        ))
        .unwrap();
    }

    fn printer_snapshot() -> PrinterSnapshot {
        use crate::catalog::{BedShape, PrinterProfile};
        PrinterSnapshot {
            name: "Voron".to_string(),
            location: Some("Bay A".to_string()),
            catalog_ref: None,
            adapter_kind: None,
            profile: PrinterProfile {
                bed_shape: BedShape::Rectangular {
                    width_mm: 250.0,
                    depth_mm: 250.0,
                    origin_x_mm: 0.0,
                    origin_y_mm: 0.0,
                },
                printable_height_mm: 250.0,
                bed_exclude_areas: Vec::new(),
                default_bed_type: "4".to_string(),
                nozzle_diameter_mm: vec![0.4],
                nozzle_type: "brass".to_string(),
                gcode_flavor: "marlin".to_string(),
                has_auxiliary_fan: false,
                supports_air_filtration: false,
                supports_multi_filament: false,
                suggested_host_type: None,
            },
        }
    }

    fn subject() -> AttentionSubject {
        AttentionSubject {
            printer_name: Some("Voron".into()),
            printer_location: Some("Bay A".into()),
            job_label: Some("Cube".into()),
            spool_number: None,
            spool_label: None,
        }
    }

    fn job_failed(job_id: &str, printer_id: &str) -> attention::Condition {
        attention::Condition {
            kind: ConditionKind::JobFailed,
            source_id: job_id.to_string(),
            printer_id: Some(printer_id.to_string()),
            job_id: Some(job_id.to_string()),
            spool_id: None,
            requirement_id: None,
            subject: subject(),
            detail: AttentionDetail::JobFailed {
                ended_at: NOW_TEXT.to_string(),
            },
            acknowledge: false,
        }
    }

    fn job_host_cancelled(job_id: &str, printer_id: &str) -> attention::Condition {
        attention::Condition {
            kind: ConditionKind::JobHostCancelled,
            source_id: job_id.to_string(),
            printer_id: Some(printer_id.to_string()),
            job_id: Some(job_id.to_string()),
            spool_id: None,
            requirement_id: None,
            subject: subject(),
            detail: AttentionDetail::JobHostCancelled {
                ended_at: NOW_TEXT.to_string(),
            },
            acknowledge: false,
        }
    }

    fn host_failed(printer_id: &str) -> attention::Condition {
        attention::Condition {
            kind: ConditionKind::PrinterHostFailed,
            source_id: printer_id.to_string(),
            printer_id: Some(printer_id.to_string()),
            job_id: None,
            spool_id: None,
            requirement_id: None,
            subject: subject(),
            detail: AttentionDetail::PrinterHostFailed,
            acknowledge: false,
        }
    }

    #[test]
    fn open_writes_an_opened_entry_and_links_the_seeding_event() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                let event = attention::repository::insert(
                    tx,
                    &host_failed("prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let incident = open(
                    tx,
                    IncidentKind::PrinterHostFailed,
                    "prn-a",
                    None,
                    &printer_snapshot(),
                    &event.id,
                    now(),
                )?;
                assert_eq!(incident.revision, 1);
                assert_eq!(incident.state, IncidentState::Open);
                assert_eq!(incident.linked_event_ids, vec![event.id.clone()]);
                assert_eq!(incident.open_linked_event_count, 1);

                let linked_event = attention::repository::load_event(tx, &event.id)?.unwrap();
                assert_eq!(
                    linked_event.incident_id.as_deref(),
                    Some(incident.id.as_str())
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn a_second_open_for_the_same_job_returns_the_existing_incident() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                seed_job(tx, "job-a", "prn-a");
                let e1 = attention::repository::insert(
                    tx,
                    &job_failed("job-a", "prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let first = open(
                    tx,
                    IncidentKind::JobFailed,
                    "prn-a",
                    Some("job-a"),
                    &printer_snapshot(),
                    &e1.id,
                    now(),
                )?;
                let second = open(
                    tx,
                    IncidentKind::JobFailed,
                    "prn-a",
                    Some("job-a"),
                    &printer_snapshot(),
                    "att-unused",
                    now(),
                )?;
                assert_eq!(first.id, second.id);
                assert_eq!(second.revision, 1, "the idempotent hit changes nothing");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn link_event_appends_and_bumps_revision_and_reopens_a_closed_incident() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                seed_job(tx, "job-a", "prn-a");
                let t0 = now();
                let t1 = t0 + chrono::Duration::minutes(1);
                let e1 = attention::repository::insert(
                    tx,
                    &job_failed("job-a", "prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    t0,
                )?;
                let incident = open(
                    tx,
                    IncidentKind::JobFailed,
                    "prn-a",
                    Some("job-a"),
                    &printer_snapshot(),
                    &e1.id,
                    t0,
                )?;
                assert_eq!(incident.revision, 1);

                // Resolve e1 and close the Incident.
                attention::repository::resolve(
                    tx,
                    &e1.id,
                    AttentionResolution::OperatorResolved,
                    t0,
                )?;
                let closed = close_if_settled(tx, &incident.id, t0)?;
                assert_eq!(closed.state, IncidentState::Closed);
                assert_eq!(closed.revision, 2, "closing bumps once");

                // A second Event links in and reopens it.
                let e2 = attention::repository::insert(
                    tx,
                    &job_host_cancelled("job-a", "prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    t1,
                )?;
                let reopened = link_event(tx, &incident.id, &e2.id, t1)?;
                assert_eq!(reopened.state, IncidentState::Open);
                assert_eq!(reopened.closed_at, None);
                // +1 for `reopened`, +1 for `eventLinked`.
                assert_eq!(reopened.revision, 4);
                assert_eq!(
                    reopened.linked_event_ids,
                    vec![e1.id.clone(), e2.id.clone()]
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn append_entry_sequence_is_monotonic_per_incident() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                let event = attention::repository::insert(
                    tx,
                    &host_failed("prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let incident = open(
                    tx,
                    IncidentKind::PrinterHostFailed,
                    "prn-a",
                    None,
                    &printer_snapshot(),
                    &event.id,
                    now(),
                )?;
                let note1 = append_entry(
                    tx,
                    &incident.id,
                    &IncidentEntryDetail::NoteAdded {
                        text: "first note".to_string(),
                    },
                    None,
                    now(),
                )?;
                let note2 = append_entry(
                    tx,
                    &incident.id,
                    &IncidentEntryDetail::NoteAdded {
                        text: "second note".to_string(),
                    },
                    None,
                    now(),
                )?;
                assert_eq!(note1.sequence, 2, "sequence 1 is the opened entry");
                assert_eq!(note2.sequence, 3);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn close_if_settled_only_closes_when_every_linked_actionable_event_is_resolved() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                seed_job(tx, "job-a", "prn-a");
                let e1 = attention::repository::insert(
                    tx,
                    &job_failed("job-a", "prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let incident = open(
                    tx,
                    IncidentKind::JobFailed,
                    "prn-a",
                    Some("job-a"),
                    &printer_snapshot(),
                    &e1.id,
                    now(),
                )?;

                let e2 = attention::repository::insert(
                    tx,
                    &job_host_cancelled("job-a", "prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                link_event(tx, &incident.id, &e2.id, now())?;

                // Only e1 resolved: still open.
                attention::repository::resolve(
                    tx,
                    &e1.id,
                    AttentionResolution::OperatorResolved,
                    now(),
                )?;
                let still_open = close_if_settled(tx, &incident.id, now())?;
                assert_eq!(still_open.state, IncidentState::Open);

                // Both resolved: closes.
                attention::repository::resolve(
                    tx,
                    &e2.id,
                    AttentionResolution::ActionCompleted,
                    now(),
                )?;
                let closed = close_if_settled(tx, &incident.id, now())?;
                assert_eq!(closed.state, IncidentState::Closed);
                assert!(closed.closed_at.is_some());

                // Calling it again on an already-closed Incident is a no-op.
                let revision_before = closed.revision;
                let unchanged = close_if_settled(tx, &incident.id, now())?;
                assert_eq!(unchanged.revision, revision_before);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn list_orders_by_opened_at_descending_then_id_and_filters_by_state() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                seed_printer(tx, "prn-b");
                let t0 = now();
                let t1 = t0 + chrono::Duration::minutes(1);
                let e1 = attention::repository::insert(
                    tx,
                    &host_failed("prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    t0,
                )?;
                let older = open(
                    tx,
                    IncidentKind::PrinterHostFailed,
                    "prn-a",
                    None,
                    &printer_snapshot(),
                    &e1.id,
                    t0,
                )?;
                let e2 = attention::repository::insert(
                    tx,
                    &host_failed("prn-b"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    t1,
                )?;
                let newer = open(
                    tx,
                    IncidentKind::PrinterHostFailed,
                    "prn-b",
                    None,
                    &printer_snapshot(),
                    &e2.id,
                    t1,
                )?;

                let all = list(tx, None, None, None, 200)?;
                assert_eq!(
                    all.incidents
                        .iter()
                        .map(|i| i.id.clone())
                        .collect::<Vec<_>>(),
                    [newer.id.clone(), older.id.clone()]
                );

                attention::repository::resolve(
                    tx,
                    &e1.id,
                    AttentionResolution::ConditionCleared,
                    t0,
                )?;
                close_if_settled(tx, &older.id, t0)?;

                let open_only = list(tx, Some(IncidentState::Open), None, None, 200)?;
                assert_eq!(
                    open_only
                        .incidents
                        .iter()
                        .map(|i| i.id.clone())
                        .collect::<Vec<_>>(),
                    [newer.id.clone()]
                );
                let closed_only = list(tx, Some(IncidentState::Closed), None, None, 200)?;
                assert_eq!(
                    closed_only
                        .incidents
                        .iter()
                        .map(|i| i.id.clone())
                        .collect::<Vec<_>>(),
                    [older.id.clone()]
                );
                Ok(())
            })
            .unwrap();
    }
}
