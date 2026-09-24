//! P3 Task 5: slot layouts in Printer reads, create, batch, and export (D4,
//! D12). See
//! `.superpowers/sdd/2026-09-23-p3-spools-material-slots/task-5-brief.md`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use common::{a_catalog, a_ref_json, invoke};
use farm3d_lib::connections::status_repository::StatusRepository;
use farm3d_lib::connections::supervisor::ConnectionManager;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::document_io::{DocumentIo, DocumentKind};
use farm3d_lib::persistence::{MetadataRootLease, Storage};
use farm3d_lib::spools::ledger::AmountEntry;
use farm3d_lib::spools::movement::{self, MoveDestination};
use farm3d_lib::spools::repository::{self, StoredSpool};
use farm3d_lib::spools::{AmountConfidence, FilamentDiameter, MaterialFamily, SpoolFields};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::{Manager, WebviewWindowBuilder};

// --- Fixtures ----------------------------------------------------------------

fn storage() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>) {
    let (temp, lease, storage, _db) = common::storage();
    (temp, lease, storage)
}

fn no_connection(
    _config: &ConnectionConfig,
    _secret: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

/// A deterministic, in-memory [`DocumentIo`] for the export/import tests:
/// `save`/`open` return whatever path the test pre-loads, `read` returns
/// whatever bytes the test pre-loads, and `atomic_write` records every call
/// rather than touching a real file.
#[derive(Default)]
struct InjectedDocuments {
    open: Mutex<Option<PathBuf>>,
    save: Mutex<Option<PathBuf>>,
    bytes: Mutex<Option<Vec<u8>>>,
    writes: Mutex<Vec<Vec<u8>>>,
}

impl DocumentIo for InjectedDocuments {
    fn open_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.open.lock().unwrap().clone())
    }
    fn save_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        Ok(self.save.lock().unwrap().clone())
    }
    fn read(&self, _path: &Path) -> Result<Vec<u8>, CommandError> {
        Ok(self.bytes.lock().unwrap().clone().unwrap_or_default())
    }
    fn atomic_write(&self, _path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        self.writes.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
}

/// This file's command set: `create_printer`, `set_material_slot_layout`,
/// `create_printers_batch`, `list_printers`, `export_printers`,
/// `import_printers`.
fn runtime(
    storage: Arc<Storage>,
    documents: Arc<InjectedDocuments>,
) -> (
    tauri::App<MockRuntime>,
    tauri::WebviewWindow<MockRuntime>,
    Arc<RuntimeServices<MockRuntime>>,
) {
    let app = tauri::test::mock_builder()
        .invoke_handler(tauri::generate_handler![
            farm3d_lib::printers::commands::list_printers,
            farm3d_lib::printers::commands::create_printer,
            farm3d_lib::printers::commands::set_material_slot_layout,
            farm3d_lib::printers::commands::export_printers,
            farm3d_lib::printers::commands::import_printers,
            farm3d_lib::printers::batch::create_printers_batch,
        ])
        .build(tauri::test::mock_context(tauri::test::noop_assets()))
        .unwrap();
    let catalog = Arc::new(a_catalog());
    let manager = Arc::new(ConnectionManager::with_clock_and_factory(
        app.handle().clone(),
        Arc::new(StatusRepository::new(Arc::clone(&storage))),
        chrono::Utc::now,
        no_connection,
    ));
    let documents: Arc<dyn DocumentIo> = documents;
    let services = Arc::new(RuntimeServices::for_test(
        Arc::clone(&storage),
        catalog,
        manager,
        documents,
    ));
    app.manage(farm3d_lib::bootstrap::BootstrapState::ready_with(
        Arc::clone(&services),
    ));
    let webview = WebviewWindowBuilder::new(&app, "main", Default::default())
        .build()
        .unwrap();
    (app, webview, services)
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

/// A fresh, active Spool in storage (no slot).
fn a_storage_spool(storage: &Storage) -> StoredSpool {
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    storage
        .write_repo(|tx| repository::insert_spool(tx, &valid_fields(), &entry, None))
        .unwrap()
}

fn spool(storage: &Storage, id: &str) -> StoredSpool {
    storage
        .write_repo(|tx| Ok(repository::load_spool(tx, id)?.unwrap()))
        .unwrap()
}

fn create_printer_body(name: &str, extra: Value) -> Value {
    let mut body = json!({
        "contractVersion": 1,
        "name": name,
        "catalogRef": a_ref_json(),
    });
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    body
}

fn slot_spec(name: &str, feeder_label: Option<&str>) -> Value {
    let mut spec = json!({ "name": name });
    if let Some(label) = feeder_label {
        spec["feederLabel"] = json!(label);
    }
    spec
}

fn material_slots<'a>(printer: &'a Value) -> &'a Vec<Value> {
    printer["materialSlots"].as_array().unwrap()
}

fn error_of(result: Result<Value, Value>) -> Value {
    result.expect_err("expected an error response")
}

// --- 1. Default layout -------------------------------------------------------

#[test]
fn create_printer_without_a_layout_gets_the_default_main_slot() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _services) = runtime(storage, Arc::new(InjectedDocuments::default()));

    let response = invoke(
        &webview,
        "create_printer",
        create_printer_body("Bay Printer", json!({})),
    )
    .unwrap();

    let slots = material_slots(&response["data"]["printer"]);
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0]["name"], json!("Main"));
    assert!(slots[0]["id"].as_str().unwrap().starts_with("slt-"));
    assert_eq!(slots[0]["position"], json!(0));
    assert!(slots[0].get("occupantSpoolId").is_none());
}

// --- 2. Layout and initial loads ----------------------------------------------

/// Fix round 1, Critical 1: a create with SEVERAL initial loads must occupy
/// every targeted slot, not just the first — a bare `apply_move` per load
/// shares one `operationId`, so the second+ calls would find the first
/// load's row under that id and silently replay (no-op) instead of
/// applying. This exercises 3 loads and asserts every target slot ends up
/// occupied and every resulting movement row shares one `operationId`
/// (`spools::movement::apply_moves`, Fix round 1).
///
/// Fix round 1, Important 1: `apply_moves` bumps the Printer's own revision
/// once per load, so the create response must return the POST-load
/// revision (not the just-inserted 1) — asserted here both directly and by
/// using it for a follow-up `set_material_slot_layout` call, which must
/// succeed rather than CONFLICT.
#[test]
fn create_printer_with_a_layout_and_several_initial_loads_occupies_every_target_slot() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _services) =
        runtime(Arc::clone(&storage), Arc::new(InjectedDocuments::default()));
    let spools: Vec<StoredSpool> = (0..3).map(|_| a_storage_spool(&storage)).collect();

    let layout = json!([
        slot_spec("1", Some("AMS 1")),
        slot_spec("2", Some("AMS 1")),
        slot_spec("3", Some("AMS 1")),
        slot_spec("4", Some("AMS 1")),
    ]);
    let response = invoke(
        &webview,
        "create_printer",
        create_printer_body(
            "AMS Printer",
            json!({
                "slotLayout": layout,
                "initialLoads": [
                    { "slotIndex": 0, "spoolId": spools[0].id, "expectedSpoolRevision": spools[0].revision },
                    { "slotIndex": 1, "spoolId": spools[1].id, "expectedSpoolRevision": spools[1].revision },
                    { "slotIndex": 2, "spoolId": spools[2].id, "expectedSpoolRevision": spools[2].revision },
                ],
            }),
        ),
    )
    .unwrap();

    let printer = &response["data"]["printer"];
    let slots = material_slots(printer);
    assert_eq!(slots.len(), 4);
    for (index, slot) in slots.iter().enumerate() {
        assert_eq!(slot["name"], json!((index + 1).to_string()));
        assert_eq!(slot["feederLabel"], json!("AMS 1"));
    }
    for (index, spool_row) in spools.iter().enumerate() {
        assert_eq!(
            slots[index]["occupantSpoolId"],
            json!(spool_row.id),
            "slot {index} must be occupied by initial load {index}"
        );
    }
    assert!(
        slots[3].get("occupantSpoolId").is_none(),
        "the fourth slot got no initial load"
    );

    let printer_id = printer["id"].as_str().unwrap().to_string();
    let returned_revision = printer["revision"].as_i64().unwrap();
    assert_eq!(
        returned_revision, 4,
        "revision 1 at insert, +1 per of the 3 loads"
    );
    assert_eq!(printer_revision(&storage, &printer_id), returned_revision);

    let mut movement_ids = std::collections::HashSet::new();
    for (index, spool_row) in spools.iter().enumerate() {
        let after = spool(&storage, &spool_row.id);
        assert_eq!(
            after.slot_id.as_deref(),
            Some(slots[index]["id"].as_str().unwrap()),
            "spool {index} must be loaded into its target slot"
        );
        let history = storage
            .write(|tx| movement::history(tx, &spool_row.id))
            .unwrap();
        assert_eq!(history.len(), 1);
        assert_eq!(
            history[0].reason,
            farm3d_lib::spools::movement::MovementReason::Load
        );
        movement_ids.insert(history[0].operation_id.clone());
    }
    assert_eq!(
        movement_ids.len(),
        1,
        "every initial load must share one operationId: {movement_ids:?}"
    );

    // The returned (post-load) revision must be directly usable for a
    // follow-up write, not stale.
    let follow_up = invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": returned_revision,
            "slots": slots
                .iter()
                .map(|slot| json!({ "id": slot["id"], "name": slot["name"] }))
                .collect::<Vec<_>>(),
        }),
    );
    assert!(
        follow_up.is_ok(),
        "the create response's revision must be current, not stale: {follow_up:?}"
    );
}

#[test]
fn an_initial_load_of_a_spool_already_loaded_elsewhere_fails_the_whole_create() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _services) =
        runtime(Arc::clone(&storage), Arc::new(InjectedDocuments::default()));

    // Seed a printer with a slot, and load the Spool into it — no longer in
    // storage.
    let seed = invoke(
        &webview,
        "create_printer",
        create_printer_body("Seed Printer", json!({})),
    )
    .unwrap();
    let seed_slot_id = material_slots(&seed["data"]["printer"])[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let spool_row = a_storage_spool(&storage);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-seed-load",
                &spool_row.id,
                spool_row.revision,
                &MoveDestination::Slot {
                    slot_id: seed_slot_id.clone(),
                    expected_occupant_spool_id: None,
                    displaced_storage_label: None,
                },
                None,
            )
        })
        .unwrap();
    let loaded = spool(&storage, &spool_row.id);

    let before_count = storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM printers", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .unwrap();

    let error = error_of(invoke(
        &webview,
        "create_printer",
        create_printer_body(
            "New Printer",
            json!({
                "initialLoads": [
                    { "slotIndex": 0, "spoolId": loaded.id, "expectedSpoolRevision": loaded.revision },
                ],
            }),
        ),
    ));

    assert_eq!(error["code"], json!("VALIDATION"));
    assert_eq!(
        error["details"]["fieldPath"],
        json!("initialLoads[0].spoolId")
    );

    let after_count = storage
        .read(|connection| {
            connection.query_row("SELECT COUNT(*) FROM printers", [], |row| {
                row.get::<_, i64>(0)
            })
        })
        .unwrap();
    assert_eq!(before_count, after_count, "no Printer row was created");
}

// --- 3. Batch copies the layout ----------------------------------------------

#[test]
fn batch_create_copies_the_shared_layout_into_each_row_with_disjoint_ids() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _services) = runtime(storage, Arc::new(InjectedDocuments::default()));

    let shared_layout = json!([
        slot_spec("1", None),
        slot_spec("2", None),
        slot_spec("3", None),
        slot_spec("4", None),
    ]);
    let body = json!({
        "contractVersion": 1,
        "input": {
            "batchId": format!("batch-{}", uuid::Uuid::new_v4()),
            "shared": {
                "catalogRef": a_ref_json(),
                "startSafety": "confirmBedClear",
                "slotLayout": shared_layout,
            },
            "probe": false,
            "rows": [
                { "rowId": "r1", "name": "Row 1" },
                { "rowId": "r2", "name": "Row 2" },
                { "rowId": "r3", "name": "Row 3" },
            ],
        },
    });
    let response = invoke(&webview, "create_printers_batch", body).unwrap();
    let rows = response["data"]["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 3);

    let mut all_ids = std::collections::HashSet::new();
    let mut printers = Vec::new();
    for row in rows {
        assert_eq!(row["outcome"], json!("createdSetupIncomplete"));
        let printer = row["printer"].clone();
        let slots = material_slots(&printer).clone();
        assert_eq!(slots.len(), 4);
        for slot in &slots {
            assert!(slot.get("occupantSpoolId").is_none());
            let id = slot["id"].as_str().unwrap().to_string();
            assert!(all_ids.insert(id), "slot ids must be pairwise disjoint");
        }
        printers.push(printer);
    }

    // Renaming a slot on one Printer leaves the other two unchanged.
    let target = &printers[0];
    let target_id = target["id"].as_str().unwrap();
    let target_revision = target["revision"].as_i64().unwrap();
    let target_slots = material_slots(target);
    let mut new_layout: Vec<Value> = target_slots
        .iter()
        .map(|slot| json!({ "id": slot["id"], "name": slot["name"] }))
        .collect();
    new_layout[0]["name"] = json!("Renamed");

    let rename_response = invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": target_id,
            "expectedRevision": target_revision,
            "slots": new_layout,
        }),
    )
    .unwrap();
    assert_eq!(
        material_slots(&rename_response["data"]["printer"])[0]["name"],
        json!("Renamed")
    );

    for other in &printers[1..] {
        let other_id = other["id"].as_str().unwrap();
        let list_response =
            invoke(&webview, "list_printers", json!({"contractVersion": 1})).unwrap();
        let current = list_response["data"]
            .as_array()
            .unwrap()
            .iter()
            .find(|p| p["id"] == json!(other_id))
            .unwrap();
        assert_eq!(material_slots(current), material_slots(other));
    }
}

// --- 4. Layout editing --------------------------------------------------------

fn printer_revision(storage: &Storage, printer_id: &str) -> i64 {
    storage
        .read(|connection| {
            connection.query_row(
                "SELECT revision FROM printers WHERE id = ?1",
                [printer_id],
                |row| row.get(0),
            )
        })
        .unwrap()
}

fn slot_removed_at(storage: &Storage, slot_id: &str) -> Option<String> {
    storage
        .read(|connection| {
            connection.query_row(
                "SELECT removed_at FROM material_slots WHERE id = ?1",
                [slot_id],
                |row| row.get(0),
            )
        })
        .unwrap()
}

/// Fix round 2: queries `material_slots` directly (not `live_slots`, which
/// excludes removed rows) — a removed slot's real name must survive
/// `set_layout`'s pass 1, not be left holding its `"tmp-<position>"`
/// placeholder.
fn slot_name(storage: &Storage, slot_id: &str) -> String {
    storage
        .read(|connection| {
            connection.query_row(
                "SELECT name FROM material_slots WHERE id = ?1",
                [slot_id],
                |row| row.get(0),
            )
        })
        .unwrap()
}

#[test]
fn set_material_slot_layout_reorders_renames_adds_and_removes_an_empty_slot() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _services) =
        runtime(Arc::clone(&storage), Arc::new(InjectedDocuments::default()));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body(
            "Editable",
            json!({ "slotLayout": [slot_spec("A", None), slot_spec("B", None)] }),
        ),
    )
    .unwrap();
    let printer = &created["data"]["printer"];
    let printer_id = printer["id"].as_str().unwrap().to_string();
    let slots = material_slots(printer);
    let (a_id, b_id) = (
        slots[0]["id"].as_str().unwrap().to_string(),
        slots[1]["id"].as_str().unwrap().to_string(),
    );

    // Load then unload a Spool through slot B, so it has movement history
    // before it's removed.
    let spool_row = a_storage_spool(&storage);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-b-load",
                &spool_row.id,
                spool_row.revision,
                &MoveDestination::Slot {
                    slot_id: b_id.clone(),
                    expected_occupant_spool_id: None,
                    displaced_storage_label: None,
                },
                None,
            )
        })
        .unwrap();
    let after_load = spool(&storage, &spool_row.id);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-b-unload",
                &spool_row.id,
                after_load.revision,
                &MoveDestination::Storage {
                    storage_label: None,
                },
                None,
            )
        })
        .unwrap();
    // Both moves above bumped the Printer's revision (occupancy changed) —
    // re-read it before using it as `expectedRevision`.
    let mut revision = printer_revision(&storage, &printer_id);

    // Reorder (B before A), rename A -> "Renamed A", and add a new "C" —
    // dropping nothing yet.
    let response = invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [
                { "id": b_id, "name": "B" },
                { "id": a_id, "name": "Renamed A" },
                { "name": "C" },
            ],
        }),
    )
    .unwrap();
    let printer = &response["data"]["printer"];
    revision = printer["revision"].as_i64().unwrap();
    let slots = material_slots(printer);
    assert_eq!(slots.len(), 3);
    assert_eq!(slots[0]["id"], json!(b_id));
    assert_eq!(slots[0]["position"], json!(0));
    assert_eq!(slots[1]["id"], json!(a_id));
    assert_eq!(slots[1]["name"], json!("Renamed A"));
    assert_eq!(slots[1]["position"], json!(1));
    let c_id = slots[2]["id"].as_str().unwrap().to_string();
    assert_eq!(slots[2]["name"], json!("C"));
    assert_eq!(slots[2]["position"], json!(2));

    // Now remove the (empty) B slot.
    let response = invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [
                { "id": a_id, "name": "Renamed A" },
                { "id": c_id, "name": "C" },
            ],
        }),
    )
    .unwrap();
    let printer = &response["data"]["printer"];
    let slots = material_slots(printer);
    assert_eq!(slots.len(), 2);
    assert!(slots.iter().all(|slot| slot["id"] != json!(b_id)));

    // The removed slot keeps its row, and its history is still readable.
    assert!(slot_removed_at(&storage, &b_id).is_some());
    // Fix round 2: it also keeps its REAL name ("B") — pass 1's
    // `tmp-<position>` rename must never touch an already-removed row, so
    // e.g. the Spool history UI never shows a past movement's slot as
    // "tmp-100".
    assert_eq!(slot_name(&storage, &b_id), "B");
    let history = storage
        .write(|tx| movement::history(tx, &spool_row.id))
        .unwrap();
    assert_eq!(history.len(), 2);
    assert!(history
        .iter()
        .any(|row| row.to.slot_id.as_deref() == Some(b_id.as_str())));
    assert!(history
        .iter()
        .any(|row| row.from.slot_id.as_deref() == Some(b_id.as_str())));

    // Removing an occupied slot fails with SLOT_OCCUPIED.
    let occupant = a_storage_spool(&storage);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-c-load",
                &occupant.id,
                occupant.revision,
                &MoveDestination::Slot {
                    slot_id: c_id.clone(),
                    expected_occupant_spool_id: None,
                    displaced_storage_label: None,
                },
                None,
            )
        })
        .unwrap();
    // The load above bumped the Printer's revision too.
    revision = printer_revision(&storage, &printer_id);
    let error = error_of(invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [ { "id": a_id, "name": "Renamed A" } ],
        }),
    ));
    assert_eq!(error["code"], json!("SLOT_OCCUPIED"));
    assert_eq!(error["details"]["slotId"], json!(c_id));
    assert_eq!(error["details"]["spoolId"], json!(occupant.id));

    // More than 16 slots, fewer than 1, or case-insensitive duplicate names
    // are all VALIDATION.
    let too_many: Vec<Value> = (0..17).map(|i| slot_spec(&format!("S{i}"), None)).collect();
    let error = error_of(invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": too_many,
        }),
    ));
    assert_eq!(error["code"], json!("VALIDATION"));

    let error = error_of(invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [],
        }),
    ));
    assert_eq!(error["code"], json!("VALIDATION"));

    let error = error_of(invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [slot_spec("Dup", None), slot_spec("dup", None)],
        }),
    ));
    assert_eq!(error["code"], json!("VALIDATION"));

    // Fix round 1, Important 4: a repeated `id` across entries must be
    // rejected, not silently collapsed into a gapped layout.
    let error = error_of(invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [
                { "id": a_id, "name": "One" },
                { "id": a_id, "name": "Two" },
            ],
        }),
    ));
    assert_eq!(error["code"], json!("VALIDATION"));
}

/// Fix round 1, Important 2: renaming two live slots A/B -> B/A by name,
/// keeping the same ids, must not trip `material_slots_live_name` mid-pass
/// — swapping names is a normal, valid edit, not a transient collision.
#[test]
fn set_material_slot_layout_swaps_two_live_names() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _services) =
        runtime(Arc::clone(&storage), Arc::new(InjectedDocuments::default()));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body(
            "Swappable",
            json!({ "slotLayout": [slot_spec("A", None), slot_spec("B", None)] }),
        ),
    )
    .unwrap();
    let printer = &created["data"]["printer"];
    let printer_id = printer["id"].as_str().unwrap().to_string();
    let revision = printer["revision"].as_i64().unwrap();
    let slots = material_slots(printer);
    let (a_id, b_id) = (
        slots[0]["id"].as_str().unwrap().to_string(),
        slots[1]["id"].as_str().unwrap().to_string(),
    );

    let response = invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [
                { "id": a_id, "name": "B" },
                { "id": b_id, "name": "A" },
            ],
        }),
    )
    .unwrap();
    let slots = material_slots(&response["data"]["printer"]);
    assert_eq!(slots[0]["id"], json!(a_id));
    assert_eq!(slots[0]["name"], json!("B"));
    assert_eq!(slots[1]["id"], json!(b_id));
    assert_eq!(slots[1]["name"], json!("A"));
}

/// Fix round 1, Important 2: a kept slot giving up a name in the same call
/// that a brand-new slot takes that same name must not collide either — the
/// freed name must already be clear by the time pass 2 assigns it.
#[test]
fn set_material_slot_layout_lets_a_new_slot_take_a_name_a_kept_slot_gives_up() {
    let (_temp, _lease, storage) = storage();
    let (_app, webview, _services) =
        runtime(Arc::clone(&storage), Arc::new(InjectedDocuments::default()));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body(
            "Handoff",
            json!({ "slotLayout": [slot_spec("A", None), slot_spec("B", None)] }),
        ),
    )
    .unwrap();
    let printer = &created["data"]["printer"];
    let printer_id = printer["id"].as_str().unwrap().to_string();
    let revision = printer["revision"].as_i64().unwrap();
    let slots = material_slots(printer);
    let (a_id, b_id) = (
        slots[0]["id"].as_str().unwrap().to_string(),
        slots[1]["id"].as_str().unwrap().to_string(),
    );

    // A gives up its name ("A" -> "Renamed"); a new slot takes "A".
    let response = invoke(
        &webview,
        "set_material_slot_layout",
        json!({
            "contractVersion": 1,
            "printerId": printer_id,
            "expectedRevision": revision,
            "slots": [
                { "id": a_id, "name": "Renamed" },
                { "id": b_id, "name": "B" },
                { "name": "A" },
            ],
        }),
    )
    .unwrap();
    let slots = material_slots(&response["data"]["printer"]);
    assert_eq!(slots.len(), 3);
    assert_eq!(slots[0]["id"], json!(a_id));
    assert_eq!(slots[0]["name"], json!("Renamed"));
    assert_eq!(slots[1]["id"], json!(b_id));
    assert_eq!(slots[1]["name"], json!("B"));
    assert_eq!(slots[2]["name"], json!("A"));
    assert_ne!(slots[2]["id"], json!(a_id));
    assert_ne!(slots[2]["id"], json!(b_id));
}

// --- 5. Export and import -----------------------------------------------------

fn printers_document_versioned(schema_version: i64, printers: Value) -> Vec<u8> {
    serde_json::to_vec(&json!({
        "schemaVersion": schema_version,
        "exportedAt": "2026-09-23T00:00:00Z",
        "printers": printers,
    }))
    .unwrap()
}

fn imported_printer(id: &str, revision: i64, extra: Value) -> Value {
    let mut printer = json!({
        "id": id,
        "revision": revision,
        "name": "Imported",
        "catalogRef": {
            "vendor": "Unknown Vendor",
            "model": "Unknown Model",
            "variant": "Unknown Variant",
            "modelId": "unknown",
            "printerVariant": "0.4"
        },
        "notes": "",
        "overrides": {},
        "lastKnownGood": null,
        "connection": null,
    });
    for (key, value) in extra.as_object().unwrap() {
        printer[key] = value.clone();
    }
    printer
}

#[test]
fn export_v3_carries_material_slots_without_occupancy() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    let (_app, webview, _services) = runtime(Arc::clone(&storage), Arc::clone(&documents));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body(
            "Exportable",
            json!({ "slotLayout": [slot_spec("1", Some("AMS 1")), slot_spec("2", None)] }),
        ),
    )
    .unwrap();
    let slot_id = material_slots(&created["data"]["printer"])[0]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let occupant = a_storage_spool(&storage);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-export-load",
                &occupant.id,
                occupant.revision,
                &MoveDestination::Slot {
                    slot_id: slot_id.clone(),
                    expected_occupant_spool_id: None,
                    displaced_storage_label: None,
                },
                None,
            )
        })
        .unwrap();

    *documents.save.lock().unwrap() = Some(PathBuf::from("/tmp/printers-export.json"));
    let response = invoke(&webview, "export_printers", json!({"contractVersion": 1})).unwrap();
    assert_eq!(response["data"]["status"], json!("exported"));

    let writes = documents.writes.lock().unwrap();
    assert_eq!(writes.len(), 1);
    let document: Value = serde_json::from_slice(&writes[0]).unwrap();
    assert_eq!(document["schemaVersion"], json!(3));
    let exported_slots = document["printers"][0]["materialSlots"].as_array().unwrap();
    assert_eq!(exported_slots.len(), 2);
    assert_eq!(
        exported_slots[0],
        json!({"name": "1", "feederLabel": "AMS 1"})
    );
    assert_eq!(exported_slots[1], json!({"name": "2"}));
    assert!(exported_slots[0].get("id").is_none());
    assert!(exported_slots[0].get("position").is_none());
    assert!(exported_slots[0].get("occupantSpoolId").is_none());
}

#[test]
fn importing_v1_and_v2_documents_gives_each_printer_the_default_main_layout() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    let (_app, webview, _services) = runtime(Arc::clone(&storage), Arc::clone(&documents));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body("Seed", json!({})),
    )
    .unwrap();
    let id = created["data"]["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let revision = created["data"]["printer"]["revision"].as_i64().unwrap();

    for version in [1, 2] {
        let document = printers_document_versioned(
            version,
            json!([imported_printer(&id, revision, json!({}))]),
        );
        *documents.open.lock().unwrap() =
            Some(PathBuf::from(format!("/tmp/import-v{version}.json")));
        *documents.bytes.lock().unwrap() = Some(document);

        let current = invoke(&webview, "list_printers", json!({"contractVersion": 1})).unwrap();
        let current_revision = current["data"][0]["revision"].as_i64().unwrap();
        let response = invoke(
            &webview,
            "import_printers",
            json!({
                "contractVersion": 1,
                "expectedRevisions": [{"id": id, "revision": current_revision}],
            }),
        )
        .unwrap();
        assert_eq!(response["data"]["status"], json!("applied"), "v{version}");
        let printers = response["data"]["printers"].as_array().unwrap();
        assert_eq!(material_slots(&printers[0]).len(), 1, "v{version}");
        assert_eq!(
            material_slots(&printers[0])[0]["name"],
            json!("Main"),
            "v{version}"
        );
    }
}

#[test]
fn importing_v3_recreates_the_layout_with_new_ids() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    let (_app, webview, _services) = runtime(Arc::clone(&storage), Arc::clone(&documents));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body("Seed", json!({ "slotLayout": [slot_spec("Old", None)] })),
    )
    .unwrap();
    let printer = &created["data"]["printer"];
    let id = printer["id"].as_str().unwrap().to_string();
    let revision = printer["revision"].as_i64().unwrap();
    let old_slot_id = material_slots(printer)[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let document = printers_document_versioned(
        3,
        json!([imported_printer(
            &id,
            revision,
            json!({ "materialSlots": [slot_spec("New 1", None), slot_spec("New 2", Some("AMS 1"))] })
        )]),
    );
    *documents.open.lock().unwrap() = Some(PathBuf::from("/tmp/import-v3.json"));
    *documents.bytes.lock().unwrap() = Some(document);

    let response = invoke(
        &webview,
        "import_printers",
        json!({
            "contractVersion": 1,
            "expectedRevisions": [{"id": id, "revision": revision}],
        }),
    )
    .unwrap();
    assert_eq!(response["data"]["status"], json!("applied"));
    let printers = response["data"]["printers"].as_array().unwrap();
    let slots = material_slots(&printers[0]);
    assert_eq!(slots.len(), 2);
    assert_eq!(slots[0]["name"], json!("New 1"));
    assert_eq!(slots[1]["name"], json!("New 2"));
    assert_eq!(slots[1]["feederLabel"], json!("AMS 1"));
    for slot in slots {
        let slot_id = slot["id"].as_str().unwrap();
        assert!(slot_id.starts_with("slt-"));
        assert_ne!(slot_id, old_slot_id, "import must mint fresh slot ids");
    }
}

/// Fix round 1, Important 5: `replace_all` deletes and reinserts every
/// Printer, and `spools.slot_id` has no `ON DELETE` action (unlike
/// `spool_movements`'s slot columns, which cascade) — so an import must be
/// rejected up front while any Spool is still loaded, rather than let the
/// delete hit that foreign key and surface as an opaque
/// `PERSISTENCE_UNAVAILABLE`.
#[test]
fn importing_printers_is_rejected_while_any_spool_is_loaded() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    let (_app, webview, _services) = runtime(Arc::clone(&storage), Arc::clone(&documents));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body("Seed", json!({})),
    )
    .unwrap();
    let printer = &created["data"]["printer"];
    let id = printer["id"].as_str().unwrap().to_string();
    let slot_id = material_slots(printer)[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let occupant = a_storage_spool(&storage);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-load",
                &occupant.id,
                occupant.revision,
                &MoveDestination::Slot {
                    slot_id: slot_id.clone(),
                    expected_occupant_spool_id: None,
                    displaced_storage_label: None,
                },
                None,
            )
        })
        .unwrap();
    // The load above bumped the Printer's own revision.
    let revision = printer_revision(&storage, &id);

    let document = printers_document_versioned(
        3,
        json!([imported_printer(
            &id,
            revision,
            json!({ "materialSlots": [slot_spec("Only", None)] })
        )]),
    );
    *documents.open.lock().unwrap() = Some(PathBuf::from("/tmp/import-blocked.json"));
    *documents.bytes.lock().unwrap() = Some(document);

    let error = error_of(invoke(
        &webview,
        "import_printers",
        json!({
            "contractVersion": 1,
            "expectedRevisions": [{"id": id, "revision": revision}],
        }),
    ));
    assert_eq!(error["code"], json!("VALIDATION"));
    assert_eq!(error["details"]["fieldPath"], json!("printers"));
    // Fix round 2: `RepositoryError::SpoolsLoadedForImport` is a typed
    // variant now, not a magic-string `field_path` match -- the mapped
    // `CommandError` must still read exactly as it did before.
    assert_eq!(
        error["message"],
        json!("Unload every Spool before importing Printers.")
    );

    // Nothing changed: the Printer, its slot, and the loaded Spool are all
    // untouched — the whole import rolled back inside the transaction.
    assert_eq!(printer_revision(&storage, &id), revision);
    let current = invoke(&webview, "list_printers", json!({"contractVersion": 1})).unwrap();
    assert_eq!(material_slots(&current["data"][0]).len(), 1);
    assert_eq!(
        spool(&storage, &occupant.id).slot_id.as_deref(),
        Some(slot_id.as_str())
    );
}

/// Fix round 1, Important 5: with no Spool loaded, the import still goes
/// through, and the replaced Printer's OLD Material Slot row and its
/// `spool_movements` history are both actually gone afterward (the
/// migration's `ON DELETE CASCADE`) — accepted, not merely un-erroring.
#[test]
fn importing_with_no_spool_loaded_cascades_away_the_replaced_printers_old_slot_history() {
    let (_temp, _lease, storage) = storage();
    let documents = Arc::new(InjectedDocuments::default());
    let (_app, webview, _services) = runtime(Arc::clone(&storage), Arc::clone(&documents));

    let created = invoke(
        &webview,
        "create_printer",
        create_printer_body("Seed", json!({ "slotLayout": [slot_spec("Old", None)] })),
    )
    .unwrap();
    let printer = &created["data"]["printer"];
    let id = printer["id"].as_str().unwrap().to_string();
    let old_slot_id = material_slots(printer)[0]["id"]
        .as_str()
        .unwrap()
        .to_string();

    // Give the old slot movement history, then unload it — no Spool is
    // loaded anywhere by the time the import runs.
    let spool_row = a_storage_spool(&storage);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-load",
                &spool_row.id,
                spool_row.revision,
                &MoveDestination::Slot {
                    slot_id: old_slot_id.clone(),
                    expected_occupant_spool_id: None,
                    displaced_storage_label: None,
                },
                None,
            )
        })
        .unwrap();
    let after_load = spool(&storage, &spool_row.id);
    storage
        .write_repo(|tx| {
            movement::apply_move(
                tx,
                "op-unload",
                &spool_row.id,
                after_load.revision,
                &MoveDestination::Storage {
                    storage_label: None,
                },
                None,
            )
        })
        .unwrap();
    let revision = printer_revision(&storage, &id);
    let movements_before: i64 = storage
        .read(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM spool_movements WHERE from_slot_id = ?1 OR to_slot_id = ?1",
                [old_slot_id.as_str()],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(movements_before, 2);

    let document = printers_document_versioned(
        3,
        json!([imported_printer(
            &id,
            revision,
            json!({ "materialSlots": [slot_spec("New", None)] })
        )]),
    );
    *documents.open.lock().unwrap() = Some(PathBuf::from("/tmp/import-cascade.json"));
    *documents.bytes.lock().unwrap() = Some(document);

    let response = invoke(
        &webview,
        "import_printers",
        json!({
            "contractVersion": 1,
            "expectedRevisions": [{"id": id, "revision": revision}],
        }),
    )
    .unwrap();
    assert_eq!(response["data"]["status"], json!("applied"));

    let old_slot_exists: i64 = storage
        .read(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM material_slots WHERE id = ?1",
                [old_slot_id.as_str()],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        old_slot_exists, 0,
        "the old slot row must be gone, not soft-removed"
    );
    let movements_after: i64 = storage
        .read(|connection| {
            connection.query_row(
                "SELECT COUNT(*) FROM spool_movements WHERE from_slot_id = ?1 OR to_slot_id = ?1",
                [old_slot_id.as_str()],
                |row| row.get(0),
            )
        })
        .unwrap();
    assert_eq!(
        movements_after, 0,
        "the old slot's movement history must cascade away too"
    );
}
