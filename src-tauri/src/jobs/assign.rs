//! P7 D4: the Job transactions that need no host — assign, release,
//! retry, and cancel before start. Each runs inside the caller's
//! `Storage::write_repo` transaction, claims its operation id there first
//! (a replay returns the current rows and writes nothing), and does all of
//! its work in that one transaction, so a refusal rolls everything back
//! and never burns the id.
//!
//! The Printer lock (D4) is the caller's: the commands take
//! `host_ops.printer_lock(printer_id)` before calling in here, and nothing
//! here calls into `host_ops`.

use rusqlite::{params, OptionalExtension, Transaction};
use serde::Serialize;

use crate::persistence::{RepositoryError, StorageError};
use crate::queue::eligibility::{self, AssignMode, AssignmentCheckError};
use crate::queue::repository as queue_repository;
use crate::queue::state::EntryEvent;
use crate::queue::world::{facts_for, World, WorldReader};
use crate::queue::{
    BlockerCode, CloseReason, DispatchPolicy, OriginKind, QueueChange, QueueEntry,
    QueueEntryAction, QueueEntryState,
};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::reservations::{self, ReservationError, ReservationHolder};

use super::repository::{self as jobs_repository, JobChange, NewJob};
use super::{AssignedBy, CancelReason, Job, JobAction, JobEventKind, PrinterSnapshot};

/// `assign_queue_entry`'s request, and the evaluator's (D6).
#[derive(Clone, Debug)]
pub struct AssignRequest {
    pub operation_id: String,
    pub entry_id: String,
    pub printer_id: String,
    pub spool_id: String,
    pub mode: AssignMode,
    pub assigned_by: AssignedBy,
}

impl AssignRequest {
    fn acknowledge_manual_facts(&self) -> bool {
        matches!(
            self.mode,
            AssignMode::Operator {
                acknowledge_manual_facts: true
            }
        )
    }
}

/// D4's ledger digest for `assign_queue_entry` (fields in this order).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AssignDigest<'a> {
    entry_id: &'a str,
    printer_id: &'a str,
    spool_id: &'a str,
    acknowledge_manual_facts: bool,
    assigned_by: AssignedBy,
}

/// D4's ledger digest for the Job commands keyed by the Job alone.
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
pub struct JobDigest<'a> {
    pub job_id: &'a str,
}

/// What an assignment committed (or, on a replay, what it had committed).
#[derive(Clone, Debug)]
pub struct Assigned {
    pub entry: QueueEntry,
    pub job: Job,
    pub replayed: bool,
}

impl Assigned {
    pub fn change(&self) -> QueueChange {
        QueueChange {
            entries: vec![self.entry.clone()],
            jobs: vec![self.job.clone()],
            requirements: Vec::new(),
            spool_ids: vec![self.job.spool_id.clone()],
        }
    }
}

/// What a release committed: the Job (`cancelled{releasedBeforeStart}`),
/// its entry (`closed{released}`), and the replacement at the entry's
/// old position.
#[derive(Clone, Debug)]
pub struct Released {
    pub job: Job,
    pub entry: QueueEntry,
    pub replacement: QueueEntry,
    pub replayed: bool,
}

impl Released {
    pub fn change(&self) -> QueueChange {
        QueueChange {
            entries: vec![self.entry.clone(), self.replacement.clone()],
            jobs: vec![self.job.clone()],
            requirements: Vec::new(),
            spool_ids: vec![self.job.spool_id.clone()],
        }
    }
}

/// What a retry committed: the new entry at the end of the Queue.
#[derive(Clone, Debug)]
pub struct Retried {
    pub entry: QueueEntry,
    pub replayed: bool,
}

impl Retried {
    pub fn change(&self) -> QueueChange {
        QueueChange {
            entries: vec![self.entry.clone()],
            ..QueueChange::default()
        }
    }
}

/// What a cancel before start committed: the Job
/// (`cancelled{cancelledBeforeStart}`), its entry (`closed{cancelled}`),
/// and every later open entry the close moved up.
#[derive(Clone, Debug)]
pub struct CancelledBeforeStart {
    pub job: Job,
    pub entry: QueueEntry,
    pub renumbered: Vec<QueueEntry>,
    pub replayed: bool,
}

impl CancelledBeforeStart {
    pub fn change(&self) -> QueueChange {
        let mut entries = vec![self.entry.clone()];
        entries.extend(self.renumbered.iter().cloned());
        QueueChange {
            entries,
            jobs: vec![self.job.clone()],
            requirements: Vec::new(),
            spool_ids: vec![self.job.spool_id.clone()],
        }
    }
}

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
}

fn job_holder(job_id: &str) -> ReservationHolder {
    ReservationHolder {
        kind: "job".to_string(),
        id: job_id.to_string(),
    }
}

fn load_job(tx: &Transaction<'_>, job_id: &str) -> Result<Job, RepositoryError> {
    jobs_repository::load_job(tx, job_id)?.ok_or_else(|| not_found(job_id))
}

fn load_entry(tx: &Transaction<'_>, entry_id: &str) -> Result<QueueEntry, RepositoryError> {
    queue_repository::load(tx, entry_id)?.ok_or_else(|| not_found(entry_id))
}

/// The entry `origin_entry_id` names as its origin (at most one: D2's
/// `queue_entries_one_successor` index), if any.
fn successor(tx: &Transaction<'_>, entry_id: &str) -> Result<Option<QueueEntry>, RepositoryError> {
    let id: Option<String> = tx
        .query_row(
            "SELECT id FROM queue_entries WHERE origin_entry_id = ?1",
            [entry_id],
            |row| row.get(0),
        )
        .optional()?;
    id.map(|id| load_entry(tx, &id)).transpose()
}

/// A replayed assignment's Job: the one whose reservation the original
/// assignment made under this operation id.
fn assigned_by_operation(
    tx: &Transaction<'_>,
    operation_id: &str,
) -> Result<Assigned, RepositoryError> {
    let job_id: String = tx
        .query_row(
            "SELECT holder_id FROM spool_reservations
             WHERE operation_id = ?1 AND holder_kind = 'job'",
            [operation_id],
            |row| row.get(0),
        )
        .optional()?
        .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
    let job = load_job(tx, &job_id)?;
    let entry = load_entry(tx, &job.queue_entry_id)?;
    Ok(Assigned {
        entry,
        job,
        replayed: true,
    })
}

pub(crate) fn reservation_error(
    world_spool_number: Option<i64>,
    spool_id: &str,
    reservation_id: Option<&str>,
    required_mg: Option<i64>,
    error: ReservationError,
) -> RepositoryError {
    RepositoryError::Reservation {
        spool_id: spool_id.to_string(),
        spool_number: world_spool_number,
        reservation_id: reservation_id.map(str::to_string),
        required_mg,
        error,
    }
}

pub(crate) fn spool_number(tx: &Transaction<'_>, spool_id: &str) -> Option<i64> {
    tx.query_row(
        "SELECT spool_number FROM spools WHERE id = ?1",
        [spool_id],
        |row| row.get(0),
    )
    .ok()
}

/// D4 "Assign": claims the operation id, then — all inside `tx` — the
/// entry is `queued` (`QUEUE_ENTRY_ACTION_NOT_ALLOWED`), the Printer exists
/// (`NOT_FOUND`) and has no active Job (`JOB_ACTIVE`), D5's
/// `check_assignment` passes over rows read in this transaction
/// (`ASSIGNMENT_BLOCKED`, or `VALIDATION` on `acknowledgeManualFacts` for
/// a Manual entry whose unconfirmed facts the operator didn't
/// acknowledge), the Spool is reserved for `("job", jobId)`, and the Job
/// (`assigned`, settlement `open`) and its `assigned` event are inserted
/// before the entry becomes `assigned`.
pub fn assign(
    tx: &Transaction<'_>,
    world: &dyn WorldReader,
    req: &AssignRequest,
    now: &str,
) -> Result<Assigned, RepositoryError> {
    let digest = operations::digest(&AssignDigest {
        entry_id: &req.entry_id,
        printer_id: &req.printer_id,
        spool_id: &req.spool_id,
        acknowledge_manual_facts: req.acknowledge_manual_facts(),
        assigned_by: req.assigned_by,
    });
    if operations::claim(
        tx,
        &req.operation_id,
        OperationKind::AssignQueueEntry,
        &digest,
    )? == Claim::Replay
    {
        return assigned_by_operation(tx, &req.operation_id);
    }

    let entry = load_entry(tx, &req.entry_id)?;
    if entry.state != QueueEntryState::Queued {
        return Err(RepositoryError::QueueEntryActionNotAllowed {
            entry_id: entry.id,
            action: QueueEntryAction::Assign,
            state: entry.state,
        });
    }
    let printer_exists: bool = tx.query_row(
        "SELECT EXISTS(SELECT 1 FROM printers WHERE id = ?1)",
        [&req.printer_id],
        |row| row.get(0),
    )?;
    if !printer_exists {
        return Err(not_found(&req.printer_id));
    }
    if let Some(active) = jobs_repository::active_job_for_printer(tx, &req.printer_id)? {
        return Err(RepositoryError::JobActive {
            printer_id: req.printer_id.clone(),
            job_id: active.id,
        });
    }

    // D5, re-checked inside the transaction over what it is about to
    // commit against.
    let rows = World::read(tx, world)?;
    let facts = facts_for(tx, &entry)?;
    let views = rows.views(&entry, Some(&req.printer_id));
    let input = rows.input(&entry, &facts, &views);
    match eligibility::check_assignment(&input, &req.printer_id, &req.spool_id, req.mode) {
        Ok(()) => {}
        Err(AssignmentCheckError::PrinterNotInView) => return Err(not_found(&req.printer_id)),
        Err(AssignmentCheckError::Blocked(blockers)) => {
            let unacknowledged_manual_facts = entry.policy == DispatchPolicy::Manual
                && !req.acknowledge_manual_facts()
                && blockers
                    .iter()
                    .any(|blocker| blocker.code == BlockerCode::NeedsManualPrinter);
            if unacknowledged_manual_facts {
                return Err(RepositoryError::Validation {
                    field_path: "acknowledgeManualFacts",
                });
            }
            return Err(RepositoryError::AssignmentBlocked {
                entry_id: entry.id,
                printer_id: req.printer_id.clone(),
                spool_id: req.spool_id.clone(),
                blockers,
            });
        }
    }

    let printer = rows
        .printers
        .iter()
        .find(|row| row.stored.id == req.printer_id)
        .ok_or_else(|| not_found(&req.printer_id))?;
    let job_id = jobs_repository::new_job_id();
    let reservation_id = reservations::reserve(
        tx,
        &req.spool_id,
        &job_holder(&job_id),
        entry.estimate.amount_mg,
        &req.operation_id,
    )
    .map_err(|error| {
        reservation_error(
            rows.spool(&req.spool_id).map(|spool| spool.spool_number),
            &req.spool_id,
            None,
            Some(entry.estimate.amount_mg),
            error,
        )
    })?;

    let job = jobs_repository::insert_job(
        tx,
        &NewJob {
            id: job_id,
            queue_entry_id: entry.id.clone(),
            slice_revision_id: entry.slice_revision_id.clone(),
            printer_id: req.printer_id.clone(),
            printer_snapshot: PrinterSnapshot {
                name: printer.stored.name.clone(),
                location: printer.stored.location.clone(),
                catalog_ref: Some(printer.stored.catalog_ref.clone()),
                adapter_kind: printer
                    .stored
                    .connection
                    .as_ref()
                    .map(|connection| connection.kind.clone()),
                profile: printer.profile.clone(),
            },
            spool_id: req.spool_id.clone(),
            reservation_id,
            estimate_mg: entry.estimate.amount_mg,
            assigned_by: req.assigned_by,
            manual_facts_acknowledged: req.acknowledge_manual_facts(),
        },
        now,
    )?;
    let entry = queue_repository::apply(tx, &entry.id, &EntryEvent::Assign, Some(&job.id), now)?;
    Ok(Assigned {
        entry,
        job,
        replayed: false,
    })
}

/// A Job command's precondition: D3's state table allows `event` (the
/// command's `action`) from the Job's current state.
fn require_allowed(
    job: &Job,
    event: JobEventKind,
    action: JobAction,
) -> Result<(), RepositoryError> {
    if super::state::transition(job.state, event).is_err() {
        return Err(RepositoryError::JobActionNotAllowed {
            job_id: job.id.clone(),
            action,
            state: job.state,
        });
    }
    Ok(())
}

fn claim_job_command(
    tx: &Transaction<'_>,
    operation_id: &str,
    kind: OperationKind,
    job_id: &str,
) -> Result<Claim, RepositoryError> {
    operations::claim(
        tx,
        operation_id,
        kind,
        &operations::digest(&JobDigest { job_id }),
    )
}

/// D4 "Release": claims `releaseJob`; the Job is `assigned` or
/// `awaitingStart` (`JOB_ACTION_NOT_ALLOWED`); it becomes
/// `cancelled{releasedBeforeStart}` with settlement `notRequired`; its
/// reservation is released; its entry closes `released` (position NULL);
/// and a replacement is inserted at the freed position, joining the
/// entry's lineage. Nothing else moves (D2).
pub fn release(
    tx: &Transaction<'_>,
    operation_id: &str,
    job_id: &str,
    now: &str,
) -> Result<Released, RepositoryError> {
    if claim_job_command(tx, operation_id, OperationKind::ReleaseJob, job_id)? == Claim::Replay {
        let job = load_job(tx, job_id)?;
        let entry = load_entry(tx, &job.queue_entry_id)?;
        let replacement = successor(tx, &entry.id)?
            .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
        return Ok(Released {
            job,
            entry,
            replacement,
            replayed: true,
        });
    }

    let job = load_job(tx, job_id)?;
    require_allowed(&job, JobEventKind::Released, JobAction::Release)?;
    let freed_position = load_entry(tx, &job.queue_entry_id)?
        .position
        .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;

    let job = jobs_repository::transition(
        tx,
        job_id,
        JobEventKind::Released,
        JobChange {
            cancel_reason: Some(CancelReason::ReleasedBeforeStart),
            operation_id: Some(operation_id.to_string()),
            ..JobChange::default()
        },
        now,
    )?;
    reservations::release(tx, &job.reservation_id).map_err(|error| {
        reservation_error(
            spool_number(tx, &job.spool_id),
            &job.spool_id,
            Some(&job.reservation_id),
            None,
            error,
        )
    })?;
    let entry = queue_repository::apply(tx, &job.queue_entry_id, &EntryEvent::Release, None, now)?;
    let replacement = queue_repository::create_linked(
        tx,
        &entry,
        OriginKind::Release,
        Some(freed_position),
        now,
    )?;
    Ok(Released {
        job,
        entry,
        replacement,
        replayed: false,
    })
}

/// D4 "Retry": claims `retryJob`; the Job is terminal and wasn't released
/// before start (`JOB_ACTION_NOT_ALLOWED`: a released entry's replacement
/// is already queued) and hasn't been retried (`JOB_ALREADY_RETRIED`);
/// a linked entry (`origin_kind = retry`, the origin's lineage and
/// `copyIndex`) goes to the end. The Job and its entry are untouched.
pub fn retry(
    tx: &Transaction<'_>,
    operation_id: &str,
    job_id: &str,
    now: &str,
) -> Result<Retried, RepositoryError> {
    if claim_job_command(tx, operation_id, OperationKind::RetryJob, job_id)? == Claim::Replay {
        let job = load_job(tx, job_id)?;
        let entry = successor(tx, &job.queue_entry_id)?
            .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
        return Ok(Retried {
            entry,
            replayed: true,
        });
    }

    let job = load_job(tx, job_id)?;
    let retryable =
        job.state.is_terminal() && job.cancel_reason != Some(CancelReason::ReleasedBeforeStart);
    if !retryable {
        return Err(RepositoryError::JobActionNotAllowed {
            job_id: job.id,
            action: JobAction::Retry,
            state: job.state,
        });
    }
    if let Some(existing) = successor(tx, &job.queue_entry_id)? {
        return Err(RepositoryError::JobAlreadyRetried {
            job_id: job.id,
            retry_entry_id: existing.id,
        });
    }
    let origin = load_entry(tx, &job.queue_entry_id)?;
    let entry = queue_repository::create_linked(tx, &origin, OriginKind::Retry, None, now)?;
    Ok(Retried {
        entry,
        replayed: false,
    })
}

/// D4 "Cancel before start": claims `cancelJob`; the Job is `assigned` or
/// `awaitingStart` (`JOB_ACTION_NOT_ALLOWED`; cancelling after start is a
/// host handoff, a later task's branch); it becomes
/// `cancelled{cancelledBeforeStart}` with settlement `notRequired`; its
/// reservation is released; its entry closes `cancelled`, and every later
/// open entry moves up.
pub fn cancel_before_start(
    tx: &Transaction<'_>,
    operation_id: &str,
    job_id: &str,
    now: &str,
) -> Result<CancelledBeforeStart, RepositoryError> {
    if claim_job_command(tx, operation_id, OperationKind::CancelJob, job_id)? == Claim::Replay {
        let job = load_job(tx, job_id)?;
        let entry = load_entry(tx, &job.queue_entry_id)?;
        return Ok(CancelledBeforeStart {
            job,
            entry,
            renumbered: Vec::new(),
            replayed: true,
        });
    }

    let job = load_job(tx, job_id)?;
    require_allowed(&job, JobEventKind::CancelledBeforeStart, JobAction::Cancel)?;
    let freed_position = load_entry(tx, &job.queue_entry_id)?
        .position
        .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;

    let job = jobs_repository::transition(
        tx,
        job_id,
        JobEventKind::CancelledBeforeStart,
        JobChange {
            cancel_reason: Some(CancelReason::CancelledBeforeStart),
            operation_id: Some(operation_id.to_string()),
            ..JobChange::default()
        },
        now,
    )?;
    reservations::release(tx, &job.reservation_id).map_err(|error| {
        reservation_error(
            spool_number(tx, &job.spool_id),
            &job.spool_id,
            Some(&job.reservation_id),
            None,
            error,
        )
    })?;
    let entry = queue_repository::apply(
        tx,
        &job.queue_entry_id,
        &EntryEvent::JobTerminal(CloseReason::Cancelled),
        None,
        now,
    )?;
    let renumbered = open_entries_from(tx, freed_position)?;
    Ok(CancelledBeforeStart {
        job,
        entry,
        renumbered,
        replayed: false,
    })
}

/// Every open entry at `position` or later, in position order — the rows
/// a close at `position` just moved up.
pub fn open_entries_from(
    tx: &Transaction<'_>,
    position: i64,
) -> Result<Vec<QueueEntry>, RepositoryError> {
    let mut statement =
        tx.prepare("SELECT id FROM queue_entries WHERE position >= ?1 ORDER BY position")?;
    let ids = statement
        .query_map(params![position], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.iter().map(|id| load_entry(tx, id)).collect()
}
