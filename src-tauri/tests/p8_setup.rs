//! Task 10 (P8 D4, "Wire types", "Commands"): camera sources and alert
//! defaults in single and batch Printer setup, and the Printers export and
//! import at schema 4.
//!
//! The acceptance rule: setup copying never copies an endpoint, a test
//! result, runtime health, or evidence between Printers. Each batch row's
//! camera is built on its own, from its own Connection or its own host
//! override, and the rows share nothing but the template's shape.

mod common;

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use common::fake_camera::{Answer, FakeCamera, JPEG};
use common::fake_moonraker::FakeMoonraker;
use common::{a_catalog, a_ref_json, invoke};
use farm3d_lib::cameras::services::{CameraServices, CameraTimings};
use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::document_io::{DocumentIo, DocumentKind};
use farm3d_lib::notifications::NAVIGATE_EVENT;
use farm3d_lib::persistence::{MetadataRootLease, Storage};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::ipc::{CallbackFn, InvokeResponseBody};
use tauri::test::{MockRuntime, INVOKE_KEY};
use tauri::webview::InvokeRequest;
use tauri::Listener;

// --- The rig ---------------------------------------------------------------------------

/// An in-memory `DocumentIo`: `save`/`open` return the pre-loaded path,
/// `read` the pre-loaded bytes, and `atomic_write` records the bytes.
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

struct Rig {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    storage: Arc<Storage>,
    documents: Arc<InjectedDocuments>,
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    events: Arc<Mutex<Vec<String>>>,
    /// Every command response and error this rig saw, as text.
    responses: Mutex<Vec<String>>,
}

impl Rig {
    fn new() -> Self {
        let (temp, lease, storage, _database) = common::storage();
        let documents = Arc::new(InjectedDocuments::default());
        let documents_for_services = Arc::clone(&documents) as Arc<dyn DocumentIo>;
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::printers::commands::list_printers,
                farm3d_lib::printers::commands::create_printer,
                farm3d_lib::printers::commands::delete_printer,
                farm3d_lib::printers::commands::archive_printer,
                farm3d_lib::printers::commands::export_printers,
                farm3d_lib::printers::commands::import_printers,
                farm3d_lib::printers::batch::create_printers_batch,
                farm3d_lib::printers::alerts::get_printer_alert_defaults,
                farm3d_lib::cameras::commands::get_printer_camera,
                farm3d_lib::cameras::commands::camera_preview_frame,
                farm3d_lib::attention::commands::list_attention,
            ],
            Arc::clone(&storage),
            Arc::new(a_catalog()),
            temp.path().join("credentials"),
            |_config, _key| None,
            move |services| {
                services.documents = documents_for_services;
                // Every preview fetches: no reuse window.
                services.cameras = Arc::new(CameraServices::new(CameraTimings {
                    preview_min_interval: Duration::ZERO,
                    ..CameraTimings::default()
                }));
            },
        );
        let events = Arc::new(Mutex::new(Vec::new()));
        for name in [STATUS_EVENT, NAVIGATE_EVENT] {
            let sink = Arc::clone(&events);
            app.listen(name, move |event| {
                sink.lock().unwrap().push(event.payload().to_string());
            });
        }
        Self {
            _temp: temp,
            _lease: lease,
            storage,
            documents,
            _app: app,
            webview,
            services,
            events,
            responses: Mutex::new(Vec::new()),
        }
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        let result = invoke(&self.webview, command, body);
        let text = match &result {
            Ok(value) | Err(value) => value.to_string(),
        };
        self.responses.lock().unwrap().push(text);
        result.map(|success| success["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    /// `create_printer` with `extra` merged into a minimal body.
    fn create(&self, name: &str, extra: Value) -> Result<Value, Value> {
        let mut body = json!({"name": name, "catalogRef": a_ref_json()});
        for (key, value) in extra.as_object().unwrap() {
            body[key] = value.clone();
        }
        self.call("create_printer", body)
    }

    fn batch(&self, shared_extra: Value, rows: Value) -> Result<Value, Value> {
        let mut shared = json!({"catalogRef": a_ref_json(), "startSafety": "confirmBedClear"});
        for (key, value) in shared_extra.as_object().unwrap() {
            shared[key] = value.clone();
        }
        self.call(
            "create_printers_batch",
            json!({"input": {
                "batchId": format!("batch-{}", uuid::Uuid::new_v4()),
                "shared": shared,
                "probe": false,
                "rows": rows,
            }}),
        )
    }

    fn count(&self, sql: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    /// The Printer's `printer_cameras` row as JSON (`null` when none).
    fn camera_row(&self, printer_id: &str) -> Value {
        self.storage
            .read(|connection| {
                use rusqlite::OptionalExtension;
                connection
                    .query_row(
                        "SELECT revision, source_kind, webcam_name, webcam_service, web_port, \
                         snapshot_url FROM printer_cameras WHERE printer_id = ?1",
                        [printer_id],
                        |row| {
                            Ok(json!({
                                "revision": row.get::<_, i64>(0)?,
                                "sourceKind": row.get::<_, String>(1)?,
                                "webcamName": row.get::<_, Option<String>>(2)?,
                                "webcamService": row.get::<_, Option<String>>(3)?,
                                "webPort": row.get::<_, Option<i64>>(4)?,
                                "snapshotUrl": row.get::<_, Option<String>>(5)?,
                            }))
                        },
                    )
                    .optional()
                    .map(|row| row.unwrap_or(Value::Null))
            })
            .unwrap()
    }

    fn alert_defaults(&self, printer_id: &str) -> Value {
        self.ok(
            "get_printer_alert_defaults",
            json!({"printerId": printer_id}),
        )
    }

    /// Every `camera.health.changed` health for `printer`, in order.
    fn health_events(&self, printer: &str) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .filter_map(|text| serde_json::from_str::<Value>(text).ok())
            .filter(|event| {
                event["type"] == "camera.health.changed" && event["subject"]["id"] == printer
            })
            .map(|event| event["payload"]["health"].clone())
            .collect()
    }

    /// The `expectedRevisions` precondition for an import: every current
    /// Printer, sorted by id bytes.
    fn expected_revisions(&self) -> Value {
        let mut current: Vec<(String, i64)> = self
            .ok("list_printers", json!({}))
            .as_array()
            .unwrap()
            .iter()
            .map(|printer| {
                (
                    printer["id"].as_str().unwrap().to_string(),
                    printer["revision"].as_i64().unwrap(),
                )
            })
            .collect();
        current.sort_by(|left, right| left.0.as_bytes().cmp(right.0.as_bytes()));
        Value::Array(
            current
                .into_iter()
                .map(|(id, revision)| json!({"id": id, "revision": revision}))
                .collect(),
        )
    }

    fn import(&self, document: &Value) -> Result<Value, Value> {
        *self.documents.open.lock().unwrap() = Some(PathBuf::from("printers-import.json"));
        *self.documents.bytes.lock().unwrap() = Some(serde_json::to_vec(document).unwrap());
        let expected = self.expected_revisions();
        self.call("import_printers", json!({"expectedRevisions": expected}))
    }

    /// Exports and returns (the command's result, the written document).
    fn export(&self) -> (Value, Value) {
        *self.documents.save.lock().unwrap() = Some(PathBuf::from("printers-export.json"));
        let result = self.ok("export_printers", json!({}));
        let writes = self.documents.writes.lock().unwrap();
        let document = serde_json::from_slice(writes.last().unwrap()).unwrap();
        (result, document)
    }

    fn preview(&self, printer_id: &str) -> Result<Vec<u8>, Value> {
        let mut body = json!({"printerId": printer_id});
        body["contractVersion"] = json!(1);
        let response = tauri::test::get_ipc_response(
            &self.webview,
            InvokeRequest {
                cmd: "camera_preview_frame".to_string(),
                callback: CallbackFn(0),
                error: CallbackFn(1),
                url: "tauri://localhost".parse().unwrap(),
                body: body.into(),
                headers: Default::default(),
                invoke_key: INVOKE_KEY.to_string(),
            },
        )?;
        let bytes = match response {
            InvokeResponseBody::Raw(bytes) => bytes,
            InvokeResponseBody::Json(json) => panic!("a preview answered JSON: {json}"),
        };
        let length = u32::from_be_bytes(bytes[..4].try_into().unwrap()) as usize;
        let header: Value = serde_json::from_slice(&bytes[4..4 + length]).unwrap();
        self.responses.lock().unwrap().push(header.to_string());
        Ok(bytes[4 + length..].to_vec())
    }

    /// Every persisted value, as text, except `printer_cameras.snapshot_url`.
    fn persisted_text_except_the_manual_url(&self) -> String {
        self.storage
            .read(|connection| {
                let tables: Vec<String> = connection
                    .prepare("SELECT name FROM sqlite_master WHERE type = 'table'")?
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()?;
                let mut text = String::new();
                for table in tables {
                    let columns: Vec<String> = connection
                        .prepare(&format!("SELECT name FROM pragma_table_info('{table}')"))?
                        .query_map([], |row| row.get(0))?
                        .collect::<rusqlite::Result<_>>()?;
                    for column in columns {
                        if table == "printer_cameras" && column == "snapshot_url" {
                            continue;
                        }
                        let mut statement = connection.prepare(&format!(
                            "SELECT CAST(\"{column}\" AS TEXT) FROM \"{table}\""
                        ))?;
                        for value in
                            statement.query_map([], |row| row.get::<_, Option<String>>(0))?
                        {
                            text.push_str(&value?.unwrap_or_default());
                            text.push('\n');
                        }
                    }
                }
                Ok(text)
            })
            .unwrap()
    }

    fn wait_until(&self, what: &str, done: impl Fn() -> bool) {
        let deadline = Instant::now() + Duration::from_secs(15);
        while !done() {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(5));
        }
    }
}

fn field_path(error: &Value) -> &str {
    error["details"]["fieldPath"].as_str().unwrap_or_default()
}

fn muted_alerts() -> Value {
    json!({
        "offlineAfterMinutes": null,
        "notifications": "muted",
        "snapshotOnIncident": false,
        "snapshotOnCompletion": true,
    })
}

fn default_alerts() -> Value {
    json!({
        "offlineAfterMinutes": 5,
        "notifications": "follow",
        "snapshotOnIncident": true,
        "snapshotOnCompletion": true,
    })
}

fn moonraker_connection(host: &str, port: u16) -> Value {
    json!({"kind": "moonraker", "host": host, "port": port, "useTls": false})
}

fn row_connection(host: &str, port: u16) -> Value {
    json!({"kind": "moonraker", "host": host, "port": port, "credential": {"source": "none"}})
}

// --- Step 1: single create -------------------------------------------------------------

#[test]
fn a_single_create_persists_its_camera_and_alert_defaults() {
    let rig = Rig::new();
    let url = "http://192.0.2.40:8080/snapshot?token=single-create-token";
    let created = rig
        .create(
            "Bay 1",
            json!({
                "camera": {"kind": "snapshotUrl", "snapshotUrl": url},
                "alertDefaults": muted_alerts(),
            }),
        )
        .unwrap();
    let id = created["printer"]["id"].as_str().unwrap().to_string();
    assert!(
        !created.to_string().contains("single-create-token"),
        "create_printer never returns the manual URL: {created}"
    );

    let camera = rig.ok("get_printer_camera", json!({"printerId": id}));
    assert_eq!(camera["revision"], 1);
    assert_eq!(
        camera["source"],
        json!({"kind": "snapshotUrl", "snapshotUrl": url})
    );
    let alerts = rig.alert_defaults(&id);
    assert_eq!(alerts["revision"], 1);
    assert_eq!(alerts["alertDefaults"], muted_alerts());

    // A fresh source: health `unknown`, published, nothing fetched.
    let health = rig.health_events(&id);
    assert_eq!(health.len(), 1, "{health:?}");
    assert_eq!(health[0]["state"], "unknown");
    assert_eq!(health[0]["sourceKind"], "snapshotUrl");
    assert_eq!(health[0]["lastSuccessAt"], Value::Null);
    assert_eq!(health[0]["lastFailureAt"], Value::Null);

    // A host webcam (a Moonraker Connection), with no alert defaults: the
    // defaults apply and no row is written.
    let webcam = rig
        .create(
            "Bay 2",
            json!({
                "connection": moonraker_connection("192.0.2.41", 7125),
                "camera": {"kind": "hostWebcam", "webcamName": " front ", "webcamService": "", "webPort": 8081},
            }),
        )
        .unwrap();
    let webcam_id = webcam["printer"]["id"].as_str().unwrap().to_string();
    assert_eq!(
        rig.camera_row(&webcam_id),
        json!({"revision": 1, "sourceKind": "hostWebcam", "webcamName": "front",
               "webcamService": null, "webPort": 8081, "snapshotUrl": null})
    );
    let defaults = rig.alert_defaults(&webcam_id);
    assert_eq!(defaults["revision"], Value::Null);
    assert_eq!(defaults["alertDefaults"], default_alerts());

    // Neither is set: no rows at all, exactly as before P8.
    let plain = rig.create("Bay 3", json!({})).unwrap();
    let plain_id = plain["printer"]["id"].as_str().unwrap();
    assert_eq!(rig.camera_row(plain_id), Value::Null);
    assert!(rig.health_events(plain_id).is_empty());
    assert_eq!(rig.count("SELECT COUNT(*) FROM printer_cameras"), 2);
    assert_eq!(rig.count("SELECT COUNT(*) FROM printer_alert_defaults"), 1);
}

#[test]
fn an_invalid_camera_or_alert_default_rejects_the_whole_create_with_a_field_path() {
    let rig = Rig::new();
    let cases = [
        (
            json!({"camera": {"kind": "snapshotUrl", "snapshotUrl": "http://user:pw-9f1@192.0.2.42/snap"}}),
            "camera.snapshotUrl",
        ),
        (
            json!({"camera": {"kind": "snapshotUrl", "snapshotUrl": "https://192.0.2.42/snap"}}),
            "camera.snapshotUrl",
        ),
        (
            json!({"camera": {"kind": "hostWebcam", "webcamName": "  "}}),
            "camera.webcamName",
        ),
        (
            json!({"camera": {"kind": "hostWebcam", "webcamName": "front", "webPort": 0},
                   "connection": moonraker_connection("192.0.2.42", 7125)}),
            "camera.webPort",
        ),
        // A host webcam needs a Connection, and one that can list webcams.
        (
            json!({"camera": {"kind": "hostWebcam", "webcamName": "front"}}),
            "camera.kind",
        ),
        (
            json!({"camera": {"kind": "hostWebcam", "webcamName": "front"},
                   "connection": {"kind": "octoprint", "host": "192.0.2.43", "port": 80, "useTls": false}}),
            "camera.kind",
        ),
        (
            json!({"alertDefaults": {"offlineAfterMinutes": 7, "notifications": "follow",
                                     "snapshotOnIncident": true, "snapshotOnCompletion": true}}),
            "alertDefaults.offlineAfterMinutes",
        ),
        (
            json!({"alertDefaults": {"offlineAfterMinutes": 5, "notifications": "loud",
                                     "snapshotOnIncident": true, "snapshotOnCompletion": true}}),
            "alertDefaults.notifications",
        ),
        (
            json!({"alertDefaults": {"offlineAfterMinutes": 5, "notifications": "follow",
                                     "snapshotOnCompletion": true}}),
            "alertDefaults.snapshotOnIncident",
        ),
    ];
    for (extra, path) in cases {
        let error = rig.create("Rejected", extra.clone()).unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{extra}: {error}");
        assert_eq!(field_path(&error), path, "{extra}: {error}");
        assert!(!error.to_string().contains("pw-9f1"), "{error}");
        assert!(!error.to_string().contains("192.0.2.42"), "{error}");
    }

    // Written in the create transaction: a failure later in it (an invalid
    // slot layout, checked at insert) leaves no camera or alert row.
    let error = rig
        .create(
            "Rolled back",
            json!({
                "camera": {"kind": "snapshotUrl", "snapshotUrl": "http://192.0.2.44/snap"},
                "alertDefaults": muted_alerts(),
                "slotLayout": [{"name": "Main"}, {"name": "main"}],
            }),
        )
        .unwrap_err();
    assert_eq!(error["code"], "VALIDATION");
    assert_eq!(field_path(&error), "slots");
    assert_eq!(rig.count("SELECT COUNT(*) FROM printers"), 0);
    assert_eq!(rig.count("SELECT COUNT(*) FROM printer_cameras"), 0);
    assert_eq!(rig.count("SELECT COUNT(*) FROM printer_alert_defaults"), 0);
    assert!(rig
        .events
        .lock()
        .unwrap()
        .iter()
        .all(|event| !event.contains("camera.health")));
}

// --- Step 1: batch ---------------------------------------------------------------------

#[test]
fn a_snapshot_url_template_builds_each_rows_url_from_its_own_host() {
    let rig = Rig::new();
    let pokes_before = rig.services.attention.pokes();
    let output = rig
        .batch(
            json!({
                "cameraTemplate": {"kind": "snapshotUrl", "path": "/snapshot?token=batch-template-token", "port": 8080},
                "alertDefaults": muted_alerts(),
            }),
            json!([
                {"rowId": "r0", "name": "Row 0", "connection": row_connection("192.0.2.11", 7125)},
                {"rowId": "r1", "name": "Row 1", "connection": row_connection("192.0.2.12", 7125),
                 "cameraHostOverride": " 192.0.2.51 "},
                {"rowId": "r2", "name": "Row 2", "cameraHostOverride": "192.0.2.52"},
                {"rowId": "r3", "name": "Row 3"},
                {"rowId": "r4", "name": "Row 4", "connection": row_connection("192.0.2.14", 7125),
                 "cameraHostOverride": ""},
            ]),
        )
        .unwrap();
    let rows = output["rows"].as_array().unwrap();
    assert_eq!(rows.len(), 5);
    assert!(
        !output.to_string().contains("batch-template-token"),
        "{output}"
    );
    assert!(!output.to_string().contains("192.0.2.51"), "{output}");
    assert!(!output.to_string().contains("192.0.2.52"), "{output}");

    let expected = [
        (
            "created",
            Some("http://192.0.2.11:8080/snapshot?token=batch-template-token"),
        ),
        (
            "created",
            Some("http://192.0.2.51:8080/snapshot?token=batch-template-token"),
        ),
        (
            "createdSetupIncomplete",
            Some("http://192.0.2.52:8080/snapshot?token=batch-template-token"),
        ),
        // No Connection and no override: no camera, Setup incomplete, as today.
        ("createdSetupIncomplete", None),
        (
            "created",
            Some("http://192.0.2.14:8080/snapshot?token=batch-template-token"),
        ),
    ];
    let mut ids = Vec::new();
    for (row, (outcome, url)) in rows.iter().zip(expected) {
        assert_eq!(row["outcome"], outcome, "{row}");
        assert_eq!(row["errors"], json!([]), "{row}");
        let id = row["printer"]["id"].as_str().unwrap().to_string();
        match url {
            Some(url) => assert_eq!(
                rig.camera_row(&id),
                json!({"revision": 1, "sourceKind": "snapshotUrl", "webcamName": null,
                       "webcamService": null, "webPort": null, "snapshotUrl": url}),
                "{row}"
            ),
            None => assert_eq!(rig.camera_row(&id), Value::Null),
        }
        // Every Printer gets its own copy of the shared alert defaults.
        let alerts = rig.alert_defaults(&id);
        assert_eq!(
            (alerts["revision"].clone(), alerts["alertDefaults"].clone()),
            (json!(1), muted_alerts())
        );
        ids.push(id);
    }
    // No two Printers share a camera row, and nothing but configuration was
    // written: no health beyond a fresh `unknown`, no test result, no
    // snapshot.
    assert_eq!(
        rig.count("SELECT COUNT(DISTINCT printer_id) FROM printer_cameras"),
        4
    );
    assert_eq!(rig.count("SELECT COUNT(*) FROM camera_snapshots"), 0);
    assert_eq!(rig.count("SELECT COUNT(*) FROM printer_alert_defaults"), 5);
    for id in &ids[..3] {
        let health = rig.services.cameras.health(id).expect("a fresh health");
        let health = serde_json::to_value(health).unwrap();
        assert_eq!(health["state"], "unknown");
        assert_eq!(health["lastSuccessAt"], Value::Null);
        assert_eq!(health["lastFailureAt"], Value::Null);
        assert_eq!(health["lastFailureKind"], Value::Null);
        assert!(!rig.services.cameras.has_preview_frame(id));
    }
    assert!(rig.services.cameras.health(&ids[3]).is_none());
    // The Attention projector re-reads the new Printers' alert defaults.
    assert!(rig.services.attention.pokes() > pokes_before);
}

#[test]
fn a_bad_camera_template_fails_the_batch_and_a_bad_override_rejects_its_row() {
    let rig = Rig::new();
    let one_row =
        json!([{"rowId": "r0", "name": "Row 0", "connection": row_connection("192.0.2.11", 7125)}]);
    for (template, path) in [
        (
            json!({"kind": "snapshotUrl", "path": "snapshot", "port": 8080}),
            "shared.cameraTemplate.path",
        ),
        (
            json!({"kind": "snapshotUrl", "path": "/snap#frag", "port": 8080}),
            "shared.cameraTemplate.path",
        ),
        (
            json!({"kind": "snapshotUrl", "path": "/snap shot", "port": 8080}),
            "shared.cameraTemplate.path",
        ),
        (
            json!({"kind": "snapshotUrl", "path": format!("/{}", "a".repeat(1024)), "port": 8080}),
            "shared.cameraTemplate.path",
        ),
        (
            json!({"kind": "snapshotUrl", "path": "/snap", "port": 0}),
            "shared.cameraTemplate.port",
        ),
        (
            json!({"kind": "hostWebcam", "webcamName": "", "webPort": null}),
            "shared.cameraTemplate.webcamName",
        ),
        (
            json!({"kind": "hostWebcam", "webcamName": "front", "webPort": 0}),
            "shared.cameraTemplate.webPort",
        ),
    ] {
        let error = rig
            .batch(json!({"cameraTemplate": template}), one_row.clone())
            .unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{template}: {error}");
        assert_eq!(field_path(&error), path, "{template}: {error}");
    }
    assert_eq!(rig.count("SELECT COUNT(*) FROM printers"), 0);

    let output = rig
        .batch(
            json!({"cameraTemplate": {"kind": "snapshotUrl", "path": "/snap", "port": 8080}}),
            json!([
                {"rowId": "r0", "name": "Row 0", "cameraHostOverride": "user:pw-7a1@192.0.2.60"},
                {"rowId": "r1", "name": "Row 1", "cameraHostOverride": "192.0.2.61/other"},
                {"rowId": "r2", "name": "Row 2", "cameraHostOverride": "192.0.2.62:9000"},
                {"rowId": "r3", "name": "Row 3", "cameraHostOverride": "192.0.2.63"},
            ]),
        )
        .unwrap();
    let rows = output["rows"].as_array().unwrap();
    for row in &rows[..3] {
        assert_eq!(row["outcome"], "rejected", "{row}");
        assert_eq!(row["errors"][0]["code"], "VALIDATION");
        assert_eq!(
            row["errors"][0]["fieldPath"],
            format!(
                "rows[{}].cameraHostOverride",
                &row["rowId"].as_str().unwrap()[1..]
            )
        );
    }
    assert!(!output.to_string().contains("pw-7a1"), "{output}");
    assert!(!output.to_string().contains("192.0.2.6"), "{output}");
    assert_eq!(rows[3]["outcome"], "createdSetupIncomplete");
    assert_eq!(rig.count("SELECT COUNT(*) FROM printers"), 1);

    // A host override only means something for a snapshot URL template.
    let output = rig
        .batch(
            json!({"cameraTemplate": {"kind": "hostWebcam", "webcamName": "front", "webPort": null}}),
            json!([{"rowId": "r0", "name": "Row 9", "connection": row_connection("192.0.2.19", 7125),
                    "cameraHostOverride": "192.0.2.69"}]),
        )
        .unwrap();
    assert_eq!(output["rows"][0]["outcome"], "rejected");
    assert_eq!(
        output["rows"][0]["errors"][0]["fieldPath"],
        "rows[0].cameraHostOverride"
    );
}

#[test]
fn a_host_webcam_template_resolves_against_each_rows_own_connection() {
    let rig = Rig::new();
    let camera = FakeCamera::start(Answer::Jpeg);
    let first = FakeMoonraker::start();
    let second = FakeMoonraker::start();
    let webcams = |moonraker: &FakeMoonraker, snapshot: &str| {
        let entry = json!({"name": "front", "service": "mjpegstreamer", "enabled": true, "snapshot_url": snapshot});
        moonraker.with_state(|state| state.webcams = vec![entry]);
    };
    webcams(&first, "/first/snapshot?token=first-row");
    webcams(&second, "/second/snapshot?token=second-row");

    let output = rig
        .batch(
            json!({"cameraTemplate": {"kind": "hostWebcam", "webcamName": "front", "webPort": camera.port}}),
            json!([
                {"rowId": "a", "name": "Webcam A", "connection": row_connection("127.0.0.1", first.port)},
                {"rowId": "b", "name": "Webcam B", "connection": row_connection("127.0.0.1", second.port)},
                // No Connection: a host webcam has nothing to resolve against.
                {"rowId": "c", "name": "Webcam C"},
                // An adapter that can't list webcams keeps its Printer and
                // Connection, and gets no camera.
                {"rowId": "d", "name": "Webcam D", "connection": {"kind": "octoprint", "host": "192.0.2.30",
                 "port": 80, "credential": {"source": "none"}}},
            ]),
        )
        .unwrap();
    let rows = output["rows"].as_array().unwrap();
    let id = |index: usize| rows[index]["printer"]["id"].as_str().unwrap().to_string();
    for index in 0..2 {
        assert_eq!(rows[index]["outcome"], "created", "{}", rows[index]);
        assert_eq!(
            rig.camera_row(&id(index)),
            json!({"revision": 1, "sourceKind": "hostWebcam", "webcamName": "front",
                   "webcamService": null, "webPort": camera.port, "snapshotUrl": null})
        );
    }
    assert_eq!(rows[2]["outcome"], "createdSetupIncomplete");
    assert_eq!(rig.camera_row(&id(2)), Value::Null);
    assert_eq!(rows[3]["outcome"], "created", "{}", rows[3]);
    assert_eq!(rows[3]["errors"][0]["code"], "VALIDATION");
    assert_eq!(
        rows[3]["errors"][0]["fieldPath"],
        "shared.cameraTemplate.kind"
    );
    assert_eq!(rig.camera_row(&id(3)), Value::Null);

    // Each Printer's webcam resolves through its own Connection at fetch
    // time: the same stored name, two different snapshot paths.
    assert_eq!(rig.preview(&id(0)).unwrap(), JPEG);
    assert_eq!(rig.preview(&id(1)).unwrap(), JPEG);
    let targets: Vec<String> = camera
        .requests()
        .into_iter()
        .map(|request| request.target)
        .collect();
    assert_eq!(
        targets,
        [
            "/first/snapshot?token=first-row",
            "/second/snapshot?token=second-row"
        ]
    );
    // The first Printer's success is its own: the second's health moved on
    // its own fetch, never copied.
    rig.wait_until("both healths", || {
        [id(0), id(1)].iter().all(|printer| {
            rig.services
                .cameras
                .health(printer)
                .is_some_and(|health| serde_json::to_value(health).unwrap()["state"] == "ok")
        })
    });
    webcams(&second, "/second/missing");
    camera.answer(Answer::Status(404));
    assert!(rig.preview(&id(1)).is_err());
    let first_health = serde_json::to_value(rig.services.cameras.health(&id(0)).unwrap()).unwrap();
    assert_eq!(first_health["state"], "ok");
}

// --- Step 1: export and import at schema 4 ---------------------------------------------

fn document(schema_version: i64, printers: Value) -> Value {
    json!({"schemaVersion": schema_version, "exportedAt": "2026-09-28T00:00:00Z", "printers": printers})
}

fn imported_printer(id: &str, extra: Value) -> Value {
    let mut printer = json!({
        "id": id,
        "revision": 1,
        "name": "Imported",
        "catalogRef": a_ref_json(),
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
fn export_schema_4_round_trips_cameras_and_alert_defaults() {
    let rig = Rig::new();
    let url = "http://192.0.2.45:8080/snap?token=export-token";
    let manual = rig
        .create(
            "Manual",
            json!({"camera": {"kind": "snapshotUrl", "snapshotUrl": url}, "alertDefaults": muted_alerts()}),
        )
        .unwrap()["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let webcam = rig
        .create(
            "Webcam",
            json!({
                "connection": moonraker_connection("192.0.2.46", 7125),
                "camera": {"kind": "hostWebcam", "webcamName": "front", "webcamService": "mjpegstreamer", "webPort": null},
            }),
        )
        .unwrap()["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    let plain = rig.create("Plain", json!({})).unwrap()["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let (result, exported) = rig.export();
    assert_eq!(result["status"], "exported");
    assert_eq!(result["recordCount"], 3);
    assert!(!result.to_string().contains("export-token"), "{result}");
    assert_eq!(exported["schemaVersion"], 4);
    let by_id = |id: &str| {
        exported["printers"]
            .as_array()
            .unwrap()
            .iter()
            .find(|printer| printer["id"] == id)
            .unwrap()
            .clone()
    };
    // The file carries the manual URL, as it carries the Connection host.
    assert_eq!(
        by_id(&manual)["camera"],
        json!({"kind": "snapshotUrl", "snapshotUrl": url})
    );
    assert_eq!(by_id(&manual)["alertDefaults"], muted_alerts());
    assert_eq!(
        by_id(&webcam)["camera"],
        json!({"kind": "hostWebcam", "webcamName": "front", "webcamService": "mjpegstreamer", "webPort": null})
    );
    assert_eq!(by_id(&webcam)["alertDefaults"], Value::Null);
    assert_eq!(by_id(&plain)["camera"], Value::Null);
    assert_eq!(by_id(&plain)["alertDefaults"], Value::Null);

    // Into a fresh database, the export comes back exactly as it was.
    let fresh = Rig::new();
    let applied = fresh.import(&exported).unwrap();
    assert_eq!(applied["status"], "applied");
    assert!(!applied.to_string().contains("export-token"), "{applied}");
    assert_eq!(
        fresh.ok("get_printer_camera", json!({"printerId": manual}))["source"]["snapshotUrl"],
        url
    );
    assert_eq!(
        fresh.alert_defaults(&manual)["alertDefaults"],
        muted_alerts()
    );
    assert_eq!(fresh.camera_row(&webcam)["webcamService"], "mjpegstreamer");
    assert_eq!(fresh.alert_defaults(&webcam)["revision"], Value::Null);
    assert_eq!(fresh.camera_row(&plain), Value::Null);
    let (_, again) = fresh.export();
    for id in [&manual, &webcam, &plain] {
        for key in ["camera", "alertDefaults"] {
            assert_eq!(
                again["printers"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .find(|p| p["id"] == **id)
                    .unwrap()[key],
                by_id(id)[key]
            );
        }
    }
    assert_eq!(
        fresh.health_events(&manual).last().unwrap()["state"],
        "unknown"
    );

    // Over the same Printers: an import that keeps a Printer's source
    // publishes nothing for it; one that changes it resets its health.
    let before = rig.health_events(&manual).len();
    rig.import(&exported).unwrap();
    assert_eq!(
        rig.health_events(&manual).len(),
        before,
        "an identical source publishes nothing"
    );
    assert_eq!(
        rig.ok("get_printer_camera", json!({"printerId": manual}))["source"]["snapshotUrl"],
        url
    );
    let mut changed = exported.clone();
    for printer in changed["printers"].as_array_mut().unwrap() {
        if printer["id"] == manual {
            printer["camera"] = Value::Null;
        }
        if printer["id"] == plain {
            printer["camera"] =
                json!({"kind": "snapshotUrl", "snapshotUrl": "http://192.0.2.47/snap"});
        }
    }
    rig.import(&changed).unwrap();
    assert_eq!(
        rig.health_events(&manual).last().unwrap()["state"],
        "notConfigured"
    );
    assert_eq!(
        rig.health_events(&plain).last().unwrap()["state"],
        "unknown"
    );
    assert_eq!(
        rig.health_events(&plain).last().unwrap()["sourceKind"],
        "snapshotUrl"
    );

    // A Printer the import removes is forgotten by the camera services.
    assert!(rig.services.cameras.health(&plain).is_some());
    let kept: Vec<Value> = changed["printers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|printer| printer["id"] != plain)
        .cloned()
        .collect();
    rig.import(&document(4, Value::Array(kept))).unwrap();
    assert!(rig.services.cameras.health(&plain).is_none());
}

#[test]
fn schemas_2_and_3_still_import_with_no_camera_and_the_default_alerts() {
    let rig = Rig::new();
    for version in [2, 3] {
        let applied = rig
            .import(&document(
                version,
                json!([imported_printer("prn-legacy", json!({}))]),
            ))
            .unwrap();
        assert_eq!(applied["status"], "applied", "v{version}");
        assert_eq!(rig.camera_row("prn-legacy"), Value::Null);
        let alerts = rig.alert_defaults("prn-legacy");
        assert_eq!(alerts["revision"], Value::Null);
        assert_eq!(alerts["alertDefaults"], default_alerts());
    }
    // A schema-3 document can't carry the schema-4 fields.
    for key in ["camera", "alertDefaults"] {
        let mut extra = serde_json::Map::new();
        extra.insert(key.to_string(), Value::Null);
        let error = rig
            .import(&document(
                3,
                json!([imported_printer("prn-legacy", Value::Object(extra))]),
            ))
            .unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{key}");
    }
    let future = rig.import(&document(5, json!([]))).unwrap_err();
    assert_eq!(future["code"], "UNSUPPORTED_SCHEMA_VERSION");
}

#[test]
fn an_invalid_imported_camera_or_alert_default_rejects_the_import() {
    let rig = Rig::new();
    rig.create("Keep", json!({})).unwrap();
    for (extra, path) in [
        (
            json!({"camera": {"kind": "snapshotUrl", "snapshotUrl": "http://user:pw-5d0@192.0.2.48/snap"}}),
            "printers[0].camera.snapshotUrl",
        ),
        (
            json!({"camera": {"kind": "hostWebcam", "webcamName": "front"}}),
            "printers[0].camera.kind",
        ),
        (json!({"camera": {"kind": "rtsp"}}), "printers[0].camera"),
        (
            json!({"alertDefaults": {"offlineAfterMinutes": 2, "notifications": "follow",
                                     "snapshotOnIncident": true, "snapshotOnCompletion": true}}),
            "printers[0].alertDefaults.offlineAfterMinutes",
        ),
    ] {
        let error = rig
            .import(&document(
                4,
                json!([imported_printer("prn-new", extra.clone())]),
            ))
            .unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{extra}: {error}");
        assert_eq!(field_path(&error), path, "{extra}: {error}");
        assert!(!error.to_string().contains("pw-5d0"), "{error}");
        assert!(!error.to_string().contains("192.0.2.48"), "{error}");
    }
    assert_eq!(
        rig.count("SELECT COUNT(*) FROM printers WHERE name = 'Keep'"),
        1
    );
}

/// P8 D8: an import that would replace a Printer with an Incident fails
/// `EVIDENCE_EXISTS` before anything is attempted, and writes nothing.
#[test]
fn an_import_over_a_printer_with_incident_history_fails_evidence_exists() {
    let rig = Rig::new();
    let id = rig
        .create(
            "With history",
            json!({"camera": {"kind": "snapshotUrl", "snapshotUrl": "http://192.0.2.49/snap"},
                   "alertDefaults": muted_alerts()}),
        )
        .unwrap()["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();
    rig.storage
        .write(|tx| {
            tx.execute(
                "INSERT INTO incidents(id, kind, printer_id, job_id, printer_snapshot_json, opened_at) \
                 VALUES ('inc-carry', 'printer.hostFailed', ?1, NULL, '{}', '2026-09-28T00:00:00Z')",
                [&id],
            )?;
            Ok(())
        })
        .unwrap();
    let error = rig
        .import(&document(
            4,
            json!([imported_printer("prn-other", json!({}))]),
        ))
        .unwrap_err();
    assert_eq!(error["code"], "EVIDENCE_EXISTS", "{error}");
    assert_eq!(
        error["details"],
        json!({"printerIds": [id], "incidentIds": ["inc-carry"], "snapshotIds": []})
    );
    assert!(!error.to_string().contains("192.0.2.49"), "{error}");
    assert_eq!(rig.count("SELECT COUNT(*) FROM printers"), 1);
    assert_eq!(rig.camera_row(&id)["snapshotUrl"], "http://192.0.2.49/snap");
    assert_eq!(rig.alert_defaults(&id)["alertDefaults"], muted_alerts());
    assert_eq!(rig.count("SELECT COUNT(*) FROM incidents"), 1);
}

// --- Step 1: the seeded-secret corpus --------------------------------------------------

const SEED_PASS: &str = "SEEDED-P8-SETUP-PASS-6c2e";
const SEED_QUERY_TOKEN: &str = "SEEDED-P8-SETUP-TOKEN-91ab";
const SEED_TEMPLATE_TOKEN: &str = "SEEDED-P8-TEMPLATE-TOKEN-4d7f";
/// RFC 5737 hosts that appear only inside camera URLs (never as a
/// Connection host, which is persisted and returned by design).
const SEED_HOSTS: [&str; 5] = [
    "192.0.2.77",
    "192.0.2.78",
    "192.0.2.81",
    "192.0.2.82",
    "192.0.2.83",
];

/// Global constraint 3 through setup: a manual URL with userinfo (rejected,
/// not echoed), a manual URL with a query token, and a batch template plus
/// row overrides from RFC 5737 hosts. Scans every command response (except
/// `get_printer_camera`), every error, every event, the batch request's
/// `Debug`, and every persisted value except `printer_cameras.snapshot_url`.
/// The export file carries the manual URL; the command result doesn't.
#[test]
fn the_seeded_corpus_stays_in_the_camera_column_and_the_export_file() {
    let rig = Rig::new();
    let userinfo_url =
        format!("http://operator:{SEED_PASS}@192.0.2.77:8080/snap?token={SEED_QUERY_TOKEN}");
    let token_url = format!("http://192.0.2.78:8080/snap?token={SEED_QUERY_TOKEN}");

    let rejected = rig
        .create(
            "Userinfo",
            json!({"camera": {"kind": "snapshotUrl", "snapshotUrl": userinfo_url}}),
        )
        .unwrap_err();
    assert_eq!(field_path(&rejected), "camera.snapshotUrl");
    let single = rig
        .create(
            "Token",
            json!({"camera": {"kind": "snapshotUrl", "snapshotUrl": token_url}}),
        )
        .unwrap()["printer"]["id"]
        .as_str()
        .unwrap()
        .to_string();

    let template = json!({"kind": "snapshotUrl", "path": format!("/snap?token={SEED_TEMPLATE_TOKEN}"), "port": 8080});
    let rows = json!([
        {"rowId": "a", "name": "Seed A", "connection": row_connection("printer-a.example.test", 7125),
         "cameraHostOverride": "192.0.2.81"},
        {"rowId": "b", "name": "Seed B", "cameraHostOverride": "192.0.2.82"},
        {"rowId": "c", "name": "Seed C", "connection": row_connection("printer-c.example.test", 7125)},
        {"rowId": "d", "name": "Seed D", "cameraHostOverride": format!("operator:{SEED_PASS}@192.0.2.83")},
    ]);
    let input: farm3d_lib::printers::batch::CreatePrintersBatchInput = serde_json::from_value(json!({
        "batchId": "debug", "shared": {"catalogRef": a_ref_json(), "startSafety": "confirmBedClear",
        "cameraTemplate": template}, "probe": false, "rows": rows,
    }))
    .unwrap();
    let debug = format!("{input:?}");
    let output = rig
        .batch(json!({"cameraTemplate": template}), rows)
        .unwrap();
    assert_eq!(output["rows"][3]["outcome"], "rejected");
    assert_eq!(
        output["rows"][3]["errors"][0]["fieldPath"],
        "rows[3].cameraHostOverride"
    );
    let ids: Vec<String> = output["rows"].as_array().unwrap()[..3]
        .iter()
        .map(|row| row["printer"]["id"].as_str().unwrap().to_string())
        .collect();
    assert_eq!(
        rig.camera_row(&ids[0])["snapshotUrl"],
        format!("http://192.0.2.81:8080/snap?token={SEED_TEMPLATE_TOKEN}")
    );
    assert_eq!(
        rig.camera_row(&ids[2])["snapshotUrl"],
        format!("http://printer-c.example.test:8080/snap?token={SEED_TEMPLATE_TOKEN}")
    );
    rig.ok("list_printers", json!({}));
    rig.ok("list_attention", json!({}));
    rig.alert_defaults(&single);

    let (result, exported) = rig.export();
    let file = exported.to_string();
    assert!(
        file.contains(&token_url),
        "the export file carries the manual URL"
    );
    assert!(file.contains(SEED_TEMPLATE_TOKEN));
    assert!(file.contains("192.0.2.81"));
    rig.import(&exported).unwrap();
    rig.ok("list_printers", json!({}));

    // The single exception, checked and left out of the scan.
    let camera = invoke(
        &rig.webview,
        "get_printer_camera",
        json!({"contractVersion": 1, "printerId": single}),
    )
    .unwrap();
    assert!(camera.to_string().contains(SEED_QUERY_TOKEN));

    let mut corpus = vec![
        SEED_PASS.to_string(),
        SEED_QUERY_TOKEN.to_string(),
        SEED_TEMPLATE_TOKEN.to_string(),
        userinfo_url.clone(),
        token_url.clone(),
    ];
    corpus.extend(SEED_HOSTS.iter().map(|host| host.to_string()));
    let check = |what: &str, text: &str| {
        for needle in &corpus {
            assert!(
                !text.contains(needle.as_str()),
                "{what} leaked {needle:?}: {text}"
            );
        }
    };
    let responses = rig.responses.lock().unwrap().clone();
    assert!(responses.len() >= 9, "{}", responses.len());
    for response in &responses {
        check("a command response", response);
    }
    check("the export result", &result.to_string());
    let events = rig.events.lock().unwrap().join("\n");
    assert!(events.contains("camera.health.changed"));
    check("an event", &events);
    check("the batch request's Debug", &debug);
    let persisted = rig.persisted_text_except_the_manual_url();
    assert!(persisted.contains(&single), "the scan read the rows");
    check("a persisted row", &persisted);
}

/// The touched setup and camera modules have no logging path at all (the
/// crate logs with `eprintln!` only, and has no `log` or `tracing`
/// dependency), so no camera URL can reach a log.
#[test]
fn the_setup_modules_never_log() {
    for (name, source) in [
        (
            "printers/create.rs",
            include_str!("../src/printers/create.rs"),
        ),
        (
            "printers/batch.rs",
            include_str!("../src/printers/batch.rs"),
        ),
        (
            "printers/commands.rs",
            include_str!("../src/printers/commands.rs"),
        ),
        (
            "printers/repository.rs",
            include_str!("../src/printers/repository.rs"),
        ),
        (
            "printers/alerts.rs",
            include_str!("../src/printers/alerts.rs"),
        ),
        (
            "cameras/config.rs",
            include_str!("../src/cameras/config.rs"),
        ),
    ] {
        // No `log`/`tracing` dependency exists; these are the only log paths.
        for needle in ["println!", "print!(", "dbg!("] {
            assert!(!source.contains(needle), "{name} contains {needle}");
        }
    }
}
