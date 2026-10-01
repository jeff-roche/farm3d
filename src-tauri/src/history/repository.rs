//! P9 D11: the history query builder, the cursor codec, and the timeline
//! joins. Everything here reads; nothing writes.
//!
//! The list query names its index (`INDEXED BY`), because farm3d never
//! runs `ANALYZE` and the planner would otherwise be free to pick
//! `jobs_state` and a temporary B-tree for the first page. `INDEXED BY`
//! fails the statement when the index can't serve it, so a regression is
//! an error rather than a slow scan.

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Value;
use rusqlite::{params_from_iter, Connection, OptionalExtension};

use crate::incidents::repository as incidents_repository;
use crate::jobs::repository as jobs_repository;
use crate::persistence::StorageError;
use crate::slicing::repository as slicing_repository;
use crate::spools::decode_enum;
use crate::spools::ledger;

use super::{
    JobHistoryPage, JobHistoryQuery, JobHistoryRow, JobHistoryState, JobTimeline, JobTimelineItem,
    JobTimelineSliceRevision, PrinterLifecycleFilter,
};

/// The default page size.
pub const DEFAULT_LIMIT: i64 = 50;
/// The largest page.
pub const MAX_LIMIT: i64 = 200;
/// The longest search text, in characters after trimming.
pub const TEXT_MAX_CHARS: usize = 200;

/// A query field that failed validation: `field` is the `VALIDATION`
/// field path (`query.text`, ...).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct QueryError {
    pub field: &'static str,
    pub message: &'static str,
}

/// A [`JobHistoryQuery`] that passed [`validate`].
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct ValidQuery {
    states: Vec<JobHistoryState>,
    lifecycle: PrinterLifecycleFilter,
    printer_id: Option<String>,
    spool_id: Option<String>,
    model_id: Option<String>,
    ended_after: Option<String>,
    ended_before: Option<String>,
    text: Option<String>,
    after: Option<(String, String)>,
    limit: i64,
}

fn error(field: &'static str, message: &'static str) -> QueryError {
    QueryError { field, message }
}

/// Checks every field and applies the defaults.
pub fn validate(query: &JobHistoryQuery) -> Result<ValidQuery, QueryError> {
    let states = match &query.states {
        None => JobHistoryState::DEFAULT.to_vec(),
        Some(states) => {
            let mut distinct = states.clone();
            distinct.sort_by_key(|state| state.as_sql());
            distinct.dedup();
            if states.is_empty() || distinct.len() != states.len() {
                return Err(error(
                    "query.states",
                    "states must hold one to four distinct states",
                ));
            }
            states.clone()
        }
    };
    let limit = query.limit.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        return Err(error("query.limit", "limit must be between 1 and 200"));
    }
    let text = match query.text.as_deref().map(str::trim) {
        None | Some("") => None,
        Some(text) if text.chars().count() > TEXT_MAX_CHARS => {
            return Err(error("query.text", "text must be at most 200 characters"))
        }
        Some(text) => Some(text.to_string()),
    };
    let after = query
        .after
        .as_deref()
        .map(|cursor| {
            decode_cursor(cursor).ok_or_else(|| {
                error(
                    "query.after",
                    "after must be a cursor list_job_history returned",
                )
            })
        })
        .transpose()?;
    Ok(ValidQuery {
        states,
        lifecycle: query.printer_lifecycle.unwrap_or_default(),
        printer_id: query.printer_id.clone(),
        spool_id: query.spool_id.clone(),
        model_id: query.model_id.clone(),
        ended_after: query
            .ended_after
            .as_deref()
            .map(|at| {
                normalize_instant(at)
                    .ok_or_else(|| error("query.endedAfter", "endedAfter must be an RFC 3339 time"))
            })
            .transpose()?,
        ended_before: query
            .ended_before
            .as_deref()
            .map(|at| {
                normalize_instant(at).ok_or_else(|| {
                    error("query.endedBefore", "endedBefore must be an RFC 3339 time")
                })
            })
            .transpose()?,
        text,
        after,
        limit,
    })
}

/// The instant as UTC RFC 3339. Stored timestamps carry a varying number
/// of fraction digits, so the query compares `julianday` values rather than
/// this text.
fn normalize_instant(text: &str) -> Option<String> {
    DateTime::parse_from_rfc3339(text).ok().map(|at| {
        at.with_timezone(&Utc)
            .to_rfc3339_opts(SecondsFormat::AutoSi, true)
    })
}

/// The cursor for the row keyed `(history_at, id)`.
pub fn encode_cursor(history_at: &str, id: &str) -> String {
    URL_SAFE_NO_PAD.encode(format!("{history_at}|{id}"))
}

fn decode_cursor(cursor: &str) -> Option<(String, String)> {
    let bytes = URL_SAFE_NO_PAD.decode(cursor).ok()?;
    let text = String::from_utf8(bytes).ok()?;
    let (at, id) = text.split_once('|')?;
    (!at.is_empty() && !id.is_empty()).then(|| (at.to_string(), id.to_string()))
}

/// Escapes `\`, `%`, and `_` for `LIKE ... ESCAPE '\'`.
fn escape_like(text: &str) -> String {
    let mut escaped = String::with_capacity(text.len());
    for character in text.chars() {
        if matches!(character, '\\' | '%' | '_') {
            escaped.push('\\');
        }
        escaped.push(character);
    }
    escaped
}

/// The index the query names: the Printer one when a Printer is given.
pub fn index_for(query: &ValidQuery) -> &'static str {
    if query.printer_id.is_some() {
        "jobs_history_printer"
    } else {
        "jobs_history"
    }
}

const HISTORY_AT: &str = "COALESCE(j.ended_at, j.created_at)";

/// The list statement and its parameters, `limit + 1` rows.
pub fn build_list_sql(query: &ValidQuery) -> (String, Vec<Value>) {
    let mut params: Vec<Value> = Vec::new();
    let push = |params: &mut Vec<Value>, value: Value| -> String {
        params.push(value);
        format!("?{}", params.len())
    };
    let mut clauses: Vec<String> = vec![
        // The index's own term, so the partial indexes apply.
        "j.state IN ('completed','failed','cancelled','outcomeUnknown')".to_string(),
    ];
    let states = query
        .states
        .iter()
        .map(|state| push(&mut params, Value::Text(state.as_sql().to_string())))
        .collect::<Vec<_>>()
        .join(",");
    clauses.push(format!("j.state IN ({states})"));
    match query.lifecycle {
        PrinterLifecycleFilter::Any => {}
        PrinterLifecycleFilter::Active => clauses.push("p.archived_at IS NULL".to_string()),
        PrinterLifecycleFilter::Archived => clauses.push("p.archived_at IS NOT NULL".to_string()),
    }
    if let Some(id) = &query.printer_id {
        let placeholder = push(&mut params, Value::Text(id.clone()));
        clauses.push(format!("j.printer_id = {placeholder}"));
    }
    if let Some(id) = &query.spool_id {
        let placeholder = push(&mut params, Value::Text(id.clone()));
        clauses.push(format!("j.spool_id = {placeholder}"));
    }
    if let Some(id) = &query.model_id {
        let placeholder = push(&mut params, Value::Text(id.clone()));
        clauses.push(format!("sr.model_id = {placeholder}"));
    }
    if let Some(at) = &query.ended_after {
        let placeholder = push(&mut params, Value::Text(at.clone()));
        clauses.push(format!(
            "julianday({HISTORY_AT}) >= julianday({placeholder})"
        ));
    }
    if let Some(at) = &query.ended_before {
        let placeholder = push(&mut params, Value::Text(at.clone()));
        clauses.push(format!(
            "julianday({HISTORY_AT}) < julianday({placeholder})"
        ));
    }
    if let Some(text) = &query.text {
        let pattern = push(&mut params, Value::Text(escape_like(text)));
        let mut any = vec![
            format!("json_extract(j.printer_snapshot_json, '$.name') LIKE '%' || {pattern} || '%' ESCAPE '\\'"),
            format!("m.name LIKE '%' || {pattern} || '%' ESCAPE '\\'"),
            format!("sr.plate_name LIKE '%' || {pattern} || '%' ESCAPE '\\'"),
            format!("j.id LIKE {pattern} || '%' ESCAPE '\\'"),
        ];
        let digits = text.strip_prefix('#').unwrap_or(text);
        if !digits.is_empty() && digits.bytes().all(|byte| byte.is_ascii_digit()) {
            if let Ok(number) = digits.parse::<i64>() {
                let placeholder = push(&mut params, Value::Integer(number));
                any.push(format!("s.spool_number = {placeholder}"));
            }
        }
        clauses.push(format!("({})", any.join(" OR ")));
    }
    if let Some((at, id)) = &query.after {
        let at = push(&mut params, Value::Text(at.clone()));
        let id = push(&mut params, Value::Text(id.clone()));
        clauses.push(format!(
            "{HISTORY_AT} <= {at} AND ({HISTORY_AT} < {at} OR j.id < {id})"
        ));
    }
    let limit = push(&mut params, Value::Integer(query.limit + 1));
    let index = index_for(query);
    let sql = format!(
        "SELECT j.id, j.state, j.cancel_reason, {HISTORY_AT}, j.started_at, j.ended_at,
                j.printer_id, json_extract(j.printer_snapshot_json, '$.name'),
                p.archived_at IS NOT NULL, j.spool_id, s.spool_number, sr.model_id, m.name,
                j.slice_revision_id, sr.plate_name, j.settlement,
                (SELECT i.id FROM incidents i WHERE i.job_id = j.id LIMIT 1),
                (SELECT COUNT(*) FROM camera_snapshots c WHERE c.job_id = j.id)
         FROM jobs j INDEXED BY {index}
         JOIN printers p ON p.id = j.printer_id
         JOIN spools s ON s.id = j.spool_id
         JOIN slice_revisions sr ON sr.id = j.slice_revision_id
         JOIN library_models m ON m.id = sr.model_id
         WHERE {clauses}
         ORDER BY {HISTORY_AT} DESC, j.id DESC
         LIMIT {limit}",
        clauses = clauses.join("\n           AND "),
    );
    (sql, params)
}

fn decode_text<T: serde::de::DeserializeOwned>(index: usize, text: &str) -> rusqlite::Result<T> {
    decode_enum(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(
            index,
            rusqlite::types::Type::Text,
            Box::new(error),
        )
    })
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobHistoryRow> {
    let state: String = row.get(1)?;
    let cancel_reason: Option<String> = row.get(2)?;
    let settlement: String = row.get(15)?;
    Ok(JobHistoryRow {
        job_id: row.get(0)?,
        state: decode_text(1, &state)?,
        cancel_reason: cancel_reason
            .map(|text| decode_text(2, &text))
            .transpose()?,
        history_at: row.get(3)?,
        started_at: row.get(4)?,
        ended_at: row.get(5)?,
        printer_id: row.get(6)?,
        printer_snapshot_name: row.get::<_, Option<String>>(7)?.unwrap_or_default(),
        printer_archived: row.get(8)?,
        spool_id: row.get(9)?,
        spool_number: row.get(10)?,
        model_id: row.get(11)?,
        model_name: row.get(12)?,
        slice_revision_id: row.get(13)?,
        plate_name: row.get(14)?,
        settlement: decode_text(15, &settlement)?,
        incident_id: row.get(16)?,
        snapshot_count: row.get(17)?,
    })
}

/// `list_job_history`: one page, newest first.
pub fn list(conn: &Connection, query: &ValidQuery) -> Result<JobHistoryPage, StorageError> {
    let (sql, params) = build_list_sql(query);
    let mut statement = conn.prepare(&sql)?;
    let mut rows = statement
        .query_map(params_from_iter(params), decode_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = rows.len() as i64 > query.limit;
    rows.truncate(query.limit as usize);
    let next_cursor = has_more
        .then(|| rows.last())
        .flatten()
        .map(|row| encode_cursor(&row.history_at, &row.job_id));
    Ok(JobHistoryPage { rows, next_cursor })
}

fn instant(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// The source order at one instant.
fn rank(item: &JobTimelineItem) -> u8 {
    match item {
        JobTimelineItem::Job { .. } => 0,
        JobTimelineItem::HostOperation { .. } => 1,
        JobTimelineItem::Reservation { .. } => 2,
        JobTimelineItem::AmountEvent { .. } => 3,
        JobTimelineItem::Requirement { .. } => 4,
        JobTimelineItem::Attention { .. } => 5,
        JobTimelineItem::Incident { .. } => 6,
        JobTimelineItem::Snapshot { .. } => 7,
    }
}

/// The source's own sequence, else `None`.
fn sequence(item: &JobTimelineItem) -> Option<i64> {
    match item {
        JobTimelineItem::Job { event, .. } => Some(event.sequence),
        JobTimelineItem::AmountEvent { amount_event, .. } => Some(amount_event.sequence),
        JobTimelineItem::Incident { entry, .. } => Some(entry.sequence),
        _ => None,
    }
}

fn id(item: &JobTimelineItem) -> &str {
    match item {
        JobTimelineItem::Job { event, .. } => &event.id,
        JobTimelineItem::HostOperation { host_operation, .. } => &host_operation.id,
        JobTimelineItem::Reservation { reservation, .. } => &reservation.id,
        JobTimelineItem::AmountEvent { amount_event, .. } => &amount_event.id,
        JobTimelineItem::Requirement { requirement, .. } => &requirement.id,
        JobTimelineItem::Attention { event, .. } => &event.id,
        JobTimelineItem::Incident { entry, .. } => &entry.id,
        JobTimelineItem::Snapshot { snapshot, .. } => &snapshot.id,
    }
}

/// `at` ascending, then source, then the source's sequence, else id
/// bytewise. A time that doesn't parse sorts before every parsed one, by
/// its text.
fn order(left: &JobTimelineItem, right: &JobTimelineItem) -> std::cmp::Ordering {
    let (left_at, right_at) = (instant(left.at()), instant(right.at()));
    left_at
        .cmp(&right_at)
        .then_with(|| {
            if left_at.is_none() {
                left.at().cmp(right.at())
            } else {
                std::cmp::Ordering::Equal
            }
        })
        .then_with(|| rank(left).cmp(&rank(right)))
        .then_with(|| sequence(left).cmp(&sequence(right)))
        .then_with(|| id(left).as_bytes().cmp(id(right).as_bytes()))
}

/// `get_job_timeline`'s assembly, or `None` for an unknown Job. Every item
/// is read from a row that never changes after the Job settles, except a
/// snapshot's pruned fields.
pub fn timeline(conn: &Connection, job_id: &str) -> Result<Option<JobTimeline>, StorageError> {
    if jobs_repository::load_job(conn, job_id)?.is_none() {
        return Ok(None);
    }
    let history = jobs_repository::history(conn, job_id)?;
    let job = history.job;
    let correction_event_id: Option<String> = conn
        .query_row(
            "SELECT correction_event_id FROM jobs WHERE id = ?1",
            [job_id],
            |row| row.get(0),
        )
        .optional()?
        .flatten();
    let spool_number: i64 = conn.query_row(
        "SELECT spool_number FROM spools WHERE id = ?1",
        [&job.spool_id],
        |row| row.get(0),
    )?;
    let revision = slicing_repository::load_revision(conn, &job.slice_revision_id)?
        .ok_or(StorageError::OperationFailed)?;
    let summary = revision.summary;
    let slice_revision = JobTimelineSliceRevision {
        id: summary.id,
        kind: summary.kind,
        model_id: summary.model_id,
        source_revision_id: summary.source_revision_id,
        plate: summary.plate,
        target: revision.target,
        runtime: summary.runtime,
        estimates: summary.estimates,
        facts: summary.facts,
        created_at: summary.created_at,
    };
    let incident = incidents_repository::for_job(conn, job_id)?;

    let mut items: Vec<JobTimelineItem> = Vec::new();
    for event in history.events {
        items.push(JobTimelineItem::Job {
            at: event.at.clone(),
            event,
        });
    }
    for host_operation in history.host_operations {
        items.push(JobTimelineItem::HostOperation {
            at: host_operation.created_at.clone(),
            host_operation,
        });
    }
    for reservation in history.reservations {
        items.push(JobTimelineItem::Reservation {
            at: reservation.created_at.clone(),
            reservation,
        });
    }
    for amount_event in ledger::history(conn, &job.spool_id)? {
        let is_correction = correction_event_id.as_deref() == Some(amount_event.id.as_str());
        let of_this_job = is_correction
            || amount_event.reservation_id.as_deref() == Some(job.reservation_id.as_str());
        if of_this_job {
            items.push(JobTimelineItem::AmountEvent {
                at: amount_event.occurred_at.clone(),
                amount_event,
                is_correction,
            });
        }
    }
    for requirement in history.requirements {
        items.push(JobTimelineItem::Requirement {
            at: requirement.opened_at.clone(),
            requirement,
        });
    }
    for event in crate::attention::repository::events_for_job(conn, job_id)? {
        items.push(JobTimelineItem::Attention {
            at: event.first_observed_at.clone(),
            event,
        });
    }
    if let Some(incident) = &incident {
        for entry in incidents_repository::entries(conn, &incident.id)? {
            items.push(JobTimelineItem::Incident {
                at: entry.at.clone(),
                entry,
            });
        }
    }
    for snapshot in incidents_repository::snapshots_for_job(conn, job_id)? {
        items.push(JobTimelineItem::Snapshot {
            at: snapshot.captured_at.clone(),
            snapshot,
        });
    }
    items.sort_by(order);

    Ok(Some(JobTimeline {
        printer_snapshot: job.printer_snapshot.clone(),
        spool_id: job.spool_id.clone(),
        spool_number,
        job,
        entry: history.entry,
        lineage: history.lineage,
        slice_revision,
        incident,
        items,
    }))
}
