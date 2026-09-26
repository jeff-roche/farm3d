//! D2/D3/D5: the SQL for `host_operations`. Every function takes the
//! caller's `&Transaction`/`&Connection` and returns `RepositoryError`
//! (P3+'s pattern), so a command composes several of these into one atomic
//! commit through `Storage::write_repo`.
//!
//! [`insert_dispatching`] is the write-ahead insert with its ledger claim.
//! [`mark_sent`] and [`transition`] are the executor's two commits around
//! the send (D3). [`record_attempt`] is the reconciler's
//! `reconciling -> uncertain` (attempts counted); [`recover_after_restart`]
//! is startup recovery's `dispatching`/`reconciling` sweep (attempts never
//! counted). [`set_no_longer_pending`] is the informational D5 flag, set
//! independently of any state move. The rest are reads: [`load`],
//! [`load_by_operation_id`], [`list_for_printer`], [`list_unresolved`],
//! [`snapshot`], and [`has_unresolved`]. [`delete_terminal_for_printer`] is
//! owner decision 5 (a permanent Printer delete's cascade).

use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};
use serde::de::DeserializeOwned;
use serde::Serialize;

use crate::connections::capabilities::{HostOperationFailureCode, InconclusiveReason};
use crate::persistence::RepositoryError;
use crate::printers::now_rfc3339;
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::{decode_enum, encode_enum};

use super::state;
use super::{
    new_host_operation_id, HostOperation, HostOperationEndpoint, HostOperationFailure,
    HostOperationKind, HostOperationLastAttempt, HostOperationResolution, HostOperationState,
};

/// The columns [`decode_row`] reads, in the order it reads them. A macro,
/// not a `const`, so `concat!` can build each query shape from it at
/// compile time.
macro_rules! host_operation_columns {
    () => {
        "id, printer_id, kind, slice_revision_id, source_host_operation_id, \
         gcode_sha256, gcode_size, host_path, history_mark, endpoint_json, state, failure_json, \
         resolution_json, attempts, last_attempt_at, last_attempt_reason, no_longer_pending, \
         abandoned_at, abandon_note, created_at, dispatched_at, uncertain_since, resolved_at"
    };
}

/// Every row's columns. `WHERE` clauses append after this.
const HOST_OPERATION_SELECT: &str = concat!(
    "SELECT ",
    host_operation_columns!(),
    " FROM host_operations"
);

/// The same columns over [`snapshot`]'s `ranked_terminal` window.
const RANKED_TERMINAL_SELECT: &str = concat!(
    "SELECT ",
    host_operation_columns!(),
    " FROM ranked_terminal"
);

fn to_json(value: &impl Serialize) -> String {
    serde_json::to_string(value).expect("host_ops wire types always serialize")
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

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
}

fn illegal(id: &str, from: HostOperationState, to: HostOperationState) -> RepositoryError {
    RepositoryError::IllegalHostOperationTransition {
        host_operation_id: id.to_string(),
        from,
        to,
    }
}

/// Wraps a `state::transition` rejection as a `RepositoryError`, reporting
/// the state the rejected event would have reached.
fn illegal_from(id: &str, error: state::IllegalTransition) -> RepositoryError {
    illegal(id, error.from, error.attempted_target())
}

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<HostOperation> {
    let last_attempt_at: Option<String> = row.get(14)?;
    let last_attempt_reason: Option<String> = row.get(15)?;
    let last_attempt = match (last_attempt_at, last_attempt_reason) {
        (Some(at), Some(reason)) => Some(HostOperationLastAttempt {
            at,
            reason: decode_text_enum(15, &reason)?,
        }),
        _ => None,
    };
    let failure: Option<String> = row.get(11)?;
    let resolution: Option<String> = row.get(12)?;

    Ok(HostOperation {
        id: row.get(0)?,
        printer_id: row.get(1)?,
        kind: decode_text_enum(2, &row.get::<_, String>(2)?)?,
        slice_revision_id: row.get(3)?,
        source_host_operation_id: row.get(4)?,
        gcode_sha256: row.get(5)?,
        gcode_size: row.get(6)?,
        host_path: row.get(7)?,
        history_mark: row.get(8)?,
        endpoint: from_json(9, &row.get::<_, String>(9)?)?,
        state: decode_text_enum(10, &row.get::<_, String>(10)?)?,
        failure: failure.map(|text| from_json(11, &text)).transpose()?,
        resolution: resolution.map(|text| from_json(12, &text)).transpose()?,
        attempts: row.get(13)?,
        last_attempt,
        no_longer_pending: row.get::<_, i64>(16)? != 0,
        abandoned_at: row.get(17)?,
        abandon_note: row.get(18)?,
        created_at: row.get(19)?,
        dispatched_at: row.get(20)?,
        uncertain_since: row.get(21)?,
        resolved_at: row.get(22)?,
    })
}

pub fn load(connection: &Connection, id: &str) -> Result<Option<HostOperation>, RepositoryError> {
    Ok(connection
        .query_row(
            &format!("{HOST_OPERATION_SELECT} WHERE id = ?1"),
            [id],
            decode_row,
        )
        .optional()?)
}

pub fn load_by_operation_id(
    connection: &Connection,
    operation_id: &str,
) -> Result<Option<HostOperation>, RepositoryError> {
    Ok(connection
        .query_row(
            &format!("{HOST_OPERATION_SELECT} WHERE operation_id = ?1"),
            [operation_id],
            decode_row,
        )
        .optional()?)
}

pub fn list_for_printer(
    connection: &Connection,
    printer_id: &str,
) -> Result<Vec<HostOperation>, RepositoryError> {
    let mut statement = connection.prepare(&format!(
        "{HOST_OPERATION_SELECT} WHERE printer_id = ?1 ORDER BY created_at, id"
    ))?;
    let rows = statement
        .query_map([printer_id], decode_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every unresolved (`dispatching`/`uncertain`/`reconciling`) row, for one
/// Printer or (`None`) every Printer.
pub fn list_unresolved(
    connection: &Connection,
    printer_id: Option<&str>,
) -> Result<Vec<HostOperation>, RepositoryError> {
    let mut statement = connection.prepare(&format!(
        "{HOST_OPERATION_SELECT}
         WHERE state IN ('dispatching','uncertain','reconciling') AND (?1 IS NULL OR printer_id = ?1)
         ORDER BY created_at, id"
    ))?;
    let rows = statement
        .query_map(params![printer_id], decode_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

pub fn has_unresolved(connection: &Connection, printer_id: &str) -> Result<bool, RepositoryError> {
    Ok(connection.query_row(
        "SELECT EXISTS(
             SELECT 1 FROM host_operations
             WHERE printer_id = ?1 AND state IN ('dispatching','uncertain','reconciling')
         )",
        [printer_id],
        |row| row.get(0),
    )?)
}

/// `list_host_operations`' backfill (spec "Commands"): every unresolved
/// row, the newest `succeeded` upload per `host_path` (the staged
/// artifacts), and the newest 20 other terminal rows — each per Printer,
/// scoped to `printer_id` when given. Ordered newest first.
pub fn snapshot(
    connection: &Connection,
    printer_id: Option<&str>,
) -> Result<Vec<HostOperation>, RepositoryError> {
    let sql = format!(
        "WITH staged AS (
             SELECT id FROM (
                 SELECT id, ROW_NUMBER() OVER (
                     PARTITION BY printer_id, host_path ORDER BY created_at DESC, id DESC
                 ) AS rn
                 FROM host_operations
                 WHERE kind = 'upload' AND state = 'succeeded' AND (?1 IS NULL OR printer_id = ?1)
             ) WHERE rn = 1
         ),
         ranked_terminal AS (
             SELECT *, ROW_NUMBER() OVER (
                 PARTITION BY printer_id ORDER BY created_at DESC, id DESC
             ) AS rn
             FROM host_operations
             WHERE state IN ('succeeded','failed','abandoned')
               AND id NOT IN (SELECT id FROM staged)
               AND (?1 IS NULL OR printer_id = ?1)
         )
         {select} WHERE state IN ('dispatching','uncertain','reconciling') AND (?1 IS NULL OR printer_id = ?1)
         UNION ALL
         {select} WHERE id IN (SELECT id FROM staged)
         UNION ALL
         {select_ranked} WHERE rn <= 20
         ORDER BY created_at DESC, id DESC",
        select = HOST_OPERATION_SELECT,
        select_ranked = RANKED_TERMINAL_SELECT,
    );
    let mut statement = connection.prepare(&sql)?;
    let rows = statement
        .query_map(params![printer_id], decode_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Owner decision 5: a permanent Printer delete also deletes that
/// Printer's *terminal* Host Operation rows (`printer_id` is
/// `ON DELETE RESTRICT`, so these must go first).
pub fn delete_terminal_for_printer(
    tx: &Transaction<'_>,
    printer_id: &str,
) -> Result<usize, RepositoryError> {
    Ok(tx.execute(
        "DELETE FROM host_operations WHERE printer_id = ?1 AND state IN ('succeeded','failed','abandoned')",
        [printer_id],
    )?)
}

/// The write-ahead row (D2/D3): what [`insert_dispatching`] needs beyond
/// the ledger claim.
pub struct NewHostOperation {
    pub operation_id: String,
    pub operation_kind: OperationKind,
    pub request_digest: String,
    pub printer_id: String,
    pub kind: HostOperationKind,
    pub slice_revision_id: Option<String>,
    pub source_host_operation_id: Option<String>,
    pub gcode_sha256: Option<String>,
    pub gcode_size: Option<i64>,
    pub host_path: String,
    pub history_mark: Option<i64>,
    pub endpoint: HostOperationEndpoint,
}

/// D2 "Write-ahead": claims `operation_id` in the operations ledger, then
/// writes the row `dispatching` — one transaction. A replay of the same
/// request (same kind and digest) returns the current row found by
/// `operation_id`, with nothing written; a reused id for a different
/// request is [`RepositoryError::OperationIdReused`]. A rejected request
/// (any `Err` the caller's transaction doesn't commit) never burns its id.
pub fn insert_dispatching(
    tx: &Transaction<'_>,
    new_operation: &NewHostOperation,
) -> Result<HostOperation, RepositoryError> {
    match operations::claim(
        tx,
        &new_operation.operation_id,
        new_operation.operation_kind,
        &new_operation.request_digest,
    )? {
        Claim::Replay => {
            return load_by_operation_id(tx, &new_operation.operation_id)?
                .ok_or_else(|| not_found(&new_operation.operation_id));
        }
        Claim::Fresh => {}
    }

    let id = new_host_operation_id();
    tx.execute(
        "INSERT INTO host_operations(
             id, operation_id, printer_id, kind, slice_revision_id, source_host_operation_id,
             gcode_sha256, gcode_size, host_path, history_mark, endpoint_json, state, created_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, 'dispatching', ?12)",
        params![
            id,
            new_operation.operation_id,
            new_operation.printer_id,
            encode_enum(new_operation.kind),
            new_operation.slice_revision_id,
            new_operation.source_host_operation_id,
            new_operation.gcode_sha256,
            new_operation.gcode_size,
            new_operation.host_path,
            new_operation.history_mark,
            to_json(&new_operation.endpoint),
            now_rfc3339(),
        ],
    )?;
    load(tx, &id)?.ok_or_else(|| not_found(&id))
}

/// D3 "two commits around the send": commits `dispatched_at` immediately
/// before the adapter opens its connection. Only legal once, on a
/// `dispatching` row that hasn't been sent yet: no such row is
/// [`RepositoryError::NotFound`], and a row that exists but is already
/// sent or isn't `dispatching` is
/// [`RepositoryError::HostOperationAlreadySent`] — an executor bug either
/// way (the invariant is that the executor sends nothing unless this
/// returns `Ok`), but a distinct error so the two can't be confused.
pub fn mark_sent(tx: &Transaction<'_>, id: &str) -> Result<HostOperation, RepositoryError> {
    let current = load(tx, id)?.ok_or_else(|| not_found(id))?;
    if current.state != HostOperationState::Dispatching || current.dispatched_at.is_some() {
        return Err(RepositoryError::HostOperationAlreadySent {
            host_operation_id: id.to_string(),
        });
    }
    tx.execute(
        "UPDATE host_operations SET dispatched_at = ?2 WHERE id = ?1",
        params![id, now_rfc3339()],
    )?;
    load(tx, id)?.ok_or_else(|| not_found(id))
}

/// The outcome [`transition`] commits, one variant per legal D3 edge it
/// covers. `reconciling -> uncertain` (inconclusive) is
/// [`record_attempt`]'s job instead — it counts an attempt, which no edge
/// here does — and every startup edge is [`recover_after_restart`]'s.
#[derive(Clone, Debug)]
pub enum Outcome {
    /// `dispatching`/`reconciling` -> `succeeded`.
    Succeeded { resolution: HostOperationResolution },
    /// `dispatching`/`reconciling` -> `failed`.
    Failed { failure: HostOperationFailure },
    /// `dispatching` -> `uncertain`: the executor's own indeterminate
    /// dispatch result (D3). Sets `uncertain_since` — D2's one and only
    /// place that column is written.
    Uncertain {
        reason: InconclusiveReason,
        no_longer_pending: bool,
    },
    /// `uncertain` -> `reconciling`: an attempt begins.
    Reconciling,
    /// `uncertain` -> `abandoned` (D8).
    Abandoned { note: Option<String> },
}

impl Outcome {
    fn target(&self) -> HostOperationState {
        match self {
            Outcome::Succeeded { .. } => HostOperationState::Succeeded,
            Outcome::Failed { .. } => HostOperationState::Failed,
            Outcome::Uncertain { .. } => HostOperationState::Uncertain,
            Outcome::Reconciling => HostOperationState::Reconciling,
            Outcome::Abandoned { .. } => HostOperationState::Abandoned,
        }
    }
}

/// Which D3 event an `Outcome` fires, given the row's current state. The
/// same `Outcome` variant maps to a different event depending on where
/// the row is coming from — `Succeeded`/`Failed` from `dispatching` are
/// the executor's definitive dispatch answer; from anywhere else (in
/// practice, only `reconciling`) they're a reconcile read's proof. That
/// distinction is exactly why `state.rs` is keyed on events, not bare
/// `(from, to)` pairs (see its module docs): a `(from, to)`-keyed check
/// couldn't tell `record_attempt`'s `reconciling -> uncertain`
/// (`Inconclusive`) apart from this function's own `dispatching ->
/// uncertain` (`Indeterminate`), and letting the latter fire from
/// `reconciling` would reset `uncertain_since` — exactly the bug this
/// event-keyed table prevents.
fn event_for(current_state: HostOperationState, outcome: &Outcome) -> state::Event {
    match outcome {
        Outcome::Succeeded { .. } if current_state == HostOperationState::Dispatching => {
            state::Event::DefinitiveSuccess
        }
        Outcome::Succeeded { .. } => state::Event::ProvedApplied,
        Outcome::Failed { .. } if current_state == HostOperationState::Dispatching => {
            state::Event::DefinitiveFailure
        }
        Outcome::Failed { .. } => state::Event::ProvedNotApplied,
        Outcome::Uncertain { .. } => state::Event::Indeterminate,
        Outcome::Reconciling => state::Event::AttemptBegins,
        Outcome::Abandoned { .. } => state::Event::Abandon,
    }
}

/// Moves `id` to `outcome`'s target state, after checking `state::transition`
/// allows the resulting event ([`event_for`]) from the row's current state
/// — nothing is written otherwise
/// ([`RepositoryError::IllegalHostOperationTransition`]). D3/D5's extra
/// rule beyond state alone: `reconciling`'s "proved not applied"
/// (`ProvedNotApplied`) only ever fires for an `upload` row — a start,
/// pause, resume, or cancel is never failed by reconciliation.
pub fn transition(
    tx: &Transaction<'_>,
    id: &str,
    outcome: Outcome,
) -> Result<HostOperation, RepositoryError> {
    transition_at(tx, id, outcome, &now_rfc3339())
}

/// [`transition`], stamping `uncertain_since`, `resolved_at`, and the
/// other times it writes with `now` (RFC 3339). The executor passes its
/// injected clock, which the settle check compares `uncertain_since`
/// against, as `recover_after_restart` does.
pub fn transition_at(
    tx: &Transaction<'_>,
    id: &str,
    outcome: Outcome,
    now: &str,
) -> Result<HostOperation, RepositoryError> {
    let current = load(tx, id)?.ok_or_else(|| not_found(id))?;
    let event = event_for(current.state, &outcome);
    if event == state::Event::ProvedNotApplied && current.kind != HostOperationKind::Upload {
        return Err(illegal(id, current.state, outcome.target()));
    }
    state::transition(current.state, event).map_err(|error| illegal_from(id, error))?;

    match outcome {
        Outcome::Succeeded { resolution } => tx.execute(
            "UPDATE host_operations SET state = 'succeeded', resolution_json = ?2, resolved_at = ?3
             WHERE id = ?1",
            params![id, to_json(&resolution), now],
        )?,
        Outcome::Failed { failure } => tx.execute(
            "UPDATE host_operations SET state = 'failed', failure_json = ?2, resolved_at = ?3
             WHERE id = ?1",
            params![id, to_json(&failure), now],
        )?,
        Outcome::Uncertain {
            reason,
            no_longer_pending,
        } => tx.execute(
            "UPDATE host_operations
             SET state = 'uncertain', uncertain_since = ?2, last_attempt_at = ?2,
                 last_attempt_reason = ?3, no_longer_pending = no_longer_pending OR ?4
             WHERE id = ?1",
            params![id, now, encode_enum(reason), no_longer_pending],
        )?,
        Outcome::Reconciling => tx.execute(
            "UPDATE host_operations SET state = 'reconciling' WHERE id = ?1",
            [id],
        )?,
        Outcome::Abandoned { note } => tx.execute(
            "UPDATE host_operations
             SET state = 'abandoned', abandoned_at = ?2, abandon_note = ?3, resolved_at = ?2
             WHERE id = ?1",
            params![id, now, note],
        )?,
    };
    load(tx, id)?.ok_or_else(|| not_found(id))
}

/// D3 `reconciling -> uncertain` (`Event::Inconclusive`): `attempts += 1`,
/// the reason recorded, `uncertain_since` left untouched. Only legal from
/// `reconciling` — `Event::Inconclusive`'s own required source state, so
/// this can never fire on a `dispatching` row (that's [`transition`]'s
/// `Event::Indeterminate`, a different event to the same target, and it
/// never counts an attempt).
pub fn record_attempt(
    tx: &Transaction<'_>,
    id: &str,
    reason: InconclusiveReason,
) -> Result<HostOperation, RepositoryError> {
    let current = load(tx, id)?.ok_or_else(|| not_found(id))?;
    state::transition(current.state, state::Event::Inconclusive)
        .map_err(|error| illegal_from(id, error))?;
    tx.execute(
        "UPDATE host_operations
         SET state = 'uncertain', attempts = attempts + 1,
             last_attempt_at = ?2, last_attempt_reason = ?3
         WHERE id = ?1",
        params![id, now_rfc3339(), encode_enum(reason)],
    )?;
    load(tx, id)?.ok_or_else(|| not_found(id))
}

/// D5: sets the informational `no_longer_pending` flag (never changes the
/// state, and never clears once set).
pub fn set_no_longer_pending(
    tx: &Transaction<'_>,
    id: &str,
) -> Result<HostOperation, RepositoryError> {
    let affected = tx.execute(
        "UPDATE host_operations SET no_longer_pending = 1 WHERE id = ?1",
        [id],
    )?;
    if affected == 0 {
        return Err(not_found(id));
    }
    load(tx, id)?.ok_or_else(|| not_found(id))
}

fn collect_ids(tx: &Transaction<'_>, sql: &str) -> Result<Vec<String>, RepositoryError> {
    let mut statement = tx.prepare(sql)?;
    let ids = statement
        .query_map([], |row| row.get(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(ids)
}

/// D3's startup recovery, run once inside `build_runtime_services` before
/// commands are served and before the first reconcile pass. Replaces the
/// plan's `mark_dispatching_uncertain`:
///
/// - `dispatching` with `dispatched_at IS NULL` (never sent) -> `failed`
///   with [`HostOperationFailureCode::NeverSent`].
/// - `dispatching` with `dispatched_at` set (may have been sent) ->
///   `uncertain`, `uncertain_since = now`, reason
///   [`InconclusiveReason::InterruptedByRestart`].
/// - `reconciling` (a crash mid-attempt) -> `uncertain`, `attempts` and
///   `uncertain_since` unchanged.
///
/// Returns every row this touched, so the caller can publish their events
/// after commit.
pub fn recover_after_restart(
    tx: &Transaction<'_>,
    now: &str,
) -> Result<Vec<HostOperation>, RepositoryError> {
    let never_sent_ids = collect_ids(
        tx,
        "SELECT id FROM host_operations WHERE state = 'dispatching' AND dispatched_at IS NULL",
    )?;
    let interrupted_ids = collect_ids(
        tx,
        "SELECT id FROM host_operations WHERE state = 'dispatching' AND dispatched_at IS NOT NULL",
    )?;
    let returned_ids = collect_ids(
        tx,
        "SELECT id FROM host_operations WHERE state = 'reconciling'",
    )?;

    // Every row this touches is already known (by its `WHERE` clause) to
    // be in the one state each event requires, so these can never fail —
    // but going through `state::transition` still ties the state written
    // below to D3's table, rather than a hand-typed literal that could
    // drift from it.
    let never_sent_state = state::transition(
        HostOperationState::Dispatching,
        state::Event::StartupNeverSent,
    )
    .expect("D3: dispatching -> failed (StartupNeverSent) is always legal");
    let interrupted_state =
        state::transition(HostOperationState::Dispatching, state::Event::StartupSent)
            .expect("D3: dispatching -> uncertain (StartupSent) is always legal");
    let reconciling_state = state::transition(
        HostOperationState::Reconciling,
        state::Event::StartupReconciling,
    )
    .expect("D3: reconciling -> uncertain (StartupReconciling) is always legal");

    let never_sent_failure = to_json(&HostOperationFailure::for_code(
        HostOperationFailureCode::NeverSent,
    ));
    tx.execute(
        "UPDATE host_operations SET state = ?3, failure_json = ?2, resolved_at = ?1
         WHERE state = 'dispatching' AND dispatched_at IS NULL",
        params![now, never_sent_failure, encode_enum(never_sent_state)],
    )?;
    tx.execute(
        "UPDATE host_operations
         SET state = ?3, uncertain_since = ?1, last_attempt_at = ?1,
             last_attempt_reason = ?2
         WHERE state = 'dispatching' AND dispatched_at IS NOT NULL",
        params![
            now,
            encode_enum(InconclusiveReason::InterruptedByRestart),
            encode_enum(interrupted_state),
        ],
    )?;
    tx.execute(
        "UPDATE host_operations SET state = ?1 WHERE state = 'reconciling'",
        params![encode_enum(reconciling_state)],
    )?;

    let mut recovered = Vec::new();
    for id in never_sent_ids
        .into_iter()
        .chain(interrupted_ids)
        .chain(returned_ids)
    {
        if let Some(row) = load(tx, &id)? {
            recovered.push(row);
        }
    }
    Ok(recovered)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::host_ops::{
        HostOperationLastAttempt, HostOperationObservedState, StartEvidenceSource,
    };
    use crate::persistence::StorageError;
    use crate::printers::repository::PrinterRepository;
    use crate::printers::StoredPrinter;

    fn endpoint() -> HostOperationEndpoint {
        HostOperationEndpoint {
            kind: "moonraker".to_string(),
            host: "192.0.2.1".to_string(),
            port: 7125,
        }
    }

    fn seed_printer_arc(storage: &std::sync::Arc<crate::persistence::Storage>, id: &str) {
        PrinterRepository::new(std::sync::Arc::clone(storage))
            .create(StoredPrinter {
                id: id.to_string(),
                name: "Printer".to_string(),
                ..Default::default()
            })
            .expect("printer");
    }

    /// `load`, unwrapped: the two levels `storage.read` adds plus the row
    /// itself, since every caller here knows the row exists.
    fn load_row(storage: &std::sync::Arc<crate::persistence::Storage>, id: &str) -> HostOperation {
        storage
            .read(|connection| Ok(load(connection, id)))
            .expect("read")
            .expect("repository")
            .expect("row exists")
    }

    fn upload(operation_id: &str, printer_id: &str, host_path: &str) -> NewHostOperation {
        NewHostOperation {
            operation_id: operation_id.to_string(),
            operation_kind: OperationKind::StageSliceRevision,
            // Ties the digest to every field a real command's digest would
            // cover, so a reused id with different content (`host_path`)
            // is `OperationIdReused`, not a replay.
            request_digest: format!("digest-{printer_id}-{host_path}"),
            printer_id: printer_id.to_string(),
            kind: HostOperationKind::Upload,
            slice_revision_id: None,
            source_host_operation_id: None,
            gcode_sha256: Some("a".repeat(64)),
            gcode_size: Some(100),
            host_path: host_path.to_string(),
            history_mark: None,
            endpoint: endpoint(),
        }
    }

    /// A non-upload (control) row: `pause`, which `reconciling -> failed`
    /// (m1) must never reach.
    fn pause(operation_id: &str, printer_id: &str, host_path: &str) -> NewHostOperation {
        NewHostOperation {
            operation_id: operation_id.to_string(),
            operation_kind: OperationKind::PauseHostPrint,
            request_digest: format!("digest-{printer_id}-{host_path}"),
            printer_id: printer_id.to_string(),
            kind: HostOperationKind::Pause,
            slice_revision_id: None,
            source_host_operation_id: None,
            gcode_sha256: None,
            gcode_size: None,
            host_path: host_path.to_string(),
            history_mark: None,
            endpoint: endpoint(),
        }
    }

    /// Drives a freshly `insert_dispatching`-ed row through
    /// `dispatching -> uncertain -> reconciling`, the path every
    /// `reconciling`-only test needs before it can exercise its own event.
    fn advance_to_reconciling(storage: &std::sync::Arc<crate::persistence::Storage>, id: &str) {
        storage
            .write_repo(|tx| {
                transition(
                    tx,
                    id,
                    Outcome::Uncertain {
                        reason: InconclusiveReason::ResponseLost,
                        no_longer_pending: false,
                    },
                )
            })
            .expect("dispatching -> uncertain");
        storage
            .write_repo(|tx| transition(tx, id, Outcome::Reconciling))
            .expect("uncertain -> reconciling");
    }

    #[test]
    fn insert_dispatching_writes_a_dispatching_row_with_the_ledger_claim() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");

        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        assert_eq!(operation.state, HostOperationState::Dispatching);
        assert!(operation.id.starts_with("hop-"));
        assert_eq!(operation.printer_id, "prn-a");
        assert!(!operation.created_at.is_empty());
        assert!(operation.dispatched_at.is_none());
    }

    #[test]
    fn a_replay_of_the_same_request_returns_the_existing_row_and_writes_nothing() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");

        let first = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        let replay = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("replay");

        assert_eq!(first.id, replay.id);
        let all = storage
            .read(|connection| Ok(list_for_printer(connection, "prn-a")))
            .expect("read")
            .expect("list");
        assert_eq!(all.len(), 1, "a replay must write no second row");
    }

    #[test]
    fn a_reused_operation_id_for_a_different_request_is_rejected() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");

        storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        let result = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/b.gcode")));

        assert!(matches!(result, Err(RepositoryError::OperationIdReused)));
    }

    #[test]
    fn a_second_unresolved_row_for_the_same_printer_violates_the_partial_unique_index() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");

        storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("first insert");
        let result = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-2", "prn-a", "farm3d/b.gcode")));

        // The raw SQLite constraint violation, unmapped to any
        // Host-Operation-specific error — a later task's guard is meant
        // to catch this case earlier and report `HOST_OPERATION_PENDING`
        // instead of ever reaching this index.
        assert!(
            matches!(
                result,
                Err(RepositoryError::Storage(StorageError::Database))
            ),
            "unexpected error: {result:?}"
        );
    }

    #[test]
    fn mark_sent_commits_dispatched_at_once_and_rejects_a_second_call() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        let sent = storage
            .write_repo(|tx| mark_sent(tx, &operation.id))
            .expect("mark sent");
        assert!(sent.dispatched_at.is_some());

        let second = storage.write_repo(|tx| mark_sent(tx, &operation.id));
        assert!(matches!(
            second,
            Err(RepositoryError::HostOperationAlreadySent { .. })
        ));
    }

    #[test]
    fn mark_sent_on_a_missing_row_is_not_found() {
        let (_temp, _lease, storage) = crate::test_storage();
        let result = storage.write_repo(|tx| mark_sent(tx, "hop-missing"));
        assert!(matches!(result, Err(RepositoryError::NotFound { .. })));
    }

    #[test]
    fn mark_sent_on_a_non_dispatching_row_is_already_sent_not_not_found() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &operation.id,
                    Outcome::Failed {
                        failure: HostOperationFailure {
                            code: HostOperationFailureCode::HostUnreachable,
                            message: "unreachable".to_string(),
                        },
                    },
                )
            })
            .expect("fail");

        let result = storage.write_repo(|tx| mark_sent(tx, &operation.id));
        assert!(matches!(
            result,
            Err(RepositoryError::HostOperationAlreadySent { .. })
        ));
    }

    #[test]
    fn transition_persists_a_legal_move_and_rejects_an_illegal_one() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        let succeeded = storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &operation.id,
                    Outcome::Succeeded {
                        resolution: HostOperationResolution::ArtifactVerified { reconciled: false },
                    },
                )
            })
            .expect("dispatching -> succeeded");
        assert_eq!(succeeded.state, HostOperationState::Succeeded);
        assert!(succeeded.resolved_at.is_some());
        assert_eq!(
            succeeded.resolution,
            Some(HostOperationResolution::ArtifactVerified { reconciled: false })
        );

        // A terminal row never accepts another move.
        let illegal_result =
            storage.write_repo(|tx| transition(tx, &operation.id, Outcome::Reconciling));
        assert!(matches!(
            illegal_result,
            Err(RepositoryError::IllegalHostOperationTransition { .. })
        ));
    }

    #[test]
    fn the_uncertain_since_column_is_written_once_and_never_changes() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        let uncertain = storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &operation.id,
                    Outcome::Uncertain {
                        reason: InconclusiveReason::ResponseLost,
                        no_longer_pending: false,
                    },
                )
            })
            .expect("dispatching -> uncertain");
        let first_uncertain_since = uncertain
            .uncertain_since
            .clone()
            .expect("uncertain_since set");
        // `last_attempt_at` and `uncertain_since` are set from the same
        // `now` in this one UPDATE.
        assert_eq!(
            uncertain.last_attempt,
            Some(HostOperationLastAttempt {
                at: first_uncertain_since.clone(),
                reason: InconclusiveReason::ResponseLost,
            })
        );

        storage
            .write_repo(|tx| transition(tx, &operation.id, Outcome::Reconciling))
            .expect("uncertain -> reconciling");
        let reconciled_again = storage
            .write_repo(|tx| record_attempt(tx, &operation.id, InconclusiveReason::HostNotReady))
            .expect("reconciling -> uncertain");

        assert_eq!(reconciled_again.attempts, 1);
        assert_eq!(
            reconciled_again.uncertain_since,
            Some(first_uncertain_since),
            "uncertain_since must never change after it is first set"
        );
    }

    #[test]
    fn record_attempt_is_illegal_outside_reconciling() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        let result = storage
            .write_repo(|tx| record_attempt(tx, &operation.id, InconclusiveReason::ResponseLost));
        assert!(matches!(
            result,
            Err(RepositoryError::IllegalHostOperationTransition { .. })
        ));
    }

    /// Regression test for the bug fix round 1 exists to close: a
    /// `(from, to)`-keyed `transition` let a `reconciling` row accept the
    /// dispatch-time `Indeterminate` event (`Outcome::Uncertain`) because it
    /// shares `reconciling`'s own `Inconclusive` target (`uncertain`). That
    /// reset `uncertain_since` and skipped `attempts += 1`, breaking the
    /// binding rule that `uncertain_since` is never changed on
    /// `reconciling -> uncertain`. The event-keyed `state::transition` must
    /// reject it outright, writing nothing.
    #[test]
    fn a_reconciling_row_rejects_the_dispatch_time_indeterminate_event_and_keeps_uncertain_since() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        advance_to_reconciling(&storage, &operation.id);
        let uncertain_since_before = load_row(&storage, &operation.id)
            .uncertain_since
            .expect("uncertain_since set");

        // The dispatch-time Indeterminate event (Outcome::Uncertain) is
        // dispatching-only; a reconciling row must reject it, not silently
        // reset uncertain_since via the shared `uncertain` target.
        let result = storage.write_repo(|tx| {
            transition(
                tx,
                &operation.id,
                Outcome::Uncertain {
                    reason: InconclusiveReason::HostUnreachable,
                    no_longer_pending: false,
                },
            )
        });
        assert!(matches!(
            result,
            Err(RepositoryError::IllegalHostOperationTransition { .. })
        ));

        let reloaded = load_row(&storage, &operation.id);
        assert_eq!(reloaded.state, HostOperationState::Reconciling);
        assert_eq!(
            reloaded.uncertain_since,
            Some(uncertain_since_before),
            "a rejected transition must never touch uncertain_since"
        );
        assert_eq!(
            reloaded.attempts, 0,
            "a rejected transition never counts an attempt"
        );
    }

    #[test]
    fn reconciling_to_succeeded_and_failed_are_legal_for_an_upload_row() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");

        let succeed_target = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        advance_to_reconciling(&storage, &succeed_target.id);
        let succeeded = storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &succeed_target.id,
                    Outcome::Succeeded {
                        resolution: HostOperationResolution::ArtifactVerified { reconciled: true },
                    },
                )
            })
            .expect("reconciling -> succeeded");
        assert_eq!(succeeded.state, HostOperationState::Succeeded);

        let fail_target = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-2", "prn-a", "farm3d/b.gcode")))
            .expect("insert");
        advance_to_reconciling(&storage, &fail_target.id);
        let failed = storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &fail_target.id,
                    Outcome::Failed {
                        failure: HostOperationFailure::for_code(
                            HostOperationFailureCode::NotApplied,
                        ),
                    },
                )
            })
            .expect("reconciling -> failed");
        assert_eq!(failed.state, HostOperationState::Failed);
    }

    /// m1: `reconciling -> failed` ("proved not applied") is legal only for
    /// an `upload` row — start and control operations are never failed by
    /// reconciliation.
    #[test]
    fn reconciling_to_failed_is_illegal_for_a_non_upload_kind() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &pause("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        advance_to_reconciling(&storage, &operation.id);

        let result = storage.write_repo(|tx| {
            transition(
                tx,
                &operation.id,
                Outcome::Failed {
                    failure: HostOperationFailure::for_code(HostOperationFailureCode::NotApplied),
                },
            )
        });
        assert!(matches!(
            result,
            Err(RepositoryError::IllegalHostOperationTransition { .. })
        ));

        let reloaded = load_row(&storage, &operation.id);
        assert_eq!(reloaded.state, HostOperationState::Reconciling);
    }

    /// m4: a non-terminal illegal pair not covered by any of the tests
    /// above — rejected by `state::transition` before any SQL runs.
    #[test]
    fn dispatching_to_reconciling_is_illegal_and_writes_nothing() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        let result = storage.write_repo(|tx| transition(tx, &operation.id, Outcome::Reconciling));
        assert!(matches!(
            result,
            Err(RepositoryError::IllegalHostOperationTransition { .. })
        ));

        let reloaded = load_row(&storage, &operation.id);
        assert_eq!(reloaded.state, HostOperationState::Dispatching);
        assert!(reloaded.dispatched_at.is_none());
    }

    #[test]
    fn abandon_moves_uncertain_to_abandoned_with_its_note() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &operation.id,
                    Outcome::Uncertain {
                        reason: InconclusiveReason::ResponseLost,
                        no_longer_pending: false,
                    },
                )
            })
            .expect("dispatching -> uncertain");

        let abandoned = storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &operation.id,
                    Outcome::Abandoned {
                        note: Some("operator note".to_string()),
                    },
                )
            })
            .expect("uncertain -> abandoned");

        assert_eq!(abandoned.state, HostOperationState::Abandoned);
        assert_eq!(abandoned.abandon_note, Some("operator note".to_string()));
        assert!(abandoned.abandoned_at.is_some());
    }

    #[test]
    fn set_no_longer_pending_never_changes_state() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        let flagged = storage
            .write_repo(|tx| set_no_longer_pending(tx, &operation.id))
            .expect("flag");
        assert!(flagged.no_longer_pending);
        assert_eq!(flagged.state, HostOperationState::Dispatching);
    }

    #[test]
    fn recover_after_restart_covers_every_startup_rule() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        seed_printer_arc(&storage, "prn-b");
        seed_printer_arc(&storage, "prn-c");

        // never sent: dispatching, dispatched_at IS NULL.
        let never_sent = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");

        // interrupted: dispatching, dispatched_at set.
        let interrupted = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-2", "prn-b", "farm3d/b.gcode")))
            .expect("insert");
        storage
            .write_repo(|tx| mark_sent(tx, &interrupted.id))
            .expect("mark sent");

        // returned: reconciling.
        let returned = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-3", "prn-c", "farm3d/c.gcode")))
            .expect("insert");
        storage
            .write_repo(|tx| mark_sent(tx, &returned.id))
            .expect("mark sent");
        storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &returned.id,
                    Outcome::Uncertain {
                        reason: InconclusiveReason::ResponseLost,
                        no_longer_pending: false,
                    },
                )
            })
            .expect("dispatching -> uncertain");
        storage
            .write_repo(|tx| transition(tx, &returned.id, Outcome::Reconciling))
            .expect("uncertain -> reconciling");
        let uncertain_since_before_crash = load_row(&storage, &returned.id)
            .uncertain_since
            .expect("uncertain_since set before crash");

        let now = "2026-02-02T00:00:00.000Z";
        let recovered = storage
            .write_repo(|tx| recover_after_restart(tx, now))
            .expect("recover");
        assert_eq!(recovered.len(), 3);

        let after_never_sent = load_row(&storage, &never_sent.id);
        assert_eq!(after_never_sent.state, HostOperationState::Failed);
        assert_eq!(
            after_never_sent.failure.map(|failure| failure.code),
            Some(HostOperationFailureCode::NeverSent)
        );

        let after_interrupted = load_row(&storage, &interrupted.id);
        assert_eq!(after_interrupted.state, HostOperationState::Uncertain);
        assert_eq!(after_interrupted.uncertain_since, Some(now.to_string()));
        assert_eq!(
            after_interrupted.last_attempt.map(|attempt| attempt.reason),
            Some(InconclusiveReason::InterruptedByRestart)
        );

        let after_returned = load_row(&storage, &returned.id);
        assert_eq!(after_returned.state, HostOperationState::Uncertain);
        assert_eq!(after_returned.attempts, 0, "attempts must be unchanged");
        assert_eq!(
            after_returned.uncertain_since,
            Some(uncertain_since_before_crash),
            "uncertain_since must be kept, not reset to `now`"
        );
    }

    /// Backdates a row's `created_at` while it is still non-terminal, so
    /// two inserts made back to back in the same test (the `created_at`
    /// clock only has second granularity) rank unambiguously, as they
    /// would for two real writes made apart in time.
    fn backdate(storage: &std::sync::Arc<crate::persistence::Storage>, id: &str, created_at: &str) {
        storage
            .write_repo(|tx| {
                Ok(tx.execute(
                    "UPDATE host_operations SET created_at = ?2 WHERE id = ?1",
                    params![id, created_at],
                )?)
            })
            .expect("backdate");
    }

    fn succeed_upload(storage: &std::sync::Arc<crate::persistence::Storage>, id: &str) {
        storage
            .write_repo(|tx| {
                transition(
                    tx,
                    id,
                    Outcome::Succeeded {
                        resolution: HostOperationResolution::ArtifactVerified { reconciled: false },
                    },
                )
            })
            .expect("succeed");
    }

    #[test]
    fn snapshot_stages_only_the_newest_upload_per_host_path_others_are_still_terminal_rows() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");

        // Two succeeded uploads to the same host_path: both are terminal
        // rows, but only the newer one is the "staged" one for that path.
        let older_upload = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        backdate(&storage, &older_upload.id, "2020-01-01T00:00:00.000Z");
        succeed_upload(&storage, &older_upload.id);
        let newer_upload = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-2", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        succeed_upload(&storage, &newer_upload.id);

        let rows = storage
            .read(|connection| Ok(snapshot(connection, Some("prn-a"))))
            .expect("read")
            .expect("snapshot");

        // Both rows are present (older_upload as an "other terminal" row),
        // but the newer one is never duplicated between the two legs.
        let occurrences = |id: &str| rows.iter().filter(|row| row.id == id).count();
        assert_eq!(occurrences(&older_upload.id), 1);
        assert_eq!(occurrences(&newer_upload.id), 1);
    }

    #[test]
    fn snapshot_includes_every_unresolved_row_and_caps_other_terminal_rows_at_20() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");

        // 25 other terminal rows (failed uploads to distinct host_paths, so
        // none of them is "staged"), backdated to a strict, oldest-first
        // order: op-terminal-0 is the oldest, op-terminal-24 the newest.
        // Built before the unresolved row below, since only one unresolved
        // row is allowed per Printer at a time — each of these passes
        // through `dispatching` and must be resolved before the next one
        // is inserted.
        let mut terminal_ids = Vec::new();
        for index in 0..25 {
            let operation_id = format!("op-terminal-{index}");
            let host_path = format!("farm3d/terminal-{index}.gcode");
            let operation = storage
                .write_repo(|tx| {
                    insert_dispatching(tx, &upload(&operation_id, "prn-a", &host_path))
                })
                .expect("insert");
            backdate(
                &storage,
                &operation.id,
                &format!("2020-01-01T00:00:{index:02}.000Z"),
            );
            storage
                .write_repo(|tx| {
                    transition(
                        tx,
                        &operation.id,
                        Outcome::Failed {
                            failure: HostOperationFailure {
                                code: HostOperationFailureCode::HostUnreachable,
                                message: "unreachable".to_string(),
                            },
                        },
                    )
                })
                .expect("fail");
            terminal_ids.push(operation.id);
        }

        // One unresolved row: always included, regardless of the cap.
        let unresolved = storage
            .write_repo(|tx| {
                insert_dispatching(
                    tx,
                    &upload("op-unresolved", "prn-a", "farm3d/unresolved.gcode"),
                )
            })
            .expect("insert");

        let rows = storage
            .read(|connection| Ok(snapshot(connection, Some("prn-a"))))
            .expect("read")
            .expect("snapshot");
        let ids: Vec<&str> = rows.iter().map(|row| row.id.as_str()).collect();

        assert!(ids.contains(&unresolved.id.as_str()));
        let newest_20 = &terminal_ids[5..];
        let oldest_5 = &terminal_ids[..5];
        for id in newest_20 {
            assert!(
                ids.contains(&id.as_str()),
                "{id} is one of the newest 20 and must be kept"
            );
        }
        for id in oldest_5 {
            assert!(
                !ids.contains(&id.as_str()),
                "{id} falls outside the 20-row cap"
            );
        }
        // unresolved + 20 capped terminal rows.
        assert_eq!(rows.len(), 21);
    }

    #[test]
    fn has_unresolved_reflects_the_partial_index() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        assert!(!storage
            .read(|connection| Ok(has_unresolved(connection, "prn-a")))
            .expect("read")
            .expect("bool"));

        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        assert!(storage
            .read(|connection| Ok(has_unresolved(connection, "prn-a")))
            .expect("read")
            .expect("bool"));

        storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &operation.id,
                    Outcome::Failed {
                        failure: HostOperationFailure {
                            code: HostOperationFailureCode::HostUnreachable,
                            message: "unreachable".to_string(),
                        },
                    },
                )
            })
            .expect("fail");
        assert!(!storage
            .read(|connection| Ok(has_unresolved(connection, "prn-a")))
            .expect("read")
            .expect("bool"));
    }

    #[test]
    fn delete_terminal_for_printer_removes_only_terminal_rows() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let terminal = storage
            .write_repo(|tx| insert_dispatching(tx, &upload("op-1", "prn-a", "farm3d/a.gcode")))
            .expect("insert");
        storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &terminal.id,
                    Outcome::Failed {
                        failure: HostOperationFailure {
                            code: HostOperationFailureCode::HostUnreachable,
                            message: "unreachable".to_string(),
                        },
                    },
                )
            })
            .expect("fail");

        let deleted = storage
            .write_repo(|tx| delete_terminal_for_printer(tx, "prn-a"))
            .expect("delete");
        assert_eq!(deleted, 1);
        assert!(storage
            .read(|connection| Ok(load(connection, &terminal.id)))
            .expect("read")
            .expect("load")
            .is_none());
    }

    /// D11's `startObserved`/`stateObserved` shapes round-trip through the
    /// repository's JSON column, not only through serde directly.
    #[test]
    fn start_and_state_observed_resolutions_round_trip_through_the_row() {
        let (_temp, _lease, storage) = crate::test_storage();
        seed_printer_arc(&storage, "prn-a");
        let start = NewHostOperation {
            operation_id: "op-start".to_string(),
            operation_kind: OperationKind::StartStagedArtifact,
            request_digest: "digest".to_string(),
            printer_id: "prn-a".to_string(),
            kind: HostOperationKind::Start,
            slice_revision_id: None,
            source_host_operation_id: None,
            gcode_sha256: Some("c".repeat(64)),
            gcode_size: Some(10),
            host_path: "farm3d/a.gcode".to_string(),
            history_mark: Some(0),
            endpoint: endpoint(),
        };
        let operation = storage
            .write_repo(|tx| insert_dispatching(tx, &start))
            .expect("insert");

        let succeeded = storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &operation.id,
                    Outcome::Succeeded {
                        resolution: HostOperationResolution::StartObserved {
                            source: StartEvidenceSource::History,
                            history_job_id: Some("0000A1".to_string()),
                            interrupted: false,
                        },
                    },
                )
            })
            .expect("succeed");

        let reloaded = load_row(&storage, &succeeded.id);
        assert_eq!(
            reloaded.resolution,
            Some(HostOperationResolution::StartObserved {
                source: StartEvidenceSource::History,
                history_job_id: Some("0000A1".to_string()),
                interrupted: false,
            })
        );

        // Sanity for stateObserved's shape too (pause/resume/cancel).
        let pause = NewHostOperation {
            operation_id: "op-pause".to_string(),
            operation_kind: OperationKind::PauseHostPrint,
            request_digest: "digest-pause".to_string(),
            printer_id: "prn-a".to_string(),
            kind: HostOperationKind::Pause,
            slice_revision_id: None,
            source_host_operation_id: None,
            gcode_sha256: None,
            gcode_size: None,
            host_path: "farm3d/a.gcode".to_string(),
            history_mark: None,
            endpoint: endpoint(),
        };
        let pause_operation = storage
            .write_repo(|tx| insert_dispatching(tx, &pause))
            .expect("insert");
        let pause_succeeded = storage
            .write_repo(|tx| {
                transition(
                    tx,
                    &pause_operation.id,
                    Outcome::Succeeded {
                        resolution: HostOperationResolution::StateObserved {
                            observed_state: HostOperationObservedState::Paused,
                            reconciled: false,
                        },
                    },
                )
            })
            .expect("succeed");
        assert_eq!(
            pause_succeeded.resolution,
            Some(HostOperationResolution::StateObserved {
                observed_state: HostOperationObservedState::Paused,
                reconciled: false,
            })
        );
    }
}
