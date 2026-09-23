//! P3 Task 2: the Spool repository, the append-only amount ledger (D7), and
//! tares (D3). Covers every point of the task-2 brief's Step 1: estimated
//! create, scale entry (with its tare snapshot/default-tare/delete
//! behavior), validation, corrections, restart durability, derived facets,
//! and case-insensitive duplicate tare names.

mod common;

use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StorageError, StoragePaths};
use farm3d_lib::spools::ledger::{self, AmountEntry, AmountEventKind, LedgerSnapshot};
use farm3d_lib::spools::{repository, tares, AmountConfidence, FilamentDiameter, MaterialFamily, SpoolFields};

use common::storage;

fn valid_fields() -> SpoolFields {
    SpoolFields {
        manufacturer: "Polymaker".to_string(),
        product: Some("PolyTerra".to_string()),
        material_family: MaterialFamily::Pla,
        material_other: None,
        color_name: "Black".to_string(),
        color_hex: Some("#000000".to_string()),
        diameter: FilamentDiameter::D175,
        nominal_mg: 1_000_000,
        low_threshold_mg: 100_000,
        tare_id: None,
        notes: None,
    }
}

/// 1. Estimated create: `insert_spool` with `Net{1_000_000, Estimated}`
///    writes one `initial` event (`before_mg = None`), the cache matches,
///    and `spool_number == 1`. A second Spool gets number 2.
#[test]
fn insert_spool_writes_one_initial_event_and_allocates_sequential_spool_numbers() {
    let (_temp, _lease, storage, _db) = storage();
    let fields = valid_fields();
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };

    let first = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap();
    assert_eq!(first.spool_number, 1);
    assert_eq!(first.current_mg, 1_000_000);
    assert!(matches!(first.confidence, AmountConfidence::Estimated));

    let history = storage.write(|tx| ledger::history(tx, &first.id)).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].before_mg, None);
    assert_eq!(history[0].after_mg, 1_000_000);
    assert!(matches!(history[0].kind, AmountEventKind::Initial));
    assert!(!history[0].is_correction);
    // The cache (spools.current_mg/confidence) matches the latest row (D7).
    assert_eq!(first.current_mg, history[0].after_mg);
    assert_eq!(first.confidence, history[0].confidence_after);

    let second = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap();
    assert_eq!(second.spool_number, 2);
}

/// 2. Scale entry: a tare "Cardboard" of 140 g, then `Scale{gross 752_300,
///    tareId}` gives 612.3 g net (612_300), `measured`, with snapshots
///    `grossMg = 752_300` and `tareMg = 140_000`. Updating the tare to
///    150 g leaves that event's `tareMg` unchanged. Deleting the tare sets
///    the Spool's `tareId = None`.
///
/// Controller ruling (D3): a Scale entry's `tareId` snapshots the value it
/// measured against but never changes the Spool's own default tare — that
/// comes only from `SpoolFields.tareId` (at create, or `update_spool_fields`
/// later). So the Spool here is created with `tareId` already set to
/// "Cardboard", and a later scale entry against a *different* tare is
/// checked to leave that default untouched.
#[test]
fn scale_entry_snapshots_gross_and_tare_without_ever_changing_the_spools_default_tare() {
    let (_temp, _lease, storage, _db) = storage();
    let cardboard = storage
        .write_repo(|tx| tares::create(tx, "Cardboard", 140_000))
        .unwrap();

    let mut fields = valid_fields();
    fields.tare_id = Some(cardboard.id.clone());
    let entry = AmountEntry::Scale {
        gross_mg: 752_300,
        tare_id: Some(cardboard.id.clone()),
        tare_mg: None,
    };
    let spool = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap();

    assert_eq!(spool.current_mg, 612_300);
    assert!(matches!(spool.confidence, AmountConfidence::Measured));
    assert_eq!(spool.tare_id.as_deref(), Some(cardboard.id.as_str()));

    let history = storage.write(|tx| ledger::history(tx, &spool.id)).unwrap();
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].gross_mg, Some(752_300));
    assert_eq!(history[0].tare_mg, Some(140_000));

    // Updating the tare's weight leaves the already-recorded snapshot
    // unchanged (D3: each measurement snapshots the values it used).
    let updated_cardboard = storage
        .write_repo(|tx| {
            tares::update(tx, &cardboard.id, cardboard.revision, "Cardboard", 150_000)
        })
        .unwrap();
    let history_after_tare_edit = storage.write(|tx| ledger::history(tx, &spool.id)).unwrap();
    assert_eq!(history_after_tare_edit[0].tare_mg, Some(140_000));

    // A later scale entry against a *different* tare must never change the
    // Spool's own default tare.
    let other_tare = storage
        .write_repo(|tx| tares::create(tx, "Spool Core", 50_000))
        .unwrap();
    storage
        .write_repo(|tx| {
            let other_entry = AmountEntry::Scale {
                gross_mg: 600_000,
                tare_id: Some(other_tare.id.clone()),
                tare_mg: None,
            };
            let (after_mg, confidence, snapshot) = ledger::resolve_entry(tx, &other_entry)?;
            ledger::append(
                tx,
                &spool.id,
                AmountEventKind::Measurement,
                after_mg,
                confidence,
                snapshot,
            )
        })
        .unwrap();
    let after_other_tare_entry = storage
        .write(|tx| repository::load_spool(tx, &spool.id))
        .unwrap()
        .unwrap();
    assert_eq!(
        after_other_tare_entry.tare_id.as_deref(),
        Some(cardboard.id.as_str()),
        "a scale entry's tareId must never change the Spool's default tare"
    );

    // Deleting the Spool's default tare clears it (ON DELETE SET NULL),
    // but every ledger snapshot above is untouched.
    storage
        .write_repo(|tx| tares::delete(tx, &updated_cardboard.id, updated_cardboard.revision))
        .unwrap();
    let reloaded = storage
        .write(|tx| repository::load_spool(tx, &spool.id))
        .unwrap()
        .unwrap();
    assert_eq!(reloaded.tare_id, None);
    let history_after_delete = storage.write(|tx| ledger::history(tx, &spool.id)).unwrap();
    assert_eq!(history_after_delete[0].tare_mg, Some(140_000));
}

/// 3a. Gross below the tare gives `Validation{field_path: "entry.grossMg"}`.
#[test]
fn scale_entry_with_gross_below_the_tare_is_a_validation_error() {
    let (_temp, _lease, storage, _db) = storage();
    let fields = valid_fields();
    let entry = AmountEntry::Scale {
        gross_mg: 50_000,
        tare_id: None,
        tare_mg: Some(140_000),
    };
    let error = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap_err();
    assert!(matches!(
        error,
        RepositoryError::Validation {
            field_path: "entry.grossMg"
        }
    ));
}

/// 3b. Giving both `tareId` and `tareMg`, or neither, is a validation
///     error.
#[test]
fn scale_entry_requires_exactly_one_of_tare_id_or_tare_mg() {
    let (_temp, _lease, storage, _db) = storage();
    let fields = valid_fields();

    let neither = AmountEntry::Scale {
        gross_mg: 500_000,
        tare_id: None,
        tare_mg: None,
    };
    let error = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &neither, None))
        .unwrap_err();
    assert!(matches!(
        error,
        RepositoryError::Validation {
            field_path: "entry.tareId"
        }
    ));

    let both = AmountEntry::Scale {
        gross_mg: 500_000,
        tare_id: Some("tar-nonexistent".to_string()),
        tare_mg: Some(1_000),
    };
    let error = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &both, None))
        .unwrap_err();
    assert!(matches!(
        error,
        RepositoryError::Validation {
            field_path: "entry.tareId"
        }
    ));
}

/// 4. Corrections: the sequence initial (estimated) -> `Consumption`
///    (appended directly with `ledger::append`) -> `Measurement` gives
///    `history()[2].is_correction == true`, and its `before_mg` equals the
///    consumption row's `after_mg`.
#[test]
fn a_measurement_after_a_consumption_is_flagged_as_a_correction() {
    let (_temp, _lease, storage, _db) = storage();
    let fields = valid_fields();
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    let spool = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap();

    let consumption = storage
        .write_repo(|tx| {
            ledger::append(
                tx,
                &spool.id,
                AmountEventKind::Consumption,
                400_000,
                AmountConfidence::Estimated,
                LedgerSnapshot::default(),
            )
        })
        .unwrap();

    storage
        .write_repo(|tx| {
            ledger::append(
                tx,
                &spool.id,
                AmountEventKind::Measurement,
                380_000,
                AmountConfidence::Measured,
                LedgerSnapshot::default(),
            )
        })
        .unwrap();

    let history = storage.write(|tx| ledger::history(tx, &spool.id)).unwrap();
    assert_eq!(history.len(), 3);
    assert!(!history[0].is_correction, "the initial row is never a correction");
    assert!(!history[1].is_correction, "a consumption is never itself a correction");
    assert!(history[2].is_correction);
    assert_eq!(history[2].before_mg, Some(consumption.after_mg));
}

/// 5. Restart: reopening `Storage` on the same paths leaves the history
///    equal and ordered by `sequence`, and the cache equal to the last row.
#[test]
fn ledger_history_and_the_cache_survive_a_restart() {
    let temp = tempfile::tempdir().expect("temp dir");
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
        .expect("storage paths");
    let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
    let storage = Storage::open(paths.clone(), &lease).expect("storage");

    let fields = valid_fields();
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    let spool_id = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap()
        .id;
    storage
        .write_repo(|tx| {
            ledger::append(
                tx,
                &spool_id,
                AmountEventKind::Estimate,
                900_000,
                AmountConfidence::Estimated,
                LedgerSnapshot::default(),
            )
        })
        .unwrap();

    let before_history = storage.write(|tx| ledger::history(tx, &spool_id)).unwrap();
    drop(storage);

    let reopened = Storage::open(paths, &lease).expect("reopened storage");
    let after_history = reopened.write(|tx| ledger::history(tx, &spool_id)).unwrap();
    assert_eq!(before_history, after_history);
    let sequences: Vec<i64> = after_history.iter().map(|event| event.sequence).collect();
    assert_eq!(sequences, vec![1, 2], "history must be ordered by sequence");

    let reloaded = reopened
        .write(|tx| repository::load_spool(tx, &spool_id))
        .unwrap()
        .unwrap();
    let last = after_history.last().unwrap();
    assert_eq!(reloaded.current_mg, last.after_mg);
    assert_eq!(reloaded.confidence, last.confidence_after);
}

/// 6. Facets: `low` is true when `current_mg <= low_threshold_mg` and the
///    lifecycle is active, and false when the lifecycle is empty.
///    `confidence` mirrors the cache.
#[test]
fn low_facet_reflects_the_threshold_and_lifecycle_confidence_mirrors_the_cache() {
    let (_temp, _lease, storage, _db) = storage();
    let mut fields = valid_fields();
    fields.low_threshold_mg = 100_000;
    let entry = AmountEntry::Net {
        net_mg: 100_000,
        confidence: AmountConfidence::Measured,
    };
    let spool = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap();

    let records = storage.write(repository::list_spools).unwrap();
    let record = records.iter().find(|record| record.id == spool.id).unwrap();
    assert!(record.facets.low, "at the threshold, active, must be low");
    assert_eq!(record.facets.confidence, AmountConfidence::Measured);

    // Task 6 owns `set_spool_lifecycle`; direct SQL is enough here to
    // exercise the derivation for an `empty` lifecycle.
    storage
        .write(|tx| -> Result<(), StorageError> {
            tx.execute("UPDATE spools SET lifecycle = 'empty' WHERE id = ?1", [&spool.id])?;
            Ok(())
        })
        .unwrap();
    let records_after = storage.write(repository::list_spools).unwrap();
    let record_after = records_after
        .iter()
        .find(|record| record.id == spool.id)
        .unwrap();
    assert!(!record_after.facets.low, "an empty spool is never low");
}

/// 7. Duplicate tare: a tare name that differs only by case gives
///    `Validation{field_path: "name"}`.
#[test]
fn a_tare_name_that_differs_only_by_case_is_rejected() {
    let (_temp, _lease, storage, _db) = storage();
    storage
        .write_repo(|tx| tares::create(tx, "Cardboard", 140_000))
        .unwrap();

    let error = storage
        .write_repo(|tx| tares::create(tx, "cardboard", 150_000))
        .unwrap_err();
    assert!(matches!(
        error,
        RepositoryError::Validation { field_path: "name" }
    ));
}

/// `check_and_bump_revision` and `update_spool_fields`: not directly listed
/// in the brief's Step 1, but part of the task's interface and Task 3's
/// (`move_spool`) handoff, so covered directly here.
#[test]
fn check_and_bump_revision_bumps_without_changing_other_fields() {
    let (_temp, _lease, storage, _db) = storage();
    let fields = valid_fields();
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    let spool = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap();

    let bumped = storage
        .write_repo(|tx| repository::check_and_bump_revision(tx, &spool.id, spool.revision))
        .unwrap();
    assert_eq!(bumped.revision, spool.revision + 1);
    assert_eq!(bumped.manufacturer, spool.manufacturer);
    assert_eq!(bumped.current_mg, spool.current_mg);

    let stale = storage
        .write_repo(|tx| repository::check_and_bump_revision(tx, &spool.id, spool.revision))
        .unwrap_err();
    assert!(matches!(
        stale,
        RepositoryError::Conflict {
            expected_revision,
            current_revision,
            ..
        } if expected_revision == spool.revision && current_revision == bumped.revision
    ));

    let missing = storage
        .write_repo(|tx| repository::check_and_bump_revision(tx, "spl-missing", 1))
        .unwrap_err();
    assert!(matches!(missing, RepositoryError::NotFound { .. }));
}

#[test]
fn update_spool_fields_replaces_editable_fields_and_checks_revision() {
    let (_temp, _lease, storage, _db) = storage();
    let fields = valid_fields();
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    let spool = storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap();

    let mut patch = fields.clone();
    patch.manufacturer = "Prusament".to_string();
    patch.color_name = "Galaxy Black".to_string();

    let updated = storage
        .write_repo(|tx| repository::update_spool_fields(tx, &spool.id, spool.revision, &patch))
        .unwrap();
    assert_eq!(updated.manufacturer, "Prusament");
    assert_eq!(updated.color_name, "Galaxy Black");
    assert_eq!(updated.revision, spool.revision + 1);
    // Fields update never touches the amount cache.
    assert_eq!(updated.current_mg, spool.current_mg);

    let stale = storage
        .write_repo(|tx| repository::update_spool_fields(tx, &spool.id, spool.revision, &patch))
        .unwrap_err();
    assert!(matches!(stale, RepositoryError::Conflict { .. }));
}
