pub mod inherits;
pub mod shape;

use crate::catalog::{CatalogModel, CatalogVariant, PointMm};
use inherits::{resolve_machine_preset, InheritsError};
use serde_json::Value;
use shape::{parse_printable_area, ShapeError};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug)]
pub enum IngestError {
    Io(String),
    Json(String),
}

impl std::fmt::Display for IngestError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            IngestError::Io(e) => write!(f, "io error: {e}"),
            IngestError::Json(e) => write!(f, "json error: {e}"),
        }
    }
}

impl From<InheritsError> for IngestError {
    fn from(e: InheritsError) -> Self {
        IngestError::Json(e.0)
    }
}

impl From<ShapeError> for IngestError {
    fn from(e: ShapeError) -> Self {
        IngestError::Json(e.0)
    }
}

/// Ingests OrcaSlicer's `resources/profiles` directory into a list of
/// CatalogModels, allowlisting only the factual capability fields farm3d
/// reads — never g-code, bed/hotend model filenames, or vendor descriptions.
/// A broken individual model or variant (unresolvable inherits, missing
/// fields) is skipped rather than failing the whole vendor.
pub fn ingest_profiles_dir(dir: &Path) -> Result<Vec<CatalogModel>, IngestError> {
    let mut models = Vec::new();
    let mut vendor_files: Vec<PathBuf> = fs::read_dir(dir)
        .map_err(|e| IngestError::Io(e.to_string()))?
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| p.extension().map(|x| x == "json").unwrap_or(false))
        .collect();
    vendor_files.sort();

    for vendor_file in vendor_files {
        let vendor_name = vendor_file
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or_default()
            .to_string();
        let bundle: Value = match fs::read_to_string(&vendor_file) {
            Ok(s) => match serde_json::from_str(&s) {
                Ok(v) => v,
                Err(_) => continue,
            },
            Err(_) => continue,
        };

        let machine_dir = dir.join(&vendor_name).join("machine");
        let index = index_machine_dir(&machine_dir)?;

        let model_list = bundle["machine_model_list"]
            .as_array()
            .cloned()
            .unwrap_or_default();
        let machine_list = bundle["machine_list"]
            .as_array()
            .cloned()
            .unwrap_or_default();

        for model_entry in &model_list {
            let sub_path = model_entry["sub_path"].as_str().unwrap_or_default();
            let model_json: Value = match fs::read_to_string(dir.join(&vendor_name).join(sub_path))
            {
                Ok(s) => match serde_json::from_str(&s) {
                    Ok(v) => v,
                    Err(_) => continue,
                },
                Err(_) => continue,
            };
            if let Some(tech) = model_json.get("machine_tech").and_then(|v| v.as_str()) {
                if tech != "FFF" {
                    continue;
                }
            }
            let model_name = model_json["name"].as_str().unwrap_or_default().to_string();
            let model_id = model_json["model_id"]
                .as_str()
                .unwrap_or_default()
                .to_string();

            let mut variants = Vec::new();
            for variant_entry in &machine_list {
                let variant_name = match variant_entry["name"].as_str() {
                    Some(n) => n,
                    None => continue,
                };
                let resolved = match resolve_machine_preset(&index, variant_name) {
                    Ok(v) => v,
                    Err(_) => continue,
                };
                if resolved.get("printer_model").and_then(|v| v.as_str())
                    != Some(model_name.as_str())
                {
                    continue;
                }
                if let Ok(variant) = extract_variant(variant_name, &resolved) {
                    variants.push(variant);
                }
            }
            if !variants.is_empty() {
                models.push(CatalogModel {
                    model_id,
                    vendor: vendor_name.clone(),
                    model: model_name,
                    variants,
                });
            }
        }
    }
    Ok(models)
}

fn index_machine_dir(machine_dir: &Path) -> Result<HashMap<String, Value>, IngestError> {
    let mut index = HashMap::new();
    if !machine_dir.exists() {
        return Ok(index);
    }
    for path in walk_json_files(machine_dir)? {
        let contents = fs::read_to_string(&path).map_err(|e| IngestError::Io(e.to_string()))?;
        let json: Value = match serde_json::from_str(&contents) {
            Ok(v) => v,
            Err(_) => continue,
        };
        if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
            index.insert(name.to_string(), json);
        }
    }
    Ok(index)
}

fn walk_json_files(dir: &Path) -> Result<Vec<PathBuf>, IngestError> {
    let mut out = Vec::new();
    for entry in fs::read_dir(dir).map_err(|e| IngestError::Io(e.to_string()))? {
        let path = entry.map_err(|e| IngestError::Io(e.to_string()))?.path();
        if path.is_dir() {
            out.extend(walk_json_files(&path)?);
        } else if path.extension().map(|x| x == "json").unwrap_or(false) {
            out.push(path);
        }
    }
    Ok(out)
}

/// Allowlist: only these fields are ever copied out of a resolved preset.
/// Never g-code, filenames, or descriptions — see ADR 0007.
fn extract_variant(name: &str, resolved: &Value) -> Result<CatalogVariant, IngestError> {
    let printable_area: Vec<String> = resolved["printable_area"]
        .as_array()
        .ok_or_else(|| IngestError::Json(format!("{name}: missing printable_area")))?
        .iter()
        .filter_map(|v| v.as_str().map(|s| s.to_string()))
        .collect();
    let bed_shape = parse_printable_area(&printable_area)?;

    let printable_height_mm = resolved["printable_height"]
        .as_str()
        .and_then(|s| s.parse::<f64>().ok())
        .ok_or_else(|| IngestError::Json(format!("{name}: missing/invalid printable_height")))?;

    let nozzle_diameter_mm: Vec<f64> = resolved["nozzle_diameter"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().and_then(|s| s.parse::<f64>().ok()))
                .collect()
        })
        .unwrap_or_default();

    let bed_exclude_areas: Vec<PointMm> = resolved
        .get("bed_exclude_area")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str())
                .filter_map(|s| s.split_once('x'))
                .filter_map(|(x, y)| {
                    Some(PointMm {
                        x_mm: x.parse().ok()?,
                        y_mm: y.parse().ok()?,
                    })
                })
                .collect()
        })
        .unwrap_or_default();

    let flag = |key: &str| resolved.get(key).and_then(|v| v.as_str()) == Some("1");

    Ok(CatalogVariant {
        variant: name.to_string(),
        printer_variant: resolved["printer_variant"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        bed_shape,
        printable_height_mm,
        bed_exclude_areas,
        default_bed_type: resolved["default_bed_type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        nozzle_diameter_mm,
        nozzle_type: resolved["nozzle_type"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        gcode_flavor: resolved["gcode_flavor"]
            .as_str()
            .unwrap_or_default()
            .to_string(),
        has_auxiliary_fan: flag("auxiliary_fan"),
        supports_air_filtration: flag("support_air_filtration"),
        supports_multi_filament: flag("support_multi_filament"),
        suggested_host_type: resolved["host_type"].as_str().map(|s| s.to_string()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::BedShape;

    fn fixtures_dir() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/profiles")
    }

    #[test]
    fn ingests_all_three_valid_models_and_skips_the_ghost_entry() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let names: Vec<&str> = models.iter().map(|m| m.model.as_str()).collect();
        assert_eq!(names.len(), 3);
        assert!(names.contains(&"Test Printer"));
        assert!(names.contains(&"Test Delta"));
        assert!(names.contains(&"Test Printer 5T"));
    }

    #[test]
    fn resolves_printable_area_and_fan_flags_through_a_three_deep_chain() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let printer = models.iter().find(|m| m.model == "Test Printer").unwrap();
        let variant = &printer.variants[0];
        assert_eq!(
            variant.bed_shape,
            BedShape::Rectangular {
                width_mm: 220.0,
                depth_mm: 220.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            }
        );
        assert_eq!(variant.printable_height_mm, 250.0);
        // Overridden by the mid-chain preset, not the base.
        assert!(variant.has_auxiliary_fan);
        assert!(variant.supports_air_filtration);
    }

    #[test]
    fn non_rectangular_own_printable_area_becomes_polygon() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let delta = models.iter().find(|m| m.model == "Test Delta").unwrap();
        match &delta.variants[0].bed_shape {
            BedShape::Polygon { points } => assert_eq!(points.len(), 5),
            other => panic!("expected Polygon, got {other:?}"),
        }
        assert_eq!(delta.variants[0].gcode_flavor, "klipper");
    }

    #[test]
    fn multi_toolhead_nozzle_diameter_is_preserved_as_an_array() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let t5 = models
            .iter()
            .find(|m| m.model == "Test Printer 5T")
            .unwrap();
        assert_eq!(
            t5.variants[0].nozzle_diameter_mm,
            vec![0.4, 0.4, 0.4, 0.4, 0.4]
        );
    }

    #[test]
    fn model_id_and_vendor_are_captured() {
        let models = ingest_profiles_dir(&fixtures_dir()).unwrap();
        let printer = models.iter().find(|m| m.model == "Test Printer").unwrap();
        assert_eq!(printer.model_id, "TestVendor-TP");
        assert_eq!(printer.vendor, "TestVendor");
    }
}
