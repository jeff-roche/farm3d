//! P9 D11: `list_job_history` and `get_job_timeline`. Both are read-only
//! and run in one read transaction.

use std::sync::Arc;

use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::{RepositoryError, StorageError};
use crate::RuntimeServices;

use super::repository as history_repository;
use super::{JobHistoryPage, JobHistoryQuery, JobTimeline};

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

/// `list_job_history`: settled Jobs, newest first, filtered and paged by
/// the opaque `after` cursor.
#[tauri::command]
pub async fn list_job_history<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    query: JobHistoryQuery,
) -> Result<CommandSuccess<JobHistoryPage>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let valid = history_repository::validate(&query)
        .map_err(|error| CommandError::validation_at(error.field, error.message))?;
    let page = services
        .storage
        .read_transaction(|tx| Ok(history_repository::list(tx, &valid)))
        .map_err(storage_error)?
        .map_err(storage_error)?;
    Ok(CommandSuccess::new(page))
}

/// `get_job_timeline`: every record linked to the Job, in order.
#[tauri::command]
pub async fn get_job_timeline<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    job_id: String,
) -> Result<CommandSuccess<JobTimeline>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    services
        .storage
        .read_transaction(|tx| Ok(history_repository::timeline(tx, &job_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?
        .map(CommandSuccess::new)
        .ok_or_else(|| CommandError::not_found(&job_id))
}
