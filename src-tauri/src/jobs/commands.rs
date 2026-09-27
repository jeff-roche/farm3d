//! P7's Job commands (spec "Commands"): `assign_queue_entry`,
//! `release_job`, `retry_job`, `cancel_job`, and `get_job_history`, which
//! need no host, and the handoffs `stage_job`, `start_job`, `pause_job`,
//! `resume_job`, and `cancel_job` after start (Task 8a), and
//! `declare_job_outcome` (Task 8b, with the tracker). Settlement comes
//! with its own task.
//!
//! The host-free writes take the Printer lock first (D4) and run one
//! `Storage::write_repo` transaction from `jobs::assign`. The handoffs go
//! through `jobs::dispatch`, which calls `host_ops::api`: that takes the
//! non-reentrant Printer lock itself, so these commands never hold it
//! around a handoff. After commit, a command publishes the changed rows on
//! the `queue` stream (Jobs with live `startBlockers`) and the touched
//! Spools on the inventory stream; a replay publishes nothing.

use std::sync::Arc;

use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;
use crate::queue::commands::publish;
use crate::queue::eligibility::AssignMode;
use crate::queue::world::LiveWorld;
use crate::host_ops::start_rule::ControlVerb;
use crate::host_ops::PriorState;
use crate::queue::QueueChange;
use crate::RuntimeServices;

use super::assign::{self, AssignRequest};
use super::dispatch::{self, Handoff};
use super::repository as jobs_repository;
use super::tracker;
use super::{AssignedBy, Job, JobHistory, JobState, StartConfirmation};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

fn ready<R: tauri::Runtime>(
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<RuntimeServices<R>>, CommandError> {
    contract_version.validate()?;
    bootstrap.ready()
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// The committed Job, read outside any write so the command can take its
/// Printer's lock before the transaction (`NOT_FOUND` when it's missing).
fn committed_job<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    job_id: &str,
) -> Result<Job, CommandError> {
    services
        .storage
        .read(|connection| Ok(jobs_repository::load_job(connection, job_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?
        .ok_or_else(|| CommandError::not_found(job_id))
}

/// D4 "Assign": under the Printer lock, one transaction claims the id,
/// re-checks D5 over rows read inside it, reserves the Spool, and inserts
/// the Job.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn assign_queue_entry<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    entry_id: String,
    printer_id: String,
    spool_id: String,
    acknowledge_manual_facts: Option<bool>,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let printer_lock = services.host_ops.printer_lock(&printer_id);
    let _serialized = printer_lock.lock().await;

    let request = AssignRequest {
        operation_id,
        entry_id,
        printer_id,
        spool_id,
        mode: AssignMode::Operator {
            acknowledge_manual_facts: acknowledge_manual_facts.unwrap_or(false),
        },
        assigned_by: AssignedBy::Operator,
    };
    let reader = LiveWorld::of(&services);
    let now = now_rfc3339();
    let assigned = services
        .storage
        .write_repo(|tx| assign::assign(tx, &reader, &request, &now))
        .map_err(CommandError::from_repository)?;
    let change = assigned.change();
    if !assigned.replayed {
        publish(&app, &services, &change);
        // D7: the driver stages every new Job at once, whatever the policy.
        services.jobs.request_stage(&assigned.job.id);
    }
    Ok(CommandSuccess::new(change))
}

/// D4 "Release": the Job ends `cancelled{releasedBeforeStart}`, its
/// reservation is released, and a replacement entry takes its entry's
/// place in the Queue.
#[tauri::command]
pub async fn release_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let job = committed_job(&services, &job_id)?;
    let printer_lock = services.host_ops.printer_lock(&job.printer_id);
    let _serialized = printer_lock.lock().await;

    let now = now_rfc3339();
    let released = services
        .storage
        .write_repo(|tx| assign::release(tx, &operation_id, &job_id, &now))
        .map_err(CommandError::from_repository)?;
    let change = released.change();
    if !released.replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

/// D2/D4 "Retry": a linked entry at the end of the Queue. The Job and its
/// history are untouched, so no lock is needed.
#[tauri::command]
pub async fn retry_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let now = now_rfc3339();
    let retried = services
        .storage
        .write_repo(|tx| assign::retry(tx, &operation_id, &job_id, &now))
        .map_err(CommandError::from_repository)?;
    let change = retried.change();
    if !retried.replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

/// D4 "Cancel": before start (`assigned`, `awaitingStart`) the Job ends
/// `cancelled{cancelledBeforeStart}`, its reservation is released, and its
/// entry closes. After start (`printing`, `paused`) it is a control
/// handoff (D7): the Job keeps its state with the cancel op active, and
/// the tracker proves the end from history. Any other state is
/// `JOB_ACTION_NOT_ALLOWED`. A replay returns the current rows either way.
#[tauri::command]
pub async fn cancel_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let job = committed_job(&services, &job_id)?;
    if matches!(job.state, JobState::Printing | JobState::Paused) {
        let handoff =
            dispatch::control_job(&services, operation_id, &job_id, ControlVerb::Cancel).await?;
        return Ok(finish(&app, &services, handoff));
    }
    let printer_lock = services.host_ops.printer_lock(&job.printer_id);
    let _serialized = printer_lock.lock().await;

    let now = now_rfc3339();
    let cancelled = services
        .storage
        .write_repo(|tx| assign::cancel_before_start(tx, &operation_id, &job_id, &now))
        .map_err(CommandError::from_repository)?;
    let change = cancelled.change();
    if !cancelled.replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

/// The Job, its entry and lineage, its timeline, reservation, and
/// requirements. `NOT_FOUND` for an unknown Job (checked first:
/// `jobs::repository::history` assumes the Job exists).
#[tauri::command]
pub async fn get_job_history<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    job_id: String,
) -> Result<CommandSuccess<JobHistory>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let history = services
        .storage
        .read_transaction(|connection| {
            Ok(match jobs_repository::load_job(connection, &job_id) {
                Ok(None) => Ok(None),
                Ok(Some(_)) => jobs_repository::history(connection, &job_id).map(Some),
                Err(error) => Err(error),
            })
        })
        .map_err(storage_error)?
        .map_err(storage_error)?
        .ok_or_else(|| CommandError::not_found(&job_id))?;
    let mut history = history;
    dispatch::present_jobs(&services, std::slice::from_mut(&mut history.job));
    Ok(CommandSuccess::new(history))
}

/// A handoff's result as the command returns it: the Job with live
/// `startBlockers`, published unless it was a replay.
fn finish<R: tauri::Runtime>(
    app: &AppHandle<R>,
    services: &RuntimeServices<R>,
    handoff: Handoff,
) -> CommandSuccess<QueueChange> {
    let mut change = handoff.change();
    dispatch::present_change(services, &mut change);
    if !handoff.replayed {
        publish(app, services, &change);
    }
    CommandSuccess::new(change)
}

/// D3/D4/D7 "Stage": hands the Job's Slice Revision to P6's upload, from
/// `assigned` or (staging again) `awaitingStart`. The Host Operation's id
/// is `<operationId>#hostOperation`.
#[tauri::command]
pub async fn stage_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let handoff = dispatch::stage_job(&services, operation_id, &job_id).await?;
    Ok(finish(&app, &services, handoff))
}

/// D7 "Start": `acknowledgement` must be `"bedClear"` (`VALIDATION`), the
/// Job `awaitingStart` (`JOB_ACTION_NOT_ALLOWED`), and nothing may block
/// the start (`JOB_START_BLOCKED`); then P6's start order runs with
/// `priorState`, and its errors pass through unchanged.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn start_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
    prior_state: PriorState,
    acknowledgement: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    if acknowledgement != "bedClear" {
        return Err(CommandError::validation_at(
            "acknowledgement",
            "Confirm that the bed is clear.",
        ));
    }
    let handoff = dispatch::start_job(
        &services,
        operation_id,
        &job_id,
        prior_state,
        StartConfirmation::BedClear,
    )
    .await?;
    Ok(finish(&app, &services, handoff))
}

/// D7 "Pause": P6's pause, through the Job (`printing` only).
#[tauri::command]
pub async fn pause_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let handoff = dispatch::control_job(&services, operation_id, &job_id, ControlVerb::Pause).await?;
    Ok(finish(&app, &services, handoff))
}

/// D7 "Resume": P6's resume, through the Job (`paused` only).
#[tauri::command]
pub async fn resume_job<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let handoff =
        dispatch::control_job(&services, operation_id, &job_id, ControlVerb::Resume).await?;
    Ok(finish(&app, &services, handoff))
}

/// D9 "declare_job_outcome": the operator declares how an `outcomeUnknown`
/// Job ended, or a `printing`/`paused` one whose host has been unreachable
/// for `JobTimings.unreachable_declare_after`. farm3d sends nothing to the
/// host.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn declare_job_outcome<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    job_id: String,
    outcome: super::DeclaredOutcome,
    acknowledgement: String,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    if acknowledgement != tracker::DECLARE_ACKNOWLEDGEMENT {
        return Err(CommandError::validation_at(
            "acknowledgement",
            "Confirm that farm3d can't know how this print ended.",
        ));
    }
    let job = committed_job(&services, &job_id)?;
    let printer_lock = services.host_ops.printer_lock(&job.printer_id);
    let _serialized = printer_lock.lock().await;

    let at = services.jobs.now();
    let declare_after = services.jobs.timings.unreachable_declare_after;
    let declared = services
        .storage
        .write_repo(|tx| tracker::declare(tx, &operation_id, &job_id, outcome, at, declare_after))
        .map_err(CommandError::from_repository)?;
    let mut change = declared.change;
    dispatch::present_change(&services, &mut change);
    if !declared.replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}
