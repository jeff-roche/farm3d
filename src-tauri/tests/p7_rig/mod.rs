//! The P7 Queue/Job rig shared by `p7_queue.rs` and `p7_jobs.rs`: a leased
//! temp `Storage`, two Moonraker Printers (RFC 5737 hosts, a seeded
//! credential) seeded Online/Ready/fresh in the `ConnectionManager`, a
//! farm3d Slice Revision whose facts match their profile, an external one
//! with a file-claimed estimate, Spools in storage, and a mock app with
//! the Queue and Job commands registered and every emitted event captured.
//!
//! Each test crate uses a different subset, hence the `dead_code` allow.
#![allow(dead_code)]

use std::sync::{Arc, Mutex};

use farm3d_lib::catalog::{Catalog, PrinterProfile};
use farm3d_lib::connections::capabilities::{capabilities_for, PrinterCapabilities};
use farm3d_lib::connections::credentials::CredentialStore;
use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::connections::{
    ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND,
};
use farm3d_lib::persistence::{MetadataRootLease, RepositoryError, Storage};
use farm3d_lib::printers::operational::{OperationalState, TelemetryFreshness};
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::StoredPrinter;
use farm3d_lib::queue::world::WorldReader;
use farm3d_lib::slicing::facts::{
    ConfirmedFact, ConfirmedFacts, ExternalFacts, Farm3dFacts, ProfileSnapshot,
};
use farm3d_lib::slicing::repository::{
    insert_external_revision, insert_farm3d_revision, NewExternalRevision, NewFarm3dRevision,
};
use farm3d_lib::slicing::{
    ClaimedEstimateSource, ClaimedEstimates, RuntimeChannel, SliceControls, SliceEstimates,
    SlicePlateRef, SliceRevisionTarget, SliceRuntimeInfo, SliceTarget,
};
use farm3d_lib::spools::ledger::AmountEntry;
use farm3d_lib::spools::{
    repository as spools_repository, AmountConfidence, FilamentDiameter, MaterialFamily,
    SpoolFields,
};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;

use crate::common;

pub const PRINTER_A: &str = "prn-a";
pub const PRINTER_B: &str = "prn-b";
/// A farm3d revision: PLA 1.75, 12.5 g (12 500 mg), matching the
/// Printers' profile.
pub const SLR: &str = "slr-farm3d";
pub const SLR_ESTIMATE_MG: i64 = 12_500;
/// An external revision whose file claims 20.2 g (20 200 mg).
pub const SLR_EXTERNAL: &str = "slr-external";
pub const EXTERNAL_CLAIM_MG: i64 = 20_200;
/// An external revision whose material family nobody confirmed: Manual
/// only, with `acknowledgeManualFacts` (D5).
pub const SLR_UNCONFIRMED: &str = "slr-unconfirmed";
pub const SECRET: &str = "SEEDED-P7-API-KEY-51d0";
const CREDENTIAL_REF: &str = "farm3d/printer/prn-a/apikey";
const NOW: &str = "2026-09-27T00:00:00Z";
const STL_HASH: &str = "1111111111111111111111111111111111111111111111111111111111111111";
const GCODE_HASH: &str = "2222222222222222222222222222222222222222222222222222222222222222";
const OUTPUT_HASH: &str = "3333333333333333333333333333333333333333333333333333333333333333";

pub struct Rig {
    _temp: tempfile::TempDir,
    _lease: MetadataRootLease,
    _credentials: tempfile::TempDir,
    pub app: tauri::App<MockRuntime>,
    pub webview: tauri::WebviewWindow<MockRuntime>,
    pub services: Arc<RuntimeServices<MockRuntime>>,
    pub storage: Arc<Storage>,
    pub events: Arc<Mutex<Vec<String>>>,
}

fn unused_factory(
    _config: &ConnectionConfig,
    _key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    None
}

pub fn a_printer(id: &str, name: &str, host: &str) -> StoredPrinter {
    StoredPrinter {
        name: name.to_string(),
        connection: Some(ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: host.to_string(),
            port: 7125,
            use_tls: false,
            credential_ref: Some(CREDENTIAL_REF.to_string()),
        }),
        ..common::a_stored_printer(id)
    }
}

pub fn ready_status() -> PrinterStatus {
    let mut status = PrinterStatus::new(ConnectionState::Online);
    status.operational_state = OperationalState::Ready;
    status.freshness = TelemetryFreshness::Fresh;
    status
}

pub fn catalog_profile() -> PrinterProfile {
    PrinterProfile::from(&common::a_catalog().models[0].variants[0])
}

fn profile_snapshot() -> ProfileSnapshot {
    ProfileSnapshot::new(common::a_ref(), &catalog_profile())
}

fn seed_revisions(storage: &Storage) {
    storage
        .write_repo(|tx| {
            tx.execute_batch(&format!(
                "INSERT INTO content_blobs(sha256, size_bytes, created_at) VALUES
                   ('{STL_HASH}', 100, '{NOW}'), ('{GCODE_HASH}', 300, '{NOW}'),
                   ('{OUTPUT_HASH}', 400, '{NOW}');
                 INSERT INTO library_models(id, revision, name, format, storage_mode, created_at, updated_at)
                   VALUES ('mdl-stl', 1, 'Bracket', 'stl', 'managed', '{NOW}', '{NOW}'),
                          ('mdl-gcode', 1, 'Hook', 'gcode', 'managed', '{NOW}', '{NOW}');
                 INSERT INTO model_source_revisions(id, model_id, sequence, content_sha256, size_bytes,
                   format, origin, source_file_name, source_path, captured_at, inspector_version,
                   inspection_json)
                   VALUES ('msr-stl-1', 'mdl-stl', 1, '{STL_HASH}', 100, 'stl', 'import', 'part',
                           '/src/part', '{NOW}', 1, '{{}}'),
                          ('msr-gcode-1', 'mdl-gcode', 1, '{GCODE_HASH}', 300, 'gcode', 'import',
                           'part', '/src/part', '{NOW}', 1, '{{}}');"
            ))?;
            insert_farm3d_revision(
                tx,
                &NewFarm3dRevision {
                    id: SLR.to_string(),
                    source_revision_id: "msr-stl-1".to_string(),
                    plate: SlicePlateRef {
                        plate_key: "plate-1".to_string(),
                        plate_index: 1,
                        plate_name: Some("Left".to_string()),
                    },
                    gcode_sha256: OUTPUT_HASH.to_string(),
                    gcode_size: 400,
                    target: SliceRevisionTarget {
                        target: SliceTarget::Profile {
                            catalog_ref: common::a_ref(),
                        },
                        profile: profile_snapshot(),
                        machine_preset: "Test Printer 0.4 nozzle".to_string(),
                        process_preset: "0.20mm Standard".to_string(),
                        filament_preset: "Generic PLA".to_string(),
                        controls: SliceControls::default(),
                    },
                    facts: Farm3dFacts::new(profile_snapshot(), 0.4, MaterialFamily::Pla, None, 1.75),
                    estimates: SliceEstimates {
                        print_seconds: Some(3723),
                        filament_grams: Some(12.5),
                        ..SliceEstimates::none()
                    },
                    runtime: SliceRuntimeInfo {
                        engine_version: "2.4.2".to_string(),
                        engine_channel: RuntimeChannel::Release,
                        preset_source_version: "2.4.2".to_string(),
                        preset_source_channel: RuntimeChannel::Release,
                    },
                    blobs: Vec::new(),
                },
            )?;
            insert_external_revision(
                tx,
                &NewExternalRevision {
                    id: SLR_EXTERNAL.to_string(),
                    source_revision_id: "msr-gcode-1".to_string(),
                    facts: ExternalFacts::new(ConfirmedFacts {
                        printer_profile: ConfirmedFact::Confirmed(profile_snapshot()),
                        nozzle_diameter_mm: ConfirmedFact::Confirmed(0.4),
                        material_family: ConfirmedFact::Confirmed(MaterialFamily::Pla),
                        material_other: None,
                        filament_diameter_mm: ConfirmedFact::Confirmed(1.75),
                    }),
                    claimed_estimates: ClaimedEstimates {
                        print_seconds: Some(60),
                        filament_grams: Some(20.2),
                        filament_mm: None,
                        layer_count: None,
                        max_z_mm: None,
                        source: ClaimedEstimateSource::FileClaim,
                        trusted: false,
                    },
                    producer: None,
                },
            )?;
            insert_external_revision(
                tx,
                &NewExternalRevision {
                    id: SLR_UNCONFIRMED.to_string(),
                    source_revision_id: "msr-gcode-1".to_string(),
                    facts: ExternalFacts::new(ConfirmedFacts {
                        printer_profile: ConfirmedFact::Confirmed(profile_snapshot()),
                        nozzle_diameter_mm: ConfirmedFact::Confirmed(0.4),
                        material_family: ConfirmedFact::Absent,
                        material_other: None,
                        filament_diameter_mm: ConfirmedFact::Confirmed(1.75),
                    }),
                    claimed_estimates: ClaimedEstimates {
                        print_seconds: None,
                        filament_grams: None,
                        filament_mm: None,
                        layer_count: None,
                        max_z_mm: None,
                        source: ClaimedEstimateSource::FileClaim,
                        trusted: false,
                    },
                    producer: None,
                },
            )?;
            Ok(())
        })
        .map_err(|error: RepositoryError| format!("{error:?}"))
        .expect("seed revisions");
}

impl Rig {
    pub fn new() -> Self {
        let (temp, lease, storage, _database) = common::storage();
        let credentials = tempfile::tempdir().unwrap();
        CredentialStore::file_backed(credentials.path().to_path_buf())
            .set(CREDENTIAL_REF, SECRET)
            .unwrap();
        let printers = PrinterRepository::new(Arc::clone(&storage));
        printers
            .create(a_printer(PRINTER_A, "Alpha", "192.0.2.10"))
            .unwrap();
        printers
            .create(a_printer(PRINTER_B, "Bravo", "192.0.2.11"))
            .unwrap();
        seed_revisions(&storage);

        let (app, webview, manager, services) = common::runtime_with(
            tauri::generate_handler![
                farm3d_lib::queue::commands::list_queue,
                farm3d_lib::queue::commands::add_to_queue,
                farm3d_lib::queue::commands::update_queue_entry,
                farm3d_lib::queue::commands::move_queue_entry,
                farm3d_lib::queue::commands::remove_queue_entry,
                farm3d_lib::queue::commands::explain_queue_entry,
                farm3d_lib::jobs::commands::assign_queue_entry,
                farm3d_lib::jobs::commands::release_job,
                farm3d_lib::jobs::commands::retry_job,
                farm3d_lib::jobs::commands::cancel_job,
                farm3d_lib::jobs::commands::get_job_history,
            ],
            Arc::clone(&storage),
            Arc::new(common::a_catalog()),
            credentials.path().to_path_buf(),
            unused_factory,
            |_| {},
        );
        manager.seed(PRINTER_A, ready_status());
        manager.seed(PRINTER_B, ready_status());
        let events = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&events);
        app.listen(STATUS_EVENT, move |event| {
            sink.lock().unwrap().push(event.payload().to_string());
        });
        Self {
            _temp: temp,
            _lease: lease,
            _credentials: credentials,
            app,
            webview,
            services,
            storage,
            events,
        }
    }

    pub fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    /// A Spool in storage, PLA 1.75, with `net_mg` on it.
    pub fn spool(&self, net_mg: i64) -> String {
        let fields = SpoolFields {
            manufacturer: "Polymaker".to_string(),
            product: None,
            material_family: MaterialFamily::Pla,
            material_other: None,
            color_name: "Black".to_string(),
            color_hex: None,
            diameter: FilamentDiameter::D175,
            nominal_mg: 1_000_000,
            low_threshold_mg: 10_000,
            tare_id: None,
            notes: None,
        };
        let entry = AmountEntry::Net {
            net_mg,
            confidence: AmountConfidence::Estimated,
        };
        self.storage
            .write_repo(|tx| spools_repository::insert_spool(tx, &fields, &entry, None))
            .unwrap()
            .id
    }

    /// Adds `quantity` copies of the farm3d revision, Recommended.
    pub fn add(&self, operation_id: &str, quantity: i64) -> Vec<Value> {
        let change = self
            .call(
                "add_to_queue",
                json!({
                    "operationId": operation_id,
                    "sliceRevisionId": SLR,
                    "quantity": quantity,
                    "policy": "recommended",
                    "preference": "loadedFirst",
                }),
            )
            .expect("add_to_queue");
        change["entries"].as_array().unwrap().clone()
    }

    pub fn assign(
        &self,
        operation_id: &str,
        entry_id: &str,
        printer_id: &str,
        spool_id: &str,
    ) -> Result<Value, Value> {
        self.call(
            "assign_queue_entry",
            json!({
                "operationId": operation_id,
                "entryId": entry_id,
                "printerId": printer_id,
                "spoolId": spool_id,
            }),
        )
    }

    pub fn job_command(
        &self,
        command: &str,
        operation_id: &str,
        job_id: &str,
    ) -> Result<Value, Value> {
        self.call(
            command,
            json!({"operationId": operation_id, "jobId": job_id}),
        )
    }

    pub fn list(&self) -> Value {
        self.call("list_queue", json!({})).expect("list_queue")
    }

    pub fn entry(&self, entry_id: &str) -> Value {
        let snapshot = self.list();
        snapshot["entries"]
            .as_array()
            .unwrap()
            .iter()
            .find(|entry| entry["id"] == entry_id)
            .cloned()
            .unwrap_or_else(|| panic!("entry {entry_id} listed"))
    }

    pub fn count(&self, sql: &str) -> i64 {
        self.storage
            .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
            .unwrap()
    }

    pub fn available_mg(&self, spool_id: &str) -> i64 {
        self.storage
            .read(|connection| {
                Ok(spools_repository::load_record(connection, spool_id)
                    .unwrap()
                    .unwrap()
                    .availability
                    .available_mg)
            })
            .unwrap()
    }

    pub fn event_count(&self) -> usize {
        self.events.lock().unwrap().len()
    }

    /// The captured events of the `queue` stream, parsed.
    pub fn queue_events(&self) -> Vec<Value> {
        self.events
            .lock()
            .unwrap()
            .iter()
            .map(|text| serde_json::from_str::<Value>(text).unwrap())
            .filter(|event| {
                event["type"]
                    .as_str()
                    .is_some_and(|kind| kind.starts_with("queue."))
            })
            .collect()
    }
}

/// A fixed [`WorldReader`] for tests that call `jobs::assign` directly
/// from several threads: every Printer Ready and fresh, registry
/// capabilities, the rig's catalog.
pub struct FixedWorld {
    catalog: Catalog,
}

impl FixedWorld {
    pub fn new() -> Self {
        Self {
            catalog: common::a_catalog(),
        }
    }
}

impl WorldReader for FixedWorld {
    fn status(&self, _printer_id: &str) -> Option<PrinterStatus> {
        Some(ready_status())
    }

    fn capabilities(&self, printer: &StoredPrinter) -> PrinterCapabilities {
        capabilities_for(printer, None)
    }

    fn catalog(&self) -> &Catalog {
        &self.catalog
    }
}
