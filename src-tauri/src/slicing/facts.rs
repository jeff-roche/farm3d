//! D15: the compatibility facts every Slice Revision carries, and where
//! each one came from.
//!
//! A fact is a value with a provenance ([`Fact`]). A farm3d revision's
//! facts are all `farm3dInput`, taken from the slice's own inputs. An
//! external revision's facts are `operatorConfirmed` or `absent`, and never
//! `farm3dInput`. [`Farm3dFacts`] and [`ExternalFacts`] enforce that split:
//! each can only be built from its own kind of input, and the repository's
//! insert for each revision kind takes only its own wrapper.

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::catalog::{BedShape, PointMm, PrinterProfile};
use crate::printers::CatalogRef;
use crate::spools::MaterialFamily;

/// Where a fact's value came from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/FactProvenance.ts")]
pub enum FactProvenance {
    /// farm3d chose the value as a slicing input.
    Farm3dInput,
    /// The operator entered or explicitly accepted the value.
    OperatorConfirmed,
    /// Nobody provided the value.
    Absent,
}

/// One fact: `{ provenance, value }`, where `value` is `null` exactly when
/// the provenance is `absent`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "provenance", rename_all = "camelCase")]
#[ts(
    tag = "provenance",
    rename_all = "camelCase",
    export_to = "domain/Fact.ts"
)]
pub enum Fact<T> {
    Farm3dInput { value: T },
    OperatorConfirmed { value: T },
    Absent { value: () },
}

impl<T> Fact<T> {
    pub fn absent() -> Self {
        Self::Absent { value: () }
    }

    pub fn provenance(&self) -> FactProvenance {
        match self {
            Self::Farm3dInput { .. } => FactProvenance::Farm3dInput,
            Self::OperatorConfirmed { .. } => FactProvenance::OperatorConfirmed,
            Self::Absent { .. } => FactProvenance::Absent,
        }
    }

    pub fn value(&self) -> Option<&T> {
        match self {
            Self::Farm3dInput { value } | Self::OperatorConfirmed { value } => Some(value),
            Self::Absent { .. } => None,
        }
    }

    pub fn is_absent(&self) -> bool {
        matches!(self, Self::Absent { .. })
    }
}

/// The part of a target Printer Profile that decides where a Slice
/// Revision may be printed.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ProfileSnapshot.ts")]
pub struct ProfileSnapshot {
    pub catalog_ref: CatalogRef,
    pub bed_shape: BedShape,
    pub printable_height_mm: f64,
    pub bed_exclude_areas: Vec<PointMm>,
    pub nozzle_type: String,
    pub gcode_flavor: String,
}

impl ProfileSnapshot {
    pub fn new(catalog_ref: CatalogRef, profile: &PrinterProfile) -> Self {
        Self {
            catalog_ref,
            bed_shape: profile.bed_shape.clone(),
            printable_height_mm: profile.printable_height_mm,
            bed_exclude_areas: profile.bed_exclude_areas.clone(),
            nozzle_type: profile.nozzle_type.clone(),
            gcode_flavor: profile.gcode_flavor.clone(),
        }
    }
}

/// D15's four facts. `materialOther` carries the raw material name when
/// `materialFamily` is `OTHER`, and is omitted otherwise.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceFacts.ts")]
pub struct SliceFacts {
    pub printer_profile: Fact<ProfileSnapshot>,
    pub nozzle_diameter_mm: Fact<f64>,
    pub material_family: Fact<MaterialFamily>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub material_other: Option<String>,
    pub filament_diameter_mm: Fact<f64>,
}

impl SliceFacts {
    /// D15: true exactly when any fact is absent. P7 reads it.
    pub fn requires_manual_printer_selection(&self) -> bool {
        self.provenances().contains(&FactProvenance::Absent)
    }

    /// Each fact's provenance, in declaration order.
    pub fn provenances(&self) -> [FactProvenance; 4] {
        [
            self.printer_profile.provenance(),
            self.nozzle_diameter_mm.provenance(),
            self.material_family.provenance(),
            self.filament_diameter_mm.provenance(),
        ]
    }
}

/// `materialOther` is kept only beside a known `OTHER` family.
fn material_other_for(family: Option<&MaterialFamily>, other: Option<String>) -> Option<String> {
    match family {
        Some(MaterialFamily::Other) => other,
        _ => None,
    }
}

/// A farm3d revision's facts: every one `farm3dInput`, from the target and
/// the presets the slice ran with.
#[derive(Clone, PartialEq, Debug)]
pub struct Farm3dFacts(SliceFacts);

impl Farm3dFacts {
    pub fn new(
        printer_profile: ProfileSnapshot,
        nozzle_diameter_mm: f64,
        material_family: MaterialFamily,
        material_other: Option<String>,
        filament_diameter_mm: f64,
    ) -> Self {
        Self(SliceFacts {
            material_other: material_other_for(Some(&material_family), material_other),
            printer_profile: Fact::Farm3dInput {
                value: printer_profile,
            },
            nozzle_diameter_mm: Fact::Farm3dInput {
                value: nozzle_diameter_mm,
            },
            material_family: Fact::Farm3dInput {
                value: material_family,
            },
            filament_diameter_mm: Fact::Farm3dInput {
                value: filament_diameter_mm,
            },
        })
    }

    pub fn facts(&self) -> &SliceFacts {
        &self.0
    }
}

/// One operator answer for an external revision's fact: a confirmed value,
/// or nothing. There is no way to say `farm3dInput` here.
#[derive(Clone, PartialEq, Debug)]
pub enum ConfirmedFact<T> {
    Confirmed(T),
    Absent,
}

impl<T> ConfirmedFact<T> {
    fn into_fact(self) -> Fact<T> {
        match self {
            Self::Confirmed(value) => Fact::OperatorConfirmed { value },
            Self::Absent => Fact::absent(),
        }
    }
}

/// The operator's answers for every D15 fact of an external revision.
#[derive(Clone, PartialEq, Debug)]
pub struct ConfirmedFacts {
    pub printer_profile: ConfirmedFact<ProfileSnapshot>,
    pub nozzle_diameter_mm: ConfirmedFact<f64>,
    pub material_family: ConfirmedFact<MaterialFamily>,
    pub material_other: Option<String>,
    pub filament_diameter_mm: ConfirmedFact<f64>,
}

/// An external revision's facts: each `operatorConfirmed` or `absent`,
/// never `farm3dInput` (D15), because the only constructor takes
/// [`ConfirmedFacts`].
#[derive(Clone, PartialEq, Debug)]
pub struct ExternalFacts(SliceFacts);

impl ExternalFacts {
    pub fn new(confirmed: ConfirmedFacts) -> Self {
        let material_family = confirmed.material_family.into_fact();
        Self(SliceFacts {
            material_other: material_other_for(material_family.value(), confirmed.material_other),
            printer_profile: confirmed.printer_profile.into_fact(),
            nozzle_diameter_mm: confirmed.nozzle_diameter_mm.into_fact(),
            material_family,
            filament_diameter_mm: confirmed.filament_diameter_mm.into_fact(),
        })
    }

    pub fn facts(&self) -> &SliceFacts {
        &self.0
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    pub(crate) fn a_profile_snapshot() -> ProfileSnapshot {
        ProfileSnapshot {
            catalog_ref: CatalogRef {
                vendor: "Elegoo".to_string(),
                model: "Centauri Carbon".to_string(),
                variant: "Elegoo Centauri Carbon 0.4 nozzle".to_string(),
                model_id: "ECC".to_string(),
                printer_variant: "0.4".to_string(),
            },
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
        }
    }

    fn answer<T>(confirmed: bool, value: T) -> ConfirmedFact<T> {
        if confirmed {
            ConfirmedFact::Confirmed(value)
        } else {
            ConfirmedFact::Absent
        }
    }

    /// Every combination of confirmed and absent answers: none yields
    /// `farm3dInput`, and `requiresManualPrinterSelection` is true exactly
    /// when some answer is absent.
    #[test]
    fn external_facts_are_never_farm3d_input_for_any_combination_of_answers() {
        for mask in 0u8..16 {
            let bit = |n: u8| mask & (1 << n) != 0;
            let facts = ExternalFacts::new(ConfirmedFacts {
                printer_profile: answer(bit(0), a_profile_snapshot()),
                nozzle_diameter_mm: answer(bit(1), 0.4),
                material_family: answer(bit(2), MaterialFamily::Petg),
                material_other: None,
                filament_diameter_mm: answer(bit(3), 1.75),
            });
            let provenances = facts.facts().provenances();

            assert!(
                !provenances.contains(&FactProvenance::Farm3dInput),
                "mask {mask:04b}: {provenances:?}"
            );
            for (index, provenance) in provenances.iter().enumerate() {
                let expected = if bit(index as u8) {
                    FactProvenance::OperatorConfirmed
                } else {
                    FactProvenance::Absent
                };
                assert_eq!(*provenance, expected, "mask {mask:04b}, fact {index}");
            }
            assert_eq!(
                facts.facts().requires_manual_printer_selection(),
                mask != 0b1111,
                "mask {mask:04b}"
            );
        }
    }

    #[test]
    fn farm3d_facts_are_all_farm3d_input_and_need_no_manual_selection() {
        let facts = Farm3dFacts::new(a_profile_snapshot(), 0.4, MaterialFamily::Pla, None, 1.75);

        assert_eq!(
            facts.facts().provenances(),
            [FactProvenance::Farm3dInput; 4]
        );
        assert!(!facts.facts().requires_manual_printer_selection());
    }

    #[test]
    fn material_other_is_kept_only_beside_the_other_family() {
        let other = Farm3dFacts::new(
            a_profile_snapshot(),
            0.4,
            MaterialFamily::Other,
            Some("PPS".to_string()),
            1.75,
        );
        let known = Farm3dFacts::new(
            a_profile_snapshot(),
            0.4,
            MaterialFamily::Pla,
            Some("PPS".to_string()),
            1.75,
        );
        let absent = ExternalFacts::new(ConfirmedFacts {
            printer_profile: ConfirmedFact::Absent,
            nozzle_diameter_mm: ConfirmedFact::Absent,
            material_family: ConfirmedFact::Absent,
            material_other: Some("PPS".to_string()),
            filament_diameter_mm: ConfirmedFact::Absent,
        });

        assert_eq!(other.facts().material_other.as_deref(), Some("PPS"));
        assert_eq!(known.facts().material_other, None);
        assert_eq!(absent.facts().material_other, None);
    }

    #[test]
    fn facts_serialize_to_the_d15_wire_shape_and_round_trip() {
        let facts = ExternalFacts::new(ConfirmedFacts {
            printer_profile: ConfirmedFact::Absent,
            nozzle_diameter_mm: ConfirmedFact::Confirmed(0.4),
            material_family: ConfirmedFact::Confirmed(MaterialFamily::Other),
            material_other: Some("PPS".to_string()),
            filament_diameter_mm: ConfirmedFact::Absent,
        });
        let json = serde_json::to_value(facts.facts()).expect("serialize");

        assert_eq!(
            json,
            serde_json::json!({
                "printerProfile": { "provenance": "absent", "value": null },
                "nozzleDiameterMm": { "provenance": "operatorConfirmed", "value": 0.4 },
                "materialFamily": { "provenance": "operatorConfirmed", "value": "OTHER" },
                "materialOther": "PPS",
                "filamentDiameterMm": { "provenance": "absent", "value": null },
            })
        );
        let back: SliceFacts = serde_json::from_value(json).expect("deserialize");
        assert_eq!(&back, facts.facts());
    }
}
