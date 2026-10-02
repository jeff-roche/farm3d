use farm3d_lib::catalog::{BedShape, Catalog};
use std::path::Path;

fn load_snapshot_raw() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../public/catalog/printer-catalog.json"),
    )
    .expect("public/catalog/printer-catalog.json should be committed")
}

#[test]
fn snapshot_is_complete() {
    let raw = load_snapshot_raw();
    let catalog: Catalog = serde_json::from_str(&raw).expect("snapshot should be valid JSON");
    assert!(!catalog.models.is_empty(), "snapshot has no models");

    for model in &catalog.models {
        assert!(
            !model.variants.is_empty(),
            "{} has no variants",
            model.model
        );
        for variant in &model.variants {
            assert!(
                variant.printable_height_mm > 0.0,
                "{} has no printable height",
                variant.variant
            );
            match &variant.bed_shape {
                BedShape::Rectangular {
                    width_mm, depth_mm, ..
                } => {
                    assert!(
                        *width_mm > 0.0 && *depth_mm > 0.0,
                        "{} has a zero-size rectangular bed",
                        variant.variant
                    );
                }
                BedShape::Polygon { points } => {
                    assert!(
                        points.len() >= 3,
                        "{} has a degenerate polygon bed",
                        variant.variant
                    );
                }
            }
        }
    }
}

/// `(vendor, model)` is the key `resolve_catalog_ref` and `list_catalog_variants`
/// match on, so it has to stay unique across every regenerated catalog.
/// `modelId` is deliberately NOT asserted unique — the real catalog legitimately
/// has duplicate and empty modelIds, which is precisely why resolution keys on
/// the name pair instead.
#[test]
fn snapshot_vendor_model_pairs_are_unique() {
    let raw = load_snapshot_raw();
    let catalog: Catalog = serde_json::from_str(&raw).expect("snapshot should be valid JSON");
    let mut seen = std::collections::HashSet::new();
    for model in &catalog.models {
        let key = (model.vendor.clone(), model.model.clone());
        assert!(
            seen.insert(key),
            "duplicate (vendor, model) pair: ({}, {})",
            model.vendor,
            model.model
        );
    }
}

/// The executable form of ADR 0007's allowlist rule: never let a future field
/// addition smuggle g-code or filenames into the shipped catalog.
#[test]
fn snapshot_carries_no_disallowed_fields() {
    let raw = load_snapshot_raw();
    let lower = raw.to_lowercase();
    let disallowed = [
        "start_gcode",
        "startgcode",
        "end_gcode",
        "endgcode",
        "change_filament_gcode",
        "changefilamentgcode",
        "bed_model",
        "bedmodel",
        "bed_texture",
        "bedtexture",
        "hotend_model",
        "hotendmodel",
    ];
    for marker in disallowed {
        assert!(
            !lower.contains(marker),
            "snapshot contains disallowed field marker: {marker}"
        );
    }
}

/// #26: OrcaSlicer tags every Elegoo FDM model `elegoolink`, but farm3d's
/// ElegooLink means SDCP V3 only. `catalog::ingest::overrides` corrects the
/// rest; this pins one model of each outcome in the committed snapshot.
#[test]
fn snapshot_carries_the_elegoo_host_corrections() {
    let raw = load_snapshot_raw();
    let catalog: Catalog = serde_json::from_str(&raw).expect("snapshot should be valid JSON");
    let hints = |model_id: &str| -> Vec<(Option<String>, Option<u16>)> {
        let model = catalog
            .models
            .iter()
            .find(|m| m.vendor == "Elegoo" && m.model_id == model_id)
            .unwrap_or_else(|| panic!("no Elegoo model {model_id}"));
        model
            .variants
            .iter()
            .map(|v| (v.suggested_host_type.clone(), v.suggested_port))
            .collect()
    };
    let all = |model_id: &str, expected: (Option<&str>, Option<u16>)| {
        for hint in hints(model_id) {
            assert_eq!(
                (hint.0.as_deref(), hint.1),
                expected,
                "{model_id} has the wrong connection hint"
            );
        }
    };

    all("Elegoo-CC", (Some("elegoolink"), None));
    all("Elegoo-CC2", (None, None));
    all("Elegoo-N4", (Some("moonraker"), Some(80)));
    all("Elegoo-N3", (None, None));

    // Only the corrections carry a port.
    let ported = catalog
        .models
        .iter()
        .flat_map(|m| m.variants.iter().map(move |v| (m, v)))
        .filter(|(_, v)| v.suggested_port.is_some())
        .all(|(m, _)| m.vendor == "Elegoo");
    assert!(ported, "a non-Elegoo variant carries a suggested port");
}
