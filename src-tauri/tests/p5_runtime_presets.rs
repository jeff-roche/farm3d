//! P5 Task 4: `list_slice_options` and the D4 mapping against the
//! `TestVendor` profile fixture with a real Printer store, plus the
//! `#[ignore]` real-OrcaSlicer checks (spec D23).
//!
//! The real-OrcaSlicer tests read `FARM3D_ORCA` (the engine) and, when set,
//! `FARM3D_ORCA_PRESETS` (a separate preset source). Run them with
//! `cargo test --test p5_runtime_presets -- --ignored --nocapture`.

mod common;

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use farm3d_lib::catalog::{Catalog, CatalogModel, CatalogVariant};
use farm3d_lib::contracts::command::ErrorCode;
use farm3d_lib::library::content::CancelFlag;
use farm3d_lib::persistence::Storage;
use farm3d_lib::printers::repository::PrinterRepository;
use farm3d_lib::printers::{CatalogRef, PrinterProfileOverrides, StoredPrinter};
use farm3d_lib::slicing::mapping::{apply_overrides_and_controls, mapped_keys, SliceSettingsInput};
use farm3d_lib::slicing::presets::{list_slice_options, resolve_target, PresetIndex, PresetKind};
use farm3d_lib::slicing::repository::SlicerRuntimeConfig;
use farm3d_lib::slicing::runtime::{
    probe_engine, resolve_runtime, DiscoveryEnv, EngineState, PresetSourceState, ProbeOutcome,
    PROFILE_CACHE_DIR,
};
use farm3d_lib::slicing::{BrimType, InfillPattern, SliceControls, SliceTarget, SupportMode};
use farm3d_lib::spools::ledger::AmountEntry;
use farm3d_lib::spools::repository::{self, StoredSpool};
use farm3d_lib::spools::slots::{InitialLoad, SlotSpec};
use farm3d_lib::spools::{AmountConfidence, FilamentDiameter, MaterialFamily, SpoolFields};

use common::{a_catalog, a_ref, a_stored_printer};

fn fixture_index() -> PresetIndex {
    let profiles = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profiles");
    PresetIndex::build(&profiles, "2.4.2", &CancelFlag::never()).expect("fixture index")
}

fn spool(storage: &Storage, family: MaterialFamily) -> StoredSpool {
    let fields = SpoolFields {
        manufacturer: "Polymaker".to_string(),
        product: None,
        material_family: family,
        material_other: None,
        color_name: "Black".to_string(),
        color_hex: None,
        diameter: FilamentDiameter::D175,
        nominal_mg: 1_000_000,
        low_threshold_mg: 100_000,
        tare_id: None,
        notes: None,
    };
    let entry = AmountEntry::Net {
        net_mg: 1_000_000,
        confidence: AmountConfidence::Estimated,
    };
    storage
        .write_repo(|tx| repository::insert_spool(tx, &fields, &entry, None))
        .unwrap()
}

fn create(storage: &Arc<Storage>, printer: StoredPrinter, loads: &[&StoredSpool]) {
    let layout = vec![SlotSpec {
        id: None,
        name: "Slot 1".to_string(),
        feeder_label: None,
    }];
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
        .create_with_layout(printer, None, &layout, &loads)
        .unwrap();
}

/// `prn-a` holds a PETG Spool, `prn-b` overrides its printable height,
/// `prn-c` is archived, and `prn-d` is an empty stock Printer.
fn farm(storage: &Arc<Storage>) {
    let petg = spool(storage, MaterialFamily::Petg);
    create(storage, a_stored_printer("prn-a"), &[&petg]);
    create(
        storage,
        StoredPrinter {
            overrides: PrinterProfileOverrides {
                printable_height_mm: Some(200.0),
                ..PrinterProfileOverrides::default()
            },
            ..a_stored_printer("prn-b")
        },
        &[],
    );
    create(
        storage,
        StoredPrinter {
            archived_at: Some("2026-09-01T00:00:00.000Z".to_string()),
            ..a_stored_printer("prn-c")
        },
        &[],
    );
    create(storage, a_stored_printer("prn-d"), &[]);
}

fn printer(id: &str) -> SliceTarget {
    SliceTarget::Printer {
        printer_id: id.to_string(),
    }
}

#[test]
fn options_for_a_printer_offer_its_presets_and_default_to_its_spool() {
    let (_temp, _lease, storage, _) = common::storage();
    farm(&storage);
    let options =
        list_slice_options(&storage, &a_catalog(), &fixture_index(), &printer("prn-a")).unwrap();
    assert_eq!(options.machine_preset, "Test Printer 0.4 nozzle");
    let processes: Vec<&str> = options
        .process_presets
        .iter()
        .map(|p| p.name.as_str())
        .collect();
    assert_eq!(processes, ["0.12mm Fine @Test", "0.20mm Standard @Test"]);
    let filaments: Vec<&str> = options
        .filament_presets
        .iter()
        .map(|f| f.name.as_str())
        .collect();
    assert_eq!(
        filaments,
        [
            "Generic PLA @Test Printer",
            "Test ABS @Test Printer",
            "Test PETG @Test Printer"
        ]
    );
    assert_eq!(
        options.filament_presets[0].material_family,
        Some(MaterialFamily::Pla)
    );
    assert_eq!(
        options.defaults.process_preset.as_deref(),
        Some("0.20mm Standard @Test")
    );
    assert_eq!(
        options.defaults.filament_preset.as_deref(),
        Some("Test PETG @Test Printer")
    );
    assert_eq!(options.profile_snapshot.catalog_ref, a_ref());
    assert_eq!(options.profile_snapshot.printable_height_mm, 256.0);
    // The archived Printer and the one with a different height don't match.
    assert_eq!(options.matching_printer_ids, ["prn-a", "prn-d"]);

    let wire = serde_json::to_value(&options).unwrap();
    assert_eq!(
        wire["defaults"]["filamentPreset"],
        "Test PETG @Test Printer"
    );
    assert_eq!(wire["filamentPresets"][2]["materialFamily"], "PETG");
    assert!(wire["matchingPrinterIds"].is_array());
}

#[test]
fn options_for_a_catalog_profile_default_to_generic_pla() {
    let (_temp, _lease, storage, _) = common::storage();
    farm(&storage);
    let target = SliceTarget::Profile {
        catalog_ref: a_ref(),
    };
    let options = list_slice_options(&storage, &a_catalog(), &fixture_index(), &target).unwrap();
    assert_eq!(
        options.defaults.filament_preset.as_deref(),
        Some("Generic PLA @Test Printer")
    );
    assert_eq!(options.matching_printer_ids, ["prn-a", "prn-d"]);
}

#[test]
fn an_overridden_printer_matches_only_itself_and_its_override_reaches_the_machine_preset() {
    let (_temp, _lease, storage, _) = common::storage();
    farm(&storage);
    let catalog = a_catalog();
    let index = fixture_index();
    let options = list_slice_options(&storage, &catalog, &index, &printer("prn-b")).unwrap();
    assert_eq!(options.profile_snapshot.printable_height_mm, 200.0);
    assert_eq!(options.matching_printer_ids, ["prn-b"]);
    // No Spool loaded: the Generic PLA default.
    assert_eq!(
        options.defaults.filament_preset.as_deref(),
        Some("Generic PLA @Test Printer")
    );

    let target = resolve_target(&storage, &catalog, &printer("prn-b")).unwrap();
    assert_eq!(target.overridden_fields, ["printableHeightMm"]);
    let controls = SliceControls {
        layer_height_mm: Some(0.16),
        ..SliceControls::default()
    };
    let documents = apply_overrides_and_controls(
        &index,
        SliceSettingsInput {
            machine_preset: &target.machine_preset,
            process_preset: options.defaults.process_preset.as_deref().unwrap(),
            filament_preset: options.defaults.filament_preset.as_deref().unwrap(),
            profile: &target.profile,
            overridden_fields: &target.overridden_fields,
            unknown_override_keys: &target.unknown_override_keys,
            controls: &controls,
        },
    )
    .unwrap();
    assert_eq!(documents.machine["printable_height"], "200");
    assert_eq!(documents.process["layer_height"], "0.16");
}

/// A Printer whose stored variant name drifted is rematched by its printer
/// variant. The snapshot names the variant actually sliced, not the stale
/// stored ref.
#[test]
fn a_rematched_printer_snapshots_the_variant_it_slices() {
    let (_temp, _lease, storage, _) = common::storage();
    let stale = CatalogRef {
        variant: "Test Printer 0.4 nozzle (renamed)".to_string(),
        ..a_ref()
    };
    create(
        &storage,
        StoredPrinter {
            catalog_ref: stale,
            ..a_stored_printer("prn-r")
        },
        &[],
    );
    let catalog = a_catalog();
    let target = resolve_target(&storage, &catalog, &printer("prn-r")).unwrap();
    assert_eq!(target.machine_preset, "Test Printer 0.4 nozzle");
    assert_eq!(target.catalog_ref, a_ref());
    let options =
        list_slice_options(&storage, &catalog, &fixture_index(), &printer("prn-r")).unwrap();
    assert_eq!(options.profile_snapshot.catalog_ref, a_ref());
    assert_eq!(options.machine_preset, "Test Printer 0.4 nozzle");
}

#[test]
fn an_unknown_override_key_blocks_slicing_for_a_printer() {
    let (_temp, _lease, storage, _) = common::storage();
    let mut overrides = PrinterProfileOverrides::default();
    overrides
        .extra
        .insert("nozzleHeaterWatts".to_string(), serde_json::json!(60));
    create(
        &storage,
        StoredPrinter {
            overrides,
            ..a_stored_printer("prn-x")
        },
        &[],
    );
    let catalog = a_catalog();
    let index = fixture_index();
    let target = resolve_target(&storage, &catalog, &printer("prn-x")).unwrap();
    let error = apply_overrides_and_controls(
        &index,
        SliceSettingsInput {
            machine_preset: &target.machine_preset,
            process_preset: "0.20mm Standard @Test",
            filament_preset: "Generic PLA @Test Printer",
            profile: &target.profile,
            overridden_fields: &target.overridden_fields,
            unknown_override_keys: &target.unknown_override_keys,
            controls: &SliceControls::default(),
        },
    )
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::UnmappedProfileOverride);
}

#[test]
fn missing_targets_and_presets_are_refused() {
    let (_temp, _lease, storage, _) = common::storage();
    let index = fixture_index();
    let missing =
        list_slice_options(&storage, &a_catalog(), &index, &printer("prn-gone")).unwrap_err();
    assert_eq!(missing.code, ErrorCode::NotFound);

    // A catalog variant the preset source doesn't have.
    let mut catalog = a_catalog();
    let mut variant = catalog.models[0].variants[0].clone();
    variant.variant = "Other Printer 0.4 nozzle".to_string();
    variant.printer_variant = "0.6".to_string();
    catalog.models[0].variants.push(variant);
    let target = SliceTarget::Profile {
        catalog_ref: CatalogRef {
            variant: "Other Printer 0.4 nozzle".to_string(),
            printer_variant: "0.6".to_string(),
            ..a_ref()
        },
    };
    let error = list_slice_options(&storage, &catalog, &index, &target).unwrap_err();
    assert_eq!(error.code, ErrorCode::PresetNotFound);

    // Multi-nozzle profiles are out of P5's scope.
    let multi = Catalog {
        models: vec![CatalogModel {
            variants: vec![CatalogVariant {
                nozzle_diameter_mm: vec![0.4, 0.4],
                ..a_catalog().models[0].variants[0].clone()
            }],
            ..a_catalog().models[0].clone()
        }],
        ..a_catalog()
    };
    let target = SliceTarget::Profile {
        catalog_ref: a_ref(),
    };
    assert_eq!(
        list_slice_options(&storage, &multi, &index, &target)
            .unwrap_err()
            .code,
        ErrorCode::Validation
    );
}

// ---------------------------------------------------------------------------
// Real OrcaSlicer (spec D23): `FARM3D_ORCA=<engine>`, optional
// `FARM3D_ORCA_PRESETS=<preset source>`.
// ---------------------------------------------------------------------------

fn real_orca() -> Option<(PathBuf, Option<PathBuf>)> {
    let engine = std::env::var_os("FARM3D_ORCA").map(PathBuf::from)?;
    let presets = std::env::var_os("FARM3D_ORCA_PRESETS").map(PathBuf::from);
    Some((engine, presets))
}

fn real_config(engine: &Path, presets: Option<&Path>) -> SlicerRuntimeConfig {
    SlicerRuntimeConfig {
        revision: 1,
        engine_path: Some(engine.to_str().unwrap().to_string()),
        preset_source_path: presets.map(|path| path.to_str().unwrap().to_string()),
        updated_at: None,
    }
}

fn no_discovery() -> DiscoveryEnv {
    DiscoveryEnv {
        home: None,
        path_var: None,
        probe_timeout: Duration::from_secs(10),
    }
}

#[test]
#[ignore = "needs a real OrcaSlicer: set FARM3D_ORCA"]
fn real_orca_engine_probes_as_a_supported_version() {
    let Some((engine, presets)) = real_orca() else {
        eprintln!("FARM3D_ORCA is not set; skipping");
        return;
    };
    let probe = probe_engine(&engine, Duration::from_secs(10));
    eprintln!("probe: {probe:?}");
    assert!(
        matches!(probe.outcome, ProbeOutcome::Supported(_)),
        "{probe:?}"
    );

    let cache = tempfile::tempdir().unwrap();
    let runtime = resolve_runtime(
        &real_config(&engine, presets.as_deref()),
        &no_discovery(),
        cache.path(),
    );
    eprintln!(
        "status: {}",
        serde_json::to_string_pretty(&runtime.status).unwrap()
    );
    assert!(matches!(
        runtime.status.engine,
        EngineState::Available { .. }
    ));
    if let PresetSourceState::Available { .. } = runtime.status.preset_source {
        let source = runtime.preset_source.unwrap();
        if let Some(hash) = &source.cache_hash {
            // AppImage extraction: only JSON files are kept in the cache.
            let root = cache.path().join(PROFILE_CACHE_DIR).join(hash);
            assert!(source.profiles_dir.starts_with(&root));
            let non_json = walk(&root)
                .into_iter()
                .filter(|path| path.extension().is_none_or(|ext| ext != "json"))
                .count();
            assert_eq!(non_json, 0);
            eprintln!(
                "extracted {} vendors into {}",
                source.vendor_count,
                root.display()
            );
        }
    }
}

fn walk(dir: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    for entry in std::fs::read_dir(dir).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files.extend(walk(&path));
        } else {
            files.push(path);
        }
    }
    files
}

#[test]
#[ignore = "needs a real OrcaSlicer with readable presets: set FARM3D_ORCA (and FARM3D_ORCA_PRESETS)"]
fn real_orca_preset_source_knows_every_mapped_key() {
    let Some((engine, presets)) = real_orca() else {
        eprintln!("FARM3D_ORCA is not set; skipping");
        return;
    };
    let cache = tempfile::tempdir().unwrap();
    let runtime = resolve_runtime(
        &real_config(&engine, presets.as_deref()),
        &no_discovery(),
        cache.path(),
    );
    let source = runtime.preset_source.unwrap_or_else(|| {
        panic!(
            "no readable preset source: {:?}",
            runtime.status.preset_source
        )
    });
    let started = std::time::Instant::now();
    let index = PresetIndex::build(
        &source.profiles_dir,
        &source.version.to_string(),
        &CancelFlag::never(),
    )
    .unwrap();
    eprintln!(
        "indexed {} vendors in {:?}",
        index.vendor_count(),
        started.elapsed()
    );
    for (kind, key) in mapped_keys() {
        assert!(
            index.is_known_key(kind, key),
            "{key} is unknown to the {kind:?} presets"
        );
    }

    // The spike's Elegoo Centauri Carbon: offered presets, defaults, and
    // every control and override applied.
    let machine = "Elegoo Centauri Carbon 0.4 nozzle";
    let processes = index.offered(PresetKind::Process, machine);
    let filaments = index.offered(PresetKind::Filament, machine);
    eprintln!(
        "{} processes, {} filaments offered for {machine}",
        processes.len(),
        filaments.len()
    );
    let process = farm3d_lib::slicing::presets::default_process(&processes).unwrap();
    let filament =
        farm3d_lib::slicing::presets::default_filament(&filaments, &[MaterialFamily::Pla]).unwrap();
    eprintln!("defaults: {process} / {filament}");
    assert_eq!(process, "0.20mm Standard @Elegoo CC 0.4 nozzle");

    let catalog: Catalog = serde_json::from_str(
        &std::fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("../public/catalog/printer-catalog.json"),
        )
        .unwrap(),
    )
    .unwrap();
    let variant = catalog
        .models
        .iter()
        .flat_map(|model| &model.variants)
        .find(|variant| variant.variant == machine)
        .unwrap();
    let profile = farm3d_lib::catalog::PrinterProfile::from(variant);
    let every_field: Vec<String> = [
        "bedShape",
        "printableHeightMm",
        "bedExcludeAreas",
        "nozzleDiameterMm",
        "nozzleType",
        "gcodeFlavor",
        "defaultBedType",
    ]
    .iter()
    .map(|field| field.to_string())
    .collect();
    let controls = SliceControls {
        layer_height_mm: Some(0.2),
        wall_loops: Some(3),
        top_shell_layers: Some(5),
        bottom_shell_layers: Some(4),
        infill_density_percent: Some(20.0),
        infill_pattern: Some(InfillPattern::Gyroid),
        supports: Some(SupportMode::TreeAuto),
        support_threshold_angle_deg: Some(35.0),
        brim_type: Some(BrimType::OuterOnly),
        brim_width_mm: Some(4.0),
        skirt_loops: Some(1),
    };
    let documents = apply_overrides_and_controls(
        &index,
        SliceSettingsInput {
            machine_preset: machine,
            process_preset: &process,
            filament_preset: &filament,
            profile: &profile,
            overridden_fields: &every_field,
            unknown_override_keys: &[],
            controls: &controls,
        },
    )
    .unwrap();
    // Overriding with the catalog's own values reproduces the preset's.
    let flat = index.flatten(PresetKind::Machine, machine).unwrap();
    for key in [
        "printable_area",
        "printable_height",
        "bed_exclude_area",
        "nozzle_diameter",
        "nozzle_type",
        "gcode_flavor",
        "default_bed_type",
    ] {
        assert_eq!(documents.machine[key], flat[key], "{key}");
    }
    assert_eq!(documents.machine["from"], "system");
    assert!(documents.machine.get("inherits").is_none());
}
