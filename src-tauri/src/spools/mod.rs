//! P3 Spool and Material Slot domain types (D1-D5, D9) — the wire and
//! persisted shapes every later P3 task builds on: the ledger and tares
//! (Task 2), movement (Task 3), reservations (Task 4), slot layouts
//! (Task 5), archive dispositions (Task 6), and commands (Task 7).
//!
//! Rust remains persisted truth (global constraint: "the frontend never
//! derives facets, availability, or eligibility"). Amounts are integer
//! milligrams everywhere below the UI (D1); see [`weight`] for the
//! gram<->milligram conversion and range constants `validate_fields` checks
//! against.

pub mod weight;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::RepositoryError;

/// D2's closed material-family enum. Confirmed against OrcaSlicer's
/// `filament_type` option at the pinned catalog tag `v2.4.2`: that option's
/// values are generated from `MaterialType::all()` rather than a literal
/// `enum_values` array, but all 13 real families below are present
/// verbatim in that list, so D2's list is unchanged (see the Task 1
/// report). `Other` is farm3d's own sentinel, not a slicer value.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[ts(export_to = "domain/MaterialFamily.ts")]
pub enum MaterialFamily {
    #[serde(rename = "PLA")]
    Pla,
    #[serde(rename = "PLA-CF")]
    PlaCf,
    #[serde(rename = "PETG")]
    Petg,
    #[serde(rename = "PET-CF")]
    PetCf,
    #[serde(rename = "ABS")]
    Abs,
    #[serde(rename = "ASA")]
    Asa,
    #[serde(rename = "TPU")]
    Tpu,
    #[serde(rename = "PA")]
    Pa,
    #[serde(rename = "PA-CF")]
    PaCf,
    #[serde(rename = "PC")]
    Pc,
    #[serde(rename = "PVA")]
    Pva,
    #[serde(rename = "HIPS")]
    Hips,
    #[serde(rename = "PP")]
    Pp,
    #[serde(rename = "OTHER")]
    Other,
}

/// D1: filament diameter is an enum, not a free number.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[ts(export_to = "domain/FilamentDiameter.ts")]
pub enum FilamentDiameter {
    #[serde(rename = "1.75")]
    D175,
    #[serde(rename = "2.85")]
    D285,
}

/// D7/D9: whether a cached amount comes from a scale/direct entry or a
/// user estimate.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AmountConfidence.ts")]
pub enum AmountConfidence {
    Measured,
    Estimated,
}

/// D9: a Spool's lifecycle state.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SpoolLifecycle.ts")]
pub enum SpoolLifecycle {
    Active,
    Empty,
    Archived,
}

/// D5: a Spool is always in exactly one Material Slot or in storage. The DB
/// enforces this too (`spools_slot_occupancy` and the table's CHECKs), not
/// only this type.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(tag = "kind", rename_all = "camelCase")]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    export_to = "domain/SpoolLocation.ts"
)]
pub enum SpoolLocation {
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Slot { slot_id: String, printer_id: String },
    #[serde(rename_all = "camelCase")]
    #[ts(rename_all = "camelCase")]
    Storage { storage_label: Option<String> },
}

/// D9: derived, orthogonal facets the frontend filters on. Rust is the only
/// thing that computes these — the frontend never re-derives them.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SpoolFacets.ts")]
pub struct SpoolFacets {
    pub loaded: bool,
    pub reserved: bool,
    pub low: bool,
    pub confidence: AmountConfidence,
}

/// D8: `availableMg = currentMg - reservedMg`. It can go negative once an
/// estimate or measurement drops `currentMg` below what's reserved.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Availability.ts")]
pub struct Availability {
    #[ts(type = "number")]
    pub current_mg: i64,
    #[ts(type = "number")]
    pub reserved_mg: i64,
    #[ts(type = "number")]
    pub available_mg: i64,
}

/// A Spool record (D1-D5, D9). Every amount is integer milligrams (D1).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SpoolRecord.ts")]
pub struct SpoolRecord {
    pub id: String,
    #[ts(type = "number")]
    pub revision: i64,
    #[ts(type = "number")]
    pub spool_number: i64,
    pub manufacturer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub product: Option<String>,
    pub material_family: MaterialFamily,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub material_other: Option<String>,
    pub color_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color_hex: Option<String>,
    pub diameter: FilamentDiameter,
    #[ts(type = "number")]
    pub nominal_mg: i64,
    #[ts(type = "number")]
    pub low_threshold_mg: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tare_id: Option<String>,
    pub lifecycle: SpoolLifecycle,
    pub location: SpoolLocation,
    pub availability: Availability,
    pub facets: SpoolFacets,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub last_measured_at: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub notes: Option<String>,
    pub created_at: String,
    pub updated_at: String,
}

/// D4/D12: one Printer's Material Slot.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/MaterialSlot.ts")]
pub struct MaterialSlot {
    pub id: String,
    pub position: u8,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub feeder_label: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub occupant_spool_id: Option<String>,
}

/// Every user-editable Spool field (D2), validated by [`validate_fields`].
/// `create_spool`/`update_spool` (Task 2/7) build this from a command's raw
/// input before calling it. Excludes `lifecycle`, `location`,
/// `availability`, and `facets` — those change only through
/// `set_spool_lifecycle`, `move_spool`, and derivation, never through a
/// field edit.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SpoolFields.ts")]
pub struct SpoolFields {
    pub manufacturer: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub product: Option<String>,
    pub material_family: MaterialFamily,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub material_other: Option<String>,
    pub color_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub color_hex: Option<String>,
    pub diameter: FilamentDiameter,
    #[ts(type = "number")]
    pub nominal_mg: i64,
    #[ts(type = "number")]
    pub low_threshold_mg: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub tare_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub notes: Option<String>,
}

impl SpoolFields {
    /// Normalizes `colorHex`'s case in place (`"#aabbcc"` -> `"#AABBCC"`)
    /// before validation/storage, matching the migration's uppercase-only
    /// `GLOB` CHECK. Case is the only thing normalized here — format is
    /// [`validate_fields`]'s job.
    pub fn normalize(&mut self) {
        if let Some(hex) = &mut self.color_hex {
            *hex = hex.to_ascii_uppercase();
        }
    }
}

fn validate_trimmed_len(
    value: &str,
    min: usize,
    max: usize,
    field_path: &'static str,
) -> Result<(), RepositoryError> {
    let len = value.chars().count();
    if value.trim() != value || len < min || len > max {
        return Err(RepositoryError::Validation { field_path });
    }
    Ok(())
}

/// D2: `materialOther` must be present, 1-32 chars, and trimmed when the
/// family is `Other`; forbidden for every other family.
fn validate_material_other(
    family: MaterialFamily,
    material_other: &Option<String>,
) -> Result<(), RepositoryError> {
    match (family, material_other) {
        (MaterialFamily::Other, Some(other)) => validate_trimmed_len(other, 1, 32, "materialOther"),
        (MaterialFamily::Other, None) => Err(RepositoryError::Validation {
            field_path: "materialOther",
        }),
        (_, None) => Ok(()),
        (_, Some(_)) => Err(RepositoryError::Validation {
            field_path: "materialOther",
        }),
    }
}

/// D2: `#RRGGBB`, uppercase only. The caller normalizes case first (see
/// [`SpoolFields::normalize`]) — this only checks the format the migration's
/// `GLOB` CHECK also enforces.
fn validate_color_hex(color_hex: &str) -> Result<(), RepositoryError> {
    let is_valid = color_hex.len() == 7
        && color_hex.as_bytes()[0] == b'#'
        && color_hex.as_bytes()[1..]
            .iter()
            .all(|byte| byte.is_ascii_digit() || (b'A'..=b'F').contains(byte));
    if is_valid {
        Ok(())
    } else {
        Err(RepositoryError::Validation {
            field_path: "colorHex",
        })
    }
}

/// D1/D2: every range and presence rule a `SpoolFields` must satisfy before
/// `create_spool`/`update_spool` (Task 2/7) write it. Assumes the caller has
/// already normalized `colorHex`'s case (see [`SpoolFields::normalize`]).
pub fn validate_fields(fields: &SpoolFields) -> Result<(), RepositoryError> {
    validate_trimmed_len(&fields.manufacturer, 1, 64, "manufacturer")?;
    if let Some(product) = &fields.product {
        validate_trimmed_len(product, 1, 64, "product")?;
    }
    validate_material_other(fields.material_family, &fields.material_other)?;
    validate_trimmed_len(&fields.color_name, 1, 32, "colorName")?;
    if let Some(color_hex) = &fields.color_hex {
        validate_color_hex(color_hex)?;
    }
    if !weight::NOMINAL_MG_RANGE.contains(&fields.nominal_mg) {
        return Err(RepositoryError::Validation {
            field_path: "nominalMg",
        });
    }
    if !weight::LOW_THRESHOLD_MG_RANGE.contains(&fields.low_threshold_mg) {
        return Err(RepositoryError::Validation {
            field_path: "lowThresholdMg",
        });
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn valid_fields() -> SpoolFields {
        SpoolFields {
            manufacturer: "Polymaker".to_string(),
            product: Some("PolyTerra".to_string()),
            material_family: MaterialFamily::Pla,
            material_other: None,
            color_name: "Black".to_string(),
            color_hex: Some("#000000".to_string()),
            diameter: FilamentDiameter::D175,
            nominal_mg: 1_000_000,
            low_threshold_mg: 100_000,
            tare_id: None,
            notes: None,
        }
    }

    #[test]
    fn valid_fields_pass() {
        assert!(validate_fields(&valid_fields()).is_ok());
    }

    #[test]
    fn manufacturer_must_be_one_to_sixty_four_chars() {
        let mut fields = valid_fields();
        fields.manufacturer = String::new();
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "manufacturer"
            })
        ));

        fields.manufacturer = "x".repeat(65);
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "manufacturer"
            })
        ));

        fields.manufacturer = "x".repeat(64);
        assert!(validate_fields(&fields).is_ok());
    }

    #[test]
    fn manufacturer_must_already_be_trimmed() {
        let mut fields = valid_fields();
        fields.manufacturer = " Polymaker".to_string();
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "manufacturer"
            })
        ));
    }

    #[test]
    fn product_is_optional_but_bounded_and_trimmed_when_present() {
        let mut fields = valid_fields();
        fields.product = None;
        assert!(validate_fields(&fields).is_ok());

        fields.product = Some(String::new());
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "product"
            })
        ));

        fields.product = Some("x".repeat(65));
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "product"
            })
        ));

        fields.product = Some("PolyTerra ".to_string());
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "product"
            })
        ));
    }

    #[test]
    fn other_requires_material_other_one_to_thirty_two_chars_trimmed() {
        let mut fields = valid_fields();
        fields.material_family = MaterialFamily::Other;
        fields.material_other = None;
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "materialOther"
            })
        ));

        fields.material_other = Some(String::new());
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "materialOther"
            })
        ));

        fields.material_other = Some("x".repeat(33));
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "materialOther"
            })
        ));

        fields.material_other = Some(" PVDF".to_string());
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "materialOther"
            })
        ));

        fields.material_other = Some("PVDF".to_string());
        assert!(validate_fields(&fields).is_ok());
    }

    #[test]
    fn every_other_family_forbids_material_other() {
        let mut fields = valid_fields();
        fields.material_family = MaterialFamily::Pla;
        fields.material_other = Some("PVDF".to_string());
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "materialOther"
            })
        ));
    }

    #[test]
    fn color_name_must_be_one_to_thirty_two_chars_and_trimmed() {
        let mut fields = valid_fields();
        fields.color_name = String::new();
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "colorName"
            })
        ));

        fields.color_name = "x".repeat(33);
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "colorName"
            })
        ));

        fields.color_name = " Black".to_string();
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "colorName"
            })
        ));
    }

    #[test]
    fn color_hex_is_optional_but_must_be_hash_rrggbb_uppercase() {
        let mut fields = valid_fields();
        fields.color_hex = None;
        assert!(validate_fields(&fields).is_ok());

        for invalid in ["000000", "#00000", "#0000000", "#GGGGGG", "#aabbcc"] {
            fields.color_hex = Some(invalid.to_string());
            assert!(
                matches!(
                    validate_fields(&fields),
                    Err(RepositoryError::Validation {
                        field_path: "colorHex"
                    })
                ),
                "{invalid} must be rejected"
            );
        }

        fields.color_hex = Some("#AABBCC".to_string());
        assert!(validate_fields(&fields).is_ok());
    }

    #[test]
    fn normalize_uppercases_color_hex_case_only() {
        let mut fields = valid_fields();
        fields.color_hex = Some("#aabbcc".to_string());
        fields.normalize();
        assert_eq!(fields.color_hex, Some("#AABBCC".to_string()));

        // A malformed value's case is still normalized; format validation
        // happens separately in `validate_fields`.
        fields.color_hex = Some("#zzzzzz".to_string());
        fields.normalize();
        assert_eq!(fields.color_hex, Some("#ZZZZZZ".to_string()));
    }

    #[test]
    fn nominal_mg_must_be_one_gram_to_fifty_kilograms() {
        let mut fields = valid_fields();
        fields.nominal_mg = 999;
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "nominalMg"
            })
        ));

        fields.nominal_mg = 50_000_001;
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "nominalMg"
            })
        ));

        fields.nominal_mg = 1_000;
        assert!(validate_fields(&fields).is_ok());
        fields.nominal_mg = 50_000_000;
        assert!(validate_fields(&fields).is_ok());
    }

    #[test]
    fn low_threshold_mg_must_be_zero_to_fifty_kilograms() {
        let mut fields = valid_fields();
        fields.low_threshold_mg = -1;
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "lowThresholdMg"
            })
        ));

        fields.low_threshold_mg = 50_000_001;
        assert!(matches!(
            validate_fields(&fields),
            Err(RepositoryError::Validation {
                field_path: "lowThresholdMg"
            })
        ));

        fields.low_threshold_mg = 0;
        assert!(validate_fields(&fields).is_ok());
        fields.low_threshold_mg = 50_000_000;
        assert!(validate_fields(&fields).is_ok());
    }

    #[test]
    fn material_family_wire_values_match_d2() {
        let cases = [
            (MaterialFamily::Pla, "\"PLA\""),
            (MaterialFamily::PlaCf, "\"PLA-CF\""),
            (MaterialFamily::Petg, "\"PETG\""),
            (MaterialFamily::PetCf, "\"PET-CF\""),
            (MaterialFamily::Abs, "\"ABS\""),
            (MaterialFamily::Asa, "\"ASA\""),
            (MaterialFamily::Tpu, "\"TPU\""),
            (MaterialFamily::Pa, "\"PA\""),
            (MaterialFamily::PaCf, "\"PA-CF\""),
            (MaterialFamily::Pc, "\"PC\""),
            (MaterialFamily::Pva, "\"PVA\""),
            (MaterialFamily::Hips, "\"HIPS\""),
            (MaterialFamily::Pp, "\"PP\""),
            (MaterialFamily::Other, "\"OTHER\""),
        ];
        for (family, wire) in cases {
            assert_eq!(serde_json::to_string(&family).unwrap(), wire);
        }
    }

    #[test]
    fn filament_diameter_wire_values_are_the_mm_strings() {
        assert_eq!(
            serde_json::to_string(&FilamentDiameter::D175).unwrap(),
            "\"1.75\""
        );
        assert_eq!(
            serde_json::to_string(&FilamentDiameter::D285).unwrap(),
            "\"2.85\""
        );
    }

    #[test]
    fn spool_location_is_tagged_by_kind() {
        let slot = SpoolLocation::Slot {
            slot_id: "slt-a".to_string(),
            printer_id: "prn-a".to_string(),
        };
        assert_eq!(
            serde_json::to_value(&slot).unwrap(),
            serde_json::json!({"kind": "slot", "slotId": "slt-a", "printerId": "prn-a"})
        );

        let storage = SpoolLocation::Storage {
            storage_label: Some("Shelf 1".to_string()),
        };
        assert_eq!(
            serde_json::to_value(&storage).unwrap(),
            serde_json::json!({"kind": "storage", "storageLabel": "Shelf 1"})
        );
    }
}
