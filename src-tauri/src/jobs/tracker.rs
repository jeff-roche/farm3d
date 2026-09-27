//! P7 D7: the outcome tracker. It runs inside the dispatch driver's task
//! (`jobs::services`) and follows every `printing` or `paused` Job to its
//! end from the host's print history, never from a status string.
//!
//! - **Pure:** [`verdict_from_history`] (the pin rule, with P6's start-time
//!   bound, and D7's verdict table), [`next_inconclusive_checks`],
//!   [`cancel_reason`], [`status_hint`], and [`reports_print_of`].
//! - **A poll** ([`check`]): one `job_history` read from the Printer's
//!   *current* Connection, with no lock held, then one transaction under
//!   the Printer lock ([`apply_poll`]) that re-reads the Job and writes the
//!   pin, the verdict, `inconclusive_checks`, and `host_unreachable_since`.
//!   A poll that could not run sets `host_unreachable_since`; only a poll
//!   that ran clears it (rulings R5, R7).
//! - **A status** ([`observe_status`]): progress and `printing` ⇄ `paused`
//!   on the Job's own file, and a hint to [`check`] at once. Never an end.
//! - **An end** ([`end_job`]): the Job's terminal transition, its entry
//!   closed and the Queue renumbered, and the material hook
//!   [`settle_terminal_material`] (Task 9) in the same transaction. The
//!   operator's [`declare`] ends a Job the same way.

use std::time::Duration;

use chrono::{DateTime, Utc};
use rusqlite::{Connection, Transaction};
use serde::Serialize;

use crate::connections::capabilities::{HistoryJob, HistoryQuery};
use crate::connections::{ConnectionState, PrinterStatus};
use crate::host_ops::{
    parse_time, repository as host_ops_repository, HostOperationEndpoint, HostOperationState,
};
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::operational::{OperationalState, TelemetryFreshness};
use crate::printers::StoredPrinter;
use crate::queue::repository as queue_repository;
use crate::queue::state::EntryEvent;
use crate::queue::{CloseReason, QueueChange};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::{decode_enum, encode_enum};
use crate::RuntimeServices;

use super::assign::open_entries_from;
use super::dispatch::open_outcome_unknown_requirement;
use super::repository::{self as jobs_repository, JobChange};
use super::{
    CancelReason, DeclaredOutcome, Job, JobAction, JobEventKind, JobState,
    ReconciliationRequirement, RequirementKind, RequirementResolution, RequirementStatus,
    SettlementMethod,
};

/// What one history poll says about a Job (D7's verdict table).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum TrackerVerdict {
    StillRunning,
    Completed,
    Failed,
    Cancelled,
    Inconclusive,
}

/// The backend-only facts [`verdict_from_history`] reads about a Job.
#[derive(Clone, PartialEq, Debug)]
pub struct TrackedJob {
    /// The file the Job staged (`jobs.host_path`).
    pub host_path: String,
    /// The newest history job id before the start was sent.
    pub history_mark: u64,
    /// The history job already pinned, if any.
    pub host_job_id: Option<u64>,
    /// The earliest `start_time` a history job may have to be pinned:
    /// the start Host Operation's `dispatched_at` less P6's
    /// `START_SKEW_TOLERANCE`. `None` (the start was never sent) never pins.
    pub earliest_start_epoch_s: Option<f64>,
}

/// What the tracker observed besides the history page.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Observed {
    /// The Printer's current endpoint is the one the start was sent to.
    pub same_endpoint: bool,
    /// The live status reports a print of the Job's own file.
    pub reports_our_file: bool,
}

/// D7's verdict for `job` over one history page: the history job the
/// verdict comes from (the existing pin, or one pinned by this poll), and
/// the verdict. Pure.
pub fn verdict_from_history(
    job: &TrackedJob,
    history: &[HistoryJob],
    observed: Observed,
) -> (Option<u64>, TrackerVerdict) {
    // History from a host other than the one the start went to: its ids
    // and the mark don't belong to this Job.
    if !observed.same_endpoint {
        return (None, TrackerVerdict::Inconclusive);
    }
    let pinned = match job.host_job_id {
        Some(id) => Some(id),
        None => pin_candidate(job, history),
    };
    match pinned {
        Some(id) => {
            let verdict = history
                .iter()
                .find(|entry| entry.job_id == id)
                .map_or(TrackerVerdict::Inconclusive, |entry| {
                    verdict_of(&entry.status)
                });
            (Some(id), verdict)
        }
        None if observed.reports_our_file => (None, TrackerVerdict::StillRunning),
        None => (None, TrackerVerdict::Inconclusive),
    }
}

/// D7 "Pinning", P6's start rule (b): the lowest history job for our file
/// above the mark that started no earlier than the dispatch bound.
fn pin_candidate(job: &TrackedJob, history: &[HistoryJob]) -> Option<u64> {
    let earliest = job.earliest_start_epoch_s?;
    history
        .iter()
        .filter(|entry| {
            entry.filename == job.host_path
                && entry.job_id > job.history_mark
                && entry.start_time_epoch_s >= earliest
        })
        .map(|entry| entry.job_id)
        .min()
}

/// D7's verdict table for a pinned history job's status.
fn verdict_of(status: &str) -> TrackerVerdict {
    match status {
        "in_progress" => TrackerVerdict::StillRunning,
        "completed" => TrackerVerdict::Completed,
        "cancelled" => TrackerVerdict::Cancelled,
        "error" | "klippy_shutdown" | "klippy_disconnect" | "server_exit" => TrackerVerdict::Failed,
        _ => TrackerVerdict::Inconclusive,
    }
}

/// D7 "Unprovable outcome": the `inconclusive_checks` after a poll with
/// `verdict`, and whether the Job now becomes `outcomeUnknown`. Pure.
pub fn next_inconclusive_checks(verdict: TrackerVerdict, checks: u32, limit: u32) -> (u32, bool) {
    if verdict != TrackerVerdict::Inconclusive {
        return (0, false);
    }
    let checks = checks.saturating_add(1);
    (checks, checks >= limit)
}

/// D7 "Cancel reason": history proved `cancelled`. `cancelledByOperator`
/// when the Job handed off a cancel whose Host Operation is anything but
/// `failed` (`cancel_ops` are those ops' states); otherwise the printer
/// cancelled it (`hostCancelled`). Pure.
pub fn cancel_reason(cancel_ops: &[HostOperationState]) -> CancelReason {
    if cancel_ops
        .iter()
        .any(|state| *state != HostOperationState::Failed)
    {
        CancelReason::CancelledByOperator
    } else {
        CancelReason::HostCancelled
    }
}

/// What one live status says about a `printing`/`paused` Job (D7): a hint,
/// never proof of an end.
#[derive(Clone, Copy, PartialEq, Eq, Debug, Default)]
pub struct StatusHint {
    /// `floor(progress × 100)`, when it grew past `max_progress_pct`.
    pub progress_pct: Option<i64>,
    /// `Paused` or `Resumed`: the status shows our file in the other state.
    pub follow: Option<JobEventKind>,
    /// The status says our file finished, was cancelled, or failed: check
    /// history now.
    pub check_now: bool,
}

/// D7's status inputs for a Job in `state` with `host_path`, whose
/// progress so far is `max_progress_pct`. Only an Online status that
/// reports the Job's own file says anything, and only one that reports it
/// printing or paused moves the progress. Pure.
pub fn status_hint(
    state: JobState,
    host_path: &str,
    max_progress_pct: i64,
    status: &PrinterStatus,
) -> StatusHint {
    let ours = status.connection_state == ConnectionState::Online
        && status.telemetry.job_name.as_deref() == Some(host_path);
    if !ours {
        return StatusHint::default();
    }
    // Owner decision 5: only this Job's print counts. A host that ended the
    // previous run of the same file still reports that file and its
    // progress until the next print begins, so an ended status says nothing
    // about this Job's progress.
    let in_progress = matches!(
        status.operational_state,
        OperationalState::Printing | OperationalState::Paused
    );
    let progress_pct = status
        .telemetry
        .progress
        .filter(|_| in_progress)
        .filter(|progress| progress.is_finite())
        .map(|progress| ((progress * 100.0).floor() as i64).clamp(0, 100))
        .filter(|pct| *pct > max_progress_pct);
    let fresh = status.freshness == TelemetryFreshness::Fresh;
    let follow = match (state, status.operational_state) {
        (JobState::Printing, OperationalState::Paused) if fresh => Some(JobEventKind::Paused),
        (JobState::Paused, OperationalState::Printing) if fresh => Some(JobEventKind::Resumed),
        _ => None,
    };
    let check_now = matches!(
        status.operational_state,
        OperationalState::Finished | OperationalState::Cancelled | OperationalState::Failed
    );
    StatusHint {
        progress_pct,
        follow,
        check_now,
    }
}

/// Whether `status` reports a print of `host_path` in progress (printing
/// or paused, Online): the verdict table's "the Printer reports our file".
/// A finished print is not one: its end comes from history.
pub fn reports_print_of(status: Option<&PrinterStatus>, host_path: &str) -> bool {
    status.is_some_and(|status| {
        status.connection_state == ConnectionState::Online
            && matches!(
                status.operational_state,
                OperationalState::Printing | OperationalState::Paused
            )
            && status.telemetry.job_name.as_deref() == Some(host_path)
    })
}

// --- the tracked Job ---------------------------------------------------------

/// `dispatched_at` is stored to the second: the send happened up to 1 s
/// after it. The pin bound adds that back, as P6's rule (b) does, so the
/// bound never lets in a job that started more than the skew tolerance
/// before the real send.
const STORED_PRECISION: Duration = Duration::from_secs(1);

/// A `printing`/`paused` Job as the tracker reads it inside one
/// transaction: its row, the backend-only columns, and the endpoint its
/// start was sent to.
struct Subject {
    job: Job,
    tracked: TrackedJob,
    inconclusive_checks: u32,
    start_endpoint: Option<HostOperationEndpoint>,
}

fn epoch_seconds(time: DateTime<Utc>) -> f64 {
    time.timestamp() as f64 + f64::from(time.timestamp_subsec_millis()) / 1000.0
}

fn chrono_duration(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX)
}

/// The Job, when it is `printing` or `paused`; `None` otherwise. `skew` is
/// P6's `START_SKEW_TOLERANCE`.
fn read_subject(
    conn: &Connection,
    job_id: &str,
    skew: Duration,
) -> Result<Option<Subject>, RepositoryError> {
    let Some(job) = jobs_repository::load_job(conn, job_id)? else {
        return Ok(None);
    };
    if !matches!(job.state, JobState::Printing | JobState::Paused) {
        return Ok(None);
    }
    let Some(columns) = jobs_repository::tracking(conn, job_id)? else {
        return Ok(None);
    };
    let start = match &columns.start_host_operation_id {
        Some(id) => host_ops_repository::load(conn, id)?,
        None => None,
    };
    // Fail-safe: anything missing (none can be, by the CHECKs) never pins.
    let earliest_start_epoch_s = start
        .as_ref()
        .and_then(|op| op.dispatched_at.as_deref())
        .and_then(parse_time)
        .filter(|_| job.host_path.is_some())
        .map(|at| epoch_seconds(at + chrono_duration(STORED_PRECISION) - chrono_duration(skew)));
    let tracked = TrackedJob {
        host_path: job.host_path.clone().unwrap_or_default(),
        history_mark: columns
            .history_mark
            .map_or(u64::MAX, |mark| u64::try_from(mark).unwrap_or(u64::MAX)),
        host_job_id: columns.host_job_id.and_then(|id| u64::try_from(id).ok()),
        earliest_start_epoch_s,
    };
    Ok(Some(Subject {
        inconclusive_checks: u32::try_from(columns.inconclusive_checks).unwrap_or(u32::MAX),
        start_endpoint: start.map(|op| op.endpoint),
        tracked,
        job,
    }))
}

// --- one poll ------------------------------------------------------------------

/// A history poll's result.
#[derive(Clone, Debug)]
pub(crate) enum Poll {
    /// The Printer is not Online, has no usable Connection or credential,
    /// or the history query failed.
    CouldNotRun,
    /// The history page, read from `endpoint`.
    Read {
        endpoint: HostOperationEndpoint,
        history: Vec<HistoryJob>,
    },
}

/// One `job_history` read from the Printer's current Connection (D7
/// "Endpoint"). Holds no lock: it is a network read.
async fn read_history<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer: &StoredPrinter,
) -> Poll {
    let host_ops = &services.host_ops;
    let Some(config) = printer.connection.as_ref() else {
        return Poll::CouldNotRun;
    };
    if !host_ops.is_online(&printer.id) {
        return Poll::CouldNotRun;
    }
    let Ok(key) = host_ops.credential(printer) else {
        return Poll::CouldNotRun;
    };
    let Some(query) = host_ops.factory.host_state(config, key) else {
        return Poll::CouldNotRun;
    };
    let page = query
        .job_history(HistoryQuery {
            since_epoch_s: None,
            limit: host_ops.timings.history_query_limit,
        })
        .await;
    match page {
        Ok(history) => Poll::Read {
            endpoint: HostOperationEndpoint::of(config),
            history,
        },
        Err(_) => Poll::CouldNotRun,
    }
}

/// D7: one tracker poll of `job_id`. Returns what it committed, for the
/// driver to publish; `None` when the Job is not `printing`/`paused` or
/// nothing changed.
pub(crate) async fn check<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    job_id: &str,
) -> Result<Option<QueueChange>, RepositoryError> {
    let skew = services.host_ops.timings.start_skew_tolerance;
    // One read connection for the Job and its Printer.
    let read = services
        .storage
        .read_transaction(|tx| {
            Ok((|| {
                let Some(subject) = read_subject(tx, job_id, skew)? else {
                    return Ok(None);
                };
                let printer =
                    crate::printers::repository::load_in(tx, &subject.job.printer_id).ok();
                Ok::<_, RepositoryError>(Some((subject, printer)))
            })())
        })
        .map_err(RepositoryError::Storage)??;
    let Some((subject, printer)) = read else {
        return Ok(None);
    };
    let printer_id = subject.job.printer_id.clone();
    let poll = match &printer {
        Some(printer) => read_history(services, printer).await,
        None => Poll::CouldNotRun,
    };
    let reports_our_file = reports_print_of(
        services.manager.statuses().get(&printer_id),
        &subject.tracked.host_path,
    );

    let printer_lock = services.host_ops.printer_lock(&printer_id);
    let _serialized = printer_lock.lock().await;
    let now = crate::host_ops::format_time(services.jobs.now());
    let limit = services.jobs.timings.inconclusive_limit;
    services
        .storage
        .write_repo(|tx| apply_poll(tx, job_id, &poll, reports_our_file, skew, limit, &now))
}

/// D7, inside the Printer lock: re-reads the Job and applies one poll —
/// `host_unreachable_since`, the pin (`HostJobPinned`), the verdict, and
/// `inconclusive_checks`.
pub(crate) fn apply_poll(
    tx: &Transaction<'_>,
    job_id: &str,
    poll: &Poll,
    reports_our_file: bool,
    skew: Duration,
    inconclusive_limit: u32,
    now: &str,
) -> Result<Option<QueueChange>, RepositoryError> {
    let Some(subject) = read_subject(tx, job_id, skew)? else {
        return Ok(None);
    };
    let (endpoint, history) = match poll {
        Poll::CouldNotRun => {
            if subject.job.host_unreachable_since.is_some() {
                return Ok(None);
            }
            let job = jobs_repository::update_columns(
                tx,
                job_id,
                JobChange {
                    host_unreachable_since: Some(now.to_string()),
                    ..JobChange::default()
                },
                now,
            )?;
            return Ok(Some(job_change(job)));
        }
        Poll::Read { endpoint, history } => (endpoint, history),
    };

    let observed = Observed {
        same_endpoint: subject.start_endpoint.as_ref() == Some(endpoint),
        reports_our_file,
    };
    let (pinned, mut verdict) = verdict_from_history(&subject.tracked, history, observed);
    let pinned = match pinned.map(i64::try_from) {
        Some(Ok(id)) => Some(id),
        // An id SQLite can't hold is never pinned (fail-safe).
        Some(Err(_)) => {
            verdict = TrackerVerdict::Inconclusive;
            None
        }
        None => None,
    };
    let (checks, outcome_unknown) =
        next_inconclusive_checks(verdict, subject.inconclusive_checks, inconclusive_limit);

    let mut job = None;
    let clear_unreachable = subject.job.host_unreachable_since.is_some();
    if clear_unreachable || checks != subject.inconclusive_checks {
        job = Some(jobs_repository::update_columns(
            tx,
            job_id,
            JobChange {
                clear_host_unreachable_since: clear_unreachable,
                inconclusive_checks: Some(i64::from(checks)),
                ..JobChange::default()
            },
            now,
        )?);
    }
    if let (None, Some(id)) = (subject.tracked.host_job_id, pinned) {
        job = Some(jobs_repository::transition(
            tx,
            job_id,
            JobEventKind::HostJobPinned,
            JobChange {
                host_job_id: Some(id),
                detail: Some(serde_json::json!({ "hostJobId": id })),
                ..JobChange::default()
            },
            now,
        )?);
    }

    let proved = |status: &str| serde_json::json!({ "hostJobId": pinned, "status": status });
    let status = pinned
        .and_then(|id| {
            history
                .iter()
                .find(|entry| i64::try_from(entry.job_id) == Ok(id))
        })
        .map(|entry| entry.status.clone())
        .unwrap_or_default();
    let ended = match verdict {
        TrackerVerdict::Completed => Some(end_job(
            tx,
            job_id,
            JobEventKind::Completed,
            JobChange {
                settlement_method: Some(SettlementMethod::Estimated),
                detail: Some(proved(&status)),
                ..JobChange::default()
            },
            now,
        )?),
        TrackerVerdict::Failed => Some(end_job(
            tx,
            job_id,
            JobEventKind::Failed,
            JobChange {
                detail: Some(proved(&status)),
                ..JobChange::default()
            },
            now,
        )?),
        TrackerVerdict::Cancelled => {
            let reason = cancel_reason(&cancel_op_states(tx, job_id)?);
            Some(end_job(
                tx,
                job_id,
                JobEventKind::Cancelled,
                JobChange {
                    cancel_reason: Some(reason),
                    detail: Some(proved(&status)),
                    ..JobChange::default()
                },
                now,
            )?)
        }
        TrackerVerdict::StillRunning | TrackerVerdict::Inconclusive => None,
    };
    if let Some(ended) = ended {
        return Ok(Some(ended));
    }
    if outcome_unknown {
        let job = jobs_repository::transition(
            tx,
            job_id,
            JobEventKind::OutcomeUnknown,
            JobChange {
                detail: Some(serde_json::json!({ "inconclusiveChecks": checks })),
                ..JobChange::default()
            },
            now,
        )?;
        let mut change = job_change(job);
        change
            .requirements
            .extend(open_outcome_unknown_requirement(tx, job_id, now)?);
        return Ok(Some(change));
    }
    Ok(job.map(job_change))
}

fn job_change(job: Job) -> QueueChange {
    QueueChange {
        jobs: vec![job],
        ..QueueChange::default()
    }
}

/// The states of every cancel Host Operation the Job handed off.
fn cancel_op_states(
    tx: &Transaction<'_>,
    job_id: &str,
) -> Result<Vec<HostOperationState>, RepositoryError> {
    let mut statement =
        tx.prepare("SELECT state FROM host_operations WHERE job_id = ?1 AND kind = 'cancel'")?;
    let states = statement
        .query_map([job_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    states
        .iter()
        .map(|state| {
            decode_enum(state).map_err(|_| {
                RepositoryError::Storage(crate::persistence::StorageError::OperationFailed)
            })
        })
        .collect()
}

// --- status ------------------------------------------------------------------------

/// D7's status inputs for the Printer's `printing`/`paused` Job: its
/// progress and `printing` ⇄ `paused` on its own file, written under the
/// Printer lock. Returns what it committed, and whether the status asks
/// for a history check now.
pub(crate) async fn observe_status<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    job: &Job,
) -> Result<(Option<QueueChange>, bool), RepositoryError> {
    let Some(host_path) = job.host_path.as_deref() else {
        return Ok((None, false));
    };
    let Some(status) = services.manager.statuses().remove(&job.printer_id) else {
        return Ok((None, false));
    };
    let hint = status_hint(job.state, host_path, job.max_progress_pct, &status);
    if hint.progress_pct.is_none() && hint.follow.is_none() {
        return Ok((None, hint.check_now));
    }
    let printer_lock = services.host_ops.printer_lock(&job.printer_id);
    let _serialized = printer_lock.lock().await;
    let now = crate::host_ops::format_time(services.jobs.now());
    let written = services.storage.write_repo(|tx| {
        let Some(job) = jobs_repository::load_job(tx, &job.id)? else {
            return Ok(None);
        };
        let Some(host_path) = job.host_path.as_deref() else {
            return Ok(None);
        };
        if !matches!(job.state, JobState::Printing | JobState::Paused) {
            return Ok(None);
        }
        // Re-read against the committed row: the state may have moved.
        let hint = status_hint(job.state, host_path, job.max_progress_pct, &status);
        let mut written = None;
        if let Some(pct) = hint.progress_pct {
            written = Some(jobs_repository::update_columns(
                tx,
                &job.id,
                JobChange {
                    max_progress_pct: Some(pct),
                    ..JobChange::default()
                },
                &now,
            )?);
        }
        if let Some(event) = hint.follow {
            written = Some(jobs_repository::transition(
                tx,
                &job.id,
                event,
                JobChange {
                    detail: Some(serde_json::json!({ "source": "status" })),
                    ..JobChange::default()
                },
                &now,
            )?);
        }
        Ok(written)
    })?;
    Ok((written.map(job_change), hint.check_now))
}

// --- ends ------------------------------------------------------------------------

/// What [`settle_terminal_material`] wrote.
#[derive(Clone, Debug, Default)]
pub struct SettlementEffects {
    /// Requirements it opened (a `materialReconciliation`).
    pub requirements: Vec<ReconciliationRequirement>,
    /// Spools whose reservation it changed.
    pub spool_ids: Vec<String>,
}

/// The material work D4's "Tracker terminal" and "Declare" rows put in the
/// terminal transaction, run by [`end_job`] right after the Job's terminal
/// transition (which already set the settlement *state*: `settled/
/// estimated` for `Completed` and `DeclaredCompleted`, `pending` for the
/// failed and cancelled events):
///
/// - completed: [`settlement::on_completed`] (`reservations::consume` the
///   full estimate, one `Consumption`);
/// - failed or cancelled: [`settlement::on_failed_or_cancelled`]
///   (`reservations::mark_unresolved`, and a `materialReconciliation`
///   requirement, `pending`).
pub(crate) fn settle_terminal_material(
    tx: &Transaction<'_>,
    job: &Job,
    now: &str,
) -> Result<SettlementEffects, RepositoryError> {
    match job.state {
        JobState::Completed => {
            super::settlement::on_completed(tx, job, now)?;
            Ok(SettlementEffects {
                requirements: Vec::new(),
                spool_ids: vec![job.spool_id.clone()],
            })
        }
        JobState::Failed | JobState::Cancelled => {
            super::settlement::on_failed_or_cancelled(tx, job, now)?;
            let requirement = jobs_repository::requirements_for_job(tx, &job.id)?
                .into_iter()
                .find(|requirement| requirement.kind == RequirementKind::MaterialReconciliation)
                .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
            Ok(SettlementEffects {
                requirements: vec![requirement],
                spool_ids: vec![job.spool_id.clone()],
            })
        }
        // `end_job`'s caller already rejected a non-terminal event before
        // this runs.
        _ => Ok(SettlementEffects::default()),
    }
}

/// D4 "Tracker terminal" (and "Declare"): the Job's terminal `event`, its
/// Queue Entry closed (`JobTerminal`) with every later open entry moved
/// up, and [`settle_terminal_material`], in the caller's transaction.
pub(crate) fn end_job(
    tx: &Transaction<'_>,
    job_id: &str,
    event: JobEventKind,
    change: JobChange,
    now: &str,
) -> Result<QueueChange, RepositoryError> {
    let job = jobs_repository::transition(tx, job_id, event, change, now)?;
    let reason = match job.state {
        JobState::Completed => CloseReason::Completed,
        JobState::Failed => CloseReason::Failed,
        JobState::Cancelled => CloseReason::Cancelled,
        // Only terminal events reach here.
        _ => {
            return Err(RepositoryError::IllegalJobTransition {
                job_id: job_id.to_string(),
                from: job.state,
                event,
            })
        }
    };
    let freed = queue_repository::load(tx, &job.queue_entry_id)?
        .and_then(|entry| entry.position)
        .ok_or(RepositoryError::Storage(
            crate::persistence::StorageError::OperationFailed,
        ))?;
    let entry = queue_repository::apply(
        tx,
        &job.queue_entry_id,
        &EntryEvent::JobTerminal(reason),
        None,
        now,
    )?;
    let mut entries = vec![entry];
    entries.extend(open_entries_from(tx, freed)?);
    let effects = settle_terminal_material(tx, &job, now)?;
    Ok(QueueChange {
        entries,
        jobs: vec![job],
        requirements: effects.requirements,
        spool_ids: effects.spool_ids,
    })
}

// --- declare ---------------------------------------------------------------------

/// D4's ledger digest for `declare_job_outcome` (fields in this order).
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct DeclareDigest<'a> {
    job_id: &'a str,
    outcome: DeclaredOutcome,
    acknowledgement: &'a str,
}

/// `declare_job_outcome`'s only accepted acknowledgement (D9).
pub const DECLARE_ACKNOWLEDGEMENT: &str = "hostStateUnknown";

/// What a declare committed (or, on a replay, the rows as they are).
#[derive(Clone, Debug)]
pub struct Declared {
    pub change: QueueChange,
    pub replayed: bool,
}

/// D4 "Declare" and D9, in the caller's transaction under the Printer
/// lock: claims `declareJobOutcome`; the Job is `outcomeUnknown`, or
/// `printing`/`paused` with `host_unreachable_since` at least
/// `declare_after` before `at` (else `JOB_ACTION_NOT_ALLOWED`); the Job's
/// `Declared*` event ends it through [`end_job`]; and the
/// `jobOutcomeUnknown` requirement, if any, is resolved `{ declared,
/// outcome }`. Nothing is sent to the host.
pub fn declare(
    tx: &Transaction<'_>,
    operation_id: &str,
    job_id: &str,
    outcome: DeclaredOutcome,
    at: DateTime<Utc>,
    declare_after: Duration,
) -> Result<Declared, RepositoryError> {
    let digest = operations::digest(&DeclareDigest {
        job_id,
        outcome,
        acknowledgement: DECLARE_ACKNOWLEDGEMENT,
    });
    let claim = operations::claim(tx, operation_id, OperationKind::DeclareJobOutcome, &digest)?;
    let job = jobs_repository::load_job(tx, job_id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: job_id.to_string(),
    })?;
    if claim == Claim::Replay {
        let entry = queue_repository::load(tx, &job.queue_entry_id)?;
        return Ok(Declared {
            change: QueueChange {
                entries: entry.into_iter().collect(),
                requirements: jobs_repository::requirements_for_job(tx, job_id)?,
                jobs: vec![job],
                spool_ids: Vec::new(),
            },
            replayed: true,
        });
    }
    let allowed = job.state == JobState::OutcomeUnknown
        || super::state::may_declare_while_unreachable(&job, at, declare_after);
    if !allowed {
        return Err(RepositoryError::JobActionNotAllowed {
            job_id: job.id,
            action: JobAction::DeclareOutcome,
            state: job.state,
        });
    }
    let now = crate::host_ops::format_time(at);
    let (event, change) = match outcome {
        DeclaredOutcome::Completed => (
            JobEventKind::DeclaredCompleted,
            JobChange {
                settlement_method: Some(SettlementMethod::Estimated),
                ..JobChange::default()
            },
        ),
        DeclaredOutcome::Failed => (JobEventKind::DeclaredFailed, JobChange::default()),
        DeclaredOutcome::Cancelled => (
            JobEventKind::DeclaredCancelled,
            JobChange {
                cancel_reason: Some(CancelReason::OperatorDeclared),
                ..JobChange::default()
            },
        ),
    };
    let mut change = end_job(
        tx,
        job_id,
        event,
        JobChange {
            operation_id: Some(operation_id.to_string()),
            detail: Some(serde_json::json!({ "outcome": encode_enum(outcome) })),
            ..change
        },
        &now,
    )?;
    let resolution = serde_json::to_value(RequirementResolution::Declared { outcome })
        .expect("a resolution always serializes");
    for requirement in jobs_repository::requirements_for_job(tx, job_id)? {
        if requirement.kind == RequirementKind::JobOutcomeUnknown
            && requirement.status != RequirementStatus::Resolved
        {
            change
                .requirements
                .push(jobs_repository::set_requirement_status(
                    tx,
                    &requirement.id,
                    RequirementStatus::Resolved,
                    Some(&resolution),
                    &now,
                )?);
        }
    }
    Ok(Declared {
        change,
        replayed: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use TrackerVerdict::*;

    fn status(state: OperationalState, file: Option<&str>, progress: Option<f64>) -> PrinterStatus {
        let mut status = PrinterStatus::new(ConnectionState::Online);
        status.operational_state = state;
        status.freshness = TelemetryFreshness::Fresh;
        status.telemetry.job_name = file.map(str::to_string);
        status.telemetry.progress = progress;
        status
    }

    #[test]
    fn a_cancel_seen_in_history_is_the_operators_unless_the_cancel_failed() {
        use HostOperationState as Op;
        assert_eq!(cancel_reason(&[]), CancelReason::HostCancelled);
        assert_eq!(cancel_reason(&[Op::Failed]), CancelReason::HostCancelled);
        for state in [
            Op::Succeeded,
            Op::Dispatching,
            Op::Uncertain,
            Op::Reconciling,
            Op::Abandoned,
        ] {
            assert_eq!(
                cancel_reason(&[state]),
                CancelReason::CancelledByOperator,
                "{state:?}"
            );
            assert_eq!(
                cancel_reason(&[Op::Failed, state]),
                CancelReason::CancelledByOperator,
                "a failed cancel then {state:?}"
            );
        }
    }

    #[test]
    fn progress_grows_by_whole_percents_only_on_our_file() {
        use OperationalState::Printing;
        let hint = |max, file, progress| {
            status_hint(
                JobState::Printing,
                OURS,
                max,
                &status(Printing, file, progress),
            )
            .progress_pct
        };
        assert_eq!(hint(0, Some(OURS), Some(0.379)), Some(37));
        assert_eq!(
            hint(37, Some(OURS), Some(0.379)),
            None,
            "no whole-percent growth"
        );
        assert_eq!(hint(40, Some(OURS), Some(0.2)), None, "never goes down");
        assert_eq!(hint(99, Some(OURS), Some(1.0)), Some(100));
        assert_eq!(hint(0, Some(OURS), Some(1.7)), Some(100), "clamped");
        assert_eq!(hint(0, Some(OURS), Some(-0.5)), None);
        assert_eq!(hint(0, Some("someone/else.gcode"), Some(0.5)), None);
        assert_eq!(hint(0, None, Some(0.5)), None);
        assert_eq!(hint(0, Some(OURS), None), None);
        let mut offline = status(Printing, Some(OURS), Some(0.5));
        offline.connection_state = ConnectionState::Offline;
        assert_eq!(
            status_hint(JobState::Printing, OURS, 0, &offline),
            StatusHint::default()
        );
    }

    /// Owner decision 5: the progress is the highest the host reported for
    /// this Job. A host that finished, cancelled, or failed the previous
    /// run of the same file keeps reporting that file and its progress
    /// until the next print begins, so only a print in progress counts.
    #[test]
    fn progress_counts_only_while_our_file_is_printing_or_paused() {
        use OperationalState::*;
        let hint = |state| {
            status_hint(
                JobState::Printing,
                OURS,
                0,
                &status(state, Some(OURS), Some(1.0)),
            )
            .progress_pct
        };
        assert_eq!(hint(Printing), Some(100));
        assert_eq!(hint(Paused), Some(100));
        for state in [Finished, Cancelled, Failed, Ready, Busy] {
            assert_eq!(hint(state), None, "{state:?}");
        }
    }

    #[test]
    fn paused_and_printing_on_our_file_are_followed() {
        use OperationalState::{Paused, Printing, Ready};
        let follow = |job_state, state, file| {
            status_hint(job_state, OURS, 0, &status(state, file, None)).follow
        };
        assert_eq!(
            follow(JobState::Printing, Paused, Some(OURS)),
            Some(JobEventKind::Paused)
        );
        assert_eq!(
            follow(JobState::Paused, Printing, Some(OURS)),
            Some(JobEventKind::Resumed)
        );
        assert_eq!(follow(JobState::Printing, Printing, Some(OURS)), None);
        assert_eq!(follow(JobState::Paused, Paused, Some(OURS)), None);
        assert_eq!(
            follow(JobState::Printing, Paused, Some("someone/else.gcode")),
            None
        );
        assert_eq!(follow(JobState::Printing, Paused, None), None);
        assert_eq!(follow(JobState::Paused, Ready, Some(OURS)), None);
        let mut stale = status(Paused, Some(OURS), None);
        stale.freshness = TelemetryFreshness::Stale;
        assert_eq!(
            status_hint(JobState::Printing, OURS, 0, &stale).follow,
            None
        );
    }

    /// A status is a hint that triggers a check, never an end.
    #[test]
    fn an_ended_status_on_our_file_asks_for_a_history_check_now() {
        use OperationalState::*;
        for state in [Finished, Cancelled, Failed] {
            assert!(
                status_hint(
                    JobState::Printing,
                    OURS,
                    0,
                    &status(state, Some(OURS), None)
                )
                .check_now
            );
            assert!(
                !status_hint(
                    JobState::Paused,
                    OURS,
                    0,
                    &status(state, Some("x.gcode"), None)
                )
                .check_now
            );
            assert!(
                !status_hint(JobState::Printing, OURS, 0, &status(state, None, None)).check_now
            );
        }
        for state in [Printing, Paused, Ready, Busy, Error] {
            assert!(
                !status_hint(
                    JobState::Printing,
                    OURS,
                    0,
                    &status(state, Some(OURS), None)
                )
                .check_now
            );
        }
    }

    #[test]
    fn only_a_print_in_progress_of_our_file_reports_our_file() {
        use OperationalState::*;
        assert!(reports_print_of(
            Some(&status(Printing, Some(OURS), None)),
            OURS
        ));
        assert!(reports_print_of(
            Some(&status(Paused, Some(OURS), None)),
            OURS
        ));
        for state in [Finished, Cancelled, Failed, Ready, Busy] {
            assert!(
                !reports_print_of(Some(&status(state, Some(OURS), None)), OURS),
                "{state:?}"
            );
        }
        assert!(!reports_print_of(
            Some(&status(Printing, Some("x.gcode"), None)),
            OURS
        ));
        assert!(!reports_print_of(Some(&status(Printing, None, None)), OURS));
        assert!(!reports_print_of(None, OURS));
        let mut offline = status(Printing, Some(OURS), None);
        offline.connection_state = ConnectionState::Offline;
        assert!(!reports_print_of(Some(&offline), OURS));
    }

    const OURS: &str = "farm3d/slr-a.gcode";
    const MARK: u64 = 10;
    /// The start's `dispatched_at` less the skew tolerance.
    const EARLIEST: f64 = 1_000.0;

    fn job() -> TrackedJob {
        TrackedJob {
            host_path: OURS.to_string(),
            history_mark: MARK,
            host_job_id: None,
            earliest_start_epoch_s: Some(EARLIEST),
        }
    }

    fn pinned(id: u64) -> TrackedJob {
        TrackedJob {
            host_job_id: Some(id),
            ..job()
        }
    }

    fn history_job(id: u64, filename: &str, status: &str, start: f64) -> HistoryJob {
        HistoryJob {
            job_id: id,
            filename: filename.to_string(),
            status: status.to_string(),
            start_time_epoch_s: start,
        }
    }

    const ON_OUR_FILE: Observed = Observed {
        same_endpoint: true,
        reports_our_file: true,
    };
    const NOT_OUR_FILE: Observed = Observed {
        same_endpoint: true,
        reports_our_file: false,
    };

    /// D7's table, one row per history status, for a pinned Job.
    #[test]
    fn a_pinned_jobs_status_maps_as_the_verdict_table_says() {
        let table = [
            ("in_progress", StillRunning),
            ("completed", Completed),
            ("cancelled", Cancelled),
            ("error", Failed),
            ("klippy_shutdown", Failed),
            ("klippy_disconnect", Failed),
            ("server_exit", Failed),
            ("interrupted", Inconclusive),
            ("", Inconclusive),
            ("COMPLETED", Inconclusive),
        ];
        for (status, verdict) in table {
            let history = [history_job(12, OURS, status, 2_000.0)];
            for observed in [ON_OUR_FILE, NOT_OUR_FILE] {
                assert_eq!(
                    verdict_from_history(&pinned(12), &history, observed),
                    (Some(12), verdict),
                    "{status:?} {observed:?}"
                );
            }
        }
    }

    #[test]
    fn a_pinned_job_missing_from_the_page_is_inconclusive() {
        let history = [history_job(13, OURS, "completed", 2_000.0)];
        assert_eq!(
            verdict_from_history(&pinned(12), &history, ON_OUR_FILE),
            (Some(12), Inconclusive)
        );
        assert_eq!(
            verdict_from_history(&pinned(12), &[], ON_OUR_FILE),
            (Some(12), Inconclusive)
        );
    }

    /// Pinning: the lowest qualifying id above the mark, whatever the
    /// page's order, and its status is the verdict at once.
    #[test]
    fn pins_the_first_job_of_our_file_above_the_mark() {
        let history = [
            history_job(14, OURS, "in_progress", 2_100.0),
            history_job(11, OURS, "completed", 2_000.0),
            history_job(MARK, OURS, "completed", 1_500.0),
        ];
        assert_eq!(
            verdict_from_history(&job(), &history, NOT_OUR_FILE),
            (Some(11), Completed)
        );
    }

    #[test]
    fn never_pins_an_earlier_print_of_the_same_file() {
        // At or below the mark: a print from before this start.
        let history = [
            history_job(MARK, OURS, "completed", 2_000.0),
            history_job(3, OURS, "in_progress", 2_000.0),
        ];
        assert_eq!(
            verdict_from_history(&job(), &history, NOT_OUR_FILE),
            (None, Inconclusive)
        );
        assert_eq!(
            verdict_from_history(&job(), &history, ON_OUR_FILE),
            (None, StillRunning)
        );
    }

    /// P6's start rule (b): `start_time ≥ dispatched_at − 30 s`, inclusive.
    #[test]
    fn pin_requires_the_start_time_bound() {
        let too_early = [history_job(11, OURS, "completed", EARLIEST - 0.001)];
        assert_eq!(
            verdict_from_history(&job(), &too_early, NOT_OUR_FILE),
            (None, Inconclusive)
        );
        let at_bound = [history_job(11, OURS, "completed", EARLIEST)];
        assert_eq!(
            verdict_from_history(&job(), &at_bound, NOT_OUR_FILE),
            (Some(11), Completed)
        );
        // A too-early job is skipped, and a later one still pins.
        let both = [
            history_job(11, OURS, "cancelled", EARLIEST - 5.0),
            history_job(12, OURS, "in_progress", EARLIEST + 5.0),
        ];
        assert_eq!(
            verdict_from_history(&job(), &both, NOT_OUR_FILE),
            (Some(12), StillRunning)
        );
        // A start that was never sent never pins.
        let unsent = TrackedJob {
            earliest_start_epoch_s: None,
            ..job()
        };
        assert_eq!(
            verdict_from_history(&unsent, &at_bound, NOT_OUR_FILE),
            (None, Inconclusive)
        );
    }

    /// Decision 9: a foreign print is never proof, whatever its status.
    #[test]
    fn never_adopts_another_file() {
        for status in ["completed", "cancelled", "error", "in_progress"] {
            let history = [history_job(11, "someone/else.gcode", status, 2_000.0)];
            assert_eq!(
                verdict_from_history(&job(), &history, NOT_OUR_FILE),
                (None, Inconclusive),
                "{status}"
            );
        }
    }

    /// The last two rows: unpinned, nothing qualifies yet.
    #[test]
    fn unpinned_without_a_qualifying_job_follows_what_the_printer_reports() {
        assert_eq!(
            verdict_from_history(&job(), &[], ON_OUR_FILE),
            (None, StillRunning)
        );
        assert_eq!(
            verdict_from_history(&job(), &[], NOT_OUR_FILE),
            (None, Inconclusive)
        );
        let foreign = [history_job(
            11,
            "someone/else.gcode",
            "in_progress",
            2_000.0,
        )];
        assert_eq!(
            verdict_from_history(&job(), &foreign, ON_OUR_FILE),
            (None, StillRunning)
        );
    }

    /// D7 "Endpoint": history read from another host is never proof. It
    /// never pins, never uses an existing pin, and never ends the Job.
    #[test]
    fn history_from_another_endpoint_is_always_inconclusive() {
        let moved = |reports_our_file| Observed {
            same_endpoint: false,
            reports_our_file,
        };
        let same_named = [history_job(11, OURS, "completed", 2_000.0)];
        for reports in [true, false] {
            assert_eq!(
                verdict_from_history(&job(), &same_named, moved(reports)),
                (None, Inconclusive)
            );
            assert_eq!(
                verdict_from_history(&pinned(11), &same_named, moved(reports)),
                (None, Inconclusive)
            );
        }
    }

    /// D7 "Unprovable outcome": each inconclusive poll adds one, a
    /// conclusive one resets to 0, and the limit makes the Job
    /// `outcomeUnknown`.
    #[test]
    fn inconclusive_checks_count_up_to_the_limit_and_reset_on_a_conclusive_poll() {
        assert_eq!(next_inconclusive_checks(Inconclusive, 0, 3), (1, false));
        assert_eq!(next_inconclusive_checks(Inconclusive, 1, 3), (2, false));
        assert_eq!(next_inconclusive_checks(Inconclusive, 2, 3), (3, true));
        assert_eq!(next_inconclusive_checks(Inconclusive, 7, 3), (8, true));
        for conclusive in [StillRunning, Completed, Failed, Cancelled] {
            assert_eq!(
                next_inconclusive_checks(conclusive, 2, 3),
                (0, false),
                "{conclusive:?}"
            );
            assert_eq!(
                next_inconclusive_checks(conclusive, 0, 3),
                (0, false),
                "{conclusive:?}"
            );
        }
        // A limit of 1 ends at the first inconclusive poll.
        assert_eq!(next_inconclusive_checks(Inconclusive, 0, 1), (1, true));
    }
}
