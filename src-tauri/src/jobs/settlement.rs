//! P7 D4/"Material settlement": exactly-once settlement of a Job's Spool
//! reservation. [`on_completed`] and [`on_failed_or_cancelled`] are the
//! terminal-transaction hooks `jobs::tracker::end_job` calls right after a
//! Job's terminal transition (owner decisions 5/6: a completed Job
//! deducts its full estimate automatically, in the same transaction; a
//! failed or cancelled one needs an operator settlement). [`settle`] and
//! [`correct`] back the `settle_job_material`/`correct_job_material`
//! commands: each claims its own operation id first, so a replay returns
//! the current rows and writes nothing, and a settled/corrected Job
//! refuses a second attempt (`JOB_ALREADY_SETTLED`) before ever touching
//! the pure state table (spec "Material settlement": checked against the
//! settlement, not the state, since a `notRequired` Cancelled Job and a
//! `pending` one share the same `JobState`).

use rusqlite::Transaction;
use serde::Serialize;

use crate::persistence::{RepositoryError, StorageError};
use crate::queue::QueueChange;
use crate::spools::ledger::{self, AmountEntry, AmountEventKind};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::reservations::{self, ReservationError};
use crate::spools::AmountConfidence;

use super::assign::{reservation_error, spool_number};
use super::repository::{self as jobs_repository, JobChange};
use super::{
    estimated_use_mg, Job, JobAction, JobEventKind, JobState, ReconciliationRequirement,
    RequirementKind, RequirementResolution, RequirementStatus, SettleChoice, SettleFailureReason,
    Settlement, SettlementMethod,
};

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
}

fn load_job(tx: &Transaction<'_>, job_id: &str) -> Result<Job, RepositoryError> {
    jobs_repository::load_job(tx, job_id)?.ok_or_else(|| not_found(job_id))
}

/// The open `materialReconciliation` requirement `settle`/`correct` act on
/// (D1: at most one, opened by [`on_failed_or_cancelled`], per `(job_id,
/// kind)`'s UNIQUE index).
fn open_material_requirement(
    tx: &Transaction<'_>,
    job_id: &str,
) -> Result<ReconciliationRequirement, RepositoryError> {
    jobs_repository::requirements_for_job(tx, job_id)?
        .into_iter()
        .find(|requirement| {
            requirement.kind == RequirementKind::MaterialReconciliation
                && requirement.status != RequirementStatus::Resolved
        })
        .ok_or(RepositoryError::Storage(StorageError::OperationFailed))
}

/// D4 "Tracker terminal"/"Declare", automatic-on-completion row: `consume`s
/// the full estimate (never progress-scaled -- owner decision 6). `job`'s
/// settlement is already `settled`/`estimated` (the terminal transition
/// set it via `state::settlement_after`); this only moves the reservation.
pub fn on_completed(tx: &Transaction<'_>, job: &Job, now: &str) -> Result<(), RepositoryError> {
    let _ = now;
    reservations::consume(tx, &job.reservation_id, job.estimate_mg, None).map_err(|error| {
        reservation_error(
            spool_number(tx, &job.spool_id),
            &job.spool_id,
            Some(&job.reservation_id),
            Some(job.estimate_mg),
            error,
        )
    })?;
    Ok(())
}

/// D4 "Tracker terminal"/"Declare", failed-or-cancelled row: marks the
/// reservation `unresolved` and opens one `materialReconciliation`
/// requirement, `pending`. `job`'s settlement is already `pending` (set by
/// the terminal transition).
pub fn on_failed_or_cancelled(
    tx: &Transaction<'_>,
    job: &Job,
    now: &str,
) -> Result<(), RepositoryError> {
    reservations::mark_unresolved(tx, &job.reservation_id).map_err(|error| {
        reservation_error(
            spool_number(tx, &job.spool_id),
            &job.spool_id,
            Some(&job.reservation_id),
            None,
            error,
        )
    })?;
    jobs_repository::open_requirement(
        tx,
        &job.id,
        RequirementKind::MaterialReconciliation,
        Some(&job.spool_id),
        Some(&job.reservation_id),
        now,
    )?;
    Ok(())
}

/// What a settle or correct committed (or, on a replay, what it had
/// committed).
#[derive(Clone, Debug)]
pub struct Settled {
    pub job: Job,
    pub requirement: Option<ReconciliationRequirement>,
    pub spool_ids: Vec<String>,
    pub replayed: bool,
}

impl Settled {
    /// Spec "Material settlement": every settlement path publishes the
    /// Job, the requirement (if any), and the Spool -- once, and never for
    /// a replay (the caller checks `replayed` before publishing this).
    pub fn change(&self) -> QueueChange {
        QueueChange {
            entries: Vec::new(),
            jobs: vec![self.job.clone()],
            requirements: self.requirement.clone().into_iter().collect(),
            spool_ids: self.spool_ids.clone(),
        }
    }
}

/// D4's ledger digest for `settle_job_material` (fields in this order).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettleDigest<'a> {
    job_id: &'a str,
    choice: &'a SettleChoice,
}

/// D4's ledger digest for `correct_job_material` (fields in this order).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct CorrectDigest<'a> {
    job_id: &'a str,
    entry: &'a AmountEntry,
}

/// Fix round 1 (ruling R14(c)): `ledger::resolve_entry`/`consume_measured`
/// phrase their `field_path` for a bare `entry` request field
/// (`entry.netMg`, ...). `settle_job_material`'s entry lives at
/// `choice.entry` instead (the request's top-level field is `choice`, an
/// internally-tagged `SettleChoice`), so remap before a `VALIDATION`
/// reaches `CommandError` -- `correct_job_material`'s `entry` needs no
/// remapping, since it *is* the request's top-level field.
fn choice_field_path(field_path: &'static str) -> &'static str {
    match field_path {
        "entry.netMg" => "choice.entry.netMg",
        "entry.tareId" => "choice.entry.tareId",
        "entry.tareMg" => "choice.entry.tareMg",
        "entry.grossMg" => "choice.entry.grossMg",
        "entry.confidence" => "choice.entry.confidence",
        other => other,
    }
}

/// Fix round 1 (ruling R14(b)): a settle-by-measurement or a correction
/// must carry a genuinely measured entry -- a `Net` entry silently
/// carrying `confidence: "estimated"` would promote a guess into a
/// Measurement ledger row, indistinguishable later from a real scale
/// reading. `Scale` entries are always `Measured` (`ledger::resolve_entry`
/// sets it, never taking the caller's word), so only `Net`'s explicit
/// confidence needs checking here.
fn require_measured_entry(
    entry: &AmountEntry,
    field_path: &'static str,
) -> Result<(), RepositoryError> {
    if let AmountEntry::Net { confidence, .. } = entry {
        if *confidence != AmountConfidence::Measured {
            return Err(RepositoryError::Validation { field_path });
        }
    }
    Ok(())
}

/// Checks `choice` against D3/D4's settlement rule -- before the pure
/// state table, per the spec: a `settled` Job (any state) is
/// `JOB_ALREADY_SETTLED`; `estimated`/`measured` need `pending` or
/// `deferred`; `defer` needs `pending` (a second defer from `deferred` is
/// `JOB_ACTION_NOT_ALLOWED`, the same as `open`/`notRequired`).
fn check_settleable(job: &Job, choice: &SettleChoice) -> Result<(), RepositoryError> {
    let not_allowed = || {
        Err(RepositoryError::JobActionNotAllowed {
            job_id: job.id.clone(),
            action: JobAction::SettleMaterial,
            state: job.state,
        })
    };
    match job.settlement {
        Settlement::Settled => Err(RepositoryError::JobAlreadySettled {
            job_id: job.id.clone(),
            reason: SettleFailureReason::Settled,
        }),
        Settlement::Pending => Ok(()),
        Settlement::Deferred => {
            if matches!(choice, SettleChoice::Defer) {
                not_allowed()
            } else {
                Ok(())
            }
        }
        Settlement::Open | Settlement::NotRequired => not_allowed(),
    }
}

/// D4 "Settle / defer" (spec "Material settlement"): claims
/// `settleJobMaterial`, checks the settlement rule (above), then applies
/// `choice`: `estimated` and `measured` consume the reservation, set
/// `settlementMethod`, and resolve the requirement; `defer` touches no
/// reservation and moves the requirement to `deferred`. Every branch
/// writes one `MaterialSettled`/`MaterialDeferred` event.
pub fn settle(
    tx: &Transaction<'_>,
    operation_id: &str,
    job_id: &str,
    choice: &SettleChoice,
    now: &str,
) -> Result<Settled, RepositoryError> {
    let digest = operations::digest(&SettleDigest { job_id, choice });
    if operations::claim(tx, operation_id, OperationKind::SettleJobMaterial, &digest)?
        == Claim::Replay
    {
        let job = load_job(tx, job_id)?;
        let requirement = jobs_repository::requirements_for_job(tx, job_id)?
            .into_iter()
            .find(|requirement| requirement.kind == RequirementKind::MaterialReconciliation);
        return Ok(Settled {
            job,
            requirement,
            spool_ids: Vec::new(),
            replayed: true,
        });
    }

    let job = load_job(tx, job_id)?;
    check_settleable(&job, choice)?;
    let requirement = open_material_requirement(tx, job_id)?;
    let spool_id = job.spool_id.clone();

    let (event, change, resolution) = match choice {
        SettleChoice::Estimated => {
            let used_mg = estimated_use_mg(job.estimate_mg, job.max_progress_pct);
            reservations::consume(tx, &job.reservation_id, used_mg, None).map_err(|error| {
                reservation_error(
                    spool_number(tx, &job.spool_id),
                    &job.spool_id,
                    Some(&job.reservation_id),
                    Some(used_mg),
                    error,
                )
            })?;
            (
                JobEventKind::MaterialSettled,
                JobChange {
                    settlement_method: Some(SettlementMethod::Estimated),
                    operation_id: Some(operation_id.to_string()),
                    detail: Some(serde_json::json!({"method": "estimated", "usedMg": used_mg})),
                    ..JobChange::default()
                },
                Some(RequirementResolution::Settled {
                    method: SettlementMethod::Estimated,
                    used_mg,
                }),
            )
        }
        SettleChoice::Measured { entry } => {
            require_measured_entry(entry, "choice.entry.confidence")?;
            let measurement = reservations::consume_measured(tx, &job.reservation_id, entry, None)
                .map_err(|error| match error {
                    ReservationError::Validation { field_path } => RepositoryError::Validation {
                        field_path: choice_field_path(field_path),
                    },
                    other => reservation_error(
                        spool_number(tx, &job.spool_id),
                        &job.spool_id,
                        Some(&job.reservation_id),
                        None,
                        other,
                    ),
                })?;
            let used_mg = measurement
                .before_mg
                .map(|before| (before - measurement.after_mg).max(0))
                .unwrap_or(0);
            (
                JobEventKind::MaterialSettled,
                JobChange {
                    settlement_method: Some(SettlementMethod::Measured),
                    operation_id: Some(operation_id.to_string()),
                    detail: Some(serde_json::json!({"method": "measured", "usedMg": used_mg})),
                    ..JobChange::default()
                },
                Some(RequirementResolution::Settled {
                    method: SettlementMethod::Measured,
                    used_mg,
                }),
            )
        }
        SettleChoice::Defer => (
            JobEventKind::MaterialDeferred,
            JobChange {
                operation_id: Some(operation_id.to_string()),
                ..JobChange::default()
            },
            None,
        ),
    };

    let job = jobs_repository::transition(tx, job_id, event, change, now)?;
    let requirement = if let Some(resolution) = resolution {
        let resolution_json =
            serde_json::to_value(&resolution).expect("a resolution always serializes");
        jobs_repository::set_requirement_status(
            tx,
            &requirement.id,
            RequirementStatus::Resolved,
            Some(&resolution_json),
            now,
        )?
    } else {
        jobs_repository::set_requirement_status(
            tx,
            &requirement.id,
            RequirementStatus::Deferred,
            None,
            now,
        )?
    };

    Ok(Settled {
        job,
        requirement: Some(requirement),
        spool_ids: vec![spool_id],
        replayed: false,
    })
}

/// D4 "Correct" (spec "Material settlement"): claims `correctJobMaterial`;
/// allowed only on a `completed` Job with no correction yet
/// (`JOB_ALREADY_SETTLED{reason: corrected}` on a second one,
/// `JOB_ACTION_NOT_ALLOWED` on anything but `completed`). The reservation
/// is already `consumed` and stays that way -- this appends one
/// `Measurement` ledger row referencing it, noted as a correction, and
/// records `correction_event_id` on the Job.
pub fn correct(
    tx: &Transaction<'_>,
    operation_id: &str,
    job_id: &str,
    entry: &AmountEntry,
    now: &str,
) -> Result<Settled, RepositoryError> {
    let digest = operations::digest(&CorrectDigest { job_id, entry });
    if operations::claim(tx, operation_id, OperationKind::CorrectJobMaterial, &digest)?
        == Claim::Replay
    {
        let job = load_job(tx, job_id)?;
        return Ok(Settled {
            job,
            requirement: None,
            spool_ids: Vec::new(),
            replayed: true,
        });
    }

    let job = load_job(tx, job_id)?;
    if job.corrected {
        return Err(RepositoryError::JobAlreadySettled {
            job_id: job.id,
            reason: SettleFailureReason::Corrected,
        });
    }
    if job.state != JobState::Completed {
        return Err(RepositoryError::JobActionNotAllowed {
            job_id: job.id,
            action: JobAction::CorrectMaterial,
            state: job.state,
        });
    }

    require_measured_entry(entry, "entry.confidence")?;
    let (after_mg, _confidence, mut snapshot) = ledger::resolve_entry(tx, entry)?;
    snapshot.reservation_id = Some(job.reservation_id.clone());
    snapshot.note = Some(format!("Correction for Job {}", job.id));
    let event = ledger::append(
        tx,
        &job.spool_id,
        AmountEventKind::Measurement,
        after_mg,
        AmountConfidence::Measured,
        snapshot,
    )?;

    let spool_id = job.spool_id.clone();
    let job = jobs_repository::transition(
        tx,
        job_id,
        JobEventKind::MaterialCorrected,
        JobChange {
            correction_event_id: Some(event.id.clone()),
            operation_id: Some(operation_id.to_string()),
            detail: Some(serde_json::json!({"correctionEventId": event.id})),
            ..JobChange::default()
        },
        now,
    )?;

    Ok(Settled {
        job,
        requirement: None,
        spool_ids: vec![spool_id],
        replayed: false,
    })
}
