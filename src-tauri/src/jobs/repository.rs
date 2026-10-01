//! P7 D3/D4: the SQL for `jobs`, the `job_events` timeline, and
//! `reconciliation_requirements`. Every function takes the caller's
//! `&Transaction`/`&Connection` (P3+'s pattern; see `host_ops::repository`'s
//! module doc), so a later task's command composes several of these into
//! one atomic commit through `Storage::write_repo`.
//!
//! [`insert_job`] writes the assign transaction's Job row and its
//! `assigned` insert event (not a transition — D3). [`transition`] runs an
//! event through the pure `jobs::state::transition` before any SQL, then
//! writes the Job's row and exactly one `job_events` row with the next
//! sequence, in one call; it also clears `host_unreachable_since` on every
//! move out of `printing`/`paused` (ruling R7), whatever [`JobChange`]
//! says. [`open_requirement`] and [`set_requirement_status`] are
//! Reconciliation Requirements' two writes. The rest are reads.
//!
//! `Job.allowedActions` is computed here, at decode, from D3's table
//! ([`super::state::allowed_actions`]) and a successor-entry column in
//! [`JOB_COLUMNS`]. `Job.startBlockers` needs the live Printer status, so
//! it decodes empty and `jobs::dispatch::present_jobs` fills it before a
//! Job leaves the backend (it also recomputes `declareOutcome` with the
//! runtime's injected timings and clock). [`update_columns`] is the one
//! write that moves no state: it bumps `revision` without an event row
//! (D7's outcome bookkeeping, the tracker's progress and reachability, and
//! driver refusals). [`tracking`] reads the backend-only columns the
//! tracker needs. `JobHistory.hostOperations` lists every Host Operation
//! whose `job_id` is the Job's.

use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::library;
use crate::persistence::{RepositoryError, StorageError};
use crate::queue::repository as queue_repository;
use crate::queue::QueueEntry;
use crate::spools::reservations::{Reservation, ReservationHolder, ReservationState};
use crate::spools::{decode_enum, encode_enum};

use super::state;
use super::{
    estimated_use_mg, AssignedBy, CancelReason, Job, JobEvent, JobEventKind, JobFailure,
    JobHistory, JobState, PrinterSnapshot, ReconciliationRequirement, RequirementKind,
    RequirementResolution, RequirementStatus, Settlement, SettlementMethod, SettlementPreview,
    StartConfirmation,
};

const JOB_ID_PREFIX: &str = "job";
const JOB_EVENT_ID_PREFIX: &str = "jev";
const REQUIREMENT_ID_PREFIX: &str = "rrq";

/// A new `job-<uuid v4>` id. `pub` so the assign transaction can name the
/// Job before it inserts it: the Spool reservation's holder is
/// `("job", jobId)`, and the Job row references that reservation.
pub fn new_job_id() -> String {
    library::new_id(JOB_ID_PREFIX)
}

fn new_event_id() -> String {
    library::new_id(JOB_EVENT_ID_PREFIX)
}

fn new_requirement_id() -> String {
    library::new_id(REQUIREMENT_ID_PREFIX)
}

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
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
    serde_json::to_string(value).expect("jobs wire types always serialize")
}

fn from_json<T: serde::de::DeserializeOwned>(index: usize, text: &str) -> rusqlite::Result<T> {
    serde_json::from_str(text).map_err(|error| {
        rusqlite::Error::FromSqlConversionFailure(index, Type::Text, Box::new(error))
    })
}

// --- jobs ------------------------------------------------------------------

// The last column is computed: whether the Job's Queue Entry already has a
// successor (a retry or a release replacement), which D3's `retry` action
// needs. Every query selects these `FROM jobs` unaliased, so the correlated
// subquery can name `jobs.queue_entry_id`.
const JOB_COLUMNS: &str = "id, revision, queue_entry_id, slice_revision_id, printer_id, \
     printer_snapshot_json, spool_id, reservation_id, estimate_mg, state, cancel_reason, \
     settlement, settlement_method, assigned_by, start_confirmation, upload_host_operation_id, \
     active_host_operation_id, host_path, max_progress_pct, host_unreachable_since, \
     last_failure_json, correction_event_id, created_at, updated_at, started_at, ended_at, \
     EXISTS(SELECT 1 FROM queue_entries successor \
            WHERE successor.origin_entry_id = jobs.queue_entry_id)";

fn decode_job_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Job> {
    let printer_snapshot_json: String = row.get(5)?;
    let estimate_mg: i64 = row.get(8)?;
    let state_text: String = row.get(9)?;
    let cancel_reason: Option<String> = row.get(10)?;
    let settlement_text: String = row.get(11)?;
    let settlement_method: Option<String> = row.get(12)?;
    let assigned_by_text: String = row.get(13)?;
    let start_confirmation: Option<String> = row.get(14)?;
    let max_progress_pct: i64 = row.get(18)?;
    let last_failure_json: Option<String> = row.get(20)?;
    let correction_event_id: Option<String> = row.get(21)?;

    let settlement: Settlement = decode_text_enum(11, &settlement_text)?;
    let settlement_preview =
        matches!(settlement, Settlement::Pending | Settlement::Deferred).then(|| {
            SettlementPreview {
                estimated_use_mg: estimated_use_mg(estimate_mg, max_progress_pct),
            }
        });

    let has_successor: bool = row.get(26)?;

    let mut job = Job {
        id: row.get(0)?,
        revision: row.get(1)?,
        queue_entry_id: row.get(2)?,
        slice_revision_id: row.get(3)?,
        printer_id: row.get(4)?,
        printer_snapshot: from_json(5, &printer_snapshot_json)?,
        spool_id: row.get(6)?,
        reservation_id: row.get(7)?,
        estimate_mg,
        state: decode_text_enum(9, &state_text)?,
        cancel_reason: cancel_reason
            .map(|text| decode_text_enum::<CancelReason>(10, &text))
            .transpose()?,
        settlement,
        settlement_method: settlement_method
            .map(|text| decode_text_enum::<SettlementMethod>(12, &text))
            .transpose()?,
        settlement_preview,
        corrected: correction_event_id.is_some(),
        assigned_by: decode_text_enum(13, &assigned_by_text)?,
        start_confirmation: start_confirmation
            .map(|text| decode_text_enum::<StartConfirmation>(14, &text))
            .transpose()?,
        upload_host_operation_id: row.get(15)?,
        active_host_operation_id: row.get(16)?,
        host_path: row.get(17)?,
        max_progress_pct,
        host_unreachable_since: row.get(19)?,
        last_failure: last_failure_json
            .map(|text| from_json::<JobFailure>(20, &text))
            .transpose()?,
        created_at: row.get(22)?,
        updated_at: row.get(23)?,
        started_at: row.get(24)?,
        ended_at: row.get(25)?,
        // Live-status dependent: `jobs::dispatch::present_jobs` fills it
        // for an `awaitingStart` Job before the Job leaves Rust.
        start_blockers: Vec::new(),
        allowed_actions: Vec::new(),
    };
    // The production timing and clock. A `printing`/`paused` Job's
    // `declareOutcome` depends on them, so `dispatch::present_jobs`
    // recomputes it with the runtime's injected ones before it leaves Rust.
    job.allowed_actions = state::allowed_actions(
        &job,
        has_successor,
        chrono::Utc::now(),
        super::services::JobTimings::default().unreachable_declare_after,
    );
    Ok(job)
}

pub fn load_job(conn: &Connection, id: &str) -> Result<Option<Job>, StorageError> {
    Ok(conn
        .query_row(
            &format!("SELECT {JOB_COLUMNS} FROM jobs WHERE id = ?1"),
            [id],
            decode_job_row,
        )
        .optional()?)
}

/// The Printer's one active (non-terminal) Job, if any — the partial
/// unique index's own invariant, read back.
pub fn active_job_for_printer(
    conn: &Connection,
    printer_id: &str,
) -> Result<Option<Job>, StorageError> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {JOB_COLUMNS} FROM jobs
                 WHERE printer_id = ?1 AND state NOT IN ('completed','failed','cancelled')"
            ),
            [printer_id],
            decode_job_row,
        )
        .optional()?)
}

/// The Job whose upload, active, or start Host Operation is
/// `host_operation_id` — `apply_host_outcome`'s lookup (a later task).
pub fn job_for_host_operation(
    conn: &Connection,
    host_operation_id: &str,
) -> Result<Option<Job>, StorageError> {
    Ok(conn
        .query_row(
            &format!(
                "SELECT {JOB_COLUMNS} FROM jobs
                 WHERE upload_host_operation_id = ?1
                    OR active_host_operation_id = ?1
                    OR start_host_operation_id = ?1"
            ),
            [host_operation_id],
            decode_job_row,
        )
        .optional()?)
}

/// Every active (non-terminal) Job, oldest first.
pub fn list_active(conn: &Connection) -> Result<Vec<Job>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {JOB_COLUMNS} FROM jobs WHERE state NOT IN ('completed','failed','cancelled')
         ORDER BY created_at, id"
    ))?;
    let rows = statement
        .query_map([], decode_job_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// The assign transaction's Job insert (spec D3/D4): `assigned`,
/// settlement `open`, and its `assigned` event (sequence 1, no
/// `from_state` — the insert event is not a transition).
pub struct NewJob {
    pub id: String,
    pub queue_entry_id: String,
    pub slice_revision_id: String,
    pub printer_id: String,
    pub printer_snapshot: PrinterSnapshot,
    pub spool_id: String,
    pub reservation_id: String,
    pub estimate_mg: i64,
    pub assigned_by: AssignedBy,
    /// D5: the operator acknowledged the Slice's unconfirmed facts
    /// (`acknowledgeManualFacts`) when assigning.
    pub manual_facts_acknowledged: bool,
}

pub fn insert_job(tx: &Transaction<'_>, new: &NewJob, now: &str) -> Result<Job, RepositoryError> {
    let id = new.id.clone();
    tx.execute(
        "INSERT INTO jobs(
             id, revision, queue_entry_id, slice_revision_id, printer_id, printer_snapshot_json,
             spool_id, reservation_id, estimate_mg, state, settlement, assigned_by,
             manual_facts_acknowledged, created_at, updated_at
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, 'assigned', 'open', ?9, ?11, ?10, ?10)",
        params![
            id,
            new.queue_entry_id,
            new.slice_revision_id,
            new.printer_id,
            to_json(&new.printer_snapshot),
            new.spool_id,
            new.reservation_id,
            new.estimate_mg,
            encode_enum(new.assigned_by),
            now,
            new.manual_facts_acknowledged,
        ],
    )?;
    write_event(
        tx,
        &id,
        1,
        JobEventKind::Assigned,
        None,
        JobState::Assigned,
        None,
        None,
        None,
        now,
    )?;
    load_job(tx, &id)?.ok_or_else(|| not_found(&id))
}

/// The optional column overrides a [`transition`] call also writes, on
/// top of what D3 alone decides (the new `state`, the settlement D3's
/// `settlement_after` table gives, and `host_unreachable_since` cleared
/// per ruling R7). Every field left `None` leaves that column unchanged.
/// `operation_id`/`host_operation_id`/`detail` land on the `job_events`
/// row, not the `jobs` row.
#[derive(Default, Debug, Clone)]
pub struct JobChange {
    pub cancel_reason: Option<CancelReason>,
    pub settlement_method: Option<SettlementMethod>,
    pub start_confirmation: Option<StartConfirmation>,
    pub upload_host_operation_id: Option<String>,
    pub active_host_operation_id: Option<String>,
    pub start_host_operation_id: Option<String>,
    pub host_path: Option<String>,
    pub history_mark: Option<i64>,
    pub host_job_id: Option<i64>,
    pub max_progress_pct: Option<i64>,
    /// D7: the tracker's count of inconclusive polls in a row.
    pub inconclusive_checks: Option<i64>,
    /// D7/R5: set by a history poll that could not run.
    pub host_unreachable_since: Option<String>,
    pub last_failure: Option<JobFailure>,
    pub correction_event_id: Option<String>,
    pub started_at: Option<String>,
    pub operation_id: Option<String>,
    pub host_operation_id: Option<String>,
    pub detail: Option<serde_json::Value>,
    /// Sets `last_failure_json` to NULL (wins over `last_failure`).
    pub clear_last_failure: bool,
    /// Sets `upload_host_operation_id` to NULL (D7: a failed re-stage no
    /// longer trusts the old staged file).
    pub clear_upload_host_operation_id: bool,
    /// Sets `active_host_operation_id` to NULL (D7: its op is terminal).
    pub clear_active_host_operation_id: bool,
    /// Sets `host_unreachable_since` to NULL (D7: a successful history
    /// poll). Wins over `host_unreachable_since`.
    pub clear_host_unreachable_since: bool,
}

fn next_sequence(tx: &Transaction<'_>, job_id: &str) -> Result<i64, RepositoryError> {
    let max: Option<i64> = tx.query_row(
        "SELECT MAX(sequence) FROM job_events WHERE job_id = ?1",
        [job_id],
        |r| r.get(0),
    )?;
    Ok(max.unwrap_or(0) + 1)
}

#[allow(clippy::too_many_arguments)]
fn write_event(
    tx: &Transaction<'_>,
    job_id: &str,
    sequence: i64,
    kind: JobEventKind,
    from_state: Option<JobState>,
    to_state: JobState,
    operation_id: Option<&str>,
    host_operation_id: Option<&str>,
    detail: Option<&serde_json::Value>,
    now: &str,
) -> Result<(), RepositoryError> {
    let id = new_event_id();
    tx.execute(
        "INSERT INTO job_events(
             id, job_id, sequence, kind, from_state, to_state, operation_id, host_operation_id,
             detail_json, at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
        params![
            id,
            job_id,
            sequence,
            encode_enum(kind),
            from_state.map(encode_enum),
            encode_enum(to_state),
            operation_id,
            host_operation_id,
            detail.map(to_json),
            now,
        ],
    )?;
    Ok(())
}

/// Runs `event` through the pure `jobs::state::transition` before any SQL
/// ([`RepositoryError::IllegalJobTransition`] on a rejection, nothing
/// written), then writes the Job's row and exactly one `job_events` row
/// with the next sequence. `change`'s fields overlay the row (`None`
/// leaves a column unchanged); the new `state` and (via
/// `state::settlement_after`) `settlement` always come from `event`
/// itself, never from `change`. `host_unreachable_since` is force-cleared
/// on every move out of `printing`/`paused` (ruling R7), whatever
/// `change` says — the CHECK `host_unreachable_since IS NULL OR state IN
/// ('printing','paused')` always holds as a result.
pub fn transition(
    tx: &Transaction<'_>,
    job_id: &str,
    event: JobEventKind,
    change: JobChange,
    now: &str,
) -> Result<Job, RepositoryError> {
    let current = load_job(tx, job_id)?.ok_or_else(|| not_found(job_id))?;
    let to = state::transition(current.state, event).map_err(|error| {
        RepositoryError::IllegalJobTransition {
            job_id: job_id.to_string(),
            from: error.from,
            event,
        }
    })?;

    let settlement = state::settlement_after(event);
    let clears_unreachable = matches!(current.state, JobState::Printing | JobState::Paused)
        && !matches!(to, JobState::Printing | JobState::Paused);
    // Only the *first* move into a terminal state sets `ended_at`.
    // `MaterialSettled`/`MaterialDeferred` (`failed`/`cancelled` ->
    // unchanged) and `MaterialCorrected` (`completed` -> unchanged) all
    // keep an already-terminal state, so `to.is_terminal()` alone would
    // be true again on every one of those calls and overwrite the
    // original print-end time with the settlement or correction time.
    let ended_at = (!current.state.is_terminal() && to.is_terminal()).then(|| now.to_string());

    write_row(
        tx,
        job_id,
        to,
        settlement,
        &change,
        ended_at,
        clears_unreachable,
        now,
    )?;

    let sequence = next_sequence(tx, job_id)?;
    write_event(
        tx,
        job_id,
        sequence,
        event,
        Some(current.state),
        to,
        change.operation_id.as_deref(),
        change.host_operation_id.as_deref(),
        change.detail.as_ref(),
        now,
    )?;

    load_job(tx, job_id)?.ok_or_else(|| not_found(job_id))
}

/// The one UPDATE both [`transition`] and [`update_columns`] run: the new
/// `state`, `revision + 1`, and `change`'s overlay (`None` leaves a column
/// unchanged; a `clear_*` flag sets its column to NULL).
#[allow(clippy::too_many_arguments)]
fn write_row(
    tx: &Transaction<'_>,
    job_id: &str,
    to: JobState,
    settlement: Option<Settlement>,
    change: &JobChange,
    ended_at: Option<String>,
    clears_unreachable: bool,
    now: &str,
) -> Result<(), RepositoryError> {
    tx.execute(
        "UPDATE jobs SET
             state = ?2,
             revision = revision + 1,
             updated_at = ?3,
             cancel_reason = COALESCE(?4, cancel_reason),
             settlement = COALESCE(?5, settlement),
             settlement_method = COALESCE(?6, settlement_method),
             start_confirmation = COALESCE(?7, start_confirmation),
             upload_host_operation_id =
                 CASE WHEN ?20 THEN NULL ELSE COALESCE(?8, upload_host_operation_id) END,
             active_host_operation_id =
                 CASE WHEN ?21 THEN NULL ELSE COALESCE(?9, active_host_operation_id) END,
             start_host_operation_id = COALESCE(?10, start_host_operation_id),
             host_path = COALESCE(?11, host_path),
             history_mark = COALESCE(?12, history_mark),
             host_job_id = COALESCE(?13, host_job_id),
             max_progress_pct = COALESCE(?14, max_progress_pct),
             last_failure_json =
                 CASE WHEN ?22 THEN NULL ELSE COALESCE(?15, last_failure_json) END,
             correction_event_id = COALESCE(?16, correction_event_id),
             started_at = COALESCE(?17, started_at),
             ended_at = COALESCE(?18, ended_at),
             host_unreachable_since = CASE WHEN ?19 OR ?23 THEN NULL
                                           ELSE COALESCE(?24, host_unreachable_since) END,
             inconclusive_checks = COALESCE(?25, inconclusive_checks)
         WHERE id = ?1",
        params![
            job_id,
            encode_enum(to),
            now,
            change.cancel_reason.map(encode_enum),
            settlement.map(encode_enum),
            change.settlement_method.map(encode_enum),
            change.start_confirmation.map(encode_enum),
            change.upload_host_operation_id,
            change.active_host_operation_id,
            change.start_host_operation_id,
            change.host_path,
            change.history_mark,
            change.host_job_id,
            change.max_progress_pct,
            change.last_failure.as_ref().map(to_json),
            change.correction_event_id,
            change.started_at,
            ended_at,
            clears_unreachable,
            change.clear_upload_host_operation_id,
            change.clear_active_host_operation_id,
            change.clear_last_failure,
            change.clear_host_unreachable_since,
            change.host_unreachable_since,
            change.inconclusive_checks,
        ],
    )?;
    Ok(())
}

/// A column update with no state change and no `job_events` row (D7: the
/// driver's `lastFailure = refused`, and clearing a terminal op's
/// `active_host_operation_id`). `revision` still goes up, so the Job is
/// republished. `change`'s event fields (`operation_id`,
/// `host_operation_id`, `detail`) are ignored: there is no event.
pub fn update_columns(
    tx: &Transaction<'_>,
    job_id: &str,
    change: JobChange,
    now: &str,
) -> Result<Job, RepositoryError> {
    let current = load_job(tx, job_id)?.ok_or_else(|| not_found(job_id))?;
    write_row(tx, job_id, current.state, None, &change, None, false, now)?;
    load_job(tx, job_id)?.ok_or_else(|| not_found(job_id))
}

/// The backend-only `jobs` columns the tracker reads (spec "Wire types":
/// never on the wire).
#[derive(Clone, Debug, PartialEq)]
pub struct Tracking {
    pub history_mark: Option<i64>,
    pub host_job_id: Option<i64>,
    pub inconclusive_checks: i64,
    pub start_host_operation_id: Option<String>,
}

pub fn tracking(conn: &Connection, job_id: &str) -> Result<Option<Tracking>, StorageError> {
    Ok(conn
        .query_row(
            "SELECT history_mark, host_job_id, inconclusive_checks, start_host_operation_id
             FROM jobs WHERE id = ?1",
            [job_id],
            |row| {
                Ok(Tracking {
                    history_mark: row.get(0)?,
                    host_job_id: row.get(1)?,
                    inconclusive_checks: row.get(2)?,
                    start_host_operation_id: row.get(3)?,
                })
            },
        )
        .optional()?)
}

// --- job_events --------------------------------------------------------------

const JOB_EVENT_COLUMNS: &str =
    "id, job_id, sequence, kind, from_state, to_state, operation_id, host_operation_id, detail_json, at";

fn decode_event_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<JobEvent> {
    let kind_text: String = row.get(3)?;
    let from_state: Option<String> = row.get(4)?;
    let to_state_text: String = row.get(5)?;
    let detail_json: Option<String> = row.get(8)?;
    Ok(JobEvent {
        id: row.get(0)?,
        job_id: row.get(1)?,
        sequence: row.get(2)?,
        kind: decode_text_enum(3, &kind_text)?,
        from_state: from_state
            .map(|text| decode_text_enum::<JobState>(4, &text))
            .transpose()?,
        to_state: decode_text_enum(5, &to_state_text)?,
        operation_id: row.get(6)?,
        host_operation_id: row.get(7)?,
        detail: detail_json
            .map(|text| from_json::<serde_json::Value>(8, &text))
            .transpose()?,
        at: row.get(9)?,
    })
}

/// The Job's own timeline, in sequence order (P8: `get_incident` merges it
/// into an Incident's timeline at read time, never copying it).
pub fn list_events(conn: &Connection, job_id: &str) -> Result<Vec<JobEvent>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {JOB_EVENT_COLUMNS} FROM job_events WHERE job_id = ?1 ORDER BY sequence"
    ))?;
    let rows = statement
        .query_map([job_id], decode_event_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

// --- reconciliation_requirements ---------------------------------------------

const REQUIREMENT_COLUMNS: &str =
    "id, job_id, kind, status, spool_id, reservation_id, opened_at, deferred_at, resolved_at, resolution_json";

fn decode_requirement_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<ReconciliationRequirement> {
    let kind_text: String = row.get(2)?;
    let status_text: String = row.get(3)?;
    let resolution_json: Option<String> = row.get(9)?;
    Ok(ReconciliationRequirement {
        id: row.get(0)?,
        job_id: row.get(1)?,
        kind: decode_text_enum(2, &kind_text)?,
        status: decode_text_enum(3, &status_text)?,
        spool_id: row.get(4)?,
        reservation_id: row.get(5)?,
        opened_at: row.get(6)?,
        deferred_at: row.get(7)?,
        resolved_at: row.get(8)?,
        resolution: resolution_json
            .map(|text| from_json::<RequirementResolution>(9, &text))
            .transpose()?,
    })
}

fn load_requirement(
    conn: &Connection,
    id: &str,
) -> Result<Option<ReconciliationRequirement>, StorageError> {
    Ok(conn
        .query_row(
            &format!("SELECT {REQUIREMENT_COLUMNS} FROM reconciliation_requirements WHERE id = ?1"),
            [id],
            decode_requirement_row,
        )
        .optional()?)
}

pub fn requirements_for_job(
    conn: &Connection,
    job_id: &str,
) -> Result<Vec<ReconciliationRequirement>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {REQUIREMENT_COLUMNS} FROM reconciliation_requirements WHERE job_id = ?1
         ORDER BY opened_at, id"
    ))?;
    let rows = statement
        .query_map([job_id], decode_requirement_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every `pending`/`deferred` Reconciliation Requirement (P8 will project
/// these into Attention; P7 keeps them durable and unresolved).
pub fn open_requirements(
    conn: &Connection,
) -> Result<Vec<ReconciliationRequirement>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {REQUIREMENT_COLUMNS} FROM reconciliation_requirements WHERE status <> 'resolved'
         ORDER BY opened_at, id"
    ))?;
    let rows = statement
        .query_map([], decode_requirement_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Opens a new Reconciliation Requirement, `pending`. The UNIQUE index on
/// `(job_id, kind)` rejects a second one for the same Job and kind (a raw
/// SQLite constraint violation — `RepositoryError::Storage`; a later
/// task's caller is expected to check first, the way `insert_dispatching`'s
/// sibling test documents for the partial unique index it shares this
/// pattern with).
pub fn open_requirement(
    tx: &Transaction<'_>,
    job_id: &str,
    kind: RequirementKind,
    spool_id: Option<&str>,
    reservation_id: Option<&str>,
    now: &str,
) -> Result<ReconciliationRequirement, RepositoryError> {
    let id = new_requirement_id();
    tx.execute(
        "INSERT INTO reconciliation_requirements(id, job_id, kind, status, spool_id, reservation_id, opened_at)
         VALUES (?1, ?2, ?3, 'pending', ?4, ?5, ?6)",
        params![id, job_id, encode_enum(kind), spool_id, reservation_id, now],
    )?;
    load_requirement(tx, &id)?.ok_or_else(|| not_found(&id))
}

/// Moves a Reconciliation Requirement to `status`: `resolved` sets
/// `resolved_at`/`resolution_json` (the CHECK requires both);
/// `deferred` sets `deferred_at`; `pending` touches neither timestamp.
pub fn set_requirement_status(
    tx: &Transaction<'_>,
    id: &str,
    status: RequirementStatus,
    resolution: Option<&serde_json::Value>,
    now: &str,
) -> Result<ReconciliationRequirement, RepositoryError> {
    let resolution_json = resolution.map(to_json);
    match status {
        RequirementStatus::Resolved => {
            tx.execute(
                "UPDATE reconciliation_requirements
                 SET status = 'resolved', resolved_at = ?2, resolution_json = ?3
                 WHERE id = ?1",
                params![id, now, resolution_json],
            )?;
        }
        RequirementStatus::Deferred => {
            tx.execute(
                "UPDATE reconciliation_requirements SET status = 'deferred', deferred_at = ?2
                 WHERE id = ?1",
                params![id, now],
            )?;
        }
        RequirementStatus::Pending => {
            tx.execute(
                "UPDATE reconciliation_requirements SET status = 'pending' WHERE id = ?1",
                params![id],
            )?;
        }
    }
    load_requirement(tx, id)?.ok_or_else(|| not_found(id))
}

// --- history -------------------------------------------------------------

fn decode_reservation_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<Reservation> {
    let state_text: String = row.get(5)?;
    Ok(Reservation {
        id: row.get(0)?,
        spool_id: row.get(1)?,
        holder: ReservationHolder {
            kind: row.get(2)?,
            id: row.get(3)?,
        },
        amount_mg: row.get(4)?,
        state: decode_text_enum::<ReservationState>(5, &state_text)?,
        operation_id: row.get(6)?,
        created_at: row.get(7)?,
        settled_at: row.get(8)?,
    })
}

/// A Job's own reservation, read directly (not through
/// `spools::reservations`, which takes `&Transaction` — `history` takes
/// `&Connection`, spec's own signature).
fn load_reservation(
    conn: &Connection,
    reservation_id: &str,
) -> Result<Option<Reservation>, StorageError> {
    Ok(conn
        .query_row(
            "SELECT id, spool_id, holder_kind, holder_id, amount_mg, state, operation_id, created_at, settled_at
             FROM spool_reservations WHERE id = ?1",
            [reservation_id],
            decode_reservation_row,
        )
        .optional()?)
}

fn lineage_entries(conn: &Connection, lineage_id: &str) -> Result<Vec<QueueEntry>, StorageError> {
    let mut statement = conn.prepare(
        "SELECT id FROM queue_entries WHERE lineage_id = ?1 ORDER BY copy_index, created_at, id",
    )?;
    let ids = statement
        .query_map([lineage_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.into_iter()
        .map(|id| queue_repository::load(conn, &id)?.ok_or(StorageError::OperationFailed))
        .collect()
}

/// Every Host Operation handed off for the Job (`host_operations.job_id`),
/// oldest first.
fn host_operations_of(
    conn: &Connection,
    job_id: &str,
) -> Result<Vec<crate::host_ops::HostOperation>, StorageError> {
    let mut statement = conn
        .prepare("SELECT id FROM host_operations WHERE job_id = ?1 ORDER BY created_at, rowid")?;
    let ids = statement
        .query_map([job_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.iter()
        .map(|id| match crate::host_ops::repository::load(conn, id) {
            Ok(Some(op)) => Ok(op),
            Ok(None) => Err(StorageError::OperationFailed),
            Err(RepositoryError::Storage(error)) => Err(error),
            Err(_) => Err(StorageError::OperationFailed),
        })
        .collect()
}

/// `get_job_history`'s assembly (spec "Backend model"): the Job, its
/// Queue Entry, that entry's whole lineage, the Job's timeline in
/// sequence order, its reservation, its Host Operations, and its
/// Reconciliation Requirements.
/// Assumes `job_id` names an existing Job (a later task's command checks
/// that first, the way every other `NOT_FOUND` read does); a missing one
/// here surfaces as `StorageError::OperationFailed`, since this function's
/// signature carries no `NotFound` variant of its own.
pub fn history(conn: &Connection, job_id: &str) -> Result<JobHistory, StorageError> {
    let job = load_job(conn, job_id)?.ok_or(StorageError::OperationFailed)?;
    let entry =
        queue_repository::load(conn, &job.queue_entry_id)?.ok_or(StorageError::OperationFailed)?;
    let lineage = lineage_entries(conn, &entry.lineage_id)?;
    let events = list_events(conn, job_id)?;
    let reservations = load_reservation(conn, &job.reservation_id)?
        .into_iter()
        .collect();
    let requirements = requirements_for_job(conn, job_id)?;
    Ok(JobHistory {
        job,
        entry,
        lineage,
        events,
        reservations,
        host_operations: host_operations_of(conn, job_id)?,
        requirements,
    })
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::catalog::{BedShape, PrinterProfile};
    use crate::persistence::{MetadataRootLease, Storage};
    use crate::printers::repository::PrinterRepository;
    use crate::printers::StoredPrinter;
    use crate::queue::repository::{create_entries, NewEntries};
    use crate::queue::state::EntryEvent;
    use crate::queue::{DispatchPolicy, DispatchPreference, EstimateSource, MaterialEstimate};
    use crate::slicing::repository::fixtures::{a_farm3d_revision, seed};
    use crate::slicing::repository::insert_farm3d_revision;
    use crate::spools::reservations::{self, ReservationHolder};
    use crate::spools::{
        repository as spools_repository, AmountConfidence, FilamentDiameter, MaterialFamily,
        SpoolFields,
    };

    const NOW: &str = "2026-02-01T00:00:00.000Z";
    const SLR: &str = "slr-a";
    const PRINTER: &str = "prn-a";

    fn spool_fields() -> SpoolFields {
        SpoolFields {
            manufacturer: "Polymaker".to_string(),
            product: None,
            material_family: MaterialFamily::Pla,
            material_other: None,
            color_name: "Black".to_string(),
            color_hex: None,
            diameter: FilamentDiameter::D175,
            nominal_mg: 1_000_000,
            low_threshold_mg: 100_000,
            tare_id: None,
            notes: None,
        }
    }

    fn a_printer_profile() -> PrinterProfile {
        PrinterProfile {
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            default_bed_type: "PEI".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: None,
            suggested_port: None,
        }
    }

    /// The environment a Job needs: a seeded Slice Revision, a Printer, a
    /// Spool, and one `queued` Queue Entry.
    struct Rig {
        _temp: tempfile::TempDir,
        _lease: MetadataRootLease,
        storage: Arc<Storage>,
        entry: QueueEntry,
        spool_id: String,
    }

    fn rig() -> Rig {
        let (temp, lease, storage) = crate::test_storage();
        storage
            .write(|tx| {
                seed(tx);
                Ok(())
            })
            .expect("seed");
        storage
            .write_repo(|tx| insert_farm3d_revision(tx, &a_farm3d_revision(SLR, 1)))
            .expect("revision");
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: PRINTER.to_string(),
                name: "Printer".to_string(),
                ..Default::default()
            })
            .expect("printer");
        let spool = storage
            .write_repo(|tx| {
                spools_repository::insert_spool(
                    tx,
                    &spool_fields(),
                    &crate::spools::ledger::AmountEntry::Net {
                        net_mg: 1_000_000,
                        confidence: AmountConfidence::Estimated,
                    },
                    None,
                )
            })
            .expect("spool");
        let new_entries = NewEntries {
            slice_revision_id: SLR.to_string(),
            quantity: 1,
            policy: DispatchPolicy::Manual,
            preference: DispatchPreference::LoadedFirst,
            estimate: MaterialEstimate {
                amount_mg: 100_000,
                source: EstimateSource::SliceEstimate,
            },
            manual_printer_id: None,
            lineage_id: None,
        };
        let entry = storage
            .write_repo(|tx| create_entries(tx, &new_entries, NOW))
            .expect("entries")
            .remove(0);
        Rig {
            _temp: temp,
            _lease: lease,
            storage,
            entry,
            spool_id: spool.id,
        }
    }

    /// Reserves the Rig's Spool, inserts a Job against it, and moves the
    /// Queue Entry to `assigned` — the assign transaction's shape, without
    /// this task's assignment logic (a later task's own concern).
    fn insert_test_job(rig: &Rig) -> Job {
        let reservation_id = rig
            .storage
            .write_repo(|tx| {
                reservations::reserve(
                    tx,
                    &rig.spool_id,
                    &ReservationHolder {
                        kind: "job".to_string(),
                        id: "pending".to_string(),
                    },
                    100_000,
                    "op-reserve",
                )
                .map_err(|_| RepositoryError::Storage(StorageError::OperationFailed))
            })
            .expect("reserve");
        let new_job = NewJob {
            id: new_job_id(),
            queue_entry_id: rig.entry.id.clone(),
            slice_revision_id: SLR.to_string(),
            printer_id: PRINTER.to_string(),
            printer_snapshot: PrinterSnapshot {
                name: "Printer".to_string(),
                location: None,
                catalog_ref: None,
                adapter_kind: None,
                profile: a_printer_profile(),
            },
            spool_id: rig.spool_id.clone(),
            reservation_id,
            estimate_mg: 100_000,
            assigned_by: AssignedBy::Operator,
            manual_facts_acknowledged: false,
        };
        let job = rig
            .storage
            .write_repo(|tx| insert_job(tx, &new_job, NOW))
            .expect("insert job");
        rig.storage
            .write_repo(|tx| {
                queue_repository::apply(tx, &rig.entry.id, &EntryEvent::Assign, Some(&job.id), NOW)
            })
            .expect("assign entry");
        job
    }

    #[test]
    fn job_transition_appends_one_event_with_the_next_sequence() {
        let rig = rig();
        let job = insert_test_job(&rig);
        assert_eq!(job.state, JobState::Assigned);

        let staged = rig
            .storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &job.id,
                    JobEventKind::StageHandedOff,
                    JobChange::default(),
                    NOW,
                )
            })
            .expect("transition");

        assert_eq!(staged.state, JobState::Staging);
        assert_eq!(staged.revision, job.revision + 1);

        let events = rig
            .storage
            .read(|conn| Ok(list_events(conn, &job.id)))
            .expect("read")
            .expect("events");
        assert_eq!(events.len(), 2, "the insert event plus this one transition");
        assert_eq!(events[0].kind, JobEventKind::Assigned);
        assert_eq!(events[0].sequence, 1);
        assert_eq!(events[0].from_state, None);
        assert_eq!(events[0].to_state, JobState::Assigned);
        assert_eq!(events[1].kind, JobEventKind::StageHandedOff);
        assert_eq!(events[1].sequence, 2);
        assert_eq!(events[1].from_state, Some(JobState::Assigned));
        assert_eq!(events[1].to_state, JobState::Staging);
    }

    #[test]
    fn illegal_job_transition_is_rejected_before_sql() {
        let rig = rig();
        let job = insert_test_job(&rig);

        // `StartHandedOff` is legal only from `awaitingStart` (D3); the
        // Job is still `assigned`.
        let result = rig.storage.write_repo(|tx| {
            transition(
                tx,
                &job.id,
                JobEventKind::StartHandedOff,
                JobChange::default(),
                NOW,
            )
        });

        match result {
            Err(RepositoryError::IllegalJobTransition { from, event, .. }) => {
                assert_eq!(from, JobState::Assigned);
                assert_eq!(event, JobEventKind::StartHandedOff);
            }
            other => panic!("expected IllegalJobTransition, got {other:?}"),
        }

        let reloaded = rig
            .storage
            .read(|conn| Ok(load_job(conn, &job.id)))
            .expect("read")
            .expect("load")
            .expect("exists");
        assert_eq!(reloaded.state, JobState::Assigned, "nothing was written");
        assert_eq!(reloaded.revision, job.revision);
        let events = rig
            .storage
            .read(|conn| Ok(list_events(conn, &job.id)))
            .expect("read")
            .expect("events");
        assert_eq!(events.len(), 1, "the rejected transition wrote no event");
    }

    #[test]
    fn requirement_is_unique_per_job_and_kind() {
        let rig = rig();
        let job = insert_test_job(&rig);

        let requirement = rig
            .storage
            .write_repo(|tx| {
                open_requirement(
                    tx,
                    &job.id,
                    RequirementKind::MaterialReconciliation,
                    Some(&rig.spool_id),
                    Some(&job.reservation_id),
                    NOW,
                )
            })
            .expect("open");
        assert_eq!(requirement.status, RequirementStatus::Pending);
        assert_eq!(requirement.job_id, job.id);

        let second = rig.storage.write_repo(|tx| {
            open_requirement(
                tx,
                &job.id,
                RequirementKind::MaterialReconciliation,
                Some(&rig.spool_id),
                Some(&job.reservation_id),
                NOW,
            )
        });
        assert!(
            second.is_err(),
            "a second requirement of the same (job, kind) violates the UNIQUE index"
        );

        let resolution =
            serde_json::json!({ "kind": "settled", "method": "estimated", "usedMg": 100_000 });
        let resolved = rig
            .storage
            .write_repo(|tx| {
                set_requirement_status(
                    tx,
                    &requirement.id,
                    RequirementStatus::Resolved,
                    Some(&resolution),
                    NOW,
                )
            })
            .expect("resolve");
        assert_eq!(resolved.status, RequirementStatus::Resolved);
        assert!(resolved.resolved_at.is_some());
        assert_eq!(
            resolved.resolution,
            Some(RequirementResolution::Settled {
                method: SettlementMethod::Estimated,
                used_mg: 100_000,
            })
        );

        let open = rig
            .storage
            .read(|conn| Ok(open_requirements(conn)))
            .expect("read")
            .expect("list");
        assert!(open.is_empty(), "a resolved requirement is not open");
    }

    #[test]
    fn job_history_returns_events_in_sequence_order() {
        let rig = rig();
        let job = insert_test_job(&rig);
        rig.storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &job.id,
                    JobEventKind::StageHandedOff,
                    JobChange::default(),
                    NOW,
                )
            })
            .expect("stage");
        rig.storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &job.id,
                    JobEventKind::StageFailed,
                    JobChange::default(),
                    "2026-02-01T00:00:01.000Z",
                )
            })
            .expect("stage failed");

        let history = rig
            .storage
            .read(|conn| Ok(history(conn, &job.id)))
            .expect("read")
            .expect("history");

        assert_eq!(history.job.id, job.id);
        assert_eq!(history.entry.id, rig.entry.id);
        assert_eq!(history.lineage.len(), 1);
        assert_eq!(history.events.len(), 3);
        assert_eq!(
            history
                .events
                .iter()
                .map(|e| e.sequence)
                .collect::<Vec<_>>(),
            vec![1, 2, 3]
        );
        assert_eq!(history.events[0].kind, JobEventKind::Assigned);
        assert_eq!(history.events[1].kind, JobEventKind::StageHandedOff);
        assert_eq!(history.events[2].kind, JobEventKind::StageFailed);
        assert_eq!(history.reservations.len(), 1);
        assert_eq!(history.reservations[0].id, job.reservation_id);
    }

    /// Fix round 1: a Job's `ended_at` must be set exactly once, on the
    /// first move into a terminal state. `MaterialSettled`,
    /// `MaterialDeferred` (`failed`/`cancelled` -> unchanged), and
    /// `MaterialCorrected` (`completed` -> unchanged) all *keep* a
    /// terminal state — `to.is_terminal()` is true again on each of
    /// those calls, so recomputing `ended_at` from `to` alone
    /// overwrites the original print-end time with the settlement or
    /// correction time.
    #[test]
    fn settling_an_already_terminal_job_does_not_move_its_ended_at() {
        let rig = rig();
        let job = insert_test_job(&rig);

        let cancelled = rig
            .storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &job.id,
                    JobEventKind::CancelledBeforeStart,
                    JobChange {
                        cancel_reason: Some(CancelReason::CancelledBeforeStart),
                        ..JobChange::default()
                    },
                    NOW,
                )
            })
            .expect("cancel before start");
        assert_eq!(cancelled.state, JobState::Cancelled);
        let ended_at_at_cancellation = cancelled.ended_at.clone().expect("ended_at set on cancel");

        let settled = rig
            .storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &job.id,
                    JobEventKind::MaterialSettled,
                    JobChange {
                        settlement_method: Some(SettlementMethod::Estimated),
                        ..JobChange::default()
                    },
                    "2026-02-01T01:00:00.000Z",
                )
            })
            .expect("settle");

        assert_eq!(
            settled.ended_at,
            Some(ended_at_at_cancellation),
            "settling an already-terminal Job must not move ended_at to the settlement time"
        );
    }
}
