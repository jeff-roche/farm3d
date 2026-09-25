//! D4: the two tables that map farm3d settings to OrcaSlicer keys, and
//! [`apply_overrides_and_controls`], which turns the flat presets, a
//! target's Printer Profile overrides, and the `SliceControls` into the
//! three flat JSON documents one slice loads.
//!
//! Overrides go into the machine preset and controls into the process
//! preset. The filament preset is only checked for compatibility (D3).
//! Before anything is returned, every key written must be known to the
//! preset source (D4's key check).

use serde_json::{Map, Value};

use crate::catalog::{BedShape, PointMm, PrinterProfile};
use crate::contracts::command::CommandError;

use super::presets::{PresetIndex, PresetKind};
use super::{BrimType, InfillPattern, SliceControls, SupportMode};

/// How a `PrinterProfile` field reaches OrcaSlicer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProfileFieldMapping {
    /// Written to this machine-preset key.
    Mapped(&'static str),
    /// Not a slicing input; an override of it changes nothing.
    NotApplicable,
}

/// D4's profile-override table: every `PrinterProfile` field, by its wire
/// name. A field missing here fails `every_profile_field_is_mapped_or_not_applicable`,
/// and an override of a field missing here is `UNMAPPED_PROFILE_OVERRIDE`.
///
/// `defaultBedType` is written as stored: the catalog copies OrcaSlicer's
/// `default_bed_type` string verbatim (`""`, `"4"`, or `"Textured PEI
/// Plate"` in v2.4.2), and v2.4.2 declares the key a string that "supports
/// both numeric and string format" (`PrintConfig.cpp`).
pub const PROFILE_FIELD_MAPPINGS: [(&str, ProfileFieldMapping); 11] = [
    ("bedShape", ProfileFieldMapping::Mapped("printable_area")),
    (
        "printableHeightMm",
        ProfileFieldMapping::Mapped("printable_height"),
    ),
    (
        "bedExcludeAreas",
        ProfileFieldMapping::Mapped("bed_exclude_area"),
    ),
    (
        "nozzleDiameterMm",
        ProfileFieldMapping::Mapped("nozzle_diameter"),
    ),
    ("nozzleType", ProfileFieldMapping::Mapped("nozzle_type")),
    ("gcodeFlavor", ProfileFieldMapping::Mapped("gcode_flavor")),
    (
        "defaultBedType",
        ProfileFieldMapping::Mapped("default_bed_type"),
    ),
    ("hasAuxiliaryFan", ProfileFieldMapping::NotApplicable),
    ("supportsAirFiltration", ProfileFieldMapping::NotApplicable),
    ("supportsMultiFilament", ProfileFieldMapping::NotApplicable),
    ("suggestedHostType", ProfileFieldMapping::NotApplicable),
];

/// D4's control table: each `SliceControls` field (wire name) and the
/// process-preset keys it writes.
pub const CONTROL_MAPPINGS: [(&str, &[&str]); 11] = [
    ("layerHeightMm", &["layer_height"]),
    ("wallLoops", &["wall_loops"]),
    ("topShellLayers", &["top_shell_layers"]),
    ("bottomShellLayers", &["bottom_shell_layers"]),
    ("infillDensityPercent", &["sparse_infill_density"]),
    ("infillPattern", &["sparse_infill_pattern"]),
    ("supports", &["enable_support", "support_type"]),
    ("supportThresholdAngleDeg", &["support_threshold_angle"]),
    ("brimType", &["brim_type"]),
    ("brimWidthMm", &["brim_width"]),
    ("skirtLoops", &["skirt_loops"]),
];

/// Every OrcaSlicer key either table can write, with its preset kind.
pub fn mapped_keys() -> Vec<(PresetKind, &'static str)> {
    let profile = PROFILE_FIELD_MAPPINGS
        .iter()
        .filter_map(|(_, mapping)| match mapping {
            ProfileFieldMapping::Mapped(key) => Some((PresetKind::Machine, *key)),
            ProfileFieldMapping::NotApplicable => None,
        });
    let controls = CONTROL_MAPPINGS
        .iter()
        .flat_map(|(_, keys)| keys.iter().map(|key| (PresetKind::Process, *key)));
    profile.chain(controls).collect()
}

fn profile_field_mapping(field: &str) -> Option<ProfileFieldMapping> {
    PROFILE_FIELD_MAPPINGS
        .iter()
        .find(|(name, _)| *name == field)
        .map(|(_, mapping)| *mapping)
}

/// A number as OrcaSlicer writes it: `256`, `0.4`, `0.12`.
fn number(value: f64) -> String {
    format!("{value}")
}

fn point(x: f64, y: f64) -> Value {
    Value::String(format!("{}x{}", number(x), number(y)))
}

fn points(points: &[PointMm]) -> Value {
    Value::Array(points.iter().map(|p| point(p.x_mm, p.y_mm)).collect())
}

fn text(value: &str) -> Value {
    Value::String(value.to_string())
}

/// P5 slices with exactly one nozzle (D4); anything else is `VALIDATION`
/// at `target`.
fn single_nozzle(profile: &PrinterProfile) -> Result<f64, CommandError> {
    match profile.nozzle_diameter_mm.as_slice() {
        [diameter] => Ok(*diameter),
        _ => Err(CommandError::validation_at(
            "target",
            "farm3d slices for printers with exactly one nozzle.",
        )),
    }
}

/// D4's encoding of one mapped field's value.
fn encode_profile_field(field: &str, profile: &PrinterProfile) -> Result<Value, CommandError> {
    Ok(match field {
        "bedShape" => match &profile.bed_shape {
            BedShape::Rectangular {
                width_mm,
                depth_mm,
                origin_x_mm: x,
                origin_y_mm: y,
            } => Value::Array(vec![
                point(*x, *y),
                point(x + width_mm, *y),
                point(x + width_mm, y + depth_mm),
                point(*x, y + depth_mm),
            ]),
            BedShape::Polygon { points: corners } => points(corners),
        },
        "printableHeightMm" => text(&number(profile.printable_height_mm)),
        "bedExcludeAreas" => points(&profile.bed_exclude_areas),
        "nozzleDiameterMm" => Value::Array(vec![text(&number(single_nozzle(profile)?))]),
        "nozzleType" => text(&profile.nozzle_type),
        "gcodeFlavor" => text(&profile.gcode_flavor),
        "defaultBedType" => text(&profile.default_bed_type),
        _ => return Err(CommandError::unmapped_profile_override(field)),
    })
}

/// Applies a target's overridden `PrinterProfile` fields to the flat
/// machine preset, using the values in `profile` (the effective profile).
/// A not-applicable field is skipped; a field or key with no row in
/// [`PROFILE_FIELD_MAPPINGS`] is `UNMAPPED_PROFILE_OVERRIDE`, never
/// silently dropped. Returns the keys written.
pub fn apply_profile_overrides(
    machine: &mut Map<String, Value>,
    profile: &PrinterProfile,
    overridden_fields: &[String],
    unknown_override_keys: &[String],
) -> Result<Vec<&'static str>, CommandError> {
    if let Some(key) = unknown_override_keys.first() {
        return Err(CommandError::unmapped_profile_override(key));
    }
    let mut written = Vec::new();
    for field in overridden_fields {
        match profile_field_mapping(field) {
            Some(ProfileFieldMapping::Mapped(key)) => {
                machine.insert(key.to_string(), encode_profile_field(field, profile)?);
                written.push(key);
            }
            Some(ProfileFieldMapping::NotApplicable) => {}
            None => return Err(CommandError::unmapped_profile_override(field)),
        }
    }
    Ok(written)
}

fn infill_pattern(pattern: InfillPattern) -> &'static str {
    match pattern {
        InfillPattern::Rectilinear => "rectilinear",
        InfillPattern::Grid => "grid",
        InfillPattern::Line => "line",
        InfillPattern::Cubic => "cubic",
        InfillPattern::Gyroid => "gyroid",
        InfillPattern::Honeycomb => "honeycomb",
        InfillPattern::Lightning => "lightning",
    }
}

fn brim_type(brim: BrimType) -> &'static str {
    match brim {
        BrimType::NoBrim => "no_brim",
        BrimType::OuterOnly => "outer_only",
        BrimType::AutoBrim => "auto_brim",
    }
}

/// Writes every set control to the flat process preset; an unset control
/// keeps the preset's value. Returns the keys written.
pub fn apply_controls(
    process: &mut Map<String, Value>,
    controls: &SliceControls,
) -> Vec<&'static str> {
    let mut written = Vec::new();
    let mut set = |key: &'static str, value: String| {
        process.insert(key.to_string(), Value::String(value));
        written.push(key);
    };
    if let Some(value) = controls.layer_height_mm {
        set("layer_height", number(value));
    }
    if let Some(value) = controls.wall_loops {
        set("wall_loops", value.to_string());
    }
    if let Some(value) = controls.top_shell_layers {
        set("top_shell_layers", value.to_string());
    }
    if let Some(value) = controls.bottom_shell_layers {
        set("bottom_shell_layers", value.to_string());
    }
    if let Some(value) = controls.infill_density_percent {
        set("sparse_infill_density", format!("{}%", number(value)));
    }
    if let Some(value) = controls.infill_pattern {
        set("sparse_infill_pattern", infill_pattern(value).to_string());
    }
    match controls.supports {
        Some(SupportMode::Off) => set("enable_support", "0".to_string()),
        Some(SupportMode::NormalAuto) => {
            set("enable_support", "1".to_string());
            set("support_type", "normal(auto)".to_string());
        }
        Some(SupportMode::TreeAuto) => {
            set("enable_support", "1".to_string());
            set("support_type", "tree(auto)".to_string());
        }
        None => {}
    }
    if let Some(value) = controls.support_threshold_angle_deg {
        set("support_threshold_angle", number(value));
    }
    if let Some(value) = controls.brim_type {
        set("brim_type", brim_type(value).to_string());
    }
    if let Some(value) = controls.brim_width_mm {
        set("brim_width", number(value));
    }
    if let Some(value) = controls.skirt_loops {
        set("skirt_loops", value.to_string());
    }
    written
}

/// D4's allowed values. `nozzle_diameter_mm` bounds the layer height (at
/// most 80% of it). A value out of range is `VALIDATION` at
/// `controls.<field>`.
pub fn validate_controls(
    controls: &SliceControls,
    nozzle_diameter_mm: f64,
) -> Result<(), CommandError> {
    fn check(ok: bool, field: &str, message: String) -> Result<(), CommandError> {
        if ok {
            Ok(())
        } else {
            Err(CommandError::validation_at(
                format!("controls.{field}"),
                message,
            ))
        }
    }
    fn within(value: f64, min: f64, max: f64) -> bool {
        value.is_finite() && value >= min && value <= max
    }
    if let Some(value) = controls.layer_height_mm {
        let max = nozzle_diameter_mm * 0.8;
        check(
            within(value, 0.05, max),
            "layerHeightMm",
            format!("Layer height must be from 0.05 mm to {} mm.", number(max)),
        )?;
    }
    if let Some(value) = controls.wall_loops {
        check(
            (1..=20).contains(&value),
            "wallLoops",
            "Walls must be from 1 to 20.".to_string(),
        )?;
    }
    for (value, field) in [
        (controls.top_shell_layers, "topShellLayers"),
        (controls.bottom_shell_layers, "bottomShellLayers"),
    ] {
        if let Some(value) = value {
            check(
                value <= 50,
                field,
                "Shells must be from 0 to 50.".to_string(),
            )?;
        }
    }
    if let Some(value) = controls.infill_density_percent {
        check(
            within(value, 0.0, 100.0),
            "infillDensityPercent",
            "Infill density must be from 0 to 100%.".to_string(),
        )?;
    }
    if let Some(value) = controls.support_threshold_angle_deg {
        check(
            within(value, 0.0, 90.0),
            "supportThresholdAngleDeg",
            "The support overhang angle must be from 0 to 90°.".to_string(),
        )?;
    }
    if let Some(value) = controls.brim_width_mm {
        check(
            within(value, 0.0, 20.0),
            "brimWidthMm",
            "Brim width must be from 0 to 20 mm.".to_string(),
        )?;
    }
    if let Some(value) = controls.skirt_loops {
        check(
            value <= 10,
            "skirtLoops",
            "Skirt loops must be from 0 to 10.".to_string(),
        )?;
    }
    Ok(())
}

/// What one slice needs from D3 and D4, by name.
#[derive(Clone, Copy, Debug)]
pub struct SliceSettingsInput<'a> {
    pub machine_preset: &'a str,
    pub process_preset: &'a str,
    pub filament_preset: &'a str,
    /// The target's effective profile.
    pub profile: &'a PrinterProfile,
    /// The target's overridden `PrinterProfile` fields.
    pub overridden_fields: &'a [String],
    /// Override keys farm3d doesn't know.
    pub unknown_override_keys: &'a [String],
    pub controls: &'a SliceControls,
}

/// The three flat presets one slice loads (D8's `input/machine.json`,
/// `process.json`, and `filament.json`), plus the keys farm3d wrote.
#[derive(Clone, Debug, PartialEq)]
pub struct SlicePresetDocuments {
    pub machine: Value,
    pub process: Value,
    pub filament: Value,
    /// Machine-preset keys written from Printer Profile overrides.
    pub override_keys: Vec<&'static str>,
    /// Process-preset keys written from the controls.
    pub control_keys: Vec<&'static str>,
}

/// D3 and D4 for one slice: checks the controls against D4's ranges (for
/// the profile's single nozzle), flattens the three presets, checks the
/// process and filament are offered for the machine, applies the overrides
/// and the controls, and checks every written key is known to the preset
/// source (`UNSUPPORTED_SETTING_FOR_RUNTIME`). An out-of-range control never
/// reaches `process.json`. The caller writes the documents to disk.
pub fn apply_overrides_and_controls(
    index: &PresetIndex,
    input: SliceSettingsInput<'_>,
) -> Result<SlicePresetDocuments, CommandError> {
    validate_controls(input.controls, single_nozzle(input.profile)?)?;
    let mut machine = index.flatten(PresetKind::Machine, input.machine_preset)?;
    let mut process = index.flatten(PresetKind::Process, input.process_preset)?;
    let filament = index.flatten(PresetKind::Filament, input.filament_preset)?;
    index.check_process_compatible(input.process_preset, input.machine_preset)?;
    index.check_filament_compatible(input.filament_preset, input.machine_preset)?;

    let override_keys = apply_profile_overrides(
        &mut machine,
        input.profile,
        input.overridden_fields,
        input.unknown_override_keys,
    )?;
    let control_keys = apply_controls(&mut process, input.controls);

    let written = override_keys
        .iter()
        .map(|key| (PresetKind::Machine, *key))
        .chain(control_keys.iter().map(|key| (PresetKind::Process, *key)));
    for (kind, key) in written {
        if !index.is_known_key(kind, key) {
            return Err(CommandError::unsupported_setting_for_runtime(
                key,
                index.version(),
            ));
        }
    }

    Ok(SlicePresetDocuments {
        machine: Value::Object(machine),
        process: Value::Object(process),
        filament: Value::Object(filament),
        override_keys,
        control_keys,
    })
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;
    use std::fs;

    use super::*;
    use crate::contracts::command::{ErrorCode, JsonValue};
    use crate::library::content::CancelFlag;
    use crate::slicing::presets::tests::{fixture_index, fixtures_dir};
    use serde_json::json;

    fn a_profile() -> PrinterProfile {
        PrinterProfile {
            bed_shape: BedShape::Rectangular {
                width_mm: 220.0,
                depth_mm: 220.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 250.0,
            bed_exclude_areas: vec![],
            default_bed_type: "1".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "brass".to_string(),
            gcode_flavor: "marlin".to_string(),
            has_auxiliary_fan: true,
            supports_air_filtration: true,
            supports_multi_filament: false,
            suggested_host_type: Some("moonraker".to_string()),
        }
    }

    fn fields(names: &[&str]) -> Vec<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    fn input<'a>(
        profile: &'a PrinterProfile,
        overridden: &'a [String],
        unknown: &'a [String],
        controls: &'a SliceControls,
    ) -> SliceSettingsInput<'a> {
        SliceSettingsInput {
            machine_preset: "Test Printer 0.4 nozzle",
            process_preset: "0.20mm Standard @Test",
            filament_preset: "Generic PLA @Test Printer",
            profile,
            overridden_fields: overridden,
            unknown_override_keys: unknown,
            controls,
        }
    }

    #[test]
    fn every_profile_field_is_mapped_or_not_applicable() {
        let Value::Object(wire) = serde_json::to_value(a_profile()).unwrap() else {
            panic!("a profile is an object");
        };
        let profile_fields: BTreeSet<&str> = wire.keys().map(String::as_str).collect();
        let table_fields: BTreeSet<&str> = PROFILE_FIELD_MAPPINGS
            .iter()
            .map(|(field, _)| *field)
            .collect();
        assert_eq!(profile_fields, table_fields);
    }

    #[test]
    fn every_control_is_in_the_control_table() {
        let every_control = SliceControls {
            layer_height_mm: Some(0.2),
            wall_loops: Some(2),
            top_shell_layers: Some(4),
            bottom_shell_layers: Some(3),
            infill_density_percent: Some(15.0),
            infill_pattern: Some(InfillPattern::Grid),
            supports: Some(SupportMode::TreeAuto),
            support_threshold_angle_deg: Some(30.0),
            brim_type: Some(BrimType::AutoBrim),
            brim_width_mm: Some(5.0),
            skirt_loops: Some(1),
        };
        let Value::Object(wire) = serde_json::to_value(&every_control).unwrap() else {
            panic!("controls are an object");
        };
        let control_fields: BTreeSet<&str> = wire.keys().map(String::as_str).collect();
        let table_fields: BTreeSet<&str> =
            CONTROL_MAPPINGS.iter().map(|(field, _)| *field).collect();
        assert_eq!(control_fields, table_fields);

        let mut process = Map::new();
        let written: BTreeSet<&str> = apply_controls(&mut process, &every_control)
            .into_iter()
            .collect();
        let table_keys: BTreeSet<&str> = CONTROL_MAPPINGS
            .iter()
            .flat_map(|(_, keys)| keys.iter().copied())
            .collect();
        assert_eq!(written, table_keys);
    }

    #[test]
    fn every_mapped_key_is_known_to_the_fixture_preset_source() {
        let index = fixture_index();
        for (kind, key) in mapped_keys() {
            assert!(index.is_known_key(kind, key), "{key} unknown for {kind:?}");
        }
    }

    #[test]
    fn profile_overrides_use_the_d4_encodings() {
        let mut profile = a_profile();
        profile.bed_shape = BedShape::Rectangular {
            width_mm: 256.0,
            depth_mm: 250.5,
            origin_x_mm: 10.0,
            origin_y_mm: 0.0,
        };
        profile.printable_height_mm = 256.0;
        profile.bed_exclude_areas = vec![
            PointMm {
                x_mm: 246.0,
                y_mm: 0.0,
            },
            PointMm {
                x_mm: 256.0,
                y_mm: 20.5,
            },
        ];
        profile.default_bed_type = "Textured PEI Plate".to_string();
        let mut machine = Map::new();
        let written = apply_profile_overrides(
            &mut machine,
            &profile,
            &fields(&[
                "bedShape",
                "printableHeightMm",
                "bedExcludeAreas",
                "nozzleDiameterMm",
                "nozzleType",
                "gcodeFlavor",
                "defaultBedType",
            ]),
            &[],
        )
        .unwrap();
        assert_eq!(written.len(), 7);
        assert_eq!(
            Value::Object(machine),
            json!({
                "printable_area": ["10x0", "266x0", "266x250.5", "10x250.5"],
                "printable_height": "256",
                "bed_exclude_area": ["246x0", "256x20.5"],
                "nozzle_diameter": ["0.4"],
                "nozzle_type": "brass",
                "gcode_flavor": "marlin",
                "default_bed_type": "Textured PEI Plate",
            })
        );
    }

    #[test]
    fn a_polygon_bed_lists_its_points() {
        let mut profile = a_profile();
        profile.bed_shape = BedShape::Polygon {
            points: vec![
                PointMm {
                    x_mm: 0.0,
                    y_mm: -100.0,
                },
                PointMm {
                    x_mm: 86.6,
                    y_mm: 50.0,
                },
                PointMm {
                    x_mm: -86.6,
                    y_mm: 50.0,
                },
            ],
        };
        let mut machine = Map::new();
        apply_profile_overrides(&mut machine, &profile, &fields(&["bedShape"]), &[]).unwrap();
        assert_eq!(
            machine["printable_area"],
            json!(["0x-100", "86.6x50", "-86.6x50"])
        );
    }

    #[test]
    fn not_applicable_overrides_change_nothing() {
        let mut machine = Map::new();
        let written = apply_profile_overrides(
            &mut machine,
            &a_profile(),
            &fields(&["hasAuxiliaryFan", "supportsAirFiltration"]),
            &[],
        )
        .unwrap();
        assert!(written.is_empty());
        assert!(machine.is_empty());
    }

    #[test]
    fn an_unmapped_override_blocks_slicing() {
        let mut machine = Map::new();
        for (overridden, unknown) in [
            (fields(&["futureField"]), vec![]),
            (vec![], fields(&["futureKey"])),
        ] {
            let error = apply_profile_overrides(&mut machine, &a_profile(), &overridden, &unknown)
                .unwrap_err();
            assert_eq!(error.code, ErrorCode::UnmappedProfileOverride);
        }

        let index = fixture_index();
        let profile = a_profile();
        let unknown = fields(&["futureKey"]);
        let error = apply_overrides_and_controls(
            &index,
            input(&profile, &[], &unknown, &SliceControls::default()),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::UnmappedProfileOverride);
        assert_eq!(
            error.details.unwrap()["field"],
            JsonValue::String("futureKey".to_string())
        );
    }

    #[test]
    fn controls_use_orcaslicer_values() {
        let mut process = Map::new();
        apply_controls(
            &mut process,
            &SliceControls {
                layer_height_mm: Some(0.12),
                infill_density_percent: Some(22.5),
                infill_pattern: Some(InfillPattern::Gyroid),
                supports: Some(SupportMode::NormalAuto),
                brim_type: Some(BrimType::NoBrim),
                ..SliceControls::default()
            },
        );
        assert_eq!(
            Value::Object(process),
            json!({
                "layer_height": "0.12",
                "sparse_infill_density": "22.5%",
                "sparse_infill_pattern": "gyroid",
                "enable_support": "1",
                "support_type": "normal(auto)",
                "brim_type": "no_brim",
            })
        );

        let mut off = Map::new();
        apply_controls(
            &mut off,
            &SliceControls {
                supports: Some(SupportMode::Off),
                ..SliceControls::default()
            },
        );
        assert_eq!(Value::Object(off), json!({ "enable_support": "0" }));
    }

    #[test]
    fn apply_produces_three_flat_documents() {
        let index = fixture_index();
        let mut profile = a_profile();
        profile.printable_height_mm = 200.0;
        let overridden = fields(&["printableHeightMm", "hasAuxiliaryFan"]);
        let controls = SliceControls {
            wall_loops: Some(5),
            ..SliceControls::default()
        };
        let documents =
            apply_overrides_and_controls(&index, input(&profile, &overridden, &[], &controls))
                .unwrap();
        assert_eq!(documents.override_keys, ["printable_height"]);
        assert_eq!(documents.control_keys, ["wall_loops"]);
        assert_eq!(documents.machine["printable_height"], "200");
        assert_eq!(documents.machine["from"], "system");
        assert_eq!(documents.machine["name"], "Test Printer 0.4 nozzle");
        assert_eq!(documents.process["wall_loops"], "5");
        // Unset controls keep the preset's value.
        assert_eq!(documents.process["layer_height"], "0.2");
        assert_eq!(documents.filament["filament_type"], json!(["PLA"]));
        for document in [&documents.machine, &documents.process, &documents.filament] {
            assert!(document.get("inherits").is_none());
        }
    }

    #[test]
    fn apply_refuses_out_of_range_controls() {
        let index = fixture_index();
        let profile = a_profile();
        // 0.33 mm is over 80% of the profile's 0.4 mm nozzle.
        let controls = SliceControls {
            layer_height_mm: Some(0.33),
            ..SliceControls::default()
        };
        let error =
            apply_overrides_and_controls(&index, input(&profile, &[], &[], &controls)).unwrap_err();
        assert_eq!(error.code, ErrorCode::Validation);
        assert_eq!(
            error.details.unwrap()["fieldPath"],
            JsonValue::String("controls.layerHeightMm".to_string())
        );

        // The same height is fine for a 0.6 mm nozzle.
        let mut wide = a_profile();
        wide.nozzle_diameter_mm = vec![0.6];
        apply_overrides_and_controls(&index, input(&wide, &[], &[], &controls)).unwrap();

        let mut two_nozzles = a_profile();
        two_nozzles.nozzle_diameter_mm = vec![0.4, 0.4];
        let error = apply_overrides_and_controls(
            &index,
            input(&two_nozzles, &[], &[], &SliceControls::default()),
        )
        .unwrap_err();
        assert_eq!(
            error.details.unwrap()["fieldPath"],
            JsonValue::String("target".to_string())
        );
    }

    #[test]
    fn an_incompatible_filament_or_missing_preset_is_refused() {
        let index = fixture_index();
        let profile = a_profile();
        let controls = SliceControls::default();
        let mut incompatible = input(&profile, &[], &[], &controls);
        incompatible.filament_preset = "Test PLA @Test Delta";
        assert_eq!(
            apply_overrides_and_controls(&index, incompatible)
                .unwrap_err()
                .code,
            ErrorCode::FilamentIncompatible
        );
        let mut missing = input(&profile, &[], &[], &controls);
        missing.machine_preset = "Gone 0.4 nozzle";
        assert_eq!(
            apply_overrides_and_controls(&index, missing)
                .unwrap_err()
                .code,
            ErrorCode::PresetNotFound
        );
    }

    /// A preset source whose process presets never set `brim_type` or
    /// `skirt_loops`: writing either is `UNSUPPORTED_SETTING_FOR_RUNTIME`.
    #[test]
    fn an_unknown_key_is_unsupported_for_the_runtime() {
        let temp = tempfile::tempdir().unwrap();
        let copy = temp.path().join("profiles");
        copy_dir(&fixtures_dir(), &copy);
        let common = copy.join("TestVendor/process/fdm_process_common.json");
        let mut base: Value = serde_json::from_str(&fs::read_to_string(&common).unwrap()).unwrap();
        base.as_object_mut().unwrap().remove("brim_type");
        fs::write(&common, base.to_string()).unwrap();
        let index = PresetIndex::build(&copy, "2.3.0", &CancelFlag::never()).unwrap();

        let profile = a_profile();
        let controls = SliceControls {
            brim_type: Some(BrimType::OuterOnly),
            ..SliceControls::default()
        };
        let error =
            apply_overrides_and_controls(&index, input(&profile, &[], &[], &controls)).unwrap_err();
        assert_eq!(error.code, ErrorCode::UnsupportedSettingForRuntime);
        let details = error.details.unwrap();
        assert_eq!(details["key"], JsonValue::String("brim_type".to_string()));
        assert_eq!(
            details["presetSourceVersion"],
            JsonValue::String("2.3.0".to_string())
        );
        // An unset control writes nothing, so nothing is checked.
        apply_overrides_and_controls(&index, input(&profile, &[], &[], &SliceControls::default()))
            .unwrap();
    }

    fn copy_dir(from: &std::path::Path, to: &std::path::Path) {
        fs::create_dir_all(to).unwrap();
        for entry in fs::read_dir(from).unwrap() {
            let entry = entry.unwrap();
            let target = to.join(entry.file_name());
            if entry.path().is_dir() {
                copy_dir(&entry.path(), &target);
            } else {
                fs::copy(entry.path(), target).unwrap();
            }
        }
    }

    #[test]
    fn controls_are_validated_against_the_d4_ranges() {
        let ok = SliceControls {
            layer_height_mm: Some(0.32),
            wall_loops: Some(20),
            top_shell_layers: Some(0),
            bottom_shell_layers: Some(50),
            infill_density_percent: Some(100.0),
            support_threshold_angle_deg: Some(90.0),
            brim_width_mm: Some(20.0),
            skirt_loops: Some(10),
            ..SliceControls::default()
        };
        validate_controls(&ok, 0.4).unwrap();
        let cases = [
            (
                SliceControls {
                    layer_height_mm: Some(0.33),
                    ..Default::default()
                },
                "controls.layerHeightMm",
            ),
            (
                SliceControls {
                    layer_height_mm: Some(0.04),
                    ..Default::default()
                },
                "controls.layerHeightMm",
            ),
            (
                SliceControls {
                    wall_loops: Some(0),
                    ..Default::default()
                },
                "controls.wallLoops",
            ),
            (
                SliceControls {
                    top_shell_layers: Some(51),
                    ..Default::default()
                },
                "controls.topShellLayers",
            ),
            (
                SliceControls {
                    infill_density_percent: Some(f64::NAN),
                    ..Default::default()
                },
                "controls.infillDensityPercent",
            ),
            (
                SliceControls {
                    support_threshold_angle_deg: Some(91.0),
                    ..Default::default()
                },
                "controls.supportThresholdAngleDeg",
            ),
            (
                SliceControls {
                    brim_width_mm: Some(-1.0),
                    ..Default::default()
                },
                "controls.brimWidthMm",
            ),
            (
                SliceControls {
                    skirt_loops: Some(11),
                    ..Default::default()
                },
                "controls.skirtLoops",
            ),
        ];
        for (controls, field) in cases {
            let error = validate_controls(&controls, 0.4).unwrap_err();
            assert_eq!(error.code, ErrorCode::Validation);
            assert_eq!(
                error.details.unwrap()["fieldPath"],
                JsonValue::String(field.to_string())
            );
        }
    }
}
