//! P7 D2/D4: the SQL for `queue_entries` — creation with lineage, order,
//! update, the D2 renumbering algorithm, and reads. Every function takes
//! the caller's `&Transaction`/`&Connection` (P3+'s pattern; see
//! `host_ops::repository`'s module doc), so a later task's command
//! composes several of these into one atomic commit through
//! `Storage::write_repo`.
//!
//! [`create_entries`] is Add to Queue's N-row insert (one lineage).
//! [`create_linked`] inserts a retry or release replacement that joins an
//! existing lineage. [`move_entry`] and [`update_entry`] are the two
//! operator edits, each guarded by `expectedRevision`. [`apply`] runs a D2
//! event through the pure `queue::state::transition` before any SQL, then
//! writes the entry and — for a close — renumbers every later open entry
//! in the same call. [`load`], [`list_open`], and [`list_history`] are
//! reads.
//!
//! Every renumber (move, close/remove, and a release's replacement) is
//! spec D2's four-step parking algorithm: park the mover (or free the
//! closed row's slot), park the shift range at `+1_000_000`, write its
//! final values in one statement, then (for a move) write the mover's
//! final position. This avoids the CHECK `position >= 1` and the partial
//! UNIQUE index ever seeing a collision, regardless of row order.

use rusqlite::types::Type;
use rusqlite::{params, Connection, OptionalExtension, Transaction};

use crate::library;
use crate::library::repository as library_repository;
use crate::persistence::{RepositoryError, StorageError};
use crate::slicing::repository as slicing_repository;
use crate::spools::{decode_enum, encode_enum};

use super::state::{self, EntryEvent};
use super::{
    allowed_actions, CloseReason, DispatchPolicy, DispatchPreference, EstimateSource,
    MaterialEstimate, OriginKind, QueueEntry, QueueEntryAction, QueueEntryDisplay, QueueEntryState,
};

const QUEUE_ENTRY_ID_PREFIX: &str = "qen";
const LINEAGE_ID_PREFIX: &str = "qln";

fn new_entry_id() -> String {
    library::new_id(QUEUE_ENTRY_ID_PREFIX)
}

fn new_lineage_id() -> String {
    library::new_id(LINEAGE_ID_PREFIX)
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

/// The raw `queue_entries` row, before the display join and the lineage's
/// `copyCount` lookup ([`hydrate`]).
struct EntryRow {
    id: String,
    revision: i64,
    slice_revision_id: String,
    lineage_id: String,
    copy_index: i64,
    origin_entry_id: Option<String>,
    origin_kind: Option<OriginKind>,
    state: QueueEntryState,
    close_reason: Option<CloseReason>,
    position: Option<i64>,
    policy: DispatchPolicy,
    preference: DispatchPreference,
    estimate_mg: i64,
    estimate_source: EstimateSource,
    manual_printer_id: Option<String>,
    job_id: Option<String>,
    created_at: String,
    updated_at: String,
    closed_at: Option<String>,
}

const ENTRY_COLUMNS: &str = "id, revision, slice_revision_id, lineage_id, copy_index, \
     origin_entry_id, origin_kind, state, close_reason, position, policy, preference, \
     estimate_mg, estimate_source, manual_printer_id, job_id, created_at, updated_at, closed_at";

fn decode_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<EntryRow> {
    let origin_kind: Option<String> = row.get(6)?;
    let state_text: String = row.get(7)?;
    let close_reason: Option<String> = row.get(8)?;
    let policy_text: String = row.get(10)?;
    let preference_text: String = row.get(11)?;
    let estimate_source_text: String = row.get(13)?;
    Ok(EntryRow {
        id: row.get(0)?,
        revision: row.get(1)?,
        slice_revision_id: row.get(2)?,
        lineage_id: row.get(3)?,
        copy_index: row.get(4)?,
        origin_entry_id: row.get(5)?,
        origin_kind: origin_kind
            .map(|text| decode_text_enum(6, &text))
            .transpose()?,
        state: decode_text_enum(7, &state_text)?,
        close_reason: close_reason
            .map(|text| decode_text_enum(8, &text))
            .transpose()?,
        position: row.get(9)?,
        policy: decode_text_enum(10, &policy_text)?,
        preference: decode_text_enum(11, &preference_text)?,
        estimate_mg: row.get(12)?,
        estimate_source: decode_text_enum(13, &estimate_source_text)?,
        manual_printer_id: row.get(14)?,
        job_id: row.get(15)?,
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
        closed_at: row.get(18)?,
    })
}

/// [`load_display`]'s result: the display projection plus whether the
/// linked Slice Revision has any absent fact (D5's `NEEDS_MANUAL_PRINTER`
/// gate reads this through `QueueEntry.requiresManualPrinterSelection`).
struct DisplayJoin {
    display: QueueEntryDisplay,
    requires_manual_printer_selection: bool,
}

/// Joins in the entry's linked Slice Revision (for its display fields and
/// `requiresManualPrinterSelection`) and Model (for its name). Both are
/// `ON DELETE RESTRICT` from `queue_entries`, so a missing one is data
/// corruption, not a normal "not found" — reported as
/// [`StorageError::CorruptData`].
fn load_display(conn: &Connection, slice_revision_id: &str) -> Result<DisplayJoin, StorageError> {
    let record = slicing_repository::load_revision(conn, slice_revision_id)?.ok_or(
        StorageError::CorruptData {
            source_name: "queue_entries.slice_revision_id",
            source_sha256: None,
        },
    )?;
    let summary = record.summary;
    let model = library_repository::load_model(conn, &summary.model_id)?.ok_or(
        StorageError::CorruptData {
            source_name: "slice_revisions.model_id",
            source_sha256: None,
        },
    )?;
    let display = QueueEntryDisplay {
        model_id: summary.model_id,
        model_name: model.name,
        plate_label: summary
            .plate
            .as_ref()
            .and_then(|plate| plate.plate_name.clone()),
        target_label: summary.target_label,
        material_family: summary.facts.material_family.value().copied(),
        material_other: summary.facts.material_other,
        print_seconds: summary.estimates.as_ref().and_then(|e| e.print_seconds),
    };
    Ok(DisplayJoin {
        display,
        requires_manual_printer_selection: summary.requires_manual_printer_selection,
    })
}

/// Assembles the wire [`QueueEntry`] from a raw row: the lineage's
/// `copyCount`, the display join, and the state-derived `allowedActions`.
fn hydrate(conn: &Connection, row: EntryRow) -> Result<QueueEntry, StorageError> {
    let copy_count: i64 = conn.query_row(
        "SELECT MAX(copy_index) FROM queue_entries WHERE lineage_id = ?1",
        [&row.lineage_id],
        |r| r.get(0),
    )?;
    let joined = load_display(conn, &row.slice_revision_id)?;
    Ok(QueueEntry {
        id: row.id,
        revision: row.revision,
        slice_revision_id: row.slice_revision_id,
        lineage_id: row.lineage_id,
        copy_index: row.copy_index,
        copy_count,
        origin_entry_id: row.origin_entry_id,
        origin_kind: row.origin_kind,
        state: row.state,
        close_reason: row.close_reason,
        position: row.position,
        policy: row.policy,
        preference: row.preference,
        estimate: MaterialEstimate {
            amount_mg: row.estimate_mg,
            source: row.estimate_source,
        },
        manual_printer_id: row.manual_printer_id,
        job_id: row.job_id,
        requires_manual_printer_selection: joined.requires_manual_printer_selection,
        allowed_actions: allowed_actions(row.state),
        display: joined.display,
        created_at: row.created_at,
        updated_at: row.updated_at,
        closed_at: row.closed_at,
    })
}

pub fn load(conn: &Connection, id: &str) -> Result<Option<QueueEntry>, StorageError> {
    let row: Option<EntryRow> = conn
        .query_row(
            &format!("SELECT {ENTRY_COLUMNS} FROM queue_entries WHERE id = ?1"),
            [id],
            decode_row,
        )
        .optional()?;
    row.map(|row| hydrate(conn, row)).transpose()
}

/// Every open (`queued`/`assigned`) entry, in position order.
pub fn list_open(conn: &Connection) -> Result<Vec<QueueEntry>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM queue_entries WHERE position IS NOT NULL ORDER BY position"
    ))?;
    let rows = statement
        .query_map([], decode_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(|row| hydrate(conn, row)).collect()
}

/// The newest `limit` closed entries.
pub fn list_history(conn: &Connection, limit: u32) -> Result<Vec<QueueEntry>, StorageError> {
    let mut statement = conn.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM queue_entries WHERE state = 'closed'
         ORDER BY closed_at DESC, id DESC LIMIT ?1"
    ))?;
    let rows = statement
        .query_map(params![limit], decode_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter().map(|row| hydrate(conn, row)).collect()
}

fn list_positions(
    tx: &Transaction<'_>,
    lo: i64,
    hi: i64,
) -> Result<Vec<QueueEntry>, RepositoryError> {
    let mut statement = tx.prepare(&format!(
        "SELECT {ENTRY_COLUMNS} FROM queue_entries WHERE position BETWEEN ?1 AND ?2 ORDER BY position"
    ))?;
    let rows = statement
        .query_map(params![lo, hi], decode_row)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    let entries: Result<Vec<QueueEntry>, StorageError> =
        rows.into_iter().map(|row| hydrate(tx, row)).collect();
    Ok(entries?)
}

fn current_max_position(tx: &Transaction<'_>) -> Result<i64, RepositoryError> {
    let max: Option<i64> =
        tx.query_row("SELECT MAX(position) FROM queue_entries", [], |r| r.get(0))?;
    Ok(max.unwrap_or(0))
}

/// D2's shift step (2 and 3): parks `[lo, hi]` at `+1_000_000`, then writes
/// its final value (`position - 1_000_000 + delta`) in one statement. A
/// no-op when `lo > hi` (nothing to shift). `bump_revision` is only ever
/// `true` for a move (spec D4: "the renumbered rows, each revision + 1");
/// a close's renumber leaves the shifted rows' revision untouched.
fn shift_range(
    tx: &Transaction<'_>,
    lo: i64,
    hi: i64,
    delta: i64,
    bump_revision: bool,
    now: &str,
) -> Result<(), RepositoryError> {
    if lo > hi {
        return Ok(());
    }
    tx.execute(
        "UPDATE queue_entries SET position = position + 1000000 WHERE position BETWEEN ?1 AND ?2",
        params![lo, hi],
    )?;
    if bump_revision {
        tx.execute(
            "UPDATE queue_entries
             SET position = position - 1000000 + ?1, revision = revision + 1, updated_at = ?2
             WHERE position BETWEEN 1000001 AND 1999999",
            params![delta, now],
        )?;
    } else {
        tx.execute(
            "UPDATE queue_entries SET position = position - 1000000 + ?1
             WHERE position BETWEEN 1000001 AND 1999999",
            params![delta],
        )?;
    }
    Ok(())
}

/// Closes `id` (position -> NULL, `close_reason`, `closed_at`), returning
/// the position it freed. No other row is touched — [`close_and_renumber`]
/// is the same close plus the shift; [`apply`]'s `Release` branch calls
/// this alone (D2: "nothing else moves" — the freed slot is filled by the
/// replacement `create_linked` inserts next, in the same transaction).
fn close_row(
    tx: &Transaction<'_>,
    id: &str,
    close_reason: CloseReason,
    now: &str,
) -> Result<i64, RepositoryError> {
    let freed: i64 = tx.query_row(
        "SELECT position FROM queue_entries WHERE id = ?1",
        [id],
        |r| r.get(0),
    )?;
    tx.execute(
        "UPDATE queue_entries
         SET state = 'closed', close_reason = ?2, position = NULL, closed_at = ?3, updated_at = ?3
         WHERE id = ?1",
        params![id, encode_enum(close_reason), now],
    )?;
    Ok(freed)
}

/// [`close_row`] plus D2's shift: every later open entry moves down by
/// one, in D2's parking order. The closed row itself is outside the shift
/// range (its position is already NULL), so this never bumps its
/// revision — only `move_entry` does that for the rows it renumbers.
/// `Remove` and a Job reaching a terminal state both close this way;
/// `Release` does not (see [`close_row`]).
fn close_and_renumber(
    tx: &Transaction<'_>,
    id: &str,
    close_reason: CloseReason,
    now: &str,
) -> Result<(), RepositoryError> {
    let freed = close_row(tx, id, close_reason, now)?;
    let max_position: Option<i64> =
        tx.query_row("SELECT MAX(position) FROM queue_entries", [], |r| r.get(0))?;
    if let Some(max) = max_position {
        if freed < max {
            shift_range(tx, freed + 1, max, -1, false, now)?;
        }
    }
    Ok(())
}

/// Add to Queue's N-row insert (spec D1/D4): one new `lineage_id`,
/// `copyIndex` 1..N, appended at the end of the open list in copy-index
/// order.
pub struct NewEntries {
    /// The lineage id to use, or `None` for a fresh `qln-<uuid v4>`.
    /// `add_to_queue` derives it from its `operationId` so a replay can
    /// find the entries it created (the pattern P5's external revisions
    /// use for their own ids).
    pub lineage_id: Option<String>,
    pub slice_revision_id: String,
    pub quantity: u8,
    pub policy: DispatchPolicy,
    pub preference: DispatchPreference,
    pub estimate: MaterialEstimate,
    pub manual_printer_id: Option<String>,
}

pub fn create_entries(
    tx: &Transaction<'_>,
    new: &NewEntries,
    now: &str,
) -> Result<Vec<QueueEntry>, RepositoryError> {
    let lineage_id = new.lineage_id.clone().unwrap_or_else(new_lineage_id);
    let base_position = current_max_position(tx)?;
    let mut ids = Vec::with_capacity(new.quantity as usize);
    for copy_index in 1..=i64::from(new.quantity) {
        let id = new_entry_id();
        let position = base_position + copy_index;
        tx.execute(
            "INSERT INTO queue_entries(
                 id, revision, slice_revision_id, lineage_id, copy_index, state, position,
                 policy, preference, estimate_mg, estimate_source, manual_printer_id,
                 created_at, updated_at
             ) VALUES (?1, 1, ?2, ?3, ?4, 'queued', ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?11)",
            params![
                id,
                new.slice_revision_id,
                lineage_id,
                copy_index,
                position,
                encode_enum(new.policy),
                encode_enum(new.preference),
                new.estimate.amount_mg,
                encode_enum(new.estimate.source),
                new.manual_printer_id,
                now,
            ],
        )?;
        ids.push(id);
    }
    // Loaded only after every row of the batch is inserted, so each one's
    // `copyCount` (MAX(copy_index) over the whole lineage) already sees
    // every copy — not just the ones inserted so far.
    ids.iter()
        .map(|id| load(tx, id)?.ok_or_else(|| not_found(id)))
        .collect()
}

/// A retry or release replacement: joins `origin`'s lineage, keeps its
/// `copyIndex`, and records `origin_entry_id`/`origin_kind`. `position`
/// is the freed slot for a release (nothing else moves — the caller
/// closes the origin first); `None` appends at the end for a retry.
pub fn create_linked(
    tx: &Transaction<'_>,
    origin: &QueueEntry,
    kind: OriginKind,
    position: Option<i64>,
    now: &str,
) -> Result<QueueEntry, RepositoryError> {
    let id = new_entry_id();
    let position = match position {
        Some(position) => position,
        None => current_max_position(tx)? + 1,
    };
    tx.execute(
        "INSERT INTO queue_entries(
             id, revision, slice_revision_id, lineage_id, copy_index, origin_entry_id, origin_kind,
             state, position, policy, preference, estimate_mg, estimate_source, manual_printer_id,
             created_at, updated_at
         ) VALUES (?1, 1, ?2, ?3, ?4, ?5, ?6, 'queued', ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?13)",
        params![
            id,
            origin.slice_revision_id,
            origin.lineage_id,
            origin.copy_index,
            origin.id,
            encode_enum(kind),
            position,
            encode_enum(origin.policy),
            encode_enum(origin.preference),
            origin.estimate.amount_mg,
            encode_enum(origin.estimate.source),
            origin.manual_printer_id,
            now,
        ],
    )?;
    load(tx, &id)?.ok_or_else(|| not_found(&id))
}

/// `move_queue_entry`'s mechanics (D2/D4): checks `expectedRevision`
/// ([`RepositoryError::Conflict`] on a stale one), then renumbers densely
/// between the old and new position, bumping every touched row's
/// revision. Returns every entry whose position changed (the mover and
/// the shifted range), position order. Refused with
/// [`RepositoryError::QueueEntryActionNotAllowed`] on a closed entry (no
/// position to move from).
pub fn move_entry(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    to_position: i64,
    now: &str,
) -> Result<Vec<QueueEntry>, RepositoryError> {
    let (current_revision, state_text, position): (i64, String, Option<i64>) = tx
        .query_row(
            "SELECT revision, state, position FROM queue_entries WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)),
        )
        .optional()?
        .ok_or_else(|| not_found(id))?;
    if current_revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision,
        });
    }
    let state: QueueEntryState = decode_enum(&state_text).map_err(|_| not_found(id))?;
    let Some(old_position) = position else {
        return Err(RepositoryError::QueueEntryActionNotAllowed {
            entry_id: id.to_string(),
            action: QueueEntryAction::Move,
            state,
        });
    };

    if old_position != to_position {
        tx.execute(
            "UPDATE queue_entries SET position = position + 2000000 WHERE id = ?1",
            [id],
        )?;
        let (lo, hi, delta) = if to_position > old_position {
            (old_position + 1, to_position, -1)
        } else {
            (to_position, old_position - 1, 1)
        };
        shift_range(tx, lo, hi, delta, true, now)?;
        tx.execute(
            "UPDATE queue_entries SET position = ?2, revision = revision + 1, updated_at = ?3
             WHERE id = ?1",
            params![id, to_position, now],
        )?;
    }

    let lo_all = old_position.min(to_position);
    let hi_all = old_position.max(to_position);
    list_positions(tx, lo_all, hi_all)
}

/// `update_queue_entry`'s mechanics (D2/D4): only legal on a `queued`
/// entry ([`RepositoryError::QueueEntryActionNotAllowed`] otherwise),
/// guarded by `expectedRevision`. Never changes state or position.
pub fn update_entry(
    tx: &Transaction<'_>,
    id: &str,
    expected_revision: i64,
    policy: Option<DispatchPolicy>,
    preference: Option<DispatchPreference>,
    now: &str,
) -> Result<QueueEntry, RepositoryError> {
    let (current_revision, state_text): (i64, String) = tx
        .query_row(
            "SELECT revision, state FROM queue_entries WHERE id = ?1",
            [id],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
        .optional()?
        .ok_or_else(|| not_found(id))?;
    if current_revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision,
        });
    }
    let state: QueueEntryState = decode_enum(&state_text).map_err(|_| not_found(id))?;
    if state != QueueEntryState::Queued {
        return Err(RepositoryError::QueueEntryActionNotAllowed {
            entry_id: id.to_string(),
            action: QueueEntryAction::Update,
            state,
        });
    }
    if let Some(policy) = policy {
        tx.execute(
            "UPDATE queue_entries SET policy = ?2 WHERE id = ?1",
            params![id, encode_enum(policy)],
        )?;
    }
    if let Some(preference) = preference {
        tx.execute(
            "UPDATE queue_entries SET preference = ?2 WHERE id = ?1",
            params![id, encode_enum(preference)],
        )?;
    }
    tx.execute(
        "UPDATE queue_entries SET revision = revision + 1, updated_at = ?2 WHERE id = ?1",
        params![id, now],
    )?;
    load(tx, id)?.ok_or_else(|| not_found(id))
}

/// Runs `event` through the pure `queue::state::transition` before any
/// SQL ([`RepositoryError::IllegalQueueEntryTransition`] on a rejection,
/// nothing written), then writes the entry: `Assign` sets `job_id` and
/// keeps the position (assignment is sticky); `Remove` and `JobTerminal`
/// close the row and renumber every later open entry in the same call
/// ([`close_and_renumber`]); `Release` closes the row with **no** shift
/// ([`close_row`] alone) — D2: "nothing else moves", since the caller
/// inserts the replacement at the freed slot next, in the same
/// transaction.
pub fn apply(
    tx: &Transaction<'_>,
    id: &str,
    event: &EntryEvent,
    job_id: Option<&str>,
    now: &str,
) -> Result<QueueEntry, RepositoryError> {
    let current = load(tx, id)?.ok_or_else(|| not_found(id))?;
    let to = state::transition(current.state, event).map_err(|error| {
        RepositoryError::IllegalQueueEntryTransition {
            entry_id: id.to_string(),
            from: error.from,
            event: error.event,
        }
    })?;

    match to {
        QueueEntryState::Assigned => {
            let job_id = job_id.expect("D2: Assign always carries a job id");
            tx.execute(
                "UPDATE queue_entries SET state = 'assigned', job_id = ?2, revision = revision + 1,
                     updated_at = ?3
                 WHERE id = ?1",
                params![id, job_id, now],
            )?;
        }
        QueueEntryState::Closed => match event {
            // D2: a release closes with no shift — the freed slot is
            // filled by the replacement `create_linked` inserts next, in
            // the same transaction.
            EntryEvent::Release => {
                close_row(tx, id, CloseReason::Released, now)?;
            }
            EntryEvent::Remove => {
                close_and_renumber(tx, id, CloseReason::Removed, now)?;
            }
            EntryEvent::JobTerminal(reason) => {
                close_and_renumber(tx, id, *reason, now)?;
            }
            EntryEvent::Assign => unreachable!("D2: Assign never targets closed"),
        },
        QueueEntryState::Queued => unreachable!("D2: no event targets queued"),
    }

    load(tx, id)?.ok_or_else(|| not_found(id))
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::persistence::{MetadataRootLease, Storage};
    use crate::slicing::repository::fixtures::{a_farm3d_revision, seed};
    use crate::slicing::repository::insert_farm3d_revision;

    const NOW: &str = "2026-02-01T00:00:00.000Z";
    const SLR: &str = "slr-a";

    fn storage_with_revision() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>) {
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
        (temp, lease, storage)
    }

    fn three() -> NewEntries {
        NewEntries {
            lineage_id: None,
            slice_revision_id: SLR.to_string(),
            quantity: 3,
            policy: DispatchPolicy::Manual,
            preference: DispatchPreference::LoadedFirst,
            estimate: MaterialEstimate {
                amount_mg: 12_500,
                source: EstimateSource::SliceEstimate,
            },
            manual_printer_id: None,
        }
    }

    #[test]
    fn create_three_copies_share_lineage_and_take_contiguous_positions_at_the_end() {
        let (_temp, _lease, storage) = storage_with_revision();

        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");

        assert_eq!(created.len(), 3);
        let lineage_id = created[0].lineage_id.clone();
        for (index, entry) in created.iter().enumerate() {
            assert_eq!(
                entry.lineage_id, lineage_id,
                "every copy shares one lineage"
            );
            assert_eq!(entry.copy_index, index as i64 + 1);
            assert_eq!(entry.copy_count, 3);
            assert_eq!(entry.position, Some(index as i64 + 1));
            assert_eq!(entry.state, QueueEntryState::Queued);
            assert_eq!(entry.revision, 1);
            assert_eq!(entry.display.model_name, "Bracket");
        }
    }

    #[test]
    fn each_copy_is_independently_removable_and_renumbers_the_rest() {
        let (_temp, _lease, storage) = storage_with_revision();
        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");
        let middle = &created[1];

        let closed = storage
            .write_repo(|tx| apply(tx, &middle.id, &EntryEvent::Remove, None, NOW))
            .expect("remove");

        assert_eq!(closed.state, QueueEntryState::Closed);
        assert_eq!(closed.close_reason, Some(CloseReason::Removed));
        assert_eq!(closed.position, None);

        let open = storage
            .read(|conn| Ok(list_open(conn)))
            .expect("read")
            .expect("list");
        assert_eq!(open.len(), 2);
        assert_eq!(open[0].id, created[0].id);
        assert_eq!(open[0].position, Some(1));
        assert_eq!(open[1].id, created[2].id);
        assert_eq!(
            open[1].position,
            Some(2),
            "the entry after the removed one shifts up to fill the gap"
        );
    }

    #[test]
    fn move_entry_renumbers_densely_and_bumps_revision() {
        let (_temp, _lease, storage) = storage_with_revision();
        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");
        let first = created[0].clone();

        let renumbered = storage
            .write_repo(|tx| move_entry(tx, &first.id, first.revision, 3, NOW))
            .expect("move");

        assert_eq!(renumbered.len(), 3, "the mover and both shifted siblings");
        let open = storage
            .read(|conn| Ok(list_open(conn)))
            .expect("read")
            .expect("list");
        assert_eq!(
            open.iter()
                .map(|entry| entry.id.clone())
                .collect::<Vec<_>>(),
            vec![
                created[1].id.clone(),
                created[2].id.clone(),
                created[0].id.clone()
            ]
        );
        let mover = open.iter().find(|entry| entry.id == first.id).unwrap();
        assert_eq!(mover.position, Some(3));
        assert_eq!(mover.revision, first.revision + 1);
        for sibling in [&created[1], &created[2]] {
            let row = open.iter().find(|entry| entry.id == sibling.id).unwrap();
            assert_eq!(
                row.revision,
                sibling.revision + 1,
                "a shifted sibling also bumps revision (D4: move renumbers each +1)"
            );
        }
    }

    #[test]
    fn move_entry_rejects_stale_revision_with_revision_conflict() {
        let (_temp, _lease, storage) = storage_with_revision();
        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");
        let first = &created[0];

        let result = storage.write_repo(|tx| move_entry(tx, &first.id, first.revision + 1, 2, NOW));

        match result {
            Err(RepositoryError::Conflict {
                expected_revision,
                current_revision,
                ..
            }) => {
                assert_eq!(expected_revision, first.revision + 1);
                assert_eq!(current_revision, first.revision);
            }
            other => panic!("expected Conflict, got {other:?}"),
        }
        // nothing was written: the entry's position is unchanged.
        let reloaded = storage
            .read(|conn| Ok(load(conn, &first.id)))
            .expect("read")
            .expect("load")
            .expect("exists");
        assert_eq!(reloaded.position, first.position);
        assert_eq!(reloaded.revision, first.revision);
    }

    #[test]
    fn renumber_paths_never_violate_the_unique_position_index() {
        let (_temp, _lease, storage) = storage_with_revision();
        let new = NewEntries {
            quantity: 6,
            ..three()
        };
        let created = storage
            .write_repo(|tx| create_entries(tx, &new, NOW))
            .expect("create");
        let ids: Vec<String> = created.iter().map(|entry| entry.id.clone()).collect();

        // Move down the list: the mover parks above the shift range, the
        // shift range parks at +1_000_000, then both write their final
        // values — never colliding with the CHECK/partial-UNIQUE index.
        storage
            .write_repo(|tx| move_entry(tx, &ids[0], 1, 4, NOW))
            .expect("move down the list");
        // Move up the list.
        storage
            .write_repo(|tx| move_entry(tx, &ids[5], 1, 2, NOW))
            .expect("move up the list");
        // Close (remove) an entry from the middle of the list.
        storage
            .write_repo(|tx| apply(tx, &ids[2], &EntryEvent::Remove, None, NOW))
            .expect("remove");
        // Release: close (the D2 no-shift path — `close_row` alone, not
        // `close_and_renumber`), then insert a replacement at the freed
        // slot. `close_row` is exercised directly here (D2's `Release`
        // event is only legal from `assigned`, which needs a real Job —
        // out of this task's scope; `apply`'s dispatch to `close_row` for
        // `Release` is covered structurally, and its SQL mechanics here).
        let origin = storage
            .read(|conn| Ok(load(conn, &ids[3])))
            .expect("read")
            .expect("load")
            .expect("exists");
        let freed_position = origin.position.expect("open");
        storage
            .write_repo(|tx| {
                close_row(tx, &ids[3], CloseReason::Released, NOW)?;
                create_linked(tx, &origin, OriginKind::Release, Some(freed_position), NOW)
            })
            .expect("release");

        // Every open entry's position is dense (1..n, no gaps, no
        // duplicates) — the partial UNIQUE index never rejected a
        // statement above, and this confirms the final layout too.
        let open = storage
            .read(|conn| Ok(list_open(conn)))
            .expect("read")
            .expect("list");
        let mut positions: Vec<i64> = open
            .iter()
            .map(|entry| entry.position.expect("open entries always have a position"))
            .collect();
        positions.sort_unstable();
        let expected: Vec<i64> = (1..=positions.len() as i64).collect();
        assert_eq!(positions, expected);
    }

    #[test]
    fn closed_entries_have_no_position_and_keep_lineage() {
        let (_temp, _lease, storage) = storage_with_revision();
        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");
        let target = &created[0];

        let closed = storage
            .write_repo(|tx| apply(tx, &target.id, &EntryEvent::Remove, None, NOW))
            .expect("remove");

        assert_eq!(closed.position, None);
        assert_eq!(closed.lineage_id, target.lineage_id);
        assert_eq!(closed.copy_index, target.copy_index);
        assert_eq!(closed.copy_count, 3);
        assert!(closed.closed_at.is_some());

        let history = storage
            .read(|conn| Ok(list_history(conn, 10)))
            .expect("read")
            .expect("list");
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].id, closed.id);
    }

    #[test]
    fn create_linked_release_takes_the_released_position_and_shifts_nothing_else() {
        let (_temp, _lease, storage) = storage_with_revision();
        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");
        let origin = created[1].clone();
        let freed_position = origin.position.expect("open");

        let replacement = storage
            .write_repo(|tx| {
                close_row(tx, &origin.id, CloseReason::Released, NOW)?;
                create_linked(tx, &origin, OriginKind::Release, Some(freed_position), NOW)
            })
            .expect("release + replacement");

        assert_eq!(replacement.position, Some(freed_position));
        assert_eq!(replacement.lineage_id, origin.lineage_id);
        assert_eq!(replacement.copy_index, origin.copy_index);
        assert_eq!(replacement.origin_entry_id, Some(origin.id.clone()));
        assert_eq!(replacement.origin_kind, Some(OriginKind::Release));
        assert_eq!(replacement.state, QueueEntryState::Queued);

        let open = storage
            .read(|conn| Ok(list_open(conn)))
            .expect("read")
            .expect("list");
        let position_of = |id: &str| open.iter().find(|entry| entry.id == id).unwrap().position;
        assert_eq!(position_of(&created[0].id), Some(1), "unmoved");
        assert_eq!(position_of(&replacement.id), Some(2), "took the freed slot");
        assert_eq!(position_of(&created[2].id), Some(3), "unmoved");
    }

    #[test]
    fn create_linked_retry_appends_at_the_end() {
        let (_temp, _lease, storage) = storage_with_revision();
        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");
        let origin = created[0].clone();

        let retry = storage
            .write_repo(|tx| create_linked(tx, &origin, OriginKind::Retry, None, NOW))
            .expect("retry");

        assert_eq!(retry.position, Some(4));
        assert_eq!(retry.lineage_id, origin.lineage_id);
        assert_eq!(retry.copy_index, origin.copy_index);
        assert_eq!(retry.origin_entry_id, Some(origin.id.clone()));
        assert_eq!(retry.origin_kind, Some(OriginKind::Retry));
        assert_eq!(retry.state, QueueEntryState::Queued);
    }

    #[test]
    fn update_entry_changes_policy_and_bumps_revision() {
        let (_temp, _lease, storage) = storage_with_revision();
        let created = storage
            .write_repo(|tx| create_entries(tx, &three(), NOW))
            .expect("create");
        let target = &created[0];

        let updated = storage
            .write_repo(|tx| {
                update_entry(
                    tx,
                    &target.id,
                    target.revision,
                    Some(DispatchPolicy::Automatic),
                    None,
                    NOW,
                )
            })
            .expect("update");

        assert_eq!(updated.policy, DispatchPolicy::Automatic);
        assert_eq!(updated.preference, target.preference);
        assert_eq!(updated.revision, target.revision + 1);
    }
}
