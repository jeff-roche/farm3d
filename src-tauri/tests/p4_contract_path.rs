//! P4: the Library commands' registration (ruling M6), and Task 7's
//! Project and Model commands through the real IPC handler (spec D1, D17,
//! D18, §Commands). `P4_COMMANDS` is the single place each P4 task adds its
//! command names; the count is asserted against the pre-P4 total rather
//! than a hard-coded sum.
//!
//! A recording listener on `farm3d-event-v1` captures the `library.*`
//! events. The blocked-delete case (brief test 7) is a unit test in
//! `library::commands`, because blocker sources are a static slice (P17).

mod common;

use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use base64::Engine;
use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::connections::{ConnectionConfig, PrinterConnection};
use farm3d_lib::library::selection::SelectionPurpose;
use farm3d_lib::persistence::{MetadataRootLease, Storage, StoragePaths};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;

use common::{a_catalog, invoke, FakeModelFileIo};

/// `COMMAND_NAMES.len()` before P4 (P3 merged).
const PRE_P4: usize = 41;

const P4_COMMANDS: &[&str] = &[
    "pick_model_files",
    "inspect_import_selection",
    "cancel_import_selection",
    "import_models",
    "list_library",
    "create_project",
    "rename_project",
    "delete_project",
    "update_model",
    "set_model_projects",
    "delete_model",
    "list_model_revisions",
    "get_revision_thumbnail",
    "library_content_info",
    "check_linked_sources",
    "locate_linked_source",
    "convert_model_to_managed",
];

const CORE_3MF_SHA256: &str = "0712090c29fed95a750f831dcb7be12a3372648719e3978d0a4cc83e45e3e0ab";

#[test]
fn every_p4_command_is_registered_with_a_contract() {
    assert_eq!(farm3d_lib::COMMAND_NAMES.len(), PRE_P4 + P4_COMMANDS.len());
    let manifest = farm3d_lib::contracts::inventory::command_contract_inventory();
    assert_eq!(manifest.len(), farm3d_lib::COMMAND_NAMES.len());
    for command in P4_COMMANDS {
        assert!(
            farm3d_lib::COMMAND_NAMES.contains(command),
            "{command} missing from COMMAND_NAMES"
        );
        assert!(
            manifest.iter().any(|entry| entry.command == *command),
            "{command} missing from COMMAND_CONTRACTS"
        );
    }
}

#[test]
fn the_generated_contracts_include_every_task_7_type() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../src/generated/contracts");
    for file in [
        "domain/LibrarySnapshot.ts",
        "domain/ModelSourceRevisionRecord.ts",
        "domain/RevisionThumbnail.ts",
        "command/LibraryContentInfo.ts",
        "command/ProjectMutationResult.ts",
        "command/ModelMutationResult.ts",
        "command/ModelPatch.ts",
        "command/DeleteProjectData.ts",
        "command/DeleteModelData.ts",
    ] {
        assert!(root.join(file).is_file(), "{file} was not generated");
    }
    let contracts = fs::read_to_string(root.join("command/CommandContracts.ts")).unwrap();
    for request in [
        "ListLibraryRequest",
        "CreateProjectRequest",
        "RenameProjectRequest",
        "DeleteProjectRequest",
        "UpdateModelRequest",
        "SetModelProjectsRequest",
        "DeleteModelRequest",
        "ListModelRevisionsRequest",
        "GetRevisionThumbnailRequest",
        "LibraryContentInfoRequest",
    ] {
        assert!(contracts.contains(request), "{request} missing");
    }
}

// --- Fixture -------------------------------------------------------------------

struct Env {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Arc<Storage>,
    sources: tempfile::TempDir,
    _app: tauri::App<MockRuntime>,
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

fn library_fixture(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/library")
        .join(name)
}

fn new_env() -> Env {
    let temp = tempfile::tempdir().unwrap();
    let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data")).unwrap();
    let lease = MetadataRootLease::acquire(&paths).unwrap();
    let storage = Arc::new(Storage::open(paths, &lease).unwrap());
    let credentials = tempfile::tempdir().unwrap();
    let (app, webview, _manager, services) = common::runtime_with_file_io(
        tauri::generate_handler![
            farm3d_lib::library::commands::inspect_import_selection,
            farm3d_lib::library::commands::import_models,
            farm3d_lib::library::commands::list_library,
            farm3d_lib::library::commands::create_project,
            farm3d_lib::library::commands::rename_project,
            farm3d_lib::library::commands::delete_project,
            farm3d_lib::library::commands::update_model,
            farm3d_lib::library::commands::set_model_projects,
            farm3d_lib::library::commands::delete_model,
            farm3d_lib::library::commands::list_model_revisions,
            farm3d_lib::library::commands::get_revision_thumbnail,
            farm3d_lib::library::commands::library_content_info,
        ],
        Arc::clone(&storage),
        Arc::new(a_catalog()),
        credentials.path().to_path_buf(),
        no_connection,
        Arc::new(FakeModelFileIo { picks: None }),
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
        sources: tempfile::tempdir().unwrap(),
        _app: app,
        webview,
        services,
        events,
        _credentials: credentials,
    }
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(bytes))
}

/// One managed, Unfiled `ImportItemRequest` with `extra` merged over it.
fn request(file_index: u32, name: &str, extra: Value) -> Value {
    let mut request = json!({
        "fileIndex": file_index,
        "name": name,
        "projectIds": [],
        "storageMode": "managed",
        "acknowledgeUnsupported": false,
    });
    for (key, value) in extra.as_object().expect("extra is an object") {
        request[key] = value.clone();
    }
    request
}

/// `core-two-objects.3mf`'s embedded thumbnail.
fn core_thumbnail() -> Vec<u8> {
    use std::io::{Cursor, Read};
    let base = fs::read(library_fixture("core-two-objects.3mf")).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(base)).unwrap();
    let mut bytes = Vec::new();
    archive
        .by_name("Metadata/thumbnail.png")
        .unwrap()
        .read_to_end(&mut bytes)
        .unwrap();
    bytes
}

/// `core-two-objects.3mf` with a different thumbnail (the core PNG plus
/// trailing bytes: the header is all inspection reads, preflight S7), so
/// its content and its thumbnail both differ from the core fixture's.
fn variant_3mf(thumbnail: &[u8]) -> Vec<u8> {
    use std::io::{Cursor, Write};
    let base = fs::read(library_fixture("core-two-objects.3mf")).unwrap();
    let mut archive = zip::ZipArchive::new(Cursor::new(base)).unwrap();
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for index in 0..archive.len() {
        let entry = archive.by_index_raw(index).unwrap();
        if entry.name() == "Metadata/thumbnail.png" {
            continue;
        }
        writer.raw_copy_file(entry).unwrap();
    }
    writer
        .start_file(
            "Metadata/thumbnail.png",
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(thumbnail).unwrap();
    writer.finish().unwrap().into_inner()
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

    fn source(&self, name: &str) -> PathBuf {
        let path = self.sources.path().join(name);
        fs::copy(library_fixture(name), &path).unwrap();
        path
    }

    fn write_source(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.sources.path().join(name);
        fs::write(&path, bytes).unwrap();
        path
    }

    fn create_project(&self, name: &str) -> String {
        self.ok("create_project", json!({ "name": name }))["project"]["id"]
            .as_str()
            .unwrap()
            .to_string()
    }

    /// Imports `path` as one item through a fresh selection; returns the
    /// item result.
    fn import_one(&self, path: PathBuf, item: Value) -> Value {
        let summary = self
            .services
            .library
            .selections
            .register(SelectionPurpose::Import, vec![path]);
        let selection = summary.selection_id.clone();
        self.ok(
            "inspect_import_selection",
            json!({ "selectionId": selection }),
        );
        let result = self.ok(
            "import_models",
            json!({
                "selectionId": selection,
                "operationId": format!("op-{}", uuid::Uuid::new_v4()),
                "items": [item],
            }),
        );
        let item = result["items"][0].clone();
        assert_ne!(item["outcome"], "rejected", "{item}");
        item
    }

    /// Imports `path` as a new Model; returns its `ModelRecord`.
    fn import_model(&self, path: PathBuf, name: &str, project_ids: Value) -> Value {
        self.import_one(path, request(0, name, json!({ "projectIds": project_ids })))["model"]
            .clone()
    }

    fn snapshot(&self) -> Value {
        self.ok("list_library", json!({}))
    }

    fn model(&self, id: &str) -> Value {
        self.snapshot()["models"]
            .as_array()
            .unwrap()
            .iter()
            .find(|model| model["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn project(&self, id: &str) -> Value {
        self.snapshot()["projects"]
            .as_array()
            .unwrap()
            .iter()
            .find(|project| project["id"] == id)
            .cloned()
            .unwrap_or(Value::Null)
    }

    fn count(&self, sql: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    fn blob_rows(&self, sha256: &str) -> i64 {
        self.count(&format!(
            "SELECT COUNT(*) FROM content_blobs WHERE sha256 = '{sha256}'"
        ))
    }

    fn blob(&self, sha256: &str) -> PathBuf {
        self.storage
            .paths()
            .content_root()
            .join("blobs/sha256")
            .join(&sha256[..2])
            .join(sha256)
    }

    /// `library.*` events recorded since the last call, then clears them.
    fn take_library_events(&self) -> Vec<Value> {
        std::mem::take(&mut *self.events.lock().unwrap())
            .into_iter()
            .filter(|event| {
                event["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("library."))
            })
            .collect()
    }
}

fn types(events: &[Value]) -> Vec<&str> {
    events
        .iter()
        .map(|event| event["type"].as_str().unwrap())
        .collect()
}

fn id(record: &Value) -> String {
    record["id"].as_str().unwrap().to_string()
}

fn assert_conflict(error: &Value, entity_id: &str, expected: i64, current: i64) {
    assert_eq!(error["code"], "CONFLICT", "{error}");
    assert_eq!(
        error["details"],
        json!({
            "entityId": entity_id,
            "expectedRevision": expected,
            "currentRevision": current,
        }),
        "{error}"
    );
}

// --- 1. Backfill ordering (D17) ----------------------------------------------------

#[test]
fn the_snapshot_sequence_precedes_the_next_change() {
    let env = new_env();
    let snapshot = env.snapshot();
    let sequence = snapshot["snapshotSequence"].as_u64().unwrap();
    assert_eq!(snapshot["projects"], json!([]));
    assert_eq!(snapshot["models"], json!([]));
    let stream_id = snapshot["streamId"].as_str().unwrap().to_string();

    let project = env.ok("create_project", json!({ "name": "Brackets" }))["project"].clone();

    let events = env.take_library_events();
    assert_eq!(types(&events), ["library.project.changed"]);
    assert_eq!(events[0]["sequence"], sequence + 1);
    assert_eq!(events[0]["streamId"], stream_id);
    assert_eq!(
        events[0]["subject"],
        json!({ "kind": "project", "id": id(&project) })
    );
    assert_eq!(events[0]["payload"], project);
    assert_eq!(project["name"], "Brackets");
    assert_eq!(project["modelCount"], 0);
    assert_eq!(project["revision"], 1);

    let after = env.snapshot();
    assert_eq!(after["snapshotSequence"], sequence + 1);
    assert_eq!(after["projects"], json!([project]));
}

// --- 2. Project validation ----------------------------------------------------------

#[test]
fn project_names_are_unique_case_insensitively_and_renames_check_the_revision() {
    let env = new_env();
    let brackets = env.create_project("Brackets");
    env.take_library_events();

    let duplicate = env.err("create_project", json!({ "name": "  bRACKETS " }));
    assert_eq!(duplicate["code"], "VALIDATION", "{duplicate}");
    assert_eq!(duplicate["details"]["fieldPath"], "name");
    let blank = env.err("create_project", json!({ "name": "   " }));
    assert_eq!(blank["details"]["fieldPath"], "name");

    let stale = env.err(
        "rename_project",
        json!({ "id": brackets, "expectedRevision": 7, "name": "Mounts" }),
    );
    assert_conflict(&stale, &brackets, 7, 1);
    let missing = env.err(
        "rename_project",
        json!({ "id": "prj-missing", "expectedRevision": 1, "name": "Mounts" }),
    );
    assert_eq!(missing["code"], "NOT_FOUND");
    assert!(
        env.take_library_events().is_empty(),
        "a failed write emits nothing"
    );

    let renamed = env.ok(
        "rename_project",
        json!({ "id": brackets, "expectedRevision": 1, "name": "Mounts" }),
    )["project"]
        .clone();
    assert_eq!(renamed["name"], "Mounts");
    assert_eq!(renamed["revision"], 2);
    let events = env.take_library_events();
    assert_eq!(types(&events), ["library.project.changed"]);
    assert_eq!(events[0]["payload"], renamed);
}

// --- 3. delete_project removes memberships only (D18) -------------------------------

#[test]
fn deleting_a_project_keeps_its_models_and_reports_the_newly_unfiled() {
    let env = new_env();
    let parts = env.create_project("Parts");
    let quarry = env.create_project("Quarry");
    let alpha = env.import_model(
        env.source("cube-binary.stl"),
        "Alpha",
        json!([parts, quarry]),
    );
    let bravo = env.import_model(env.source("cube-ascii.stl"), "Bravo", json!([parts]));
    let (alpha, bravo) = (id(&alpha), id(&bravo));
    let revisions_before = env.count("SELECT COUNT(*) FROM model_source_revisions");
    let blobs_before = env.count("SELECT COUNT(*) FROM content_blobs");
    env.take_library_events();

    let stale = env.err(
        "delete_project",
        json!({ "id": parts, "expectedRevision": 2 }),
    );
    assert_conflict(&stale, &parts, 2, 1);
    assert!(env.take_library_events().is_empty());

    let result = env.ok(
        "delete_project",
        json!({ "id": parts, "expectedRevision": 1 }),
    );

    assert_eq!(
        result,
        json!({
            "deletedId": parts,
            "affectedModelIds": [alpha, bravo],
            "nowUnfiledModelIds": [bravo],
        })
    );
    let alpha_record = env.model(&alpha);
    let bravo_record = env.model(&bravo);
    assert_eq!(alpha_record["projectIds"], json!([quarry]));
    assert_eq!(alpha_record["revision"], 2);
    assert_eq!(bravo_record["projectIds"], json!([]));
    assert_eq!(bravo_record["revision"], 2);
    assert_eq!(env.project(&parts), Value::Null);
    assert_eq!(env.count("SELECT COUNT(*) FROM library_models"), 2);
    assert_eq!(
        env.count("SELECT COUNT(*) FROM model_source_revisions"),
        revisions_before
    );
    assert_eq!(
        env.count("SELECT COUNT(*) FROM content_blobs"),
        blobs_before
    );
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM project_models WHERE project_id = '{parts}'"
        )),
        0
    );

    let events = env.take_library_events();
    assert_eq!(
        types(&events),
        [
            "library.model.changed",
            "library.model.changed",
            "library.project.removed",
        ]
    );
    assert_eq!(events[0]["payload"], alpha_record);
    assert_eq!(events[1]["payload"], bravo_record);
    assert_eq!(
        events[2]["subject"],
        json!({ "kind": "project", "id": parts })
    );
    assert_eq!(events[2]["payload"], json!({}));
}

// --- 4. set_model_projects (§Commands) ----------------------------------------------

#[test]
fn set_model_projects_edits_membership_as_a_set() {
    let env = new_env();
    let parts = env.create_project("Parts");
    let quarry = env.create_project("Quarry");
    let model = id(&env.import_model(env.source("cube-binary.stl"), "Cube", json!([])));
    env.take_library_events();

    let added = env.ok(
        "set_model_projects",
        json!({ "modelId": model, "expectedRevision": 1, "add": [quarry, parts], "remove": [] }),
    );
    assert_eq!(added["model"]["projectIds"], json!([parts, quarry]));
    assert_eq!(added["model"]["revision"], 2);
    assert_eq!(added["warnings"], json!([]));
    let events = env.take_library_events();
    assert_eq!(
        types(&events),
        [
            "library.model.changed",
            "library.project.changed",
            "library.project.changed",
        ]
    );
    assert_eq!(events[0]["payload"], added["model"]);
    assert_eq!(
        events[1]["subject"],
        json!({ "kind": "project", "id": parts })
    );
    assert_eq!(events[1]["payload"]["modelCount"], 1);
    assert_eq!(
        events[2]["subject"],
        json!({ "kind": "project", "id": quarry })
    );
    assert_eq!(events[2]["payload"]["modelCount"], 1);

    // The same add again is a successful no-op.
    let again = env.ok(
        "set_model_projects",
        json!({ "modelId": model, "expectedRevision": 2, "add": [parts, quarry], "remove": [] }),
    );
    assert_eq!(again["model"], added["model"]);
    assert!(env.take_library_events().is_empty());

    // Failures change nothing and emit nothing.
    let both = env.err(
        "set_model_projects",
        json!({ "modelId": model, "expectedRevision": 2, "add": [parts], "remove": [parts] }),
    );
    assert_eq!(both["code"], "VALIDATION", "{both}");
    let unknown = env.err(
        "set_model_projects",
        json!({ "modelId": model, "expectedRevision": 2, "add": ["prj-missing"], "remove": [parts] }),
    );
    assert_eq!(unknown["code"], "NOT_FOUND", "{unknown}");
    assert_eq!(unknown["details"]["entityId"], "prj-missing");
    let stale = env.err(
        "set_model_projects",
        json!({ "modelId": model, "expectedRevision": 1, "add": [], "remove": [parts] }),
    );
    assert_conflict(&stale, &model, 1, 2);
    let missing_model = env.err(
        "set_model_projects",
        json!({ "modelId": "mdl-missing", "expectedRevision": 1, "add": [parts], "remove": [] }),
    );
    assert_eq!(missing_model["code"], "NOT_FOUND");
    assert_eq!(env.model(&model), added["model"]);
    assert!(env.take_library_events().is_empty());

    // Removing the last memberships makes the Model Unfiled.
    let removed = env.ok(
        "set_model_projects",
        json!({ "modelId": model, "expectedRevision": 2, "add": [], "remove": [parts, quarry] }),
    );
    assert_eq!(removed["model"]["projectIds"], json!([]));
    assert_eq!(removed["model"]["revision"], 3);
    assert_eq!(env.project(&parts)["modelCount"], 0);
    assert_eq!(
        types(&env.take_library_events()),
        [
            "library.model.changed",
            "library.project.changed",
            "library.project.changed",
        ]
    );
}

// --- 5. update_model and DUPLICATE_NAME -----------------------------------------------

#[test]
fn renaming_a_model_warns_about_a_same_name_model_it_shares_a_project_with() {
    let env = new_env();
    let parts = env.create_project("Parts");
    let quarry = env.create_project("Quarry");
    let cube = id(&env.import_model(env.source("cube-binary.stl"), "Cube", json!([parts])));
    let other = id(&env.import_model(env.source("cube-ascii.stl"), "Other", json!([parts])));
    let elsewhere = id(&env.import_model(
        env.source("cube-ascii-bare-solid.stl"),
        "Elsewhere",
        json!([quarry]),
    ));
    env.take_library_events();

    let renamed = env.ok(
        "update_model",
        json!({ "id": other, "expectedRevision": 1, "patch": { "name": " cube " } }),
    );
    assert_eq!(renamed["model"]["name"], "cube");
    assert_eq!(renamed["model"]["revision"], 2);
    assert_eq!(
        renamed["warnings"][0]["code"], "DUPLICATE_NAME",
        "{renamed}"
    );
    assert_eq!(renamed["warnings"].as_array().unwrap().len(), 1);
    let events = env.take_library_events();
    assert_eq!(types(&events), ["library.model.changed"]);
    assert_eq!(events[0]["payload"], renamed["model"]);

    // A same-name Model in a different Project is not a duplicate.
    let apart = env.ok(
        "update_model",
        json!({ "id": elsewhere, "expectedRevision": 1, "patch": { "name": "CUBE" } }),
    );
    assert_eq!(apart["warnings"], json!([]));

    // An empty patch and an unchanged name change nothing.
    let unchanged = env.ok(
        "update_model",
        json!({ "id": cube, "expectedRevision": 1, "patch": {} }),
    );
    assert_eq!(unchanged["model"]["revision"], 1);
    let same = env.ok(
        "update_model",
        json!({ "id": cube, "expectedRevision": 1, "patch": { "name": "Cube" } }),
    );
    assert_eq!(same["model"]["revision"], 1);
    env.take_library_events();

    let stale = env.err(
        "update_model",
        json!({ "id": cube, "expectedRevision": 5, "patch": { "name": "Box" } }),
    );
    assert_conflict(&stale, &cube, 5, 1);
    let blank = env.err(
        "update_model",
        json!({ "id": cube, "expectedRevision": 1, "patch": { "name": "  " } }),
    );
    assert_eq!(blank["details"]["fieldPath"], "name");
    assert!(env.take_library_events().is_empty());

    // Import applies the same rule: both Unfiled counts as shared.
    let unfiled = env.import_one(env.source("plain.gcode"), request(0, "Loose", json!({})));
    assert_eq!(unfiled["warnings"], json!([]));
    let twin = env.import_one(
        env.source("prusa-cube.gcode"),
        request(0, "LOOSE", json!({})),
    );
    assert_eq!(twin["warnings"][0]["code"], "DUPLICATE_NAME", "{twin}");
    let in_parts = env.import_one(
        env.source("cube-for-slicers.stl"),
        request(0, "Cube", json!({ "projectIds": [parts] })),
    );
    assert_eq!(
        in_parts["warnings"][0]["code"], "DUPLICATE_NAME",
        "{in_parts}"
    );
}

// --- 6. delete_model and content cleanup (D18, preflight S7) --------------------------

#[test]
fn deleting_a_managed_model_releases_only_the_blobs_nothing_else_holds() {
    let env = new_env();
    let parts = env.create_project("Parts");
    let variant_thumbnail = [core_thumbnail(), b"variant".to_vec()].concat();
    let variant = variant_3mf(&variant_thumbnail);
    let (x, t1) = (sha256_hex(&variant), sha256_hex(&variant_thumbnail));
    let t2 = sha256_hex(&core_thumbnail());

    let alpha = env.import_model(env.write_source("x.3mf", &variant), "Alpha", json!([parts]));
    let alpha = id(&alpha);
    let beta = env.import_one(
        env.write_source("x-again.3mf", &variant),
        request(0, "Beta", json!({ "duplicateAction": "addAnother" })),
    )["model"]
        .clone();
    let added = env.import_one(
        env.source("core-two-objects.3mf"),
        request(
            0,
            "Alpha",
            json!({
                "duplicateAction": "addRevision",
                "targetModelId": alpha,
                "targetExpectedRevision": 1,
            }),
        ),
    );
    assert_eq!(added["outcome"], "revisionAdded", "{added}");
    for sha in [&x, &t1, CORE_3MF_SHA256, &t2] {
        assert_eq!(env.blob_rows(sha), 1, "{sha}");
        assert!(env.blob(sha).is_file(), "{sha}");
    }
    let info = env.ok("library_content_info", json!({}));
    assert_eq!(info["blobCount"], 4);
    env.take_library_events();

    let stale = env.err(
        "delete_model",
        json!({ "id": alpha, "expectedRevision": 1 }),
    );
    assert_conflict(&stale, &alpha, 1, 2);
    assert!(env.take_library_events().is_empty());
    assert_eq!(env.blob_rows(CORE_3MF_SHA256), 1);

    let result = env.ok(
        "delete_model",
        json!({ "id": alpha, "expectedRevision": 2 }),
    );

    assert_eq!(result, json!({ "deletedId": alpha, "warnings": [] }));
    for kept in [&x, &t1] {
        assert_eq!(env.blob_rows(kept), 1, "{kept} is still Beta's");
        assert!(env.blob(kept).is_file(), "{kept}");
    }
    for released in [CORE_3MF_SHA256, t2.as_str()] {
        assert_eq!(env.blob_rows(released), 0, "{released}");
        assert!(!env.blob(released).exists(), "{released} was not unlinked");
    }
    assert_eq!(env.count("SELECT COUNT(*) FROM pending_blob_cleanup"), 0);
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM model_source_revisions WHERE model_id = '{alpha}'"
        )),
        0
    );
    assert_eq!(
        env.count(&format!(
            "SELECT COUNT(*) FROM project_models WHERE model_id = '{alpha}'"
        )),
        0
    );
    assert_eq!(env.model(&alpha), Value::Null);
    assert_eq!(env.model(&id(&beta))["revision"], 1);
    let info = env.ok("library_content_info", json!({}));
    assert_eq!(
        info,
        json!({
            "blobCount": 2,
            "totalBytes": (variant.len() + variant_thumbnail.len()) as u64,
            "pendingCleanupCount": 0,
        })
    );

    let events = env.take_library_events();
    assert_eq!(
        types(&events),
        ["library.model.removed", "library.project.changed"]
    );
    assert_eq!(
        events[0]["subject"],
        json!({ "kind": "model", "id": alpha })
    );
    assert_eq!(events[0]["payload"], json!({}));
    assert_eq!(
        events[1]["subject"],
        json!({ "kind": "project", "id": parts })
    );
    assert_eq!(events[1]["payload"]["modelCount"], 0);

    let gone = env.err(
        "delete_model",
        json!({ "id": alpha, "expectedRevision": 2 }),
    );
    assert_eq!(gone["code"], "NOT_FOUND");
}

#[test]
fn deleting_a_linked_model_never_touches_its_source_file() {
    let env = new_env();
    let source = env.source("cube-binary.stl");
    let original = fs::read(&source).unwrap();
    let item = env.import_one(
        source.clone(),
        request(0, "Linked", json!({ "storageMode": "linked" })),
    );
    let model = item["model"].clone();
    assert_eq!(model["storageMode"], "linked");
    let sha = model["currentRevision"]["sha256"]
        .as_str()
        .unwrap()
        .to_string();
    env.take_library_events();

    let result = env.ok(
        "delete_model",
        json!({ "id": id(&model), "expectedRevision": 1 }),
    );

    assert_eq!(result["deletedId"], model["id"]);
    assert_eq!(fs::read(&source).unwrap(), original);
    assert_eq!(env.blob_rows(&sha), 0);
    assert!(!env.blob(&sha).exists());
    assert_eq!(types(&env.take_library_events()), ["library.model.removed"]);
}

// --- 8. Revisions and thumbnails (D12) -------------------------------------------------

#[test]
fn revisions_list_newest_first_and_thumbnails_come_back_as_base64() {
    let env = new_env();
    let variant_thumbnail = [core_thumbnail(), b"variant".to_vec()].concat();
    let model = env.import_model(
        env.write_source("x.3mf", &variant_3mf(&variant_thumbnail)),
        "Alpha",
        json!([]),
    );
    let model_id = id(&model);
    env.import_one(
        env.source("core-two-objects.3mf"),
        request(
            0,
            "Alpha",
            json!({
                "duplicateAction": "addRevision",
                "targetModelId": model_id,
                "targetExpectedRevision": 1,
            }),
        ),
    );
    let stl = env.import_model(env.source("cube-binary.stl"), "Cube", json!([]));

    let revisions = env.ok("list_model_revisions", json!({ "modelId": model_id }));
    let revisions = revisions.as_array().unwrap();
    assert_eq!(revisions.len(), 2);
    assert_eq!(revisions[0]["sequence"], 2);
    assert_eq!(revisions[1]["sequence"], 1);
    assert_eq!(revisions[0]["origin"], "addedRevision");
    assert_eq!(revisions[0]["sha256"], CORE_3MF_SHA256);
    assert_eq!(revisions[0]["sourceFileName"], "core-two-objects.3mf");
    assert_eq!(revisions[1]["id"], model["currentRevision"]["id"]);
    assert_eq!(revisions[0]["inspection"]["format"], "3mf");
    assert_eq!(revisions[0]["inspection"]["objectCount"], 2);
    assert_eq!(revisions[0]["inspection"]["thumbnails"][0]["width"], 2);
    assert_eq!(revisions[0]["warnings"], json!([]));
    assert!(revisions[0]["inspectorVersion"].as_i64().unwrap() >= 1);
    assert_eq!(revisions[0]["hasThumbnail"], true);
    assert_eq!(revisions[0]["summary"]["format"], "3mf");
    assert!(revisions[0]["sourcePath"]
        .as_str()
        .unwrap()
        .ends_with("core-two-objects.3mf"));

    let thumbnail = env.ok(
        "get_revision_thumbnail",
        json!({ "revisionId": revisions[0]["id"] }),
    );
    assert_eq!(thumbnail["mediaType"], "image/png");
    assert_eq!(thumbnail["width"], 2);
    assert_eq!(thumbnail["height"], 2);
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(thumbnail["dataBase64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, core_thumbnail());
    let older = env.ok(
        "get_revision_thumbnail",
        json!({ "revisionId": revisions[1]["id"] }),
    );
    let decoded = base64::engine::general_purpose::STANDARD
        .decode(older["dataBase64"].as_str().unwrap())
        .unwrap();
    assert_eq!(decoded, variant_thumbnail);

    let none = env.ok(
        "get_revision_thumbnail",
        json!({ "revisionId": stl["currentRevision"]["id"] }),
    );
    assert_eq!(none, Value::Null);
    let unknown = env.err(
        "get_revision_thumbnail",
        json!({ "revisionId": "msr-missing" }),
    );
    assert_eq!(unknown["code"], "NOT_FOUND");
    let unknown_model = env.err("list_model_revisions", json!({ "modelId": "mdl-missing" }));
    assert_eq!(unknown_model["code"], "NOT_FOUND");
}
