//! P5 runtime slicing and Slice Revisions: the domain and wire types, and
//! their ids (spec D1, D5, D10, D12, D14-D16, and "Wire types").
//!
//! Rust remains persisted truth. [`repository`] holds the SQL for D14,
//! [`facts`] the D15 facts and their provenance, and [`blockers`] the
//! Slice Revision deletion registry plus the Model deletion blocker P5
//! registers with the Library. [`runtime`] discovers the OrcaSlicer engine
//! and preset source (D2), [`presets`] indexes and flattens presets and
//! lists the slice options (D3), and [`mapping`] holds the D4 tables.
//! [`geometry`] and [`hull`] compute revision geometry and the D5 instance
//! transforms (D6), and [`plate3mf`] writes the per-plate input 3MF (D7).
//! [`invocation`] lays out an operation's work directory and records its
//! manifest (D8), [`process`] supervises OrcaSlicer (D9), and [`publish`]
//! validates the output and publishes the Slice Revision (D11-D13).

pub mod blockers;
pub mod facts;
pub mod geometry;
pub mod hull;
pub mod invocation;
pub mod mapping;
pub mod plate3mf;
pub mod presets;
pub mod process;
pub mod process_group;
pub mod publish;
pub mod repository;
pub mod runtime;

pub use geometry::{GeometryBuildItem, GeometryObject, LayFlatFace, RevisionGeometry};
pub use runtime::{
    EngineSource, EngineState, PresetSourceOrigin, PresetSourceState, SlicerRuntimeStatus,
};

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::library::formats::Producer;
use crate::printers::CatalogRef;
use crate::spools::MaterialFamily;

pub use facts::{
    ConfirmedFact, ConfirmedFacts, ExternalFacts, Fact, FactProvenance, Farm3dFacts,
    ProfileSnapshot, SliceFacts,
};

/// D1: the id prefix of a Preparation (`prp-`).
pub const PREPARATION_ID_PREFIX: &str = "prp";
/// D1: the id prefix of a slice operation (`sop-`).
pub const OPERATION_ID_PREFIX: &str = "sop";
/// D1: the id prefix of a Slice Revision (`slr-`).
pub const SLICE_REVISION_ID_PREFIX: &str = "slr";

pub fn new_preparation_id() -> String {
    crate::library::new_id(PREPARATION_ID_PREFIX)
}

pub fn new_operation_id() -> String {
    crate::library::new_id(OPERATION_ID_PREFIX)
}

pub fn new_slice_revision_id() -> String {
    crate::library::new_id(SLICE_REVISION_ID_PREFIX)
}

/// D5: what a Preparation slices for — a Printer (its resolved profile) or
/// a catalog profile.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/SliceTarget.ts"
)]
pub enum SliceTarget {
    Printer { printer_id: String },
    Profile { catalog_ref: CatalogRef },
}

/// D4: the infill patterns P5 offers, by their OrcaSlicer enum names.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "lowercase")]
#[ts(rename_all = "lowercase", export_to = "domain/InfillPattern.ts")]
pub enum InfillPattern {
    Rectilinear,
    Grid,
    Line,
    Cubic,
    Gyroid,
    Honeycomb,
    Lightning,
}

/// D4: support generation — off, or OrcaSlicer's `normal(auto)` or
/// `tree(auto)` support type.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[ts(export_to = "domain/SupportMode.ts")]
pub enum SupportMode {
    #[serde(rename = "off")]
    #[ts(rename = "off")]
    Off,
    #[serde(rename = "normal(auto)")]
    #[ts(rename = "normal(auto)")]
    NormalAuto,
    #[serde(rename = "tree(auto)")]
    #[ts(rename = "tree(auto)")]
    TreeAuto,
}

/// D4: the brim types P5 offers, by their OrcaSlicer enum names.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "snake_case")]
#[ts(rename_all = "snake_case", export_to = "domain/BrimType.ts")]
pub enum BrimType {
    NoBrim,
    OuterOnly,
    AutoBrim,
}

/// D4: the slicing controls P5 exposes. Each is optional; an unset control
/// takes the chosen process preset's value. Ranges are checked by
/// Preparation validation, not here.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, Default, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceControls.ts")]
pub struct SliceControls {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub layer_height_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub wall_loops: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub top_shell_layers: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub bottom_shell_layers: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub infill_density_percent: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub infill_pattern: Option<InfillPattern>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub supports: Option<SupportMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub support_threshold_angle_deg: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub brim_type: Option<BrimType>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub brim_width_mm: Option<f64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub skirt_loops: Option<u32>,
}

/// D5: one instance's placement. The local mesh is scaled, rotated X then
/// Y then Z (extrinsic), and translated on XY; Z is derived so the
/// instance rests on the bed.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/InstanceTransform.ts")]
pub struct InstanceTransform {
    pub translate_mm: [f64; 2],
    pub rotate_deg: [f64; 3],
    pub scale: [f64; 3],
}

/// D5: one placed copy of a source object.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/InstanceDoc.ts")]
pub struct InstanceDoc {
    pub instance_key: String,
    /// The source object: `GeometryObject.objectKey` (D6).
    pub object_key: u32,
    pub transform: InstanceTransform,
}

/// D5: one build plate, with a stable `plateKey`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PlateDoc.ts")]
pub struct PlateDoc {
    pub plate_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    pub instances: Vec<InstanceDoc>,
}

/// D5: a Preparation's whole editable document, stored as
/// `slice_preparations.document_json`. The presets are unset until one is
/// chosen (for example while no slicer runtime is available).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PreparationDocument.ts")]
pub struct PreparationDocument {
    pub plates: Vec<PlateDoc>,
    pub target: SliceTarget,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub process_preset: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub filament_preset: Option<String>,
    pub controls: SliceControls,
}

/// D5: a Preparation as the frontend sees it. `stale` is derived on read:
/// true when `sourceRevisionId` is not the Model's current revision.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PreparationRecord.ts")]
pub struct PreparationRecord {
    pub id: String,
    pub model_id: String,
    pub source_revision_id: String,
    #[ts(type = "number")]
    pub revision: i64,
    pub stale: bool,
    pub document: PreparationDocument,
    pub created_at: String,
    pub updated_at: String,
}

/// D10: a slice operation's state. `succeeded`, `failed`, `cancelled`, and
/// `interrupted` are terminal.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceOperationState.ts")]
pub enum SliceOperationState {
    Queued,
    Running,
    Succeeded,
    Failed,
    Cancelled,
    Interrupted,
}

/// D11: why a slice operation failed. Codes with a payload carry it
/// beside `kind`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/SliceFailureCode.ts"
)]
pub enum SliceFailureCode {
    ObjectsOutsidePlate,
    PresetInvalid,
    InputMissing,
    InputInvalid,
    PresetIncompatible,
    EngineError { return_code: i32 },
    OutputMissing,
    OutputInvalid { reason: String },
    Timeout,
    EngineCrashed { signal: i32 },
    SpawnFailed,
}

/// D11: a failed operation's code and its user-facing text, stored as
/// `slice_operations.failure_json`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceFailure.ts")]
pub struct SliceFailure {
    pub code: SliceFailureCode,
    pub message: String,
}

/// The plate an operation slices, frozen when it is queued and stored as
/// `slice_operations.plate_snapshot_json`. Not wire-exported.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase")]
pub struct PlateSnapshot {
    /// 1-based position of the plate in the Preparation.
    pub plate_index: u32,
    pub plate: PlateDoc,
}

/// D10: one slice operation as the frontend sees it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceOperationRecord.ts")]
pub struct SliceOperationRecord {
    pub id: String,
    pub preparation_id: String,
    pub source_revision_id: String,
    pub plate_key: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plate_name: Option<String>,
    pub plate_index: u32,
    pub state: SliceOperationState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub failure: Option<SliceFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub slice_revision_id: Option<String>,
    pub queued_at: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub started_at: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub finished_at: Option<String>,
}

/// D1: the two kinds of Slice Revision.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceRevisionKind.ts")]
pub enum SliceRevisionKind {
    Farm3d,
    External,
}

/// D1: a farm3d revision's plate identity. External revisions have none.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SlicePlateRef.ts")]
pub struct SlicePlateRef {
    pub plate_key: String,
    /// 1-based.
    pub plate_index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plate_name: Option<String>,
}

/// D2: whether an OrcaSlicer build is a release or a prerelease.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/RuntimeChannel.ts")]
pub enum RuntimeChannel {
    Release,
    Prerelease,
}

/// The engine and preset source a farm3d revision was sliced with, stored
/// as `slice_revisions.runtime_json`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceRuntimeInfo.ts")]
pub struct SliceRuntimeInfo {
    pub engine_version: String,
    pub engine_channel: RuntimeChannel,
    pub preset_source_version: String,
    pub preset_source_channel: RuntimeChannel,
}

/// D12: always `farm3dSlice` — the estimates came from farm3d's own slice.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceEstimateSource.ts")]
pub enum SliceEstimateSource {
    Farm3dSlice,
}

/// D12: always `fileClaim` — the values are what a G-code file says.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/ClaimedEstimateSource.ts"
)]
pub enum ClaimedEstimateSource {
    FileClaim,
}

/// D12: the estimates parsed from the G-code farm3d produced. Each is
/// `null` when its claim is missing. Only farm3d revisions have them: an
/// external revision's `estimates` is `null`, and its file's values are
/// [`ClaimedEstimates`], never copied here.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceEstimates.ts")]
pub struct SliceEstimates {
    #[ts(type = "number | null")]
    pub print_seconds: Option<u64>,
    pub filament_grams: Option<f64>,
    pub filament_mm: Option<f64>,
    pub layer_count: Option<u32>,
    pub max_z_mm: Option<f64>,
    pub source: SliceEstimateSource,
}

impl SliceEstimates {
    /// Every estimate `null`.
    pub fn none() -> Self {
        Self {
            print_seconds: None,
            filament_grams: None,
            filament_mm: None,
            layer_count: None,
            max_z_mm: None,
            source: SliceEstimateSource::Farm3dSlice,
        }
    }
}

/// D12/D16: an external G-code file's own estimate claims, shown as "What
/// the file says (not verified)".
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ClaimedEstimates.ts")]
pub struct ClaimedEstimates {
    #[ts(type = "number | null")]
    pub print_seconds: Option<u64>,
    pub filament_grams: Option<f64>,
    pub filament_mm: Option<f64>,
    pub layer_count: Option<u32>,
    pub max_z_mm: Option<f64>,
    pub source: ClaimedEstimateSource,
    #[ts(type = "false")]
    pub trusted: bool,
}

/// D15: what a farm3d revision was sliced for — the Preparation's target
/// (with the Printer id, if it was a Printer), the profile snapshot, the
/// flat preset names, and the controls. Stored as
/// `slice_revisions.target_json`; external revisions have none.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceRevisionTarget.ts")]
pub struct SliceRevisionTarget {
    pub target: SliceTarget,
    pub profile: ProfileSnapshot,
    pub machine_preset: String,
    pub process_preset: String,
    pub filament_preset: String,
    pub controls: SliceControls,
}

/// D14: what a `slice_revision_blobs` row holds.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/SliceRevisionBlobRole.ts"
)]
pub enum SliceRevisionBlobRole {
    Plate3mf,
    MachinePreset,
    ProcessPreset,
    FilamentPreset,
    Manifest,
    Log,
}

/// One input blob of a farm3d revision, as the record lists it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceRevisionBlob.ts")]
pub struct SliceRevisionBlob {
    pub role: SliceRevisionBlobRole,
    #[ts(type = "number")]
    pub size_bytes: i64,
}

/// A Slice Revision as lists show it.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceRevisionSummary.ts")]
pub struct SliceRevisionSummary {
    pub id: String,
    pub kind: SliceRevisionKind,
    pub model_id: String,
    pub source_revision_id: String,
    #[ts(type = "number")]
    pub source_revision_sequence: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub plate: Option<SlicePlateRef>,
    pub target_label: String,
    /// `null` for an external revision (D12).
    pub estimates: Option<SliceEstimates>,
    pub facts: SliceFacts,
    pub requires_manual_printer_selection: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub runtime: Option<SliceRuntimeInfo>,
    pub created_at: String,
}

/// A Slice Revision in full: the summary plus the target (farm3d only),
/// the file's own claims and producer (external only), and the input
/// blobs (farm3d only).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceRevisionRecord.ts")]
pub struct SliceRevisionRecord {
    #[serde(flatten)]
    #[ts(flatten)]
    pub summary: SliceRevisionSummary,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub target: Option<SliceRevisionTarget>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub claimed_estimates: Option<ClaimedEstimates>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub producer: Option<Producer>,
    pub blobs: Vec<SliceRevisionBlob>,
}

/// One offered process preset.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ProcessPresetOption.ts")]
pub struct ProcessPresetOption {
    pub name: String,
}

/// One offered filament preset. `materialFamily` is its `filament_type`
/// mapped as D15 maps it; both are absent when the preset has no type.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/FilamentPresetOption.ts")]
pub struct FilamentPresetOption {
    pub name: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub filament_type: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub material_family: Option<MaterialFamily>,
}

/// D3's deterministic defaults; `null` when nothing is offered.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceOptionDefaults.ts")]
pub struct SliceOptionDefaults {
    pub process_preset: Option<String>,
    pub filament_preset: Option<String>,
}

/// `list_slice_options`' result (built by [`presets::list_slice_options`]).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SliceOptions.ts")]
pub struct SliceOptions {
    pub machine_preset: String,
    pub process_presets: Vec<ProcessPresetOption>,
    pub filament_presets: Vec<FilamentPresetOption>,
    pub defaults: SliceOptionDefaults,
    pub profile_snapshot: ProfileSnapshot,
    pub matching_printer_ids: Vec<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_use_the_d1_prefixes() {
        assert!(new_preparation_id().starts_with("prp-"));
        assert!(new_operation_id().starts_with("sop-"));
        assert!(new_slice_revision_id().starts_with("slr-"));
    }

    /// The migration's `CHECK` lists spell each stored enum exactly as
    /// serde does.
    #[test]
    fn stored_enums_serialize_to_the_d14_check_spellings() {
        let wire = |value: serde_json::Value| value.as_str().unwrap().to_string();
        let states = [
            SliceOperationState::Queued,
            SliceOperationState::Running,
            SliceOperationState::Succeeded,
            SliceOperationState::Failed,
            SliceOperationState::Cancelled,
            SliceOperationState::Interrupted,
        ]
        .map(|state| wire(serde_json::to_value(state).unwrap()));
        assert_eq!(
            states,
            [
                "queued",
                "running",
                "succeeded",
                "failed",
                "cancelled",
                "interrupted"
            ]
        );
        let kinds = [SliceRevisionKind::Farm3d, SliceRevisionKind::External]
            .map(|kind| wire(serde_json::to_value(kind).unwrap()));
        assert_eq!(kinds, ["farm3d", "external"]);
        let roles = [
            SliceRevisionBlobRole::Plate3mf,
            SliceRevisionBlobRole::MachinePreset,
            SliceRevisionBlobRole::ProcessPreset,
            SliceRevisionBlobRole::FilamentPreset,
            SliceRevisionBlobRole::Manifest,
            SliceRevisionBlobRole::Log,
        ]
        .map(|role| wire(serde_json::to_value(role).unwrap()));
        assert_eq!(
            roles,
            [
                "plate3mf",
                "machinePreset",
                "processPreset",
                "filamentPreset",
                "manifest",
                "log"
            ]
        );
    }

    #[test]
    fn slice_target_and_failure_use_their_tagged_wire_shapes() {
        assert_eq!(
            serde_json::to_value(SliceTarget::Printer {
                printer_id: "prn-a".to_string()
            })
            .unwrap(),
            serde_json::json!({ "kind": "printer", "printerId": "prn-a" })
        );
        assert_eq!(
            serde_json::to_value(SliceFailure {
                code: SliceFailureCode::EngineError { return_code: -9 },
                message: "boom".to_string(),
            })
            .unwrap(),
            serde_json::json!({
                "code": { "kind": "engineError", "returnCode": -9 },
                "message": "boom",
            })
        );
    }

    #[test]
    fn estimates_are_null_when_their_claim_is_missing() {
        assert_eq!(
            serde_json::to_value(SliceEstimates::none()).unwrap(),
            serde_json::json!({
                "printSeconds": null, "filamentGrams": null, "filamentMm": null,
                "layerCount": null, "maxZMm": null, "source": "farm3dSlice",
            })
        );
    }
}
