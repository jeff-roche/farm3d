//! P7's Job commands (spec "Commands"). This task lands the ones that
//! need no host: `assign_queue_entry`, `release_job`, `retry_job`,
//! `cancel_job` (its before-start branch), and `get_job_history`. The
//! host handoffs (`stage_job`, `start_job`, `pause_job`, `resume_job`,
//! `cancel_job` after start) and `declare_job_outcome` come with the
//! dispatch driver; settlement with its own task.
//!
//! Every write takes the Printer lock first (D4) and runs one
//! `Storage::write_repo` transaction from `jobs::assign`. Nothing here
//! calls into `host_ops`, so holding its non-reentrant lock is safe.
//! After commit, the command publishes the changed rows on the `queue`
//! stream and the touched Spools on the inventory stream; a replay
//! publishes nothing.

use std::sync::Arc;

use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;
use crate::queue::commands::publish;
use crate::queue::eligibility::AssignMode;
use crate::queue::world::LiveWorld;
use crate::queue::QueueChange;
use crate::RuntimeServices;

use super::assign::{self, AssignRequest};
use super::repository as jobs_repository;
use super::{AssignedBy, Job, JobHistory};

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

/// D4 "Cancel before start": the Job ends
/// `cancelled{cancelledBeforeStart}`, its reservation is released, and its
/// entry closes. Cancelling a started Job is a host handoff the dispatch
/// driver's task adds; until then it is `JOB_ACTION_NOT_ALLOWED`.
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
    Ok(CommandSuccess::new(history))
}
