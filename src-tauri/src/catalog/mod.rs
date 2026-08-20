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
