//! P3 Task 7: the inventory commands, their events, and the Tauri path
//! (spec D11, D13). Every command runs through the real IPC handler, and a
//! recording listener on `farm3d-event-v1` captures the inventory events.

mod common;

use std::sync::{Arc, Mutex};

use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::spools::events::InventoryChange;
use farm3d_lib::spools::slots::SlotSpec;
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;
use tokio::sync::broadcast::error::TryRecvError;

use common::{a_catalog, a_stored_printer, invoke};

// --- Fixture -------------------------------------------------------------------

struct Env {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Arc<Storage>,
    app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    events: Arc<Mutex<Vec<Value>>>,
    _credentials: tempfile::TempDir,
}

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

fn env() -> Env {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    let credentials = tempfile::tempdir().unwrap();
    let (app, webview, _manager, services) = common::runtime(
        tauri::generate_handler![
            farm3d_lib::spools::commands::list_spools,
            farm3d_lib::spools::commands::spool_history,
            farm3d_lib::spools::commands::create_spool,
            farm3d_lib::spools::commands::update_spool,
            farm3d_lib::spools::commands::record_spool_amount,
            farm3d_lib::spools::commands::move_spool,
            farm3d_lib::spools::commands::set_spool_lifecycle,
            farm3d_lib::spools::commands::create_tare,
            farm3d_lib::spools::commands::update_tare,
            farm3d_lib::spools::commands::delete_tare,
            farm3d_lib::spools::commands::debug_seed_reservation,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::set_material_slot_layout,
            farm3d_lib::printers::commands::archive_printer,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials.path().to_path_buf(),
        no_connection,
    );
    let events = Arc::new(Mutex::new(Vec::new()));
    let recorded = Arc::clone(&events);
    app.listen(STATUS_EVENT, move |event| {
        let value: Value = serde_json::from_str(event.payload()).unwrap();
        recorded.lock().unwrap().push(value);
    });
    Env {
        _temp: temp,
        _lease: lease,
        storage,
        app,
        webview,
        services,
        events,
        _credentials: credentials,
    }
}

impl Env {
    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        invoke(&self.webview, command, body).map(|response| response["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    fn err(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .expect_err(&format!("{command} should have failed"))
    }

    /// Inventory events recorded since the last call, then clears them.
    fn take_events(&self) -> Vec<Value> {
        std::mem::take(&mut *self.events.lock().unwrap())
            .into_iter()
            .filter(|event| {
                let kind = event["type"].as_str().unwrap_or_default();
                kind.starts_with("spool.") || kind == "printer.slots.changed"
            })
            .collect()
    }

    fn create_spool(&self) -> Value {
        self.ok(
            "create_spool",
            json!({
                "fields": fields(),
                "initialAmount": {"kind": "net", "netMg": 1_000_000, "confidence": "estimated"},
            }),
        )["spool"]
            .clone()
    }

    fn printer(&self, slots: &[&str]) -> StoredPrinter {
        let layout: Vec<SlotSpec> = slots
            .iter()
            .map(|name| SlotSpec {
                id: None,
                name: name.to_string(),
                feeder_label: None,
            })
            .collect();
        PrinterRepository::new(Arc::clone(&self.storage))
            .create_with_layout(a_stored_printer("prn-a"), None, &layout, &[])
            .unwrap()
    }

    fn load(&self, spool: &Value, slot_id: &str, expected_occupant: Option<&str>) -> Value {
        self.ok(
            "move_spool",
            json!({
                "operationId": uuid::Uuid::new_v4().to_string(),
                "spoolId": spool["id"],
                "expectedSpoolRevision": spool["revision"],
                "destination": {
                    "kind": "slot",
                    "slotId": slot_id,
                    "expectedOccupantSpoolId": expected_occupant,
                },
            }),
        )
    }

    fn current(&self, spool_id: &str) -> Value {
        let snapshot = self.ok("list_spools", json!({}));
        snapshot["spools"]
            .as_array()
            .unwrap()
            .iter()
            .find(|spool| spool["id"] == spool_id)
            .cloned()
            .unwrap()
    }
}

fn fields() -> Value {
    json!({
        "manufacturer": "Polymaker",
        "materialFamily": "PLA",
        "colorName": "Black",
        "diameter": "1.75",
        "nominalMg": 1_000_000,
        "lowThresholdMg": 100_000,
    })
}

fn count(events: &[Value], kind: &str) -> usize {
    events.iter().filter(|event| event["type"] == kind).count()
}

fn sorted(mut ids: Vec<String>) -> Vec<String> {
    ids.sort();
    ids
}

// --- 1. Create ------------------------------------------------------------------

#[test]
fn create_spool_returns_the_spool_and_emits_one_spool_changed_after_the_snapshot() {
    let env = env();
    let snapshot = env.ok("list_spools", json!({}));
    assert_eq!(snapshot["spools"], json!([]));
    assert_eq!(snapshot["tares"], json!([]));
    env.take_events();

    let result = env.ok(
        "create_spool",
        json!({
            "fields": fields(),
            "initialAmount": {"kind": "net", "netMg": 750_000, "confidence": "measured"},
            "storageLabel": "Shelf 1",
        }),
    );

    let spool = &result["spool"];
    assert_eq!(spool["spoolNumber"], json!(1));
    assert_eq!(spool["availability"]["currentMg"], json!(750_000));
    assert_eq!(
        spool["location"],
        json!({"kind": "storage", "storageLabel": "Shelf 1"})
    );
    assert_eq!(result["printers"], json!([]));
    assert_eq!(result["warnings"], json!([]));

    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    let changed = events
        .iter()
        .find(|event| event["type"] == "spool.changed")
        .unwrap();
    assert_eq!(changed["streamId"], snapshot["streamId"]);
    assert!(changed["sequence"].as_u64().unwrap() > snapshot["snapshotSequence"].as_u64().unwrap());
    assert_eq!(
        changed["subject"],
        json!({"kind": "spool", "id": spool["id"]})
    );
    assert_eq!(changed["payload"]["type"], json!("spoolChanged"));
    assert_eq!(changed["payload"]["spool"], *spool);

    // The next backfill's snapshot covers the event.
    let after = env.ok("list_spools", json!({}));
    assert!(after["snapshotSequence"].as_u64() >= changed["sequence"].as_u64());
    assert_eq!(after["spools"][0], *spool);
}

// --- 2. Move and replay ----------------------------------------------------------

#[test]
fn move_into_an_occupied_slot_emits_both_spools_the_printer_and_one_broadcast() {
    let env = env();
    let printer = env.printer(&["Main"]);
    let slot_id = printer.material_slots[0].id.clone();
    let a = env.create_spool();
    let b = env.create_spool();
    env.load(&a, &slot_id, None);
    env.take_events();
    let mut changes = env.services.inventory_changes.subscribe();

    let operation_id = uuid::Uuid::new_v4().to_string();
    let request = json!({
        "operationId": operation_id,
        "spoolId": b["id"],
        "expectedSpoolRevision": b["revision"],
        "destination": {
            "kind": "slot",
            "slotId": slot_id,
            "expectedOccupantSpoolId": a["id"],
            "displacedStorageLabel": "Bin",
        },
    });
    let result = env.ok("move_spool", request.clone());

    let spool_ids: Vec<&Value> = result["spools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|spool| &spool["id"])
        .collect();
    assert_eq!(spool_ids, vec![&b["id"], &a["id"]]);
    assert_eq!(result["spools"][0]["location"]["slotId"], json!(slot_id));
    assert_eq!(
        result["spools"][1]["location"],
        json!({"kind": "storage", "storageLabel": "Bin"})
    );
    assert_eq!(result["printers"].as_array().unwrap().len(), 1);
    assert_eq!(result["printers"][0]["id"], json!(printer.id));
    assert_eq!(
        result["printers"][0]["materialSlots"][0]["occupantSpoolId"],
        b["id"]
    );
    let reasons: Vec<&Value> = result["movements"]
        .as_array()
        .unwrap()
        .iter()
        .map(|movement| &movement["reason"])
        .collect();
    assert_eq!(reasons, vec!["displaced", "load"]);

    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 2);
    assert_eq!(count(&events, "printer.slots.changed"), 1);
    assert_eq!(count(&events, "spool.availability.changed"), 2);
    let slots_changed = events
        .iter()
        .find(|event| event["type"] == "printer.slots.changed")
        .unwrap();
    assert_eq!(
        slots_changed["subject"],
        json!({"kind": "printer", "id": printer.id})
    );
    assert_eq!(
        slots_changed["payload"]["revision"],
        result["printers"][0]["revision"]
    );
    assert_eq!(
        slots_changed["payload"]["materialSlots"],
        result["printers"][0]["materialSlots"]
    );
    let sequences: Vec<u64> = events
        .iter()
        .map(|event| event["sequence"].as_u64().unwrap())
        .collect();
    assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1));

    let change: InventoryChange = changes.try_recv().unwrap();
    assert_eq!(
        sorted(change.spool_ids),
        sorted(vec![
            a["id"].as_str().unwrap().to_string(),
            b["id"].as_str().unwrap().to_string()
        ])
    );
    assert_eq!(change.printer_ids, vec![printer.id.clone()]);
    assert!(matches!(changes.try_recv(), Err(TryRecvError::Empty)));

    // Replaying the same operationId returns the recorded outcome and emits
    // nothing.
    let replay = env.ok("move_spool", request);
    assert_eq!(replay["spools"], result["spools"]);
    assert_eq!(replay["movements"], result["movements"]);
    assert!(env.take_events().is_empty());
    assert!(matches!(changes.try_recv(), Err(TryRecvError::Empty)));
}

#[test]
fn reusing_an_operation_id_for_a_different_spool_is_a_validation_error() {
    let env = env();
    let printer = env.printer(&["Main"]);
    let slot_id = printer.material_slots[0].id.clone();
    let a = env.create_spool();
    let b = env.create_spool();
    let operation_id = uuid::Uuid::new_v4().to_string();
    env.ok(
        "move_spool",
        json!({
            "operationId": operation_id,
            "spoolId": a["id"],
            "expectedSpoolRevision": a["revision"],
            "destination": {"kind": "slot", "slotId": slot_id, "expectedOccupantSpoolId": null},
        }),
    );
    env.take_events();

    let error = env.err(
        "move_spool",
        json!({
            "operationId": operation_id,
            "spoolId": b["id"],
            "expectedSpoolRevision": b["revision"],
            "destination": {"kind": "storage", "storageLabel": "Shelf"},
        }),
    );

    assert_eq!(error["code"], json!("VALIDATION"));
    assert_eq!(error["details"]["fieldPath"], json!("operationId"));
    assert_eq!(
        error["message"],
        json!("operationId was already used for a different request")
    );
    assert!(env.take_events().is_empty());
    // B did not move.
    assert_eq!(
        env.current(b["id"].as_str().unwrap())["revision"],
        b["revision"]
    );
}

// --- 3. Failures emit nothing ----------------------------------------------------

#[test]
fn a_conflicting_move_emits_no_events_and_broadcasts_nothing() {
    let env = env();
    let printer = env.printer(&["Main"]);
    let slot_id = printer.material_slots[0].id.clone();
    let a = env.create_spool();
    let b = env.create_spool();
    env.load(&a, &slot_id, None);
    env.take_events();
    let mut changes = env.services.inventory_changes.subscribe();

    // B expects the slot to be empty, but A is in it.
    let error = env.err(
        "move_spool",
        json!({
            "operationId": uuid::Uuid::new_v4().to_string(),
            "spoolId": b["id"],
            "expectedSpoolRevision": b["revision"],
            "destination": {"kind": "slot", "slotId": slot_id, "expectedOccupantSpoolId": null},
        }),
    );

    assert_eq!(error["code"], json!("CONFLICT"));
    assert_eq!(error["details"]["currentOccupantSpoolId"], a["id"]);
    assert!(env.take_events().is_empty());
    assert!(matches!(changes.try_recv(), Err(TryRecvError::Empty)));
}

// --- 4. Error shapes -------------------------------------------------------------

#[test]
fn a_scale_entry_below_its_tare_is_a_validation_error_on_entry_gross_mg() {
    let env = env();
    let spool = env.create_spool();
    env.take_events();

    let error = env.err(
        "record_spool_amount",
        json!({
            "id": spool["id"],
            "expectedRevision": spool["revision"],
            "entry": {"kind": "scale", "grossMg": 100_000, "tareMg": 200_000},
        }),
    );

    assert_eq!(error["code"], json!("VALIDATION"));
    assert_eq!(error["details"]["fieldPath"], json!("entry.grossMg"));
    assert!(env.take_events().is_empty());
}

#[test]
fn removing_an_occupied_slot_is_slot_occupied_with_the_spool_id() {
    let env = env();
    let printer = env.printer(&["Left", "Right"]);
    let a = env.create_spool();
    env.load(&a, &printer.material_slots[1].id, None);
    let revision = PrinterRepository::new(Arc::clone(&env.storage))
        .get(&printer.id)
        .unwrap()
        .unwrap()
        .revision;
    env.take_events();

    let error = env.err(
        "set_material_slot_layout",
        json!({
            "printerId": printer.id,
            "expectedRevision": revision,
            "slots": [{"id": printer.material_slots[0].id, "name": "Left"}],
        }),
    );

    assert_eq!(error["code"], json!("SLOT_OCCUPIED"));
    assert_eq!(error["details"]["spoolId"], a["id"]);
    assert_eq!(
        error["details"]["slotId"],
        json!(printer.material_slots[1].id)
    );
    assert!(env.take_events().is_empty());
}

// --- 5. Lifecycle ----------------------------------------------------------------

#[test]
fn mark_empty_on_a_loaded_spool_unloads_it_and_returns_the_printer() {
    let env = env();
    let printer = env.printer(&["Main"]);
    let a = env.create_spool();
    env.load(&a, &printer.material_slots[0].id, None);
    let a = env.current(a["id"].as_str().unwrap());
    env.take_events();

    let result = env.ok(
        "set_spool_lifecycle",
        json!({
            "operationId": uuid::Uuid::new_v4().to_string(),
            "id": a["id"],
            "expectedRevision": a["revision"],
            "action": "markEmpty",
            "storageLabel": "Empties",
        }),
    );

    assert_eq!(result["spool"]["lifecycle"], json!("empty"));
    assert_eq!(result["spool"]["availability"]["currentMg"], json!(0));
    assert_eq!(
        result["spool"]["location"],
        json!({"kind": "storage", "storageLabel": "Empties"})
    );
    assert_eq!(result["printers"].as_array().unwrap().len(), 1);
    assert_eq!(result["printers"][0]["id"], json!(printer.id));
    assert!(result["printers"][0]["materialSlots"][0]
        .get("occupantSpoolId")
        .is_none());

    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    assert_eq!(count(&events, "printer.slots.changed"), 1);
    assert_eq!(count(&events, "spool.availability.changed"), 1);

    // Reactivating an in-storage Spool touches no Printer.
    let result = env.ok(
        "set_spool_lifecycle",
        json!({
            "operationId": uuid::Uuid::new_v4().to_string(),
            "id": a["id"],
            "expectedRevision": result["spool"]["revision"],
            "action": "reactivate",
        }),
    );
    assert_eq!(result["spool"]["lifecycle"], json!("active"));
    assert_eq!(result["printers"], json!([]));
}

/// Final follow-ups review, minor: a command-level check that a
/// `set_spool_lifecycle` replay (same `operationId`, same request) is
/// silent -- no `spool.changed`/`spool.availability.changed` events, on top
/// of the repository-level replay coverage in `p3_lifecycle.rs`.
#[test]
fn replaying_a_set_spool_lifecycle_operation_id_emits_no_events() {
    let env = env();
    let a = env.create_spool();
    env.take_events();

    let body = json!({
        "operationId": uuid::Uuid::new_v4().to_string(),
        "id": a["id"],
        "expectedRevision": a["revision"],
        "action": "markEmpty",
        "storageLabel": "Empties",
    });

    let first = env.ok("set_spool_lifecycle", body.clone());
    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    assert_eq!(count(&events, "spool.availability.changed"), 1);

    let replay = env.ok("set_spool_lifecycle", body);
    assert_eq!(replay, first);
    assert!(env.take_events().is_empty());
}

// --- Other commands --------------------------------------------------------------

#[test]
fn update_and_record_amount_return_the_spool_and_history_lists_every_row() {
    let env = env();
    let spool = env.create_spool();

    let mut patch = fields();
    patch["colorName"] = json!("Galaxy Blue");
    let updated = env.ok(
        "update_spool",
        json!({"id": spool["id"], "expectedRevision": spool["revision"], "patch": patch}),
    );
    assert_eq!(updated["spool"]["colorName"], json!("Galaxy Blue"));
    assert_eq!(updated["spool"]["revision"], json!(2));

    let tare = env.ok(
        "create_tare",
        json!({"name": "Bambu reel", "weightMg": 250_000}),
    )["tare"]
        .clone();
    env.take_events();
    let recorded = env.ok(
        "record_spool_amount",
        json!({
            "id": spool["id"],
            "expectedRevision": 2,
            "entry": {"kind": "scale", "grossMg": 862_300, "tareId": tare["id"]},
            "note": " weighed ",
        }),
    );
    assert_eq!(
        recorded["spool"]["availability"]["currentMg"],
        json!(612_300)
    );
    assert_eq!(recorded["spool"]["facets"]["confidence"], json!("measured"));
    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    assert_eq!(count(&events, "spool.availability.changed"), 1);

    let stale = env.err(
        "record_spool_amount",
        json!({
            "id": spool["id"],
            "expectedRevision": 2,
            "entry": {"kind": "net", "netMg": 1, "confidence": "estimated"},
        }),
    );
    assert_eq!(stale["code"], json!("CONFLICT"));

    let history = env.ok("spool_history", json!({"spoolId": spool["id"]}));
    let kinds: Vec<&Value> = history["amountEvents"]
        .as_array()
        .unwrap()
        .iter()
        .map(|event| &event["kind"])
        .collect();
    assert_eq!(kinds, vec!["initial", "measurement"]);
    assert_eq!(history["amountEvents"][1]["note"], json!("weighed"));
    assert_eq!(history["amountEvents"][1]["tareMg"], json!(250_000));
    assert_eq!(history["movements"], json!([]));
    assert_eq!(history["reservations"], json!([]));

    let missing = env.err("spool_history", json!({"spoolId": "spl-missing"}));
    assert_eq!(missing["code"], json!("NOT_FOUND"));
}

#[test]
fn tare_commands_are_revisioned_and_delete_emits_the_spools_that_used_it() {
    let env = env();
    let created =
        env.ok("create_tare", json!({"name": "Reel", "weightMg": 200_000}))["tare"].clone();
    assert_eq!(created["revision"], json!(1));

    let updated = env.ok(
        "update_tare",
        json!({"id": created["id"], "expectedRevision": 1, "name": "Cardboard reel", "weightMg": 180_000}),
    )["tare"]
        .clone();
    assert_eq!(updated["revision"], json!(2));
    assert_eq!(updated["weightMg"], json!(180_000));

    let conflict = env.err(
        "update_tare",
        json!({"id": created["id"], "expectedRevision": 1, "name": "X", "weightMg": 1}),
    );
    assert_eq!(conflict["code"], json!("CONFLICT"));

    let mut with_tare = fields();
    with_tare["tareId"] = created["id"].clone();
    let spool = env.ok(
        "create_spool",
        json!({
            "fields": with_tare,
            "initialAmount": {"kind": "net", "netMg": 1_000_000, "confidence": "estimated"},
        }),
    )["spool"]
        .clone();
    let snapshot = env.ok("list_spools", json!({}));
    assert_eq!(snapshot["tares"].as_array().unwrap().len(), 1);
    env.take_events();

    let deleted = env.ok(
        "delete_tare",
        json!({"id": created["id"], "expectedRevision": 2}),
    );
    assert_eq!(deleted["tare"]["id"], created["id"]);

    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    let changed = &events
        .iter()
        .find(|event| event["type"] == "spool.changed")
        .unwrap()["payload"]["spool"];
    assert_eq!(changed["id"], spool["id"]);
    assert!(changed.get("tareId").is_none());
    assert!(changed["revision"].as_i64() > spool["revision"].as_i64());
    assert_eq!(env.ok("list_spools", json!({}))["tares"], json!([]));
}

// --- Printer commands (ruling R2) ------------------------------------------------

#[test]
fn slot_layout_create_with_loads_and_archive_emit_inventory_events_after_commit() {
    let env = env();
    let a = env.create_spool();
    let b = env.create_spool();
    env.take_events();
    let mut changes = env.services.inventory_changes.subscribe();

    // create_printer with an initial load.
    let created = env.ok(
        "create_printer",
        json!({
            "name": "Loaded",
            "catalogRef": common::a_ref_json(),
            "slotLayout": [{"name": "Main"}],
            "initialLoads": [{"slotIndex": 0, "spoolId": a["id"], "expectedSpoolRevision": a["revision"]}],
        }),
    )["printer"]
        .clone();
    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    assert_eq!(count(&events, "printer.slots.changed"), 1);
    assert_eq!(count(&events, "spool.availability.changed"), 1);
    let change = changes.try_recv().unwrap();
    assert_eq!(
        change.spool_ids,
        vec![a["id"].as_str().unwrap().to_string()]
    );
    assert_eq!(
        change.printer_ids,
        vec![created["id"].as_str().unwrap().to_string()]
    );

    // set_material_slot_layout changes the Printer's slots only.
    let relaid = env.ok(
        "set_material_slot_layout",
        json!({
            "printerId": created["id"],
            "expectedRevision": created["revision"],
            "slots": [{"id": created["materialSlots"][0]["id"], "name": "Main"}, {"name": "Aux"}],
        }),
    )["printer"]
        .clone();
    let events = env.take_events();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["type"], json!("printer.slots.changed"));
    assert_eq!(
        events[0]["payload"]["materialSlots"],
        relaid["materialSlots"]
    );
    assert_eq!(changes.try_recv().unwrap().printer_ids.len(), 1);

    // A failed create (B is fine, but slot 5 doesn't exist) emits nothing.
    let failed = env.err(
        "create_printer",
        json!({
            "name": "Broken",
            "catalogRef": common::a_ref_json(),
            "initialLoads": [{"slotIndex": 5, "spoolId": b["id"], "expectedSpoolRevision": b["revision"]}],
        }),
    );
    assert_eq!(failed["code"], json!("VALIDATION"));
    assert!(env.take_events().is_empty());
    assert!(matches!(changes.try_recv(), Err(TryRecvError::Empty)));

    // archive_printer relocates A and reports it.
    let a_now = env.current(a["id"].as_str().unwrap());
    let archived = env.ok(
        "archive_printer",
        json!({
            "id": created["id"],
            "expectedRevision": relaid["revision"],
            "operationId": uuid::Uuid::new_v4().to_string(),
            "spoolDispositions": [{
                "spoolId": a["id"],
                "expectedSpoolRevision": a_now["revision"],
                "disposition": {"kind": "storage", "storageLabel": "Shelf"},
            }],
        }),
    )["printer"]
        .clone();
    assert!(archived["archivedAt"].is_string());
    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    assert_eq!(count(&events, "printer.slots.changed"), 1);
    assert_eq!(count(&events, "spool.availability.changed"), 1);
    let spool_event = events
        .iter()
        .find(|event| event["type"] == "spool.changed")
        .unwrap();
    assert_eq!(
        spool_event["payload"]["spool"]["location"],
        json!({"kind": "storage", "storageLabel": "Shelf"})
    );
    let change = changes.try_recv().unwrap();
    assert_eq!(
        change.spool_ids,
        vec![a["id"].as_str().unwrap().to_string()]
    );
}

/// Final follow-ups review, Important: a replay of `archive_printer` with a
/// loaded Spool (exercising the `find_operation` branch in
/// `archive_with_outcome`, not just the no-Spools empty-outcome branch)
/// must be silent end to end -- not only no inventory events, but also no
/// re-publish of the supervisor's `printer.status.removed` tombstone
/// (`printers::setup::supervise_persisted`/`supervise_printer`'s
/// unconditional `manager.stop()` for an archived Printer), and no further
/// write.
#[test]
fn replaying_an_archive_with_loaded_spools_emits_no_events_and_writes_nothing() {
    let env = env();
    let printer = env.printer(&["Main"]);
    let a = env.create_spool();
    let loaded = env.load(&a, &printer.material_slots[0].id, None);
    let printer_revision = loaded["printers"][0]["revision"].as_i64().unwrap();
    let a_loaded = env.current(a["id"].as_str().unwrap());
    env.take_events();

    let body = json!({
        "id": printer.id,
        "expectedRevision": printer_revision,
        "operationId": uuid::Uuid::new_v4().to_string(),
        "spoolDispositions": [{
            "spoolId": a["id"],
            "expectedSpoolRevision": a_loaded["revision"],
            "disposition": {"kind": "storage", "storageLabel": "Shelf"},
        }],
    });

    let first = env.ok("archive_printer", body.clone());
    assert!(first["printer"]["archivedAt"].is_string());
    let events = env.take_events();
    assert_eq!(count(&events, "spool.changed"), 1);
    assert_eq!(count(&events, "printer.slots.changed"), 1);
    // `take_events` drains every recorded event, filtered or not, so this
    // also clears the first (non-replay) archive's own
    // `printer.status.removed` tombstone -- only the replay's silence is
    // under test below.
    let a_after_first = env.current(a["id"].as_str().unwrap());
    let printer_after_first = PrinterRepository::new(Arc::clone(&env.storage))
        .get(printer.id.as_str())
        .unwrap()
        .unwrap();

    let replay = env.ok("archive_printer", body);

    assert_eq!(replay["printer"], first["printer"]);
    assert_eq!(env.current(a["id"].as_str().unwrap()), a_after_first);
    assert_eq!(
        PrinterRepository::new(Arc::clone(&env.storage))
            .get(printer.id.as_str())
            .unwrap()
            .unwrap()
            .revision,
        printer_after_first.revision
    );
    assert!(
        env.events.lock().unwrap().is_empty(),
        "a replay must publish no events at all, including the supervisor's"
    );
}

/// `publish_ids` still broadcasts the ids when the post-commit record read
/// fails (here, an id with no row): P7's evaluator reloads from ids, so it
/// must not miss a committed change. Only the record-bearing events are
/// skipped.
#[test]
fn publish_ids_broadcasts_the_ids_even_when_the_record_read_fails() {
    let env = env();
    env.take_events();
    let mut changes = env.services.inventory_changes.subscribe();
    let spool_ids = vec!["spl-missing".to_string()];
    let printer_ids = vec!["prn-missing".to_string()];

    farm3d_lib::spools::events::publish_ids(
        env.app.handle(),
        &env.services,
        &spool_ids,
        &printer_ids,
    );

    assert!(env.take_events().is_empty());
    assert_eq!(
        changes.try_recv().unwrap(),
        InventoryChange {
            spool_ids,
            printer_ids,
        }
    );
}

// --- Debug fixture (D8) ----------------------------------------------------------

#[cfg(debug_assertions)]
#[test]
fn debug_seed_reservation_reserves_and_reports_the_spool() {
    let env = env();
    let spool = env.create_spool();
    env.take_events();

    let result = env.ok(
        "debug_seed_reservation",
        json!({"spoolId": spool["id"], "amountMg": 200_000}),
    );

    assert_eq!(result["spool"]["facets"]["reserved"], json!(true));
    assert_eq!(
        result["spool"]["availability"]["reservedMg"],
        json!(200_000)
    );
    assert_eq!(
        result["spool"]["availability"]["availableMg"],
        json!(800_000)
    );
    assert_eq!(count(&env.take_events(), "spool.availability.changed"), 1);
    let history = env.ok("spool_history", json!({"spoolId": spool["id"]}));
    assert_eq!(history["reservations"][0]["amountMg"], json!(200_000));
    assert_eq!(history["reservations"][0]["state"], json!("active"));

    let too_much = env.err(
        "debug_seed_reservation",
        json!({"spoolId": spool["id"], "amountMg": 900_000}),
    );
    assert_eq!(too_much["code"], json!("VALIDATION"));
    assert_eq!(too_much["details"]["fieldPath"], json!("amountMg"));
}

// --- 6. Contracts ----------------------------------------------------------------

const INVENTORY_COMMANDS: [&str; 10] = [
    "list_spools",
    "spool_history",
    "create_spool",
    "update_spool",
    "record_spool_amount",
    "move_spool",
    "set_spool_lifecycle",
    "create_tare",
    "update_tare",
    "delete_tare",
];

#[test]
fn every_inventory_command_is_registered_with_a_contract() {
    assert_eq!(farm3d_lib::COMMAND_NAMES.len(), 55);
    let manifest = farm3d_lib::contracts::inventory::command_contract_inventory();
    for command in INVENTORY_COMMANDS {
        assert!(
            farm3d_lib::COMMAND_NAMES.contains(&command),
            "{command} missing from COMMAND_NAMES"
        );
        assert!(
            manifest.iter().any(|entry| entry.command == command),
            "{command} missing from COMMAND_CONTRACTS"
        );
    }
    assert!(!farm3d_lib::COMMAND_NAMES.contains(&"debug_seed_reservation"));
    assert!(!manifest
        .iter()
        .any(|entry| entry.command == "debug_seed_reservation"));
}

#[test]
fn generated_contracts_include_every_inventory_type() {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/generated/contracts");
    for path in [
        "command/SpoolMutationResult.ts",
        "command/MoveSpoolData.ts",
        "command/TareMutationResult.ts",
        "command/SpoolHistory.ts",
        "domain/InventorySnapshot.ts",
        "domain/InventoryEvent.ts",
        "domain/InventoryEventType.ts",
        "domain/InventoryEventPayload.ts",
        "domain/SpoolLifecycleAction.ts",
        "domain/MoveDestination.ts",
        "domain/MovementReason.ts",
        "domain/SpoolLocationSnapshot.ts",
        "domain/SpoolMovement.ts",
        "domain/AmountEvent.ts",
        "domain/AmountEventKind.ts",
        "domain/AmountEntry.ts",
        "domain/SpoolFields.ts",
        "domain/Tare.ts",
        "domain/Reservation.ts",
        "domain/ReservationState.ts",
    ] {
        assert!(root.join(path).is_file(), "{path} was not generated");
    }
    let contracts = std::fs::read_to_string(root.join("command/CommandContracts.ts")).unwrap();
    for name in [
        "ListSpoolsRequest",
        "SpoolHistoryRequest",
        "CreateSpoolRequest",
        "UpdateSpoolRequest",
        "RecordSpoolAmountRequest",
        "MoveSpoolRequest",
        "SetSpoolLifecycleRequest",
        "CreateTareRequest",
        "UpdateTareRequest",
        "DeleteTareRequest",
    ] {
        assert!(
            contracts.contains(&format!("export type {name} =")),
            "{name} missing from CommandContracts.ts"
        );
    }
    let event = std::fs::read_to_string(root.join("domain/InventoryEvent.ts")).unwrap();
    assert!(event.contains("EventEnvelope<InventoryEventType, InventoryEventPayload>"));
}
