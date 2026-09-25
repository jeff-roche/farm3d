//! Format detection (D8) and inspection (D9-D12) of untrusted model files.
//!
//! Every reader streams: it keeps counters, bounds, and at most one
//! candidate thumbnail, never vertex arrays or whole parts. `inspect` always
//! reads the *staged* copy it is given, never the user's source file.
//!
//! [`read_mesh`] is the one exception (P5 D6): the same STL and 3MF readers,
//! with the same limits, also collect the vertices and triangles, capped by
//! [`MAX_MESH_TRIANGLES`] and [`MAX_MESH_VERTICES`].
//!
//! Messages in [`InspectError`] and [`ImportWarning`] name extensions,
//! line numbers, and 3MF part names only, never a filesystem path.

pub mod gcode;
pub mod png;
pub mod stl;
pub mod threemf;

use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::content::{CancelFlag, MAX_SOURCE_BYTES};
use super::{ImportWarning, ImportWarningCode, ModelFormat};

/// Stored on every revision so a later inspector can tell which rules
/// produced its `inspection`.
pub const INSPECTOR_VERSION: i64 = 1;

/// How much of the file detection reads (D8's G-code text window).
const DETECTION_WINDOW: usize = 64 * 1024;

/// P5 D6: the most triangles a revision's geometry may hold across its
/// objects, as placed in their object frames. It is the most a binary STL
/// under the 1 GiB import limit ([`MAX_SOURCE_BYTES`]) can hold, so every
/// importable STL fits and a 3MF can't expand past it through components.
pub const MAX_MESH_TRIANGLES: u64 = (MAX_SOURCE_BYTES - 84) / 50;
/// P5 D6: the most vertices a revision's geometry may hold.
pub const MAX_MESH_VERTICES: u64 = 3 * MAX_MESH_TRIANGLES;

/// P5 D6: one object's mesh in its own frame, in millimetres. Every
/// triangle's indices are below `positions.len()`.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct ObjectMesh {
    pub positions: Vec<[f32; 3]>,
    pub triangles: Vec<[u32; 3]>,
}

/// P5 D6: a model file's objects and build items, as [`read_mesh`] reads
/// them. An STL is object 1 with one identity build item.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshModel {
    /// Objects the build names, in the order it first names them.
    pub objects: Vec<MeshObject>,
    pub build_items: Vec<MeshBuildItem>,
}

/// A build-item object with its components flattened into its own frame.
#[derive(Clone, Debug, PartialEq)]
pub struct MeshObject {
    pub id: u32,
    pub name: Option<String>,
    pub mesh: ObjectMesh,
}

#[derive(Clone, Debug, PartialEq)]
pub struct MeshBuildItem {
    pub object_id: u32,
    /// The 3MF `transform` attribute order, in millimetres: a point maps to
    /// `x' = x·m[0] + y·m[3] + z·m[6] + m[9]`, and likewise for y and z.
    pub transform: [f64; 12],
    /// The Orca/Bambu plate (P4 [`Plate::index`]) that lists this item.
    pub plate_index: Option<u32>,
    pub printable: bool,
}

/// The identity in [`MeshBuildItem::transform`] order.
pub const IDENTITY_TRANSFORM: [f64; 12] =
    [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0];

/// P5 D6: reads the geometry of an STL or 3MF with the inspection readers,
/// which apply the same limits and rejections as [`inspect`]. G-code has
/// no geometry and is `UNSUPPORTED_FORMAT`.
pub fn read_mesh<R: Read + Seek>(
    mut reader: R,
    format: ModelFormat,
    cancel: &CancelFlag,
) -> Result<MeshModel, InspectError> {
    if cancel.is_cancelled() {
        return Err(InspectError::Cancelled);
    }
    match format {
        ModelFormat::Stl => {
            let file_len = reader.seek(SeekFrom::End(0))?;
            reader.seek(SeekFrom::Start(0))?;
            let mut head = Vec::with_capacity(DETECTION_WINDOW);
            reader
                .by_ref()
                .take(DETECTION_WINDOW as u64)
                .read_to_end(&mut head)?;
            reader.seek(SeekFrom::Start(0))?;
            let encoding = stl::detect(&head, file_len)
                .ok_or_else(|| InspectError::invalid("This file isn't an STL."))?;
            let mesh = stl::read_mesh(reader, file_len, encoding, cancel)?;
            Ok(MeshModel {
                objects: vec![MeshObject {
                    id: 1,
                    name: None,
                    mesh,
                }],
                build_items: vec![MeshBuildItem {
                    object_id: 1,
                    transform: IDENTITY_TRANSFORM,
                    plate_index: None,
                    printable: true,
                }],
            })
        }
        ModelFormat::ThreeMf => threemf::read_mesh(reader, cancel),
        ModelFormat::Gcode => Err(InspectError::unsupported("G-code has no model geometry.")),
    }
}

/// What [`detect`] found: the format, the STL encoding for STL, and any
/// `EXTENSION_MISMATCH` warning.
#[derive(Clone, Debug, PartialEq)]
pub struct Detected {
    pub format: ModelFormat,
    pub stl_encoding: Option<StlEncoding>,
    pub warnings: Vec<ImportWarning>,
}

/// A successful inspection. `warnings` includes the detection warnings.
#[derive(Clone, Debug, PartialEq)]
pub struct InspectOutcome {
    pub inspection: Inspection,
    pub summary: InspectionSummary,
    pub thumbnail: Option<ThumbnailBytes>,
    pub warnings: Vec<ImportWarning>,
}

/// The one preferred embedded PNG (D12): at most 1 MiB and 1024×1024.
/// `origin_part` is the 3MF part name, or `line <n>` for a G-code block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ThumbnailBytes {
    pub origin_part: String,
    pub width: u32,
    pub height: u32,
    pub bytes: Vec<u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum InspectError {
    /// D8/D10: not a format farm3d reads, or a 3MF that requires an
    /// extension farm3d doesn't implement (`extensions` holds the
    /// `requiredextensions` prefixes).
    UnsupportedFormat {
        reason: String,
        extensions: Vec<String>,
    },
    /// The format was recognised but the content is unusable.
    InvalidContent(String),
    Cancelled,
    /// Reading the staged file failed.
    Io(io::ErrorKind),
}

impl InspectError {
    /// The `ImportItemErrorCode` spelling this error maps to.
    pub fn code(&self) -> &'static str {
        match self {
            Self::UnsupportedFormat { .. } => "UNSUPPORTED_FORMAT",
            Self::InvalidContent(_) => "INVALID_CONTENT",
            Self::Cancelled => "CANCELLED",
            Self::Io(_) => "SOURCE_UNREADABLE",
        }
    }

    pub fn message(&self) -> String {
        match self {
            Self::UnsupportedFormat { reason, .. } => reason.clone(),
            Self::InvalidContent(reason) => reason.clone(),
            Self::Cancelled => "The inspection was cancelled.".to_string(),
            Self::Io(_) => "farm3d couldn't read the file.".to_string(),
        }
    }

    pub(crate) fn unsupported(reason: impl Into<String>) -> Self {
        Self::UnsupportedFormat {
            reason: reason.into(),
            extensions: Vec::new(),
        }
    }

    pub(crate) fn invalid(reason: impl Into<String>) -> Self {
        Self::InvalidContent(reason.into())
    }
}

impl From<io::Error> for InspectError {
    fn from(error: io::Error) -> Self {
        Self::Io(error.kind())
    }
}

/// Axis-aligned bounds in millimetres.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, TS)]
#[ts(export_to = "domain/BoundsMm.ts")]
pub struct BoundsMm {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl BoundsMm {
    pub(crate) fn point(point: [f64; 3]) -> Self {
        Self {
            min: point,
            max: point,
        }
    }

    pub(crate) fn include(&mut self, point: [f64; 3]) {
        for (axis, value) in point.into_iter().enumerate() {
            self.min[axis] = self.min[axis].min(value);
            self.max[axis] = self.max[axis].max(value);
        }
    }

    pub(crate) fn union(&mut self, other: &BoundsMm) {
        self.include(other.min);
        self.include(other.max);
    }

    pub(crate) fn scaled(self, factor: f64) -> Self {
        Self {
            min: self.min.map(|value| value * factor),
            max: self.max.map(|value| value * factor),
        }
    }
}

/// Accumulates bounds over points without storing them.
pub(crate) fn include_point(bounds: &mut Option<BoundsMm>, point: [f64; 3]) {
    match bounds {
        Some(existing) => existing.include(point),
        None => *bounds = Some(BoundsMm::point(point)),
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StlEncoding.ts")]
pub enum StlEncoding {
    Ascii,
    Binary,
}

/// D9's output. STL has no units; millimetres are assumed.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StlInspection.ts")]
pub struct StlInspection {
    pub encoding: StlEncoding,
    /// The ASCII `solid <name>`, when the file names one.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub solid_name: Option<String>,
    #[ts(type = "number")]
    pub triangle_count: u64,
    pub bounds_mm: BoundsMm,
    #[ts(type = "true")]
    pub units_assumed: bool,
}

/// D10's output.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ThreeMfInspection.ts")]
pub struct ThreeMfInspection {
    /// The model's `unit` attribute as written. `boundsMm` is converted.
    pub unit: String,
    /// The start part's `Application` metadata, verbatim.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub producer: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub title: Option<String>,
    /// `<object>` elements in the start part.
    pub object_count: u32,
    pub build_item_count: u32,
    /// Triangles as placed: every build item and component placement counts
    /// its mesh's triangles again, matching `boundsMm`, which is also per
    /// placement. A mesh used twice counts twice.
    #[ts(type = "number")]
    pub triangle_count: u64,
    /// The union of every build item's transformed object box (the corner
    /// method), so it is conservative for rotated items.
    pub bounds_mm: BoundsMm,
    pub plates: Vec<Plate>,
    /// `requiredextensions` prefixes, as written, across every model part.
    pub required_extensions: Vec<String>,
    pub unsupported: Vec<UnsupportedEntry>,
    pub thumbnails: Vec<ThumbnailInfo>,
}

/// An Orca/Bambu plate from `Metadata/model_settings.config`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Plate.ts")]
pub struct Plate {
    pub index: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub name: Option<String>,
    pub object_ids: Vec<u32>,
}

/// Something in a 3MF that farm3d keeps but won't use (D10).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, PartialOrd, Ord, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/UnsupportedEntry.ts")]
pub struct UnsupportedEntry {
    pub part: String,
    pub code: UnsupportedCode,
    pub detail: String,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "domain/UnsupportedCode.ts"
)]
pub enum UnsupportedCode {
    /// `Metadata/project_settings.config`, `Metadata/Slic3r_PE*.config`.
    SlicerSettings,
    /// Any other `Metadata/*.config|.gcode|.txt|.xml|.json` part.
    SlicerMetadata,
    /// `Metadata/*.gcode`, such as `plate_1.gcode`.
    EmbeddedGcode,
    /// A layer-height profile part.
    LayerHeightProfile,
    /// Keys in `model_settings.config` other than plate grouping.
    PerObjectSettings,
    /// Paint and MMU segmentation triangle attributes.
    Paint,
    /// Material and colour groups, or a required Materials extension.
    Materials,
}

#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ThumbnailImageFormat.ts")]
pub enum ThumbnailImageFormat {
    Png,
    Qoi,
    Jpg,
}

/// An embedded image found during inspection. 3MF images name their
/// `part`; G-code blocks name their `line`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ThumbnailInfo.ts")]
pub struct ThumbnailInfo {
    pub format: ThumbnailImageFormat,
    pub width: u32,
    pub height: u32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub part: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional, type = "number")]
    pub line: Option<u64>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/Producer.ts")]
pub struct Producer {
    pub name: String,
    pub version: String,
}

/// D11: one allowlisted `key = value` or `key: value` comment, verbatim.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/GcodeClaim.ts")]
pub struct GcodeClaim {
    pub key: String,
    pub value: String,
    #[ts(type = "number")]
    pub line: u64,
}

/// D11's output. Claims are untrusted text; nothing here is a Printer,
/// nozzle, or material fact.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/GcodeInspection.ts")]
pub struct GcodeInspection {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub producer: Option<Producer>,
    pub claims: Vec<GcodeClaim>,
    #[ts(type = "false")]
    pub trusted: bool,
    #[ts(type = "number")]
    pub line_count: u64,
    #[ts(type = "number")]
    pub command_count: u64,
    pub tools_used: Vec<u32>,
    pub relative_positioning_seen: bool,
    pub relative_extrusion_seen: bool,
    /// The extents of `G0`/`G1` X, Y, and Z words. Omitted once `G91` is
    /// seen (relative moves make them ambiguous) or when an axis never
    /// appears.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub observed_bounds_mm: Option<BoundsMm>,
    pub thumbnails: Vec<ThumbnailInfo>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(tag = "format")]
#[ts(tag = "format", export_to = "domain/Inspection.ts")]
pub enum Inspection {
    #[serde(rename = "stl")]
    #[ts(rename = "stl")]
    Stl(StlInspection),
    #[serde(rename = "3mf")]
    #[ts(rename = "3mf")]
    ThreeMf(ThreeMfInspection),
    #[serde(rename = "gcode")]
    #[ts(rename = "gcode")]
    Gcode(GcodeInspection),
}

/// The list-display projection of an [`Inspection`] (spec §Domain types).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(tag = "format")]
#[ts(tag = "format", export_to = "domain/InspectionSummary.ts")]
pub enum InspectionSummary {
    #[serde(rename = "stl")]
    #[ts(rename = "stl")]
    Stl(StlSummary),
    #[serde(rename = "3mf")]
    #[ts(rename = "3mf")]
    ThreeMf(ThreeMfSummary),
    #[serde(rename = "gcode")]
    #[ts(rename = "gcode")]
    Gcode(GcodeSummary),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StlSummary.ts")]
pub struct StlSummary {
    #[ts(type = "number")]
    pub triangle_count: u64,
    pub bounds_mm: BoundsMm,
    #[ts(type = "true")]
    pub units_assumed: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ThreeMfSummary.ts")]
pub struct ThreeMfSummary {
    pub object_count: u32,
    pub plate_count: u32,
    #[ts(type = "number")]
    pub triangle_count: u64,
    pub bounds_mm: BoundsMm,
    pub unsupported_count: u32,
}

/// `claimed*` values are verbatim claims, for list display only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/GcodeSummary.ts")]
pub struct GcodeSummary {
    pub producer: Option<Producer>,
    #[ts(type = "number")]
    pub line_count: u64,
    pub claimed_printer_model: Option<String>,
    pub claimed_estimated_time: Option<String>,
}

impl Inspection {
    pub fn summary(&self) -> InspectionSummary {
        match self {
            Self::Stl(stl) => InspectionSummary::Stl(StlSummary {
                triangle_count: stl.triangle_count,
                bounds_mm: stl.bounds_mm,
                units_assumed: true,
            }),
            Self::ThreeMf(model) => InspectionSummary::ThreeMf(ThreeMfSummary {
                object_count: model.object_count,
                plate_count: model.plates.len() as u32,
                triangle_count: model.triangle_count,
                bounds_mm: model.bounds_mm,
                unsupported_count: model.unsupported.len() as u32,
            }),
            Self::Gcode(gcode) => {
                let claim = |key: &str| {
                    gcode
                        .claims
                        .iter()
                        .find(|claim| claim.key == key)
                        .map(|claim| claim.value.clone())
                };
                InspectionSummary::Gcode(GcodeSummary {
                    producer: gcode.producer.clone(),
                    line_count: gcode.line_count,
                    claimed_printer_model: claim("printer_model"),
                    claimed_estimated_time: claim("estimated printing time (normal mode)")
                        .or_else(|| claim("TIME")),
                })
            }
        }
    }
}

/// D8 over the file's own name. Import callers that detect a staged copy
/// use [`detect_named`] with the source basename instead, so the
/// `EXTENSION_MISMATCH` check sees the user's extension.
pub fn detect(path: &Path) -> Result<Detected, InspectError> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    detect_named(path, &file_name)
}

/// D8: detects the format of `path` from its content, and warns when
/// `file_name`'s extension disagrees.
pub fn detect_named(path: &Path, file_name: &str) -> Result<Detected, InspectError> {
    let mut file = File::open(path)?;
    let file_len = file.metadata()?.len();
    let mut head = Vec::with_capacity(DETECTION_WINDOW);
    file.by_ref()
        .take(DETECTION_WINDOW as u64)
        .read_to_end(&mut head)?;

    let (format, stl_encoding) = if head.starts_with(b"PK\x03\x04") {
        if threemf::is_3mf_package(file)? {
            (ModelFormat::ThreeMf, None)
        } else {
            return Err(InspectError::unsupported(
                "This ZIP file isn't a 3MF package.",
            ));
        }
    } else if head.starts_with(b"GCDE") {
        return Err(InspectError::unsupported(
            "Binary G-code isn't supported yet.",
        ));
    } else if let Some(encoding) = stl::detect(&head, file_len) {
        (ModelFormat::Stl, Some(encoding))
    } else if gcode::looks_like_gcode(&head) {
        (ModelFormat::Gcode, None)
    } else if let Some(reason) = has_extension(file_name, "stl")
        .then(|| stl::truncated_binary_reason(&head, file_len))
        .flatten()
    {
        return Err(InspectError::InvalidContent(reason));
    } else {
        return Err(InspectError::unsupported(
            "farm3d can't read this file. It reads STL, 3MF, and G-code.",
        ));
    };

    let warnings = extension_mismatch(file_name, format).into_iter().collect();
    Ok(Detected {
        format,
        stl_encoding,
        warnings,
    })
}

fn has_extension(file_name: &str, extension: &str) -> bool {
    file_name
        .rsplit_once('.')
        .is_some_and(|(_, actual)| actual.eq_ignore_ascii_case(extension))
}

/// D8: an extension that disagrees with the detected format is a warning,
/// not a rejection. A name with no extension doesn't disagree.
pub fn extension_mismatch(file_name: &str, format: ModelFormat) -> Option<ImportWarning> {
    let (_, extension) = file_name.rsplit_once('.')?;
    let extension = extension.to_ascii_lowercase();
    let (expected, label): (&[&str], &str) = match format {
        ModelFormat::Stl => (&["stl"], "STL"),
        ModelFormat::ThreeMf => (&["3mf"], "3MF"),
        ModelFormat::Gcode => (&["gcode", "gco", "g"], "G-code"),
    };
    (!expected.contains(&extension.as_str())).then(|| {
        ImportWarning::new(
            ImportWarningCode::ExtensionMismatch,
            format!("The file name ends in .{extension}, but its content is {label}."),
        )
    })
}

/// Inspects the staged file at `path` as `detected` says. The outcome's
/// warnings start with `detected.warnings`.
pub fn inspect(
    path: &Path,
    detected: &Detected,
    cancel: &CancelFlag,
) -> Result<InspectOutcome, InspectError> {
    if cancel.is_cancelled() {
        return Err(InspectError::Cancelled);
    }
    let mut warnings = detected.warnings.clone();
    let (inspection, thumbnail) = match detected.format {
        ModelFormat::Stl => {
            let encoding = match detected.stl_encoding {
                Some(encoding) => encoding,
                None => {
                    let head = read_head(path)?;
                    let len = std::fs::metadata(path)?.len();
                    stl::detect(&head, len)
                        .ok_or_else(|| InspectError::invalid("This file isn't an STL."))?
                }
            };
            let (stl, stl_warnings) = stl::inspect(path, encoding, cancel)?;
            warnings.extend(stl_warnings);
            (Inspection::Stl(stl), None)
        }
        ModelFormat::ThreeMf => {
            let (model, thumbnail, model_warnings) = threemf::inspect(path, cancel)?;
            warnings.extend(model_warnings);
            (Inspection::ThreeMf(model), thumbnail)
        }
        ModelFormat::Gcode => {
            let (gcode, thumbnail, gcode_warnings) = gcode::inspect(path, cancel)?;
            warnings.extend(gcode_warnings);
            (Inspection::Gcode(gcode), thumbnail)
        }
    };
    Ok(InspectOutcome {
        summary: inspection.summary(),
        inspection,
        thumbnail,
        warnings,
    })
}

fn read_head(path: &Path) -> Result<Vec<u8>, InspectError> {
    let mut head = Vec::with_capacity(DETECTION_WINDOW);
    File::open(path)?
        .take(DETECTION_WINDOW as u64)
        .read_to_end(&mut head)?;
    Ok(head)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_mismatch_warns_only_when_a_known_format_disagrees() {
        assert!(extension_mismatch("cube.stl", ModelFormat::Stl).is_none());
        assert!(extension_mismatch("CUBE.STL", ModelFormat::Stl).is_none());
        assert!(extension_mismatch("print.gco", ModelFormat::Gcode).is_none());
        assert!(extension_mismatch("no-extension", ModelFormat::Gcode).is_none());
        let warning = extension_mismatch("cube.3mf", ModelFormat::Stl).unwrap();
        assert_eq!(warning.code, ImportWarningCode::ExtensionMismatch);
        assert!(warning.message.contains(".3mf"));
    }

    #[test]
    fn detect_named_uses_the_given_name_for_the_extension_check() {
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("0.part");
        std::fs::write(&staged, b"G28\nG1 X1 Y1\n").unwrap();
        let detected = detect_named(&staged, "print.gcode").unwrap();
        assert_eq!(detected.format, ModelFormat::Gcode);
        assert!(detected.warnings.is_empty());
        let detected = detect_named(&staged, "print.stl").unwrap();
        assert_eq!(
            detected.warnings[0].code,
            ImportWarningCode::ExtensionMismatch
        );
    }

    #[test]
    fn inspect_carries_detection_warnings_and_honours_cancellation() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("print.stl");
        std::fs::write(&path, b"G28\nG1 X1 Y1 Z1\n").unwrap();
        let detected = detect(&path).unwrap();
        let outcome = inspect(&path, &detected, &CancelFlag::never()).unwrap();
        assert_eq!(outcome.warnings, detected.warnings);
        assert!(matches!(outcome.summary, InspectionSummary::Gcode(_)));

        let (sender, receiver) = tokio::sync::watch::channel(false);
        sender.send(true).unwrap();
        assert_eq!(
            inspect(&path, &detected, &CancelFlag::new(receiver)),
            Err(InspectError::Cancelled)
        );
    }

    #[test]
    fn a_zip_without_content_types_is_unsupported() {
        use std::io::Write;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("archive.zip");
        let mut writer = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        writer
            .start_file("readme.txt", zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"hello").unwrap();
        writer.finish().unwrap();
        let error = detect(&path).unwrap_err();
        assert_eq!(error.code(), "UNSUPPORTED_FORMAT");
    }

    #[test]
    fn a_binary_stl_cut_mid_record_is_truncated_only_when_named_stl() {
        let mut bytes = b"cut".to_vec();
        bytes.resize(80, 0);
        bytes.extend_from_slice(&12u32.to_le_bytes());
        bytes.extend(std::iter::repeat_n(0u8, 3 * 50 + 17));
        let dir = tempfile::tempdir().unwrap();
        let staged = dir.path().join("0.part");
        std::fs::write(&staged, &bytes).unwrap();

        let error = detect_named(&staged, "cut.STL").unwrap_err();
        assert_eq!(error.code(), "INVALID_CONTENT");
        assert!(error.message().contains("truncated"), "{}", error.message());
        assert_eq!(
            detect_named(&staged, "cut.bin").unwrap_err().code(),
            "UNSUPPORTED_FORMAT"
        );
    }

    #[test]
    fn unreadable_bytes_are_unsupported() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("noise.bin");
        std::fs::write(&path, [0u8, 1, 2, 3, 0xff, 0xfe]).unwrap();
        assert_eq!(detect(&path).unwrap_err().code(), "UNSUPPORTED_FORMAT");
    }

    #[test]
    fn gcode_summary_takes_claimed_values_verbatim() {
        let inspection = Inspection::Gcode(GcodeInspection {
            producer: None,
            claims: vec![
                GcodeClaim {
                    key: "TIME".into(),
                    value: "1234".into(),
                    line: 2,
                },
                GcodeClaim {
                    key: "printer_model".into(),
                    value: "MK4S".into(),
                    line: 9,
                },
            ],
            trusted: false,
            line_count: 10,
            command_count: 5,
            tools_used: vec![],
            relative_positioning_seen: false,
            relative_extrusion_seen: false,
            observed_bounds_mm: None,
            thumbnails: vec![],
        });
        assert_eq!(
            serde_json::to_value(inspection.summary()).unwrap(),
            serde_json::json!({
                "format": "gcode",
                "producer": null,
                "lineCount": 10,
                "claimedPrinterModel": "MK4S",
                "claimedEstimatedTime": "1234",
            })
        );
    }
}
