use farm3d_lib::catalog::{BedShape, Catalog};
use std::path::Path;

fn load_snapshot_raw() -> String {
    std::fs::read_to_string(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("resources/printer-catalog.json"),
    )
    .expect("resources/printer-catalog.json should be committed")
}

#[test]
fn snapshot_is_complete() {
    let raw = load_snapshot_raw();
    let catalog: Catalog = serde_json::from_str(&raw).expect("snapshot should be valid JSON");
    assert!(!catalog.models.is_empty(), "snapshot has no models");

    for model in &catalog.models {
        assert!(!model.variants.is_empty(), "{} has no variants", model.model);
        for variant in &model.variants {
            assert!(
                variant.printable_height_mm > 0.0,
                "{} has no printable height",
                variant.variant
            );
            match &variant.bed_shape {
                BedShape::Rectangular { width_mm, depth_mm, .. } => {
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
