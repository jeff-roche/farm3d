//! P9 Task 9 (spec D13, D12, D18, ADR-0017): the diagnostics bundle and
//! About, over the every-domain Farm (`tests/p9_farm`) with the full secret
//! corpus seeded into the credential store, Connection hosts and Host
//! Operation endpoints, the stored camera URL, every user-authored name and
//! note, host telemetry job names, linked and source paths, the Slicer
//! runtime paths, and log lines from error paths.
//!
//! - Each section holds its expected facts, and no corpus item reaches any
//!   entry (raw, compressed, or decompressed).
//! - Pseudonyms are per-bundle ordinals: stable within one bundle (health,
//!   logs, and recent problems agree), different between bundles, and
//!   every log line's `ids` is rewritten.
//! - Printers named after state words don't block the export.
//! - A deliberately leaking collector (the test-only entry hook) is caught
//!   for every corpus class, gives `DIAGNOSTICS_REDACTION_FAILED` naming
//!   the section, and writes no file.
//! - D18: the dialog owns the path, a cancel records nothing, a replay
//!   returns the cached result, and a reuse is `VALIDATION`.

mod common;
mod p9_farm;
#[path = "common/secrets.rs"]
mod secrets;

use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::io::Read;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use serde_json::{json, Value};
use tauri::test::MockRuntime;

use farm3d_lib::backup::dialogs::PortabilityDialogs;
use farm3d_lib::connections::supervisor::PrinterSetupFacts;
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus,
};
use farm3d_lib::contracts::command::CommandError;
use farm3d_lib::diagnostics::bundle::DiagnosticsSection;
use farm3d_lib::diagnostics::log::LogId;
use farm3d_lib::diagnostics::DiagnosticsServices;
use farm3d_lib::persistence::{RepositoryError, StorageError};
use farm3d_lib::RuntimeServices;

use p9_farm::{ids, Farm};

// --- the extra corpus: every user-authored or host-supplied text ---------------------

const MODEL_NAME: &str = "P9 Secret Model Beta";
const LINKED_MODEL_NAME: &str = "P9 Secret Linked Model";
const LINKED_PATH: &str = "/home/p9-operator/farm3d/linked/part.stl";
const SPOOL_MANUFACTURER: &str = "P9 Secret Maker";
const SPOOL_PRODUCT: &str = "P9 Secret Product";
const SPOOL_COLOR: &str = "P9 Secret Color";
const STORAGE_LABEL: &str = "P9 Secret Shelf";
const TARE_NAME: &str = "P9 Secret Tare";
const PROJECT_NAME: &str = "P9 Secret Project";
const PRINTER_NOTES: &str = "P9 secret notes about the printer";
const LOCATION: &str = "P9 Secret Bay";
const INCIDENT_NOTE: &str = "P9 secret incident note";
const JOB_NAME: &str = "p9-secret-job-name.gcode";
const MIGRATION_SOURCE: &str = "p9-secret-migration-source.json";
const MIGRATION_MESSAGE: &str = "P9 secret migration message";
/// A credential ref only `pending_credential_cleanup` names.
const ORPHAN_REF: &str = "farm3d/printer/prn-p9-orphan/apikey";

/// Printers named after words the bundle's enum values use.
const PRINTER_STATE_WORD: &str = "prn-c";
const PRINTER_ADAPTER_WORD: &str = "prn-d";
const PRINTER_PROFILE_ONLY: &str = "prn-e";

/// Everything a bundle must not contain: the shared corpus and the texts
/// above.
fn every_needle() -> Vec<&'static str> {
    let mut needles = secrets::FULL_CORPUS.to_vec();
    needles.extend([
        MODEL_NAME,
        LINKED_MODEL_NAME,
        LINKED_PATH,
        SPOOL_MANUFACTURER,
        SPOOL_PRODUCT,
        SPOOL_COLOR,
        STORAGE_LABEL,
        TARE_NAME,
        PROJECT_NAME,
        PRINTER_NOTES,
        LOCATION,
        INCIDENT_NOTE,
        JOB_NAME,
        MIGRATION_SOURCE,
        MIGRATION_MESSAGE,
        ORPHAN_REF,
        ids::CREDENTIAL_REF_A,
        ids::CREDENTIAL_REF_B,
        secrets::HOME_PATH,
    ]);
    needles
}

// --- serialization: the global log is shared by every test here ---------------------

static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// --- the Farm ------------------------------------------------------------------------

fn sql(farm: &Farm, statement: &str, values: &[&dyn rusqlite::ToSql]) {
    let connection = rusqlite::Connection::open(farm.paths().database()).unwrap();
    connection
        .execute_batch("PRAGMA foreign_keys = ON;")
        .unwrap();
    connection.execute(statement, values).unwrap();
}

/// The every-domain Farm plus the texts above, three more Printers, a
/// linked Model, a migration warning, an orphaned credential ref, and a
/// Host Operation endpoint on the corpus host name.
fn diagnostics_farm() -> Farm {
    let farm = Farm::with_every_domain();
    let now = ids::NOW;
    sql(
        &farm,
        "UPDATE printers SET notes = ?1, location = ?2 WHERE id = ?3",
        &[&PRINTER_NOTES, &LOCATION, &ids::PRINTER_A],
    );
    sql(
        &farm,
        "UPDATE library_models SET name = ?1 WHERE id = ?2",
        &[&MODEL_NAME, &ids::MODEL_3MF],
    );
    sql(
        &farm,
        "UPDATE library_projects SET name = ?1 WHERE id = ?2",
        &[&PROJECT_NAME, &ids::PROJECT],
    );
    sql(
        &farm,
        "INSERT INTO spool_tares(id, revision, name, weight_mg, created_at, updated_at)
         VALUES ('tare-p9', 1, ?1, 200000, ?2, ?2)",
        &[&TARE_NAME, &now],
    );
    sql(
        &farm,
        "UPDATE spools SET manufacturer = ?1, product = ?2, color_name = ?3,
                storage_label = ?4, tare_id = 'tare-p9'
          WHERE id = 'spl-a'",
        &[
            &SPOOL_MANUFACTURER,
            &SPOOL_PRODUCT,
            &SPOOL_COLOR,
            &STORAGE_LABEL,
        ],
    );
    sql(
        &farm,
        "INSERT INTO library_models(id, revision, name, format, storage_mode, linked_path,
           link_state, created_at, updated_at)
         VALUES ('mdl-linked', 1, ?1, 'stl', 'linked', ?2, 'ok', ?3, ?3)",
        &[&LINKED_MODEL_NAME, &LINKED_PATH, &now],
    );
    sql(
        &farm,
        "UPDATE printer_status_snapshots SET telemetry_json = ?1 WHERE printer_id = ?2",
        &[
            &json!({ "jobName": JOB_NAME, "note": secrets::HEADER_LINE }).to_string(),
            &ids::PRINTER_A,
        ],
    );
    sql(
        &farm,
        "UPDATE host_operations SET endpoint_json = ?1, host_path = ?2 WHERE id = 'hop-a'",
        &[
            &json!({ "kind": "moonraker", "host": secrets::HOST_NAME, "port": 7125 }).to_string(),
            &JOB_NAME,
        ],
    );
    sql(
        &farm,
        "INSERT INTO incident_events(id, incident_id, sequence, kind, detail_json, at)
         VALUES ('iev-note', 'inc-a', 3, 'noteAdded', ?1, ?2)",
        &[
            &json!({ "kind": "noteAdded", "text": INCIDENT_NOTE }).to_string(),
            &now,
        ],
    );
    sql(
        &farm,
        "INSERT INTO migration_warnings(id, code, source_name, message, details_json, created_at)
         VALUES ('0b6d3f1e-8c1a-4f2b-9a3d-5e6f7a8b9c0d', 'LEGACY_SETTINGS_INVALID', ?1, ?2,
                 ?3, ?4)",
        &[
            &MIGRATION_SOURCE,
            &MIGRATION_MESSAGE,
            &json!({ "path": secrets::HOME_PATH }).to_string(),
            &now,
        ],
    );
    sql(
        &farm,
        "INSERT INTO pending_credential_cleanup(credential_ref, printer_id, reason, created_at)
         VALUES (?1, NULL, 'printer_deleted', ?2)",
        &[&ORPHAN_REF, &now],
    );
    // State-word names: a severity, an adapter kind, and a Profile-only
    // Printer named after a connection state.
    for (id, name) in [
        (PRINTER_STATE_WORD, "warning"),
        (PRINTER_ADAPTER_WORD, "moonraker"),
        (PRINTER_PROFILE_ONLY, "printing"),
    ] {
        sql(
            &farm,
            "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
               catalog_variant, catalog_model_id, catalog_printer_variant, notes,
               overrides_json, created_at, updated_at)
             VALUES (?1, 1, ?2, '', '', '', '', '', '', '{}', ?3, ?3)",
            &[&id, &name, &now],
        );
    }
    sql(
        &farm,
        "UPDATE printers SET connection_json = ?1, archived_at = ?2 WHERE id = ?3",
        &[
            &json!({
                "kind": "moonraker", "host": "192.0.2.44", "port": 7125, "useTls": false,
            })
            .to_string(),
            &now,
            &PRINTER_ADAPTER_WORD,
        ],
    );
    farm
}

// --- the command rig -----------------------------------------------------------------

fn no_connection(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

/// A save dialog that returns `destination` (or cancels when `None`) and
/// counts its calls.
struct SaveTo {
    destination: Mutex<Option<PathBuf>>,
    calls: AtomicUsize,
    suggested: Mutex<Vec<String>>,
}

impl SaveTo {
    fn new(destination: Option<PathBuf>) -> Arc<SaveTo> {
        Arc::new(SaveTo {
            destination: Mutex::new(destination),
            calls: AtomicUsize::new(0),
            suggested: Mutex::new(Vec::new()),
        })
    }

    fn set(&self, destination: Option<PathBuf>) {
        *self.destination.lock().unwrap() = destination;
    }
}

impl PortabilityDialogs for SaveTo {
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_backup(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_diagnostics(&self, suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.suggested
            .lock()
            .unwrap()
            .push(suggested_name.to_string());
        Ok(self.destination.lock().unwrap().clone())
    }
}

type EntryHook = farm3d_lib::diagnostics::EntryHook;

struct Rig {
    app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    dialogs: Arc<SaveTo>,
    exports: PathBuf,
}

impl Rig {
    fn new(farm: &Farm) -> Rig {
        Rig::with_hook(farm, None)
    }

    fn with_hook(farm: &Farm, hook: Option<EntryHook>) -> Rig {
        let exports = farm.temp.path().join("exports");
        fs::create_dir_all(&exports).unwrap();
        let dialogs = SaveTo::new(Some(exports.join("bundle.zip")));
        let injected = Arc::clone(&dialogs);
        let (app, webview, _manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::diagnostics::commands::diagnostics_preview,
                farm3d_lib::diagnostics::commands::export_diagnostics,
                farm3d_lib::diagnostics::commands::about_farm3d,
            ],
            Arc::clone(&farm.storage),
            Arc::new(common::a_catalog()),
            farm.credentials_dir.clone(),
            no_connection,
            move |services| {
                services.backup = Arc::new(services.backup.with_dialogs(injected));
                let mut diagnostics =
                    DiagnosticsServices::default().with_home_dir(PathBuf::from(secrets::HOME_PATH));
                if let Some(hook) = hook {
                    diagnostics = diagnostics.with_entry_hook(hook);
                }
                services.diagnostics = Arc::new(diagnostics);
                // No real engine discovery: only the configured (missing)
                // corpus paths.
                services
                    .slicing
                    .set_discovery_env(farm3d_lib::slicing::runtime::DiscoveryEnv {
                        home: None,
                        path_var: None,
                        probe_timeout: std::time::Duration::from_millis(200),
                    });
            },
        );
        let rig = Rig {
            app,
            webview,
            services,
            dialogs,
            exports,
        };
        rig.seed_statuses();
        rig
    }

    /// Printer A errored (with the corpus in its message), Printer B online.
    fn seed_statuses(&self) {
        let mut online = PrinterStatus::new(ConnectionState::Online);
        online.last_observed_at = Some(ids::NOW.to_string());
        self.services.manager.seed(ids::PRINTER_B, online);
        self.services.manager.apply_connection_error(
            ids::PRINTER_A,
            format!("could not reach {}", secrets::USERINFO_URL),
            PrinterSetupFacts::complete(),
        );
    }

    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|envelope| envelope["data"].clone())
    }

    fn export(&self, operation_id: &str, sections: &[&str]) -> Result<Value, Value> {
        self.call(
            "export_diagnostics",
            json!({ "operationId": operation_id, "sections": sections }),
        )
    }

    /// Exports every section to a fresh file and returns the zip's bytes.
    fn export_all(&self, name: &str) -> Vec<u8> {
        let destination = self.exports.join(name);
        self.dialogs.set(Some(destination.clone()));
        let outcome = self
            .export(&format!("op-{name}"), ALL_SECTIONS)
            .unwrap_or_else(|error| panic!("export_diagnostics: {error}"));
        assert_eq!(outcome["status"], "exported", "{outcome}");
        fs::read(destination).unwrap()
    }
}

const ALL_SECTIONS: &[&str] = &[
    "about",
    "health",
    "storage",
    "configuration",
    "logs",
    "recentProblems",
];

// --- the log -------------------------------------------------------------------------

/// Starts the process log in the Farm's `log_root` and drives error paths
/// whose errors carry the corpus; also writes a rotated file with an id
/// that no longer exists and an unparseable line. Returns after a flush.
fn seed_log(farm: &Farm) {
    let log_root = farm.paths().log_root().to_path_buf();
    farm3d_lib::diagnostics::log::init(&log_root);
    farm3d_lib::f3d_log!(
        warn,
        "connections.cacheSaveFailed",
        printer_id = LogId::printer(ids::PRINTER_A),
        error = StorageError::DuplicateHost(secrets::HOST.to_string()),
    );
    farm3d_lib::f3d_log!(
        warn,
        "library.linkCheckFailed",
        model_id = LogId::model(ids::MODEL_3MF),
        error = RepositoryError::Storage(StorageError::CorruptData {
            source_name: "printers.json",
            source_sha256: Some(secrets::CREDENTIAL_VALUE.to_string()),
        }),
    );
    farm3d_lib::f3d_log!(
        info,
        "cameras.captureFailed",
        printer_id = LogId::printer(ids::PRINTER_B),
        job_id = LogId::job(ids::JOB),
        attempt = 2u32,
    );
    farm3d_lib::diagnostics::log::flush();
    farm3d_lib::diagnostics::log::shutdown();
    // A rotated file: an id that no longer exists, a state-word field, and
    // a line that isn't JSON (it carries the corpus and must be dropped).
    fs::write(
        log_root.join("farm3d.1.log"),
        format!(
            "{}\n{}\nnot json {} {}\n",
            json!({
                "ts": "2026-01-01T00:00:00.000Z", "level": "warn", "code": "jobs.dispatchFailed",
                "ids": { "printerId": "prn-gone", "jobId": ids::JOB },
                "fields": { "errorKind": "printing" },
            }),
            json!({
                "ts": "2026-01-01T00:00:01.000Z", "level": "info", "code": "cameras.captured",
                "ids": { "printerId": ids::PRINTER_A }, "fields": {},
            }),
            secrets::USERINFO_URL,
            secrets::PRINTER_NAME,
        ),
    )
    .unwrap();
}

// --- the zip -------------------------------------------------------------------------

fn entries(bytes: &[u8]) -> BTreeMap<String, Vec<u8>> {
    let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    let mut out = BTreeMap::new();
    for index in 0..archive.len() {
        let mut entry = archive.by_index(index).unwrap();
        let mut content = Vec::new();
        entry.read_to_end(&mut content).unwrap();
        out.insert(entry.name().to_string(), content);
    }
    out
}

fn entry_names(bytes: &[u8]) -> Vec<String> {
    let archive = zip::ZipArchive::new(std::io::Cursor::new(bytes)).unwrap();
    archive.file_names().map(str::to_string).collect()
}

fn json_entry(bytes: &[u8], name: &str) -> Value {
    serde_json::from_slice(&entries(bytes)[name]).unwrap()
}

fn log_lines(bytes: &[u8], name: &str) -> Vec<Value> {
    String::from_utf8(entries(bytes)[name].clone())
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect()
}

/// The Printers' pseudonyms as health reports them, in its order, by the
/// fact that tells them apart (A errored, B online).
fn health_printer<'a>(health: &'a Value, predicate: impl Fn(&Value) -> bool) -> &'a Value {
    let matches: Vec<&Value> = health["printers"]
        .as_array()
        .unwrap()
        .iter()
        .filter(|printer| predicate(printer))
        .collect();
    assert_eq!(matches.len(), 1, "{health}");
    matches[0]
}

fn printer_a(health: &Value) -> &Value {
    health_printer(health, |printer| printer["connectionState"] == "error")
}

fn printer_b(health: &Value) -> &Value {
    health_printer(health, |printer| printer["connectionState"] == "online")
}

fn is_pseudonym(value: &Value, kind: &str) -> bool {
    value.as_str().is_some_and(|text| {
        text.strip_prefix(kind)
            .and_then(|rest| rest.strip_prefix('-'))
            .is_some_and(|number| !number.is_empty() && number.chars().all(|c| c.is_ascii_digit()))
    })
}

// --- sections ------------------------------------------------------------------------

#[test]
fn the_bundle_holds_bundle_json_then_each_selected_section_and_no_corpus_item() {
    let _serial = serial();
    let farm = diagnostics_farm();
    seed_log(&farm);
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("all.zip");

    secrets::assert_none_of(&every_needle(), &bytes, "bundle");
    assert_eq!(
        entry_names(&bytes),
        [
            "bundle.json",
            "about.json",
            "health.json",
            "storage.json",
            "configuration.json",
            "logs/farm3d.log",
            "logs/farm3d.1.log",
            "recent-problems.json",
        ]
    );
    let bundle = json_entry(&bytes, "bundle.json");
    assert_eq!(bundle["format"], "farm3d-diagnostics");
    assert_eq!(bundle["formatVersion"], 1);
    assert_eq!(
        bundle["appVersion"],
        rig.app.package_info().version.to_string()
    );
    assert!(bundle["createdAt"]
        .as_str()
        .unwrap()
        .parse::<chrono::DateTime<chrono::Utc>>()
        .is_ok());
    assert_eq!(bundle["sections"], json!(ALL_SECTIONS));

    // Only the selected sections, in canonical order.
    let destination = rig.exports.join("two.zip");
    rig.dialogs.set(Some(destination.clone()));
    let outcome = rig.export("op-two", &["recentProblems", "about"]).unwrap();
    assert_eq!(outcome["sections"], json!(["about", "recentProblems"]));
    let two = fs::read(destination).unwrap();
    assert_eq!(
        entry_names(&two),
        ["bundle.json", "about.json", "recent-problems.json"]
    );
    secrets::assert_none_of(&every_needle(), &two, "two sections");
}

#[test]
fn about_holds_the_version_schema_catalog_slicer_version_and_credential_tier_never_a_path() {
    let _serial = serial();
    let farm = diagnostics_farm();
    let rig = Rig::new(&farm);
    let about = rig.call("about_farm3d", json!({})).unwrap();
    assert_eq!(
        about["appVersion"],
        rig.app.package_info().version.to_string()
    );
    assert_eq!(
        about["schemaVersion"],
        farm3d_lib::persistence::CURRENT_SCHEMA_VERSION
    );
    assert_eq!(about["backupFormatVersion"], 1);
    assert_eq!(about["platform"]["os"], std::env::consts::OS);
    assert_eq!(about["platform"]["arch"], std::env::consts::ARCH);
    assert_eq!(about["catalog"]["sourceTag"], "v-test");
    assert_eq!(about["catalog"]["generatedAt"], "2026-09-22T00:00:00Z");
    // The configured engine is missing, so no version, and never a path.
    assert_eq!(
        about["slicer"],
        json!({ "configured": false, "version": null })
    );
    assert_eq!(
        about["credentialStore"],
        json!({ "kind": "fallbackFile", "available": true })
    );
    let text = about.to_string();
    secrets::assert_none_of(&every_needle(), text.as_bytes(), "about_farm3d");

    let bytes = rig.export_all("about.zip");
    assert_eq!(json_entry(&bytes, "about.json"), about);
}

#[test]
fn health_reports_each_printer_by_pseudonym_with_typed_facts_only() {
    let _serial = serial();
    let farm = diagnostics_farm();
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("health.zip");
    let health = json_entry(&bytes, "health.json");
    let printers = health["printers"].as_array().unwrap();
    // A, B, the Farm's archived one, the two state-word Printers, and the
    // Profile-only one.
    assert_eq!(printers.len(), 6, "{health}");
    let pseudonyms: BTreeSet<&str> = printers
        .iter()
        .map(|printer| printer["printer"].as_str().unwrap())
        .collect();
    assert_eq!(pseudonyms.len(), 6);
    assert!(printers
        .iter()
        .all(|printer| is_pseudonym(&printer["printer"], "printer")));

    let a = printer_a(&health);
    assert_eq!(a["adapterKind"], "moonraker");
    assert_eq!(a["archived"], false);
    assert_eq!(a["errorCode"], "PROTOCOL_ERROR");
    assert_eq!(a["hostSoftwareVersion"], Value::Null);
    let capabilities = a["capabilities"].as_array().unwrap();
    assert_eq!(capabilities.len(), 8);
    for capability in capabilities {
        let keys: BTreeSet<&str> = capability
            .as_object()
            .unwrap()
            .keys()
            .map(String::as_str)
            .collect();
        assert_eq!(keys, BTreeSet::from(["capability", "state", "reason"]));
        assert!(["supported", "unsupported"].contains(&capability["state"].as_str().unwrap()));
    }

    let b = printer_b(&health);
    assert_eq!(b["errorCode"], Value::Null);
    assert!(b["secondsSinceLastObservation"].as_u64().unwrap() > 0);

    let archived: Vec<&Value> = printers
        .iter()
        .filter(|printer| printer["archived"] == true)
        .collect();
    assert_eq!(archived.len(), 2);
    let profile_only: Vec<&Value> = printers
        .iter()
        .filter(|printer| printer["adapterKind"].is_null())
        .collect();
    assert_eq!(profile_only.len(), 3, "{health}");
    assert!(profile_only
        .iter()
        .all(|printer| printer["setupIncomplete"] == true));
    // Every capability of a Profile-only Printer is unsupported by adapter.
    assert!(profile_only.iter().all(|printer| {
        printer["capabilities"]
            .as_array()
            .unwrap()
            .iter()
            .all(|capability| capability["reason"] == "adapter")
    }));
    secrets::assert_none_of(&every_needle(), &bytes, "health");
}

#[test]
fn storage_holds_integrity_findings_warning_codes_and_cleanup_counts_only() {
    let _serial = serial();
    let farm = diagnostics_farm();
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("storage.zip");
    let storage = json_entry(&bytes, "storage.json");

    let report = farm3d_lib::persistence::integrity::check(
        &rusqlite::Connection::open(farm.paths().database()).unwrap(),
        Some(&farm3d_lib::persistence::integrity::IntegrityRoots::from_paths(farm.paths())),
    )
    .unwrap();
    let expected: Vec<Value> = report
        .findings
        .iter()
        .map(|finding| {
            json!({
                "rule": finding.rule,
                "outcome": finding.outcome,
                "count": finding.count,
            })
        })
        .collect();
    assert_eq!(storage["integrity"], json!(expected));
    assert_eq!(
        storage["migrationWarnings"],
        json!([{ "code": "LEGACY_SETTINGS_INVALID", "count": 1 }])
    );
    assert_eq!(
        storage["pendingCredentialCleanup"],
        json!([
            { "reason": "cleared", "count": 1 },
            { "reason": "printer_deleted", "count": 1 },
        ])
    );
    assert_eq!(storage["pendingBlobCleanup"], 1);
    // D14's usage: every class in order, numbers and fixed names only.
    let classes: Vec<&str> = storage["usage"]["classes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|row| row["class"].as_str().unwrap())
        .collect();
    assert_eq!(classes.len(), 13);
    assert_eq!(classes[0], "database");
    assert_eq!(classes[12], "slicerProfileCache");
    assert_eq!(
        storage["usage"]["totalBytes"].as_u64().unwrap(),
        storage["usage"]["classes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|row| row["bytes"].as_u64().unwrap())
            .sum::<u64>()
    );
    assert!(storage["usage"]["measuredAt"].as_str().unwrap().ends_with('Z'));
    secrets::assert_none_of(&every_needle(), &bytes, "storage");
}

#[test]
fn configuration_holds_settings_counts_and_whether_a_slicer_runtime_is_configured() {
    let _serial = serial();
    let farm = diagnostics_farm();
    sql(
        &farm,
        "UPDATE settings SET theme_mode = ?1",
        &[&secrets::PRINTER_NAME],
    );
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("configuration.zip");
    let configuration = json_entry(&bytes, "configuration.json");

    let settings = &configuration["settings"];
    assert_eq!(settings["themeMode"], "custom");
    let record =
        farm3d_lib::settings::repository::SettingsRepository::new(Arc::clone(&farm.storage))
            .load()
            .unwrap();
    assert_eq!(settings["revision"], record.revision);
    assert_eq!(settings["monitorSection"], json!(record.monitor_section));
    assert_eq!(settings["monitorDensity"], json!(record.monitor_density));
    assert_eq!(settings["notifications"], json!(record.notifications));
    assert_eq!(
        settings["snapshotRetention"],
        json!(record.snapshot_retention)
    );
    assert_eq!(settings["notifications"]["fatal"], true);
    assert_eq!(settings["snapshotRetention"]["retentionDays"], 30);

    let counts = farm.counts();
    let table_counts = configuration["tableCounts"].as_object().unwrap();
    assert_eq!(table_counts.len(), counts.len());
    for (table, count) in counts {
        assert_eq!(table_counts[&table], count, "{table}");
    }
    assert_eq!(
        configuration["cameraSources"],
        json!({ "hostWebcam": 2, "snapshotUrl": 1 })
    );
    assert_eq!(
        configuration["printers"],
        json!({
            "total": 6,
            "byAdapterKind": { "moonraker": 3 },
            "archived": 2,
            "profileOnly": 3,
        })
    );
    assert_eq!(configuration["slicerRuntimeConfigured"], true);
    secrets::assert_none_of(&every_needle(), &bytes, "configuration");

    // A built-in theme is named.
    sql(&farm, "UPDATE settings SET theme_mode = 'farm3d-dark'", &[]);
    let bytes = rig.export_all("configuration-dark.zip");
    assert_eq!(
        json_entry(&bytes, "configuration.json")["settings"]["themeMode"],
        "farm3d-dark"
    );
}

#[test]
fn logs_are_bundled_with_every_id_rewritten_to_this_bundles_pseudonyms() {
    let _serial = serial();
    let farm = diagnostics_farm();
    seed_log(&farm);
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("logs.zip");
    secrets::assert_none_of(&every_needle(), &bytes, "logs");

    let health = json_entry(&bytes, "health.json");
    let a = printer_a(&health)["printer"].clone();
    let b = printer_b(&health)["printer"].clone();

    let active = log_lines(&bytes, "logs/farm3d.log");
    let by_code = |lines: &[Value], code: &str| -> Value {
        lines
            .iter()
            .find(|line| line["code"] == code)
            .unwrap_or_else(|| panic!("{code} in {lines:?}"))
            .clone()
    };
    let cache = by_code(&active, "connections.cacheSaveFailed");
    assert_eq!(cache["ids"]["printerId"], a);
    assert_eq!(cache["fields"]["error"], "DuplicateHost");
    let link = by_code(&active, "library.linkCheckFailed");
    assert!(is_pseudonym(&link["ids"]["modelId"], "model"), "{link}");
    assert_eq!(link["fields"]["error"], "Storage");
    let capture = by_code(&active, "cameras.captureFailed");
    assert_eq!(capture["ids"]["printerId"], b);
    assert!(is_pseudonym(&capture["ids"]["jobId"], "job"));
    assert_eq!(capture["fields"]["attempt"], 2);

    let rotated = log_lines(&bytes, "logs/farm3d.1.log");
    // The unparseable line is dropped.
    assert_eq!(rotated.len(), 2, "{rotated:?}");
    let gone = by_code(&rotated, "jobs.dispatchFailed");
    assert!(is_pseudonym(&gone["ids"]["printerId"], "printer"));
    assert_ne!(gone["ids"]["printerId"], a);
    assert_ne!(gone["ids"]["printerId"], b);
    // The same Job in two files and two lines: one pseudonym.
    assert_eq!(gone["ids"]["jobId"], capture["ids"]["jobId"]);
    assert_eq!(gone["fields"]["errorKind"], "printing");
    assert_eq!(by_code(&rotated, "cameras.captured")["ids"]["printerId"], a);

    // No raw id anywhere in the logs.
    for name in ["logs/farm3d.log", "logs/farm3d.1.log"] {
        let text = String::from_utf8(entries(&bytes)[name].clone()).unwrap();
        for raw in [
            ids::PRINTER_A,
            ids::PRINTER_B,
            ids::JOB,
            ids::MODEL_3MF,
            "prn-gone",
        ] {
            assert!(!text.contains(&format!("\"{raw}\"")), "{raw} in {name}");
        }
    }
}

#[test]
fn recent_problems_are_the_newest_attention_events_by_pseudonymous_source() {
    let _serial = serial();
    let farm = diagnostics_farm();
    // 60 more resolved offline events for Printer B, one per minute.
    for minute in 0..60 {
        sql(
            &farm,
            "INSERT INTO attention_events(
                 id, dedup_key, condition, severity, requires_action, resolution_mode,
                 notification_class, source_kind, source_id, printer_id, subject_snapshot_json,
                 detail_json, summary, origin, first_observed_at, last_observed_at,
                 read_at, resolved_at, resolution)
             VALUES (?1, ?2, 'printer.offline', 'warning', 1, 'auto', 'connectivity',
                     'printer', ?3, ?3, '{}', '{}', ?4, 'live', ?5, ?5, ?5, ?5, 'conditionCleared')",
            &[
                &format!("att-b{minute:02}"),
                &format!("printer.offline:printer:{}", ids::PRINTER_B),
                &ids::PRINTER_B,
                &secrets::PRINTER_NAME,
                &format!("2026-02-01T00:{minute:02}:00.000Z"),
            ],
        );
    }
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("recent.zip");
    secrets::assert_none_of(&every_needle(), &bytes, "recent problems");
    let health = json_entry(&bytes, "health.json");
    let b = printer_b(&health)["printer"].clone();

    let recent = json_entry(&bytes, "recent-problems.json");
    let events = recent["events"].as_array().unwrap();
    assert_eq!(events.len(), 50);
    assert_eq!(
        events[0],
        json!({
            "condition": "printer.offline",
            "severity": "warning",
            "sourceKind": "printer",
            "source": b,
            "firstObservedAt": "2026-02-01T00:59:00.000Z",
            "resolvedAt": "2026-02-01T00:59:00.000Z",
            "resolution": "conditionCleared",
        })
    );
    let times: Vec<&str> = events
        .iter()
        .map(|event| event["firstObservedAt"].as_str().unwrap())
        .collect();
    let mut sorted = times.clone();
    sorted.sort_by(|left, right| right.cmp(left));
    assert_eq!(times, sorted);
    assert_eq!(times[49], "2026-02-01T00:10:00.000Z");
}

// --- pseudonyms ----------------------------------------------------------------------

#[test]
fn pseudonyms_are_stable_within_a_bundle_and_differ_between_bundles() {
    let _serial = serial();
    let farm = diagnostics_farm();
    seed_log(&farm);
    let rig = Rig::new(&farm);

    // Health, logs, and recent problems name Printer A the same way.
    let orders: Vec<Vec<String>> = (0..8)
        .map(|index| {
            let bytes = rig.export_all(&format!("pseudonyms-{index}.zip"));
            let health = json_entry(&bytes, "health.json");
            let a = printer_a(&health)["printer"].clone();
            let logs = log_lines(&bytes, "logs/farm3d.log");
            let cache = logs
                .iter()
                .find(|line| line["code"] == "connections.cacheSaveFailed")
                .unwrap();
            assert_eq!(cache["ids"]["printerId"], a);
            let recent = json_entry(&bytes, "recent-problems.json");
            let offline = recent["events"]
                .as_array()
                .unwrap()
                .iter()
                .find(|event| event["condition"] == "printer.offline")
                .unwrap();
            assert_eq!(offline["source"], a);
            // The Printers in a fixed order (by their distinguishing facts):
            // their pseudonyms are this bundle's permutation.
            let mut rows: Vec<(String, String)> = health["printers"]
                .as_array()
                .unwrap()
                .iter()
                .map(|printer| {
                    (
                        format!(
                            "{}/{}/{}",
                            printer["connectionState"], printer["archived"], printer["adapterKind"]
                        ),
                        printer["printer"].as_str().unwrap().to_string(),
                    )
                })
                .collect();
            rows.sort();
            rows.into_iter().map(|(_, pseudonym)| pseudonym).collect()
        })
        .collect();
    let distinct: BTreeSet<&Vec<String>> = orders.iter().collect();
    assert!(
        distinct.len() > 1,
        "eight bundles numbered the Printers identically: {orders:?}"
    );
}

// --- egress --------------------------------------------------------------------------

/// A hook that leaks `term` into `section` as a new string value (JSON
/// entries) or a new line (logs), after serialization and before the scan.
fn leak(target: DiagnosticsSection, term: String) -> EntryHook {
    Arc::new(
        move |section: DiagnosticsSection, _name: &str, bytes: &mut Vec<u8>| {
            if section != target {
                return;
            }
            if section == DiagnosticsSection::Logs {
                bytes.extend_from_slice(
                    json!({
                        "ts": "2026-01-01T00:00:00.000Z", "level": "info", "code": "x.leak",
                        "ids": {}, "fields": {}, "leak": term,
                    })
                    .to_string()
                    .as_bytes(),
                );
                bytes.push(b'\n');
                return;
            }
            let mut value: Value = serde_json::from_slice(bytes).unwrap();
            value
                .as_object_mut()
                .unwrap()
                .insert("leak".to_string(), Value::String(term.clone()));
            *bytes = serde_json::to_vec_pretty(&value).unwrap();
        },
    )
}

fn percent_encoded(text: &str) -> String {
    text.bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
                (byte as char).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

/// A hook that leaks `term` into a log line's `fields`, where only the
/// shapes `LogSafe` produces are closed.
fn leak_log_field(term: String) -> EntryHook {
    Arc::new(
        move |section: DiagnosticsSection, _name: &str, bytes: &mut Vec<u8>| {
            if section != DiagnosticsSection::Logs {
                return;
            }
            bytes.extend_from_slice(
                json!({
                    "ts": "2026-01-01T00:00:00.000Z", "level": "info", "code": "x.leak",
                    "ids": {}, "fields": { "file": term },
                })
                .to_string()
                .as_bytes(),
            );
            bytes.push(b'\n');
        },
    )
}

fn assert_redaction_failed(farm: &Farm, section: DiagnosticsSection, term: String) {
    assert_redaction_failed_with(farm, section, term.clone(), leak(section, term));
}

fn assert_redaction_failed_with(
    farm: &Farm,
    section: DiagnosticsSection,
    term: String,
    hook: EntryHook,
) {
    let rig = Rig::with_hook(farm, Some(hook));
    let destination = rig.exports.join("leak.zip");
    rig.dialogs.set(Some(destination.clone()));
    let before: Vec<_> = fs::read_dir(&rig.exports).unwrap().collect();
    let error = rig
        .export("op-leak", ALL_SECTIONS)
        .expect_err("a leak must refuse the export");
    assert_eq!(error["code"], "DIAGNOSTICS_REDACTION_FAILED", "{error}");
    assert_eq!(
        error["details"],
        json!({ "section": serde_json::to_value(section).unwrap() })
    );
    assert!(error["retryable"] == false);
    // The matched term is never reported.
    assert!(!error.to_string().contains(&term), "{error}");
    // Nothing was written: no file, no temporary file.
    assert!(!destination.exists());
    assert_eq!(fs::read_dir(&rig.exports).unwrap().count(), before.len());
}

#[test]
fn a_leak_of_any_corpus_class_is_caught_before_the_file_is_written() {
    let _serial = serial();
    let farm = diagnostics_farm();
    seed_log(&farm);
    use DiagnosticsSection as S;
    let cases: Vec<(S, String)> = vec![
        // Credential values (from the store) and refs, in both forms.
        (S::Health, secrets::CREDENTIAL_VALUE.to_string()),
        (S::Storage, secrets::CREDENTIAL_VALUE_2.to_uppercase()),
        (S::About, ids::CREDENTIAL_REF_A.to_string()),
        (S::Configuration, percent_encoded(ORPHAN_REF)),
        (S::Logs, "farm3d/printer/prn-c/apikey".to_string()),
        // Hosts: the Connection host, a Host Operation endpoint host.
        (S::Health, format!("http://{}:7125", secrets::HOST)),
        (S::RecentProblems, secrets::HOST_NAME.to_uppercase()),
        // The stored camera URL, encoded, and its query value.
        (S::Storage, percent_encoded(secrets::STORED_CAMERA_URL)),
        (S::Logs, secrets::QUERY_TOKEN.to_string()),
        // Paths: linked, source, the Slicer runtime's, storage roots, home.
        (S::Configuration, LINKED_PATH.to_lowercase()),
        (
            S::About,
            format!("{}/models/bracket.3mf", secrets::HOME_PATH),
        ),
        (S::Health, format!("{}/orca/presets", secrets::HOME_PATH)),
        (
            S::Storage,
            farm.paths().log_root().to_string_lossy().into_owned(),
        ),
        (
            S::RecentProblems,
            percent_encoded(&farm.paths().metadata_root().to_string_lossy()),
        ),
        (S::Configuration, secrets::HOME_PATH.to_string()),
        // Names and notes, exact and percent-encoded.
        (S::Health, secrets::PRINTER_NAME.to_string()),
        (S::RecentProblems, percent_encoded(MODEL_NAME)),
        (S::Storage, format!("shelf: {STORAGE_LABEL}")),
        (S::About, TARE_NAME.to_string()),
        (S::Configuration, PROJECT_NAME.to_string()),
        (S::Logs, LOCATION.to_string()),
        (S::Health, PRINTER_NOTES.to_string()),
        (S::Storage, INCIDENT_NOTE.to_string()),
        (S::RecentProblems, SPOOL_COLOR.to_string()),
    ];
    for (section, term) in cases {
        assert_redaction_failed(&farm, section, term);
    }
    // A host job file name in a log line's `fields` isn't a `LogSafe`
    // shape, so the name scan sees it.
    assert_redaction_failed_with(
        &farm,
        DiagnosticsSection::Logs,
        JOB_NAME.to_string(),
        leak_log_field(JOB_NAME.to_string()),
    );
}

#[test]
fn a_log_field_that_isnt_a_log_safe_shape_is_redacted_in_the_bundle() {
    let _serial = serial();
    let farm = diagnostics_farm();
    fs::write(
        farm.paths().log_root().join("farm3d.log"),
        format!(
            "{}\n",
            json!({
                "ts": "2026-01-01T00:00:00.000Z", "level": "warn", "code": "jobs.dispatchFailed",
                "ids": {}, "fields": { "file": JOB_NAME, "vendor": "Polymaker Blue",
                                       "kind": "printing", "error": "PRINTER_UNREACHABLE" },
            })
        ),
    )
    .unwrap();
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("fields.zip");
    secrets::assert_none_of(&every_needle(), &bytes, "fields");
    let line = &log_lines(&bytes, "logs/farm3d.log")[0];
    assert_eq!(
        line["fields"],
        json!({
            "file": "redacted", "vendor": "redacted",
            "kind": "printing", "error": "PRINTER_UNREACHABLE",
        })
    );
}

#[test]
fn an_unreadable_credential_store_is_logged_and_the_export_goes_on() {
    let _serial = serial();
    let farm = diagnostics_farm();
    // The fallback file no longer parses: every value read fails.
    fs::write(
        farm3d_lib::connections::credentials::credentials_file_path(&farm.credentials_dir),
        b"not json",
    )
    .unwrap();
    let log_dir = farm.temp.path().join("process-log");
    farm3d_lib::diagnostics::log::init(&log_dir);
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("no-credentials.zip");
    farm3d_lib::diagnostics::log::flush();
    farm3d_lib::diagnostics::log::shutdown();
    secrets::assert_none_of(&every_needle(), &bytes, "no credentials");
    let text = fs::read_to_string(log_dir.join("farm3d.log")).unwrap();
    let line: Value = text
        .lines()
        .map(|line| serde_json::from_str::<Value>(line).unwrap())
        .find(|line| line["code"] == "diagnostics.credentialCorpusUnavailable")
        .unwrap_or_else(|| panic!("no credentialCorpusUnavailable line in {text}"));
    assert_eq!(line["level"], "warn");
    // Every ref: the two stored, the orphan, `cred-a`, and the derived
    // ref of each of the four Printers without a stored one.
    assert_eq!(line["fields"]["unreadableRefs"], 8, "{line}");
    assert_eq!(line["ids"], json!({}));
}

#[test]
fn state_word_names_and_short_names_do_not_block_the_export() {
    let _serial = serial();
    let farm = diagnostics_farm();
    seed_log(&farm);
    // A name of three characters is never a term.
    sql(
        &farm,
        "UPDATE printers SET name = 'job' WHERE id = ?1",
        &[&ids::PRINTER_B],
    );
    let rig = Rig::new(&farm);
    let bytes = rig.export_all("state-words.zip");
    let health = json_entry(&bytes, "health.json");
    assert!(health["printers"]
        .as_array()
        .unwrap()
        .iter()
        .any(|printer| printer["adapterKind"] == "moonraker"));
    let recent = json_entry(&bytes, "recent-problems.json");
    assert!(recent["events"]
        .as_array()
        .unwrap()
        .iter()
        .any(|event| event["severity"] == "warning"));
    let rotated = log_lines(&bytes, "logs/farm3d.1.log");
    assert!(rotated
        .iter()
        .any(|line| line["fields"]["errorKind"] == "printing"));
}

// --- the command contract ------------------------------------------------------------

#[test]
fn a_cancelled_dialog_writes_nothing_and_records_nothing() {
    let _serial = serial();
    let farm = diagnostics_farm();
    let rig = Rig::new(&farm);
    rig.dialogs.set(None);
    let outcome = rig.export("op-cancel", &["about"]).unwrap();
    assert_eq!(outcome, json!({ "status": "cancelled" }));
    assert_eq!(fs::read_dir(&rig.exports).unwrap().count(), 0);
    // A retry under the same id asks again.
    let destination = rig.exports.join("after-cancel.zip");
    rig.dialogs.set(Some(destination.clone()));
    let outcome = rig.export("op-cancel", &["about"]).unwrap();
    assert_eq!(outcome["status"], "exported");
    assert_eq!(rig.dialogs.calls.load(Ordering::SeqCst), 2);
    assert!(destination.exists());
    let suggested = rig.dialogs.suggested.lock().unwrap().clone();
    let name = &suggested[0];
    assert!(
        name.starts_with("farm3d-diagnostics-") && name.ends_with("Z.zip") && name.len() == 39,
        "{name}"
    );
}

#[test]
fn the_outcome_names_the_basename_and_a_replay_returns_it_without_a_second_file() {
    let _serial = serial();
    let farm = diagnostics_farm();
    let rig = Rig::new(&farm);
    let destination = rig.exports.join("support.zip");
    rig.dialogs.set(Some(destination.clone()));
    let outcome = rig.export("op-replay", &["health", "about"]).unwrap();
    assert_eq!(outcome["status"], "exported");
    assert_eq!(outcome["fileName"], "support.zip");
    assert_eq!(outcome["sections"], json!(["about", "health"]));
    assert_eq!(
        outcome["bytes"].as_u64().unwrap(),
        fs::metadata(&destination).unwrap().len()
    );
    assert!(outcome["exportedAt"]
        .as_str()
        .unwrap()
        .parse::<chrono::DateTime<chrono::Utc>>()
        .is_ok());
    assert!(!outcome
        .to_string()
        .contains(&*rig.exports.to_string_lossy()));

    // Same id, same sections (in any order): the cached outcome.
    fs::remove_file(&destination).unwrap();
    let replay = rig.export("op-replay", &["about", "health"]).unwrap();
    assert_eq!(replay, outcome);
    assert_eq!(rig.dialogs.calls.load(Ordering::SeqCst), 1);
    assert!(!destination.exists());

    // Same id, other sections: VALIDATION on operationId.
    let error = rig.export("op-replay", &["about"]).unwrap_err();
    assert_eq!(error["code"], "VALIDATION");
    assert_eq!(error["details"]["fieldPath"], "operationId");
}

#[test]
fn sections_must_be_one_to_six_distinct_values() {
    let _serial = serial();
    let farm = diagnostics_farm();
    let rig = Rig::new(&farm);
    for sections in [
        json!([]),
        json!(["about", "about"]),
        json!([
            "about",
            "health",
            "storage",
            "configuration",
            "logs",
            "recentProblems",
            "about"
        ]),
    ] {
        let error = rig
            .call(
                "export_diagnostics",
                json!({ "operationId": "op-bad", "sections": sections }),
            )
            .unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{sections}");
        assert_eq!(error["details"]["fieldPath"], "sections", "{sections}");
    }
    assert_eq!(rig.dialogs.calls.load(Ordering::SeqCst), 0);
}

#[test]
fn the_preview_estimates_every_section() {
    let _serial = serial();
    let farm = diagnostics_farm();
    seed_log(&farm);
    let rig = Rig::new(&farm);
    let preview = rig.call("diagnostics_preview", json!({})).unwrap();
    let sections = preview["sections"].as_array().unwrap();
    let names: Vec<&str> = sections
        .iter()
        .map(|row| row["section"].as_str().unwrap())
        .collect();
    assert_eq!(names, ALL_SECTIONS);
    assert!(sections
        .iter()
        .all(|row| row["estimatedBytes"].as_u64().unwrap() > 0));
    let log_bytes: u64 = ["farm3d.log", "farm3d.1.log"]
        .iter()
        .map(|name| {
            fs::metadata(farm.paths().log_root().join(name))
                .unwrap()
                .len()
        })
        .sum();
    let logs = sections
        .iter()
        .find(|row| row["section"] == "logs")
        .unwrap();
    assert_eq!(logs["estimatedBytes"].as_u64().unwrap(), log_bytes);
    assert_eq!(rig.dialogs.calls.load(Ordering::SeqCst), 0);
    // No file anywhere.
    assert_eq!(fs::read_dir(&rig.exports).unwrap().count(), 0);
}
