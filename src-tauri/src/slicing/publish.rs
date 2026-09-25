//! D11–D13: turning a finished slice run into a published Slice Revision,
//! or into a failed or cancelled operation that keeps only its log.
//!
//! - **Validation** ([`validate_output`], D11 checks 3–6). `out/plate_1.gcode`
//!   must be a regular file (not a symlink) of at most 1 GiB. It is staged
//!   first, and the staged copy is what the P4 G-code inspector reads: it
//!   must be G-code, name OrcaSlicer as its producer, hold at least one
//!   command, keep its printed bounds ([`super::printed_bounds`]) inside
//!   the target's printable area (2 mm of XY tolerance) and height (0.05 mm
//!   of Z tolerance), and claim the machine and filament presets farm3d
//!   passed. Untrackable positioning skips the bounds check, and the
//!   manifest says so. Checks 1–2 (the return code and a missing output) are
//!   [`process::outcome`]'s.
//! - **Estimates** ([`estimates_from_claims`], D12) come from the same
//!   inspection's claims. The inspector streams the file one bounded line at
//!   a time and keeps one value per allowlisted key, so no G-code is ever
//!   held in memory whole.
//! - **Publishing** ([`publish`], D13) stages the plate 3MF, the three flat
//!   presets, the invocation manifest, and the log beside the G-code under
//!   the operation's staging key, then one `place_and_commit` inserts the
//!   blobs, the `slice_revisions` row with its six `slice_revision_blobs`
//!   rows, and moves the operation to `succeeded`.
//! - **No revision** ([`record_unpublished`]): a failed or cancelled
//!   operation commits only its log blob, referenced from
//!   `slice_operations.log_sha256`.
//!
//! [`finish_run`] strings these together for the operation layer. Every
//! function blocks; the caller runs it on a blocking thread.

use std::fs::{self, File};
use std::io::Read;
use std::path::Path;

use crate::catalog::BedShape;
use crate::library::content::{
    CancelFlag, ContentError, ContentStore, StagedFile, MAX_SOURCE_BYTES,
};
use crate::library::formats::{gcode, BoundsMm, GcodeClaim, InspectError};
use crate::persistence::{RepositoryError, Storage};

use super::facts::{Farm3dFacts, ProfileSnapshot};
use super::invocation::{
    runtime_info, EngineIdentity, InputHashes, InvocationManifest, PresetSourceIdentity, WorkDir,
};
use super::printed_bounds::{BoundsCheck, PrintedBounds, PrintedBoundsScanner};
use super::process::{self, output_missing, SliceLog, SliceOutcome, SliceRun};
use super::repository::{
    insert_farm3d_revision, load_operation, transition_operation, NewFarm3dRevision,
    OperationTransition,
};
use super::{
    new_slice_revision_id, ClaimedEstimateSource, ClaimedEstimates, SliceEstimateSource,
    SliceEstimates, SliceFailure, SliceFailureCode, SliceOperationRecord, SlicePlateRef,
    SliceRevisionBlobRole, SliceRevisionRecord, SliceRevisionTarget,
};

/// D11 check 3: the largest G-code farm3d publishes.
pub const MAX_OUTPUT_BYTES: u64 = MAX_SOURCE_BYTES;

/// D11 check 5: how far outside the printable area, on X and Y, the
/// printed bounds may reach.
pub const BOUNDS_XY_TOLERANCE_MM: f64 = 2.0;

/// D11 check 5: how far above the printable height the printed bounds may
/// reach.
pub const BOUNDS_Z_TOLERANCE_MM: f64 = 0.05;

/// D11 check 4: the producer the G-code must name.
pub const ENGINE_PRODUCER: &str = "OrcaSlicer";

/// How much of the output the "is it G-code at all" check reads.
const GCODE_HEAD_BYTES: u64 = 64 * 1024;

/// The staged file index (`<index>.part`) of each file copied from disk.
const GCODE_INDEX: usize = 0;
const PLATE_3MF_INDEX: usize = 1;
const MACHINE_INDEX: usize = 2;
const PROCESS_INDEX: usize = 3;
const FILAMENT_INDEX: usize = 4;

/// The staged names of the files farm3d writes from memory.
const MANIFEST_NAME: &str = "manifest.json";
const LOG_NAME: &str = "log.txt";

const PRINTER_PRESET_CLAIM: &str = "printer_settings_id";
const FILAMENT_PRESET_CLAIM: &str = "filament_settings_id";

/// Everything a farm3d revision records besides its output, known when
/// the operation started.
#[derive(Clone, Debug)]
pub struct PublishInputs {
    /// The operation, which is also the content-store staging key.
    pub operation_id: String,
    pub target: SliceRevisionTarget,
    pub facts: Farm3dFacts,
    pub engine: EngineIdentity,
    pub preset_source: PresetSourceIdentity,
    /// The machine-preset keys written from Printer Profile overrides (D4).
    pub profile_overrides: Vec<String>,
}

/// A G-code that passed D11 checks 3–6, staged, with its estimates and
/// whether the bounds check ran.
#[derive(Debug)]
pub struct ValidatedOutput {
    gcode: StagedFile,
    estimates: SliceEstimates,
    bounds_check: BoundsCheck,
}

impl ValidatedOutput {
    pub fn estimates(&self) -> &SliceEstimates {
        &self.estimates
    }

    pub fn bounds_check(&self) -> &BoundsCheck {
        &self.bounds_check
    }
}

/// Why [`validate_output`] published nothing.
#[derive(Debug)]
pub enum OutputRejection {
    /// The output failed a D11 check; the operation fails with this.
    Invalid(SliceFailure),
    /// `cancel` was raised while the output was staged or inspected.
    Cancelled,
    /// The content store itself failed; the operation's fate is the
    /// caller's.
    Store(ContentError),
}

/// How an operation that publishes nothing ends.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Unpublished {
    Failed(SliceFailure),
    Cancelled,
}

/// What [`finish_run`] did with a run. Made once per run and moved
/// straight to the caller, so the size of the revision doesn't matter.
#[derive(Debug)]
#[allow(clippy::large_enum_variant)]
pub enum FinishedRun {
    /// The revision, and the operation as the same commit left it
    /// (`succeeded`), so its event never needs a second read.
    Published {
        revision: SliceRevisionRecord,
        operation: SliceOperationRecord,
    },
    /// The operation is `failed` or `cancelled`, with its log.
    Unpublished(SliceOperationRecord),
}

fn output_invalid(reason: impl Into<String>) -> SliceFailure {
    let reason = reason.into();
    SliceFailure {
        code: SliceFailureCode::OutputInvalid {
            reason: reason.clone(),
        },
        message: reason,
    }
}

// The `outputInvalid` reasons (D11 checks 3–6).
pub const NOT_A_FILE: &str = "OrcaSlicer's G-code isn't a regular file.";
pub const TOO_LARGE: &str = "OrcaSlicer's G-code is larger than 1 GiB.";
pub const UNREADABLE: &str = "OrcaSlicer's G-code couldn't be read.";
pub const NOT_GCODE: &str = "OrcaSlicer's output isn't G-code.";
pub const NOT_INSPECTABLE: &str = "OrcaSlicer's G-code couldn't be inspected: ";
pub const WRONG_PRODUCER: &str = "The G-code doesn't say OrcaSlicer wrote it.";
pub const NO_COMMANDS: &str = "The G-code has no commands.";
pub const EMPTY_BED: &str = "The target printer has no printable area.";
pub const OUTSIDE_AREA: &str = "The G-code prints outside the printable area.";
pub const ABOVE_HEIGHT: &str = "The G-code prints above the printable height.";
pub const WRONG_PRINTER_PRESET: &str =
    "The G-code names a different printer preset than farm3d passed.";
pub const WRONG_FILAMENT_PRESET: &str =
    "The G-code names a different filament preset than farm3d passed.";

/// D11 checks 3–6 on `work`'s `out/plate_1.gcode`, for an operation whose
/// run ended with return code 0. The G-code is staged under
/// `operation_id`; on any rejection that staging is discarded.
pub fn validate_output(
    store: &ContentStore,
    operation_id: &str,
    work: &WorkDir,
    target: &SliceRevisionTarget,
    cancel: &CancelFlag,
) -> Result<ValidatedOutput, OutputRejection> {
    let result = stage_and_check(store, operation_id, work, target, cancel);
    if result.is_err() {
        store.discard_staging(operation_id);
    }
    result
}

fn stage_and_check(
    store: &ContentStore,
    operation_id: &str,
    work: &WorkDir,
    target: &SliceRevisionTarget,
    cancel: &CancelFlag,
) -> Result<ValidatedOutput, OutputRejection> {
    let invalid = |reason: &str| OutputRejection::Invalid(output_invalid(reason));
    let path = work.gcode();
    let metadata = match fs::symlink_metadata(&path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            return Err(OutputRejection::Invalid(output_missing()))
        }
        Err(_) => return Err(invalid(UNREADABLE)),
    };
    if !metadata.file_type().is_file() {
        return Err(invalid(NOT_A_FILE));
    }
    if metadata.len() > MAX_OUTPUT_BYTES {
        return Err(invalid(TOO_LARGE));
    }
    let gcode = store
        .stage_from_path(&path, operation_id, GCODE_INDEX, cancel, &mut |_, _| {})
        .map_err(|error| match error {
            ContentError::TooLarge => invalid(TOO_LARGE),
            ContentError::NotAFile => invalid(NOT_A_FILE),
            ContentError::Unreadable(_) | ContentError::ChangedDuringRead => invalid(UNREADABLE),
            ContentError::Cancelled => OutputRejection::Cancelled,
            other => OutputRejection::Store(other),
        })?;
    let (estimates, bounds_check) = check_gcode(&gcode.path, target, cancel)?;
    Ok(ValidatedOutput {
        gcode,
        estimates,
        bounds_check,
    })
}

/// D11 checks 4–6 on the staged G-code at `path`, with its D12 estimates
/// and how check 5 went. The inspector's one streaming pass also feeds the
/// printed-bounds scanner.
fn check_gcode(
    path: &Path,
    target: &SliceRevisionTarget,
    cancel: &CancelFlag,
) -> Result<(SliceEstimates, BoundsCheck), OutputRejection> {
    let invalid = |reason: &str| OutputRejection::Invalid(output_invalid(reason));
    let mut head = Vec::new();
    File::open(path)
        .and_then(|file| file.take(GCODE_HEAD_BYTES).read_to_end(&mut head))
        .map_err(|_| invalid(UNREADABLE))?;
    if !gcode::looks_like_gcode(&head) {
        return Err(invalid(NOT_GCODE));
    }
    let mut printed = PrintedBoundsScanner::new();
    let (inspection, _, _) = gcode::inspect_visiting(path, cancel, &mut |line| printed.line(line))
        .map_err(|error| match error {
            InspectError::Cancelled => OutputRejection::Cancelled,
            other => OutputRejection::Invalid(output_invalid(format!(
                "{NOT_INSPECTABLE}{}",
                other.message()
            ))),
        })?;
    if inspection
        .producer
        .as_ref()
        .is_none_or(|producer| producer.name != ENGINE_PRODUCER)
    {
        return Err(invalid(WRONG_PRODUCER));
    }
    // D11 restated on purpose: today the inspector already rejects a
    // G-code without commands, so this can't fail, but check 4 must hold
    // even if the inspector's rules change.
    if inspection.command_count == 0 {
        return Err(invalid(NO_COMMANDS));
    }
    let bounds_check = match printed.finish() {
        PrintedBounds::Tracked { scope, bounds } => {
            if let Some(bounds) = bounds {
                check_printed_bounds(&bounds, &target.profile).map_err(invalid)?;
            }
            BoundsCheck::Checked { scope }
        }
        PrintedBounds::Untracked { reason } => BoundsCheck::Skipped {
            reason: reason.to_string(),
        },
    };
    if claimed_preset(&inspection.claims, PRINTER_PRESET_CLAIM)
        != Some(target.machine_preset.as_str())
    {
        return Err(invalid(WRONG_PRINTER_PRESET));
    }
    if claimed_preset(&inspection.claims, FILAMENT_PRESET_CLAIM)
        != Some(target.filament_preset.as_str())
    {
        return Err(invalid(WRONG_FILAMENT_PRESET));
    }
    Ok((estimates_from_claims(&inspection.claims), bounds_check))
}

/// D11 check 5: the printed bounds lie within the bed grown by
/// [`BOUNDS_XY_TOLERANCE_MM`], and no higher than the printable height plus
/// [`BOUNDS_Z_TOLERANCE_MM`]. Returns the failure reason.
fn check_printed_bounds(bounds: &BoundsMm, profile: &ProfileSnapshot) -> Result<(), &'static str> {
    let (min_x, min_y, max_x, max_y) = printable_xy(&profile.bed_shape).ok_or(EMPTY_BED)?;
    let tolerance = BOUNDS_XY_TOLERANCE_MM;
    let within = bounds.min[0] >= min_x - tolerance
        && bounds.min[1] >= min_y - tolerance
        && bounds.max[0] <= max_x + tolerance
        && bounds.max[1] <= max_y + tolerance;
    if !within {
        return Err(OUTSIDE_AREA);
    }
    if bounds.max[2] > profile.printable_height_mm + BOUNDS_Z_TOLERANCE_MM {
        return Err(ABOVE_HEIGHT);
    }
    Ok(())
}

/// The bed's XY extent: the rectangle, or a polygon's bounding box. A
/// polygon of fewer than three points has no area, which the catalog
/// never produces but a stored snapshot could hold; it is `None`, and the
/// check fails rather than passing unchecked.
fn printable_xy(shape: &BedShape) -> Option<(f64, f64, f64, f64)> {
    match shape {
        BedShape::Rectangular {
            width_mm,
            depth_mm,
            origin_x_mm,
            origin_y_mm,
        } => Some((
            *origin_x_mm,
            *origin_y_mm,
            origin_x_mm + width_mm,
            origin_y_mm + depth_mm,
        )),
        BedShape::Polygon { points } if points.len() < 3 => None,
        BedShape::Polygon { points } => Some(points.iter().fold(
            (
                f64::INFINITY,
                f64::INFINITY,
                f64::NEG_INFINITY,
                f64::NEG_INFINITY,
            ),
            |(min_x, min_y, max_x, max_y), point| {
                (
                    min_x.min(point.x_mm),
                    min_y.min(point.y_mm),
                    max_x.max(point.x_mm),
                    max_y.max(point.y_mm),
                )
            },
        )),
    }
}

fn claim<'a>(claims: &'a [GcodeClaim], key: &str) -> Option<&'a str> {
    claims
        .iter()
        .find(|claim| claim.key == key)
        .map(|claim| claim.value.trim())
}

/// A preset-name claim with one pair of surrounding quotes removed (the
/// G-code writes `"Elegoo PLA @ECC"` quoted, spike Gate B).
fn claimed_preset<'a>(claims: &'a [GcodeClaim], key: &str) -> Option<&'a str> {
    claim(claims, key).map(|value| {
        value
            .strip_prefix('"')
            .and_then(|inner| inner.strip_suffix('"'))
            .unwrap_or(value)
    })
}

/// D12: the estimates a G-code's claims state, each `null` when its claim
/// is missing or doesn't parse.
pub fn estimates_from_claims(claims: &[GcodeClaim]) -> SliceEstimates {
    SliceEstimates {
        print_seconds: claim(claims, "estimated printing time (normal mode)")
            .and_then(parse_duration),
        filament_grams: claim(claims, "filament used [g]").and_then(parse_total),
        filament_mm: claim(claims, "filament used [mm]").and_then(parse_total),
        layer_count: claim(claims, "total layer number").and_then(|text| text.parse().ok()),
        max_z_mm: claim(claims, "max_z_height").and_then(parse_amount),
        source: SliceEstimateSource::Farm3dSlice,
    }
}

/// D12/D16: the same claims, read for an external revision's own estimates
/// display ("What the file says (not verified)"). Never trusted, and never
/// folded into [`SliceFacts`](super::SliceFacts) — an external revision's
/// facts come only from the operator (D15).
pub fn claimed_estimates_from_claims(claims: &[GcodeClaim]) -> ClaimedEstimates {
    ClaimedEstimates {
        print_seconds: claim(claims, "estimated printing time (normal mode)")
            .and_then(parse_duration),
        filament_grams: claim(claims, "filament used [g]").and_then(parse_total),
        filament_mm: claim(claims, "filament used [mm]").and_then(parse_total),
        layer_count: claim(claims, "total layer number").and_then(|text| text.parse().ok()),
        max_z_mm: claim(claims, "max_z_height").and_then(parse_amount),
        source: ClaimedEstimateSource::FileClaim,
        trusted: false,
    }
}

/// OrcaSlicer's `1d 2h 3m 4s` (any subset, in that form) as seconds.
fn parse_duration(text: &str) -> Option<u64> {
    let mut seconds: u64 = 0;
    let mut any = false;
    for token in text.split_whitespace() {
        let unit = match token.chars().last()? {
            'd' => 86_400,
            'h' => 3_600,
            'm' => 60,
            's' => 1,
            _ => return None,
        };
        let amount: u64 = token[..token.len() - 1].parse().ok()?;
        seconds = seconds.checked_add(amount.checked_mul(unit)?)?;
        any = true;
    }
    any.then_some(seconds)
}

/// A finite, non-negative number.
fn parse_amount(text: &str) -> Option<f64> {
    text.trim()
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value >= 0.0)
}

/// A number, or the sum of a comma-separated per-filament list.
fn parse_total(text: &str) -> Option<f64> {
    text.split(',')
        .map(parse_amount)
        .try_fold(0.0, |total, value| value.map(|value| total + value))
}

/// D13: publishes `output` as a farm3d Slice Revision of the operation's
/// plate. Stages the four input files from `work`, the invocation
/// manifest, and `run`'s log, then one `place_and_commit` writes the blobs,
/// the revision, its six `slice_revision_blobs` rows, and the operation's
/// `running → succeeded` move. An operation that isn't `running` is
/// [`RepositoryError::IllegalSliceTransition`] and nothing is written.
/// Whatever the result, the operation's staging is removed afterwards.
pub fn publish(
    store: &ContentStore,
    storage: &Storage,
    inputs: &PublishInputs,
    work: &WorkDir,
    run: &SliceRun,
    output: ValidatedOutput,
    cancel: &CancelFlag,
) -> Result<(SliceRevisionRecord, SliceOperationRecord), ContentError> {
    let result = stage_and_commit(store, storage, inputs, work, run, &output, cancel);
    store.discard_staging(&inputs.operation_id);
    result
}

fn stage_and_commit(
    store: &ContentStore,
    storage: &Storage,
    inputs: &PublishInputs,
    work: &WorkDir,
    run: &SliceRun,
    output: &ValidatedOutput,
    cancel: &CancelFlag,
) -> Result<(SliceRevisionRecord, SliceOperationRecord), ContentError> {
    let key = inputs.operation_id.as_str();
    let stage =
        |path: &Path, index| store.stage_from_path(path, key, index, cancel, &mut |_, _| {});
    let plate_3mf = stage(&work.plate_3mf(), PLATE_3MF_INDEX)?;
    let machine = stage(&work.machine_json(), MACHINE_INDEX)?;
    let process = stage(&work.process_json(), PROCESS_INDEX)?;
    let filament = stage(&work.filament_json(), FILAMENT_INDEX)?;
    let hashes = InputHashes {
        plate_3mf: plate_3mf.sha256.clone(),
        machine: machine.sha256.clone(),
        process: process.sha256.clone(),
        filament: filament.sha256.clone(),
    };
    let manifest = InvocationManifest::new(
        work,
        run.progress_piped,
        &inputs.engine,
        &inputs.preset_source,
        &inputs.target,
        &inputs.profile_overrides,
        &hashes,
        &output.bounds_check,
    );
    let manifest = store.stage_bytes(&manifest.to_bytes(), key, MANIFEST_NAME)?;
    let log = store.stage_bytes(run.log.text.as_bytes(), key, LOG_NAME)?;

    let blobs = vec![
        (SliceRevisionBlobRole::Plate3mf, plate_3mf.sha256.clone()),
        (SliceRevisionBlobRole::MachinePreset, machine.sha256.clone()),
        (SliceRevisionBlobRole::ProcessPreset, process.sha256.clone()),
        (
            SliceRevisionBlobRole::FilamentPreset,
            filament.sha256.clone(),
        ),
        (SliceRevisionBlobRole::Manifest, manifest.sha256.clone()),
        (SliceRevisionBlobRole::Log, log.sha256.clone()),
    ];
    let staged = [
        &output.gcode,
        &plate_3mf,
        &machine,
        &process,
        &filament,
        &manifest,
        &log,
    ];
    let gcode_size = i64::try_from(output.gcode.size).map_err(|_| {
        ContentError::Repository(RepositoryError::Validation {
            field_path: "gcodeSize",
        })
    })?;
    store.place_and_commit_unless_cancelled(storage, &staged, cancel, |tx| {
        let operation = load_operation(tx, key)?.ok_or_else(|| RepositoryError::NotFound {
            entity_id: key.to_string(),
        })?;
        let revision = NewFarm3dRevision {
            id: new_slice_revision_id(),
            source_revision_id: operation.source_revision_id,
            plate: SlicePlateRef {
                plate_key: operation.plate_key,
                plate_index: operation.plate_index,
                plate_name: operation.plate_name,
            },
            gcode_sha256: output.gcode.sha256.clone(),
            gcode_size,
            target: inputs.target.clone(),
            facts: inputs.facts.clone(),
            estimates: output.estimates.clone(),
            runtime: runtime_info(&inputs.engine, &inputs.preset_source),
            blobs,
        };
        let record = insert_farm3d_revision(tx, &revision)?;
        let operation = transition_operation(
            tx,
            key,
            OperationTransition::Succeed {
                slice_revision_id: revision.id,
            },
        )?;
        Ok((record, operation))
    })
}

/// D13: ends operation `operation_id` without a revision. Only `log` is
/// stored, as the operation's `log_sha256`, in the same commit as its move
/// to `failed` or `cancelled`. Any other staging of the operation (a
/// rejected G-code) is discarded first.
pub fn record_unpublished(
    store: &ContentStore,
    storage: &Storage,
    operation_id: &str,
    ending: Unpublished,
    log: &SliceLog,
) -> Result<SliceOperationRecord, ContentError> {
    store.discard_staging(operation_id);
    let result = store
        .stage_bytes(log.text.as_bytes(), operation_id, LOG_NAME)
        .and_then(|staged| {
            let log_sha256 = Some(staged.sha256.clone());
            let transition = match ending {
                Unpublished::Failed(failure) => OperationTransition::Fail {
                    failure,
                    log_sha256,
                },
                Unpublished::Cancelled => OperationTransition::Cancel { log_sha256 },
            };
            store.place_and_commit(storage, &[&staged], |tx| {
                transition_operation(tx, operation_id, transition)
            })
        });
    store.discard_staging(operation_id);
    result
}

/// D11–D13 for one finished run: maps how it ended ([`process::outcome`]),
/// validates the output, and either publishes a revision or records the
/// failure or cancellation with the log. A content-store failure is
/// returned as is and leaves the operation `running`, as a crash would;
/// startup recovery (D10) interrupts it.
pub fn finish_run(
    store: &ContentStore,
    storage: &Storage,
    inputs: &PublishInputs,
    work: &WorkDir,
    run: &SliceRun,
    cancel: &CancelFlag,
) -> Result<FinishedRun, ContentError> {
    let ending = match process::outcome(run, work) {
        SliceOutcome::OutputWritten => {
            match validate_output(store, &inputs.operation_id, work, &inputs.target, cancel) {
                Ok(output) => match publish(store, storage, inputs, work, run, output, cancel) {
                    Ok((revision, operation)) => {
                        return Ok(FinishedRun::Published {
                            revision,
                            operation,
                        })
                    }
                    Err(ContentError::Cancelled) => Unpublished::Cancelled,
                    Err(error) => return Err(error),
                },
                Err(OutputRejection::Invalid(failure)) => Unpublished::Failed(failure),
                Err(OutputRejection::Cancelled) => Unpublished::Cancelled,
                Err(OutputRejection::Store(error)) => return Err(error),
            }
        }
        SliceOutcome::Failed(failure) => Unpublished::Failed(failure),
        SliceOutcome::Cancelled => Unpublished::Cancelled,
    };
    record_unpublished(store, storage, &inputs.operation_id, ending, &run.log)
        .map(FinishedRun::Unpublished)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;
    use std::sync::Arc;

    use super::*;
    use crate::library::content::ContentFailurePoint;
    use crate::persistence::MetadataRootLease;
    use crate::slicing::facts::tests::a_profile_snapshot;
    use crate::slicing::process::{OrcaResult, RunExit};
    use crate::slicing::repository::fixtures::{a_document, seed};
    use crate::slicing::repository::{
        insert_operation, insert_preparation, load_revision, NewSliceOperation,
    };
    use crate::slicing::runtime::{OrcaVersion, PresetSourceOrigin};
    use crate::slicing::{PlateSnapshot, SliceControls, SliceOperationState, SliceTarget};
    use crate::spools::MaterialFamily;

    const OPERATION: &str = "sop-publish";
    /// The presets `orca-cube.gcode` claims.
    const MACHINE: &str = "MyKlipper 0.4 nozzle";
    const FILAMENT: &str = "Generic PLA @System";

    fn orca_cube() -> String {
        fs::read_to_string(
            Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/library/orca-cube.gcode"),
        )
        .unwrap()
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        _lease: MetadataRootLease,
        storage: Arc<Storage>,
        store: ContentStore,
        work: WorkDir,
        inputs: PublishInputs,
    }

    impl Fixture {
        /// A `running` operation on plate 1 of `mdl-stl`, its work directory
        /// with inputs written and `out/` holding `orca-cube.gcode`.
        fn new() -> Self {
            let (temp, lease, storage) = crate::test_storage();
            let content_root = storage.paths().content_root().to_path_buf();
            let store = ContentStore::open(&content_root).unwrap();
            let document = a_document();
            storage
                .write_repo(|tx| {
                    seed(tx);
                    insert_preparation(tx, "prp-publish", "mdl-stl", "msr-stl-1", &document)?;
                    insert_operation(
                        tx,
                        &NewSliceOperation {
                            id: OPERATION.to_string(),
                            preparation_id: "prp-publish".to_string(),
                            source_revision_id: "msr-stl-1".to_string(),
                            plate: PlateSnapshot {
                                plate_index: 1,
                                plate: document.plates[0].clone(),
                            },
                        },
                    )?;
                    transition_operation(
                        tx,
                        OPERATION,
                        OperationTransition::Start {
                            pid: 42,
                            pid_started_at: 7,
                        },
                    )
                })
                .unwrap();
            let work = WorkDir::for_operation(&content_root, OPERATION);
            work.create().unwrap();
            fs::write(work.plate_3mf(), b"PK plate").unwrap();
            for (path, name) in [
                (work.machine_json(), MACHINE),
                (work.process_json(), "0.20mm Standard"),
                (work.filament_json(), FILAMENT),
            ] {
                fs::write(path, format!("{{\"name\":\"{name}\"}}")).unwrap();
            }
            fs::write(work.gcode(), orca_cube()).unwrap();
            let version = OrcaVersion {
                major: 2,
                minor: 4,
                patch: 2,
                prerelease: None,
            };
            let inputs = PublishInputs {
                operation_id: OPERATION.to_string(),
                target: SliceRevisionTarget {
                    target: SliceTarget::Printer {
                        printer_id: "prn-a".to_string(),
                    },
                    profile: a_profile_snapshot(),
                    machine_preset: MACHINE.to_string(),
                    process_preset: "0.20mm Standard".to_string(),
                    filament_preset: FILAMENT.to_string(),
                    controls: SliceControls::default(),
                },
                facts: Farm3dFacts::new(a_profile_snapshot(), 0.4, MaterialFamily::Pla, None, 1.75),
                engine: EngineIdentity {
                    version: version.to_string(),
                    channel: version.channel(),
                    sha256: "e".repeat(64),
                },
                preset_source: PresetSourceIdentity::new(&version, PresetSourceOrigin::Engine),
                profile_overrides: Vec::new(),
            };
            Self {
                _temp: temp,
                _lease: lease,
                storage,
                store,
                work,
                inputs,
            }
        }

        fn finish(&self, run: &SliceRun) -> Result<FinishedRun, ContentError> {
            finish_run(
                &self.store,
                &self.storage,
                &self.inputs,
                &self.work,
                run,
                &CancelFlag::never(),
            )
        }

        fn count(&self, sql: &str) -> i64 {
            self.storage
                .read(|connection| connection.query_row(sql, [], |row| row.get(0)))
                .unwrap()
        }

        fn blob_files(&self) -> Vec<PathBuf> {
            let root = self.storage.paths().content_root().join("blobs/sha256");
            let mut files = Vec::new();
            for prefix in fs::read_dir(root).unwrap() {
                for blob in fs::read_dir(prefix.unwrap().path()).unwrap() {
                    files.push(blob.unwrap().path());
                }
            }
            files
        }

        fn staging_is_empty(&self) -> bool {
            let staging = self.storage.paths().content_root().join("staging");
            fs::read_dir(staging).unwrap().next().is_none()
        }

        fn blob_bytes(&self, sha256: &str) -> Vec<u8> {
            let mut bytes = Vec::new();
            self.store
                .open_verified(sha256)
                .unwrap()
                .read_to_end(&mut bytes)
                .unwrap();
            bytes
        }

        fn operation(&self) -> SliceOperationRecord {
            self.storage
                .write(|tx| load_operation(tx, OPERATION))
                .unwrap()
                .unwrap()
        }
    }

    /// The seeded rows: five `content_blobs` (with no files).
    const SEEDED_BLOBS: i64 = 5;

    fn a_run(code: i32, log: &str) -> SliceRun {
        SliceRun {
            exit: RunExit::Exited { code },
            result: Some(OrcaResult {
                return_code: code,
                error_string: String::new(),
            }),
            log: SliceLog {
                text: log.to_string(),
                truncated: false,
            },
            killed: false,
            progress_piped: false,
        }
    }

    #[test]
    fn estimates_parse_from_the_orca_cube_claims() {
        let fixture = Fixture::new();
        let (inspection, _, _) = gcode::inspect(&fixture.work.gcode(), &CancelFlag::never())
            .expect("orca-cube inspects");

        assert_eq!(
            estimates_from_claims(&inspection.claims),
            SliceEstimates {
                print_seconds: Some(3 * 60 + 42),
                filament_grams: Some(0.73),
                filament_mm: Some(245.37),
                layer_count: Some(50),
                max_z_mm: Some(10.0),
                source: SliceEstimateSource::Farm3dSlice,
            }
        );
    }

    #[test]
    fn claimed_estimates_parse_from_the_orca_cube_claims_but_are_never_trusted() {
        let fixture = Fixture::new();
        let (inspection, _, _) = gcode::inspect(&fixture.work.gcode(), &CancelFlag::never())
            .expect("orca-cube inspects");

        assert_eq!(
            claimed_estimates_from_claims(&inspection.claims),
            ClaimedEstimates {
                print_seconds: Some(3 * 60 + 42),
                filament_grams: Some(0.73),
                filament_mm: Some(245.37),
                layer_count: Some(50),
                max_z_mm: Some(10.0),
                source: ClaimedEstimateSource::FileClaim,
                trusted: false,
            }
        );
    }

    #[test]
    fn estimate_claims_parse_or_are_null() {
        assert_eq!(parse_duration("1h 2m 3s"), Some(3723));
        assert_eq!(parse_duration("1d 0h 0m 1s"), Some(86_401));
        assert_eq!(parse_duration("45s"), Some(45));
        assert_eq!(parse_duration(""), None);
        assert_eq!(parse_duration("3 minutes"), None);
        assert_eq!(parse_duration("1x"), None);
        assert_eq!(parse_total("1.5, 2.25"), Some(3.75));
        assert_eq!(parse_total("1.5, n/a"), None);
        assert_eq!(parse_amount("-1"), None);
        assert_eq!(parse_amount("NaN"), None);

        let claim = |key: &str, value: &str| GcodeClaim {
            key: key.to_string(),
            value: value.to_string(),
            line: 1,
        };
        assert_eq!(estimates_from_claims(&[]), SliceEstimates::none());
        let partial = estimates_from_claims(&[
            claim("total layer number", " 12 "),
            claim("estimated printing time (normal mode)", "soon"),
        ]);
        assert_eq!(partial.layer_count, Some(12));
        assert_eq!(partial.print_seconds, None);
    }

    #[test]
    fn a_valid_output_publishes_one_revision_with_six_blobs_and_succeeds_the_operation() {
        let fixture = Fixture::new();
        let gcode_bytes = fs::read(fixture.work.gcode()).unwrap();
        let log = "OrcaSlicer-2.4.2: loading <work>/input/plate.3mf\n";

        let FinishedRun::Published { revision, .. } = fixture.finish(&a_run(0, log)).unwrap()
        else {
            panic!("expected a published revision");
        };

        let summary = &revision.summary;
        assert!(summary.id.starts_with("slr-"));
        assert_eq!(summary.source_revision_id, "msr-stl-1");
        assert_eq!(
            summary.plate,
            Some(SlicePlateRef {
                plate_key: "plate-a".to_string(),
                plate_index: 1,
                plate_name: Some("Left".to_string()),
            })
        );
        assert_eq!(summary.estimates.as_ref().unwrap().layer_count, Some(50));
        assert_eq!(summary.runtime.as_ref().unwrap().engine_version, "2.4.2");
        assert!(!summary.requires_manual_printer_selection);
        assert_eq!(revision.target.as_ref(), Some(&fixture.inputs.target));
        let roles: Vec<SliceRevisionBlobRole> = revision.blobs.iter().map(|b| b.role).collect();
        assert_eq!(
            roles,
            [
                SliceRevisionBlobRole::Plate3mf,
                SliceRevisionBlobRole::MachinePreset,
                SliceRevisionBlobRole::ProcessPreset,
                SliceRevisionBlobRole::FilamentPreset,
                SliceRevisionBlobRole::Manifest,
                SliceRevisionBlobRole::Log,
            ]
        );

        let operation = fixture.operation();
        assert_eq!(operation.state, SliceOperationState::Succeeded);
        assert_eq!(
            operation.slice_revision_id.as_deref(),
            Some(summary.id.as_str())
        );

        let (gcode_sha256, gcode_size): (String, i64) = fixture
            .storage
            .read(|connection| {
                connection.query_row(
                    "SELECT gcode_sha256, gcode_size FROM slice_revisions",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
            })
            .unwrap();
        assert_eq!(gcode_size, gcode_bytes.len() as i64);
        assert_eq!(fixture.blob_bytes(&gcode_sha256), gcode_bytes);

        let blob = |role: &str| -> String {
            fixture
                .storage
                .read(|connection| {
                    connection.query_row(
                        "SELECT sha256 FROM slice_revision_blobs WHERE role = ?1",
                        [role],
                        |row| row.get(0),
                    )
                })
                .unwrap()
        };
        assert_eq!(fixture.blob_bytes(&blob("log")), log.as_bytes());
        assert_eq!(fixture.blob_bytes(&blob("plate3mf")), b"PK plate");
        let manifest: serde_json::Value =
            serde_json::from_slice(&fixture.blob_bytes(&blob("manifest"))).unwrap();
        assert_eq!(manifest["plate3mfSha256"], blob("plate3mf"));
        assert_eq!(
            manifest["presets"]["machine"]["sha256"],
            blob("machinePreset")
        );
        assert_eq!(manifest["presets"]["machine"]["name"], MACHINE);
        assert_eq!(
            manifest["presets"]["process"]["sha256"],
            blob("processPreset")
        );
        assert_eq!(
            manifest["presets"]["filament"]["sha256"],
            blob("filamentPreset")
        );
        let manifest_text = manifest.to_string();
        let root = fixture
            .storage
            .paths()
            .content_root()
            .to_string_lossy()
            .into_owned();
        assert!(!manifest_text.contains(&root), "{manifest_text}");

        // Seven blobs: the G-code and six inputs, every one with its file.
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM content_blobs"),
            SEEDED_BLOBS + 7
        );
        assert_eq!(fixture.blob_files().len(), 7);
        assert!(fixture.staging_is_empty());
    }

    /// The first outer wall of the first layer, inside the print body.
    const FIRST_WALL: &str = ";TYPE:Outer wall\n";

    /// `orca-cube.gcode` with `extra` printed at the start of the first
    /// outer wall.
    fn with_body_move(extra: &str) -> String {
        orca_cube().replacen(FIRST_WALL, &format!("{FIRST_WALL}{extra}\n"), 1)
    }

    fn published(fixture: &Fixture) -> SliceRevisionRecord {
        match fixture.finish(&a_run(0, "log")).unwrap() {
            FinishedRun::Published { revision, .. } => revision,
            other => panic!("expected a published revision, got {other:?}"),
        }
    }

    fn manifest_of(fixture: &Fixture) -> serde_json::Value {
        let sha256: String = fixture
            .storage
            .read(|connection| {
                connection.query_row(
                    "SELECT sha256 FROM slice_revision_blobs WHERE role = 'manifest'",
                    [],
                    |row| row.get(0),
                )
            })
            .unwrap();
        serde_json::from_slice(&fixture.blob_bytes(&sha256)).unwrap()
    }

    #[test]
    fn a_quoted_printer_preset_claim_and_a_print_within_the_xy_tolerance_still_publish() {
        let fixture = Fixture::new();
        let gcode = with_body_move("G1 X257.9 Y-1.9 E.1").replace(
            &format!("; printer_settings_id = {MACHINE}"),
            &format!("; printer_settings_id = \"{MACHINE}\""),
        );
        fs::write(fixture.work.gcode(), gcode).unwrap();

        published(&fixture);
    }

    #[test]
    fn a_print_reaching_the_printable_height_within_the_z_tolerance_publishes() {
        // orca-cube's last layer extrudes at Z 10.0.
        for height in [10.0, 9.96] {
            let mut fixture = Fixture::new();
            fixture.inputs.target.profile.printable_height_mm = height;

            published(&fixture);
        }
    }

    #[test]
    fn an_off_bed_purge_and_park_outside_the_print_body_still_publish() {
        let fixture = Fixture::new();
        let gcode = orca_cube()
            .replacen(
                ";LAYER_CHANGE\n",
                "G1 Z0.2 F720\nG1 Y-3 F1000 ; go outside print area\nG92 E0\n\
                 G1 X60 E9 F1000 ; intro line\nG1 X100 E12.5 F1000 ; intro line\n\
                 G92 E0\n;LAYER_CHANGE\n",
                1,
            )
            .replace(
                "; EXECUTABLE_BLOCK_END",
                "G1 Z300 F720 ; park\nG1 X300 Y300 E2 ; an end-code extrusion\n\
                 ; EXECUTABLE_BLOCK_END",
            );
        fs::write(fixture.work.gcode(), gcode).unwrap();

        published(&fixture);

        assert_eq!(
            manifest_of(&fixture)["boundsCheck"],
            serde_json::json!({ "status": "checked", "scope": "printBody" })
        );
    }

    #[test]
    fn untrackable_positioning_skips_the_bounds_check_and_the_manifest_says_so() {
        let fixture = Fixture::new();
        // Inch units make every position unknowable; even this far-off
        // extrusion can't be judged.
        fs::write(fixture.work.gcode(), with_body_move("G20\nG1 X900 E1")).unwrap();

        published(&fixture);

        assert_eq!(
            manifest_of(&fixture)["boundsCheck"],
            serde_json::json!({
                "status": "skipped",
                "reason": crate::slicing::printed_bounds::INCH_UNITS,
            })
        );
    }

    /// `setup` rewrites (or removes) the output or the target, and the run
    /// fails with exactly `expected`, storing only its log.
    fn assert_fails_with_only_its_log(
        name: &str,
        setup: impl FnOnce(&mut Fixture),
        run: SliceRun,
        expected: SliceFailureCode,
    ) {
        let mut fixture = Fixture::new();
        setup(&mut fixture);

        let finished = fixture.finish(&run).unwrap();

        let FinishedRun::Unpublished(operation) = finished else {
            panic!("{name}: expected a failed operation, got {finished:?}");
        };
        assert_eq!(operation.state, SliceOperationState::Failed, "{name}");
        let failure = operation.failure.expect("a failure");
        assert_eq!(failure.code, expected, "{name}");
        if let SliceFailureCode::OutputInvalid { reason } = &failure.code {
            assert_eq!(&failure.message, reason, "{name}");
        }
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM slice_revisions"),
            0,
            "{name}"
        );
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM slice_revision_blobs"),
            0,
            "{name}"
        );
        // One new row, the log's, referenced by the operation; no file for
        // anything else.
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM content_blobs"),
            SEEDED_BLOBS + 1,
            "{name}"
        );
        assert_eq!(
            fixture.count(
                "SELECT COUNT(*) FROM slice_operations o
                 JOIN content_blobs c ON c.sha256 = o.log_sha256"
            ),
            1,
            "{name}"
        );
        assert_eq!(fixture.blob_files().len(), 1, "{name}");
        assert!(fixture.staging_is_empty(), "{name}");
    }

    fn invalid(reason: &str) -> SliceFailureCode {
        SliceFailureCode::OutputInvalid {
            reason: reason.to_string(),
        }
    }

    fn rewrite(fixture: &Fixture, edit: impl FnOnce(String) -> String) {
        fs::write(fixture.work.gcode(), edit(orca_cube())).unwrap();
    }

    #[test]
    fn each_validation_failure_fails_the_operation_with_only_its_log() {
        assert_fails_with_only_its_log(
            "missing output",
            |fixture| fs::remove_file(fixture.work.gcode()).unwrap(),
            a_run(0, "Success."),
            process::output_missing().code,
        );
        #[cfg(unix)]
        assert_fails_with_only_its_log(
            "symlinked output",
            |fixture| {
                let real = fixture.work.root().join("real.gcode");
                fs::rename(fixture.work.gcode(), &real).unwrap();
                std::os::unix::fs::symlink(&real, fixture.work.gcode()).unwrap();
            },
            a_run(0, "log"),
            invalid(NOT_A_FILE),
        );
        assert_fails_with_only_its_log(
            "a directory",
            |fixture| {
                fs::remove_file(fixture.work.gcode()).unwrap();
                fs::create_dir(fixture.work.gcode()).unwrap();
            },
            a_run(0, "log"),
            invalid(NOT_A_FILE),
        );
        assert_fails_with_only_its_log(
            "over 1 GiB",
            |fixture| {
                File::options()
                    .write(true)
                    .open(fixture.work.gcode())
                    .unwrap()
                    .set_len(MAX_OUTPUT_BYTES + 1)
                    .unwrap();
            },
            a_run(0, "log"),
            invalid(TOO_LARGE),
        );
        assert_fails_with_only_its_log(
            "not G-code",
            |fixture| fs::write(fixture.work.gcode(), b"\x00\x01binary\x02").unwrap(),
            a_run(0, "log"),
            invalid(NOT_GCODE),
        );
        assert_fails_with_only_its_log(
            "another producer",
            |fixture| {
                rewrite(fixture, |gcode| {
                    gcode.replace("generated by OrcaSlicer", "generated by PrusaSlicer")
                })
            },
            a_run(0, "log"),
            invalid(WRONG_PRODUCER),
        );
        // The P4 inspector already refuses a G-code without commands.
        assert_fails_with_only_its_log(
            "no commands",
            |fixture| {
                rewrite(fixture, |gcode| {
                    gcode
                        .lines()
                        .filter(|line| line.starts_with(';'))
                        .map(|line| format!("{line}\n"))
                        .collect()
                })
            },
            a_run(0, "log"),
            invalid(&format!(
                "{NOT_INSPECTABLE}This G-code file has no commands."
            )),
        );
        assert_fails_with_only_its_log(
            "an extrusion in the print body outside the plate",
            |fixture| rewrite(fixture, |_| with_body_move("G1 X258.5 Y130 E.1")),
            a_run(0, "log"),
            invalid(OUTSIDE_AREA),
        );
        assert_fails_with_only_its_log(
            "above the printable height",
            |fixture| fixture.inputs.target.profile.printable_height_mm = 9.0,
            a_run(0, "log"),
            invalid(ABOVE_HEIGHT),
        );
        assert_fails_with_only_its_log(
            "a bed with no printable area",
            |fixture| {
                fixture.inputs.target.profile.bed_shape = BedShape::Polygon { points: Vec::new() }
            },
            a_run(0, "log"),
            invalid(EMPTY_BED),
        );
        assert_fails_with_only_its_log(
            "another printer preset",
            |fixture| fixture.inputs.target.machine_preset = "Other 0.4 nozzle".to_string(),
            a_run(0, "log"),
            invalid(WRONG_PRINTER_PRESET),
        );
        assert_fails_with_only_its_log(
            "another filament preset",
            |fixture| fixture.inputs.target.filament_preset = "Other PLA".to_string(),
            a_run(0, "log"),
            invalid(WRONG_FILAMENT_PRESET),
        );
        assert_fails_with_only_its_log(
            "a nonzero return code",
            |_| {},
            a_run(-5, "load_from_json: failed"),
            SliceFailureCode::PresetInvalid,
        );
    }

    /// The operation's `log_sha256` names a new blob holding `log`, and the
    /// startup sweep keeps it.
    fn assert_log_is_kept(fixture: &Fixture, log: &str) {
        let sha256: String = fixture
            .storage
            .read(|connection| {
                connection.query_row("SELECT log_sha256 FROM slice_operations", [], |row| {
                    row.get(0)
                })
            })
            .unwrap();
        assert_eq!(fixture.blob_bytes(&sha256), log.as_bytes());

        let report = fixture.store.startup_sweep(&fixture.storage).unwrap();

        assert_eq!(report.orphans_removed, 0);
        assert_eq!(report.pending_released, 0);
        assert_eq!(fixture.blob_bytes(&sha256), log.as_bytes());
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM content_blobs"),
            SEEDED_BLOBS + 1
        );
    }

    #[test]
    fn a_failed_run_keeps_its_log_through_the_startup_sweep() {
        let fixture = Fixture::new();

        let FinishedRun::Unpublished(operation) = fixture.finish(&a_run(-5, "failed log")).unwrap()
        else {
            panic!("expected a failed operation");
        };

        assert_eq!(operation.state, SliceOperationState::Failed);
        assert_log_is_kept(&fixture, "failed log");
    }

    #[test]
    fn a_cancelled_run_is_cancelled_with_only_its_log() {
        let fixture = Fixture::new();
        let mut run = a_run(0, "cancelled log");
        run.exit = RunExit::Cancelled;
        run.result = None;

        let FinishedRun::Unpublished(operation) = fixture.finish(&run).unwrap() else {
            panic!("expected a cancelled operation");
        };

        assert_eq!(operation.state, SliceOperationState::Cancelled);
        assert_eq!(fixture.count("SELECT COUNT(*) FROM slice_revisions"), 0);
        assert_eq!(fixture.blob_files().len(), 1);
        assert_log_is_kept(&fixture, "cancelled log");
    }

    #[test]
    fn a_crash_between_placement_and_commit_leaves_no_revision_and_the_sweep_cleans_up() {
        let fixture = Fixture::new();
        fixture
            .store
            .inject_failure_once(ContentFailurePoint::AfterPlacementBeforeCommit);

        let error = fixture.finish(&a_run(0, "log")).unwrap_err();

        assert!(matches!(error, ContentError::Io), "{error:?}");
        assert_eq!(fixture.count("SELECT COUNT(*) FROM slice_revisions"), 0);
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM content_blobs"),
            SEEDED_BLOBS
        );
        assert_eq!(fixture.operation().state, SliceOperationState::Running);
        // The placed files are orphans until the sweep.
        assert_eq!(fixture.blob_files().len(), 7);

        let report = fixture.store.startup_sweep(&fixture.storage).unwrap();

        assert_eq!(report.orphans_removed, 7);
        assert!(fixture.blob_files().is_empty());
        assert_eq!(
            fixture.count("SELECT COUNT(*) FROM content_blobs"),
            SEEDED_BLOBS
        );
    }

    #[test]
    fn publishing_an_operation_that_is_not_running_is_illegal_and_writes_nothing() {
        let fixture = Fixture::new();
        let FinishedRun::Published {
            revision: first, ..
        } = fixture.finish(&a_run(0, "log")).unwrap()
        else {
            panic!("expected a published revision");
        };
        let output = validate_output(
            &fixture.store,
            OPERATION,
            &fixture.work,
            &fixture.inputs.target,
            &CancelFlag::never(),
        )
        .unwrap();

        let error = publish(
            &fixture.store,
            &fixture.storage,
            &fixture.inputs,
            &fixture.work,
            &a_run(0, "log"),
            output,
            &CancelFlag::never(),
        )
        .unwrap_err();

        assert!(
            matches!(
                error,
                ContentError::Repository(RepositoryError::IllegalSliceTransition {
                    from: SliceOperationState::Succeeded,
                    to: SliceOperationState::Succeeded,
                    ..
                })
            ),
            "{error:?}"
        );
        assert_eq!(fixture.count("SELECT COUNT(*) FROM slice_revisions"), 1);
        let operation = fixture.operation();
        assert_eq!(
            operation.slice_revision_id.as_deref(),
            Some(first.summary.id.as_str())
        );
        assert!(fixture
            .storage
            .write(|tx| load_revision(tx, &first.summary.id))
            .unwrap()
            .is_some());
        assert!(fixture.staging_is_empty());
    }

    #[test]
    fn a_polygon_bed_is_checked_against_its_bounding_box() {
        use crate::catalog::PointMm;
        let mut profile = a_profile_snapshot();
        profile.bed_shape = BedShape::Polygon {
            points: [(0.0, 0.0), (100.0, 0.0), (50.0, 80.0)]
                .map(|(x_mm, y_mm)| PointMm { x_mm, y_mm })
                .to_vec(),
        };
        let bounds = |max_x: f64, max_y: f64| BoundsMm {
            min: [-1.0, 0.0, 0.0],
            max: [max_x, max_y, 1.0],
        };

        assert_eq!(check_printed_bounds(&bounds(101.9, 81.9), &profile), Ok(()));
        assert_eq!(
            check_printed_bounds(&bounds(102.1, 10.0), &profile),
            Err(OUTSIDE_AREA)
        );
        assert_eq!(
            check_printed_bounds(&bounds(10.0, 82.1), &profile),
            Err(OUTSIDE_AREA)
        );
        profile.bed_shape = BedShape::Polygon {
            points: vec![
                PointMm {
                    x_mm: 0.0,
                    y_mm: 0.0
                };
                2
            ],
        };
        assert_eq!(
            check_printed_bounds(&bounds(1.0, 1.0), &profile),
            Err(EMPTY_BED)
        );
    }
}
