pub mod ingest;

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "camelCase", rename_all_fields = "camelCase")]
pub enum BedShape {
    Rectangular {
        width_mm: f64,
        depth_mm: f64,
        origin_x_mm: f64,
        origin_y_mm: f64,
    },
    Polygon {
        points: Vec<PointMm>,
    },
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PointMm {
    pub x_mm: f64,
    pub y_mm: f64,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogVariant {
    pub variant: String,
    pub printer_variant: String,
    pub bed_shape: BedShape,
    pub printable_height_mm: f64,
    pub bed_exclude_areas: Vec<PointMm>,
    pub default_bed_type: String,
    pub nozzle_diameter_mm: Vec<f64>,
    pub nozzle_type: String,
    pub gcode_flavor: String,
    pub has_auxiliary_fan: bool,
    pub supports_air_filtration: bool,
    pub supports_multi_filament: bool,
    pub suggested_host_type: Option<String>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct CatalogModel {
    pub model_id: String,
    pub vendor: String,
    pub model: String,
    pub variants: Vec<CatalogVariant>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase")]
pub struct Catalog {
    pub generated_at: String,
    pub source_tag: String,
    pub notice: String,
    pub models: Vec<CatalogModel>,
}

pub fn load_snapshot(path: &Path) -> Result<Catalog, String> {
    let contents = fs::read_to_string(path).map_err(|e| e.to_string())?;
    serde_json::from_str(&contents).map_err(|e| e.to_string())
}

/// The fully-resolved capability set for one Printer instance — every field
/// present, whether inherited from the catalog or overridden. Distinct from
/// `CatalogVariant`, which additionally carries `variant`/`printer_variant`
/// identity fields that describe the catalog entry, not a resolved instance.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PrinterProfile {
    pub bed_shape: BedShape,
    pub printable_height_mm: f64,
    pub bed_exclude_areas: Vec<PointMm>,
    pub default_bed_type: String,
    pub nozzle_diameter_mm: Vec<f64>,
    pub nozzle_type: String,
    pub gcode_flavor: String,
    pub has_auxiliary_fan: bool,
    pub supports_air_filtration: bool,
    pub supports_multi_filament: bool,
    pub suggested_host_type: Option<String>,
}

impl From<&CatalogVariant> for PrinterProfile {
    fn from(v: &CatalogVariant) -> Self {
        Self {
            bed_shape: v.bed_shape.clone(),
            printable_height_mm: v.printable_height_mm,
            bed_exclude_areas: v.bed_exclude_areas.clone(),
            default_bed_type: v.default_bed_type.clone(),
            nozzle_diameter_mm: v.nozzle_diameter_mm.clone(),
            nozzle_type: v.nozzle_type.clone(),
            gcode_flavor: v.gcode_flavor.clone(),
            has_auxiliary_fan: v.has_auxiliary_fan,
            supports_air_filtration: v.supports_air_filtration,
            supports_multi_filament: v.supports_multi_filament,
            suggested_host_type: v.suggested_host_type.clone(),
        }
    }
}

#[cfg(test)]
mod snapshot_tests {
    use super::*;
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_path() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!(
            "farm3d-catalog-test-{}-{}.json",
            std::process::id(),
            id
        ))
    }

    #[test]
    fn loads_a_valid_snapshot() {
        let path = temp_path();
        let catalog = Catalog {
            generated_at: "2026-08-20T00:00:00Z".to_string(),
            source_tag: "v2.4.2".to_string(),
            notice: "test".to_string(),
            models: vec![],
        };
        fs::write(&path, serde_json::to_string(&catalog).unwrap()).unwrap();

        let loaded = load_snapshot(&path).unwrap();
        assert_eq!(loaded, catalog);
        fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_is_an_error() {
        let path = temp_path();
        assert!(load_snapshot(&path).is_err());
    }

    #[test]
    fn corrupt_file_is_an_error() {
        let path = temp_path();
        fs::write(&path, "not valid json").unwrap();
        assert!(load_snapshot(&path).is_err());
        fs::remove_file(&path).ok();
    }
}
