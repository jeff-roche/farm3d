//! P3 Task 4: reservation primitives and the availability signal (D8).
//! Covers every point of the task-4 brief's Step 1, starting from a 1000 g
//! (1_000_000 mg) Spool: reserve, release, consume, the shortfall clamp,
//! `mark_unresolved`, over-reservation, non-reservable Spools, and rollback
//! inside a `storage.write`/`write_repo` closure.

mod common;

use rusqlite::Transaction;

use farm3d_lib::persistence::{RepositoryError, Storage};
use farm3d_lib::spools::ledger::{self, AmountEntry, AmountEventKind, LedgerSnapshot};
use farm3d_lib::spools::reservations::{
    self, ReservationError, ReservationHolder, ReservationState,
};
use farm3d_lib::spools::{
    repository, AmountConfidence, Availability, FilamentDiameter, MaterialFamily, SpoolFields,
    SpoolLifecycle,
};

use common::storage;

/// `Storage::write_repo` is fixed to `RepositoryError` (it exists so
/// `spools::repository`/`movement` callers get their own error type
/// straight out — see its doc comment); `spools::reservations` functions
/// return `ReservationError` instead. Wrapping the inner call's `Result` in
/// an outer `Ok` (mirroring `printers::batch`'s
/// `Result<Result<PlannedConnection, BatchRowError>, CommandError>`) lets a
/// test run one inside a real transaction and still get the specific
/// `ReservationError` back, rather than lossily converting it.
fn call<T>(
    storage: &Storage,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, ReservationError>,
) -> Result<T, ReservationError> {
    storage.write_repo(|tx| Ok(operation(tx))).unwrap()
}

fn valid_fields() -> SpoolFields {
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

/// A fresh 1000 g (1_000_000 mg) Spool, estimated.
fn new_spool(storage: &Storage) -> String {
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    storage
        .write_repo(|tx| repository::insert_spool(tx, &valid_fields(), &entry, None))
        .unwrap()
        .id
}

fn job(id: &str) -> ReservationHolder {
    ReservationHolder {
        kind: "job".to_string(),
        id: id.to_string(),
    }
}

fn availability(storage: &Storage, spool_id: &str) -> Availability {
    call(storage, |tx| reservations::availability(tx, spool_id)).unwrap()
}

fn current_mg(storage: &Storage, spool_id: &str) -> i64 {
    storage
        .write(|tx| repository::load_spool(tx, spool_id))
        .unwrap()
        .unwrap()
        .current_mg
}

/// 1. Reserve: reserve 300 g, then 600 g. Availability is 100 g. Reserving
///    101 g gives `InsufficientAvailable{available_mg: 100_000}`.
#[test]
fn reserving_tracks_available_mg_and_rejects_amounts_over_it() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);

    call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-1"), 300_000, "op-1")
    })
    .unwrap();
    call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-2"), 600_000, "op-2")
    })
    .unwrap();

    let avail = availability(&storage, &spool_id);
    assert_eq!(avail.current_mg, 1_000_000);
    assert_eq!(avail.reserved_mg, 900_000);
    assert_eq!(avail.available_mg, 100_000);

    let error = call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-3"), 101_000, "op-3")
    })
    .unwrap_err();
    assert!(matches!(
        error,
        ReservationError::InsufficientAvailable {
            available_mg: 100_000
        }
    ));
}

/// 2. Release: releasing the 300 g reservation brings availability to
///    400 g. Releasing it again gives `InvalidTransition{Released}`.
#[test]
fn releasing_frees_the_amount_and_cannot_be_repeated() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);
    let reservation_id = call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-1"), 300_000, "op-1")
    })
    .unwrap();
    call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-2"), 600_000, "op-2")
    })
    .unwrap();

    call(&storage, |tx| reservations::release(tx, &reservation_id)).unwrap();
    assert_eq!(availability(&storage, &spool_id).available_mg, 400_000);

    let error = call(&storage, |tx| reservations::release(tx, &reservation_id)).unwrap_err();
    assert!(matches!(
        error,
        ReservationError::InvalidTransition {
            from: ReservationState::Released
        }
    ));
}

/// 3. Consume: consuming the 600 g reservation with `used_mg = 640_000`
///    writes one `Consumption` event (`reservation_id` set, estimated) and
///    leaves the current amount at 360 g.
#[test]
fn consuming_writes_one_estimated_consumption_event_carrying_the_reservation_id() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);
    let reservation_id = call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-1"), 600_000, "op-1")
    })
    .unwrap();

    let event = call(&storage, |tx| {
        reservations::consume(tx, &reservation_id, 640_000, None)
    })
    .unwrap();

    assert!(matches!(event.kind, AmountEventKind::Consumption));
    assert_eq!(event.confidence_after, AmountConfidence::Estimated);
    assert_eq!(
        event.reservation_id.as_deref(),
        Some(reservation_id.as_str())
    );
    assert_eq!(event.after_mg, 360_000);
    assert_eq!(current_mg(&storage, &spool_id), 360_000);

    let history = storage.write(|tx| ledger::history(tx, &spool_id)).unwrap();
    assert_eq!(history.len(), 2);
    assert_eq!(history[1].id, event.id);
}

/// 4. Clamp: consuming 500 g against a 100 g current amount leaves the
///    current amount at 0, with a note that records the 400 g shortfall.
#[test]
fn consuming_more_than_the_current_amount_clamps_to_zero_and_notes_the_shortfall() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);
    // Bring the current amount down to 100 g directly through the ledger
    // (a measurement), independent of the reservation being consumed.
    storage
        .write_repo(|tx| {
            ledger::append(
                tx,
                &spool_id,
                AmountEventKind::Measurement,
                100_000,
                AmountConfidence::Measured,
                LedgerSnapshot::default(),
            )
        })
        .unwrap();
    let reservation_id = call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-1"), 100_000, "op-1")
    })
    .unwrap();

    let event = call(&storage, |tx| {
        reservations::consume(tx, &reservation_id, 500_000, None)
    })
    .unwrap();

    assert_eq!(event.after_mg, 0);
    assert_eq!(current_mg(&storage, &spool_id), 0);
    let note = event.note.expect("a clamp must record a note");
    assert!(
        note.contains("400000") || note.contains("400_000"),
        "note must record the 400 g (400_000 mg) shortfall: {note}"
    );
}

/// 5. Unresolved: `mark_unresolved` keeps the amount unavailable.
#[test]
fn marking_unresolved_keeps_the_amount_counted_against_availability() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);
    let reservation_id = call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-1"), 300_000, "op-1")
    })
    .unwrap();

    call(&storage, |tx| {
        reservations::mark_unresolved(tx, &reservation_id)
    })
    .unwrap();

    assert_eq!(availability(&storage, &spool_id).available_mg, 700_000);
    let open = storage
        .write_repo(|tx| reservations::open_reservations(tx, &spool_id).map_err(Into::into))
        .unwrap();
    assert_eq!(open.len(), 1);
    assert_eq!(open[0].state, ReservationState::Unresolved);
}

/// 6. Over-reservation: a measurement that lowers the current amount below
///    the reserved total gives a negative `available_mg`, and later
///    reserves fail.
#[test]
fn a_measurement_below_the_reserved_total_makes_availability_negative_and_blocks_further_reserves()
{
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);
    call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-1"), 900_000, "op-1")
    })
    .unwrap();

    storage
        .write_repo(|tx| {
            ledger::append(
                tx,
                &spool_id,
                AmountEventKind::Measurement,
                800_000,
                AmountConfidence::Measured,
                LedgerSnapshot::default(),
            )
        })
        .unwrap();

    let avail = availability(&storage, &spool_id);
    assert_eq!(avail.current_mg, 800_000);
    assert_eq!(avail.reserved_mg, 900_000);
    assert_eq!(avail.available_mg, -100_000);

    let error = call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-2"), 1_000, "op-2")
    })
    .unwrap_err();
    assert!(matches!(
        error,
        ReservationError::InsufficientAvailable {
            available_mg: -100_000
        }
    ));
}

/// 7. Not reservable: reserving on an empty or archived Spool gives
///    `SpoolNotReservable`.
#[test]
fn reserving_on_an_empty_or_archived_spool_is_rejected() {
    let (_temp, _lease, storage, _db) = storage();
    let empty_id = new_spool(&storage);
    storage
        .write(|tx| {
            tx.execute(
                "UPDATE spools SET lifecycle = 'empty' WHERE id = ?1",
                [&empty_id],
            )?;
            Ok(())
        })
        .unwrap();
    let error = call(&storage, |tx| {
        reservations::reserve(tx, &empty_id, &job("job-1"), 1_000, "op-1")
    })
    .unwrap_err();
    assert!(matches!(
        error,
        ReservationError::SpoolNotReservable {
            lifecycle: SpoolLifecycle::Empty
        }
    ));

    let archived_id = new_spool(&storage);
    storage
        .write(|tx| {
            tx.execute(
                "UPDATE spools SET lifecycle = 'archived', archived_from = 'active', slot_id = NULL
                 WHERE id = ?1",
                [&archived_id],
            )?;
            Ok(())
        })
        .unwrap();
    let error = call(&storage, |tx| {
        reservations::reserve(tx, &archived_id, &job("job-1"), 1_000, "op-1")
    })
    .unwrap_err();
    assert!(matches!(
        error,
        ReservationError::SpoolNotReservable {
            lifecycle: SpoolLifecycle::Archived
        }
    ));
}

/// 8. Rollback: a reservation inside a `storage.write_repo` closure that
///    returns `Err` afterwards leaves no row.
#[test]
fn a_reservation_inside_a_failing_transaction_leaves_no_row() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);

    let result: Result<(), RepositoryError> = storage.write_repo(|tx| {
        let _ = reservations::reserve(tx, &spool_id, &job("job-1"), 300_000, "op-1");
        Err(RepositoryError::NotFound {
            entity_id: "forced-rollback".to_string(),
        })
    });
    assert!(result.is_err());

    let open = storage
        .write_repo(|tx| reservations::open_reservations(tx, &spool_id).map_err(Into::into))
        .unwrap();
    assert!(
        open.is_empty(),
        "the reservation must not survive the rollback"
    );
    assert_eq!(availability(&storage, &spool_id).available_mg, 1_000_000);
}

/// `reserve` rejects a zero or negative amount (D8: `amount_mg > 0`) with
/// `InvalidAmount`, before touching the table.
#[test]
fn reserving_a_zero_or_negative_amount_is_rejected() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);

    for amount_mg in [0, -1, -500_000] {
        let error = call(&storage, |tx| {
            reservations::reserve(tx, &spool_id, &job("job-1"), amount_mg, "op-1")
        })
        .unwrap_err();
        assert!(
            matches!(error, ReservationError::InvalidAmount),
            "{amount_mg}: {error:?}"
        );
    }
    assert_eq!(availability(&storage, &spool_id).reserved_mg, 0);
}

/// `consume` rejects a negative `used_mg` with `InvalidAmount`, leaving the
/// reservation `active` and the ledger untouched. Zero is allowed (a Job
/// that used nothing).
#[test]
fn consuming_a_negative_amount_is_rejected_but_zero_is_allowed() {
    let (_temp, _lease, storage, _db) = storage();
    let spool_id = new_spool(&storage);
    let reservation_id = call(&storage, |tx| {
        reservations::reserve(tx, &spool_id, &job("job-1"), 300_000, "op-1")
    })
    .unwrap();

    let error = call(&storage, |tx| {
        reservations::consume(tx, &reservation_id, -1, None)
    })
    .unwrap_err();
    assert!(
        matches!(error, ReservationError::InvalidAmount),
        "{error:?}"
    );
    assert_eq!(current_mg(&storage, &spool_id), 1_000_000);
    assert_eq!(availability(&storage, &spool_id).reserved_mg, 300_000);

    let event = call(&storage, |tx| {
        reservations::consume(tx, &reservation_id, 0, None)
    })
    .unwrap();
    assert_eq!(event.after_mg, 1_000_000);
}
