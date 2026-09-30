//! P7 D4 "Recovery order": `jobs::recover_after_restart`, run right after
//! `host_ops::recover_after_restart` (which has already settled every row
//! a restart could leave `dispatching` or `reconciling`) and before any
//! command is served.

use chrono::{DateTime, Utc};

use crate::host_ops::{format_time, repository as host_ops_repository};
use crate::persistence::{RepositoryError, Storage, StorageError};

use super::dispatch::apply_host_outcome;
use super::repository as jobs_repository;
use super::{Job, JobState};

/// D4 "Recovery", steps 1–3:
///
/// 1. every `staging` or `starting` Job, and every `printing` or `paused`
///    Job with an `active_host_operation_id`, has that Host Operation
///    loaded and run through [`apply_host_outcome`] (idempotent), each Job
///    in its own transaction;
/// 2. the Jobs that changed are returned, for the runtime to publish once
///    it starts;
/// 3. an `assigned` Job that never staged is left for the driver's first
///    pass (R2), and `printing`/`paused` Jobs for the tracker (R9–R11).
pub fn recover_after_restart(
    storage: &Storage,
    now: DateTime<Utc>,
) -> Result<Vec<Job>, StorageError> {
    let now = format_time(now);
    let active = storage.read(|connection| Ok(jobs_repository::list_active(connection)))??;
    let mut changed = Vec::new();
    for job in active {
        let catches_up = matches!(
            job.state,
            JobState::Staging | JobState::Starting | JobState::Printing | JobState::Paused
        );
        let Some(op_id) = job.active_host_operation_id.filter(|_| catches_up) else {
            continue;
        };
        let applied = storage
            .write_repo(|tx| match host_ops_repository::load(tx, &op_id)? {
                Some(op) => apply_host_outcome(tx, &op, &now),
                None => Ok(None),
            })
            .map_err(|error| match error {
                RepositoryError::Storage(error) => error,
                _ => StorageError::Database,
            })?;
        if let Some(applied) = applied {
            changed.push(applied.job);
        }
    }
    Ok(changed)
}
