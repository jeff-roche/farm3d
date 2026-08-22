//! Persistence for `printers.json` — the user-owned counterpart to the
//! read-only catalog.

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
    /// Phase 2's Connection config. Holds a `credentialRef` only — the
    /// secret it names lives in the OS keychain (or the 0600 fallback file),
    /// never here.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub connection: Option<crate::connections::ConnectionConfig>,
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

use crate::catalog::resolve::{resolve_catalog_ref, resolve_printer, ResolvedPrinter};
use crate::catalog::{Catalog, CatalogVariant};
use std::sync::Arc;
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterDraft {
    pub name: String,
    pub catalog_ref: CatalogRef,
    pub group: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PrinterPatch {
    pub name: Option<String>,
    pub group: Option<String>,
    pub notes: Option<String>,
}

fn app_config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

fn find_printer_mut<'a>(
    file: &'a mut PrintersFile,
    id: &str,
) -> Result<&'a mut StoredPrinter, String> {
    file.printers
        .iter_mut()
        .find(|p| p.id == id)
        .ok_or_else(|| format!("no printer with id {id:?}"))
}

/// Guarantees uniqueness within this process's lifetime regardless of clock
/// resolution — the nanosecond timestamp alone wrapped every ~4.3s once
/// truncated to 32 bits, a real risk once rapid, repeated creation (e.g. an
/// "add another like this" shortcut) is something a user actually does.
static ID_SEQUENCE: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);

fn generate_id() -> String {
    use std::sync::atomic::Ordering;
    use std::time::{SystemTime, UNIX_EPOCH};
    let nanos = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_nanos();
    let seq = ID_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    format!("prn-{:x}-{:x}", nanos & 0xFFFF_FFFF, seq)
}

/// Builds a fresh drift baseline from the RAW catalog variant — never from a
/// `ResolvedPrinter`'s merged/effective profile, which may have overrides
/// baked in. Overrides must never leak into `last_known_good`, or a later
/// override revert can produce spurious drift (and "pin" can silently
/// re-instate an override the user just removed).
fn baseline_from(variant: &CatalogVariant, catalog: &Catalog) -> LastKnownGood {
    LastKnownGood {
        profile: PrinterProfile::from(variant),
        catalog_version: catalog.source_tag.clone(),
        resolved_at: now_rfc3339(),
    }
}

#[tauri::command]
pub fn list_printers(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
) -> Result<Vec<ResolvedPrinter>, String> {
    let file = load_printers_from(&app_config_dir(&app)?)?;
    Ok(file.printers.iter().map(|p| resolve_printer(&catalog, p)).collect())
}

#[tauri::command]
pub fn create_printer(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    draft: PrinterDraft,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;

    let (variant, status) = resolve_catalog_ref(&catalog, &draft.catalog_ref);
    let variant = variant.ok_or_else(|| {
        format!("cannot add a printer for an unresolvable catalog reference (status: {status:?})")
    })?;

    let stored = StoredPrinter {
        id: generate_id(),
        name: draft.name,
        catalog_ref: draft.catalog_ref,
        group: draft.group.unwrap_or_default(),
        notes: String::new(),
        overrides: PrinterProfileOverrides::default(),
        last_known_good: Some(baseline_from(variant, &catalog)),
        connection: None,
    };

    let resolved = resolve_printer(&catalog, &stored);
    file.printers.push(stored);
    write_printers_to(&config_dir, &file)?;
    Ok(resolved)
}

#[tauri::command]
pub fn update_printer(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    patch: PrinterPatch,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored = find_printer_mut(&mut file, &id)?;
        if let Some(name) = patch.name {
            stored.name = name;
        }
        if let Some(group) = patch.group {
            stored.group = group;
        }
        if let Some(notes) = patch.notes {
            stored.notes = notes;
        }
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

/// The credential (if any) a deleted printer's connection pointed at, so the
/// caller can remove it from the store. Extracted as pure logic over
/// `PrintersFile` so it's testable without a Tauri `AppHandle` — the command
/// itself stays thin.
fn credential_to_forget(file: &PrintersFile, id: &str) -> Option<String> {
    file.printers
        .iter()
        .find(|p| p.id == id)
        .and_then(|p| p.connection.as_ref())
        .and_then(|c| c.credential_ref.clone())
}

#[tauri::command]
pub async fn delete_printer(
    app: AppHandle,
    manager: tauri::State<'_, Arc<crate::connections::supervisor::ConnectionManager>>,
    id: String,
) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;

    // Stop the supervisor FIRST, before the config it's reconnecting
    // against is removed — otherwise it keeps retrying forever under an id
    // that no longer exists in printers.json.
    manager.stop(&id);

    // Then clean up any stored credential — a printer record is the only
    // place the UI can ever re-enter one, so once it's gone the credential
    // would otherwise be orphaned in the keychain/file store forever.
    if let Some(key) = credential_to_forget(&file, &id) {
        crate::connections::credentials::CredentialStore::detect(config_dir.clone()).delete(&key)?;
    }

    file.printers.retain(|p| p.id != id);
    write_printers_to(&config_dir, &file)
}

#[tauri::command]
pub fn set_printer_override(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    field: String,
    value: Option<serde_json::Value>,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored = find_printer_mut(&mut file, &id)?;
        stored.overrides = apply_override(&stored.overrides, &field, value)?;
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn rebind_printer(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    catalog_ref: CatalogRef,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored = find_printer_mut(&mut file, &id)?;
        stored.catalog_ref = catalog_ref;
        let (variant, status) = resolve_catalog_ref(&catalog, &stored.catalog_ref);
        let variant = variant.ok_or_else(|| {
            format!("cannot rebind to an unresolvable catalog reference (status: {status:?})")
        })?;
        // Rebinding invalidates any stale drift baseline — re-baseline immediately.
        stored.last_known_good = Some(baseline_from(variant, &catalog));
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

/// "Keep my value": pins each drifted field back to its OLD (last-known-good)
/// value as an explicit override.
///
/// Drift is reported for every profile field, but only `OVERRIDABLE_FIELDS` can
/// be pinned — variant-identity fields (`nozzleType`, `gcodeFlavor`,
/// `nozzleDiameterMm`, `supportsMultiFilament`, `suggestedHostType`) are a
/// rebind, not something a user overrides. Those are skipped rather than passed
/// to `apply_override`, which would reject them and abort the entire pin action,
/// making "Keep my value" silently do nothing for any drift set containing one.
fn pin_drift(
    overrides: &PrinterProfileOverrides,
    drift: &[crate::catalog::resolve::ProfileDrift],
) -> Result<PrinterProfileOverrides, String> {
    let mut updated = overrides.clone();
    for d in drift {
        if !OVERRIDABLE_FIELDS.contains(&d.field.as_str()) {
            continue;
        }
        updated = apply_override(&updated, &d.field, Some(d.from.clone()))?;
    }
    Ok(updated)
}

#[tauri::command]
pub fn resolve_profile_drift(
    app: AppHandle,
    catalog: tauri::State<Arc<Catalog>>,
    id: String,
    action: String,
) -> Result<ResolvedPrinter, String> {
    let config_dir = app_config_dir(&app)?;
    let mut file = load_printers_from(&config_dir)?;
    {
        let stored_ref = find_printer_mut(&mut file, &id)?;
        let resolved = resolve_printer(&catalog, stored_ref);
        match action.as_str() {
            "accept" => {
                let (variant, status) = resolve_catalog_ref(&catalog, &stored_ref.catalog_ref);
                let variant = variant.ok_or_else(|| {
                    format!(
                        "cannot accept drift for an unresolvable catalog reference (status: {status:?})"
                    )
                })?;
                stored_ref.last_known_good = Some(baseline_from(variant, &catalog));
            }
            "pin" => {
                stored_ref.overrides = pin_drift(&stored_ref.overrides, &resolved.profile_drift)?;
            }
            other => return Err(format!("unknown drift action: {other:?}")),
        }
    }
    write_printers_to(&config_dir, &file)?;
    Ok(resolve_printer(&catalog, file.printers.iter().find(|p| p.id == id).unwrap()))
}

#[tauri::command]
pub fn open_printers_file(app: AppHandle) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let path = printers_file_path(&config_dir);
    if !path.exists() {
        write_printers_to(
            &config_dir,
            &PrintersFile { schema_version: PRINTERS_SCHEMA_VERSION, printers: vec![] },
        )?;
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

/// A dependency-free RFC3339 UTC timestamp for `LastKnownGood.resolved_at` —
/// avoids adding a date/time crate for one field. Runs at runtime (unlike
/// gen-catalog's shell-out to `date`, which is fine for a dev-only tool but
/// wouldn't be portable inside the shipped app).
pub fn now_rfc3339() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};
    let secs = SystemTime::now().duration_since(UNIX_EPOCH).unwrap().as_secs();
    rfc3339_from_unix_seconds(secs)
}

fn rfc3339_from_unix_seconds(secs: u64) -> String {
    let days = (secs / 86400) as i64;
    let rem = secs % 86400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let (y, mo, d) = civil_from_days(days);
    format!("{y:04}-{mo:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// Howard Hinnant's days-from-civil algorithm, inverted (public domain) —
/// converts a day count since the Unix epoch into a (year, month, day) civil
/// date without a date/time crate dependency.
fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

#[cfg(test)]
mod rfc3339_tests {
    use super::*;

    #[test]
    fn epoch_zero_is_the_unix_epoch_date() {
        assert_eq!(rfc3339_from_unix_seconds(0), "1970-01-01T00:00:00Z");
    }

    #[test]
    fn a_known_timestamp_round_trips_correctly() {
        // 946684800 is the well-known Unix timestamp for 2000-01-01T00:00:00Z.
        assert_eq!(rfc3339_from_unix_seconds(946_684_800), "2000-01-01T00:00:00Z");
    }
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

    fn a_variant() -> CatalogVariant {
        CatalogVariant {
            variant: "Test Printer 0.4 nozzle".to_string(),
            printer_variant: "0.4".to_string(),
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 256.0,
            bed_exclude_areas: vec![],
            default_bed_type: "4".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: true,
            supports_air_filtration: true,
            supports_multi_filament: false,
            suggested_host_type: None,
        }
    }

    fn a_catalog_for_baseline() -> Catalog {
        Catalog {
            generated_at: "2026-08-20T00:00:00Z".to_string(),
            source_tag: "v2.4.2".to_string(),
            notice: "test".to_string(),
            models: vec![],
        }
    }

    #[test]
    fn baseline_from_uses_the_raw_catalog_profile_not_a_merged_one() {
        // This is the regression test for the drift-acceptance bug: baseline_from
        // must always produce PrinterProfile::from(variant) — the raw catalog
        // value — never a profile with override values mixed in. If a caller
        // passed a merged/effective profile here instead, an active override
        // would get baked into the new last_known_good baseline, causing bogus
        // drift alerts (and silent override re-instatement via "pin") once the
        // user reverts that override later.
        let variant = a_variant();
        let catalog = a_catalog_for_baseline();
        let baseline = baseline_from(&variant, &catalog);
        assert_eq!(baseline.profile, PrinterProfile::from(&variant));
        assert_eq!(baseline.catalog_version, "v2.4.2");
    }

    /// Regression test for the "pin" action hard-erroring: drift routinely
    /// includes variant-identity fields that `apply_override` rejects, which
    /// used to abort the whole action, so "Keep my value" did nothing at all.
    #[test]
    fn pinning_drift_skips_non_overridable_fields_instead_of_erroring() {
        use crate::catalog::CatalogModel;

        // Catalog holds the current variant; last_known_good holds an older
        // snapshot that differs in BOTH an overridable field (printableHeightMm)
        // and a non-overridable one (nozzleType).
        let current = a_variant();
        let catalog = Catalog {
            generated_at: "2026-08-20T00:00:00Z".to_string(),
            source_tag: "v2.4.2".to_string(),
            notice: "test".to_string(),
            models: vec![CatalogModel {
                model_id: "TestVendor-TP".to_string(),
                vendor: "TestVendor".to_string(),
                model: "Test Printer".to_string(),
                variants: vec![current.clone()],
            }],
        };

        let mut stale = a_variant();
        stale.printable_height_mm = 200.0;
        stale.nozzle_type = "brass".to_string();

        let mut printer = a_printer();
        printer.last_known_good = Some(LastKnownGood {
            profile: PrinterProfile::from(&stale),
            catalog_version: "v2.4.1".to_string(),
            resolved_at: "2026-08-01T00:00:00Z".to_string(),
        });

        let resolved = resolve_printer(&catalog, &printer);
        let drifted: Vec<_> = resolved.profile_drift.iter().map(|d| d.field.as_str()).collect();
        assert!(drifted.contains(&"printableHeightMm"), "expected height drift, got {drifted:?}");
        assert!(drifted.contains(&"nozzleType"), "expected nozzleType drift, got {drifted:?}");

        let pinned = pin_drift(&printer.overrides, &resolved.profile_drift)
            .expect("pinning must not fail just because drift includes a non-overridable field");

        // The overridable field is pinned to its OLD value...
        assert_eq!(pinned.printable_height_mm, Some(200.0));
        // ...and the non-overridable one is simply not recorded anywhere.
        let json = serde_json::to_value(&pinned).unwrap();
        assert!(json.get("nozzleType").is_none(), "nozzleType must not be pinned");
        assert!(pinned.extra.is_empty(), "no stray override keys: {:?}", pinned.extra);
    }

    #[test]
    fn printers_file_without_a_connection_key_still_loads() {
        // Every printer written by phase 1 looks like this. Typing the slot must
        // not orphan them.
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(
            printers_file_path(&dir),
            r#"{"schemaVersion":1,"printers":[{"id":"prn-1","name":"Bay 1"}]}"#,
        )
        .unwrap();
        let loaded = load_printers_from(&dir).unwrap();
        assert_eq!(loaded.printers.len(), 1);
        assert_eq!(loaded.printers[0].connection, None);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn a_stored_connection_round_trips_and_never_carries_a_secret() {
        use crate::connections::{ConnectionConfig, DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND};

        let dir = temp_dir();
        let mut file = PrintersFile {
            schema_version: 1,
            printers: vec![StoredPrinter {
                id: "prn-1".to_string(),
                name: "Bay 1".to_string(),
                ..Default::default()
            }],
        };
        file.printers[0].connection = Some(ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: DEFAULT_MOONRAKER_PORT,
            use_tls: false,
            credential_ref: Some("farm3d/printer/prn-1/apikey".to_string()),
        });
        write_printers_to(&dir, &file).unwrap();

        let raw = fs::read_to_string(printers_file_path(&dir)).unwrap();
        // The reference is stored; the secret it points at never is.
        assert!(raw.contains("credentialRef"));
        assert!(!raw.contains("apiKey\""));
        assert_eq!(load_printers_from(&dir).unwrap(), file);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn credential_to_forget_finds_the_deleted_printers_credential_ref() {
        use crate::connections::{ConnectionConfig, DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND};

        let mut printer = a_printer();
        printer.connection = Some(ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: DEFAULT_MOONRAKER_PORT,
            use_tls: false,
            credential_ref: Some("farm3d/printer/prn-1/apikey".to_string()),
        });
        let file = PrintersFile { schema_version: 1, printers: vec![printer] };

        assert_eq!(
            credential_to_forget(&file, "prn-1"),
            Some("farm3d/printer/prn-1/apikey".to_string())
        );
    }

    #[test]
    fn credential_to_forget_is_none_when_the_printer_has_no_connection() {
        let file = PrintersFile { schema_version: 1, printers: vec![a_printer()] };
        assert_eq!(credential_to_forget(&file, "prn-1"), None);
        // Also None for an id that isn't even in the file, rather than panicking.
        assert_eq!(credential_to_forget(&file, "prn-ghost"), None);
    }

    #[test]
    fn deleting_a_printer_with_a_credential_removes_it_from_a_file_backed_store() {
        // Regression test: before this fix, `delete_printer` left the
        // supervisor running and the credential orphaned in the store. This
        // exercises the same sequence `delete_printer` performs (identify
        // the credential, delete it from the store, then drop the printer
        // from the file) without needing a Tauri AppHandle.
        use crate::connections::credentials::CredentialStore;
        use crate::connections::{ConnectionConfig, DEFAULT_MOONRAKER_PORT, MOONRAKER_KIND};

        let dir = temp_dir();
        let key = "farm3d/printer/prn-1/apikey".to_string();

        let mut printer = a_printer();
        printer.connection = Some(ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "voron.local".to_string(),
            port: DEFAULT_MOONRAKER_PORT,
            use_tls: false,
            credential_ref: Some(key.clone()),
        });
        let mut file = PrintersFile { schema_version: 1, printers: vec![printer] };
        write_printers_to(&dir, &file).unwrap();

        let store = CredentialStore::file_backed(dir.clone());
        store.set(&key, "s3cret").unwrap();
        assert_eq!(store.get(&key).unwrap(), Some("s3cret".to_string()));

        // What `delete_printer` does, minus the supervisor stop (which needs
        // a live ConnectionManager/AppHandle).
        if let Some(key) = credential_to_forget(&file, "prn-1") {
            store.delete(&key).unwrap();
        }
        file.printers.retain(|p| p.id != "prn-1");
        write_printers_to(&dir, &file).unwrap();

        assert_eq!(store.get(&key).unwrap(), None, "credential must not survive printer deletion");
        assert!(load_printers_from(&dir).unwrap().printers.is_empty());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn generate_id_never_repeats_under_rapid_creation() {
        // Regression test for a real collision window: the old
        // nanosecond-truncated-to-32-bits id wrapped roughly every ~4.3s, so
        // creating several printers in a tight loop (exactly what an "add
        // another like this" shortcut does) could produce duplicate ids.
        let ids: std::collections::HashSet<String> = (0..1000).map(|_| generate_id()).collect();
        assert_eq!(ids.len(), 1000, "generate_id produced a duplicate under rapid, repeated calls");
    }
}
