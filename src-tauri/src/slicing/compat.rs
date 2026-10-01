//! P7 D5's tolerant profile and material comparisons.
//!
//! [`profile_compatible`] is D5 gate 2: it compares a Slice Revision's D15
//! facts with a Printer's resolved profile. [`material_compatible`] is
//! (part of) gate 4: it compares those facts with one candidate Spool's
//! material. Both use the spec's 0.01 mm tolerance for every geometric or
//! diameter comparison, in place of P5's exact `==`
//! (`presets.rs`'s former private `profiles_match`). [`profiles_match`]
//! generalizes that same comparison for two already-resolved profiles —
//! `slicing::presets::matching_printer_ids`' P5 display count — with the
//! same tolerance.

use crate::catalog::{BedShape, PrinterProfile};
use crate::slicing::facts::SliceFacts;
use crate::spools::{FilamentDiameter, MaterialFamily, SpoolRecord};

/// D5's tolerance for every geometric or diameter comparison in this
/// module: 0.01 mm.
const TOLERANCE_MM: f64 = 0.01;

fn approx_eq(a: f64, b: f64) -> bool {
    (a - b).abs() <= TOLERANCE_MM
}

/// Which D15 fact was absent, for [`CompatMismatch::FactAbsent`] and
/// [`MaterialMismatch::FactAbsent`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FactKey {
    PrinterProfile,
    NozzleDiameterMm,
    MaterialFamily,
    FilamentDiameterMm,
}

/// D5 gate 2: one way a Slice Revision's facts and a Printer's resolved
/// profile disagree.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum CompatMismatch {
    BedShape,
    PrintableHeight,
    NozzleCount { found: usize },
    NozzleDiameter { want: f64, have: f64 },
    NozzleType,
    GcodeFlavor,
    FactAbsent(FactKey),
}

fn bed_shape_matches(a: &BedShape, b: &BedShape) -> bool {
    match (a, b) {
        (
            BedShape::Rectangular {
                width_mm: aw,
                depth_mm: ad,
                origin_x_mm: ax,
                origin_y_mm: ay,
            },
            BedShape::Rectangular {
                width_mm: bw,
                depth_mm: bd,
                origin_x_mm: bx,
                origin_y_mm: by,
            },
        ) => {
            approx_eq(*aw, *bw) && approx_eq(*ad, *bd) && approx_eq(*ax, *bx) && approx_eq(*ay, *by)
        }
        (BedShape::Polygon { points: ap }, BedShape::Polygon { points: bp }) => {
            ap.len() == bp.len()
                && ap
                    .iter()
                    .zip(bp.iter())
                    .all(|(p, q)| approx_eq(p.x_mm, q.x_mm) && approx_eq(p.y_mm, q.y_mm))
        }
        _ => false,
    }
}

/// D5 gate 2: compares the revision's facts with the Printer's resolved
/// profile on bed shape, printable height, single-nozzle diameter, nozzle
/// type, and G-code flavor (`bedExcludeAreas` is never compared, as in
/// P5's matching count). Empty means compatible. Every mismatch found is
/// returned, in this order, so the caller can report the first as
/// `detail`. An absent `printerProfile` or `nozzleDiameterMm` fact yields
/// `FactAbsent` for that half instead of a comparison — the caller (D5)
/// decides whether that is tolerable (Manual policy only).
pub fn profile_compatible(facts: &SliceFacts, printer: &PrinterProfile) -> Vec<CompatMismatch> {
    let mut mismatches = Vec::new();
    let snapshot = facts.printer_profile.value();

    match snapshot {
        Some(snapshot) => {
            if !bed_shape_matches(&snapshot.bed_shape, &printer.bed_shape) {
                mismatches.push(CompatMismatch::BedShape);
            }
            if !approx_eq(snapshot.printable_height_mm, printer.printable_height_mm) {
                mismatches.push(CompatMismatch::PrintableHeight);
            }
        }
        None => mismatches.push(CompatMismatch::FactAbsent(FactKey::PrinterProfile)),
    }

    match printer.nozzle_diameter_mm.as_slice() {
        [have] => match facts.nozzle_diameter_mm.value() {
            Some(&want) => {
                if !approx_eq(want, *have) {
                    mismatches.push(CompatMismatch::NozzleDiameter { want, have: *have });
                }
            }
            None => mismatches.push(CompatMismatch::FactAbsent(FactKey::NozzleDiameterMm)),
        },
        other => mismatches.push(CompatMismatch::NozzleCount { found: other.len() }),
    }

    if let Some(snapshot) = snapshot {
        if snapshot.nozzle_type != printer.nozzle_type {
            mismatches.push(CompatMismatch::NozzleType);
        }
        if snapshot.gcode_flavor != printer.gcode_flavor {
            mismatches.push(CompatMismatch::GcodeFlavor);
        }
    }

    mismatches
}

/// Whether `a` and `b` match on the fields P5's matching-Printer count
/// compares (`bedShape`, `printableHeightMm`, `nozzleDiameterMm`,
/// `nozzleType`, `gcodeFlavor`; `bedExcludeAreas` is never compared),
/// within D5's 0.01 mm tolerance. Generalizes the former private
/// `presets.rs` `profiles_match`, which used exact `==`.
/// [`slicing::presets::matching_printer_ids`] calls this.
pub fn profiles_match(a: &PrinterProfile, b: &PrinterProfile) -> bool {
    bed_shape_matches(&a.bed_shape, &b.bed_shape)
        && approx_eq(a.printable_height_mm, b.printable_height_mm)
        && a.nozzle_diameter_mm.len() == b.nozzle_diameter_mm.len()
        && a.nozzle_diameter_mm
            .iter()
            .zip(b.nozzle_diameter_mm.iter())
            .all(|(x, y)| approx_eq(*x, *y))
        && a.nozzle_type == b.nozzle_type
        && a.gcode_flavor == b.gcode_flavor
}

/// D5 gate 4: one way a Slice Revision's facts and a candidate Spool
/// disagree on material.
#[derive(Clone, PartialEq, Debug)]
pub enum MaterialMismatch {
    Family,
    Diameter { want: f64, have: f64 },
    FactAbsent(FactKey),
}

fn diameter_mm(diameter: FilamentDiameter) -> f64 {
    match diameter {
        FilamentDiameter::D175 => 1.75,
        FilamentDiameter::D285 => 2.85,
    }
}

fn family_matches(want: MaterialFamily, want_other: Option<&str>, spool: &SpoolRecord) -> bool {
    if spool.material_family != want {
        return false;
    }
    if want == MaterialFamily::Other {
        let want_other = want_other.unwrap_or_default().trim().to_lowercase();
        let have_other = spool
            .material_other
            .as_deref()
            .unwrap_or_default()
            .trim()
            .to_lowercase();
        want_other == have_other
    } else {
        true
    }
}

/// D5 gate 4: whether `spool`'s material matches the revision's facts —
/// `materialFamily` (and, for `OTHER`, `materialOther` case-insensitively
/// after trimming) and `filamentDiameterMm` within 0.01 mm. Ignores
/// `active`/availability, which the caller (`queue::eligibility`) checks
/// separately.
///
/// The two facts are checked independently: an absent fact only skips
/// *its own* comparison (Manual policy only, D5) and is reported as
/// `FactAbsent` — but only when the *other* half actually matched. A
/// concrete mismatch on the other half is reported instead, even when this
/// half's fact is absent, so a genuine problem (e.g. a 2.85 mm Spool
/// against a 1.75 mm fact) is never masked by an unrelated absent fact.
pub fn material_compatible(
    facts: &SliceFacts,
    spool: &SpoolRecord,
) -> Result<(), MaterialMismatch> {
    let family_absent;
    let mut family_mismatch = None;
    match facts.material_family.value() {
        Some(&family) => {
            family_absent = false;
            if !family_matches(family, facts.material_other.as_deref(), spool) {
                family_mismatch = Some(MaterialMismatch::Family);
            }
        }
        None => family_absent = true,
    }

    let diameter_absent;
    let mut diameter_mismatch = None;
    match facts.filament_diameter_mm.value() {
        Some(&want) => {
            diameter_absent = false;
            let have = diameter_mm(spool.diameter);
            if !approx_eq(want, have) {
                diameter_mismatch = Some(MaterialMismatch::Diameter { want, have });
            }
        }
        None => diameter_absent = true,
    }

    // Concrete mismatches take priority over reporting either half's
    // absent fact.
    if let Some(mismatch) = family_mismatch {
        return Err(mismatch);
    }
    if let Some(mismatch) = diameter_mismatch {
        return Err(mismatch);
    }
    if family_absent {
        return Err(MaterialMismatch::FactAbsent(FactKey::MaterialFamily));
    }
    if diameter_absent {
        return Err(MaterialMismatch::FactAbsent(FactKey::FilamentDiameterMm));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printers::CatalogRef;
    use crate::slicing::facts::{ConfirmedFact, ConfirmedFacts, ExternalFacts, Farm3dFacts};
    use crate::spools::{
        AmountConfidence, Availability, SpoolFacets, SpoolLifecycle, SpoolLocation,
    };

    fn a_profile_snapshot_source() -> CatalogRef {
        CatalogRef {
            vendor: "Elegoo".to_string(),
            model: "Centauri Carbon".to_string(),
            variant: "Elegoo Centauri Carbon 0.4 nozzle".to_string(),
            model_id: "ECC".to_string(),
            printer_variant: "0.4".to_string(),
        }
    }

    fn a_bed_shape() -> BedShape {
        BedShape::Rectangular {
            width_mm: 256.0,
            depth_mm: 256.0,
            origin_x_mm: 0.0,
            origin_y_mm: 0.0,
        }
    }

    fn a_profile_snapshot() -> crate::slicing::facts::ProfileSnapshot {
        crate::slicing::facts::ProfileSnapshot {
            catalog_ref: a_profile_snapshot_source(),
            bed_shape: a_bed_shape(),
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
        }
    }

    fn a_printer_profile() -> PrinterProfile {
        PrinterProfile {
            bed_shape: a_bed_shape(),
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            default_bed_type: "PEI".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: None,
            suggested_port: None,
        }
    }

    fn a_farm3d_facts() -> SliceFacts {
        Farm3dFacts::new(a_profile_snapshot(), 0.4, MaterialFamily::Pla, None, 1.75)
            .facts()
            .clone()
    }

    fn a_spool(family: MaterialFamily, diameter: FilamentDiameter) -> SpoolRecord {
        SpoolRecord {
            id: "spl-1".to_string(),
            revision: 1,
            spool_number: 1,
            manufacturer: "Test".to_string(),
            product: None,
            material_family: family,
            material_other: None,
            color_name: "Black".to_string(),
            color_hex: None,
            diameter,
            nominal_mg: 1_000_000,
            low_threshold_mg: 100_000,
            tare_id: None,
            lifecycle: SpoolLifecycle::Active,
            location: SpoolLocation::Storage {
                storage_label: None,
            },
            availability: Availability {
                current_mg: 900_000,
                reserved_mg: 0,
                available_mg: 900_000,
            },
            facets: SpoolFacets {
                loaded: false,
                reserved: false,
                low: false,
                reconciliation: false,
                confidence: AmountConfidence::Estimated,
            },
            last_measured_at: None,
            notes: None,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            updated_at: "2026-01-01T00:00:00Z".to_string(),
        }
    }

    #[test]
    fn matching_profiles_have_no_mismatches() {
        let facts = a_farm3d_facts();
        assert_eq!(profile_compatible(&facts, &a_printer_profile()), Vec::new());
    }

    #[test]
    fn nozzle_list_with_two_entries_is_nozzle_count_mismatch() {
        let facts = a_farm3d_facts();
        let mut printer = a_printer_profile();
        printer.nozzle_diameter_mm = vec![0.4, 0.4];

        let mismatches = profile_compatible(&facts, &printer);

        assert_eq!(mismatches, vec![CompatMismatch::NozzleCount { found: 2 }]);
    }

    #[test]
    fn diameter_175_matches_1_75_fact_within_tolerance() {
        let facts = ExternalFacts::new(ConfirmedFacts {
            printer_profile: ConfirmedFact::Absent,
            nozzle_diameter_mm: ConfirmedFact::Absent,
            material_family: ConfirmedFact::Confirmed(MaterialFamily::Pla),
            material_other: None,
            filament_diameter_mm: ConfirmedFact::Confirmed(1.751),
        })
        .facts()
        .clone();
        let spool = a_spool(MaterialFamily::Pla, FilamentDiameter::D175);

        assert_eq!(material_compatible(&facts, &spool), Ok(()));
    }

    #[test]
    fn diameter_just_outside_tolerance_is_a_mismatch() {
        let facts = ExternalFacts::new(ConfirmedFacts {
            printer_profile: ConfirmedFact::Absent,
            nozzle_diameter_mm: ConfirmedFact::Absent,
            material_family: ConfirmedFact::Confirmed(MaterialFamily::Pla),
            material_other: None,
            filament_diameter_mm: ConfirmedFact::Confirmed(1.77),
        })
        .facts()
        .clone();
        let spool = a_spool(MaterialFamily::Pla, FilamentDiameter::D175);

        assert_eq!(
            material_compatible(&facts, &spool),
            Err(MaterialMismatch::Diameter {
                want: 1.77,
                have: 1.75
            })
        );
    }

    #[test]
    fn absent_printer_profile_fact_is_reported_and_skips_its_comparison() {
        let facts = ExternalFacts::new(ConfirmedFacts {
            printer_profile: ConfirmedFact::Absent,
            nozzle_diameter_mm: ConfirmedFact::Confirmed(0.4),
            material_family: ConfirmedFact::Confirmed(MaterialFamily::Pla),
            material_other: None,
            filament_diameter_mm: ConfirmedFact::Confirmed(1.75),
        })
        .facts()
        .clone();

        assert_eq!(
            profile_compatible(&facts, &a_printer_profile()),
            vec![CompatMismatch::FactAbsent(FactKey::PrinterProfile)]
        );
    }

    #[test]
    fn absent_material_family_fact_is_reported() {
        let facts = ExternalFacts::new(ConfirmedFacts {
            printer_profile: ConfirmedFact::Absent,
            nozzle_diameter_mm: ConfirmedFact::Absent,
            material_family: ConfirmedFact::Absent,
            material_other: None,
            filament_diameter_mm: ConfirmedFact::Confirmed(1.75),
        })
        .facts()
        .clone();
        let spool = a_spool(MaterialFamily::Pla, FilamentDiameter::D175);

        assert_eq!(
            material_compatible(&facts, &spool),
            Err(MaterialMismatch::FactAbsent(FactKey::MaterialFamily))
        );
    }

    #[test]
    fn absent_family_with_a_real_diameter_mismatch_reports_the_mismatch_not_fact_absent() {
        // D5: "an absent fact skips only that comparison" — a concrete
        // mismatch on the other half must still be reported, not masked by
        // the absent family.
        let facts = ExternalFacts::new(ConfirmedFacts {
            printer_profile: ConfirmedFact::Absent,
            nozzle_diameter_mm: ConfirmedFact::Absent,
            material_family: ConfirmedFact::Absent,
            material_other: None,
            filament_diameter_mm: ConfirmedFact::Confirmed(1.75),
        })
        .facts()
        .clone();
        let spool = a_spool(MaterialFamily::Pla, FilamentDiameter::D285);

        assert_eq!(
            material_compatible(&facts, &spool),
            Err(MaterialMismatch::Diameter {
                want: 1.75,
                have: 2.85
            })
        );
    }

    #[test]
    fn other_family_matches_case_insensitively_after_trimming() {
        let facts = Farm3dFacts::new(
            a_profile_snapshot(),
            0.4,
            MaterialFamily::Other,
            Some("  PPS  ".to_string()),
            1.75,
        )
        .facts()
        .clone();
        let mut spool = a_spool(MaterialFamily::Other, FilamentDiameter::D175);
        spool.material_other = Some("pps".to_string());

        assert_eq!(material_compatible(&facts, &spool), Ok(()));
    }

    #[test]
    fn profiles_match_is_tolerant_like_profile_compatible() {
        let mut other = a_printer_profile();
        other.printable_height_mm += 0.005;
        assert!(profiles_match(&a_printer_profile(), &other));

        other.printable_height_mm += 1.0;
        assert!(!profiles_match(&a_printer_profile(), &other));
    }
}
