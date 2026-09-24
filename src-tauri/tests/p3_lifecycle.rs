//! P3 Task 6: archiving a Printer with Spool dispositions, the
//! `SPOOLS_LOADED`/`SPOOL_RESERVED` lifecycle blockers, Spool lifecycle
//! actions, and the Printer-delete cascade (spec D8, D9, D10). Covers every
//! point of the task-6 brief's Step 1.
//!
//! Fixture: Printer P has three slots holding Spools A, B, and C; Printer Q
//! has slot Q1 holding D; Printer R is empty. Printer lifecycle goes through
//! the real Tauri commands; Spool lifecycle goes through
//! `spools::lifecycle::apply_lifecycle` directly (ruling R4 — its command
//! lands in Task 7).

mod common;

use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use farm3d_lib::connections::{ConnectionConfig, PrinterConnection, MOONRAKER_KIND};
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage, StoragePaths};
use farm3d_lib::printers::lifecycle::{LifecycleAction, LifecycleBlockerCode};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::spools::dispositions::{SpoolDisposition, SpoolDispositionInput};
use farm3d_lib::spools::ledger::{self, AmountEntry, AmountEvent, AmountEventKind};
use farm3d_lib::spools::lifecycle::{apply_lifecycle, SpoolLifecycleAction};
use farm3d_lib::spools::movement::{self, MoveDestination, MovementReason, SpoolMovement};
use farm3d_lib::spools::repository::{self, StoredSpool};
use farm3d_lib::spools::reservations::{self, ReservationHolder};
use farm3d_lib::spools::slots::{InitialLoad, SlotSpec};
use farm3d_lib::spools::{
    AmountConfidence, FilamentDiameter, MaterialFamily, SpoolFields, SpoolLifecycle,
};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;

use common::{a_catalog, a_stored_printer, invoke};

const CREDENTIAL_REF: &str = "farm3d/credential/6ba7b810-9dad-4f83-a131-2a6f44cbbf89";

// --- Fixture -------------------------------------------------------------------

struct Env {
    temp: tempfile::TempDir,
    lease: MetadataRootLease,
    storage: Arc<Storage>,
}

fn env() -> Env {
    let temp = tempfile::tempdir().unwrap();
    let lease = MetadataRootLease::acquire(&paths(&temp)).unwrap();
    let storage = Arc::new(Storage::open(paths(&temp), &lease).unwrap());
    Env {
        temp,
        lease,
        storage,
    }
}

fn paths(temp: &tempfile::TempDir) -> StoragePaths {
    StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap()
}

/// The ids the fixture created. `p_slots` is P's three slots in position
/// order (A, B, C's slots respectively).
struct Farm {
    p: StoredPrinter,
    r: StoredPrinter,
    p_slots: Vec<String>,
    q1: String,
    a: String,
    b: String,
    c: String,
    d: String,
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

fn new_spool(storage: &Storage) -> StoredSpool {
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    storage
        .write_repo(|tx| repository::insert_spool(tx, &valid_fields(), &entry, None))
        .unwrap()
}

fn slot(name: &str) -> SlotSpec {
    SlotSpec {
        id: None,
        name: name.to_string(),
        feeder_label: None,
    }
}

fn moonraker(host: &str) -> ConnectionConfig {
    ConnectionConfig {
        kind: MOONRAKER_KIND.to_string(),
        host: host.to_string(),
        port: 7125,
        use_tls: false,
        credential_ref: Some(CREDENTIAL_REF.to_string()),
    }
}

fn create_printer(
    storage: &Arc<Storage>,
    printer: StoredPrinter,
    layout: &[SlotSpec],
    loads: &[&StoredSpool],
) -> StoredPrinter {
    let loads: Vec<InitialLoad> = loads
        .iter()
        .enumerate()
        .map(|(slot_index, spool)| InitialLoad {
            slot_index,
            spool_id: spool.id.clone(),
            expected_spool_revision: spool.revision,
        })
        .collect();
    PrinterRepository::new(Arc::clone(storage))
        .create_with_layout(printer, None, layout, &loads)
        .unwrap()
}

/// P (connected, so its supervision can be observed) holds A, B, C; Q holds
/// D in Q1; R is empty.
fn farm(storage: &Arc<Storage>) -> Farm {
    let (a, b, c, d) = (
        new_spool(storage),
        new_spool(storage),
        new_spool(storage),
        new_spool(storage),
    );
    let mut p = a_stored_printer("prn-p");
    p.connection = Some(moonraker("p.local"));
    let p = create_printer(
        storage,
        p,
        &[slot("P1"), slot("P2"), slot("P3")],
        &[&a, &b, &c],
    );
    let q = create_printer(storage, a_stored_printer("prn-q"), &[slot("Q1")], &[&d]);
    let r = create_printer(storage, a_stored_printer("prn-r"), &[slot("R1")], &[]);
    Farm {
        p_slots: p.material_slots.iter().map(|s| s.id.clone()).collect(),
        q1: q.material_slots[0].id.clone(),
        p,
        r,
        a: a.id,
        b: b.id,
        c: c.id,
        d: d.id,
    }
}

// --- Helpers ---------------------------------------------------------------------

fn spool(storage: &Storage, id: &str) -> StoredSpool {
    storage
        .write_repo(|tx| Ok(repository::load_spool(tx, id)?.unwrap()))
        .unwrap()
}

fn spool_revision(storage: &Storage, id: &str) -> i64 {
    spool(storage, id).revision
}

fn printer(storage: &Arc<Storage>, id: &str) -> StoredPrinter {
    PrinterRepository::new(Arc::clone(storage))
        .get(id)
        .unwrap()
        .unwrap()
}

fn movement_history(storage: &Storage, spool_id: &str) -> Vec<SpoolMovement> {
    storage
        .write_repo(|tx| Ok(movement::history(tx, spool_id)?))
        .unwrap()
}

fn ledger_history(storage: &Storage, spool_id: &str) -> Vec<AmountEvent> {
    storage
        .write_repo(|tx| Ok(ledger::history(tx, spool_id)?))
        .unwrap()
}

fn movements_of_operation(storage: &Storage, operation_id: &str) -> Vec<SpoolMovement> {
    storage
        .write_repo(|tx| {
            Ok(movement::find_operation(tx, operation_id)?
                .map(|outcome| outcome.movements)
                .unwrap_or_default())
        })
        .unwrap()
}

fn movement_count(storage: &Storage) -> i64 {
    storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM spool_movements", [], |row| row.get(0))
        })
        .unwrap()
}

fn reserve(storage: &Storage, spool_id: &str) -> String {
    storage
        .write_repo(|tx| {
            Ok(reservations::reserve(
                tx,
                spool_id,
                &ReservationHolder {
                    kind: "job".to_string(),
                    id: "job-1".to_string(),
                },
                100_000,
                "op-reserve",
            )
            .unwrap())
        })
        .unwrap()
}

fn to_storage(storage: &Storage, spool_id: &str, label: Option<&str>) -> SpoolDispositionInput {
    SpoolDispositionInput {
        spool_id: spool_id.to_string(),
        expected_spool_revision: spool_revision(storage, spool_id),
        disposition: SpoolDisposition::Storage {
            storage_label: label.map(str::to_string),
        },
    }
}

fn to_slot(
    storage: &Storage,
    spool_id: &str,
    slot_id: &str,
    expected_occupant: Option<&str>,
    displaced_label: Option<&str>,
) -> SpoolDispositionInput {
    SpoolDispositionInput {
        spool_id: spool_id.to_string(),
        expected_spool_revision: spool_revision(storage, spool_id),
        disposition: SpoolDisposition::Slot {
            slot_id: slot_id.to_string(),
            expected_occupant_spool_id: expected_occupant.map(str::to_string),
            displaced_storage_label: displaced_label.map(str::to_string),
        },
    }
}

fn mark_empty(storage: &Storage, spool_id: &str) -> SpoolDispositionInput {
    SpoolDispositionInput {
        spool_id: spool_id.to_string(),
        expected_spool_revision: spool_revision(storage, spool_id),
        disposition: SpoolDisposition::MarkEmpty {
            storage_label: None,
        },
    }
}

/// The brief's step-3 dispositions: A to "Shelf", B into Q1 (displacing D
/// to "Bin"), C marked empty.
fn happy_dispositions(storage: &Storage, farm: &Farm) -> Vec<SpoolDispositionInput> {
    vec![
        to_storage(storage, &farm.a, Some("Shelf")),
        to_slot(storage, &farm.b, &farm.q1, Some(&farm.d), Some("Bin")),
        mark_empty(storage, &farm.c),
    ]
}

fn archive_repo(
    storage: &Arc<Storage>,
    id: &str,
    operation_id: &str,
    dispositions: &[SpoolDispositionInput],
) -> Result<StoredPrinter, RepositoryError> {
    let revision = printer(storage, id).revision;
    PrinterRepository::new(Arc::clone(storage)).archive(id, revision, operation_id, dispositions)
}

fn archive_body(
    storage: &Arc<Storage>,
    id: &str,
    operation_id: &str,
    dispositions: &[SpoolDispositionInput],
) -> Value {
    json!({
        "contractVersion": 1,
        "id": id,
        "expectedRevision": printer(storage, id).revision,
        "operationId": operation_id,
        "spoolDispositions": serde_json::to_value(dispositions).unwrap(),
    })
}

fn blocker_codes(
    blockers: &[farm3d_lib::printers::lifecycle::LifecycleBlocker],
) -> Vec<LifecycleBlockerCode> {
    blockers.iter().map(|blocker| blocker.code).collect()
}

// --- Runtime -------------------------------------------------------------------

/// A connection factory that records each host it's asked to build a
/// Connection for and always declines — the P2 lifecycle tests' way of
/// observing whether supervision (re)started.
fn recording_factory() -> (
    impl Fn(
            &ConnectionConfig,
            Option<zeroize::Zeroizing<String>>,
        ) -> Option<Box<dyn PrinterConnection>>
        + Send
        + Sync
        + 'static,
    Receiver<String>,
) {
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let tx = Mutex::new(tx);
    let factory = move |config: &ConnectionConfig, _key: Option<zeroize::Zeroizing<String>>| {
        let _ = tx.lock().unwrap().send(config.host.clone());
        None
    };
    (factory, rx)
}

struct Runtime {
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    manager: Arc<farm3d_lib::connections::supervisor::ConnectionManager<MockRuntime>>,
    services: Arc<RuntimeServices<MockRuntime>>,
    calls: Receiver<String>,
    _credentials: tempfile::TempDir,
}

fn runtime(storage: &Arc<Storage>) -> Runtime {
    let credentials = tempfile::tempdir().unwrap();
    let (factory, calls) = recording_factory();
    let (app, webview, manager, services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::printers::commands::printer_lifecycle_eligibility,
            farm3d_lib::printers::commands::archive_printer,
            farm3d_lib::printers::commands::delete_printer,
        ],
        Arc::clone(storage),
        Arc::new(a_catalog()),
        credentials.path().to_path_buf(),
        factory,
    );
    services.credentials.set(CREDENTIAL_REF, "s3cret").unwrap();
    Runtime {
        _app: app,
        webview,
        manager,
        services,
        calls,
        _credentials: credentials,
    }
}

fn expect_a_call(calls: &Receiver<String>) -> String {
    calls
        .recv_timeout(Duration::from_secs(2))
        .expect("the connection factory should have been called")
}

fn expect_no_call(calls: &Receiver<String>) {
    match calls.recv_timeout(Duration::from_millis(200)) {
        Err(RecvTimeoutError::Timeout) => {}
        other => panic!("expected no further factory call, got {other:?}"),
    }
}

// --- 1. Eligibility ------------------------------------------------------------

#[test]
fn eligibility_of_a_printer_with_loaded_spools_blocks_archive_and_lists_them() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);

    let response = invoke(
        &rt.webview,
        "printer_lifecycle_eligibility",
        json!({"contractVersion": 1, "id": farm.p.id}),
    )
    .unwrap();

    let data = &response["data"];
    assert_eq!(data["canArchive"], json!(false));
    let archive_codes: Vec<&str> = data["blockers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|blocker| blocker["action"] == "archive")
        .map(|blocker| blocker["code"].as_str().unwrap())
        .collect();
    assert_eq!(archive_codes, vec!["SPOOLS_LOADED"]);
    let loaded: Vec<&str> = data["loadedSpools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|spool| spool["id"].as_str().unwrap())
        .collect();
    assert_eq!(
        loaded,
        vec![farm.a.as_str(), farm.b.as_str(), farm.c.as_str()]
    );
    assert_eq!(
        data["loadedSpools"][0]["location"]["printerId"],
        json!(farm.p.id)
    );

    let empty = invoke(
        &rt.webview,
        "printer_lifecycle_eligibility",
        json!({"contractVersion": 1, "id": farm.r.id}),
    )
    .unwrap();
    assert_eq!(empty["data"]["canArchive"], json!(true));
    assert_eq!(empty["data"]["loadedSpools"], json!([]));
}

// --- 2. Missing dispositions ---------------------------------------------------

#[test]
fn archiving_with_loaded_spools_and_no_dispositions_is_lifecycle_blocked() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);

    let error = invoke(
        &rt.webview,
        "archive_printer",
        archive_body(&env.storage, &farm.p.id, "op-none", &[]),
    )
    .unwrap_err();

    assert_eq!(error["code"], "LIFECYCLE_BLOCKED");
    assert_eq!(error["details"]["blockers"][0]["code"], "SPOOLS_LOADED");
    let message = error["message"].as_str().unwrap();
    assert!(message.starts_with("This action is blocked:"), "{message}");
    assert!(!message.contains("Printer cannot be changed"), "{message}");
    assert!(printer(&env.storage, &farm.p.id).archived_at.is_none());
}

#[test]
fn dispositions_that_miss_a_loaded_spool_are_a_validation_error() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);
    let partial = vec![
        to_storage(&env.storage, &farm.a, Some("Shelf")),
        to_storage(&env.storage, &farm.b, None),
    ];

    let error = invoke(
        &rt.webview,
        "archive_printer",
        archive_body(&env.storage, &farm.p.id, "op-partial", &partial),
    )
    .unwrap_err();

    assert_eq!(error["code"], "VALIDATION");
    assert_eq!(error["details"]["fieldPath"], "spoolDispositions");
    assert!(printer(&env.storage, &farm.p.id).archived_at.is_none());
    assert_eq!(
        spool(&env.storage, &farm.a).slot_id.as_deref(),
        Some(farm.p_slots[0].as_str())
    );
}

#[test]
fn dispositions_naming_a_spool_twice_or_an_unloaded_spool_are_a_validation_error() {
    let env = env();
    let farm = farm(&env.storage);

    let twice = vec![
        to_storage(&env.storage, &farm.a, Some("Shelf")),
        to_storage(&env.storage, &farm.a, Some("Shelf")),
        to_storage(&env.storage, &farm.b, None),
        mark_empty(&env.storage, &farm.c),
    ];
    let error = archive_repo(&env.storage, &farm.p.id, "op-twice", &twice).unwrap_err();
    assert!(matches!(
        error,
        RepositoryError::Validation {
            field_path: "spoolDispositions"
        }
    ));

    let extra = vec![
        to_storage(&env.storage, &farm.a, Some("Shelf")),
        to_storage(&env.storage, &farm.b, None),
        mark_empty(&env.storage, &farm.c),
        to_storage(&env.storage, &farm.d, None),
    ];
    let error = archive_repo(&env.storage, &farm.p.id, "op-extra", &extra).unwrap_err();
    assert!(matches!(
        error,
        RepositoryError::Validation {
            field_path: "spoolDispositions"
        }
    ));
    assert!(printer(&env.storage, &farm.p.id).archived_at.is_none());
}

// --- 3. Atomic archive ---------------------------------------------------------

#[test]
fn archiving_applies_every_disposition_atomically_then_stops_supervision() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);
    tauri::async_runtime::block_on(farm3d_lib::printers::setup::supervise_printer(
        &rt.manager,
        rt.services.credentials.as_ref(),
        &rt.services.catalog,
        &printer(&env.storage, &farm.p.id),
    ));
    assert_eq!(expect_a_call(&rt.calls), "p.local");
    let dispositions = happy_dispositions(&env.storage, &farm);

    let response = invoke(
        &rt.webview,
        "archive_printer",
        archive_body(&env.storage, &farm.p.id, "op-archive", &dispositions),
    )
    .unwrap();

    // P is archived; the response carries the committed revision and
    // empty slots.
    let stored = printer(&env.storage, &farm.p.id);
    assert!(stored.archived_at.is_some());
    let returned = &response["data"]["printer"];
    assert!(returned["archivedAt"].is_string());
    assert_eq!(returned["revision"], json!(stored.revision));
    let slots = returned["materialSlots"].as_array().unwrap();
    assert_eq!(slots.len(), 3);
    assert!(slots
        .iter()
        .all(|slot| slot.get("occupantSpoolId").is_none()));

    // Every Spool ended where its disposition sent it.
    let a = spool(&env.storage, &farm.a);
    assert_eq!(
        (a.slot_id.as_deref(), a.storage_label.as_deref()),
        (None, Some("Shelf"))
    );
    assert_eq!(
        spool(&env.storage, &farm.b).slot_id.as_deref(),
        Some(farm.q1.as_str())
    );
    let d = spool(&env.storage, &farm.d);
    assert_eq!(
        (d.slot_id.as_deref(), d.storage_label.as_deref()),
        (None, Some("Bin"))
    );
    let c = spool(&env.storage, &farm.c);
    assert_eq!(c.slot_id, None);
    assert_eq!(c.lifecycle, SpoolLifecycle::Empty);
    assert_eq!(c.current_mg, 0);
    let c_last = ledger_history(&env.storage, &farm.c).pop().unwrap();
    assert_eq!(c_last.kind, AmountEventKind::MarkedEmpty);
    assert_eq!(c_last.after_mg, 0);
    assert_eq!(c_last.confidence_after, AmountConfidence::Measured);

    // One operation, with the D10 reasons.
    let rows = movements_of_operation(&env.storage, "op-archive");
    let mut reasons: Vec<(String, MovementReason)> = rows
        .iter()
        .map(|row| (row.spool_id.clone(), row.reason))
        .collect();
    reasons.sort_by(|left, right| left.0.cmp(&right.0));
    let mut expected = vec![
        (farm.a.clone(), MovementReason::PrinterArchived),
        (farm.b.clone(), MovementReason::PrinterArchived),
        (farm.c.clone(), MovementReason::Consumed),
        (farm.d.clone(), MovementReason::Displaced),
    ];
    expected.sort_by(|left, right| left.0.cmp(&right.0));
    assert_eq!(reasons, expected);
    assert!(rows.iter().all(|row| row.operation_id == "op-archive"));

    // P2's post-commit order: supervision stopped, never restarted.
    assert!(
        !rt.manager.statuses().contains_key(&farm.p.id),
        "an archived Printer must not appear in printer_statuses"
    );
    expect_no_call(&rt.calls);
}

// --- 4. Rollback ---------------------------------------------------------------

#[test]
fn a_failing_disposition_rolls_back_the_whole_archive() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);
    let before_movements = movement_count(&env.storage);
    let before_p = printer(&env.storage, &farm.p.id);
    let stale = vec![
        to_storage(&env.storage, &farm.a, Some("Shelf")),
        // Q1 actually holds D.
        to_slot(&env.storage, &farm.b, &farm.q1, None, None),
        mark_empty(&env.storage, &farm.c),
    ];

    let error = invoke(
        &rt.webview,
        "archive_printer",
        archive_body(&env.storage, &farm.p.id, "op-stale", &stale),
    )
    .unwrap_err();

    assert_eq!(error["code"], "CONFLICT");
    assert_eq!(error["details"]["slotId"], json!(farm.q1));
    let after_p = printer(&env.storage, &farm.p.id);
    assert!(after_p.archived_at.is_none());
    assert_eq!(after_p.revision, before_p.revision);
    for (spool_id, slot_id) in [
        (&farm.a, &farm.p_slots[0]),
        (&farm.b, &farm.p_slots[1]),
        (&farm.c, &farm.p_slots[2]),
    ] {
        let stored = spool(&env.storage, spool_id);
        assert_eq!(stored.slot_id.as_deref(), Some(slot_id.as_str()));
        assert_eq!(stored.lifecycle, SpoolLifecycle::Active);
    }
    assert_eq!(ledger_history(&env.storage, &farm.c).len(), 1);
    assert_eq!(movement_count(&env.storage), before_movements);
}

// --- 5. Slot on the same or an archived Printer --------------------------------

#[test]
fn a_slot_disposition_into_the_archiving_printer_itself_is_a_validation_error() {
    let env = env();
    let farm = farm(&env.storage);
    let own_slot = vec![
        to_storage(&env.storage, &farm.a, Some("Shelf")),
        // P1 is A's slot; A is leaving it in the same operation.
        to_slot(&env.storage, &farm.b, &farm.p_slots[0], Some(&farm.a), None),
        mark_empty(&env.storage, &farm.c),
    ];

    let error = archive_repo(&env.storage, &farm.p.id, "op-own", &own_slot).unwrap_err();

    assert!(matches!(
        error,
        RepositoryError::Validation {
            field_path: "spoolDispositions"
        }
    ));
    assert!(printer(&env.storage, &farm.p.id).archived_at.is_none());
}

#[test]
fn a_slot_disposition_into_an_archived_printer_is_a_validation_error() {
    let env = env();
    let farm = farm(&env.storage);
    archive_repo(&env.storage, &farm.r.id, "op-archive-r", &[]).unwrap();
    let r1 = farm.r.material_slots[0].id.clone();
    let into_archived = vec![
        to_storage(&env.storage, &farm.a, Some("Shelf")),
        to_slot(&env.storage, &farm.b, &r1, None, None),
        mark_empty(&env.storage, &farm.c),
    ];

    let error = archive_repo(&env.storage, &farm.p.id, "op-into-r", &into_archived).unwrap_err();

    assert!(matches!(
        error,
        RepositoryError::Validation {
            field_path: "spoolDispositions"
        }
    ));
    assert!(printer(&env.storage, &farm.p.id).archived_at.is_none());
    assert_eq!(
        spool(&env.storage, &farm.b).slot_id.as_deref(),
        Some(farm.p_slots[1].as_str())
    );
}

// --- 6. Restart ----------------------------------------------------------------

#[test]
fn an_archived_printer_keeps_its_slots_and_every_spool_keeps_its_history_across_a_restart() {
    let Env {
        temp,
        lease,
        storage,
    } = env();
    let farm = farm(&storage);
    let dispositions = happy_dispositions(&storage, &farm);
    archive_repo(&storage, &farm.p.id, "op-archive", &dispositions).unwrap();
    let slots_before = printer(&storage, &farm.p.id).material_slots;
    let ids = [&farm.a, &farm.b, &farm.c, &farm.d];
    let histories_before: Vec<Vec<SpoolMovement>> = ids
        .iter()
        .map(|id| movement_history(&storage, id))
        .collect();
    drop(storage);

    let reopened = Arc::new(Storage::open(paths(&temp), &lease).unwrap());

    let archived = printer(&reopened, &farm.p.id);
    assert!(archived.archived_at.is_some());
    assert_eq!(archived.material_slots, slots_before);
    assert_eq!(
        archived
            .material_slots
            .iter()
            .map(|s| s.name.as_str())
            .collect::<Vec<_>>(),
        vec!["P1", "P2", "P3"]
    );
    let histories_after: Vec<Vec<SpoolMovement>> = ids
        .iter()
        .map(|id| movement_history(&reopened, id))
        .collect();
    assert_eq!(histories_after, histories_before);
    // B's history still includes its load into P2 and its move out of it.
    let b_history = &histories_after[1];
    assert!(b_history
        .iter()
        .any(|row| row.to.slot_id.as_deref() == Some(farm.p_slots[1].as_str())));
    assert!(b_history.iter().any(|row| row.from.slot_id.as_deref()
        == Some(farm.p_slots[1].as_str())
        && row.reason == MovementReason::PrinterArchived));
}

// --- 7. Delete cascade ---------------------------------------------------------

#[test]
fn deleting_an_archived_printer_cascades_its_slot_movements_and_leaves_every_spool_intact() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);
    let reservation = reserve(&env.storage, &farm.a);
    let dispositions = happy_dispositions(&env.storage, &farm);
    archive_repo(&env.storage, &farm.p.id, "op-archive", &dispositions).unwrap();
    let ids = [&farm.a, &farm.b, &farm.c, &farm.d];
    let spools_before: Vec<StoredSpool> = ids.iter().map(|id| spool(&env.storage, id)).collect();
    let ledgers_before: Vec<Vec<AmountEvent>> = ids
        .iter()
        .map(|id| ledger_history(&env.storage, id))
        .collect();

    invoke(
        &rt.webview,
        "delete_printer",
        json!({
            "contractVersion": 1,
            "id": farm.p.id,
            "expectedRevision": printer(&env.storage, &farm.p.id).revision,
        }),
    )
    .unwrap();

    let touching_p: i64 = env
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM spool_movements
                 WHERE from_slot_id IN (?1, ?2, ?3) OR to_slot_id IN (?1, ?2, ?3)",
                [&farm.p_slots[0], &farm.p_slots[1], &farm.p_slots[2]],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(touching_p, 0);
    // B's archive row (P2 -> Q1) went with P; D's displacement out of Q1 stays.
    let b_history = movement_history(&env.storage, &farm.b);
    assert!(b_history.is_empty(), "{b_history:?}");
    assert!(movement_history(&env.storage, &farm.d)
        .iter()
        .any(|row| row.reason == MovementReason::Displaced));

    let spools_after: Vec<StoredSpool> = ids.iter().map(|id| spool(&env.storage, id)).collect();
    assert_eq!(spools_after, spools_before);
    let ledgers_after: Vec<Vec<AmountEvent>> = ids
        .iter()
        .map(|id| ledger_history(&env.storage, id))
        .collect();
    assert_eq!(ledgers_after, ledgers_before);
    let open = env
        .storage
        .write_repo(|tx| Ok(reservations::open_reservations(tx, &farm.a)?))
        .unwrap();
    assert_eq!(
        open.iter().map(|r| r.id.as_str()).collect::<Vec<_>>(),
        vec![reservation.as_str()]
    );

    let violations: i64 = env
        .storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM pragma_foreign_key_check", [], |row| {
                row.get(0)
            })
        })
        .unwrap();
    assert_eq!(violations, 0);
}

// --- 8. Reservation guards and Spool lifecycle ---------------------------------

fn lifecycle(
    storage: &Storage,
    spool_id: &str,
    action: SpoolLifecycleAction,
    storage_label: Option<&str>,
    operation_id: &str,
) -> Result<movement::MoveOutcome, RepositoryError> {
    let revision = spool_revision(storage, spool_id);
    storage.write_repo(|tx| {
        apply_lifecycle(tx, spool_id, revision, action, storage_label, operation_id)
    })
}

fn expect_blocked(
    result: Result<movement::MoveOutcome, RepositoryError>,
    code: LifecycleBlockerCode,
) {
    match result {
        Err(RepositoryError::LifecycleBlocked(blockers)) => {
            assert_eq!(blocker_codes(&blockers), vec![code]);
        }
        other => panic!("expected LifecycleBlocked({code:?}), got {other:?}"),
    }
}

#[test]
fn archiving_or_marking_empty_a_reserved_spool_is_blocked_by_its_reservation() {
    let env = env();
    let reserved = new_spool(&env.storage);
    reserve(&env.storage, &reserved.id);

    expect_blocked(
        lifecycle(
            &env.storage,
            &reserved.id,
            SpoolLifecycleAction::Archive,
            None,
            "op-1",
        ),
        LifecycleBlockerCode::SpoolReserved,
    );
    expect_blocked(
        lifecycle(
            &env.storage,
            &reserved.id,
            SpoolLifecycleAction::MarkEmpty,
            None,
            "op-2",
        ),
        LifecycleBlockerCode::SpoolReserved,
    );

    let after = spool(&env.storage, &reserved.id);
    assert_eq!(after.lifecycle, SpoolLifecycle::Active);
    assert_eq!(after.revision, reserved.revision);
    assert_eq!(ledger_history(&env.storage, &reserved.id).len(), 1);
}

#[test]
fn a_mark_empty_disposition_on_a_reserved_spool_blocks_the_whole_archive() {
    let env = env();
    let farm = farm(&env.storage);
    reserve(&env.storage, &farm.c);
    let dispositions = happy_dispositions(&env.storage, &farm);

    let error = archive_repo(&env.storage, &farm.p.id, "op-archive", &dispositions).unwrap_err();

    match error {
        RepositoryError::LifecycleBlocked(blockers) => {
            assert_eq!(
                blocker_codes(&blockers),
                vec![LifecycleBlockerCode::SpoolReserved]
            );
            assert_eq!(blockers[0].action, LifecycleAction::Archive);
        }
        other => panic!("expected SPOOL_RESERVED, got {other:?}"),
    }
    assert!(printer(&env.storage, &farm.p.id).archived_at.is_none());
    assert_eq!(
        spool(&env.storage, &farm.a).slot_id.as_deref(),
        Some(farm.p_slots[0].as_str())
    );
    assert!(movements_of_operation(&env.storage, "op-archive").is_empty());
}

#[test]
fn mark_empty_on_a_loaded_spool_unloads_it_with_a_consumed_movement_and_a_ledger_row() {
    let env = env();
    let farm = farm(&env.storage);
    let before_p = printer(&env.storage, &farm.p.id).revision;

    let outcome = lifecycle(
        &env.storage,
        &farm.a,
        SpoolLifecycleAction::MarkEmpty,
        Some("Empties"),
        "op-empty",
    )
    .unwrap();

    let a = spool(&env.storage, &farm.a);
    assert_eq!(a.lifecycle, SpoolLifecycle::Empty);
    assert_eq!(
        (a.slot_id.as_deref(), a.storage_label.as_deref()),
        (None, Some("Empties"))
    );
    assert_eq!(a.current_mg, 0);
    assert_eq!(outcome.spool_ids, vec![farm.a.clone()]);
    assert_eq!(outcome.printer_ids, vec![farm.p.id.clone()]);
    assert_eq!(outcome.movements.len(), 1);
    assert_eq!(outcome.movements[0].reason, MovementReason::Consumed);
    assert_eq!(printer(&env.storage, &farm.p.id).revision, before_p + 1);
    let events = ledger_history(&env.storage, &farm.a);
    assert_eq!(events.last().unwrap().kind, AmountEventKind::MarkedEmpty);

    // Replaying the operation writes nothing more.
    let replay = env
        .storage
        .write_repo(|tx| {
            apply_lifecycle(
                tx,
                &farm.a,
                a.revision - 1,
                SpoolLifecycleAction::MarkEmpty,
                Some("Empties"),
                "op-empty",
            )
        })
        .unwrap();
    assert!(replay.replayed);
    assert_eq!(ledger_history(&env.storage, &farm.a).len(), events.len());
    assert_eq!(spool(&env.storage, &farm.a).revision, a.revision);
}

#[test]
fn mark_empty_on_a_storage_spool_writes_only_the_ledger_row() {
    let env = env();
    let stored = new_spool(&env.storage);

    let outcome = lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::MarkEmpty,
        None,
        "op-e",
    )
    .unwrap();

    assert!(outcome.movements.is_empty());
    assert_eq!(outcome.spool_ids, vec![stored.id.clone()]);
    let after = spool(&env.storage, &stored.id);
    assert_eq!(after.lifecycle, SpoolLifecycle::Empty);
    assert_eq!(after.revision, stored.revision + 1);
    assert_eq!(ledger_history(&env.storage, &stored.id).len(), 2);
}

#[test]
fn reactivate_archive_and_unarchive_follow_the_d9_transitions() {
    let env = env();
    let stored = new_spool(&env.storage);

    // active -> archived -> active (archived_from restores it).
    lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::Archive,
        None,
        "op-1",
    )
    .unwrap();
    let archived = spool(&env.storage, &stored.id);
    assert_eq!(archived.lifecycle, SpoolLifecycle::Archived);
    assert_eq!(archived.archived_from, Some(SpoolLifecycle::Active));
    lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::Unarchive,
        None,
        "op-2",
    )
    .unwrap();
    let restored = spool(&env.storage, &stored.id);
    assert_eq!(restored.lifecycle, SpoolLifecycle::Active);
    assert_eq!(restored.archived_from, None);

    // active -> empty -> archived -> empty -> active.
    lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::MarkEmpty,
        None,
        "op-3",
    )
    .unwrap();
    lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::Archive,
        None,
        "op-4",
    )
    .unwrap();
    assert_eq!(
        spool(&env.storage, &stored.id).archived_from,
        Some(SpoolLifecycle::Empty)
    );
    lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::Unarchive,
        None,
        "op-5",
    )
    .unwrap();
    assert_eq!(
        spool(&env.storage, &stored.id).lifecycle,
        SpoolLifecycle::Empty
    );
    lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::Reactivate,
        None,
        "op-6",
    )
    .unwrap();
    let reactivated = spool(&env.storage, &stored.id);
    assert_eq!(reactivated.lifecycle, SpoolLifecycle::Active);
    assert_eq!(reactivated.revision, stored.revision + 6);
}

#[test]
fn spool_lifecycle_actions_reject_the_wrong_starting_state_or_a_stale_revision() {
    let env = env();
    let farm = farm(&env.storage);
    let stored = new_spool(&env.storage);

    for action in [
        SpoolLifecycleAction::Reactivate,
        SpoolLifecycleAction::Unarchive,
    ] {
        let error = lifecycle(&env.storage, &stored.id, action, None, "op-x").unwrap_err();
        assert!(
            matches!(
                error,
                RepositoryError::Validation {
                    field_path: "action"
                }
            ),
            "{action:?}: {error:?}"
        );
    }

    // Archiving a loaded Spool is blocked until it is unloaded.
    expect_blocked(
        lifecycle(
            &env.storage,
            &farm.a,
            SpoolLifecycleAction::Archive,
            None,
            "op-y",
        ),
        LifecycleBlockerCode::SpoolsLoaded,
    );

    let stale = env
        .storage
        .write_repo(|tx| {
            apply_lifecycle(
                tx,
                &stored.id,
                stored.revision + 5,
                SpoolLifecycleAction::Archive,
                None,
                "op-z",
            )
        })
        .unwrap_err();
    assert!(
        matches!(stale, RepositoryError::Conflict { .. }),
        "{stale:?}"
    );
    assert_eq!(
        spool(&env.storage, &stored.id).lifecycle,
        SpoolLifecycle::Active
    );
}

// --- 9. No dispositions needed -------------------------------------------------

#[test]
fn archiving_an_empty_printer_with_no_dispositions_succeeds_as_in_p2() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);
    let before = printer(&env.storage, &farm.r.id);

    let response = invoke(
        &rt.webview,
        "archive_printer",
        archive_body(&env.storage, &farm.r.id, "op-r", &[]),
    )
    .unwrap();

    assert!(response["data"]["printer"]["archivedAt"].is_string());
    assert_eq!(
        response["data"]["printer"]["revision"],
        json!(before.revision + 1)
    );
    assert_eq!(
        response["data"]["printer"]["materialSlots"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    assert!(movements_of_operation(&env.storage, "op-r").is_empty());
}

// --- 10. The operations ledger ---------------------------------------------------

fn operation_count(storage: &Storage) -> i64 {
    storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM operations", [], |row| row.get(0))
        })
        .unwrap()
}

#[test]
fn a_move_operation_id_reused_for_a_spool_lifecycle_action_or_an_archive_is_rejected() {
    let env = env();
    let farm = farm(&env.storage);
    let stored = new_spool(&env.storage);
    env.storage
        .write_repo(|tx| {
            movement::move_spool(
                tx,
                "op-move",
                &stored.id,
                stored.revision,
                &MoveDestination::Storage {
                    storage_label: Some("Shelf".to_string()),
                },
            )
        })
        .unwrap();
    let moved = spool(&env.storage, &stored.id);
    let r_before = printer(&env.storage, &farm.r.id);

    let lifecycle_error = lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::Archive,
        None,
        "op-move",
    )
    .unwrap_err();
    let archive_error = archive_repo(&env.storage, &farm.r.id, "op-move", &[]).unwrap_err();

    assert!(
        matches!(lifecycle_error, RepositoryError::OperationIdReused),
        "{lifecycle_error:?}"
    );
    assert!(
        matches!(archive_error, RepositoryError::OperationIdReused),
        "{archive_error:?}"
    );
    assert_eq!(spool(&env.storage, &stored.id), moved);
    assert_eq!(printer(&env.storage, &farm.r.id), r_before);
}

#[test]
fn replaying_a_reactivate_succeeds_and_changes_the_spool_only_once() {
    let env = env();
    let stored = new_spool(&env.storage);
    lifecycle(
        &env.storage,
        &stored.id,
        SpoolLifecycleAction::MarkEmpty,
        None,
        "op-empty",
    )
    .unwrap();
    let empty = spool(&env.storage, &stored.id);
    let ledger_rows = ledger_history(&env.storage, &stored.id).len();
    let reactivate = || {
        env.storage.write_repo(|tx| {
            apply_lifecycle(
                tx,
                &stored.id,
                empty.revision,
                SpoolLifecycleAction::Reactivate,
                None,
                "op-reactivate",
            )
        })
    };

    let first = reactivate().unwrap();
    let after_first = spool(&env.storage, &stored.id);
    let replay = reactivate().unwrap();

    assert!(!first.replayed);
    assert!(replay.replayed);
    assert_eq!(replay.spool_ids, vec![stored.id.clone()]);
    assert!(replay.movements.is_empty());
    assert_eq!(after_first.lifecycle, SpoolLifecycle::Active);
    assert_eq!(after_first.revision, empty.revision + 1);
    assert_eq!(spool(&env.storage, &stored.id), after_first);
    assert_eq!(ledger_history(&env.storage, &stored.id).len(), ledger_rows);
    assert_eq!(operation_count(&env.storage), 2);
}

#[test]
fn replaying_an_archive_of_a_printer_with_no_loaded_spools_succeeds_instead_of_conflicting() {
    let env = env();
    let farm = farm(&env.storage);
    let rt = runtime(&env.storage);
    let body = archive_body(&env.storage, &farm.r.id, "op-archive-r", &[]);

    let first = invoke(&rt.webview, "archive_printer", body.clone()).unwrap();
    let replay = invoke(&rt.webview, "archive_printer", body).unwrap();

    assert!(first["data"]["printer"]["archivedAt"].is_string());
    assert_eq!(replay["data"]["printer"], first["data"]["printer"]);
    assert_eq!(
        printer(&env.storage, &farm.r.id).revision,
        farm.r.revision + 1
    );
}
