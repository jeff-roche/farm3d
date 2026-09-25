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
//! [`preparation`] seeds, validates, and reloads Preparation documents
//! (D5), [`operations`] queues and runs slice operations and recovers them
//! at startup (D10), [`events`] is the `slicing` stream (D17), and
//! [`commands`] the Tauri commands. [`SlicingServices`] holds the runtime
//! state they share.

pub mod blockers;
pub mod commands;
pub mod events;
pub mod facts;
pub mod geometry;
pub mod hull;
pub mod invocation;
pub mod mapping;
pub mod operations;
pub mod plate3mf;
pub mod preparation;
pub mod presets;
pub mod printed_bounds;
pub mod process;
pub mod process_group;
pub mod publish;
pub mod repository;
pub mod runtime;

pub use geometry::{GeometryBuildItem, GeometryObject, LayFlatFace, RevisionGeometry};
pub use runtime::{
    EngineCandidate, EngineCandidateResult, EngineSource, EngineState, PresetSourceOrigin,
    PresetSourceState, SlicerRuntimeStatus,
};

use std::collections::{HashMap, HashSet};
use std::ffi::OsString;
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock};

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use ts_rs::TS;

use crate::catalog::Catalog;
use crate::contracts::command::CommandError;
use crate::library::content::{CancelFlag, ContentStore};
use crate::library::formats::{InspectError, Producer};
use crate::library::ModelFormat;
use crate::persistence::{RepositoryError, Storage, StorageError};
use crate::printers::CatalogRef;
use crate::spools::MaterialFamily;

use events::{SlicingEventSpec, SlicingStream};
use geometry::{GeometryCache, LoadedRevision};
use presets::{PresetIndex, PresetIndexError};
use runtime::{
    resolve_runtime_with, DiscoveryEnv, ResolvedEngine, ResolvedPresetSource, RuntimeCaches,
    SlicerRuntime, SlicerRuntimeFileIo,
};

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
    EngineError {
        return_code: i32,
    },
    OutputMissing,
    OutputInvalid {
        reason: String,
    },
    Timeout,
    EngineCrashed {
        signal: i32,
    },
    SpawnFailed,
    /// farm3d couldn't store the finished slice (the content store or the
    /// database failed). Not in D11's table: without it such an operation
    /// would stay `running` until the next start interrupted it.
    StorageFailed,
    /// farm3d itself failed while running the slice (its worker panicked).
    /// Not in D11's table either.
    InternalError,
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

// ---------------------------------------------------------------------------
// Services
// ---------------------------------------------------------------------------

/// The slicing runtime state in `RuntimeServices::slicing`: the stream,
/// the slicer runtime and its caches, the preset index, the geometry cache,
/// and the operation scheduler. Emits through the `AppHandle` it was
/// attached to ([`Self::attach`]); until then, events are dropped.
pub struct SlicingServices<R: tauri::Runtime> {
    /// D17: the `slicing` event stream.
    pub stream: SlicingStream,
    pub(crate) storage: Arc<Storage>,
    pub(crate) content: Arc<ContentStore>,
    pub(crate) catalog: Arc<Catalog>,
    file_io: Mutex<Arc<dyn SlicerRuntimeFileIo>>,
    /// Where extracted AppImage presets are cached (D2).
    cache_dir: PathBuf,
    discovery: Mutex<DiscoveryEnv>,
    /// D2: probes (60 s) and file hashes, shared by the status and every
    /// operation (D8's manifest).
    pub caches: RuntimeCaches,
    /// The last resolved runtime, which `get_slicer_runtime` returns.
    runtime: Mutex<Option<SlicerRuntime>>,
    /// D3: one preset index, for the current preset source.
    presets: Mutex<Option<(PathBuf, String, Arc<PresetIndex>)>>,
    presets_build: Mutex<()>,
    /// D6: loaded revisions, by content hash.
    geometry: GeometryCache,
    /// Content hashes verified once already (D6: verified on first load).
    verified: Mutex<HashSet<String>>,
    /// D10: the FIFO queue and the one running operation.
    pub(crate) scheduler: operations::Scheduler,
    /// Model id → Preparation id, so a Model deletion can report the
    /// Preparation that cascaded with it.
    preparations_by_model: Mutex<HashMap<String, String>>,
    /// Extra environment for OrcaSlicer, after the D8 allowlist. Empty in
    /// production; tests pass `FAKE_ORCA_*` scenarios through it.
    engine_environment: Mutex<Vec<(OsString, OsString)>>,
    app: OnceLock<AppHandle<R>>,
    /// Set once the Library listener is registered ([`Self::start`]).
    listening: OnceLock<()>,
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

impl<R: tauri::Runtime> SlicingServices<R> {
    pub fn new(
        storage: Arc<Storage>,
        content: Arc<ContentStore>,
        catalog: Arc<Catalog>,
        file_io: Arc<dyn SlicerRuntimeFileIo>,
        cache_dir: PathBuf,
    ) -> Self {
        Self {
            stream: SlicingStream::default(),
            storage,
            content,
            catalog,
            file_io: Mutex::new(file_io),
            cache_dir,
            discovery: Mutex::new(DiscoveryEnv::from_process()),
            caches: RuntimeCaches::default(),
            runtime: Mutex::new(None),
            presets: Mutex::new(None),
            presets_build: Mutex::new(()),
            geometry: GeometryCache::default(),
            verified: Mutex::new(HashSet::new()),
            scheduler: operations::Scheduler::default(),
            preparations_by_model: Mutex::new(HashMap::new()),
            engine_environment: Mutex::new(Vec::new()),
            app: OnceLock::new(),
            listening: OnceLock::new(),
        }
    }

    /// Where events go. The first handle wins; later calls do nothing.
    pub fn attach(&self, app: &AppHandle<R>) {
        let _ = self.app.set(app.clone());
    }

    /// Emits `events` in order, after the write they describe committed.
    pub fn publish(&self, events: Vec<SlicingEventSpec>) {
        if let Some(app) = self.app.get() {
            self.stream.publish(app, events);
        }
    }

    pub fn file_io(&self) -> Arc<dyn SlicerRuntimeFileIo> {
        Arc::clone(&lock(&self.file_io))
    }

    /// Test seam: whether the scheduler has nothing queued or running.
    #[doc(hidden)]
    pub fn scheduler_idle(&self) -> bool {
        self.scheduler.idle()
    }

    /// Test seam: see [`operations::SchedulerPoint`].
    #[doc(hidden)]
    pub fn set_scheduler_hook(&self, hook: Option<operations::SchedulerHook>) {
        self.scheduler.set_hook(hook);
    }

    /// Test seam: replaces the native pickers.
    pub fn set_file_io(&self, file_io: Arc<dyn SlicerRuntimeFileIo>) {
        *lock(&self.file_io) = file_io;
    }

    pub fn discovery_env(&self) -> DiscoveryEnv {
        lock(&self.discovery).clone()
    }

    /// Test seam: where discovery looks (tests keep it off this machine's
    /// `PATH` and home folders). Forgets the resolved runtime.
    pub fn set_discovery_env(&self, env: DiscoveryEnv) {
        *lock(&self.discovery) = env;
        *lock(&self.runtime) = None;
    }

    /// Test seam: environment variables every slice gets in addition to
    /// D8's allowlist. Production never sets any.
    pub fn set_engine_environment(&self, variables: Vec<(OsString, OsString)>) {
        *lock(&self.engine_environment) = variables;
    }

    pub(crate) fn engine_environment(&self) -> Vec<(OsString, OsString)> {
        lock(&self.engine_environment).clone()
    }

    pub fn cache_dir(&self) -> &Path {
        &self.cache_dir
    }

    // --- D2: the runtime ---------------------------------------------------

    /// The last resolved status, resolving it first if there is none.
    /// Blocks while it probes.
    pub fn runtime_status(&self) -> Result<SlicerRuntimeStatus, CommandError> {
        if let Some(runtime) = lock(&self.runtime).as_ref() {
            return Ok(runtime.status.clone());
        }
        self.resolve_runtime(false).map(|runtime| runtime.status)
    }

    /// Resolves the runtime from the stored configuration. `force` forgets
    /// every cached probe first (**Check again**, a configuration change);
    /// otherwise a probe from the last 60 s is reused. A status that
    /// differs from the last one goes out as `slicing.runtime.changed`.
    /// Blocks while it probes.
    pub fn resolve_runtime(&self, force: bool) -> Result<SlicerRuntime, CommandError> {
        let config = self
            .storage
            .read(|connection| Ok(repository::load_runtime_config(connection)))
            .map_err(storage_error)?
            .map_err(storage_error)?;
        if force {
            self.caches.probes.clear();
        }
        let env = self.discovery_env();
        let runtime = resolve_runtime_with(&config, &env, &self.cache_dir, &self.caches);
        // Published under the `runtime` lock, so two resolves' events go
        // out in the order their results were stored. Lock order: `runtime`,
        // then the stream; never the reverse.
        let mut current = lock(&self.runtime);
        let changed = current
            .as_ref()
            .is_none_or(|previous| previous.status != runtime.status);
        *current = Some(runtime.clone());
        if changed {
            self.publish(vec![events::runtime_changed(&runtime.status)]);
        }
        drop(current);
        Ok(runtime)
    }

    /// D2: the engine and preset source a slice (or the slice options)
    /// needs, re-probed through the 60 s cache. `SLICER_UNAVAILABLE` or
    /// `PRESET_SOURCE_UNAVAILABLE` when either is missing.
    pub fn usable_runtime(&self) -> Result<(ResolvedEngine, ResolvedPresetSource), CommandError> {
        let runtime = self.resolve_runtime(false)?;
        let engine = runtime.engine.ok_or_else(|| {
            CommandError::slicer_unavailable(&engine_reason(&runtime.status.engine))
        })?;
        let preset_source = runtime.preset_source.ok_or_else(|| {
            CommandError::preset_source_unavailable(&preset_source_reason(
                &runtime.status.preset_source,
            ))
        })?;
        Ok((engine, preset_source))
    }

    /// D3: the preset index of `source`, built once per preset source and
    /// version. Blocks while it builds.
    pub fn preset_index(
        &self,
        source: &ResolvedPresetSource,
    ) -> Result<Arc<PresetIndex>, CommandError> {
        let version = source.version.to_string();
        let cached = |presets: &Option<(PathBuf, String, Arc<PresetIndex>)>| {
            presets.as_ref().and_then(|(dir, cached_version, index)| {
                (dir == &source.profiles_dir && cached_version == &version)
                    .then(|| Arc::clone(index))
            })
        };
        if let Some(index) = cached(&lock(&self.presets)) {
            return Ok(index);
        }
        // One build at a time; a caller that waited finds it cached.
        let _building = lock(&self.presets_build);
        if let Some(index) = cached(&lock(&self.presets)) {
            return Ok(index);
        }
        let index = PresetIndex::build(&source.profiles_dir, &version, &CancelFlag::never())
            .map(Arc::new)
            .map_err(|error| match error {
                PresetIndexError::Unreadable => CommandError::preset_source_unavailable(
                    "farm3d can't read the preset source's profiles.",
                ),
                PresetIndexError::Cancelled => CommandError::internal(),
            })?;
        *lock(&self.presets) = Some((source.profiles_dir.clone(), version, Arc::clone(&index)));
        Ok(index)
    }

    // --- D6: geometry ------------------------------------------------------

    /// D6: Model Source Revision `revision_id`'s geometry and meshes, from
    /// the cache or read from its blob through a seekable handle. The
    /// blob's hash is verified in a separate streaming pass the first time
    /// its content is loaded. G-code has no geometry (`VALIDATION`).
    /// Blocks while it loads.
    pub fn revision_geometry(
        &self,
        revision_id: &str,
    ) -> Result<Arc<LoadedRevision>, CommandError> {
        let row = self
            .storage
            .read(|connection| {
                use rusqlite::OptionalExtension;
                connection
                    .query_row(
                        "SELECT content_sha256, format FROM model_source_revisions WHERE id = ?1",
                        [revision_id],
                        |row| Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?)),
                    )
                    .optional()
            })
            .map_err(storage_error)?;
        let (sha256, format) = row.ok_or_else(|| CommandError::not_found(revision_id))?;
        let format: ModelFormat =
            crate::spools::decode_enum(&format).map_err(|_| CommandError::database_corrupt())?;
        if format == ModelFormat::Gcode {
            return Err(CommandError::validation_at(
                "revisionId",
                "A G-code revision has no model geometry.",
            ));
        }
        self.geometry.get_or_load(&sha256, || {
            if !lock(&self.verified).contains(&sha256) {
                self.content.verify(&sha256)?;
                lock(&self.verified).insert(sha256.clone());
            }
            let file = self.content.open_unverified(&sha256)?;
            geometry::load_revision(BufReader::new(file), format, &CancelFlag::never())
                .map_err(geometry_error)
        })
    }

    // --- Startup -----------------------------------------------------------

    /// Attaches `app` and follows the Library: a new revision of a Model
    /// with a Preparation re-emits that Preparation (now stale, D5), and a
    /// deleted Model's cascaded Preparation goes out as removed. Call once
    /// the services are built; a second call only re-attaches.
    pub fn start(self: &Arc<Self>, app: &AppHandle<R>) {
        use tauri::Listener;
        if self.app.get().is_some() && self.listening.get().is_some() {
            return;
        }
        self.attach(app);
        if let Ok(Ok(preparations)) = self
            .storage
            .read(|connection| Ok(repository::list_preparations(connection)))
        {
            for preparation in preparations {
                self.remember_preparation(&preparation.model_id, &preparation.id);
            }
        }
        if self.listening.set(()).is_err() {
            return;
        }
        let services = Arc::downgrade(self);
        app.listen(crate::connections::supervisor::STATUS_EVENT, move |event| {
            // Every stream shares this event; only the Library's matter here,
            // so skip the rest without parsing them.
            if !event.payload().contains("\"library.") {
                return;
            }
            let Some(services) = services.upgrade() else {
                return;
            };
            let Ok(value) = serde_json::from_str::<serde_json::Value>(event.payload()) else {
                return;
            };
            let model_id = match value["type"].as_str() {
                Some("library.revision.created") => value["payload"]["modelId"].as_str(),
                Some("library.model.removed") => value["subject"]["id"].as_str(),
                _ => None,
            };
            let (Some(kind), Some(model_id)) = (value["type"].as_str(), model_id) else {
                return;
            };
            let removed = kind == "library.model.removed";
            let model_id = model_id.to_string();
            // Off the emitting thread: the Library may hold its stream lock.
            std::thread::spawn(move || services.follow_model(&model_id, removed));
        });
    }

    /// Re-emits Model `model_id`'s Preparation after its Model changed, or
    /// reports it removed with the Model.
    fn follow_model(&self, model_id: &str, removed: bool) {
        if removed {
            // Its Preparation and operations cascaded away: stop a job
            // still working for them.
            operations::abandon_deleted_jobs(self);
            if let Some(preparation_id) = self.forget_preparation_of(model_id) {
                self.publish(vec![events::preparation_removed(&preparation_id)]);
            }
            return;
        }
        let preparation = self
            .storage
            .read(|connection| Ok(repository::load_preparation_for_model(connection, model_id)));
        if let Ok(Ok(Some(preparation))) = preparation {
            self.publish(vec![events::preparation_changed(&preparation)]);
        }
    }

    /// D2: the startup probe, in the background after the command gate
    /// opens, then the sweep of extracted preset caches the current preset
    /// source doesn't use.
    pub fn probe_in_background(self: &Arc<Self>) {
        let services = Arc::clone(self);
        std::thread::spawn(move || {
            let Ok(runtime) = services.resolve_runtime(false) else {
                return;
            };
            let keep: Vec<&str> = runtime
                .preset_source
                .as_ref()
                .and_then(|source| source.cache_hash.as_deref())
                .into_iter()
                .collect();
            if let Err(error) = runtime::sweep_stale_profile_caches(&services.cache_dir, &keep) {
                eprintln!("farm3d: could not sweep the preset cache: {error}");
            }
        });
    }

    // --- Preparations by Model ---------------------------------------------

    pub(crate) fn remember_preparation(&self, model_id: &str, preparation_id: &str) {
        lock(&self.preparations_by_model).insert(model_id.to_string(), preparation_id.to_string());
    }

    pub(crate) fn forget_preparation_of(&self, model_id: &str) -> Option<String> {
        lock(&self.preparations_by_model).remove(model_id)
    }
}

/// A geometry load's failure as a command error.
fn geometry_error(error: InspectError) -> CommandError {
    match error {
        InspectError::Io(_) => CommandError::persistence_unavailable(),
        InspectError::Cancelled => CommandError::internal(),
        InspectError::UnsupportedFormat { reason, .. } | InspectError::InvalidContent(reason) => {
            CommandError::validation(format!(
                "farm3d can't read this revision's geometry: {reason}"
            ))
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Why the engine can't be used, for `SLICER_UNAVAILABLE`.
pub fn engine_reason(state: &EngineState) -> String {
    match state {
        EngineState::Available { .. } => "OrcaSlicer is available.".to_string(),
        EngineState::NotFound => "farm3d couldn't find OrcaSlicer on this computer.".to_string(),
        EngineState::UnsupportedVersion {
            version,
            executable_name,
        } => format!("{executable_name} is OrcaSlicer {version}, which farm3d doesn't support."),
        EngineState::ProbeFailed { reason, .. } => reason.clone(),
    }
}

/// Why the preset source can't be used, for `PRESET_SOURCE_UNAVAILABLE`.
pub fn preset_source_reason(state: &PresetSourceState) -> String {
    match state {
        PresetSourceState::Available { .. } => "The presets are available.".to_string(),
        PresetSourceState::NotConfigured => {
            "There is no OrcaSlicer to take presets from.".to_string()
        }
        PresetSourceState::PresetsUnreadable => {
            "This OrcaSlicer build stores its presets in a format farm3d can't read. Choose an OrcaSlicer 2.4 install or AppImage as the preset source.".to_string()
        }
        PresetSourceState::Unavailable { reason } => reason.clone(),
    }
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
