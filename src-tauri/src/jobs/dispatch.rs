//! P7 D4/D7: a Job's handoffs to P6, and a Host Operation's outcome back
//! onto its Job.
//!
//! - [`stage_job`], [`start_job`], and [`control_job`] hand a Job's work
//!   to `host_ops::api` with a Job link ([`LinkInTx`]). P6 runs its whole
//!   D9 order and its write-ahead; the link runs inside that write-ahead
//!   transaction and claims the Job command's own `operationId`, re-checks
//!   the Job (it is the Printer's active Job, and in the right state), and
//!   writes the Job's `*HandedOff` event. The Host Operation's id is
//!   `<operationId>#hostOperation` (D4's operation-id table). Job code
//!   never holds the Printer lock around `host_ops::api`, which takes that
//!   non-reentrant lock itself (spec decision 9).
//! - [`apply_host_outcome`] maps a Job-linked Host Operation's current
//!   state onto its Job (D7's table). It is pure over the caller's
//!   transaction and idempotent: an op that is no longer the Job's active
//!   one, or already applied, writes nothing.
//! - [`start_blockers`] and [`may_start_unattended`] are D7's pure start
//!   checks; [`present_jobs`] fills `Job.startBlockers` from live status.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use rusqlite::{OptionalExtension, Transaction};
use serde::Serialize;

use crate::connections::capabilities::{
    CapabilityKey, CapabilityState, EvidenceTier, PrinterCapabilities,
};
use crate::connections::{ConnectionState, PrinterStatus};
use crate::contracts::command::{CommandError, RecoveryCode};
use crate::host_ops::api::{self, LinkInTx};
use crate::host_ops::start_rule::{self, ControlVerb};
use crate::host_ops::{
    repository as host_ops_repository, HostOperation, HostOperationKind, HostOperationState,
    PriorState,
};
use crate::persistence::{RepositoryError, Storage, StorageError};
use crate::printers::operational::{OperationalState, TelemetryFreshness};
use crate::printers::{StartSafety, StoredPrinter};
use crate::queue::{Blocker, BlockerCode, QueueChange};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::{encode_enum, SpoolLocation};
use crate::RuntimeServices;

use super::assign::JobDigest;
use super::repository::{self as jobs_repository, JobChange};
use super::{
    Job, JobAction, JobEventKind, JobFailure, JobState, ReconciliationRequirement, RequirementKind,
    StartConfirmation,
};

// --- apply_host_outcome ------------------------------------------------------

/// What [`apply_host_outcome`] committed: the Job, and any Reconciliation
/// Requirement it opened (`StartAbandoned`'s `jobOutcomeUnknown`).
#[derive(Clone, Debug)]
pub struct Applied {
    pub job: Job,
    pub requirements: Vec<ReconciliationRequirement>,
}

impl Applied {
    pub fn change(&self) -> QueueChange {
        QueueChange {
            jobs: vec![self.job.clone()],
            requirements: self.requirements.clone(),
            ..QueueChange::default()
        }
    }
}

fn failed_with(op: &HostOperation, now: &str) -> Result<JobFailure, RepositoryError> {
    // P6 D2: a `failed` row always carries its failure.
    let failure = op
        .failure
        .clone()
        .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
    Ok(JobFailure::HostOperationFailed {
        at: now.to_string(),
        host_operation_id: op.id.clone(),
        failure,
    })
}

/// D7's `apply_host_outcome` table: the Job event (and its column changes)
/// a terminal op of `op.kind` in `op.state` raises while the Job is in
/// `state`. `None` for every row the table says writes no event.
fn event_for(
    state: JobState,
    op: &HostOperation,
    now: &str,
) -> Result<Option<(JobEventKind, JobChange)>, RepositoryError> {
    use HostOperationKind as Kind;
    use HostOperationState as Op;
    use JobState as Job;

    let event = match (op.kind, op.state, state) {
        (Kind::Upload, Op::Succeeded, Job::Staging) => (
            JobEventKind::StageSucceeded,
            JobChange {
                upload_host_operation_id: Some(op.id.clone()),
                host_path: Some(op.host_path.clone()),
                clear_last_failure: true,
                ..JobChange::default()
            },
        ),
        (Kind::Upload, Op::Failed, Job::Staging) => (
            JobEventKind::StageFailed,
            JobChange {
                last_failure: Some(failed_with(op, now)?),
                // D7: a failed re-stage no longer trusts the old file.
                clear_upload_host_operation_id: true,
                ..JobChange::default()
            },
        ),
        (Kind::Upload, Op::Abandoned, Job::Staging) => (
            JobEventKind::StageFailed,
            JobChange {
                last_failure: Some(JobFailure::HostOperationAbandoned {
                    at: now.to_string(),
                    host_operation_id: op.id.clone(),
                }),
                clear_upload_host_operation_id: true,
                ..JobChange::default()
            },
        ),
        (Kind::Start, Op::Succeeded, Job::Starting) => (
            JobEventKind::StartSucceeded,
            JobChange {
                // Ruling R13(b): when the start was sent, not when its
                // outcome was applied (a restart can apply it much later).
                started_at: Some(op.dispatched_at.clone().unwrap_or_else(|| now.to_string())),
                history_mark: op.history_mark,
                ..JobChange::default()
            },
        ),
        (Kind::Start, Op::Failed, Job::Starting) => (
            JobEventKind::StartFailed,
            JobChange {
                last_failure: Some(failed_with(op, now)?),
                ..JobChange::default()
            },
        ),
        (Kind::Start, Op::Abandoned, Job::Starting) => {
            (JobEventKind::StartAbandoned, JobChange::default())
        }
        (Kind::Pause, Op::Succeeded, Job::Printing) => (JobEventKind::Paused, JobChange::default()),
        (Kind::Resume, Op::Succeeded, Job::Paused) => (JobEventKind::Resumed, JobChange::default()),
        (
            Kind::Pause | Kind::Resume | Kind::Cancel,
            Op::Failed | Op::Abandoned,
            Job::Printing | Job::Paused,
        ) => (
            JobEventKind::ControlFailed,
            JobChange {
                detail: Some(serde_json::json!({
                    "hostOperationId": op.id,
                    "kind": encode_enum(op.kind),
                    "failure": op.failure,
                })),
                ..JobChange::default()
            },
        ),
        // A succeeded cancel writes no event: the tracker checks history
        // at once and proves the end (D7). Every other pair is already
        // applied, or can't follow.
        _ => return Ok(None),
    };
    Ok(Some(event))
}

/// D7: maps a Job-linked Host Operation's current state onto its Job, in
/// the caller's transaction. Idempotent: it acts only while `op` is the
/// Job's `active_host_operation_id` and terminal, and in the same UPDATE
/// clears that column (in every Job state, terminal ones included), so a
/// second call finds nothing to do. Returns `None` when nothing was
/// written. An op with no `job_id` is ignored.
pub fn apply_host_outcome(
    tx: &Transaction<'_>,
    op: &HostOperation,
    now: &str,
) -> Result<Option<Applied>, RepositoryError> {
    let Some(job_id) = op.job_id.as_deref() else {
        return Ok(None);
    };
    let Some(job) = jobs_repository::load_job(tx, job_id)? else {
        return Ok(None);
    };
    let is_active = job.active_host_operation_id.as_deref() == Some(op.id.as_str());
    let terminal = matches!(
        op.state,
        HostOperationState::Succeeded | HostOperationState::Failed | HostOperationState::Abandoned
    );
    if !is_active || !terminal {
        return Ok(None);
    }

    let mut requirements = Vec::new();
    let job = match event_for(job.state, op, now)? {
        Some((event, mut change)) => {
            change.clear_active_host_operation_id = true;
            change.host_operation_id = Some(op.id.clone());
            let job = jobs_repository::transition(tx, job_id, event, change, now)?;
            if event == JobEventKind::StartAbandoned {
                requirements.extend(open_outcome_unknown_requirement(tx, job_id, now)?);
            }
            job
        }
        None => jobs_repository::update_columns(
            tx,
            job_id,
            JobChange {
                clear_active_host_operation_id: true,
                ..JobChange::default()
            },
            now,
        )?,
    };
    Ok(Some(Applied { job, requirements }))
}

/// D1/D9: opens the Job's `jobOutcomeUnknown` requirement (`pending`),
/// unless it already has one (one per `(job, kind)`). Returns what it
/// opened.
pub(crate) fn open_outcome_unknown_requirement(
    tx: &Transaction<'_>,
    job_id: &str,
    now: &str,
) -> Result<Option<ReconciliationRequirement>, RepositoryError> {
    let exists = jobs_repository::requirements_for_job(tx, job_id)?
        .iter()
        .any(|requirement| requirement.kind == RequirementKind::JobOutcomeUnknown);
    if exists {
        return Ok(None);
    }
    jobs_repository::open_requirement(
        tx,
        job_id,
        RequirementKind::JobOutcomeUnknown,
        None,
        None,
        now,
    )
    .map(Some)
}

// --- start checks ------------------------------------------------------------

fn any_start_offered(status: &PrinterStatus) -> bool {
    [
        PriorState::Ready,
        PriorState::Finished,
        PriorState::Cancelled,
    ]
    .into_iter()
    .any(|prior| start_rule::check(status, prior).is_ok())
}

/// D7's start blockers for an `awaitingStart` Job, in the table's order
/// (every one that applies). Empty for a Job in any other state.
/// `spool_number` only names the Spool in the "Awaiting material" message.
/// The `SPOOL_NOT_LOADED` start blocker: "Awaiting material: load Spool
/// #N on <Printer>."
fn spool_not_loaded(job: &Job, printer: &StoredPrinter, spool_number: Option<i64>) -> Blocker {
    let spool = spool_number
        .map(|number| format!("Spool #{number}"))
        .unwrap_or_else(|| "this Job's Spool".to_string());
    Blocker {
        code: BlockerCode::SpoolNotLoaded,
        message: format!("Awaiting material: load {spool} on {}.", printer.name),
        detail: None,
        recovery: Some(RecoveryCode::LoadSpool),
        printer_ids: vec![job.printer_id.clone()],
    }
}

pub fn start_blockers(
    job: &Job,
    printer: &StoredPrinter,
    status: Option<&PrinterStatus>,
    loaded: &[String],
    caps: &PrinterCapabilities,
    unresolved_host_operation: bool,
    spool_number: Option<i64>,
) -> Vec<Blocker> {
    if job.state != JobState::AwaitingStart {
        return Vec::new();
    }
    let blocker = |code, message: String, detail, recovery| Blocker {
        code,
        message,
        detail,
        recovery,
        printer_ids: vec![job.printer_id.clone()],
    };
    let mut blockers = Vec::new();

    if !loaded.contains(&job.spool_id) {
        blockers.push(spool_not_loaded(job, printer, spool_number));
    }
    if unresolved_host_operation {
        blockers.push(blocker(
            BlockerCode::HostOperationPending,
            "This Printer has a pending printer operation.".to_string(),
            None,
            Some(RecoveryCode::OpenPrinterJob),
        ));
    }
    for key in [CapabilityKey::Start, CapabilityKey::ArtifactIdentity] {
        if let CapabilityState::Unsupported { detail, .. } = &caps.capabilities[key] {
            blockers.push(blocker(
                BlockerCode::CapabilityUnsupported,
                detail.clone(),
                Some(detail.clone()),
                None,
            ));
            break;
        }
    }
    let offline = PrinterStatus::new(ConnectionState::Offline);
    let status = status.unwrap_or(&offline);
    if !any_start_offered(status) {
        blockers.push(blocker(
            BlockerCode::PrinterNotReady,
            format!(
                "The printer can't start now: {}.",
                start_rule::state_label(status.operational_state, status.freshness)
            ),
            None,
            None,
        ));
    }
    blockers
}

/// D7, owner decision 1: the driver may start `job` without a bed-clear
/// confirmation only when the Printer's Start-safety rule is `unattended`,
/// it is `ready` with fresh telemetry (never `finished` or `cancelled`),
/// nothing blocks the start, and `upload`, `start`, `hostState`, and
/// `artifactIdentity` all have `tier: sim` evidence.
pub fn may_start_unattended(
    job: &Job,
    printer: &StoredPrinter,
    status: &PrinterStatus,
    caps: &PrinterCapabilities,
    loaded: &[String],
    unresolved_host_operation: bool,
) -> bool {
    const PROVEN: [CapabilityKey; 4] = [
        CapabilityKey::Upload,
        CapabilityKey::Start,
        CapabilityKey::HostState,
        CapabilityKey::ArtifactIdentity,
    ];
    let sim_proven = PROVEN.iter().all(|key| {
        matches!(
            &caps.capabilities[*key],
            CapabilityState::Supported { evidence } if evidence.tier == EvidenceTier::Sim
        )
    });
    job.state == JobState::AwaitingStart
        && printer.start_safety == StartSafety::Unattended
        && status.connection_state == ConnectionState::Online
        && status.operational_state == OperationalState::Ready
        && status.freshness == TelemetryFreshness::Fresh
        && sim_proven
        && start_blockers(
            job,
            printer,
            Some(status),
            loaded,
            caps,
            unresolved_host_operation,
            None,
        )
        .is_empty()
}

// --- live start context --------------------------------------------------------

/// What [`start_blockers`] and [`may_start_unattended`] read for one Job:
/// its Printer, the live status and capabilities, whether its Spool is
/// loaded there, and whether the Printer has an unresolved Host Operation.
pub(crate) struct StartContext {
    pub printer: StoredPrinter,
    pub status: Option<PrinterStatus>,
    pub caps: PrinterCapabilities,
    pub loaded: Vec<String>,
    pub spool_number: Option<i64>,
    pub unresolved_host_operation: bool,
}

impl StartContext {
    pub(crate) fn read<R: tauri::Runtime>(
        services: &RuntimeServices<R>,
        job: &Job,
    ) -> Result<Option<Self>, RepositoryError> {
        let Some(printer) = services.host_ops.load_printer(&job.printer_id)? else {
            return Ok(None);
        };
        let (spool, unresolved) = services
            .storage
            .read(|connection| {
                Ok((|| {
                    let spool = crate::spools::repository::load_record(connection, &job.spool_id)?;
                    let unresolved =
                        host_ops_repository::has_unresolved(connection, &job.printer_id)?;
                    Ok::<_, RepositoryError>((spool, unresolved))
                })())
            })
            .map_err(RepositoryError::Storage)??;
        let loaded = spool
            .as_ref()
            .filter(|spool| {
                matches!(&spool.location, SpoolLocation::Slot { printer_id, .. } if *printer_id == job.printer_id)
            })
            .map(|spool| vec![spool.id.clone()])
            .unwrap_or_default();
        Ok(Some(Self {
            status: services.manager.statuses().remove(&job.printer_id),
            caps: services.host_ops.capabilities(&printer),
            spool_number: spool.map(|spool| spool.spool_number),
            loaded,
            unresolved_host_operation: unresolved,
            printer,
        }))
    }

    pub(crate) fn blockers(&self, job: &Job) -> Vec<Blocker> {
        start_blockers(
            job,
            &self.printer,
            self.status.as_ref(),
            &self.loaded,
            &self.caps,
            self.unresolved_host_operation,
            self.spool_number,
        )
    }

    pub(crate) fn may_start_unattended(&self, job: &Job) -> bool {
        self.status.as_ref().is_some_and(|status| {
            may_start_unattended(
                job,
                &self.printer,
                status,
                &self.caps,
                &self.loaded,
                self.unresolved_host_operation,
            )
        })
    }
}

/// Fills `startBlockers` for every `awaitingStart` Job in `jobs` from live
/// status, just before they leave Rust (a command result, an event, or a
/// snapshot). Rust computes them; the frontend never derives them (C1).
/// A Printer that can't be read leaves the Job's blockers empty.
///
/// A `printing`/`paused` Job's `allowedActions` are recomputed here with
/// the runtime's own `JobTimings.unreachable_declare_after` and clock
/// (D3: "computed at read time"); the decode-time value used the
/// production defaults.
pub(crate) fn present_jobs<R: tauri::Runtime>(services: &RuntimeServices<R>, jobs: &mut [Job]) {
    for job in jobs.iter_mut() {
        match job.state {
            JobState::AwaitingStart => {
                if let Ok(Some(context)) = StartContext::read(services, job) {
                    job.start_blockers = context.blockers(job);
                }
            }
            JobState::Printing | JobState::Paused => {
                // Retry needs a terminal Job, so the successor flag can't
                // matter here.
                job.allowed_actions = super::state::allowed_actions(
                    job,
                    false,
                    services.jobs.now(),
                    services.jobs.timings.unreachable_declare_after,
                );
            }
            _ => {}
        }
    }
}

pub(crate) fn present_change<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    change: &mut QueueChange,
) {
    present_jobs(services, &mut change.jobs);
}

// --- handoffs ------------------------------------------------------------------

/// A handoff's result: the Job as it now is (reloaded, never assumed from
/// the host-ops call's `Ok`), and whether this call was a replay (the link
/// never ran), in which case nothing is published.
#[derive(Clone, Debug)]
pub struct Handoff {
    pub job: Job,
    pub replayed: bool,
}

impl Handoff {
    pub fn change(&self) -> QueueChange {
        QueueChange {
            jobs: vec![self.job.clone()],
            ..QueueChange::default()
        }
    }
}

/// D4's ledger digest for `start_job` (fields in this order).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartDigest<'a> {
    job_id: &'a str,
    prior_state: PriorState,
    acknowledgement: StartConfirmation,
}

/// D4: the Host Operation's id for a Job command's `operationId`.
pub fn host_operation_id_for(operation_id: &str) -> String {
    format!("{operation_id}#hostOperation")
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// D4 "Replay", read-only (as P6's `is_replay`): `true` when
/// `operation_id` already recorded exactly this request; a reused id for
/// another request is `VALIDATION` on `operationId`.
fn is_replay(
    storage: &Storage,
    operation_id: &str,
    kind: OperationKind,
    digest: &str,
) -> Result<bool, CommandError> {
    if operation_id.trim().is_empty() {
        return Err(CommandError::from_repository(RepositoryError::Validation {
            field_path: "operationId",
        }));
    }
    let recorded: Option<(String, String)> = storage
        .read(|connection| {
            connection
                .query_row(
                    "SELECT kind, request_digest FROM operations WHERE id = ?1",
                    [operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
        .map_err(storage_error)?;
    match recorded {
        None => Ok(false),
        Some((recorded_kind, recorded_digest))
            if recorded_kind == encode_enum(kind) && recorded_digest == digest =>
        {
            Ok(true)
        }
        Some(_) => Err(CommandError::from_repository(
            RepositoryError::OperationIdReused,
        )),
    }
}

fn load_committed(storage: &Storage, job_id: &str) -> Result<Job, CommandError> {
    storage
        .read(|connection| Ok(jobs_repository::load_job(connection, job_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?
        .ok_or_else(|| CommandError::not_found(job_id))
}

fn not_allowed(job: &Job, action: JobAction) -> RepositoryError {
    RepositoryError::JobActionNotAllowed {
        job_id: job.id.clone(),
        action,
        state: job.state,
    }
}

/// The action's D3 event is legal from the Job's state.
fn require(job: &Job, event: JobEventKind, action: JobAction) -> Result<(), RepositoryError> {
    super::state::transition(job.state, event)
        .map(|_| ())
        .map_err(|_| not_allowed(job, action))
}

/// The in-transaction half every link shares (C8): claims the Job
/// command's own id, re-loads the Job, requires the action's event to be
/// legal from its state, and requires the Job to be the op's Printer's
/// active Job — a linked write skips P6's raw `JOB_ACTIVE` guard, so the
/// link is that guard. Never uses `Validation { printerId }`, which the
/// write-ahead would remap to "archived".
#[allow(clippy::too_many_arguments)]
fn claim_and_recheck(
    tx: &Transaction<'_>,
    op: &HostOperation,
    operation_id: &str,
    kind: OperationKind,
    digest: &str,
    job_id: &str,
    event: JobEventKind,
    action: JobAction,
) -> Result<Job, RepositoryError> {
    if operations::claim(tx, operation_id, kind, digest)? == Claim::Replay {
        // `is_replay` ran first, and the Host Operation's own derived id
        // was fresh: this id can't have been used for this request.
        return Err(RepositoryError::OperationIdReused);
    }
    let job = jobs_repository::load_job(tx, job_id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: job_id.to_string(),
    })?;
    require(&job, event, action)?;
    match jobs_repository::active_job_for_printer(tx, &op.printer_id)? {
        Some(active) if active.id == job.id => Ok(job),
        Some(active) => Err(RepositoryError::JobActive {
            printer_id: op.printer_id.clone(),
            job_id: active.id,
        }),
        None => Err(not_allowed(&job, action)),
    }
}

fn handed_off(job: Job, linked: &AtomicBool) -> Handoff {
    Handoff {
        job,
        replayed: !linked.load(Ordering::SeqCst),
    }
}

/// D3/D4/D7 "Stage": hands the Job's Slice Revision to P6's upload. Legal
/// from `assigned` and (staging again) `awaitingStart`. The driver calls it
/// with a `drv-*` id after assignment; the operator through `stage_job`.
pub(crate) async fn stage_job<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    operation_id: String,
    job_id: &str,
) -> Result<Handoff, CommandError> {
    let digest = operations::digest(&JobDigest { job_id });
    if is_replay(
        &services.storage,
        &operation_id,
        OperationKind::StageJob,
        &digest,
    )? {
        return Ok(Handoff {
            job: load_committed(&services.storage, job_id)?,
            replayed: true,
        });
    }
    let job = load_committed(&services.storage, job_id)?;
    require(&job, JobEventKind::StageHandedOff, JobAction::Stage)
        .map_err(CommandError::from_repository)?;

    let linked = Arc::new(AtomicBool::new(false));
    let link: LinkInTx<'_> = {
        let linked = Arc::clone(&linked);
        let operation_id = operation_id.clone();
        let job_id = job_id.to_string();
        Box::new(move |tx, op| {
            let job = claim_and_recheck(
                tx,
                op,
                &operation_id,
                OperationKind::StageJob,
                &digest,
                &job_id,
                JobEventKind::StageHandedOff,
                JobAction::Stage,
            )?;
            if op.slice_revision_id.as_deref() != Some(job.slice_revision_id.as_str()) {
                return Err(not_allowed(&job, JobAction::Stage));
            }
            jobs_repository::transition(
                tx,
                &job_id,
                JobEventKind::StageHandedOff,
                JobChange {
                    active_host_operation_id: Some(op.id.clone()),
                    operation_id: Some(operation_id.clone()),
                    host_operation_id: Some(op.id.clone()),
                    ..JobChange::default()
                },
                &op.created_at,
            )?;
            linked.store(true, Ordering::SeqCst);
            Ok(())
        })
    };
    api::stage(
        &services.host_ops,
        host_operation_id_for(&operation_id),
        job.printer_id.clone(),
        job.slice_revision_id.clone(),
        Some((job.id.clone(), link)),
    )
    .await?;
    Ok(handed_off(
        load_committed(&services.storage, job_id)?,
        &linked,
    ))
}

/// D7 "Start": `start_blockers` must be empty (`JOB_START_BLOCKED`), then
/// P6's whole start order runs and its errors pass through unchanged. The
/// link requires `awaitingStart` and the staged upload P6 is starting to
/// be the Job's own, then writes `StartHandedOff` with `confirmation`.
pub(crate) async fn start_job<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    operation_id: String,
    job_id: &str,
    prior: PriorState,
    confirmation: StartConfirmation,
) -> Result<Handoff, CommandError> {
    let digest = operations::digest(&StartDigest {
        job_id,
        prior_state: prior,
        acknowledgement: confirmation,
    });
    if is_replay(
        &services.storage,
        &operation_id,
        OperationKind::StartJob,
        &digest,
    )? {
        return Ok(Handoff {
            job: load_committed(&services.storage, job_id)?,
            replayed: true,
        });
    }
    let job = load_committed(&services.storage, job_id)?;
    require(&job, JobEventKind::StartHandedOff, JobAction::Start)
        .map_err(CommandError::from_repository)?;
    let context = StartContext::read(services, &job)
        .map_err(CommandError::from_repository)?
        .ok_or_else(|| CommandError::not_found(&job.printer_id))?;
    let blockers = context.blockers(&job);
    if !blockers.is_empty() {
        return Err(CommandError::job_start_blocked(job_id, &blockers));
    }
    // D3 CHECK: an `awaitingStart` Job always has its staged upload.
    let upload = job
        .upload_host_operation_id
        .clone()
        .ok_or_else(CommandError::internal)?;

    let linked = Arc::new(AtomicBool::new(false));
    let link: LinkInTx<'_> = {
        let linked = Arc::clone(&linked);
        let operation_id = operation_id.clone();
        let job_id = job_id.to_string();
        let printer = context.printer.clone();
        let spool_number = context.spool_number;
        Box::new(move |tx, op| {
            let job = claim_and_recheck(
                tx,
                op,
                &operation_id,
                OperationKind::StartJob,
                &digest,
                &job_id,
                JobEventKind::StartHandedOff,
                JobAction::Start,
            )?;
            if job.upload_host_operation_id.is_none()
                || job.upload_host_operation_id != op.source_host_operation_id
            {
                return Err(not_allowed(&job, JobAction::Start));
            }
            recheck_start_world(tx, &job, &printer, spool_number, confirmation)?;
            jobs_repository::transition(
                tx,
                &job_id,
                JobEventKind::StartHandedOff,
                JobChange {
                    start_confirmation: Some(confirmation),
                    active_host_operation_id: Some(op.id.clone()),
                    start_host_operation_id: Some(op.id.clone()),
                    operation_id: Some(operation_id.clone()),
                    host_operation_id: Some(op.id.clone()),
                    ..JobChange::default()
                },
                &op.created_at,
            )?;
            linked.store(true, Ordering::SeqCst);
            Ok(())
        })
    };
    if let Some(hook) = services.jobs.take_before_start_link() {
        hook(&services.storage);
    }
    api::start(
        &services.host_ops,
        host_operation_id_for(&operation_id),
        job.printer_id.clone(),
        upload,
        prior,
        confirmation == StartConfirmation::Unattended,
        Some((job.id.clone(), link)),
    )
    .await?;
    Ok(handed_off(
        load_committed(&services.storage, job_id)?,
        &linked,
    ))
}

/// Ruling R13(a): what the start link re-checks inside the write-ahead
/// transaction, because the world can change after the pre-checks: the
/// Job's Spool is still in one of this Printer's Material Slots
/// (`JOB_START_BLOCKED` with `SPOOL_NOT_LOADED`), and, for an unattended
/// start, the Printer's `startSafety` is still `unattended`
/// (`START_PRECONDITION_CHANGED`).
fn recheck_start_world(
    tx: &Transaction<'_>,
    job: &Job,
    printer: &StoredPrinter,
    spool_number: Option<i64>,
    confirmation: StartConfirmation,
) -> Result<(), RepositoryError> {
    let loaded_on: Option<String> = tx
        .query_row(
            "SELECT slots.printer_id FROM spools
             JOIN material_slots slots ON slots.id = spools.slot_id
             WHERE spools.id = ?1",
            [&job.spool_id],
            |row| row.get(0),
        )
        .optional()?;
    if loaded_on.as_deref() != Some(job.printer_id.as_str()) {
        return Err(RepositoryError::JobStartBlocked {
            job_id: job.id.clone(),
            blockers: vec![spool_not_loaded(job, printer, spool_number)],
        });
    }
    if confirmation == StartConfirmation::Unattended {
        let start_safety: String = tx.query_row(
            "SELECT start_safety FROM printers WHERE id = ?1",
            [&job.printer_id],
            |row| row.get(0),
        )?;
        if start_safety != "unattended" {
            return Err(RepositoryError::StartSafetyChanged {
                job_id: job.id.clone(),
                printer_id: job.printer_id.clone(),
            });
        }
    }
    Ok(())
}

/// D7 "Pause, resume, cancel after start": P6's control rule and host
/// re-read apply unchanged. The link also requires the file the host
/// reports (the op's `host_path`) to be the Job's own, else
/// `JOB_NOT_ON_PRINTER` and nothing is sent.
pub(crate) async fn control_job<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    operation_id: String,
    job_id: &str,
    verb: ControlVerb,
) -> Result<Handoff, CommandError> {
    let (kind, event, action) = match verb {
        ControlVerb::Pause => (
            OperationKind::PauseJob,
            JobEventKind::PauseHandedOff,
            JobAction::Pause,
        ),
        ControlVerb::Resume => (
            OperationKind::ResumeJob,
            JobEventKind::ResumeHandedOff,
            JobAction::Resume,
        ),
        ControlVerb::Cancel => (
            OperationKind::CancelJob,
            JobEventKind::CancelHandedOff,
            JobAction::Cancel,
        ),
    };
    let digest = operations::digest(&JobDigest { job_id });
    if is_replay(&services.storage, &operation_id, kind, &digest)? {
        return Ok(Handoff {
            job: load_committed(&services.storage, job_id)?,
            replayed: true,
        });
    }
    let job = load_committed(&services.storage, job_id)?;
    require(&job, event, action).map_err(CommandError::from_repository)?;

    let linked = Arc::new(AtomicBool::new(false));
    let link: LinkInTx<'_> = {
        let linked = Arc::clone(&linked);
        let operation_id = operation_id.clone();
        let job_id = job_id.to_string();
        Box::new(move |tx, op| {
            let job =
                claim_and_recheck(tx, op, &operation_id, kind, &digest, &job_id, event, action)?;
            if job.host_path.as_deref() != Some(op.host_path.as_str()) {
                return Err(RepositoryError::JobNotOnPrinter {
                    job_id: job.id,
                    printer_id: op.printer_id.clone(),
                });
            }
            jobs_repository::transition(
                tx,
                &job_id,
                event,
                JobChange {
                    active_host_operation_id: Some(op.id.clone()),
                    operation_id: Some(operation_id.clone()),
                    host_operation_id: Some(op.id.clone()),
                    ..JobChange::default()
                },
                &op.created_at,
            )?;
            linked.store(true, Ordering::SeqCst);
            Ok(())
        })
    };
    api::control(
        &services.host_ops,
        host_operation_id_for(&operation_id),
        job.printer_id.clone(),
        verb,
        Some((job.id.clone(), link)),
    )
    .await?;
    Ok(handed_off(
        load_committed(&services.storage, job_id)?,
        &linked,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::capabilities::{
        CapabilityEvidence, CapabilityMap, HostOperationFailureCode, InconclusiveReason,
        UnsupportedReason,
    };
    use crate::host_ops::repository::{NewHostOperation, Outcome};
    use crate::host_ops::{HostOperationEndpoint, HostOperationFailure, HostOperationResolution};
    use crate::jobs::repository::{insert_job, new_job_id, NewJob};
    use crate::jobs::{AssignedBy, PrinterSnapshot, RequirementStatus};
    use crate::persistence::MetadataRootLease;
    use crate::printers::repository::PrinterRepository;
    use crate::queue::repository::{create_entries, NewEntries};
    use crate::queue::state::EntryEvent;
    use crate::queue::{DispatchPolicy, DispatchPreference, EstimateSource, MaterialEstimate};
    use crate::slicing::repository::fixtures::{a_farm3d_revision, seed};
    use crate::slicing::repository::insert_farm3d_revision;
    use crate::spools::reservations::{self, ReservationHolder};
    use crate::spools::{
        repository as spools_repository, AmountConfidence, FilamentDiameter, MaterialFamily,
        SpoolFields,
    };

    const NOW: &str = "2026-09-27T12:00:00Z";
    const SLR: &str = "slr-a";
    const PRINTER: &str = "prn-a";

    struct Rig {
        _temp: tempfile::TempDir,
        _lease: MetadataRootLease,
        storage: Arc<Storage>,
        job_id: String,
    }

    /// One `assigned` Job over a seeded revision, Printer, and Spool.
    fn rig() -> Rig {
        let (temp, lease, storage) = crate::test_storage();
        storage
            .write(|tx| {
                seed(tx);
                Ok(())
            })
            .unwrap();
        storage
            .write_repo(|tx| insert_farm3d_revision(tx, &a_farm3d_revision(SLR, 1)))
            .unwrap();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: PRINTER.to_string(),
                name: "Printer".to_string(),
                ..Default::default()
            })
            .unwrap();
        let spool = storage
            .write_repo(|tx| {
                spools_repository::insert_spool(
                    tx,
                    &SpoolFields {
                        manufacturer: "Polymaker".to_string(),
                        product: None,
                        material_family: MaterialFamily::Pla,
                        material_other: None,
                        color_name: "Black".to_string(),
                        color_hex: None,
                        diameter: FilamentDiameter::D175,
                        nominal_mg: 1_000_000,
                        low_threshold_mg: 100_000,
                        tare_id: None,
                        notes: None,
                    },
                    &crate::spools::ledger::AmountEntry::Net {
                        net_mg: 1_000_000,
                        confidence: AmountConfidence::Estimated,
                    },
                    None,
                )
            })
            .unwrap();
        let entry = storage
            .write_repo(|tx| {
                create_entries(
                    tx,
                    &NewEntries {
                        slice_revision_id: SLR.to_string(),
                        quantity: 1,
                        policy: DispatchPolicy::Manual,
                        preference: DispatchPreference::LoadedFirst,
                        estimate: MaterialEstimate {
                            amount_mg: 100_000,
                            source: EstimateSource::SliceEstimate,
                        },
                        manual_printer_id: None,
                        lineage_id: None,
                    },
                    NOW,
                )
            })
            .unwrap()
            .remove(0);
        let job_id = new_job_id();
        storage
            .write_repo(|tx| {
                let reservation_id = reservations::reserve(
                    tx,
                    &spool.id,
                    &ReservationHolder {
                        kind: "job".to_string(),
                        id: job_id.clone(),
                    },
                    100_000,
                    "op-reserve",
                )
                .map_err(|_| RepositoryError::Storage(StorageError::OperationFailed))?;
                let job = insert_job(
                    tx,
                    &NewJob {
                        id: job_id.clone(),
                        queue_entry_id: entry.id.clone(),
                        slice_revision_id: SLR.to_string(),
                        printer_id: PRINTER.to_string(),
                        printer_snapshot: PrinterSnapshot {
                            name: "Printer".to_string(),
                            location: None,
                            catalog_ref: None,
                            adapter_kind: None,
                            profile: crate::jobs::state::tests_support::a_profile(),
                        },
                        spool_id: spool.id.clone(),
                        reservation_id,
                        estimate_mg: 100_000,
                        assigned_by: AssignedBy::Operator,
                        manual_facts_acknowledged: false,
                    },
                    NOW,
                )?;
                crate::queue::repository::apply(
                    tx,
                    &entry.id,
                    &EntryEvent::Assign,
                    Some(&job.id),
                    NOW,
                )?;
                Ok(())
            })
            .unwrap();
        Rig {
            _temp: temp,
            _lease: lease,
            storage,
            job_id,
        }
    }

    const OP_KINDS: [HostOperationKind; 5] = [
        HostOperationKind::Upload,
        HostOperationKind::Start,
        HostOperationKind::Pause,
        HostOperationKind::Resume,
        HostOperationKind::Cancel,
    ];
    const OP_STATES: [HostOperationState; 5] = [
        HostOperationState::Dispatching,
        HostOperationState::Uncertain,
        HostOperationState::Succeeded,
        HostOperationState::Failed,
        HostOperationState::Abandoned,
    ];

    fn operation_kind(kind: HostOperationKind) -> OperationKind {
        match kind {
            HostOperationKind::Upload => OperationKind::StageSliceRevision,
            HostOperationKind::Start => OperationKind::StartStagedArtifact,
            HostOperationKind::Pause => OperationKind::PauseHostPrint,
            HostOperationKind::Resume => OperationKind::ResumeHostPrint,
            HostOperationKind::Cancel => OperationKind::CancelHostPrint,
        }
    }

    /// Inserts a linked `kind` op for the rig's Job and drives it to
    /// `state` (P6's own repository transitions).
    fn insert_op(
        tx: &Transaction<'_>,
        job_id: Option<&str>,
        kind: HostOperationKind,
        state: HostOperationState,
    ) -> HostOperation {
        let row = host_ops_repository::insert_dispatching(
            tx,
            &NewHostOperation {
                operation_id: format!("op-{}", uuid::Uuid::new_v4()),
                operation_kind: operation_kind(kind),
                request_digest: "d".to_string(),
                printer_id: PRINTER.to_string(),
                kind,
                slice_revision_id: Some(SLR.to_string()),
                source_host_operation_id: None,
                gcode_sha256: matches!(kind, HostOperationKind::Upload | HostOperationKind::Start)
                    .then(|| "a".repeat(64)),
                gcode_size: matches!(kind, HostOperationKind::Upload | HostOperationKind::Start)
                    .then_some(100),
                host_path: "farm3d/slr-a.gcode".to_string(),
                history_mark: (kind == HostOperationKind::Start).then_some(7),
                endpoint: HostOperationEndpoint {
                    kind: "moonraker".to_string(),
                    host: "192.0.2.1".to_string(),
                    port: 7125,
                },
                job_id: job_id.map(str::to_string),
            },
        )
        .unwrap();
        let uncertain = Outcome::Uncertain {
            reason: InconclusiveReason::HostUnreachable,
            no_longer_pending: false,
        };
        let failure = HostOperationFailure::for_code(HostOperationFailureCode::HostRejected);
        let steps: Vec<Outcome> = match state {
            HostOperationState::Dispatching => vec![],
            HostOperationState::Uncertain => vec![uncertain],
            HostOperationState::Reconciling => vec![uncertain, Outcome::Reconciling],
            HostOperationState::Succeeded => vec![Outcome::Succeeded {
                resolution: HostOperationResolution::StartAccepted,
            }],
            HostOperationState::Failed => vec![Outcome::Failed { failure }],
            HostOperationState::Abandoned => {
                vec![uncertain, Outcome::Abandoned { note: None }]
            }
        };
        let mut row = row;
        for step in steps {
            row = host_ops_repository::transition_at(tx, &row.id, step, NOW).unwrap();
        }
        row
    }

    /// Puts the Job in `state` directly (the CHECKs' required columns
    /// filled in) with `active` as its active Host Operation.
    fn force_state(tx: &Transaction<'_>, job_id: &str, state: JobState, active: &str) {
        let terminal = state.is_terminal();
        let settlement = match state {
            JobState::Completed => "settled",
            _ if terminal => "pending",
            _ => "open",
        };
        tx.execute(
            "UPDATE jobs SET state = ?2, active_host_operation_id = ?3,
                 upload_host_operation_id = ?3, start_host_operation_id = ?3,
                 start_confirmation = 'bedClear', started_at = ?4, history_mark = 1,
                 host_path = 'farm3d/slr-a.gcode',
                 settlement = ?5,
                 settlement_method = CASE WHEN ?5 = 'settled' THEN 'estimated' END,
                 cancel_reason = CASE WHEN ?2 = 'cancelled' THEN 'hostCancelled' END,
                 ended_at = CASE WHEN ?6 THEN ?4 END
             WHERE id = ?1",
            rusqlite::params![
                job_id,
                encode_enum(state),
                active,
                NOW,
                settlement,
                terminal
            ],
        )
        .unwrap();
    }

    /// D7's table: the event a (linked op kind, op state, Job state)
    /// triple raises. `None`: no event.
    fn expected_event(
        kind: HostOperationKind,
        op: HostOperationState,
        job: JobState,
    ) -> Option<JobEventKind> {
        use HostOperationKind as K;
        use HostOperationState as O;
        use JobState as J;
        match (kind, op, job) {
            (K::Upload, O::Succeeded, J::Staging) => Some(JobEventKind::StageSucceeded),
            (K::Upload, O::Failed | O::Abandoned, J::Staging) => Some(JobEventKind::StageFailed),
            (K::Start, O::Succeeded, J::Starting) => Some(JobEventKind::StartSucceeded),
            (K::Start, O::Failed, J::Starting) => Some(JobEventKind::StartFailed),
            (K::Start, O::Abandoned, J::Starting) => Some(JobEventKind::StartAbandoned),
            (K::Pause, O::Succeeded, J::Printing) => Some(JobEventKind::Paused),
            (K::Resume, O::Succeeded, J::Paused) => Some(JobEventKind::Resumed),
            (
                K::Pause | K::Resume | K::Cancel,
                O::Failed | O::Abandoned,
                J::Printing | J::Paused,
            ) => Some(JobEventKind::ControlFailed),
            _ => None,
        }
    }

    fn event_count(tx: &Transaction<'_>, job_id: &str) -> i64 {
        tx.query_row(
            "SELECT COUNT(*) FROM job_events WHERE job_id = ?1",
            [job_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    fn last_event(tx: &Transaction<'_>, job_id: &str) -> String {
        tx.query_row(
            "SELECT kind FROM job_events WHERE job_id = ?1 ORDER BY sequence DESC LIMIT 1",
            [job_id],
            |row| row.get(0),
        )
        .unwrap()
    }

    /// Every (linked op kind, op state, Job state) triple, against D7's
    /// table: the event (or none), and `active_host_operation_id` cleared
    /// exactly when the op is terminal, in every Job state. Each case runs
    /// in a transaction that is rolled back.
    #[test]
    fn apply_host_outcome_matches_d7s_table_for_every_pair() {
        let rig = rig();
        for kind in OP_KINDS {
            for op_state in OP_STATES {
                for job_state in JobState::ALL {
                    let _ = rig.storage.write_repo(|tx| {
                        let op = insert_op(tx, Some(&rig.job_id), kind, op_state);
                        force_state(tx, &rig.job_id, job_state, &op.id);
                        let events_before = event_count(tx, &rig.job_id);
                        let applied = apply_host_outcome(tx, &op, NOW).unwrap();
                        let job = jobs_repository::load_job(tx, &rig.job_id).unwrap().unwrap();
                        let case = format!("{kind:?} {op_state:?} {job_state:?}");
                        let terminal = matches!(
                            op_state,
                            HostOperationState::Succeeded
                                | HostOperationState::Failed
                                | HostOperationState::Abandoned
                        );
                        match expected_event(kind, op_state, job_state) {
                            Some(event) => {
                                assert_eq!(
                                    event_count(tx, &rig.job_id),
                                    events_before + 1,
                                    "{case}"
                                );
                                assert_eq!(
                                    last_event(tx, &rig.job_id),
                                    encode_enum(event),
                                    "{case}"
                                );
                                let to = crate::jobs::state::transition(job_state, event).unwrap();
                                assert_eq!(job.state, to, "{case}");
                            }
                            None => {
                                assert_eq!(event_count(tx, &rig.job_id), events_before, "{case}");
                                assert_eq!(job.state, job_state, "{case}");
                            }
                        }
                        assert_eq!(applied.is_some(), terminal, "{case}");
                        assert_eq!(
                            job.active_host_operation_id.is_none(),
                            terminal,
                            "{case}: active op cleared exactly when terminal"
                        );
                        // Idempotent: a second call writes nothing.
                        let events = event_count(tx, &rig.job_id);
                        assert!(
                            apply_host_outcome(tx, &op, NOW).unwrap().is_none(),
                            "{case}"
                        );
                        assert_eq!(event_count(tx, &rig.job_id), events, "{case}");
                        Err::<(), _>(RepositoryError::Storage(StorageError::OperationFailed))
                    });
                }
            }
        }
    }

    #[test]
    fn an_op_that_is_not_the_jobs_active_one_is_ignored() {
        let rig = rig();
        let _ = rig.storage.write_repo(|tx| {
            let stale = insert_op(
                tx,
                Some(&rig.job_id),
                HostOperationKind::Upload,
                HostOperationState::Failed,
            );
            let current = insert_op(
                tx,
                Some(&rig.job_id),
                HostOperationKind::Upload,
                HostOperationState::Dispatching,
            );
            force_state(tx, &rig.job_id, JobState::Staging, &current.id);
            assert!(apply_host_outcome(tx, &stale, NOW).unwrap().is_none());
            let job = jobs_repository::load_job(tx, &rig.job_id).unwrap().unwrap();
            assert_eq!(job.state, JobState::Staging);
            assert_eq!(
                job.active_host_operation_id.as_deref(),
                Some(current.id.as_str())
            );
            Err::<(), _>(RepositoryError::Storage(StorageError::OperationFailed))
        });
    }

    #[test]
    fn an_unlinked_op_is_ignored() {
        let rig = rig();
        let _ = rig.storage.write_repo(|tx| {
            let raw = insert_op(
                tx,
                None,
                HostOperationKind::Upload,
                HostOperationState::Succeeded,
            );
            assert!(apply_host_outcome(tx, &raw, NOW).unwrap().is_none());
            Err::<(), _>(RepositoryError::Storage(StorageError::OperationFailed))
        });
    }

    #[test]
    fn stage_succeeded_records_the_upload_and_clears_the_last_failure() {
        let rig = rig();
        let _ = rig.storage.write_repo(|tx| {
            let op = insert_op(tx, Some(&rig.job_id), HostOperationKind::Upload, HostOperationState::Succeeded);
            tx.execute(
                "UPDATE jobs SET state = 'staging', active_host_operation_id = ?2,
                     last_failure_json = '{\"kind\":\"hostOperationAbandoned\",\"at\":\"x\",\"hostOperationId\":\"hop-old\"}'
                 WHERE id = ?1",
                [&rig.job_id, &op.id],
            )
            .unwrap();
            let job = apply_host_outcome(tx, &op, NOW).unwrap().unwrap().job;
            assert_eq!(job.state, JobState::AwaitingStart);
            assert_eq!(job.upload_host_operation_id.as_deref(), Some(op.id.as_str()));
            assert_eq!(job.host_path.as_deref(), Some("farm3d/slr-a.gcode"));
            assert_eq!(job.last_failure, None);
            assert_eq!(job.active_host_operation_id, None);
            Err::<(), _>(RepositoryError::Storage(StorageError::OperationFailed))
        });
    }

    /// D7: a re-stage whose upload fails returns the Job to `assigned`
    /// and clears `upload_host_operation_id` (the old file isn't trusted).
    #[test]
    fn a_failed_restage_clears_the_old_upload() {
        let rig = rig();
        let _ = rig.storage.write_repo(|tx| {
            let old = insert_op(tx, Some(&rig.job_id), HostOperationKind::Upload, HostOperationState::Succeeded);
            let op = insert_op(tx, Some(&rig.job_id), HostOperationKind::Upload, HostOperationState::Failed);
            tx.execute(
                "UPDATE jobs SET state = 'staging', active_host_operation_id = ?2,
                     upload_host_operation_id = ?3 WHERE id = ?1",
                [&rig.job_id, &op.id, &old.id],
            )
            .unwrap();
            let job = apply_host_outcome(tx, &op, NOW).unwrap().unwrap().job;
            assert_eq!(job.state, JobState::Assigned);
            assert_eq!(job.upload_host_operation_id, None);
            assert!(matches!(
                job.last_failure,
                Some(JobFailure::HostOperationFailed { ref host_operation_id, .. }) if *host_operation_id == op.id
            ));
            Err::<(), _>(RepositoryError::Storage(StorageError::OperationFailed))
        });
    }

    #[test]
    fn start_succeeded_sets_started_at_and_the_history_mark() {
        let rig = rig();
        let _ = rig.storage.write_repo(|tx| {
            let mut op = insert_op(
                tx,
                Some(&rig.job_id),
                HostOperationKind::Start,
                HostOperationState::Succeeded,
            );
            // Ruling R13(b): the time the start was sent, not the apply time.
            op.dispatched_at = Some("2026-09-01T11:59:58Z".to_string());
            force_state(tx, &rig.job_id, JobState::Starting, &op.id);
            tx.execute(
                "UPDATE jobs SET started_at = NULL, history_mark = NULL WHERE id = ?1",
                [&rig.job_id],
            )
            .unwrap();
            let job = apply_host_outcome(tx, &op, NOW).unwrap().unwrap().job;
            assert_eq!(job.state, JobState::Printing);
            assert_eq!(job.started_at.as_deref(), Some("2026-09-01T11:59:58Z"));
            let mark: i64 = tx
                .query_row(
                    "SELECT history_mark FROM jobs WHERE id = ?1",
                    [&rig.job_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(mark, 7);
            Err::<(), _>(RepositoryError::Storage(StorageError::OperationFailed))
        });
    }

    #[test]
    fn an_abandoned_start_opens_exactly_one_outcome_unknown_requirement() {
        let rig = rig();
        let _ = rig.storage.write_repo(|tx| {
            let op = insert_op(
                tx,
                Some(&rig.job_id),
                HostOperationKind::Start,
                HostOperationState::Abandoned,
            );
            force_state(tx, &rig.job_id, JobState::Starting, &op.id);
            let applied = apply_host_outcome(tx, &op, NOW).unwrap().unwrap();
            assert_eq!(applied.job.state, JobState::OutcomeUnknown);
            assert_eq!(applied.requirements.len(), 1);
            assert_eq!(
                applied.requirements[0].kind,
                RequirementKind::JobOutcomeUnknown
            );
            assert_eq!(applied.requirements[0].status, RequirementStatus::Pending);
            assert!(apply_host_outcome(tx, &op, NOW).unwrap().is_none());
            let count: i64 = tx
                .query_row(
                    "SELECT COUNT(*) FROM reconciliation_requirements WHERE job_id = ?1",
                    [&rig.job_id],
                    |row| row.get(0),
                )
                .unwrap();
            assert_eq!(count, 1);
            Err::<(), _>(RepositoryError::Storage(StorageError::OperationFailed))
        });
    }

    // --- start_blockers / may_start_unattended ------------------------------

    fn sim_capabilities() -> PrinterCapabilities {
        PrinterCapabilities {
            printer_id: PRINTER.to_string(),
            adapter_kind: Some("moonraker".to_string()),
            capabilities: CapabilityMap::complete(|_| CapabilityState::Supported {
                evidence: CapabilityEvidence {
                    source: "test".to_string(),
                    tier: EvidenceTier::Sim,
                    verified_host_versions: Vec::new(),
                },
            }),
            host_facts: None,
            observed_at: None,
        }
    }

    fn status(state: OperationalState, freshness: TelemetryFreshness) -> PrinterStatus {
        let mut status = PrinterStatus::new(ConnectionState::Online);
        status.operational_state = state;
        status.freshness = freshness;
        status
    }

    fn ready() -> PrinterStatus {
        status(OperationalState::Ready, TelemetryFreshness::Fresh)
    }

    fn awaiting_job() -> Job {
        let mut job = crate::jobs::state::tests_support::a_job(JobState::AwaitingStart);
        job.printer_id = PRINTER.to_string();
        job.spool_id = "spl-a".to_string();
        job
    }

    fn a_printer(safety: StartSafety) -> StoredPrinter {
        StoredPrinter {
            id: PRINTER.to_string(),
            name: "Alpha".to_string(),
            start_safety: safety,
            ..Default::default()
        }
    }

    fn codes(blockers: &[Blocker]) -> Vec<BlockerCode> {
        blockers.iter().map(|blocker| blocker.code).collect()
    }

    #[test]
    fn nothing_blocks_a_loaded_ready_printer() {
        let job = awaiting_job();
        let blockers = start_blockers(
            &job,
            &a_printer(StartSafety::ConfirmBedClear),
            Some(&ready()),
            &["spl-a".to_string()],
            &sim_capabilities(),
            false,
            Some(12),
        );
        assert_eq!(blockers, Vec::new());
    }

    #[test]
    fn start_blockers_list_every_failing_check_in_d7_order() {
        let job = awaiting_job();
        let mut caps = sim_capabilities();
        caps.capabilities = CapabilityMap::complete(|key| match key {
            CapabilityKey::ArtifactIdentity => CapabilityState::Unsupported {
                reason: UnsupportedReason::NotVerified,
                detail: "No identity check here.".to_string(),
            },
            _ => CapabilityState::Supported {
                evidence: CapabilityEvidence {
                    source: "test".to_string(),
                    tier: EvidenceTier::Sim,
                    verified_host_versions: Vec::new(),
                },
            },
        });
        let blockers = start_blockers(
            &job,
            &a_printer(StartSafety::ConfirmBedClear),
            Some(&status(
                OperationalState::Printing,
                TelemetryFreshness::Fresh,
            )),
            &[],
            &caps,
            true,
            Some(12),
        );
        assert_eq!(
            codes(&blockers),
            [
                BlockerCode::SpoolNotLoaded,
                BlockerCode::HostOperationPending,
                BlockerCode::CapabilityUnsupported,
                BlockerCode::PrinterNotReady,
            ]
        );
        assert_eq!(
            blockers[0].message,
            "Awaiting material: load Spool #12 on Alpha."
        );
        assert_eq!(blockers[0].recovery, Some(RecoveryCode::LoadSpool));
        assert_eq!(blockers[2].message, "No identity check here.");
        assert_eq!(
            blockers[3].message,
            "The printer can't start now: it is printing."
        );
        assert!(blockers
            .iter()
            .all(|blocker| blocker.printer_ids == [PRINTER]));
    }

    /// P6's start rule: finished and cancelled offer a start too; stale
    /// telemetry and no status never do.
    #[test]
    fn printer_not_ready_follows_p6s_start_rule() {
        let job = awaiting_job();
        let loaded = ["spl-a".to_string()];
        for (status, blocked) in [
            (
                Some(status(
                    OperationalState::Finished,
                    TelemetryFreshness::Fresh,
                )),
                false,
            ),
            (
                Some(status(
                    OperationalState::Cancelled,
                    TelemetryFreshness::Fresh,
                )),
                false,
            ),
            (
                Some(status(OperationalState::Ready, TelemetryFreshness::Stale)),
                true,
            ),
            (
                Some(status(OperationalState::Failed, TelemetryFreshness::Fresh)),
                true,
            ),
            (None, true),
        ] {
            let blockers = start_blockers(
                &job,
                &a_printer(StartSafety::ConfirmBedClear),
                status.as_ref(),
                &loaded,
                &sim_capabilities(),
                false,
                None,
            );
            assert_eq!(!blockers.is_empty(), blocked, "{status:?}");
        }
    }

    #[test]
    fn only_an_awaiting_start_job_has_start_blockers() {
        for state in JobState::ALL {
            if state == JobState::AwaitingStart {
                continue;
            }
            let mut job = awaiting_job();
            job.state = state;
            let blockers = start_blockers(
                &job,
                &a_printer(StartSafety::ConfirmBedClear),
                None,
                &[],
                &sim_capabilities(),
                true,
                None,
            );
            assert!(blockers.is_empty(), "{state:?}");
        }
    }

    #[test]
    fn unattended_start_needs_the_rule_ready_fresh_no_blockers_and_sim_evidence() {
        let job = awaiting_job();
        let loaded = ["spl-a".to_string()];
        let unattended = a_printer(StartSafety::Unattended);
        let caps = sim_capabilities();
        assert!(may_start_unattended(
            &job,
            &unattended,
            &ready(),
            &caps,
            &loaded,
            false
        ));

        // ConfirmBedClear never starts unattended.
        assert!(!may_start_unattended(
            &job,
            &a_printer(StartSafety::ConfirmBedClear),
            &ready(),
            &caps,
            &loaded,
            false
        ));
        // Only from Ready: never Finished or Cancelled, never stale.
        for status in [
            status(OperationalState::Finished, TelemetryFreshness::Fresh),
            status(OperationalState::Cancelled, TelemetryFreshness::Fresh),
            status(OperationalState::Ready, TelemetryFreshness::Stale),
        ] {
            assert!(!may_start_unattended(
                &job,
                &unattended,
                &status,
                &caps,
                &loaded,
                false
            ));
        }
        // The Spool must be loaded, and nothing unresolved.
        assert!(!may_start_unattended(
            &job,
            &unattended,
            &ready(),
            &caps,
            &[],
            false
        ));
        assert!(!may_start_unattended(
            &job,
            &unattended,
            &ready(),
            &caps,
            &loaded,
            true
        ));
        // Each of the four capabilities needs sim evidence.
        for weak in [
            CapabilityKey::Upload,
            CapabilityKey::Start,
            CapabilityKey::HostState,
            CapabilityKey::ArtifactIdentity,
        ] {
            let mut caps = sim_capabilities();
            caps.capabilities = CapabilityMap::complete(|key| CapabilityState::Supported {
                evidence: CapabilityEvidence {
                    source: "test".to_string(),
                    tier: if key == weak {
                        EvidenceTier::ReadOnlyHardware
                    } else {
                        EvidenceTier::Sim
                    },
                    verified_host_versions: Vec::new(),
                },
            });
            assert!(
                !may_start_unattended(&job, &unattended, &ready(), &caps, &loaded, false),
                "{weak:?}"
            );
        }
        // Only an awaitingStart Job.
        let mut assigned = awaiting_job();
        assigned.state = JobState::Assigned;
        assert!(!may_start_unattended(
            &assigned,
            &unattended,
            &ready(),
            &caps,
            &loaded,
            false
        ));
    }
}
