pub mod ingest;

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(tag = "kind", rename_all = "camelCase")]
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
