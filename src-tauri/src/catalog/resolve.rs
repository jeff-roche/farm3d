use crate::catalog::{BedShape, Catalog, CatalogVariant, PrinterProfile};
use crate::printers::{CatalogRef, PrinterProfileOverrides, StoredPrinter};
use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum CatalogStatus {
    Ok,
    Rematched,
    VariantMissing,
    ModelMissing,
    VendorMissing,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ProfileDrift {
    pub field: String,
    pub from: serde_json::Value,
    pub to: serde_json::Value,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct ResolvedPrinter {
    pub id: String,
    pub name: String,
    pub group: String,
    pub notes: String,
    pub catalog_ref: CatalogRef,
    pub catalog_status: CatalogStatus,
    pub model_label: String,
    pub variant_label: String,
    pub profile: PrinterProfile,
    pub overridden_fields: Vec<String>,
    pub inherited: serde_json::Map<String, serde_json::Value>,
    pub profile_drift: Vec<ProfileDrift>,
    pub unknown_override_keys: Vec<String>,
    pub connection: Option<serde_json::Value>,
}

pub fn resolve_catalog_ref<'a>(
    catalog: &'a Catalog,
    r: &CatalogRef,
) -> (Option<&'a CatalogVariant>, CatalogStatus) {
    if let Some(model) = catalog
        .models
        .iter()
        .find(|m| m.vendor == r.vendor && m.model_id == r.model_id)
    {
        if let Some(variant) = model.variants.iter().find(|v| v.variant == r.variant) {
            return (Some(variant), CatalogStatus::Ok);
        }
        if let Some(variant) = model
            .variants
            .iter()
            .find(|v| v.printer_variant == r.printer_variant)
        {
            return (Some(variant), CatalogStatus::Rematched);
        }
        return (None, CatalogStatus::VariantMissing);
    }

    if let Some(model) = catalog
        .models
        .iter()
        .find(|m| m.vendor == r.vendor && normalize(&m.model) == normalize(&r.model))
    {
        if let Some(variant) = model
            .variants
            .iter()
            .find(|v| v.printer_variant == r.printer_variant)
        {
            return (Some(variant), CatalogStatus::Rematched);
        }
        return (None, CatalogStatus::VariantMissing);
    }

    if catalog.models.iter().any(|m| m.vendor == r.vendor) {
        return (None, CatalogStatus::ModelMissing);
    }
    (None, CatalogStatus::VendorMissing)
}

fn normalize(s: &str) -> String {
    s.to_lowercase().chars().filter(|c| c.is_alphanumeric()).collect()
}

pub fn merge_profile(
    base: &PrinterProfile,
    overrides: &PrinterProfileOverrides,
) -> (PrinterProfile, Vec<&'static str>) {
    let mut effective = base.clone();
    let mut overridden = Vec::new();

    if let Some(v) = &overrides.bed_shape {
        effective.bed_shape = v.clone();
        overridden.push("bedShape");
    }
    if let Some(v) = overrides.printable_height_mm {
        effective.printable_height_mm = v;
        overridden.push("printableHeightMm");
    }
    if let Some(v) = &overrides.bed_exclude_areas {
        effective.bed_exclude_areas = v.clone();
        overridden.push("bedExcludeAreas");
    }
    if let Some(v) = &overrides.default_bed_type {
        effective.default_bed_type = v.clone();
        overridden.push("defaultBedType");
    }
    if let Some(v) = overrides.has_auxiliary_fan {
        effective.has_auxiliary_fan = v;
        overridden.push("hasAuxiliaryFan");
    }
    if let Some(v) = overrides.supports_air_filtration {
        effective.supports_air_filtration = v;
        overridden.push("supportsAirFiltration");
    }

    (effective, overridden)
}

pub fn resolve_printer(catalog: &Catalog, stored: &StoredPrinter) -> ResolvedPrinter {
    let (variant, status) = resolve_catalog_ref(catalog, &stored.catalog_ref);

    let (base_profile, model_label, variant_label) = match variant {
        Some(v) => {
            let model_label = catalog
                .models
                .iter()
                .find(|m| m.variants.iter().any(|mv| mv.variant == v.variant))
                .map(|m| m.model.clone())
                .unwrap_or_else(|| stored.catalog_ref.model.clone());
            (PrinterProfile::from(v), model_label, v.variant.clone())
        }
        None => {
            let fallback = stored
                .last_known_good
                .as_ref()
                .map(|lkg| lkg.profile.clone())
                .unwrap_or_else(empty_profile);
            (
                fallback,
                stored.catalog_ref.model.clone(),
                stored.catalog_ref.variant.clone(),
            )
        }
    };

    let (profile, overridden) = merge_profile(&base_profile, &stored.overrides);
    let inherited = inherited_subset(&base_profile, &overridden);

    let profile_drift = stored
        .last_known_good
        .as_ref()
        .map(|lkg| diff_profile(&lkg.profile, &base_profile, &overridden))
        .unwrap_or_default();

    ResolvedPrinter {
        id: stored.id.clone(),
        name: stored.name.clone(),
        group: stored.group.clone(),
        notes: stored.notes.clone(),
        catalog_ref: stored.catalog_ref.clone(),
        catalog_status: status,
        model_label,
        variant_label,
        profile,
        overridden_fields: overridden.iter().map(|s| s.to_string()).collect(),
        inherited,
        profile_drift,
        unknown_override_keys: stored.overrides.extra.keys().cloned().collect(),
        connection: stored.connection.clone(),
    }
}

fn empty_profile() -> PrinterProfile {
    PrinterProfile {
        bed_shape: BedShape::Rectangular {
            width_mm: 0.0,
            depth_mm: 0.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        },
        printable_height_mm: 0.0,
        bed_exclude_areas: vec![],
        default_bed_type: String::new(),
        nozzle_diameter_mm: vec![],
        nozzle_type: String::new(),
        gcode_flavor: String::new(),
        has_auxiliary_fan: false,
        supports_air_filtration: false,
        supports_multi_filament: false,
        suggested_host_type: None,
    }
}

fn inherited_subset(
    base: &PrinterProfile,
    overridden_fields: &[&'static str],
) -> serde_json::Map<String, serde_json::Value> {
    let mut subset = serde_json::Map::new();
    if let serde_json::Value::Object(map) = serde_json::to_value(base).unwrap() {
        for field in overridden_fields {
            if let Some(v) = map.get(*field) {
                subset.insert(field.to_string(), v.clone());
            }
        }
    }
    subset
}

/// Compares the catalog's current values against the last user-confirmed
/// snapshot for fields the instance does NOT override — an overridden field
/// can't drift, since the user's value wins regardless of what the catalog says.
fn diff_profile(
    last: &PrinterProfile,
    current: &PrinterProfile,
    overridden: &[&'static str],
) -> Vec<ProfileDrift> {
    let last_json = serde_json::to_value(last).unwrap();
    let current_json = serde_json::to_value(current).unwrap();
    let (serde_json::Value::Object(last_map), serde_json::Value::Object(current_map)) =
        (&last_json, &current_json)
    else {
        return vec![];
    };
    let mut drift = vec![];
    for (key, current_value) in current_map {
        if overridden.contains(&key.as_str()) {
            continue;
        }
        if let Some(last_value) = last_map.get(key) {
            if last_value != current_value {
                drift.push(ProfileDrift {
                    field: key.clone(),
                    from: last_value.clone(),
                    to: current_value.clone(),
                });
            }
        }
    }
    drift
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{CatalogModel, PointMm};
    use crate::printers::LastKnownGood;

    fn variant(name: &str, printer_variant: &str, height: f64) -> CatalogVariant {
        CatalogVariant {
            variant: name.to_string(),
            printer_variant: printer_variant.to_string(),
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0, depth_mm: 256.0, origin_x_mm: 0.0, origin_y_mm: 0.0,
            },
            printable_height_mm: height,
            bed_exclude_areas: vec![],
            default_bed_type: "4".to_string(),
            nozzle_diameter_mm: vec![printer_variant.parse().unwrap()],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: true,
            supports_air_filtration: true,
            supports_multi_filament: true,
            suggested_host_type: Some("elegoolink".to_string()),
        }
    }

    fn a_catalog() -> Catalog {
        Catalog {
            generated_at: "2026-08-20T00:00:00Z".to_string(),
            source_tag: "v2.4.2".to_string(),
            notice: "test".to_string(),
            models: vec![CatalogModel {
                model_id: "Elegoo-CC".to_string(),
                vendor: "Elegoo".to_string(),
                model: "Elegoo Centauri Carbon".to_string(),
                variants: vec![
                    variant("Elegoo Centauri Carbon 0.4 nozzle", "0.4", 256.0),
                    variant("Elegoo Centauri Carbon 0.2 nozzle", "0.2", 256.0),
                ],
            }],
        }
    }

    fn a_ref() -> CatalogRef {
        CatalogRef {
            vendor: "Elegoo".to_string(),
            model: "Elegoo Centauri Carbon".to_string(),
            variant: "Elegoo Centauri Carbon 0.4 nozzle".to_string(),
            model_id: "Elegoo-CC".to_string(),
            printer_variant: "0.4".to_string(),
        }
    }

    #[test]
    fn exact_match_resolves_ok() {
        let catalog = a_catalog();
        let (v, status) = resolve_catalog_ref(&catalog, &a_ref());
        assert_eq!(status, CatalogStatus::Ok);
        assert_eq!(v.unwrap().variant, "Elegoo Centauri Carbon 0.4 nozzle");
    }

    #[test]
    fn renamed_variant_within_the_same_model_rematches() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.variant = "Elegoo Centauri Carbon 0.4 nozzle (old name)".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::Rematched);
        assert_eq!(v.unwrap().printer_variant, "0.4");
    }

    #[test]
    fn missing_variant_in_an_existing_model_is_variant_missing() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.variant = "does not exist".to_string();
        r.printer_variant = "0.8".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::VariantMissing);
        assert!(v.is_none());
    }

    #[test]
    fn missing_model_in_an_existing_vendor_is_model_missing() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.model_id = "Elegoo-DoesNotExist".to_string();
        r.model = "Elegoo Nonexistent".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::ModelMissing);
        assert!(v.is_none());
    }

    #[test]
    fn missing_vendor_is_vendor_missing() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.vendor = "NoSuchVendor".to_string();
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::VendorMissing);
        assert!(v.is_none());
    }

    #[test]
    fn never_rematches_across_vendors() {
        let mut catalog = a_catalog();
        catalog.models.push(CatalogModel {
            model_id: "Other-CC".to_string(),
            vendor: "OtherVendor".to_string(),
            model: "Other Centauri Carbon".to_string(),
            variants: vec![variant("Other Centauri Carbon 0.4 nozzle", "0.4", 256.0)],
        });
        let mut r = a_ref();
        r.model_id = "does-not-exist-anywhere".to_string();
        r.model = "Does Not Exist Anywhere".to_string();
        // Even though OtherVendor has a same-printerVariant match, vendor differs.
        let (v, status) = resolve_catalog_ref(&catalog, &r);
        assert_eq!(status, CatalogStatus::ModelMissing);
        assert!(v.is_none());
    }

    #[test]
    fn merge_profile_with_no_overrides_returns_the_base_unchanged() {
        let base = PrinterProfile::from(&variant("v", "0.4", 256.0));
        let (effective, overridden) = merge_profile(&base, &PrinterProfileOverrides::default());
        assert_eq!(effective, base);
        assert!(overridden.is_empty());
    }

    #[test]
    fn merge_profile_applies_a_single_override() {
        let base = PrinterProfile::from(&variant("v", "0.4", 256.0));
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(240.0);
        let (effective, overridden) = merge_profile(&base, &overrides);
        assert_eq!(effective.printable_height_mm, 240.0);
        assert_eq!(overridden, vec!["printableHeightMm"]);
    }

    #[test]
    fn an_override_equal_to_the_inherited_value_still_counts_as_overridden() {
        let base = PrinterProfile::from(&variant("v", "0.4", 256.0));
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(256.0); // same as base
        let (_, overridden) = merge_profile(&base, &overrides);
        assert_eq!(overridden, vec!["printableHeightMm"]);
    }

    #[test]
    fn resolve_printer_with_no_overrides_reports_the_catalog_profile_directly() {
        let catalog = a_catalog();
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: a_ref(),
            group: String::new(),
            notes: String::new(),
            overrides: PrinterProfileOverrides::default(),
            last_known_good: None,
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        assert_eq!(resolved.catalog_status, CatalogStatus::Ok);
        assert_eq!(resolved.profile.printable_height_mm, 256.0);
        assert!(resolved.overridden_fields.is_empty());
        assert_eq!(resolved.model_label, "Elegoo Centauri Carbon");
    }

    #[test]
    fn resolve_printer_with_a_dangling_ref_falls_back_to_last_known_good() {
        let catalog = a_catalog();
        let mut r = a_ref();
        r.model_id = "gone".to_string();
        r.model = "Gone Printer".to_string();
        let fallback_profile = PrinterProfile::from(&variant("v", "0.4", 300.0));
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: r,
            group: String::new(),
            notes: String::new(),
            overrides: PrinterProfileOverrides::default(),
            last_known_good: Some(LastKnownGood {
                profile: fallback_profile.clone(),
                catalog_version: "02.04.00.06".to_string(),
                resolved_at: "2026-08-01T00:00:00Z".to_string(),
            }),
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        assert_eq!(resolved.catalog_status, CatalogStatus::ModelMissing);
        assert_eq!(resolved.profile, fallback_profile);
    }

    #[test]
    fn drift_is_reported_only_for_non_overridden_fields() {
        let catalog = a_catalog();
        let mut overrides = PrinterProfileOverrides::default();
        overrides.printable_height_mm = Some(999.0); // user override, must never "drift"
        let stale_profile = PrinterProfile::from(&variant("v", "0.4", 100.0)); // stale height AND stale bed type
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: a_ref(),
            group: String::new(),
            notes: String::new(),
            overrides,
            last_known_good: Some(LastKnownGood {
                profile: stale_profile,
                catalog_version: "02.04.00.05".to_string(),
                resolved_at: "2026-08-01T00:00:00Z".to_string(),
            }),
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        // printableHeightMm is overridden, so it must NOT appear as drift even
        // though the stale snapshot's value (100) differs from the catalog's (256).
        assert!(!resolved.profile_drift.iter().any(|d| d.field == "printableHeightMm"));
    }

    #[test]
    fn unknown_override_keys_are_surfaced() {
        let catalog = a_catalog();
        let mut overrides = PrinterProfileOverrides::default();
        overrides.extra.insert("printabelHeight".to_string(), serde_json::json!(300));
        let stored = StoredPrinter {
            id: "prn-1".to_string(),
            name: "My Centauri".to_string(),
            catalog_ref: a_ref(),
            group: String::new(),
            notes: String::new(),
            overrides,
            last_known_good: None,
            connection: None,
        };
        let resolved = resolve_printer(&catalog, &stored);
        assert_eq!(resolved.unknown_override_keys, vec!["printabelHeight".to_string()]);
    }
}
