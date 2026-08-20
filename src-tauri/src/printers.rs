//! Persistence for `printers.json` — the user-owned counterpart to the
//! read-only catalog. This task lands the types and path-taking functions
//! only; nothing in the crate calls into this module outside its own tests
//! yet (Tauri commands and printer resolution wire it in as later tasks in
//! this plan), so the compiler can't see these `pub` items as reachable.
#![allow(dead_code)]

use crate::catalog::{BedShape, PointMm, PrinterProfile};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

const PRINTERS_FILE_NAME: &str = "printers.json";
const PRINTERS_SCHEMA_VERSION: u32 = 1;

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PrintersFile {
    pub schema_version: u32,
    pub printers: Vec<StoredPrinter>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct StoredPrinter {
    pub id: String,
    pub name: String,
    pub catalog_ref: CatalogRef,
    pub group: String,
    pub notes: String,
    #[serde(skip_serializing_if = "PrinterProfileOverrides::is_empty")]
    pub overrides: PrinterProfileOverrides,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_known_good: Option<LastKnownGood>,
    /// Phase 2's Connection config. Opaque here — printers.rs never
    /// interprets it, only round-trips it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<serde_json::Value>,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct CatalogRef {
    pub vendor: String,
    pub model: String,
    pub variant: String,
    pub model_id: String,
    pub printer_variant: String,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct PrinterProfileOverrides {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_shape: Option<BedShape>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub printable_height_mm: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bed_exclude_areas: Option<Vec<PointMm>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub default_bed_type: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub has_auxiliary_fan: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub supports_air_filtration: Option<bool>,
    /// Unknown/future keys, preserved verbatim through read-modify-write so
    /// an older farm3d never destroys a newer one's data.
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

impl PrinterProfileOverrides {
    pub fn is_empty(&self) -> bool {
        self.bed_shape.is_none()
            && self.printable_height_mm.is_none()
            && self.bed_exclude_areas.is_none()
            && self.default_bed_type.is_none()
            && self.has_auxiliary_fan.is_none()
            && self.supports_air_filtration.is_none()
            && self.extra.is_empty()
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct LastKnownGood {
    pub profile: PrinterProfile,
    pub catalog_version: String,
    pub resolved_at: String,
}

pub const OVERRIDABLE_FIELDS: &[&str] = &[
    "bedShape",
    "printableHeightMm",
    "bedExcludeAreas",
    "defaultBedType",
    "hasAuxiliaryFan",
    "supportsAirFiltration",
];

fn printers_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(PRINTERS_FILE_NAME)
}

pub fn write_printers_to(config_dir: &Path, file: &PrintersFile) -> Result<(), String> {
    fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(file).map_err(|e| e.to_string())?;
    fs::write(printers_file_path(config_dir), json).map_err(|e| e.to_string())
}

pub fn load_printers_from(config_dir: &Path) -> Result<PrintersFile, String> {
    let path = printers_file_path(config_dir);
    if !path.exists() {
        let defaults = PrintersFile {
            schema_version: PRINTERS_SCHEMA_VERSION,
            printers: vec![],
        };
        write_printers_to(config_dir, &defaults)?;
        return Ok(defaults);
    }
    let contents = fs::read_to_string(&path).map_err(|e| e.to_string())?;
    match serde_json::from_str::<PrintersFile>(&contents) {
        Ok(file) => Ok(file),
        Err(e) => {
            // Quarantine, never silently default to empty — unlike settings.json,
            // the next save here would overwrite the user's Farm with `[]`.
            use std::time::{SystemTime, UNIX_EPOCH};
            let ts = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
            let quarantine_path = config_dir.join(format!("{PRINTERS_FILE_NAME}.corrupt-{ts}"));
            fs::rename(&path, &quarantine_path).map_err(|e| e.to_string())?;
            Err(format!(
                "printers.json was corrupt ({e}); original preserved at {}",
                quarantine_path.display()
            ))
        }
    }
}

pub fn apply_override(
    overrides: &PrinterProfileOverrides,
    field: &str,
    value: Option<serde_json::Value>,
) -> Result<PrinterProfileOverrides, String> {
    if !OVERRIDABLE_FIELDS.contains(&field) {
        return Err(format!("`{field}` is not an overridable Printer Profile field"));
    }
    let mut map = match serde_json::to_value(overrides).map_err(|e| e.to_string())? {
        serde_json::Value::Object(m) => m,
        _ => unreachable!("PrinterProfileOverrides always serializes to a JSON object"),
    };
    match value {
        Some(v) => {
            map.insert(field.to_string(), v);
        }
        None => {
            map.remove(field);
        }
    }
    serde_json::from_value(serde_json::Value::Object(map))
        .map_err(|e| format!("invalid value for `{field}`: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("farm3d-printers-test-{}-{}", std::process::id(), id))
    }

    fn a_printer() -> StoredPrinter {
        StoredPrinter {
            id: "prn-1".to_string(),
            name: "Test Printer".to_string(),
            catalog_ref: CatalogRef {
                vendor: "TestVendor".to_string(),
                model: "Test Printer".to_string(),
                variant: "Test Printer 0.4 nozzle".to_string(),
                model_id: "TestVendor-TP".to_string(),
                printer_variant: "0.4".to_string(),
            },
            group: String::new(),
            notes: String::new(),
            overrides: PrinterProfileOverrides::default(),
            last_known_good: None,
            connection: None,
        }
    }

    #[test]
    fn load_creates_empty_file_when_missing() {
        let dir = temp_dir();
        let loaded = load_printers_from(&dir).unwrap();
        assert_eq!(loaded, PrintersFile { schema_version: 1, printers: vec![] });
        assert!(printers_file_path(&dir).exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir();
        let file = PrintersFile { schema_version: 1, printers: vec![a_printer()] };
        write_printers_to(&dir, &file).unwrap();
        let loaded = load_printers_from(&dir).unwrap();
        assert_eq!(loaded, file);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn corrupt_file_is_quarantined_not_silently_defaulted() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(printers_file_path(&dir), "not valid json").unwrap();

        let result = load_printers_from(&dir);
        assert!(result.is_err(), "a corrupt printers.json must be an error, not a silent default");

        let quarantined: Vec<_> = fs::read_dir(&dir)
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("printers.json.corrupt-"))
            .collect();
        assert_eq!(quarantined.len(), 1, "corrupt file should be quarantined, not deleted");
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn serialized_file_contains_only_overridden_override_keys() {
        let mut printer = a_printer();
        printer.overrides.supports_air_filtration = Some(false);
        let json = serde_json::to_string(&printer).unwrap();
        assert!(json.contains(r#""overrides":{"supportsAirFiltration":false}"#));
        assert!(!json.contains("printableHeightMm"));
        assert!(!json.contains("bedShape"));
    }

    #[test]
    fn unknown_override_keys_survive_a_round_trip() {
        let dir = temp_dir();
        let mut printer = a_printer();
        printer
            .overrides
            .extra
            .insert("printabelHeight".to_string(), serde_json::json!(300));
        let file = PrintersFile { schema_version: 1, printers: vec![printer] };
        write_printers_to(&dir, &file).unwrap();

        let loaded = load_printers_from(&dir).unwrap();
        assert_eq!(
            loaded.printers[0].overrides.extra.get("printabelHeight"),
            Some(&serde_json::json!(300))
        );
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn apply_override_sets_a_field() {
        let overrides = PrinterProfileOverrides::default();
        let updated =
            apply_override(&overrides, "printableHeightMm", Some(serde_json::json!(240.0))).unwrap();
        assert_eq!(updated.printable_height_mm, Some(240.0));
    }

    #[test]
    fn apply_override_with_none_reverts_to_inherited() {
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(240.0);
        let updated = apply_override(&overrides, "printableHeightMm", None).unwrap();
        assert_eq!(updated.printable_height_mm, None);
    }

    #[test]
    fn apply_override_rejects_a_non_overridable_field() {
        let overrides = PrinterProfileOverrides::default();
        let result = apply_override(&overrides, "nozzleDiameterMm", Some(serde_json::json!([0.6])));
        assert!(result.is_err());
    }

    #[test]
    fn apply_override_with_wrong_type_errors_and_leaves_struct_untouched() {
        let overrides = PrinterProfileOverrides::default();
        let result = apply_override(&overrides, "printableHeightMm", Some(serde_json::json!("not a number")));
        assert!(result.is_err());
    }
}
