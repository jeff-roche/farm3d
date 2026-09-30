//! P8 D1/D2: the SQL for `attention_events`. Every write function takes
//! the caller's `&Transaction` (P3+'s pattern; see `jobs::repository`'s
//! module doc), so the projector (Task 6) composes several of these into
//! one atomic pass and a command composes one of these with
//! `incidents::repository` into one atomic commit.
//!
//! [`insert`] writes a `PlannedAction::Insert`'s row. [`amend`] writes a
//! `PlannedAction::Amend`'s columns, honoring `changed` (never touching
//! `evidence_json`, `incident_id`, or the lifecycle columns either way).
//! [`acknowledge`]/[`resolve`]/[`mark_read`] run the pure
//! `attention::lifecycle::apply` before any SQL and write its
//! [`super::lifecycle::LifecycleChange`] verbatim, bumping `revision` only
//! when it says `changed`. [`record_evidence`] is D2 "Evidence is not
//! detail"'s one-time write to `evidence_json`, entirely separate from
//! `amend`. [`list_attention`] is `list_attention`'s open/resolved-page
//! ordering and cursor (D9 "Commands"). [`resolve_for_printer`] is the
//! Printer delete/import guard's `sourceRemoved` sweep (D8, a later task).

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::library;
use crate::persistence::{RepositoryError, StorageError};
use crate::spools::{decode_enum, encode_enum};

use super::lifecycle::{self, AckBy, LifecycleChange, LifecycleError, LifecycleOp};
use super::{
    AttentionAction, AttentionCursor, AttentionDetail, AttentionEvent, AttentionOrigin,
    AttentionResolution, AttentionSeverity, AttentionSource, Condition, EvidenceOutcome,
    ResolutionMode,
};

const EVENT_ID_PREFIX: &str = "att";

fn new_event_id() -> String {
    library::new_id(EVENT_ID_PREFIX)
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
    serde_json::to_string(value).expect("attention wire types always serialize")
}

fn from_json<T: serde::de::DeserializeOwned>(index: usize, text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

const ATTENTION_COLUMNS: &str = "id, revision, dedup_key, condition, severity, requires_action, \
     resolution_mode, notification_class, source_kind, source_id, printer_id, job_id, spool_id, \
     requirement_id, incident_id, subject_snapshot_json, detail_json, summary, origin, \
     first_observed_at, last_observed_at, observation_count, recurrence_of, read_at, \
     acknowledged_at, resolved_at, resolution, notified_at, evidence_json";

/// D1 "allowedActions": `markRead` if unread; `acknowledge` if open and
/// unacknowledged; `resolve` if open and `manual`.
fn allowed_actions(event: &AttentionEvent) -> Vec<AttentionAction> {
    let mut actions = Vec::new();
    if event.read_at.is_none() {
        actions.push(AttentionAction::MarkRead);
    }
    if event.resolved_at.is_none() {
        if event.acknowledged_at.is_none() {
            actions.push(AttentionAction::Acknowledge);
        }
        if event.resolution_mode == ResolutionMode::Manual {
            actions.push(AttentionAction::Resolve);
        }
    }
    actions
}

fn decode_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<AttentionEvent> {
    let condition_text: String = row.get(3)?;
    let severity_text: String = row.get(4)?;
    let resolution_mode_text: String = row.get(6)?;
    let notification_class_text: String = row.get(7)?;
    let source_kind_text: String = row.get(8)?;
    let source_id: String = row.get(9)?;
    let subject_json: String = row.get(15)?;
    let detail_json: String = row.get(16)?;
    let origin_text: String = row.get(18)?;
    let resolution_text: Option<String> = row.get(26)?;
    let evidence_json: Option<String> = row.get(28)?;

    let resolution_mode: ResolutionMode = decode_text_enum(6, &resolution_mode_text)?;

    let mut event = AttentionEvent {
        id: row.get(0)?,
        revision: row.get(1)?,
        dedup_key: row.get(2)?,
        condition: decode_text_enum(3, &condition_text)?,
        severity: decode_text_enum(4, &severity_text)?,
        requires_action: row.get(5)?,
        resolution_mode,
        notification_class: decode_text_enum(7, &notification_class_text)?,
        source: AttentionSource {
            kind: decode_text_enum(8, &source_kind_text)?,
            id: source_id,
        },
        printer_id: row.get(10)?,
        job_id: row.get(11)?,
        spool_id: row.get(12)?,
        requirement_id: row.get(13)?,
        incident_id: row.get(14)?,
        subject: from_json(15, &subject_json)?,
        detail: from_json(16, &detail_json)?,
        summary: row.get(17)?,
        origin: decode_text_enum(18, &origin_text)?,
        first_observed_at: row.get(19)?,
        last_observed_at: row.get(20)?,
        observation_count: row.get(21)?,
        recurrence_of: row.get(22)?,
        read_at: row.get(23)?,
        acknowledged_at: row.get(24)?,
        resolved_at: row.get(25)?,
        resolution: resolution_text
            .map(|text| decode_text_enum::<AttentionResolution>(26, &text))
            .transpose()?,
        notified_at: row.get(27)?,
        evidence: evidence_json
            .map(|text| from_json::<EvidenceOutcome>(28, &text))
            .transpose()?,
        allowed_actions: Vec::new(),
    };
    event.allowed_actions = allowed_actions(&event);
    Ok(event)
}

/// A single Attention Event by id.
pub fn load_event(conn: &Connection, id: &str) -> Result<Option<AttentionEvent>, StorageError> {
    Ok(conn
        .query_row(
            &format!("SELECT {ATTENTION_COLUMNS} FROM attention_events WHERE id = ?1"),
            [id],
            decode_event_row,
        )
        .optional()?)
}

/// D2 "Apply": a `PlannedAction::Insert`'s row. `first_observed_at =
/// last_observed_at = now`, `observation_count = 1`; when `acknowledged`,
/// `acknowledged_at = read_at = now` (a system acknowledgement — D2's
/// planner rule for a `deferred` material requirement). The partial
/// UNIQUE index on the open `dedup_key` is the backstop against a
/// concurrent duplicate; SQLite raises it as a plain constraint failure.
pub fn insert(
    tx: &Transaction<'_>,
    condition: &Condition,
    recurrence_of: Option<&str>,
    acknowledged: bool,
    origin: AttentionOrigin,
    now: DateTime<Utc>,
) -> Result<AttentionEvent, RepositoryError> {
    let id = new_event_id();
    let now_text = rfc3339(now);
    let spec = condition.spec();
    let ack_at = acknowledged.then(|| now_text.clone());
    tx.execute(
        "INSERT INTO attention_events(
             id, dedup_key, condition, severity, requires_action, resolution_mode,
             notification_class, source_kind, source_id, printer_id, job_id, spool_id,
             requirement_id, subject_snapshot_json, detail_json, summary, origin,
             first_observed_at, last_observed_at, recurrence_of, read_at, acknowledged_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17,
                   ?18, ?18, ?19, ?20, ?20)",
        params![
            id,
            condition.dedup_key(),
            condition.kind.as_str(),
            encode_enum(spec.severity),
            spec.requires_action,
            encode_enum(spec.resolution_mode),
            encode_enum(spec.notification_class),
            spec.source_kind.as_str(),
            condition.source_id,
            condition.printer_id,
            condition.job_id,
            condition.spool_id,
            condition.requirement_id,
            to_json(&condition.subject),
            to_json(&condition.detail),
            condition.summary(),
            encode_enum(origin),
            now_text,
            recurrence_of,
            ack_at,
        ],
    )?;
    load_event(tx, &id)?.ok_or_else(|| not_found(&id))
}

/// D2 "Apply": a `PlannedAction::Amend`. Always bumps `last_observed_at`
/// and `observation_count`; when `changed`, also `detail_json`,
/// `severity`, `summary`, and `revision`. Never `evidence_json`,
/// `incident_id`, or the lifecycle columns. The projector calls this for
/// an unchanged amendment at most once a minute per Event
/// (`projector::persisted_actions`, decision 40).
pub fn amend(
    tx: &Transaction<'_>,
    event_id: &str,
    detail: &AttentionDetail,
    severity: AttentionSeverity,
    summary: &str,
    changed: bool,
    now: DateTime<Utc>,
) -> Result<AttentionEvent, RepositoryError> {
    let now_text = rfc3339(now);
    if changed {
        tx.execute(
            "UPDATE attention_events SET
                 last_observed_at = ?2, observation_count = observation_count + 1,
                 detail_json = ?3, severity = ?4, summary = ?5, revision = revision + 1
             WHERE id = ?1",
            params![
                event_id,
                now_text,
                to_json(detail),
                encode_enum(severity),
                summary
            ],
        )?;
    } else {
        tx.execute(
            "UPDATE attention_events SET
                 last_observed_at = ?2, observation_count = observation_count + 1
             WHERE id = ?1",
            params![event_id, now_text],
        )?;
    }
    load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))
}

/// Writes a non-no-op [`LifecycleChange`] verbatim (`read_at`,
/// `acknowledged_at`, `resolved_at`, `resolution`), bumping `revision`. A
/// no-op (`changed: false`) writes nothing, per D2 "Lifecycle rules".
fn write_lifecycle_change(
    tx: &Transaction<'_>,
    event_id: &str,
    change: &LifecycleChange,
) -> Result<(), RepositoryError> {
    if !change.changed {
        return Ok(());
    }
    tx.execute(
        "UPDATE attention_events SET
             read_at = ?2, acknowledged_at = ?3, resolved_at = ?4, resolution = ?5,
             revision = revision + 1
         WHERE id = ?1",
        params![
            event_id,
            change.read_at,
            change.acknowledged_at,
            change.resolved_at,
            change.resolution.map(encode_enum),
        ],
    )?;
    Ok(())
}

/// D2 "Apply": a `PlannedAction::Acknowledge`, or
/// `acknowledge_attention_event`'s operator call. Returns the current row
/// and whether it changed (a no-op success returns `false`, per D2
/// "Lifecycle rules").
pub fn acknowledge(
    tx: &Transaction<'_>,
    event_id: &str,
    by: AckBy,
    now: DateTime<Utc>,
) -> Result<(AttentionEvent, bool), RepositoryError> {
    let current = load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?;
    let change = lifecycle::apply(&current, LifecycleOp::Acknowledge { by }, now)
        .expect("Acknowledge never errors (LifecycleError is Resolve-only)");
    write_lifecycle_change(tx, event_id, &change)?;
    let updated = load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?;
    Ok((updated, change.changed))
}

/// D2 "Apply"/`resolve_attention_event`: a `PlannedAction::Resolve` (a
/// system resolution) or the operator's manual resolve. Non-`manual` +
/// `operatorResolved` is [`RepositoryError::AttentionNotManual`] (checked
/// before the resolved check — the answer never depends on timing).
/// Returns the current row and whether it changed.
pub fn resolve(
    tx: &Transaction<'_>,
    event_id: &str,
    resolution: AttentionResolution,
    now: DateTime<Utc>,
) -> Result<(AttentionEvent, bool), RepositoryError> {
    let current = load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?;
    let change = lifecycle::apply(&current, LifecycleOp::Resolve(resolution), now).map_err(
        |LifecycleError::NotManual| RepositoryError::AttentionNotManual {
            event_id: event_id.to_string(),
            condition: current.condition,
            resolution_mode: current.resolution_mode,
        },
    )?;
    write_lifecycle_change(tx, event_id, &change)?;
    let updated = load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?;
    Ok((updated, change.changed))
}

/// `mark_attention_read`'s batch: every id in order, `NotFound` on the
/// first unknown one (the whole call runs in the caller's transaction, so
/// a `NotFound` here rolls every prior write in this batch back too).
pub fn mark_read(
    tx: &Transaction<'_>,
    event_ids: &[String],
    now: DateTime<Utc>,
) -> Result<Vec<AttentionEvent>, RepositoryError> {
    let mut updated = Vec::with_capacity(event_ids.len());
    for event_id in event_ids {
        let current = load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?;
        let change = lifecycle::apply(&current, LifecycleOp::MarkRead, now)
            .expect("MarkRead never errors (LifecycleError is Resolve-only)");
        write_lifecycle_change(tx, event_id, &change)?;
        updated.push(load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?);
    }
    Ok(updated)
}

/// Sets `notified_at` if it is still `NULL` (idempotent; the notification
/// service's own bookkeeping, not a domain lifecycle column).
pub fn mark_notified(
    tx: &Transaction<'_>,
    event_id: &str,
    now: DateTime<Utc>,
) -> Result<(), RepositoryError> {
    tx.execute(
        "UPDATE attention_events SET notified_at = COALESCE(notified_at, ?2) WHERE id = ?1",
        params![event_id, rfc3339(now)],
    )?;
    Ok(())
}

/// D2 "Condition detail and subject"/"Evidence is not detail": writes
/// `evidence_json` once. A second call for the same Event is a no-op
/// (`changed: false`, the existing outcome returned) so a later capture
/// attempt can never erase the first. Never touches `detail_json`.
pub fn record_evidence(
    tx: &Transaction<'_>,
    event_id: &str,
    outcome: &EvidenceOutcome,
) -> Result<(AttentionEvent, bool), RepositoryError> {
    let current = load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?;
    if current.evidence.is_some() {
        return Ok((current, false));
    }
    tx.execute(
        "UPDATE attention_events SET evidence_json = ?2, revision = revision + 1 WHERE id = ?1",
        params![event_id, to_json(outcome)],
    )?;
    let updated = load_event(tx, event_id)?.ok_or_else(|| not_found(event_id))?;
    Ok((updated, true))
}

/// Every open (`resolved_at IS NULL`) Event, id order — the pass's read of
/// the durable `FarmView`'s open Events (D2 "The pass").
pub fn open_events(conn: &Connection) -> Result<Vec<AttentionEvent>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ATTENTION_COLUMNS} FROM attention_events WHERE resolved_at IS NULL ORDER BY id"
    ))?;
    let rows = statement
        .query_map([], decode_event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// How many dedup keys one `IN (…)` binds: far under SQLite's
/// bound-parameter limit (999 on the oldest builds).
const KEY_CHUNK: usize = 500;

/// The newest resolved Event id (`resolved_at` descending, then id
/// descending) for each of `keys` that has one — `plan`'s
/// `latest_resolved` map (D2 "Planner rules"). The pass asks only for the
/// keys `plan` can consult, so the read never grows with all of history.
/// No keys, no query.
pub fn latest_resolved_for_keys<'k>(
    conn: &Connection,
    keys: impl IntoIterator<Item = &'k str>,
) -> Result<HashMap<String, String>, StorageError> {
    let keys: Vec<&str> = keys.into_iter().collect();
    let mut latest = HashMap::new();
    for chunk in keys.chunks(KEY_CHUNK) {
        let placeholders = vec!["?"; chunk.len()].join(", ");
        let mut statement = conn.prepare(&format!(
            "SELECT dedup_key, id FROM attention_events
             WHERE dedup_key IN ({placeholders}) AND resolved_at IS NOT NULL
             ORDER BY dedup_key, resolved_at DESC, id DESC"
        ))?;
        let rows = statement.query_map(rusqlite::params_from_iter(chunk.iter()), |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (key, id) = row?;
            // The first row per key is its newest resolution.
            latest.entry(key).or_insert(id);
        }
    }
    Ok(latest)
}

const SEVERITY_ORDER_SQL: &str =
    "CASE severity WHEN 'fatal' THEN 0 WHEN 'warning' THEN 1 WHEN 'info' THEN 2 ELSE 3 END";

/// `list_attention`/`AttentionBackfill.open`: every open Event, by
/// severity (fatal, warning, info), then `first_observed_at` descending,
/// then id.
pub fn list_open(conn: &Connection) -> Result<Vec<AttentionEvent>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ATTENTION_COLUMNS} FROM attention_events WHERE resolved_at IS NULL
         ORDER BY {SEVERITY_ORDER_SQL}, first_observed_at DESC, id"
    ))?;
    let rows = statement
        .query_map([], decode_event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

fn encode_cursor(resolved_at: &str, id: &str) -> AttentionCursor {
    AttentionCursor(format!("{resolved_at}|{id}"))
}

/// Decodes an [`AttentionCursor`] back into `(resolvedAt, id)`. The cursor
/// is opaque to the frontend (round-tripped only); a value this crate
/// didn't itself mint is corrupt input.
fn decode_cursor(cursor: &AttentionCursor) -> Result<(String, String), StorageError> {
    cursor
        .0
        .split_once('|')
        .map(|(resolved_at, id)| (resolved_at.to_string(), id.to_string()))
        .ok_or(StorageError::CorruptData {
            source_name: "attention cursor",
            source_sha256: None,
        })
}

/// Whether `cursor` has the shape [`list_resolved`] mints (`resolvedAt|id`),
/// so a command can reject a hand-made one as `VALIDATION` rather than
/// reading it as corrupt data.
pub fn is_well_formed_cursor(cursor: &AttentionCursor) -> bool {
    cursor
        .0
        .split_once('|')
        .is_some_and(|(resolved_at, id)| !resolved_at.is_empty() && !id.is_empty())
}

/// Every Event linked to `incident_id`, `first_observed_at` then id
/// (`get_incident`'s `events`).
pub fn events_for_incident(
    conn: &Connection,
    incident_id: &str,
) -> Result<Vec<AttentionEvent>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ATTENTION_COLUMNS} FROM attention_events WHERE incident_id = ?1
         ORDER BY first_observed_at, id"
    ))?;
    let rows = statement
        .query_map([incident_id], decode_event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every Event whose `job_id` is `job_id`, `first_observed_at` then id
/// (`get_job_timeline`'s `attention` items).
pub fn events_for_job(
    conn: &Connection,
    job_id: &str,
) -> Result<Vec<AttentionEvent>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ATTENTION_COLUMNS} FROM attention_events WHERE job_id = ?1
         ORDER BY first_observed_at, id"
    ))?;
    let rows = statement
        .query_map([job_id], decode_event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// `list_attention`/`AttentionBackfill.resolved`: up to `limit`,
/// `resolvedAt` descending then id descending, strictly before `before`
/// (exclusive) when given. Returns the page and the cursor for the next
/// one (`None` when this page reached the end).
pub fn list_resolved(
    conn: &Connection,
    before: Option<&AttentionCursor>,
    limit: i64,
) -> Result<(Vec<AttentionEvent>, Option<AttentionCursor>), StorageError> {
    let (resolved_before, id_before) = match before {
        Some(cursor) => {
            let (resolved_at, id) = decode_cursor(cursor)?;
            (Some(resolved_at), Some(id))
        }
        None => (None, None),
    };
    let mut statement = conn.prepare(&format!(
        "SELECT {ATTENTION_COLUMNS} FROM attention_events
         WHERE resolved_at IS NOT NULL
           AND (?1 IS NULL OR resolved_at < ?1 OR (resolved_at = ?1 AND id < ?2))
         ORDER BY resolved_at DESC, id DESC
         LIMIT ?3"
    ))?;
    // Fetch one extra row to tell "exactly `limit` remain" from "more
    // follow", without a second COUNT query.
    let mut rows = statement
        .query_map(
            params![resolved_before, id_before, limit + 1],
            decode_event_row,
        )?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let has_more = rows.len() as i64 > limit;
    if has_more {
        rows.truncate(limit as usize);
    }
    let next_cursor = has_more.then(|| {
        let last = rows.last().expect("has_more implies at least one row");
        encode_cursor(
            last.resolved_at
                .as_deref()
                .expect("resolved_at IS NOT NULL"),
            &last.id,
        )
    });
    Ok((rows, next_cursor))
}

/// `list_attention`'s repository half: the open list and the first/next
/// resolved page. The command layer (Task 6) decides whether to include
/// `open` in the wire result (empty when `resolvedBefore` was given).
pub struct AttentionListPage {
    pub open: Vec<AttentionEvent>,
    pub resolved: Vec<AttentionEvent>,
    pub resolved_cursor: Option<AttentionCursor>,
}

pub fn list_attention(
    conn: &Connection,
    resolved_before: Option<&AttentionCursor>,
    limit: i64,
) -> Result<AttentionListPage, StorageError> {
    let open = list_open(conn)?;
    let (resolved, resolved_cursor) = list_resolved(conn, resolved_before, limit)?;
    Ok(AttentionListPage {
        open,
        resolved,
        resolved_cursor,
    })
}

/// D8: Printer delete (and `replace_all`) resolves its open Events
/// `sourceRemoved`, through `incidents::guards::resolve_printer_events`.
/// Returns the ones that actually changed (always all of them here, since
/// every input row is open by construction).
pub fn resolve_for_printer(
    tx: &Transaction<'_>,
    printer_id: &str,
    now: DateTime<Utc>,
) -> Result<Vec<AttentionEvent>, RepositoryError> {
    let ids: Vec<String> = {
        let mut statement = tx.prepare(
            "SELECT id FROM attention_events WHERE printer_id = ?1 AND resolved_at IS NULL",
        )?;
        let ids = statement
            .query_map([printer_id], |row| row.get(0))?
            .collect::<rusqlite::Result<Vec<String>>>()?;
        ids
    };
    let mut changed_events = Vec::with_capacity(ids.len());
    for id in ids {
        let (event, changed) = resolve(tx, &id, AttentionResolution::SourceRemoved, now)?;
        if changed {
            changed_events.push(event);
        }
    }
    Ok(changed_events)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attention::{AttentionSourceKind, AttentionSubject, ConditionKind};
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

    fn subject() -> AttentionSubject {
        AttentionSubject {
            printer_name: Some("Voron".into()),
            printer_location: Some("Bay A".into()),
            job_label: None,
            spool_number: None,
            spool_label: None,
        }
    }

    fn printer_offline(printer_id: &str) -> Condition {
        Condition {
            kind: ConditionKind::PrinterOffline,
            source_id: printer_id.to_string(),
            printer_id: Some(printer_id.to_string()),
            job_id: None,
            spool_id: None,
            requirement_id: None,
            subject: subject(),
            detail: AttentionDetail::PrinterOffline {
                unreachable_since: NOW_TEXT.to_string(),
            },
            acknowledge: false,
        }
    }

    fn host_failed(printer_id: &str) -> Condition {
        Condition {
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

    fn spool_low(spool_id: &str) -> Condition {
        Condition {
            kind: ConditionKind::SpoolLow,
            source_id: spool_id.to_string(),
            printer_id: None,
            job_id: None,
            spool_id: Some(spool_id.to_string()),
            requirement_id: None,
            subject: AttentionSubject {
                spool_number: Some(12),
                ..subject()
            },
            detail: AttentionDetail::SpoolLow {
                current_mg: 80_000,
                low_threshold_mg: 100_000,
            },
            acknowledge: false,
        }
    }

    #[test]
    fn insert_amend_and_resolve_round_trip_with_revision_bumps() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                let condition = printer_offline("prn-a");
                let inserted = insert(tx, &condition, None, false, AttentionOrigin::Live, now())?;
                assert_eq!(inserted.revision, 1);
                assert_eq!(inserted.observation_count, 1);
                assert_eq!(inserted.first_observed_at, inserted.last_observed_at);
                assert!(inserted.evidence.is_none());
                assert_eq!(inserted.dedup_key, "printer.offline:printer:prn-a");

                // Amend with no change: revision and detail stay put; the
                // observation count still advances.
                let unreachable_since = inserted.first_observed_at.clone();
                let same_detail = AttentionDetail::PrinterOffline {
                    unreachable_since: unreachable_since.clone(),
                };
                let amended = amend(
                    tx,
                    &inserted.id,
                    &same_detail,
                    AttentionSeverity::Warning,
                    &inserted.summary,
                    false,
                    now(),
                )?;
                assert_eq!(amended.revision, 1);
                assert_eq!(amended.observation_count, 2);
                assert!(amended.evidence.is_none());

                // Amend with a change: revision bumps, detail_json changes,
                // evidence_json is never touched.
                let later = now() + chrono::Duration::minutes(5);
                let changed_detail = AttentionDetail::PrinterOffline {
                    unreachable_since: unreachable_since.clone(),
                };
                let amended2 = amend(
                    tx,
                    &inserted.id,
                    &changed_detail,
                    AttentionSeverity::Fatal,
                    "Voron (Bay A) is offline, still.",
                    true,
                    later,
                )?;
                assert_eq!(amended2.revision, 2);
                assert_eq!(amended2.observation_count, 3);
                assert_eq!(amended2.severity, AttentionSeverity::Fatal);
                assert_eq!(amended2.summary, "Voron (Bay A) is offline, still.");
                assert!(amended2.evidence.is_none());

                // Resolve (auto, system reason): revision bumps once more.
                let (resolved, changed) = resolve(
                    tx,
                    &inserted.id,
                    AttentionResolution::ConditionCleared,
                    later,
                )?;
                assert!(changed);
                assert_eq!(resolved.revision, 3);
                assert!(resolved.resolved_at.is_some());
                assert_eq!(
                    resolved.resolution,
                    Some(AttentionResolution::ConditionCleared)
                );
                assert!(resolved.read_at.is_some(), "resolving implies read");

                // A second resolve is a no-op: no revision bump, changed=false.
                let (resolved_again, changed_again) = resolve(
                    tx,
                    &inserted.id,
                    AttentionResolution::ConditionCleared,
                    later,
                )?;
                assert!(!changed_again);
                assert_eq!(resolved_again.revision, 3);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn resolve_non_manual_with_operator_resolved_is_not_manual() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                let condition = printer_offline("prn-a"); // auto resolution mode
                let inserted = insert(tx, &condition, None, false, AttentionOrigin::Live, now())?;
                let error = resolve(
                    tx,
                    &inserted.id,
                    AttentionResolution::OperatorResolved,
                    now(),
                )
                .expect_err("printer.offline is auto, never operator-resolved");
                assert!(matches!(
                    error,
                    RepositoryError::AttentionNotManual {
                        resolution_mode: ResolutionMode::Auto,
                        ..
                    }
                ));
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn amend_never_touches_evidence_json_and_record_evidence_writes_once() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                // job.completed needs a Job; instead exercise the CHECK-free
                // path directly against an inserted printer.offline Event by
                // using record_evidence only on... no: evidence_json's CHECK
                // requires condition = 'job.completed'. Seed a Job row too.
                tx.execute_batch(&format!(
                    "INSERT INTO content_blobs(sha256, size_bytes, created_at)
                       VALUES ('{hash}', 1, '{NOW_TEXT}');
                     INSERT INTO library_models(id, revision, name, format, storage_mode,
                       created_at, updated_at)
                       VALUES ('mdl-a', 1, 'Model', 'gcode', 'managed', '{NOW_TEXT}', '{NOW_TEXT}');
                     INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256,
                       size_bytes, format, origin, source_file_name, source_path, captured_at,
                       inspector_version, inspection_json)
                       VALUES ('msr-a', 'mdl-a', 1, '{hash}', 1, 'gcode', 'import', 'p.gcode',
                               '/p.gcode', '{NOW_TEXT}', 1, '{{}}');
                     INSERT INTO slice_revisions(id, kind, model_id, source_revision_id,
                       gcode_sha256, gcode_size, target_json, facts_json,
                       requires_manual_printer_selection, estimates_json, created_at)
                       VALUES ('slr-a', 'external', 'mdl-a', 'msr-a', '{hash}', 1, '{{}}', '{{}}',
                               1, '{{}}', '{NOW_TEXT}');
                     INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
                       color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
                       created_at, updated_at)
                       VALUES ('spl-a', 1, 1, 'Acme', 'PLA', 'Black', '1.75', 1000000, 1000000,
                               'measured', 'active', '{NOW_TEXT}', '{NOW_TEXT}');
                     INSERT INTO spool_reservations(id, spool_id, holder_kind, holder_id,
                       amount_mg, state, operation_id, created_at)
                       VALUES ('rsv-a', 'spl-a', 'job', 'job-a', 500000, 'active', 'rsv-a-op',
                               '{NOW_TEXT}');
                     INSERT INTO queue_entries(id, revision, slice_revision_id, lineage_id,
                       copy_index, state, position, policy, preference, estimate_mg,
                       estimate_source, created_at, updated_at)
                       VALUES ('qen-a', 1, 'slr-a', 'qln-a', 1, 'queued', 1, 'manual',
                               'loadedFirst', 500000, 'operatorEntered', '{NOW_TEXT}', '{NOW_TEXT}');
                     INSERT INTO jobs(id, revision, queue_entry_id, slice_revision_id, printer_id,
                       printer_snapshot_json, spool_id, reservation_id, estimate_mg, state,
                       settlement, assigned_by, created_at, updated_at)
                       VALUES ('job-a', 1, 'qen-a', 'slr-a', 'prn-a', '{{}}', 'spl-a', 'rsv-a',
                               500000, 'assigned', 'open', 'operator', '{NOW_TEXT}', '{NOW_TEXT}');",
                    hash = "a".repeat(64),
                ))
                .unwrap();

                let condition = Condition {
                    kind: ConditionKind::JobCompleted,
                    source_id: "job-a".to_string(),
                    printer_id: Some("prn-a".to_string()),
                    job_id: Some("job-a".to_string()),
                    spool_id: None,
                    requirement_id: None,
                    subject: subject(),
                    detail: AttentionDetail::JobCompleted {
                        ended_at: NOW_TEXT.to_string(),
                    },
                    acknowledge: false,
                };
                let inserted = insert(tx, &condition, None, false, AttentionOrigin::Live, now())?;

                let outcome = EvidenceOutcome::Captured {
                    snapshot_id: "snp-a".to_string(),
                };
                let (recorded, changed) = record_evidence(tx, &inserted.id, &outcome)?;
                assert!(changed);
                assert_eq!(recorded.revision, 2);
                assert_eq!(recorded.evidence, Some(outcome.clone()));

                // A second record_evidence call is a no-op.
                let other_outcome = EvidenceOutcome::Skipped {
                    reason: crate::cameras::EvidenceSkipReason::CameraError,
                    error_kind: None,
                };
                let (again, changed_again) = record_evidence(tx, &inserted.id, &other_outcome)?;
                assert!(!changed_again);
                assert_eq!(again.revision, 2);
                assert_eq!(again.evidence, Some(outcome));

                // amend never touches evidence_json even when it changes
                // detail/severity.
                let amended = amend(
                    tx,
                    &inserted.id,
                    &AttentionDetail::JobCompleted {
                        ended_at: NOW_TEXT.to_string(),
                    },
                    AttentionSeverity::Info,
                    "Cube finished.",
                    true,
                    now(),
                )?;
                assert_eq!(amended.revision, 3);
                assert_eq!(
                    amended.evidence,
                    Some(EvidenceOutcome::Captured {
                        snapshot_id: "snp-a".to_string()
                    })
                );
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn latest_resolved_for_keys_is_the_newest_per_asked_key() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                seed_printer(tx, "prn-b");
                let condition = printer_offline("prn-a");
                let first = insert(tx, &condition, None, false, AttentionOrigin::Live, now())?;
                resolve(tx, &first.id, AttentionResolution::ConditionCleared, now())?;

                let later = now() + chrono::Duration::minutes(10);
                let second = insert(
                    tx,
                    &condition,
                    Some(&first.id),
                    false,
                    AttentionOrigin::Live,
                    later,
                )?;
                let even_later = later + chrono::Duration::minutes(10);
                resolve(
                    tx,
                    &second.id,
                    AttentionResolution::ConditionCleared,
                    even_later,
                )?;

                // A resolved Event for a key nobody asks about stays out.
                let other = insert(
                    tx,
                    &printer_offline("prn-b"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                resolve(tx, &other.id, AttentionResolution::ConditionCleared, now())?;

                let map = latest_resolved_for_keys(
                    tx,
                    ["printer.offline:printer:prn-a", "spool.low:spool:spl-none"],
                )?;
                assert_eq!(
                    map,
                    HashMap::from([(
                        "printer.offline:printer:prn-a".to_string(),
                        second.id.clone()
                    )])
                );
                assert!(
                    latest_resolved_for_keys(tx, [])?.is_empty(),
                    "no keys, nothing"
                );

                // More keys than one `IN (…)` binds: the chunks still find it.
                let many: Vec<String> = (0..2 * KEY_CHUNK + 7)
                    .map(|i| format!("spool.low:spool:spl-{i}"))
                    .chain(["printer.offline:printer:prn-a".to_string()])
                    .collect();
                let map = latest_resolved_for_keys(tx, many.iter().map(String::as_str))?;
                assert_eq!(map.get("printer.offline:printer:prn-a"), Some(&second.id));
                assert_eq!(map.len(), 1);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn list_attention_orders_open_by_severity_then_recency_and_resolved_by_recency_desc() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                seed_printer(tx, "prn-b");
                let t0 = now();
                let t1 = t0 + chrono::Duration::minutes(1);
                let t2 = t0 + chrono::Duration::minutes(2);

                // Two warnings (offline) at different times, one fatal
                // (hostFailed): fatal must sort first regardless of time.
                let warn_old = insert(
                    tx,
                    &printer_offline("prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    t0,
                )?;
                let warn_new = insert(
                    tx,
                    &printer_offline("prn-b"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    t1,
                )?;
                let fatal = insert(
                    tx,
                    &host_failed("prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    t0,
                )?;

                let page = list_attention(tx, None, 200)?;
                let open_ids: Vec<_> = page.open.iter().map(|e| e.id.clone()).collect();
                assert_eq!(
                    open_ids,
                    [fatal.id.clone(), warn_new.id.clone(), warn_old.id.clone()]
                );

                // Resolve both warnings at different times; resolved order is
                // resolvedAt desc, then id desc.
                resolve(tx, &warn_old.id, AttentionResolution::ConditionCleared, t1)?;
                resolve(tx, &warn_new.id, AttentionResolution::ConditionCleared, t2)?;
                let page2 = list_attention(tx, None, 200)?;
                let resolved_ids: Vec<_> = page2.resolved.iter().map(|e| e.id.clone()).collect();
                assert_eq!(resolved_ids, [warn_new.id.clone(), warn_old.id.clone()]);
                assert!(page2.resolved_cursor.is_none());
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn list_resolved_paginates_with_an_attention_cursor() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                let mut ids = Vec::new();
                for i in 0..5 {
                    let printer_id = format!("prn-{i}");
                    seed_printer(tx, &printer_id);
                    let t = now() + chrono::Duration::minutes(i);
                    let inserted = insert(
                        tx,
                        &printer_offline(&printer_id),
                        None,
                        false,
                        AttentionOrigin::Live,
                        t,
                    )?;
                    resolve(tx, &inserted.id, AttentionResolution::ConditionCleared, t)?;
                    ids.push(inserted.id);
                }
                // Newest resolved first: ids[4], ids[3], ids[2], ids[1], ids[0].
                let (page1, cursor1) = list_resolved(tx, None, 2)?;
                assert_eq!(
                    page1.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    [ids[4].clone(), ids[3].clone()]
                );
                let cursor1 = cursor1.expect("more pages remain");

                let (page2, cursor2) = list_resolved(tx, Some(&cursor1), 2)?;
                assert_eq!(
                    page2.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    [ids[2].clone(), ids[1].clone()]
                );
                let cursor2 = cursor2.expect("one more remains");

                let (page3, cursor3) = list_resolved(tx, Some(&cursor2), 2)?;
                assert_eq!(
                    page3.iter().map(|e| e.id.clone()).collect::<Vec<_>>(),
                    [ids[0].clone()]
                );
                assert!(cursor3.is_none(), "the last page has no next cursor");
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn spool_low_never_touches_a_printer_id() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                tx.execute_batch(&format!(
                    "INSERT INTO spools(id, revision, spool_number, manufacturer, material_family,
                       color_name, diameter, nominal_mg, current_mg, confidence, lifecycle,
                       created_at, updated_at)
                     VALUES ('spl-a', 1, 12, 'Acme', 'PLA', 'Black', '1.75', 1000000, 80000,
                             'measured', 'active', '{NOW_TEXT}', '{NOW_TEXT}');"
                ))
                .unwrap();
                let inserted = insert(
                    tx,
                    &spool_low("spl-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                assert!(inserted.printer_id.is_none());
                assert_eq!(inserted.spool_id.as_deref(), Some("spl-a"));
                assert_eq!(inserted.source.kind, AttentionSourceKind::Spool);
                Ok(())
            })
            .unwrap();
    }

    #[test]
    fn resolve_for_printer_resolves_every_open_event_of_that_printer_source_removed() {
        let (_temp, storage) = storage();
        storage
            .write_repo(|tx| -> Result<(), RepositoryError> {
                seed_printer(tx, "prn-a");
                seed_printer(tx, "prn-b");
                let a1 = insert(
                    tx,
                    &printer_offline("prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let a2 = insert(
                    tx,
                    &host_failed("prn-a"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;
                let b1 = insert(
                    tx,
                    &printer_offline("prn-b"),
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )?;

                let changed = resolve_for_printer(tx, "prn-a", now())?;
                let changed_ids: std::collections::BTreeSet<_> =
                    changed.iter().map(|e| e.id.clone()).collect();
                assert_eq!(
                    changed_ids,
                    std::collections::BTreeSet::from([a1.id.clone(), a2.id.clone()])
                );
                for event in &changed {
                    assert_eq!(event.resolution, Some(AttentionResolution::SourceRemoved));
                }

                let b1_after = load_event(tx, &b1.id)?.unwrap();
                assert!(
                    b1_after.resolved_at.is_none(),
                    "another Printer's Event is untouched"
                );
                Ok(())
            })
            .unwrap();
    }
}
