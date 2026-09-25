//! D10: slice operations — starting them, the scheduler that runs them one
//! at a time, cancelling them, and startup recovery.
//!
//! - **Start** ([`start_slice`]) is idempotent by `operationId` through P3's
//!   operations ledger ([`OperationKind::StartSlice`]). Everything that can
//!   refuse a slice is checked before anything is queued: the Preparation's
//!   revision and staleness (D5), the runtime (D2), the presets and the
//!   mapping (D3, D4), and each plate (D7). Each plate's inputs are then
//!   written to its work directory (D8) and one transaction claims the
//!   operation id and writes one `queued` row per plate. Operation ids are
//!   derived from the `operationId` and the plate key, so a replay finds
//!   the same rows, even after a restart.
//! - **The scheduler** runs at most one OrcaSlicer at a time, in FIFO order,
//!   on its own long-lived thread (PDEATHSIG follows the thread that
//!   spawned the engine, so it must outlive the run). Progress is throttled
//!   to one event per 250 ms per operation, and the last update always goes
//!   out (D9).
//! - **Cancel** ([`cancel_slice_operation`]) takes a queued operation off
//!   the queue without spawning anything, and stops a running one through
//!   the supervisor (SIGTERM, 5 s, SIGKILL).
//! - **Recovery** ([`recover_after_restart`]) runs before any command is
//!   served: every `queued` or `running` row becomes `interrupted`, a
//!   recorded OrcaSlicer that is somehow still alive is stopped, and every
//!   work directory is removed. Published revisions are never touched.
//!
//! A content-store or database failure while a finished run is stored
//! fails the operation at once with `storageFailed`, keeping its log when
//! the log can still be stored.

use std::collections::VecDeque;
use std::fs;
use std::io;
use std::panic::AssertUnwindSafe;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Condvar, Mutex, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use rusqlite::{Connection, OptionalExtension};
use serde::Serialize;
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::sync::watch;

use crate::contracts::command::CommandError;
use crate::library::content::CancelFlag;
use crate::persistence::{RepositoryError, Storage, StorageError};
use crate::spools::encode_enum;
use crate::spools::operations::{self as ledger, Claim, OperationKind};

use super::events::{self, ProgressThrottle, SliceProgressPayload, PROGRESS_INTERVAL};
use super::facts::Farm3dFacts;
use super::invocation::{EngineIdentity, PresetSourceIdentity, WorkDir, WORK_ROOT_DIR};
use super::mapping::{apply_overrides_and_controls, SlicePresetDocuments, SliceSettingsInput};
use super::plate3mf::write_plate_3mf;
use super::presets::{material_family_for, resolve_target};
use super::process::{run_slice, SliceCommand, SliceLog, SliceObserver, SliceProgress, STOP_GRACE};
use super::process_group::{process_executable, process_start_time, stop_recorded_group};
use super::publish::{finish_run, record_unpublished, FinishedRun, PublishInputs, Unpublished};
use super::repository::{
    insert_operation, load_operation, load_preparation, load_runtime_config,
    mark_active_interrupted, transition_operation, InterruptedOperation, NewSliceOperation,
    OperationTransition,
};
use super::runtime::PATH_EXECUTABLE;
use super::{
    PlateDoc, PlateSnapshot, PreparationRecord, SliceFailure, SliceFailureCode,
    SliceOperationRecord, SliceOperationState, SliceRevisionTarget, SlicingServices,
};

/// D17: how many finished operations the backfill carries.
pub const RECENT_OPERATIONS: u32 = 50;

/// How long `cancel_slice_operation` waits for a running operation to
/// settle: the supervisor's SIGTERM grace, plus time to store the log.
const CANCEL_SETTLE: Duration = Duration::from_secs(10);

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

// ---------------------------------------------------------------------------
// Scheduler
// ---------------------------------------------------------------------------

/// D10: the FIFO queue and the one running operation.
#[derive(Default)]
pub struct Scheduler {
    state: Mutex<SchedulerState>,
    /// Signalled when a job is queued.
    queued: Condvar,
    /// Signalled when the running job is done.
    settled: Condvar,
    worker: OnceLock<()>,
    /// `start_slice` calls run one at a time, so a replay check and the
    /// claim that follows can't interleave with another start. It is held
    /// from the claim through the enqueue, so a cancel that finds nothing
    /// scheduled takes it to wait out a start in between. Lock order:
    /// `starting`, then `state`.
    starting: Mutex<()>,
    hook: Mutex<Option<SchedulerHook>>,
}

/// Test seam: points in the scheduler a test can pause at.
#[doc(hidden)]
#[derive(Debug)]
pub enum SchedulerPoint<'a> {
    /// `start_slice` has committed these operations and not yet queued
    /// them (it holds `starting`).
    Committed(&'a [String]),
    /// `cancel_slice_operation` found operation `id` neither queued nor
    /// running and is about to wait for any start in progress.
    CancelAwaitsStart(&'a str),
    /// The worker is about to start OrcaSlicer for operation `id`.
    Spawning(&'a str),
    /// OrcaSlicer for operation `id` has exited, and the worker is about to
    /// store the outcome and remove the work directory. Holding the worker
    /// here stands in for a farm3d that died mid-run.
    Exited(&'a str),
}

/// Test seam: called at each [`SchedulerPoint`].
#[doc(hidden)]
pub type SchedulerHook = Arc<dyn Fn(SchedulerPoint<'_>) + Send + Sync>;

/// The running job's operation, its Preparation, and its cancel switch.
struct Current {
    operation_id: String,
    preparation_id: String,
    cancel: Arc<watch::Sender<bool>>,
}

#[derive(Default)]
struct SchedulerState {
    queue: VecDeque<Job>,
    /// The running job.
    current: Option<Current>,
}

/// One queued operation: its inputs are already in its work directory.
struct Job {
    operation_id: String,
    preparation_id: String,
    engine: PathBuf,
    preset_source: PathBuf,
    inputs: PublishInputs,
    cancel_sender: Arc<watch::Sender<bool>>,
    cancel: CancelFlag,
}

impl Scheduler {
    #[doc(hidden)]
    pub fn set_hook(&self, hook: Option<SchedulerHook>) {
        *lock(&self.hook) = hook;
    }

    /// Test seam: whether nothing is queued or running.
    #[doc(hidden)]
    pub fn idle(&self) -> bool {
        let state = lock(&self.state);
        state.queue.is_empty() && state.current.is_none()
    }

    fn reach(&self, point: SchedulerPoint<'_>) {
        let hook = lock(&self.hook).clone();
        if let Some(hook) = hook {
            hook(point);
        }
    }
}

fn enqueue<R: tauri::Runtime>(services: &Arc<SlicingServices<R>>, jobs: Vec<Job>) {
    let scheduler = &services.scheduler;
    scheduler.worker.get_or_init(|| {
        let worker = Arc::clone(services);
        thread::Builder::new()
            .name("farm3d-slicer".to_string())
            .spawn(move || run_worker(worker))
            .expect("the slicer thread starts");
    });
    lock(&scheduler.state).queue.extend(jobs);
    scheduler.queued.notify_all();
}

fn run_worker<R: tauri::Runtime>(services: Arc<SlicingServices<R>>) {
    loop {
        let job = {
            let mut state = lock(&services.scheduler.state);
            loop {
                if let Some(job) = state.queue.pop_front() {
                    state.current = Some(Current {
                        operation_id: job.operation_id.clone(),
                        preparation_id: job.preparation_id.clone(),
                        cancel: Arc::clone(&job.cancel_sender),
                    });
                    break job;
                }
                state = services
                    .scheduler
                    .queued
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
            }
        };
        let operation_id = job.operation_id.clone();
        let ran = std::panic::catch_unwind(AssertUnwindSafe(|| run_job(&services, job)));
        if ran.is_err() {
            // The supervisor's guard has stopped the process group; the row
            // must not stay `running` until the next start.
            let events = fail_operation(&services, &operation_id, None, internal_failure());
            services.publish(events);
            let _ = fs::remove_dir_all(
                WorkDir::for_operation(services.storage.paths().content_root(), &operation_id)
                    .root(),
            );
        }
        lock(&services.scheduler.state).current = None;
        services.scheduler.settled.notify_all();
    }
}

/// Reports a running operation's pid and progress as they happen.
struct OperationObserver<'a, R: tauri::Runtime> {
    services: &'a SlicingServices<R>,
    operation_id: &'a str,
    throttle: ProgressThrottle,
    cancel: Arc<watch::Sender<bool>>,
}

impl<R: tauri::Runtime> OperationObserver<'_, R> {
    fn emit(&self, progress: Option<SliceProgressPayload>) {
        if let Some(progress) = progress {
            self.services.publish(vec![events::operation_progress(
                self.operation_id,
                progress,
            )]);
        }
    }
}

impl<R: tauri::Runtime> SliceObserver for OperationObserver<'_, R> {
    fn spawned(&mut self, pid: u32) {
        let started = OperationTransition::Start {
            pid: i64::from(pid),
            // Off Linux there is no start time; recovery then never
            // signals a recorded pid (D24).
            pid_started_at: process_start_time(pid).unwrap_or(0),
        };
        match self
            .services
            .storage
            .write_repo(|tx| transition_operation(tx, self.operation_id, started))
        {
            Ok(record) => self
                .services
                .publish(vec![events::operation_changed(&record)]),
            // The row is gone or no longer queued (a restart's recovery
            // interrupted it): stop the engine.
            Err(_) => {
                let _ = self.cancel.send(true);
            }
        }
    }

    fn progress(&mut self, update: SliceProgress) {
        let offered = self
            .throttle
            .offer(SliceProgressPayload::from(&update), Instant::now());
        self.emit(offered);
    }

    fn tick(&mut self) {
        let due = self.throttle.due(Instant::now());
        self.emit(due);
    }
}

fn run_job<R: tauri::Runtime>(services: &SlicingServices<R>, job: Job) {
    let work = WorkDir::for_operation(services.storage.paths().content_root(), &job.operation_id);
    let still_queued = matches!(
        read_operation(&services.storage, &job.operation_id),
        Ok(Some(SliceOperationRecord {
            state: SliceOperationState::Queued,
            ..
        }))
    );
    let events = if !still_queued {
        // Its row was ended (a cancel that raced the enqueue) or deleted
        // with its Model: nothing runs and there is nothing to report.
        Vec::new()
    } else if job.cancel.is_cancelled() {
        // Cancelled between leaving the queue and starting: nothing runs.
        cancel_unstarted(services, &job.operation_id)
    } else {
        let mut command = SliceCommand::new(
            job.engine.clone(),
            work.clone(),
            Some(job.preset_source.clone()),
        );
        command.environment.extend(services.engine_environment());
        services
            .scheduler
            .reach(SchedulerPoint::Spawning(&job.operation_id));
        let mut observer = OperationObserver {
            services,
            operation_id: &job.operation_id,
            throttle: ProgressThrottle::new(PROGRESS_INTERVAL),
            cancel: Arc::clone(&job.cancel_sender),
        };
        let run = run_slice(&command, &job.cancel, &mut observer);
        services
            .scheduler
            .reach(SchedulerPoint::Exited(&job.operation_id));
        let last = observer.throttle.finish();
        observer.emit(last);
        match finish_run(
            &services.content,
            &services.storage,
            &job.inputs,
            &work,
            &run,
            &job.cancel,
        ) {
            Ok(FinishedRun::Published {
                revision,
                operation,
            }) => vec![
                events::revision_created(&revision.summary),
                events::operation_changed(&operation),
            ],
            Ok(FinishedRun::Unpublished(operation)) => vec![events::operation_changed(&operation)],
            Err(error) => {
                eprintln!(
                    "farm3d: slice {} could not be stored: {error}",
                    job.operation_id
                );
                fail_operation(
                    services,
                    &job.operation_id,
                    Some(&run.log),
                    storage_failure(),
                )
            }
        }
    };
    let _ = fs::remove_dir_all(work.root());
    services.publish(events);
}

/// `storageFailed`: the run finished, but farm3d couldn't store it.
pub fn storage_failure() -> SliceFailure {
    SliceFailure {
        code: SliceFailureCode::StorageFailed,
        message: "farm3d couldn't store the slice result.".to_string(),
    }
}

/// `internalError`: farm3d itself failed while running the slice.
fn internal_failure() -> SliceFailure {
    SliceFailure {
        code: SliceFailureCode::InternalError,
        message: "farm3d stopped this slice after an internal error.".to_string(),
    }
}

/// Fails operation `id` with `failure`, if it is still queued or running,
/// keeping `log` when it can be stored. Returns the events to publish.
fn fail_operation<R: tauri::Runtime>(
    services: &SlicingServices<R>,
    id: &str,
    log: Option<&SliceLog>,
    failure: SliceFailure,
) -> Vec<events::SlicingEventSpec> {
    let active = read_operation(&services.storage, id)
        .ok()
        .flatten()
        .is_some_and(|operation| {
            matches!(
                operation.state,
                SliceOperationState::Queued | SliceOperationState::Running
            )
        });
    if !active {
        return Vec::new();
    }
    let with_log = log.and_then(|log| {
        record_unpublished(
            &services.content,
            &services.storage,
            id,
            Unpublished::Failed(failure.clone()),
            log,
        )
        .ok()
    });
    let record = with_log.or_else(|| {
        services
            .storage
            .write_repo(|tx| {
                transition_operation(
                    tx,
                    id,
                    OperationTransition::Fail {
                        failure,
                        log_sha256: None,
                    },
                )
            })
            .ok()
    });
    record
        .map(|record| vec![events::operation_changed(&record)])
        .unwrap_or_default()
}

/// A queued operation that never started: `cancelled`, with no log.
fn cancel_unstarted<R: tauri::Runtime>(
    services: &SlicingServices<R>,
    id: &str,
) -> Vec<events::SlicingEventSpec> {
    services
        .storage
        .write_repo(|tx| {
            transition_operation(tx, id, OperationTransition::Cancel { log_sha256: None })
        })
        .map(|record| vec![events::operation_changed(&record)])
        .unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Start
// ---------------------------------------------------------------------------

/// `start_slice`'s request.
#[derive(Clone, Debug)]
pub struct StartSliceRequest {
    pub operation_id: String,
    pub preparation_id: String,
    pub expected_revision: i64,
    pub plate_keys: Vec<String>,
    pub continue_with_source_revision: Option<String>,
}

/// The request fields that define a `start_slice` operation, in a fixed
/// order for [`ledger::digest`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartSliceDigest<'a> {
    preparation_id: &'a str,
    expected_revision: i64,
    plate_keys: &'a [String],
    continue_with_source_revision: Option<&'a str>,
}

/// The slice operation id for plate `plate_key` of `start_slice`
/// `operation_id`: `sop-` and a UUID-shaped SHA-256 prefix of both, so a
/// replay (even after a restart) names the same rows.
pub fn derived_operation_id(operation_id: &str, plate_key: &str) -> String {
    let hash = format!(
        "{:x}",
        Sha256::digest(format!("{operation_id}\u{0}{plate_key}").as_bytes())
    );
    format!(
        "sop-{}-{}-{}-{}-{}",
        &hash[0..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    )
}

/// The ledger's record for `operation_id`: its kind and request digest.
/// `pub(crate)` so [`super::external`]'s own replay check can reuse it.
pub(crate) fn recorded_claim(
    connection: &Connection,
    operation_id: &str,
) -> rusqlite::Result<Option<(String, String)>> {
    connection
        .query_row(
            "SELECT kind, request_digest FROM operations WHERE id = ?1",
            [operation_id],
            |row| Ok((row.get(0)?, row.get(1)?)),
        )
        .optional()
}

/// Operation `id`, or `None`.
fn read_operation(
    storage: &Storage,
    id: &str,
) -> Result<Option<SliceOperationRecord>, CommandError> {
    storage
        .read(|connection| Ok(load_operation(connection, id)))
        .map_err(storage_error)?
        .map_err(storage_error)
}

/// The operations of an earlier, identical `start_slice`, or `None` for a
/// new `operationId`. The same id for another request is the P3 ledger's
/// `operationId` reuse error.
fn replay(
    storage: &Storage,
    request: &StartSliceRequest,
    digest: &str,
) -> Result<Option<Vec<SliceOperationRecord>>, CommandError> {
    let recorded = storage
        .read(|connection| recorded_claim(connection, &request.operation_id))
        .map_err(storage_error)?;
    let Some((kind, recorded_digest)) = recorded else {
        return Ok(None);
    };
    if kind != encode_enum(OperationKind::StartSlice) || recorded_digest != digest {
        return Err(CommandError::from_repository(
            RepositoryError::OperationIdReused,
        ));
    }
    let mut records = Vec::new();
    for plate_key in &request.plate_keys {
        let id = derived_operation_id(&request.operation_id, plate_key);
        records.extend(read_operation(storage, &id)?);
    }
    Ok(Some(records))
}

/// The requested plates of `preparation`, with their 1-based positions, in
/// request order. Empty, duplicated, or unknown keys are `VALIDATION`.
fn requested_plates(
    preparation: &PreparationRecord,
    plate_keys: &[String],
) -> Result<Vec<(u32, PlateDoc)>, CommandError> {
    if plate_keys.is_empty() {
        return Err(CommandError::validation_at(
            "plateKeys",
            "Choose at least one plate to slice.",
        ));
    }
    let mut plates: Vec<(u32, PlateDoc)> = Vec::new();
    for key in plate_keys {
        if plates.iter().any(|(_, plate)| &plate.plate_key == key) {
            return Err(CommandError::validation_at(
                "plateKeys",
                "Each plate can be sliced once per request.",
            ));
        }
        let (position, plate) = preparation
            .document
            .plates
            .iter()
            .enumerate()
            .find(|(_, plate)| &plate.plate_key == key)
            .ok_or_else(|| {
                CommandError::validation_at("plateKeys", "That plate isn't in this Preparation.")
            })?;
        plates.push((position as u32 + 1, plate.clone()));
    }
    Ok(plates)
}

/// A string preset value, or the first of a list (OrcaSlicer stores
/// per-extruder values as lists of strings).
fn first_text(preset: &Value, key: &str) -> Option<String> {
    let value = preset.get(key)?;
    let text = match value {
        Value::Array(values) => values.first()?.as_str()?,
        Value::String(text) => text.as_str(),
        _ => return None,
    };
    let text = text.trim().trim_matches('"').trim();
    (!text.is_empty()).then(|| text.to_string())
}

fn first_number(preset: &Value, key: &str) -> Option<f64> {
    first_text(preset, key)?
        .parse::<f64>()
        .ok()
        .filter(|value| value.is_finite() && *value > 0.0)
}

/// D15: a farm3d revision's facts, from the target and the flat presets the
/// slice loads.
fn facts_for(
    snapshot: super::ProfileSnapshot,
    nozzle_fallback: f64,
    presets: &SlicePresetDocuments,
) -> Farm3dFacts {
    let nozzle = first_number(&presets.machine, "nozzle_diameter").unwrap_or(nozzle_fallback);
    let (family, other) = first_text(&presets.filament, "filament_type")
        .map(|filament_type| material_family_for(&filament_type))
        .unwrap_or((crate::spools::MaterialFamily::Other, None));
    let filament_diameter = first_number(&presets.filament, "filament_diameter").unwrap_or(1.75);
    Farm3dFacts::new(snapshot, nozzle, family, other, filament_diameter)
}

/// One plate, ready to queue.
struct PreparedPlate {
    operation_id: String,
    plate: PlateSnapshot,
    plate_3mf: Vec<u8>,
}

/// Everything a start needs, checked before anything is queued.
struct PreparedStart {
    engine: PathBuf,
    preset_source: PathBuf,
    presets: SlicePresetDocuments,
    inputs: PublishInputs,
    plates: Vec<PreparedPlate>,
}

fn prepare<R: tauri::Runtime>(
    services: &SlicingServices<R>,
    request: &StartSliceRequest,
    preparation: &PreparationRecord,
    plates: Vec<(u32, PlateDoc)>,
) -> Result<PreparedStart, CommandError> {
    let document = &preparation.document;
    let process_preset = document.process_preset.clone().ok_or_else(|| {
        CommandError::validation_at("document.processPreset", "Choose a quality preset first.")
    })?;
    let filament_preset = document.filament_preset.clone().ok_or_else(|| {
        CommandError::validation_at("document.filamentPreset", "Choose a filament preset first.")
    })?;
    let (engine, source) = services.usable_runtime()?;
    let index = services.preset_index(&source)?;
    let target = resolve_target(&services.storage, &services.catalog, &document.target)?;
    let presets = apply_overrides_and_controls(
        &index,
        SliceSettingsInput {
            machine_preset: &target.machine_preset,
            process_preset: &process_preset,
            filament_preset: &filament_preset,
            profile: &target.profile,
            overridden_fields: &target.overridden_fields,
            unknown_override_keys: &target.unknown_override_keys,
            controls: &document.controls,
        },
    )?;
    let revision = services.revision_geometry(&preparation.source_revision_id)?;
    let plates = plates
        .into_iter()
        .map(|(plate_index, plate)| {
            let plate_3mf = write_plate_3mf(&plate, &revision)?;
            Ok(PreparedPlate {
                operation_id: derived_operation_id(&request.operation_id, &plate.plate_key),
                plate: PlateSnapshot { plate_index, plate },
                plate_3mf,
            })
        })
        .collect::<Result<Vec<_>, CommandError>>()?;
    let snapshot = target.profile_snapshot();
    let facts = facts_for(
        snapshot.clone(),
        target.profile.nozzle_diameter_mm[0],
        &presets,
    );
    let engine_identity =
        EngineIdentity::of(&engine.path, &engine.version, &services.caches.hashes).map_err(
            |_| CommandError::slicer_unavailable("farm3d couldn't read the OrcaSlicer program."),
        )?;
    let inputs = PublishInputs {
        operation_id: String::new(),
        target: SliceRevisionTarget {
            target: document.target.clone(),
            profile: snapshot,
            machine_preset: target.machine_preset.clone(),
            process_preset,
            filament_preset,
            controls: document.controls.clone(),
        },
        facts,
        engine: engine_identity,
        preset_source: PresetSourceIdentity::new(&source.version, source.origin),
        profile_overrides: presets
            .override_keys
            .iter()
            .map(|key| key.to_string())
            .collect(),
    };
    Ok(PreparedStart {
        engine: engine.path,
        preset_source: source.path,
        presets,
        inputs,
        plates,
    })
}

/// D8: writes one plate's inputs into a fresh work directory.
fn write_inputs(
    work: &WorkDir,
    plate_3mf: &[u8],
    presets: &SlicePresetDocuments,
) -> io::Result<()> {
    if work.root().exists() {
        fs::remove_dir_all(work.root())?;
    }
    work.create()?;
    fs::write(work.plate_3mf(), plate_3mf)?;
    for (path, preset) in [
        (work.machine_json(), &presets.machine),
        (work.process_json(), &presets.process),
        (work.filament_json(), &presets.filament),
    ] {
        fs::write(
            path,
            serde_json::to_vec_pretty(preset).map_err(io::Error::other)?,
        )?;
    }
    Ok(())
}

/// D10 `start_slice`: queues one operation per requested plate and returns
/// them. Blocks while it checks the runtime, presets, and plates.
pub fn start_slice<R: tauri::Runtime>(
    services: &Arc<SlicingServices<R>>,
    request: &StartSliceRequest,
) -> Result<Vec<SliceOperationRecord>, CommandError> {
    if request.operation_id.trim().is_empty() {
        return Err(CommandError::from_repository(RepositoryError::Validation {
            field_path: "operationId",
        }));
    }
    let _one_at_a_time = lock(&services.scheduler.starting);
    let digest = ledger::digest(&StartSliceDigest {
        preparation_id: &request.preparation_id,
        expected_revision: request.expected_revision,
        plate_keys: &request.plate_keys,
        continue_with_source_revision: request.continue_with_source_revision.as_deref(),
    });
    if let Some(records) = replay(&services.storage, request, &digest)? {
        return Ok(records);
    }

    let preparation = services
        .storage
        .read(|connection| Ok(load_preparation(connection, &request.preparation_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?
        .ok_or_else(|| CommandError::not_found(request.preparation_id.clone()))?;
    check_startable(&preparation, request)?;
    let plates = requested_plates(&preparation, &request.plate_keys)?;
    let prepared = prepare(services, request, &preparation, plates)?;

    let content_root = services.storage.paths().content_root().to_path_buf();
    let written: Result<(), io::Error> = prepared.plates.iter().try_for_each(|plate| {
        write_inputs(
            &WorkDir::for_operation(&content_root, &plate.operation_id),
            &plate.plate_3mf,
            &prepared.presets,
        )
    });
    let remove_inputs = || {
        for plate in &prepared.plates {
            let _ = fs::remove_dir_all(
                WorkDir::for_operation(&content_root, &plate.operation_id).root(),
            );
        }
    };
    if written.is_err() {
        remove_inputs();
        return Err(CommandError::persistence_unavailable());
    }

    let committed = services.storage.write_repo(|tx| {
        if ledger::claim(
            tx,
            &request.operation_id,
            OperationKind::StartSlice,
            &digest,
        )? == Claim::Replay
        {
            // Unreachable while starts run one at a time.
            return Err(RepositoryError::OperationIdReused);
        }
        let current = load_preparation(tx, &request.preparation_id)?.ok_or_else(|| {
            RepositoryError::NotFound {
                entity_id: request.preparation_id.clone(),
            }
        })?;
        if current.revision != request.expected_revision {
            return Err(RepositoryError::Conflict {
                entity_id: current.id,
                expected_revision: request.expected_revision,
                current_revision: current.revision,
            });
        }
        prepared
            .plates
            .iter()
            .map(|plate| {
                insert_operation(
                    tx,
                    &NewSliceOperation {
                        id: plate.operation_id.clone(),
                        preparation_id: preparation.id.clone(),
                        source_revision_id: preparation.source_revision_id.clone(),
                        plate: plate.plate.clone(),
                    },
                )
            })
            .collect::<Result<Vec<_>, _>>()
    });
    let records = match committed {
        Ok(records) => records,
        Err(error) => {
            remove_inputs();
            return Err(CommandError::from_repository(error));
        }
    };

    let jobs = prepared
        .plates
        .iter()
        .map(|plate| {
            let (sender, receiver) = watch::channel(false);
            Job {
                operation_id: plate.operation_id.clone(),
                preparation_id: preparation.id.clone(),
                engine: prepared.engine.clone(),
                preset_source: prepared.preset_source.clone(),
                inputs: PublishInputs {
                    operation_id: plate.operation_id.clone(),
                    ..prepared.inputs.clone()
                },
                cancel_sender: Arc::new(sender),
                cancel: CancelFlag::new(receiver),
            }
        })
        .collect();
    services.publish(records.iter().map(events::operation_changed).collect());
    let ids: Vec<String> = records.iter().map(|record| record.id.clone()).collect();
    services.scheduler.reach(SchedulerPoint::Committed(&ids));
    enqueue(services, jobs);
    Ok(records)
}

/// D5: the Preparation must be at the expected revision, and a stale one
/// needs `continueWithSourceRevision` naming its pinned revision.
fn check_startable(
    preparation: &PreparationRecord,
    request: &StartSliceRequest,
) -> Result<(), CommandError> {
    if preparation.revision != request.expected_revision {
        return Err(CommandError::from_repository(RepositoryError::Conflict {
            entity_id: preparation.id.clone(),
            expected_revision: request.expected_revision,
            current_revision: preparation.revision,
        }));
    }
    match request.continue_with_source_revision.as_deref() {
        Some(revision) if revision != preparation.source_revision_id => {
            Err(CommandError::validation_at(
                "continueWithSourceRevision",
                "This isn't the revision the Preparation was made from.",
            ))
        }
        None if preparation.stale => Err(CommandError::preparation_stale(
            &preparation.id,
            &preparation.source_revision_id,
        )),
        _ => Ok(()),
    }
}

// ---------------------------------------------------------------------------
// Cancel
// ---------------------------------------------------------------------------

/// D9/D10 `cancel_slice_operation`. A queued operation leaves the queue and
/// becomes `cancelled` without anything being spawned. A running one is
/// stopped through its supervisor; this waits (up to about 10 s) for it to
/// settle and returns the record as it then stands. A finished operation
/// is `OPERATION_NOT_CANCELLABLE`. Blocks.
pub fn cancel_slice_operation<R: tauri::Runtime>(
    services: &Arc<SlicingServices<R>>,
    id: &str,
) -> Result<SliceOperationRecord, CommandError> {
    if let Some(cancelled) = cancel_scheduled(services, id, None) {
        return cancelled;
    }
    // Not queued or running here. A `start_slice` may be between its
    // commit and its enqueue; it holds `starting` throughout, so wait for
    // it (lock order: `starting`, then `state`) and look again.
    let scheduler = &services.scheduler;
    scheduler.reach(SchedulerPoint::CancelAwaitsStart(id));
    let starting = lock(&scheduler.starting);
    if let Some(cancelled) = cancel_scheduled(services, id, Some(starting)) {
        return cancelled;
    }
    let record =
        read_operation(&services.storage, id)?.ok_or_else(|| CommandError::not_found(id))?;
    match record.state {
        SliceOperationState::Queued | SliceOperationState::Running => {
            // No start is in progress and no job holds it, so it can't run
            // any more: end it.
            let events = cancel_unstarted(services, id);
            services.publish(events);
            read_operation(&services.storage, id)?.ok_or_else(|| CommandError::not_found(id))
        }
        state => Err(CommandError::operation_not_cancellable(
            id,
            &encode_enum(state),
        )),
    }
}

/// Cancels operation `id` if the scheduler holds it: a queued job leaves
/// the queue and is `cancelled` without spawning; a running one is
/// signalled and waited for. `None` when it holds neither. `starting` (if
/// held) is released once the scheduler's state is locked.
fn cancel_scheduled<R: tauri::Runtime>(
    services: &Arc<SlicingServices<R>>,
    id: &str,
    starting: Option<std::sync::MutexGuard<'_, ()>>,
) -> Option<Result<SliceOperationRecord, CommandError>> {
    let load = || -> Result<SliceOperationRecord, CommandError> {
        read_operation(&services.storage, id)?.ok_or_else(|| CommandError::not_found(id))
    };
    let scheduler = &services.scheduler;
    let mut state = lock(&scheduler.state);
    drop(starting);
    if let Some(position) = state.queue.iter().position(|job| job.operation_id == id) {
        let job = state
            .queue
            .remove(position)
            .expect("the position is in range");
        drop(state);
        let events = cancel_unstarted(services, id);
        let _ = fs::remove_dir_all(
            WorkDir::for_operation(services.storage.paths().content_root(), &job.operation_id)
                .root(),
        );
        services.publish(events);
        return Some(load());
    }
    let running = |state: &SchedulerState| {
        state
            .current
            .as_ref()
            .is_some_and(|current| current.operation_id == id)
    };
    if !running(&state) {
        return None;
    }
    if let Some(current) = state.current.as_ref() {
        let _ = current.cancel.send(true);
    }
    let deadline = Instant::now() + CANCEL_SETTLE;
    while running(&state) {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            break;
        }
        state = scheduler
            .settled
            .wait_timeout(state, left)
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .0;
    }
    drop(state);
    Some(load())
}

/// After a Model was deleted (its Preparation and operations cascade):
/// drops queued jobs whose rows are gone, and stops the running job if its
/// row or its Preparation is gone. The worker then stores nothing for it.
pub fn abandon_deleted_jobs<R: tauri::Runtime>(services: &SlicingServices<R>) {
    let exists = |sql: &str, id: &str| -> bool {
        services
            .storage
            .read(|connection| connection.query_row(sql, [id], |row| row.get::<_, bool>(0)))
            // Unknown is kept: the pre-spawn check and the commit's own
            // checks still stop a job whose rows are gone.
            .unwrap_or(true)
    };
    let operation_exists = |id: &str| {
        exists(
            "SELECT EXISTS(SELECT 1 FROM slice_operations WHERE id = ?1)",
            id,
        )
    };
    let preparation_exists = |id: &str| {
        exists(
            "SELECT EXISTS(SELECT 1 FROM slice_preparations WHERE id = ?1)",
            id,
        )
    };
    let content_root = services.storage.paths().content_root().to_path_buf();
    let mut state = lock(&services.scheduler.state);
    let mut dropped = Vec::new();
    state.queue.retain(|job| {
        let keep = operation_exists(&job.operation_id);
        if !keep {
            dropped.push(job.operation_id.clone());
        }
        keep
    });
    if let Some(current) = state.current.as_ref() {
        if !operation_exists(&current.operation_id) || !preparation_exists(&current.preparation_id)
        {
            let _ = current.cancel.send(true);
        }
    }
    drop(state);
    for id in dropped {
        let _ = fs::remove_dir_all(WorkDir::for_operation(&content_root, &id).root());
    }
}

/// Whether Preparation `preparation_id` has a queued or running operation.
pub fn has_active_operations(
    connection: &Connection,
    preparation_id: &str,
) -> Result<bool, StorageError> {
    Ok(connection.query_row(
        "SELECT EXISTS(SELECT 1 FROM slice_operations
                       WHERE preparation_id = ?1 AND state IN ('queued', 'running'))",
        [preparation_id],
        |row| row.get(0),
    )?)
}

// ---------------------------------------------------------------------------
// The log
// ---------------------------------------------------------------------------

/// D9 known noise: OrcaSlicer prints this on every headless run.
pub const KNOWN_NOISE: [&str; 1] = ["unable to open display"];

/// Where operation `id`'s log is stored: a succeeded operation's is its
/// revision's `log` blob (D13); a failed or cancelled one's is
/// `slice_operations.log_sha256`. `None` while it has none.
pub fn log_sha256(connection: &Connection, id: &str) -> Result<Option<String>, StorageError> {
    let row: Option<(String, Option<String>, Option<String>)> = connection
        .query_row(
            "SELECT state, log_sha256, slice_revision_id FROM slice_operations WHERE id = ?1",
            [id],
            |row| Ok((row.get(0)?, row.get(1)?, row.get(2)?)),
        )
        .optional()?;
    let Some((state, log_sha256, revision_id)) = row else {
        return Ok(None);
    };
    if state == "succeeded" {
        let Some(revision_id) = revision_id else {
            return Ok(None);
        };
        return Ok(connection
            .query_row(
                "SELECT sha256 FROM slice_revision_blobs WHERE revision_id = ?1 AND role = 'log'",
                [revision_id],
                |row| row.get(0),
            )
            .optional()?);
    }
    Ok(log_sha256)
}

/// The 1-based numbers of the lines of `text` that are known noise.
pub fn noise_lines(text: &str) -> Vec<u32> {
    text.lines()
        .enumerate()
        .filter(|(_, line)| {
            let line = line.to_ascii_lowercase();
            KNOWN_NOISE.iter().any(|noise| line.contains(noise))
        })
        .map(|(number, _)| number as u32 + 1)
        .collect()
}

/// Whether a stored log had its middle dropped (the supervisor's marker).
pub fn log_was_truncated(text: &str) -> bool {
    text.lines()
        .any(|line| line.starts_with("[farm3d: ") && line.ends_with(" bytes of log omitted]"))
}

// ---------------------------------------------------------------------------
// Recovery
// ---------------------------------------------------------------------------

/// What [`recover_after_restart`] did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Recovery {
    /// Every operation that was `queued` or `running`.
    pub interrupted: Vec<InterruptedOperation>,
    /// The ids of those whose OrcaSlicer was still running and was stopped.
    pub stopped: Vec<String>,
    /// Work directories removed.
    pub work_dirs_removed: usize,
}

/// Whether recorded process `pid`, started at `started_at`, is still that
/// OrcaSlicer: the same start time, and an executable named like the
/// engine (the configured one's basename, or `orca-slicer`, the program
/// inside every OrcaSlicer install and AppImage).
///
/// The name check can miss a real engine: a discovered (not configured)
/// AppImage may be recorded as its `AppRun` or a shell wrapper, depending
/// on how the AppImage launches, and neither is named `orca-slicer`. That
/// direction is the safe one: a missed engine is left running, while a
/// looser check could signal an unrelated process that reused the pid.
/// PDEATHSIG, set on every spawn, is the primary guard (the engine dies
/// with farm3d); this check is only the fallback for a survivor.
fn still_running_engine(pid: u32, started_at: i64, engine_path: Option<&str>) -> bool {
    if process_start_time(pid) != Some(started_at) {
        return false;
    }
    let Some(executable) = process_executable(pid) else {
        return false;
    };
    let name = executable
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default();
    let configured = engine_path
        .and_then(|path| Path::new(path).file_name())
        .map(|name| name.to_string_lossy().into_owned());
    name == PATH_EXECUTABLE || configured.is_some_and(|configured| configured == name)
}

/// D10 startup recovery, run after the content store's startup sweep and
/// before any command is served.
pub fn recover_after_restart(storage: &Storage) -> Result<Recovery, StorageError> {
    let config = storage.read(|connection| Ok(load_runtime_config(connection)))??;
    let interrupted = storage.write(mark_active_interrupted)?;
    let mut stopped = Vec::new();
    for operation in interrupted.iter().filter(|operation| operation.was_running) {
        let (Some(pid), Some(started_at)) = (operation.pid, operation.pid_started_at) else {
            continue;
        };
        let Ok(pid) = u32::try_from(pid) else {
            continue;
        };
        // PDEATHSIG normally ends it with farm3d; this is the fallback.
        if still_running_engine(pid, started_at, config.engine_path.as_deref()) {
            stop_recorded_group(pid as i32, STOP_GRACE);
            stopped.push(operation.id.clone());
        }
    }
    let work_root = storage.paths().content_root().join(WORK_ROOT_DIR);
    let mut work_dirs_removed = 0;
    if let Ok(entries) = fs::read_dir(&work_root) {
        for entry in entries.flatten() {
            let path = entry.path();
            let removed = match entry.file_type() {
                Ok(kind) if kind.is_dir() => fs::remove_dir_all(&path),
                Ok(_) => fs::remove_file(&path),
                Err(error) => Err(error),
            };
            match removed {
                Ok(()) => work_dirs_removed += 1,
                Err(error) => {
                    eprintln!(
                        "farm3d: could not remove slicing work {:?}: {error}",
                        entry.file_name()
                    )
                }
            }
        }
    }
    Ok(Recovery {
        interrupted,
        stopped,
        work_dirs_removed,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_ids_are_stable_distinct_and_fit_the_check() {
        let a = derived_operation_id("op-1", "plate-a");
        assert_eq!(a, derived_operation_id("op-1", "plate-a"));
        assert_ne!(a, derived_operation_id("op-1", "plate-b"));
        assert_ne!(a, derived_operation_id("op-2", "plate-a"));
        assert!(a.starts_with("sop-"));
        assert_eq!(a.len(), 40);
    }

    #[test]
    fn preset_values_read_as_orcaslicer_writes_them() {
        let preset = serde_json::json!({
            "nozzle_diameter": ["0.6"],
            "filament_type": ["\"PETG\""],
            "filament_diameter": "2.85",
            "empty": [],
        });
        assert_eq!(first_number(&preset, "nozzle_diameter"), Some(0.6));
        assert_eq!(
            first_text(&preset, "filament_type").as_deref(),
            Some("PETG")
        );
        assert_eq!(first_number(&preset, "filament_diameter"), Some(2.85));
        assert_eq!(first_text(&preset, "empty"), None);
        assert_eq!(first_text(&preset, "missing"), None);
    }

    #[test]
    fn the_log_marks_known_noise_and_truncation() {
        let text = "Loading\nError: unable to open display\nSlicing\n";
        assert_eq!(noise_lines(text), vec![2]);
        assert!(!log_was_truncated(text));
        assert!(log_was_truncated(
            "a\n[farm3d: 12 bytes of log omitted]\nb\n"
        ));
    }

    #[test]
    fn a_missing_or_foreign_pid_is_never_ours() {
        assert!(!still_running_engine(u32::MAX - 1, 1, None));
        // This test process is alive, but its start time is not 1.
        assert!(!still_running_engine(std::process::id(), 1, None));
    }
}
