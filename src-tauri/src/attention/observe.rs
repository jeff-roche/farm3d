//! P8 D2 "The projection": the pure Condition observer. `observe` turns a
//! [`FarmView`] (what is true now: P1 status, P7 Jobs and Reconciliation
//! Requirements, P3 Spool facets) into [`ObservedConditions`], one
//! [`Observation`] per dedup key. No I/O, no clock reads (`now` is an
//! argument), no database.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, SecondsFormat, Utc};

use super::{
    dedup_key, AttentionDetail, AttentionSubject, Condition, ConditionKind,
    MaterialReconciliationStatus, PrinterConnectionErrorCause,
};
use crate::connections::{
    ConnectionErrorCause, ConnectionState, PrinterStatus, PrinterStatusFacts,
};
use crate::jobs::{
    CancelReason, JobEventKind, JobState, ReconciliationRequirement, RequirementKind,
    RequirementStatus,
};
use crate::printers::alerts::AlertDefaults;
use crate::printers::operational::OperationalState;
use crate::printers::StartSafety;
use crate::spools::SpoolLifecycle;

/// D2 "Reachability": a Printer's status reduced to what Attention needs.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Reach {
    Reachable,
    Unreachable,
    Misconfigured(PrinterConnectionErrorCause),
    Unknown,
}

/// D2 "Reachability" table. `cause` is only read for an `error` status.
pub fn reach(status: &PrinterStatus, cause: Option<ConnectionErrorCause>) -> Reach {
    match status.connection_state {
        ConnectionState::Online => Reach::Reachable,
        ConnectionState::Offline | ConnectionState::Connecting => Reach::Unreachable,
        ConnectionState::Error => match cause {
            Some(ConnectionErrorCause::Unreachable | ConnectionErrorCause::Timeout) => {
                Reach::Unreachable
            }
            Some(ConnectionErrorCause::Auth) => {
                Reach::Misconfigured(PrinterConnectionErrorCause::Auth)
            }
            Some(ConnectionErrorCause::Protocol) => {
                Reach::Misconfigured(PrinterConnectionErrorCause::Protocol)
            }
            None => Reach::Unknown,
        },
    }
}

/// The live part of a Printer's facts, from the status map. `None` on
/// [`PrinterFacts::status`] means no status is in the map (unknown).
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct StatusFacts {
    pub reach: Reach,
    pub operational_state: OperationalState,
    /// The host's reported file (`telemetry.job_name`).
    pub reported_file: Option<String>,
}

impl StatusFacts {
    /// Reduces one status-map entry.
    pub fn from_facts(facts: &PrinterStatusFacts) -> Self {
        StatusFacts {
            reach: reach(&facts.status, facts.cause),
            operational_state: facts.status.operational_state,
            reported_file: facts.status.telemetry.job_name.clone(),
        }
    }
}

/// A Printer's latest Job: its Job with the greatest `(created_at, id)`.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LatestJob {
    pub id: String,
    pub host_path: Option<String>,
}

#[derive(Clone, PartialEq, Debug)]
pub struct PrinterFacts {
    pub id: String,
    pub name: String,
    pub location: Option<String>,
    pub archived: bool,
    pub setup_incomplete: bool,
    pub start_safety: StartSafety,
    /// `None`: no status in the map (unknown).
    pub status: Option<StatusFacts>,
    /// From the projector's [`PrinterWatch`].
    pub unreachable_since: Option<DateTime<Utc>>,
    pub alert_defaults: AlertDefaults,
    pub active_job_id: Option<String>,
    pub latest_job: Option<LatestJob>,
}

impl PrinterFacts {
    /// D2: "not applicable" means archived or Setup incomplete.
    pub fn applicable(&self) -> bool {
        !self.archived && !self.setup_incomplete
    }
}

/// D2 "Ended by": who ended a terminal Job, from its terminal
/// `job_events` row.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum JobEndedBy {
    /// `completed`, `failed`, `cancelled`: the tracker proved it.
    Tracker,
    /// `declaredCompleted`, `declaredFailed`, `declaredCancelled`.
    Declared,
    /// `released`, `cancelledBeforeStart`.
    Operator,
}

impl JobEndedBy {
    /// Maps a Job's terminal event kind; `None` for a non-terminal kind.
    pub fn from_terminal_event(kind: JobEventKind) -> Option<Self> {
        match kind {
            JobEventKind::Completed | JobEventKind::Failed | JobEventKind::Cancelled => {
                Some(JobEndedBy::Tracker)
            }
            JobEventKind::DeclaredCompleted
            | JobEventKind::DeclaredFailed
            | JobEventKind::DeclaredCancelled => Some(JobEndedBy::Declared),
            JobEventKind::Released | JobEventKind::CancelledBeforeStart => {
                Some(JobEndedBy::Operator)
            }
            _ => None,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct JobFacts {
    pub id: String,
    pub printer_id: String,
    pub state: JobState,
    pub cancel_reason: Option<CancelReason>,
    /// Set for a terminal Job.
    pub ended_by: Option<JobEndedBy>,
    pub ended_at: Option<DateTime<Utc>>,
    pub host_path: Option<String>,
    /// farm3d started this Job: it is `starting`, `printing`, `paused`, or
    /// `outcomeUnknown`, or its `started_at` is set. A per-Job fact, not
    /// [`FarmView::supervisors_started_at`]. `observe` doesn't read it:
    /// coverage goes by the Job's state (see `covered_by_a_job`).
    pub started: bool,
    pub spool_id: String,
    pub has_last_failure: bool,
    /// The Queue Entry's `display.modelName`, plus " — <plateLabel>".
    pub label: String,
}

#[derive(Clone, PartialEq, Debug)]
pub struct SpoolFacts {
    pub id: String,
    pub number: i64,
    /// "<manufacturer> <product or material>".
    pub label: String,
    pub lifecycle: SpoolLifecycle,
    /// P3's `low` facet (always false for an `empty` or `archived` Spool).
    pub low: bool,
    pub current_mg: i64,
    pub low_threshold_mg: i64,
    /// The Printer whose Material Slot holds it, if any.
    pub loaded_on: Option<String>,
}

/// D2: everything `observe` reads.
#[derive(Clone, PartialEq, Debug)]
pub struct FarmView {
    pub printers: Vec<PrinterFacts>,
    pub jobs: Vec<JobFacts>,
    pub requirements: Vec<ReconciliationRequirement>,
    pub spools: Vec<SpoolFacts>,
    /// When the attention runtime started the live passes. `None` during
    /// the startup backfill (every live-status family is then unknown).
    pub supervisors_started_at: Option<DateTime<Utc>>,
    /// Migration 0009's `applied_at`.
    pub attention_epoch: DateTime<Utc>,
}

/// D2 "The offline watch": each Printer's `unreachable_since`, in memory
/// only (the projector's).
#[derive(Clone, PartialEq, Eq, Debug, Default)]
pub struct PrinterWatch {
    unreachable_since: HashMap<String, DateTime<Utc>>,
}

impl PrinterWatch {
    pub fn unreachable_since(&self, printer_id: &str) -> Option<DateTime<Utc>> {
        self.unreachable_since.get(printer_id).copied()
    }
}

/// D2 "The offline watch": runs at the start of every pass, before the
/// view is built. `Unreachable` with no watch sets it to `now`;
/// `Reachable`, `Misconfigured`, or a Printer that is archived or Setup
/// incomplete clears it; `Unknown` (including no status in the map)
/// leaves it. A Printer no longer in `printers` is forgotten.
pub fn update_watch(
    watch: &mut PrinterWatch,
    printers: &[PrinterFacts],
    statuses: &HashMap<String, PrinterStatusFacts>,
    now: DateTime<Utc>,
) {
    watch
        .unreachable_since
        .retain(|id, _| printers.iter().any(|p| &p.id == id));
    for printer in printers {
        if !printer.applicable() {
            watch.unreachable_since.remove(&printer.id);
            continue;
        }
        let Some(facts) = statuses.get(&printer.id) else {
            continue;
        };
        match reach(&facts.status, facts.cause) {
            Reach::Unreachable => {
                watch
                    .unreachable_since
                    .entry(printer.id.clone())
                    .or_insert(now);
            }
            Reach::Reachable | Reach::Misconfigured(_) => {
                watch.unreachable_since.remove(&printer.id);
            }
            Reach::Unknown => {}
        }
    }
}

/// One dedup key's observation. Short-lived (one pass), so the
/// `Condition` stays unboxed.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, PartialEq, Debug)]
pub enum Observation {
    Present(Condition),
    Absent,
    Unknown,
}

/// Dedup key → observation, walked in key order by the planner. A key
/// the view doesn't mention is treated as `Unknown`.
pub type ObservedConditions = BTreeMap<String, Observation>;

/// D2 "Observation rules": for every source in the view, one entry per
/// Condition that source can have (three per Printer, four per Job, one
/// per requirement and per Spool).
///
/// Precedence within each row: a durable fact that makes a Condition
/// `Absent` (an archived or Setup-incomplete Printer, offline alerts off,
/// a Job that isn't completed by the tracker, isn't the latest, or ended
/// before the epoch) wins over the backfill's `Unknown`: the durable fact
/// is known before any live status is. Every live-status judgement is
/// `Unknown` during the backfill (`supervisors_started_at: None`).
pub fn observe(view: &FarmView, now: DateTime<Utc>) -> ObservedConditions {
    let ctx = Context { view, now };
    let mut observed = ObservedConditions::new();
    for printer in &view.printers {
        for (kind, observation) in [
            (ConditionKind::PrinterOffline, ctx.printer_offline(printer)),
            (
                ConditionKind::PrinterConnectionError,
                ctx.printer_connection_error(printer),
            ),
            (
                ConditionKind::PrinterHostFailed,
                ctx.printer_host_failed(printer),
            ),
        ] {
            observed.insert(dedup_key(kind, &printer.id), observation);
        }
    }
    for job in &view.jobs {
        for (kind, observation) in [
            (
                ConditionKind::JobStartConfirmation,
                ctx.job_start_confirmation(job),
            ),
            (ConditionKind::JobFailed, ctx.job_failed(job)),
            (ConditionKind::JobHostCancelled, ctx.job_host_cancelled(job)),
            (ConditionKind::JobCompleted, ctx.job_completed(job)),
        ] {
            observed.insert(dedup_key(kind, &job.id), observation);
        }
    }
    for requirement in &view.requirements {
        let (kind, observation) = ctx.requirement(requirement);
        observed.insert(dedup_key(kind, &requirement.id), observation);
    }
    for spool in &view.spools {
        observed.insert(
            dedup_key(ConditionKind::SpoolLow, &spool.id),
            ctx.spool_low(spool),
        );
    }
    observed
}

/// D2 "Runtime and wakes": the earliest `max(unreachable_since,
/// supervisors_started_at) + grace` still in the future, over every
/// applicable, unreachable Printer with offline alerts on. `None` during
/// the backfill.
pub fn next_deadline(view: &FarmView, now: DateTime<Utc>) -> Option<DateTime<Utc>> {
    let supervisors_started_at = view.supervisors_started_at?;
    view.printers
        .iter()
        .filter(|p| p.applicable())
        .filter(|p| {
            matches!(
                p.status,
                Some(StatusFacts {
                    reach: Reach::Unreachable,
                    ..
                })
            )
        })
        .filter_map(|p| offline_deadline(p, supervisors_started_at))
        .filter(|deadline| *deadline > now)
        .min()
}

/// `max(unreachable_since, supervisors_started_at) + grace`, or `None`
/// when offline alerts are off or the watch isn't set.
fn offline_deadline(
    printer: &PrinterFacts,
    supervisors_started_at: DateTime<Utc>,
) -> Option<DateTime<Utc>> {
    let grace = printer.alert_defaults.offline_after_minutes?.grace();
    let since = printer.unreachable_since?;
    Some(since.max(supervisors_started_at) + grace)
}

/// `Present(condition)`, checking (in debug builds) that the detail is
/// the Condition's own variant.
fn present(condition: Condition) -> Observation {
    debug_assert_eq!(
        condition.detail.condition_kind(),
        condition.kind,
        "a Condition's detail must be its own kind's variant"
    );
    Observation::Present(condition)
}

fn rfc3339(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

struct Context<'a> {
    view: &'a FarmView,
    now: DateTime<Utc>,
}

/// The live status, or `None` when it can't be judged: the backfill, or
/// no status in the map.
fn live(view: &FarmView, printer: &PrinterFacts) -> Option<StatusFacts> {
    view.supervisors_started_at?;
    printer.status.clone()
}

impl<'a> Context<'a> {
    fn printer(&self, id: &str) -> Option<&'a PrinterFacts> {
        self.view.printers.iter().find(|p| p.id == id)
    }

    fn job(&self, id: &str) -> Option<&'a JobFacts> {
        self.view.jobs.iter().find(|j| j.id == id)
    }

    fn spool(&self, id: &str) -> Option<&'a SpoolFacts> {
        self.view.spools.iter().find(|s| s.id == id)
    }

    fn after_epoch(&self, ended_at: Option<DateTime<Utc>>) -> bool {
        ended_at.is_some_and(|at| at >= self.view.attention_epoch)
    }

    fn printer_condition(
        &self,
        kind: ConditionKind,
        printer: &PrinterFacts,
        detail: AttentionDetail,
    ) -> Observation {
        present(Condition {
            kind,
            source_id: printer.id.clone(),
            printer_id: Some(printer.id.clone()),
            job_id: None,
            spool_id: None,
            requirement_id: None,
            subject: AttentionSubject {
                printer_name: Some(printer.name.clone()),
                printer_location: printer.location.clone(),
                job_label: None,
                spool_number: None,
                spool_label: None,
            },
            detail,
            acknowledge: false,
        })
    }

    /// The subject for a Condition about a Job (or its requirement):
    /// the Job's Printer, its label, and its Spool.
    fn job_subject(&self, job: Option<&JobFacts>, spool_id: Option<&str>) -> AttentionSubject {
        let printer = job.and_then(|j| self.printer(&j.printer_id));
        let spool = spool_id.and_then(|id| self.spool(id));
        AttentionSubject {
            printer_name: printer.map(|p| p.name.clone()),
            printer_location: printer.and_then(|p| p.location.clone()),
            job_label: job.map(|j| j.label.clone()),
            spool_number: spool.map(|s| s.number),
            spool_label: spool.map(|s| s.label.clone()),
        }
    }

    fn job_condition(
        &self,
        kind: ConditionKind,
        job: &JobFacts,
        detail: AttentionDetail,
    ) -> Observation {
        present(Condition {
            kind,
            source_id: job.id.clone(),
            printer_id: Some(job.printer_id.clone()),
            job_id: Some(job.id.clone()),
            spool_id: None,
            requirement_id: None,
            subject: self.job_subject(Some(job), Some(&job.spool_id)),
            detail,
            acknowledge: false,
        })
    }

    fn printer_offline(&self, printer: &PrinterFacts) -> Observation {
        if !printer.applicable() {
            return Observation::Absent;
        }
        let Some(grace) = printer.alert_defaults.offline_after_minutes else {
            return Observation::Absent;
        };
        let (Some(supervisors_started_at), Some(status)) =
            (self.view.supervisors_started_at, live(self.view, printer))
        else {
            return Observation::Unknown;
        };
        match status.reach {
            Reach::Reachable | Reach::Misconfigured(_) => Observation::Absent,
            Reach::Unknown => Observation::Unknown,
            Reach::Unreachable => match printer.unreachable_since {
                Some(since) if self.now >= since.max(supervisors_started_at) + grace.grace() => {
                    self.printer_condition(
                        ConditionKind::PrinterOffline,
                        printer,
                        AttentionDetail::PrinterOffline {
                            unreachable_since: rfc3339(since),
                        },
                    )
                }
                // Within the grace (or the watch not yet set).
                _ => Observation::Unknown,
            },
        }
    }

    fn printer_connection_error(&self, printer: &PrinterFacts) -> Observation {
        if !printer.applicable() {
            return Observation::Absent;
        }
        let Some(status) = live(self.view, printer) else {
            return Observation::Unknown;
        };
        match status.reach {
            Reach::Reachable => Observation::Absent,
            Reach::Misconfigured(cause) => self.printer_condition(
                ConditionKind::PrinterConnectionError,
                printer,
                AttentionDetail::PrinterConnectionError { cause },
            ),
            // Offline, connecting, hydrated, or an unreachable/timeout
            // error: none shows whether the credentials are still wrong.
            Reach::Unreachable | Reach::Unknown => Observation::Unknown,
        }
    }

    fn printer_host_failed(&self, printer: &PrinterFacts) -> Observation {
        if !printer.applicable() {
            return Observation::Absent;
        }
        let Some(status) = live(self.view, printer) else {
            return Observation::Unknown;
        };
        if status.reach != Reach::Reachable {
            return Observation::Unknown;
        }
        match status.operational_state {
            OperationalState::Unknown => Observation::Unknown,
            OperationalState::Failed if !self.covered_by_a_job(printer, &status) => self
                .printer_condition(
                    ConditionKind::PrinterHostFailed,
                    printer,
                    AttentionDetail::PrinterHostFailed,
                ),
            // Not failed, or failed and covered by a Job.
            _ => Observation::Absent,
        }
    }

    /// D2 "Covered by a Job": the reported file is the latest Job's
    /// `hostPath`, and that Job can still carry the failure: it is
    /// `starting`, `printing`, `paused`, or `outcomeUnknown` (its
    /// `job.failed` or `requirement.jobOutcomeUnknown` is still to come),
    /// or the tracker ended it `failed` (its `job.failed` is the carrier).
    /// An assigned or staged Job, and one that ended completed, cancelled,
    /// or by a declaration, never covers a failure: nothing else would
    /// ever raise it (controller ruling, Task 4 review).
    fn covered_by_a_job(&self, printer: &PrinterFacts, status: &StatusFacts) -> bool {
        let (Some(file), Some(latest)) = (&status.reported_file, &printer.latest_job) else {
            return false;
        };
        if latest.host_path.as_ref() != Some(file) {
            return false;
        }
        self.job(&latest.id).is_some_and(|job| match job.state {
            JobState::Starting
            | JobState::Printing
            | JobState::Paused
            | JobState::OutcomeUnknown => true,
            JobState::Failed => job.ended_by == Some(JobEndedBy::Tracker),
            _ => false,
        })
    }

    fn job_start_confirmation(&self, job: &JobFacts) -> Observation {
        if job.state != JobState::AwaitingStart {
            return Observation::Absent;
        }
        let Some(printer) = self.printer(&job.printer_id) else {
            return Observation::Unknown;
        };
        if printer.start_safety != StartSafety::ConfirmBedClear && !job.has_last_failure {
            return Observation::Absent;
        }
        let loaded = self
            .spool(&job.spool_id)
            .is_some_and(|spool| spool.loaded_on.as_deref() == Some(job.printer_id.as_str()));
        self.job_condition(
            ConditionKind::JobStartConfirmation,
            job,
            AttentionDetail::JobStartConfirmation {
                awaiting_material: !loaded,
            },
        )
    }

    fn job_failed(&self, job: &JobFacts) -> Observation {
        match job.ended_at {
            Some(ended_at)
                if job.state == JobState::Failed
                    && job.ended_by == Some(JobEndedBy::Tracker)
                    && self.after_epoch(job.ended_at) =>
            {
                self.job_condition(
                    ConditionKind::JobFailed,
                    job,
                    AttentionDetail::JobFailed {
                        ended_at: rfc3339(ended_at),
                    },
                )
            }
            _ => Observation::Absent,
        }
    }

    fn job_host_cancelled(&self, job: &JobFacts) -> Observation {
        match job.ended_at {
            Some(ended_at)
                if job.state == JobState::Cancelled
                    && job.cancel_reason == Some(CancelReason::HostCancelled)
                    && self.after_epoch(job.ended_at) =>
            {
                self.job_condition(
                    ConditionKind::JobHostCancelled,
                    job,
                    AttentionDetail::JobHostCancelled {
                        ended_at: rfc3339(ended_at),
                    },
                )
            }
            _ => Observation::Absent,
        }
    }

    fn job_completed(&self, job: &JobFacts) -> Observation {
        let Some(ended_at) = job.ended_at else {
            return Observation::Absent;
        };
        // Durable facts first.
        if job.state != JobState::Completed
            || job.ended_by != Some(JobEndedBy::Tracker)
            || !self.after_epoch(job.ended_at)
        {
            return Observation::Absent;
        }
        let Some(printer) = self.printer(&job.printer_id) else {
            return Observation::Unknown;
        };
        let latest = printer.latest_job.as_ref().is_some_and(|l| l.id == job.id);
        if !latest || printer.archived {
            return Observation::Absent;
        }
        // Then the live status.
        let Some(status) = live(self.view, printer) else {
            return Observation::Unknown;
        };
        if status.reach != Reach::Reachable {
            return Observation::Unknown;
        }
        match status.operational_state {
            OperationalState::Unknown => Observation::Unknown,
            OperationalState::Finished
                if job.host_path.is_some() && status.reported_file == job.host_path =>
            {
                self.job_condition(
                    ConditionKind::JobCompleted,
                    job,
                    AttentionDetail::JobCompleted {
                        ended_at: rfc3339(ended_at),
                    },
                )
            }
            _ => Observation::Absent,
        }
    }

    /// Each requirement has exactly one Condition, by its kind.
    fn requirement(&self, requirement: &ReconciliationRequirement) -> (ConditionKind, Observation) {
        let job = self.job(&requirement.job_id);
        // The subject names the requirement's Spool, else the Job's; the
        // `spool_id` column is set only for a material requirement.
        let subject_spool = requirement
            .spool_id
            .clone()
            .or_else(|| job.map(|j| j.spool_id.clone()));
        let requirement_condition = |kind, spool_id: Option<String>, detail, acknowledge| {
            present(Condition {
                kind,
                source_id: requirement.id.clone(),
                printer_id: job.map(|j| j.printer_id.clone()),
                job_id: Some(requirement.job_id.clone()),
                spool_id,
                requirement_id: Some(requirement.id.clone()),
                subject: self.job_subject(job, subject_spool.as_deref()),
                detail,
                acknowledge,
            })
        };
        match requirement.kind {
            RequirementKind::MaterialReconciliation => {
                let kind = ConditionKind::RequirementMaterialReconciliation;
                let status = match requirement.status {
                    RequirementStatus::Pending => MaterialReconciliationStatus::Pending,
                    RequirementStatus::Deferred => MaterialReconciliationStatus::Deferred,
                    RequirementStatus::Resolved => return (kind, Observation::Absent),
                };
                // 0008's CHECK: a material requirement always names its
                // Spool; the Job's is the fallback.
                let Some(spool_id) = subject_spool.clone() else {
                    return (kind, Observation::Unknown);
                };
                let observation = requirement_condition(
                    kind,
                    Some(spool_id.clone()),
                    AttentionDetail::RequirementMaterialReconciliation {
                        requirement_status: status,
                        spool_id,
                    },
                    status == MaterialReconciliationStatus::Deferred,
                );
                (kind, observation)
            }
            RequirementKind::JobOutcomeUnknown => {
                let kind = ConditionKind::RequirementJobOutcomeUnknown;
                let observation = match requirement.status {
                    RequirementStatus::Pending => requirement_condition(
                        kind,
                        None,
                        AttentionDetail::RequirementJobOutcomeUnknown,
                        false,
                    ),
                    RequirementStatus::Resolved => Observation::Absent,
                    // 0008's CHECK forbids a deferred outcome requirement;
                    // the table has no row for it, so it never opens or
                    // resolves anything.
                    RequirementStatus::Deferred => Observation::Unknown,
                };
                (kind, observation)
            }
        }
    }

    fn spool_low(&self, spool: &SpoolFacts) -> Observation {
        if !spool.low {
            return Observation::Absent;
        }
        present(Condition {
            kind: ConditionKind::SpoolLow,
            source_id: spool.id.clone(),
            printer_id: None,
            job_id: None,
            spool_id: Some(spool.id.clone()),
            requirement_id: None,
            subject: AttentionSubject {
                printer_name: None,
                printer_location: None,
                job_label: None,
                spool_number: Some(spool.number),
                spool_label: Some(spool.label.clone()),
            },
            detail: AttentionDetail::SpoolLow {
                current_mg: spool.current_mg,
                low_threshold_mg: spool.low_threshold_mg,
            },
            acknowledge: false,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printers::alerts::OfflineAlertMinutes;

    fn t(hms: &str) -> DateTime<Utc> {
        format!("2026-09-27T{hms}Z").parse().unwrap()
    }

    fn status_of(connection: ConnectionState) -> PrinterStatus {
        PrinterStatus::new(connection)
    }

    #[test]
    fn reach_follows_the_reachability_table() {
        use ConnectionErrorCause as C;
        use ConnectionState as S;
        let causes = [
            None,
            Some(C::Unreachable),
            Some(C::Timeout),
            Some(C::Auth),
            Some(C::Protocol),
        ];
        for cause in causes {
            // The cause is only read for an `error` status.
            assert_eq!(reach(&status_of(S::Online), cause), Reach::Reachable);
            assert_eq!(reach(&status_of(S::Offline), cause), Reach::Unreachable);
            assert_eq!(reach(&status_of(S::Connecting), cause), Reach::Unreachable);
        }
        let error = status_of(S::Error);
        assert_eq!(reach(&error, Some(C::Unreachable)), Reach::Unreachable);
        assert_eq!(reach(&error, Some(C::Timeout)), Reach::Unreachable);
        assert_eq!(
            reach(&error, Some(C::Auth)),
            Reach::Misconfigured(PrinterConnectionErrorCause::Auth)
        );
        assert_eq!(
            reach(&error, Some(C::Protocol)),
            Reach::Misconfigured(PrinterConnectionErrorCause::Protocol)
        );
        assert_eq!(reach(&error, None), Reach::Unknown);
    }

    #[test]
    fn status_facts_keep_the_operational_state_and_the_reported_file() {
        let mut status = status_of(ConnectionState::Online);
        status.operational_state = OperationalState::Finished;
        status.telemetry.job_name = Some("cube.gcode".into());
        assert_eq!(
            StatusFacts::from_facts(&PrinterStatusFacts {
                status,
                cause: None
            }),
            StatusFacts {
                reach: Reach::Reachable,
                operational_state: OperationalState::Finished,
                reported_file: Some("cube.gcode".into()),
            }
        );
    }

    #[test]
    fn ended_by_maps_every_terminal_event_and_nothing_else() {
        for kind in JobEventKind::ALL {
            let expected = match kind {
                JobEventKind::Completed | JobEventKind::Failed | JobEventKind::Cancelled => {
                    Some(JobEndedBy::Tracker)
                }
                JobEventKind::DeclaredCompleted
                | JobEventKind::DeclaredFailed
                | JobEventKind::DeclaredCancelled => Some(JobEndedBy::Declared),
                JobEventKind::Released | JobEventKind::CancelledBeforeStart => {
                    Some(JobEndedBy::Operator)
                }
                _ => None,
            };
            assert_eq!(JobEndedBy::from_terminal_event(kind), expected, "{kind:?}");
        }
    }

    fn printer(id: &str) -> PrinterFacts {
        PrinterFacts {
            id: id.into(),
            name: id.into(),
            location: None,
            archived: false,
            setup_incomplete: false,
            start_safety: StartSafety::ConfirmBedClear,
            status: None,
            unreachable_since: None,
            alert_defaults: AlertDefaults::default(),
            active_job_id: None,
            latest_job: None,
        }
    }

    fn facts(
        connection: ConnectionState,
        cause: Option<ConnectionErrorCause>,
    ) -> PrinterStatusFacts {
        PrinterStatusFacts {
            status: status_of(connection),
            cause,
        }
    }

    #[test]
    fn update_watch_sets_keeps_clears_and_leaves() {
        let printers = [printer("prn-1")];
        let mut watch = PrinterWatch::default();
        let mut statuses = HashMap::new();

        // Unreachable with no watch: set to now; again later: kept.
        statuses.insert("prn-1".to_string(), facts(ConnectionState::Offline, None));
        update_watch(&mut watch, &printers, &statuses, t("11:50:00"));
        assert_eq!(watch.unreachable_since("prn-1"), Some(t("11:50:00")));
        statuses.insert(
            "prn-1".to_string(),
            facts(ConnectionState::Error, Some(ConnectionErrorCause::Timeout)),
        );
        update_watch(&mut watch, &printers, &statuses, t("11:55:00"));
        assert_eq!(watch.unreachable_since("prn-1"), Some(t("11:50:00")));

        // Unknown (an error with no cause, or no status): left.
        statuses.insert("prn-1".to_string(), facts(ConnectionState::Error, None));
        update_watch(&mut watch, &printers, &statuses, t("11:56:00"));
        assert_eq!(watch.unreachable_since("prn-1"), Some(t("11:50:00")));
        update_watch(&mut watch, &printers, &HashMap::new(), t("11:57:00"));
        assert_eq!(watch.unreachable_since("prn-1"), Some(t("11:50:00")));

        // Misconfigured: cleared.
        statuses.insert(
            "prn-1".to_string(),
            facts(ConnectionState::Error, Some(ConnectionErrorCause::Auth)),
        );
        update_watch(&mut watch, &printers, &statuses, t("11:58:00"));
        assert_eq!(watch.unreachable_since("prn-1"), None);

        // Unreachable again (connecting): set anew.
        statuses.insert(
            "prn-1".to_string(),
            facts(ConnectionState::Connecting, None),
        );
        update_watch(&mut watch, &printers, &statuses, t("11:59:00"));
        assert_eq!(watch.unreachable_since("prn-1"), Some(t("11:59:00")));

        // Reachable: cleared.
        statuses.insert("prn-1".to_string(), facts(ConnectionState::Online, None));
        update_watch(&mut watch, &printers, &statuses, t("12:00:00"));
        assert_eq!(watch.unreachable_since("prn-1"), None);
    }

    #[test]
    fn update_watch_clears_archived_and_setup_incomplete_printers_and_forgets_removed_ones() {
        let mut statuses = HashMap::new();
        for id in ["prn-1", "prn-2", "prn-3"] {
            statuses.insert(id.to_string(), facts(ConnectionState::Offline, None));
        }
        let mut watch = PrinterWatch::default();
        let all = [printer("prn-1"), printer("prn-2"), printer("prn-3")];
        update_watch(&mut watch, &all, &statuses, t("11:00:00"));
        for id in ["prn-1", "prn-2", "prn-3"] {
            assert_eq!(watch.unreachable_since(id), Some(t("11:00:00")));
        }
        let mut archived = printer("prn-1");
        archived.archived = true;
        let mut incomplete = printer("prn-2");
        incomplete.setup_incomplete = true;
        update_watch(
            &mut watch,
            &[archived, incomplete],
            &statuses,
            t("11:01:00"),
        );
        assert_eq!(watch.unreachable_since("prn-1"), None);
        assert_eq!(watch.unreachable_since("prn-2"), None);
        assert_eq!(watch.unreachable_since("prn-3"), None, "prn-3 was removed");
    }

    fn job(id: &str, printer_id: &str) -> JobFacts {
        JobFacts {
            id: id.into(),
            printer_id: printer_id.into(),
            state: JobState::Printing,
            cancel_reason: None,
            ended_by: None,
            ended_at: None,
            host_path: None,
            started: true,
            spool_id: "spl-1".into(),
            has_last_failure: false,
            label: "Cube".into(),
        }
    }

    fn view() -> FarmView {
        let requirement = |id: &str, kind| ReconciliationRequirement {
            id: id.into(),
            job_id: "job-1".into(),
            kind,
            status: RequirementStatus::Pending,
            spool_id: Some("spl-1".into()),
            reservation_id: Some("rsv-1".into()),
            opened_at: "2026-09-27T11:00:00Z".into(),
            deferred_at: None,
            resolved_at: None,
            resolution: None,
        };
        FarmView {
            printers: vec![printer("prn-1"), printer("prn-2")],
            jobs: vec![job("job-1", "prn-1")],
            requirements: vec![
                requirement("rrq-1", RequirementKind::MaterialReconciliation),
                requirement("rrq-2", RequirementKind::JobOutcomeUnknown),
            ],
            spools: vec![SpoolFacts {
                id: "spl-1".into(),
                number: 1,
                label: "Polymaker PLA".into(),
                lifecycle: SpoolLifecycle::Active,
                low: false,
                current_mg: 500_000,
                low_threshold_mg: 100_000,
                loaded_on: None,
            }],
            supervisors_started_at: Some(t("11:00:00")),
            attention_epoch: t("10:00:00"),
        }
    }

    #[test]
    fn observe_emits_one_entry_per_condition_each_source_can_have() {
        let keys: Vec<String> = observe(&view(), t("12:00:00")).into_keys().collect();
        let mut expected = vec![
            "printer.offline:printer:prn-1",
            "printer.connectionError:printer:prn-1",
            "printer.hostFailed:printer:prn-1",
            "printer.offline:printer:prn-2",
            "printer.connectionError:printer:prn-2",
            "printer.hostFailed:printer:prn-2",
            "job.startConfirmation:job:job-1",
            "job.failed:job:job-1",
            "job.hostCancelled:job:job-1",
            "job.completed:job:job-1",
            "requirement.materialReconciliation:reconciliationRequirement:rrq-1",
            "requirement.jobOutcomeUnknown:reconciliationRequirement:rrq-2",
            "spool.low:spool:spl-1",
        ];
        expected.sort();
        assert_eq!(keys, expected);
    }

    #[test]
    fn offline_detail_is_the_watch_value_not_the_startup_instant() {
        let mut view = view();
        view.printers[0].status = Some(StatusFacts::from_facts(&facts(
            ConnectionState::Offline,
            None,
        )));
        view.printers[0].unreachable_since = Some(t("10:30:00"));
        view.printers[0].alert_defaults.offline_after_minutes = Some(OfflineAlertMinutes::One);
        let observed = observe(&view, t("11:01:00"));
        match &observed["printer.offline:printer:prn-1"] {
            Observation::Present(condition) => assert_eq!(
                condition.detail,
                AttentionDetail::PrinterOffline {
                    unreachable_since: "2026-09-27T10:30:00Z".into()
                }
            ),
            other => panic!("expected Present, got {other:?}"),
        }
        // One second earlier the grace since startup (11:00 + 1 min) hasn't passed.
        assert_eq!(
            observe(&view, t("11:00:59"))["printer.offline:printer:prn-1"],
            Observation::Unknown
        );
    }
}
