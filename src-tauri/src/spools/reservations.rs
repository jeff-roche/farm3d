//! D8: the reservation primitives and the availability signal.
//! Reservations belong to P7, but P3 owns the arithmetic and the
//! `spool_reservations` table (global constraint: "Reservations are touched
//! only through `spools::reservations` functions that take `&Transaction`.
//! P3 exposes no reservation command").
//!
//! Every function here takes the caller's `&Transaction`, so a later phase
//! (P7) can reserve atomically alongside its own writes (e.g. Job
//! creation), exactly like `spools::movement`/`spools::ledger` already do
//! for P3's own callers.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;

use super::ledger::{self, AmountEvent, AmountEventKind, LedgerSnapshot};
use super::{decode_enum, encode_enum, repository, AmountConfidence, Availability, SpoolLifecycle};

/// D8: `holder_kind`/`holder_id` are opaque to P3. P7 uses
/// `("job", <jobId>)`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[ts(export_to = "domain/ReservationHolder.ts")]
pub struct ReservationHolder {
    pub kind: String,
    pub id: String,
}

/// D8's reservation lifecycle. Stored as the lowercase string the
/// migration's `state` CHECK lists verbatim.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ReservationState.ts")]
pub enum ReservationState {
    Active,
    Unresolved,
    Released,
    Consumed,
}

/// One `spool_reservations` row (D8). Read-only on the wire: `spool_history`
/// lists them, and no command creates one (D8).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Reservation.ts")]
pub struct Reservation {
    pub id: String,
    pub spool_id: String,
    pub holder: ReservationHolder,
    #[ts(type = "number")]
    pub amount_mg: i64,
    pub state: ReservationState,
    pub operation_id: String,
    pub created_at: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub settled_at: Option<String>,
}

/// D8's reservation error set. `Storage` carries through an unexpected
/// database failure (e.g. an injected fault); every other variant is a
/// specific D8 rule.
#[derive(Debug)]
pub enum ReservationError {
    /// `reserve`'s amount exceeds `availableMg`, which the error carries so
    /// the caller can report it without a second read.
    InsufficientAvailable {
        available_mg: i64,
    },
    /// `reserve` on a Spool that isn't `active` (D8: empty or archived).
    SpoolNotReservable {
        lifecycle: SpoolLifecycle,
    },
    /// `release`/`consume`/`mark_unresolved` attempted from a state that
    /// doesn't allow it. `from` is the reservation's actual current state.
    InvalidTransition {
        from: ReservationState,
    },
    /// `reserve` with `amount_mg <= 0` (D8: `amount_mg > 0`), or `consume`
    /// with `used_mg < 0`.
    InvalidAmount,
    /// No reservation (or, for `reserve`, no Spool) with that id.
    NotFound,
    Storage(StorageError),
}

impl From<StorageError> for ReservationError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<rusqlite::Error> for ReservationError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

/// `ledger::append` (used by [`consume`]) and `repository::load_spool`
/// return `RepositoryError`. `Storage` passes through unchanged. Every other
/// variant collapses to `Storage(OperationFailed)`, which loses its detail.
/// [`consume`]'s guards make them unlikely: it rejects a negative `used_mg`
/// and clamps `afterMg` to `0..=currentMg`, so `append` should not reject
/// the amount, and it loads the Spool first, so `append`'s own `NotFound`
/// should not occur. Nothing here enforces that beyond those guards.
impl From<RepositoryError> for ReservationError {
    fn from(error: RepositoryError) -> Self {
        match error {
            RepositoryError::Storage(storage_error) => Self::Storage(storage_error),
            _ => Self::Storage(StorageError::OperationFailed),
        }
    }
}

const RESERVATION_COLUMNS: &str =
    "id, spool_id, holder_kind, holder_id, amount_mg, state, operation_id, created_at, settled_at";

fn decode_reservation(row: &rusqlite::Row<'_>) -> rusqlite::Result<Reservation> {
    let state_text: String = row.get(5)?;
    Ok(Reservation {
        id: row.get(0)?,
        spool_id: row.get(1)?,
        holder: ReservationHolder {
            kind: row.get(2)?,
            id: row.get(3)?,
        },
        amount_mg: row.get(4)?,
        state: decode_enum(&state_text).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                5,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        operation_id: row.get(6)?,
        created_at: row.get(7)?,
        settled_at: row.get(8)?,
    })
}

fn load_reservation(
    tx: &Transaction<'_>,
    reservation_id: &str,
) -> Result<Reservation, ReservationError> {
    tx.query_row(
        &format!("SELECT {RESERVATION_COLUMNS} FROM spool_reservations WHERE id = ?1"),
        [reservation_id],
        decode_reservation,
    )
    .optional()?
    .ok_or(ReservationError::NotFound)
}

/// D8: reserves `amount_mg` against `spool_id`'s current availability.
/// Fails with [`ReservationError::InvalidAmount`] unless `amount_mg > 0`,
/// with [`ReservationError::SpoolNotReservable`] unless the Spool is
/// `active`, and with [`ReservationError::InsufficientAvailable`] when
/// `amount_mg` exceeds `availableMg` (`currentMg` minus every other `active`
/// /`unresolved` reservation on the Spool). Returns the new reservation's
/// id.
pub fn reserve(
    tx: &Transaction<'_>,
    spool_id: &str,
    holder: &ReservationHolder,
    amount_mg: i64,
    operation_id: &str,
) -> Result<String, ReservationError> {
    if amount_mg <= 0 {
        return Err(ReservationError::InvalidAmount);
    }
    let spool = repository::load_spool(tx, spool_id)?.ok_or(ReservationError::NotFound)?;
    if spool.lifecycle != SpoolLifecycle::Active {
        return Err(ReservationError::SpoolNotReservable {
            lifecycle: spool.lifecycle,
        });
    }
    let reserved_mg = repository::reserved_mg(tx, spool_id)?;
    let available_mg = spool.current_mg - reserved_mg;
    if amount_mg > available_mg {
        return Err(ReservationError::InsufficientAvailable { available_mg });
    }

    let id = format!("rsv-{}", uuid::Uuid::new_v4());
    let now = now_rfc3339();
    tx.execute(
        "INSERT INTO spool_reservations(
            id, spool_id, holder_kind, holder_id, amount_mg, state, operation_id, created_at, settled_at
         ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, NULL)",
        params![
            id,
            spool_id,
            holder.kind,
            holder.id,
            amount_mg,
            encode_enum(ReservationState::Active),
            operation_id,
            now,
        ],
    )?;
    Ok(id)
}

/// D8: `active`/`unresolved` -> `released`. Any other current state is
/// [`ReservationError::InvalidTransition`].
pub fn release(tx: &Transaction<'_>, reservation_id: &str) -> Result<(), ReservationError> {
    let reservation = load_reservation(tx, reservation_id)?;
    if !matches!(
        reservation.state,
        ReservationState::Active | ReservationState::Unresolved
    ) {
        return Err(ReservationError::InvalidTransition {
            from: reservation.state,
        });
    }
    tx.execute(
        "UPDATE spool_reservations SET state = ?2, settled_at = ?3 WHERE id = ?1",
        params![
            reservation_id,
            encode_enum(ReservationState::Released),
            now_rfc3339(),
        ],
    )?;
    Ok(())
}

/// D8: `active` -> `unresolved` (P7's deferred reconciliation). The amount
/// stays counted against `availableMg` either way. Any other current state
/// is [`ReservationError::InvalidTransition`].
pub fn mark_unresolved(tx: &Transaction<'_>, reservation_id: &str) -> Result<(), ReservationError> {
    let reservation = load_reservation(tx, reservation_id)?;
    if reservation.state != ReservationState::Active {
        return Err(ReservationError::InvalidTransition {
            from: reservation.state,
        });
    }
    tx.execute(
        "UPDATE spool_reservations SET state = ?2 WHERE id = ?1",
        params![reservation_id, encode_enum(ReservationState::Unresolved)],
    )?;
    Ok(())
}

/// D8: moves the reservation to `consumed` and writes one `consumption`
/// ledger row via `ledger::append` (never touches `spools.current_mg`
/// directly). `afterMg = max(0, currentMg - used_mg)`; when that clamps,
/// the shortfall is recorded in the row's `note` (appended after any
/// caller-supplied `note`). `used_mg` may differ from the reservation's own
/// `amount_mg`, but a negative `used_mg` is
/// [`ReservationError::InvalidAmount`]. Any current state other than
/// `active`/`unresolved` is [`ReservationError::InvalidTransition`].
pub fn consume(
    tx: &Transaction<'_>,
    reservation_id: &str,
    used_mg: i64,
    note: Option<&str>,
) -> Result<AmountEvent, ReservationError> {
    if used_mg < 0 {
        return Err(ReservationError::InvalidAmount);
    }
    let reservation = load_reservation(tx, reservation_id)?;
    if !matches!(
        reservation.state,
        ReservationState::Active | ReservationState::Unresolved
    ) {
        return Err(ReservationError::InvalidTransition {
            from: reservation.state,
        });
    }
    let spool =
        repository::load_spool(tx, &reservation.spool_id)?.ok_or(ReservationError::NotFound)?;

    let after_mg = (spool.current_mg - used_mg).max(0);
    let shortfall_mg = used_mg - spool.current_mg;
    let final_note = if shortfall_mg > 0 {
        let shortfall_text = format!("clamped to 0; shortfall of {shortfall_mg}mg");
        Some(match note {
            Some(existing) => format!("{existing} ({shortfall_text})"),
            None => shortfall_text,
        })
    } else {
        note.map(str::to_string)
    };

    let event = ledger::append(
        tx,
        &reservation.spool_id,
        AmountEventKind::Consumption,
        after_mg,
        AmountConfidence::Estimated,
        LedgerSnapshot {
            reservation_id: Some(reservation_id.to_string()),
            note: final_note,
            ..Default::default()
        },
    )?;

    tx.execute(
        "UPDATE spool_reservations SET state = ?2, settled_at = ?3 WHERE id = ?1",
        params![
            reservation_id,
            encode_enum(ReservationState::Consumed),
            now_rfc3339(),
        ],
    )?;
    Ok(event)
}

/// D8: `availableMg = currentMg - reservedMg`, where `reservedMg` sums
/// every `active`/`unresolved` reservation on the Spool. Can go negative
/// once an estimate or measurement drops `currentMg` below what's reserved
/// — the caller (the UI, in a later task) surfaces that as an over-reserved
/// warning rather than this function clamping it.
pub fn availability(
    tx: &Transaction<'_>,
    spool_id: &str,
) -> Result<Availability, ReservationError> {
    let spool = repository::load_spool(tx, spool_id)?.ok_or(ReservationError::NotFound)?;
    let reserved_mg = repository::reserved_mg(tx, spool_id)?;
    Ok(Availability {
        current_mg: spool.current_mg,
        reserved_mg,
        available_mg: spool.current_mg - reserved_mg,
    })
}

/// Every `active`/`unresolved` reservation on `spool_id`, oldest first.
pub fn open_reservations(
    tx: &Transaction<'_>,
    spool_id: &str,
) -> Result<Vec<Reservation>, StorageError> {
    let mut statement = tx.prepare(&format!(
        "SELECT {RESERVATION_COLUMNS} FROM spool_reservations
         WHERE spool_id = ?1 AND state IN ('active', 'unresolved')
         ORDER BY created_at"
    ))?;
    let rows = statement
        .query_map([spool_id], decode_reservation)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}

/// Every reservation on `spool_id` in any state, oldest first, for
/// `spool_history`.
pub fn history(tx: &Transaction<'_>, spool_id: &str) -> Result<Vec<Reservation>, StorageError> {
    let mut statement = tx.prepare(&format!(
        "SELECT {RESERVATION_COLUMNS} FROM spool_reservations
         WHERE spool_id = ?1
         ORDER BY created_at, id"
    ))?;
    let rows = statement
        .query_map([spool_id], decode_reservation)?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    Ok(rows)
}
