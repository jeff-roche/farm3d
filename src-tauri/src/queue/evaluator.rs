//! P7 D6: the automatic evaluator.
//!
//! **One tokio task, one coalescing trigger channel, one run at a time.**
//! Every source of change pokes the channel ([`EvaluatorHandle::poke`]):
//! the Queue and Job commands, the dispatch driver when a Job ends, the
//! Printer commands, and pumps over the host-ops change broadcast, the
//! Connection status broadcast, the inventory broadcast, and the host-facts
//! broadcast. The channel holds one trigger, so a poke while a run is
//! pending is dropped: that run will see the change. The task drains the
//! channel, then runs [`run_once`], then publishes. Runs never overlap,
//! because only this task runs them.
//!
//! **First run** (D4 "Recovery order"): after `jobs::recover_after_restart`
//! (which runs before the runtime exists), the dispatch driver's first
//! pass, and the host-ops startup pass. Until a run finishes, `list_queue`
//! computes summaries itself and reports `evaluatorNotRunning` (ruling R3);
//! afterwards it serves [`Evaluator::last_run`].
//!
//! **A run** is a pure pass plus an async loop (spec decision 22):
//! [`evaluate_pass`] evaluates every `queued` entry top to bottom and
//! proposes the first Automatic entry with a candidate. [`run_once`] takes
//! that Printer's lock and calls the same `jobs::assign::assign` as the
//! operator's `assign_queue_entry`, in its own `write_repo`, with an
//! `auto-<uuid>` operation id, `mode = Automatic`, and `assigned_by =
//! automatic`. The in-transaction re-check there is the backstop against a
//! concurrent operator assignment. Then it re-reads and loops, until a pass
//! proposes nothing. The lock is held only around that local transaction
//! and its publish, never across `host_ops::api` or the network: the driver
//! stages the new Job afterwards, from its own task.
//!
//! **No automatic retry** (D6): the evaluator only ever assigns `queued`
//! entries. It never re-queues a failed Job, re-stages, or re-starts.

use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{broadcast, mpsc, watch};

use crate::connections::supervisor::ConnectionManager;
use crate::connections::PrinterStatus;
use crate::contracts::command::RecoveryCode;
use crate::jobs::assign::{self, AssignRequest};
use crate::jobs::{AssignedBy, Job};
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;
use crate::slicing::facts::SliceFacts;
use crate::spools::reservations::ReservationError;
use crate::RuntimeServices;

use super::eligibility::{self, AssignMode, EligibilityInput};
use super::repository;
use super::world::{facts_for, LiveWorld, World};
use super::{
    Blocker, BlockerCode, DispatchPolicy, EligibilitySummary, EligibilityVerdict,
    NextAutomaticAction, QueueEntry, QueueEntryState,
};

/// D6: what woke the evaluator. Triggers coalesce, so the variant says why
/// a run was asked for; every run re-reads everything regardless.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Trigger {
    /// The first run.
    Startup,
    /// An entry was created, updated, moved, or removed.
    QueueChanged,
    /// A Job reached a terminal state, was released, or was cancelled
    /// before start.
    JobChanged,
    /// `host_ops` `subscribe_changes`: an unresolved row resolving frees a
    /// Printer.
    HostOperationChanged,
    /// `ConnectionManager::subscribe_status`, when a schedulability fact
    /// (Connection state, operational state, freshness) changed.
    StatusChanged(String),
    /// `RuntimeServices.inventory_changes`: loads, moves, amounts,
    /// settlements.
    InventoryChanged,
    /// A Printer was created, edited, archived, unarchived, or deleted.
    PrinterChanged,
    /// A Printer's host facts were refreshed.
    CapabilitiesChanged,
}

/// The sending half of the trigger channel.
#[derive(Clone)]
pub struct EvaluatorHandle {
    tx: mpsc::Sender<Trigger>,
    /// Triggers the channel took (not the ones dropped as coalesced).
    accepted: Arc<AtomicU64>,
}

impl EvaluatorHandle {
    /// Asks for a run. Never blocks: when a run is already pending, the
    /// trigger is dropped, because that run will see this change too.
    pub fn poke(&self, trigger: Trigger) {
        if self.tx.try_send(trigger).is_ok() {
            self.accepted.fetch_add(1, Ordering::SeqCst);
        }
    }
}

/// One `queued` entry and its Slice facts, as a pass reads them.
pub struct PassEntry {
    pub entry: QueueEntry,
    pub facts: SliceFacts,
}

/// [`evaluate_pass`]'s input: the `World` one read transaction saw, the
/// `queued` entries in position order, and the time stamped on the
/// summaries.
pub struct PassInput<'a> {
    pub world: &'a World,
    pub entries: &'a [PassEntry],
    pub now: &'a str,
}

/// The pass's first Automatic entry with a candidate: its top-ranked
/// Printer and that Printer's chosen Spool.
#[derive(Clone, Debug, PartialEq)]
pub struct Proposal {
    pub entry_id: String,
    pub printer_id: String,
    pub spool_id: String,
}

/// One summary per `queued` entry, in position order, and the proposal.
#[derive(Clone, Debug, PartialEq)]
pub struct PassResult {
    pub summaries: Vec<EligibilitySummary>,
    pub proposal: Option<Proposal>,
}

/// What one [`run_once`] concluded.
#[derive(Clone, Debug)]
pub struct EvaluationRun {
    /// The last pass's summaries: one per entry still `queued`.
    pub summaries: Vec<EligibilitySummary>,
    /// The Jobs this run assigned, in order.
    pub assigned: Vec<Job>,
    pub next: NextAutomaticAction,
    /// The entries whose assignment the in-transaction re-check refused.
    pub refused: Vec<String>,
}

/// D6: pure and sync. Evaluates every `queued` entry top to bottom, never
/// offers a Printer in `claimed`, skips the entries in `refused` (their
/// summary is the recorded blocker), and proposes the **first** Automatic
/// entry with a candidate (its top candidate and that candidate's chosen
/// Spool).
pub fn evaluate_pass(
    inputs: &PassInput<'_>,
    claimed: &BTreeSet<String>,
    refused: &BTreeMap<String, Blocker>,
) -> PassResult {
    let mut summaries = Vec::with_capacity(inputs.entries.len());
    let mut proposal = None;
    for PassEntry { entry, facts } in inputs.entries {
        if entry.state != QueueEntryState::Queued {
            continue;
        }
        if let Some(blocker) = refused.get(&entry.id) {
            summaries.push(refused_summary(&entry.id, blocker));
            continue;
        }
        let views = inputs.world.views(entry, None);
        let evaluated = eligibility::evaluate(
            &EligibilityInput {
                entry,
                facts,
                printers: &views,
                spools: &inputs.world.spools,
                claimed_printers: claimed,
            },
            inputs.now,
        );
        if proposal.is_none() && entry.policy == DispatchPolicy::Automatic {
            if let Some(top) = evaluated.candidates.first() {
                proposal = Some(Proposal {
                    entry_id: entry.id.clone(),
                    printer_id: top.printer_id.clone(),
                    spool_id: top.spool.spool_id.clone(),
                });
            }
        }
        summaries.push(EligibilitySummary::of(&evaluated));
    }
    PassResult {
        summaries,
        proposal,
    }
}

/// A refused entry's summary: blocked, with the refusal as `topBlocker`.
fn refused_summary(entry_id: &str, blocker: &Blocker) -> EligibilitySummary {
    EligibilitySummary {
        entry_id: entry_id.to_string(),
        verdict: EligibilityVerdict::Blocked,
        eligible_count: 0,
        top_blocker: Some(blocker.clone()),
        candidate_printer_ids: Vec::new(),
    }
}

/// How the in-transaction re-check answered a proposal.
enum Outcome {
    Assigned(Box<Job>),
    /// The entry is no longer `queued` (the operator assigned or removed
    /// it first). The next pass doesn't list it.
    Gone,
    Refused(Blocker),
}

/// D6 "Refused assignment": the refusal's first blocker, as the entry's
/// `topBlocker`. `None` for an error that isn't a refusal.
fn refusal_blocker(
    error: &RepositoryError,
    proposal: &Proposal,
    pass_entry: &PassEntry,
) -> Option<Blocker> {
    let printer_ids = vec![proposal.printer_id.clone()];
    match error {
        RepositoryError::AssignmentBlocked { blockers, .. } => blockers.first().cloned(),
        RepositoryError::JobActive { .. } => Some(Blocker {
            code: BlockerCode::JobActive,
            message: "This Printer already has a Job.".to_string(),
            detail: None,
            recovery: Some(RecoveryCode::OpenJob),
            printer_ids,
        }),
        RepositoryError::Reservation {
            error: ReservationError::InsufficientAvailable { available_mg },
            ..
        } => Some(Blocker {
            code: BlockerCode::InsufficientMaterial,
            message: eligibility::insufficient_material_message(
                pass_entry.entry.estimate.amount_mg,
            ),
            detail: Some(available_mg.to_string()),
            recovery: Some(RecoveryCode::LoadSpool),
            printer_ids,
        }),
        RepositoryError::Reservation {
            error: ReservationError::SpoolNotReservable { .. },
            ..
        } => Some(Blocker {
            code: BlockerCode::NoCompatibleSpool,
            message: eligibility::no_compatible_spool_message(&pass_entry.facts),
            detail: None,
            recovery: Some(RecoveryCode::LoadSpool),
            printer_ids,
        }),
        _ => None,
    }
}

/// The blocker `waiting` names for an Automatic entry no gate explained:
/// there is no Printer to consider at all.
fn no_printer_blocker() -> Blocker {
    Blocker {
        code: BlockerCode::SetupIncomplete,
        message: "No Printer is set up to take this entry.".to_string(),
        detail: None,
        recovery: Some(RecoveryCode::OpenPrinterSetup),
        printer_ids: Vec::new(),
    }
}

/// D6 `nextAutomaticAction`: `waiting` on the first `queued` Automatic
/// entry left (with its top blocker); else `assigned`, naming the last
/// Job this run assigned; else `noAutomaticEntries`.
fn next_action(
    entries: &[PassEntry],
    summaries: &[EligibilitySummary],
    assigned: &[Job],
    now: &str,
) -> NextAutomaticAction {
    let waiting = entries.iter().find(|pass_entry| {
        pass_entry.entry.state == QueueEntryState::Queued
            && pass_entry.entry.policy == DispatchPolicy::Automatic
    });
    if let Some(pass_entry) = waiting {
        let blocker = summaries
            .iter()
            .find(|summary| summary.entry_id == pass_entry.entry.id)
            .and_then(|summary| summary.top_blocker.clone())
            .unwrap_or_else(no_printer_blocker);
        return NextAutomaticAction::Waiting {
            evaluated_at: now.to_string(),
            entry_id: pass_entry.entry.id.clone(),
            blocker,
        };
    }
    match assigned.last() {
        Some(job) => NextAutomaticAction::Assigned {
            evaluated_at: now.to_string(),
            entry_id: job.queue_entry_id.clone(),
            job_id: job.id.clone(),
            printer_id: job.printer_id.clone(),
        },
        None => NextAutomaticAction::NoAutomaticEntries {
            evaluated_at: now.to_string(),
        },
    }
}

/// One read transaction: every durable fact D5 reads, the live statuses
/// and capabilities, and the `queued` entries in position order.
fn read_pass<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
) -> Result<(World, Vec<PassEntry>), RepositoryError> {
    let reader = LiveWorld::of(services);
    services
        .storage
        .read_transaction(|tx| {
            Ok((|| {
                let world = World::read(tx, &reader)?;
                let mut entries = Vec::new();
                for entry in repository::list_open(tx)? {
                    if entry.state != QueueEntryState::Queued {
                        continue;
                    }
                    let facts = facts_for(tx, &entry)?;
                    entries.push(PassEntry { entry, facts });
                }
                Ok::<_, RepositoryError>((world, entries))
            })())
        })
        .map_err(RepositoryError::Storage)?
}

/// D6: the proposal, assigned through the same `jobs::assign::assign` as
/// `assign_queue_entry`, under the Printer lock, in its own transaction.
/// The lock covers only that transaction and the publish of what it
/// committed; the driver stages the Job afterwards, from its own task.
async fn assign_proposal<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    proposal: &Proposal,
    pass_entry: &PassEntry,
) -> Result<Outcome, RepositoryError> {
    let request = AssignRequest {
        operation_id: format!("auto-{}", uuid::Uuid::new_v4()),
        entry_id: proposal.entry_id.clone(),
        printer_id: proposal.printer_id.clone(),
        spool_id: proposal.spool_id.clone(),
        mode: AssignMode::Automatic,
        assigned_by: AssignedBy::Automatic,
    };
    let assigned = {
        let printer_lock = services.host_ops.printer_lock(&proposal.printer_id);
        let _serialized = printer_lock.lock().await;
        let reader = LiveWorld::of(services);
        let now = now_rfc3339();
        let result = services
            .storage
            .write_repo(|tx| assign::assign(tx, &reader, &request, &now));
        match result {
            Ok(assigned) => {
                // In commit order with other writes on this Printer.
                if let Some(app) = services.jobs.app() {
                    super::commands::publish_rows(app, services, &assigned.change());
                }
                assigned
            }
            Err(RepositoryError::QueueEntryActionNotAllowed { .. }) => return Ok(Outcome::Gone),
            Err(error) => {
                return match refusal_blocker(&error, proposal, pass_entry) {
                    Some(blocker) => Ok(Outcome::Refused(blocker)),
                    None => Err(error),
                }
            }
        }
    };
    // D7: the driver stages every new Job, once.
    services.jobs.request_stage(&assigned.job.id);
    Ok(Outcome::Assigned(Box::new(assigned.job)))
}

/// D6: loops [`evaluate_pass`] → take the Printer lock →
/// `jobs::assign::assign`, re-reading the input every time, until a pass
/// proposes nothing. Publishes each assignment's entry, Job, and Spool
/// events as it commits; the caller publishes the eligibility change.
pub async fn run_once<R: tauri::Runtime>(
    services: &Arc<RuntimeServices<R>>,
) -> Result<EvaluationRun, RepositoryError> {
    let mut claimed = BTreeSet::new();
    let mut refused = BTreeMap::new();
    let mut assigned = Vec::new();
    loop {
        let (world, entries) = read_pass(services)?;
        let now = now_rfc3339();
        let pass = evaluate_pass(
            &PassInput {
                world: &world,
                entries: &entries,
                now: &now,
            },
            &claimed,
            &refused,
        );
        let Some(proposal) = pass.proposal else {
            let next = next_action(&entries, &pass.summaries, &assigned, &now);
            return Ok(EvaluationRun {
                summaries: pass.summaries,
                assigned,
                next,
                refused: refused.into_keys().collect(),
            });
        };
        services.evaluator.run_before_assign(&proposal);
        let pass_entry = entries
            .iter()
            .find(|pass_entry| pass_entry.entry.id == proposal.entry_id)
            .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
        match assign_proposal(services, &proposal, pass_entry).await? {
            Outcome::Assigned(job) => {
                claimed.insert(proposal.printer_id);
                assigned.push(*job);
            }
            Outcome::Gone => {}
            Outcome::Refused(blocker) => {
                refused.insert(proposal.entry_id, blocker);
            }
        }
    }
}

// --- the evaluator's state and task --------------------------------------------

/// Test hook run at the start of every run.
pub type RunHook = Box<dyn FnMut() + Send>;
/// Test hook run once, after a pass proposed and before the Printer lock.
pub type AssignHook = Box<dyn FnOnce(&Proposal) + Send>;

/// What the last finished run concluded, which `list_queue` serves.
#[derive(Clone, Debug, PartialEq)]
pub struct LastRun {
    pub summaries: Vec<EligibilitySummary>,
    pub next: NextAutomaticAction,
}

/// The evaluator's state, held in `RuntimeServices.evaluator`: the trigger
/// channel, the last run, and the counters and hooks tests read.
pub struct Evaluator {
    handle: EvaluatorHandle,
    receiver: Mutex<Option<mpsc::Receiver<Trigger>>>,
    started: OnceLock<()>,
    last: Mutex<Option<LastRun>>,
    /// Triggers taken off the channel.
    consumed: AtomicU64,
    running: AtomicUsize,
    max_running: AtomicUsize,
    runs_started: AtomicU64,
    runs: AtomicU64,
    on_run_start: Mutex<Option<RunHook>>,
    before_assign: Mutex<Option<AssignHook>>,
    /// Runs whose assignment the in-transaction re-check refused.
    refused_runs: AtomicU64,
    /// Times a run poked the evaluator once more after a refusal or a
    /// failure.
    repokes: AtomicU64,
    /// Test hook: how many of the next runs fail as a storage error would.
    injected_failures: AtomicU64,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

impl Default for Evaluator {
    fn default() -> Self {
        // D6: capacity 1. A full channel means a run is already pending.
        let (tx, rx) = mpsc::channel(1);
        Self {
            handle: EvaluatorHandle {
                tx,
                accepted: Arc::default(),
            },
            receiver: Mutex::new(Some(rx)),
            started: OnceLock::new(),
            last: Mutex::new(None),
            consumed: AtomicU64::new(0),
            running: AtomicUsize::new(0),
            max_running: AtomicUsize::new(0),
            runs_started: AtomicU64::new(0),
            runs: AtomicU64::new(0),
            on_run_start: Mutex::new(None),
            before_assign: Mutex::new(None),
            refused_runs: AtomicU64::new(0),
            repokes: AtomicU64::new(0),
            injected_failures: AtomicU64::new(0),
        }
    }
}

impl Evaluator {
    pub fn handle(&self) -> &EvaluatorHandle {
        &self.handle
    }

    /// [`EvaluatorHandle::poke`].
    pub fn poke(&self, trigger: Trigger) {
        self.handle.poke(trigger);
    }

    /// The last finished run's summaries and `nextAutomaticAction`; `None`
    /// until one has finished (ruling R3).
    pub fn last_run(&self) -> Option<LastRun> {
        lock(&self.last).clone()
    }

    /// Test hook: finished runs.
    pub fn runs(&self) -> u64 {
        self.runs.load(Ordering::SeqCst)
    }

    /// Test hook: started runs.
    pub fn runs_started(&self) -> u64 {
        self.runs_started.load(Ordering::SeqCst)
    }

    /// Test hook: the most runs ever in progress at once.
    pub fn max_concurrent_runs(&self) -> usize {
        self.max_running.load(Ordering::SeqCst)
    }

    /// Test hook: every trigger the channel took has been drained by a
    /// run, and no run is in progress. The task marks a run in progress
    /// before it counts what it drained, so reading `consumed` first never
    /// reports idle between the two.
    pub fn is_idle(&self) -> bool {
        let consumed = self.consumed.load(Ordering::SeqCst);
        let accepted = self.handle.accepted.load(Ordering::SeqCst);
        consumed == accepted && self.running.load(Ordering::SeqCst) == 0
    }

    /// Test hook: `hook` runs at the start of every run until replaced.
    pub fn on_run_start(&self, hook: Option<RunHook>) {
        *lock(&self.on_run_start) = hook;
    }

    /// Test hook: `hook` runs once, after the next pass proposes and before
    /// the evaluator takes the Printer lock to assign.
    pub fn before_assign(&self, hook: AssignHook) {
        *lock(&self.before_assign) = Some(hook);
    }

    /// Test hook: runs that saw a refusal.
    pub fn refused_runs(&self) -> u64 {
        self.refused_runs.load(Ordering::SeqCst)
    }

    /// Test hook: re-pokes after a refusal or a failed run.
    pub fn repokes(&self) -> u64 {
        self.repokes.load(Ordering::SeqCst)
    }

    /// Test hook: the next `count` runs fail as a storage error would.
    pub fn fail_next_runs(&self, count: u64) {
        self.injected_failures.store(count, Ordering::SeqCst);
    }

    /// Whether this run should fail, per [`Evaluator::fail_next_runs`].
    fn take_injected_failure(&self) -> bool {
        self.injected_failures
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok()
    }

    fn run_before_assign(&self, proposal: &Proposal) {
        let hook = lock(&self.before_assign).take();
        if let Some(hook) = hook {
            hook(proposal);
        }
    }

    fn begin_run(&self, drained: u64) {
        let running = self.running.fetch_add(1, Ordering::SeqCst) + 1;
        self.max_running.fetch_max(running, Ordering::SeqCst);
        self.consumed.fetch_add(drained, Ordering::SeqCst);
        self.runs_started.fetch_add(1, Ordering::SeqCst);
        if let Some(hook) = lock(&self.on_run_start).as_mut() {
            hook();
        }
    }

    fn end_run(&self) {
        self.runs.fetch_add(1, Ordering::SeqCst);
        self.running.fetch_sub(1, Ordering::SeqCst);
    }

    /// Caches the run's conclusion; `true` when a summary or the action
    /// (apart from `evaluatedAt`) changed, so it must be published.
    fn remember(&self, run: &EvaluationRun) -> bool {
        let next = LastRun {
            summaries: run.summaries.clone(),
            next: run.next.clone(),
        };
        let mut last = lock(&self.last);
        let changed = last.as_ref().is_none_or(|last| {
            last.summaries != next.summaries || without_time(&last.next) != without_time(&next.next)
        });
        *last = Some(next);
        changed
    }
}

/// `action` with `evaluatedAt` blanked, for comparing two conclusions.
fn without_time(action: &NextAutomaticAction) -> NextAutomaticAction {
    let mut action = action.clone();
    match &mut action {
        NextAutomaticAction::EvaluatorNotRunning => {}
        NextAutomaticAction::NoAutomaticEntries { evaluated_at }
        | NextAutomaticAction::Waiting { evaluated_at, .. }
        | NextAutomaticAction::Assigned { evaluated_at, .. } => evaluated_at.clear(),
    }
    action
}

/// Starts the evaluator for `services` (from `start_jobs_runtime`, after the
/// driver). The pumps subscribe before this returns, so no change published
/// afterwards is missed. The first run waits for the driver's first pass
/// and the host-ops startup pass. A second call does nothing.
pub fn start<R: tauri::Runtime>(services: &Arc<RuntimeServices<R>>) {
    let evaluator = &services.evaluator;
    if evaluator.started.set(()).is_err() {
        return;
    }
    let Some(receiver) = lock(&evaluator.receiver).take() else {
        return;
    };
    let stop = services.jobs.subscribe_stop();
    let handle = evaluator.handle.clone();

    pump(
        services.jobs.task_guard(),
        services.host_ops.subscribe_changes(),
        handle.clone(),
        stop.clone(),
        |_| Some(Trigger::HostOperationChanged),
        Trigger::HostOperationChanged,
    );
    pump(
        services.jobs.task_guard(),
        services.inventory_changes.subscribe(),
        handle.clone(),
        stop.clone(),
        |_| Some(Trigger::InventoryChanged),
        Trigger::InventoryChanged,
    );
    pump(
        services.jobs.task_guard(),
        services.host_ops.subscribe_host_facts(),
        handle.clone(),
        stop.clone(),
        |_| Some(Trigger::CapabilitiesChanged),
        Trigger::CapabilitiesChanged,
    );
    let manager = Arc::downgrade(&services.manager);
    let mut seen: HashMap<String, SchedulingFacts> = services
        .manager
        .statuses()
        .iter()
        .map(|(id, status)| (id.clone(), SchedulingFacts::of(status)))
        .collect();
    pump(
        services.jobs.task_guard(),
        services.manager.subscribe_status(),
        handle.clone(),
        stop.clone(),
        move |printer_id: String| status_trigger(&manager, &mut seen, printer_id),
        Trigger::StatusChanged(String::new()),
    );

    handle.poke(Trigger::Startup);
    let gates = (
        services.jobs.subscribe_first_pass(),
        services.host_ops.subscribe_startup_pass(),
    );
    let task = services.jobs.task_guard();
    let services = Arc::downgrade(services);
    tauri::async_runtime::spawn(async move {
        let _task = task;
        run_task(services, receiver, stop, gates).await;
    });
}

/// The status facts gate 1 reads. Telemetry changes far more often than
/// these, and a run on each would only repeat the last one.
#[derive(Clone, Copy, PartialEq, Eq)]
struct SchedulingFacts {
    connection: crate::connections::ConnectionState,
    operational: crate::printers::operational::OperationalState,
    freshness: crate::printers::operational::TelemetryFreshness,
}

impl SchedulingFacts {
    fn of(status: &PrinterStatus) -> Self {
        Self {
            connection: status.connection_state,
            operational: status.operational_state,
            freshness: status.freshness,
        }
    }
}

fn status_trigger<R: tauri::Runtime>(
    manager: &Weak<ConnectionManager<R>>,
    seen: &mut HashMap<String, SchedulingFacts>,
    printer_id: String,
) -> Option<Trigger> {
    let manager = manager.upgrade()?;
    let facts = manager.statuses().get(&printer_id).map(SchedulingFacts::of);
    let changed = match facts {
        Some(facts) => seen.insert(printer_id.clone(), facts) != Some(facts),
        None => seen.remove(&printer_id).is_some(),
    };
    changed.then_some(Trigger::StatusChanged(printer_id))
}

/// Forwards a broadcast into the trigger channel until it closes or the
/// runtime stops. `Lagged` pokes too: a run re-reads everything. `task`
/// counts the pump as a live runtime task until it ends.
fn pump<T: Clone + Send + 'static>(
    task: crate::jobs::services::TaskGuard,
    mut receiver: broadcast::Receiver<T>,
    handle: EvaluatorHandle,
    mut stop: watch::Receiver<bool>,
    mut trigger: impl FnMut(T) -> Option<Trigger> + Send + 'static,
    lagged: Trigger,
) {
    tauri::async_runtime::spawn(async move {
        let _task = task;
        loop {
            tokio::select! {
                changed = stop.changed() => {
                    if changed.is_err() || *stop.borrow() {
                        return;
                    }
                }
                received = receiver.recv() => match received {
                    Ok(value) => {
                        if let Some(trigger) = trigger(value) {
                            handle.poke(trigger);
                        }
                    }
                    Err(RecvError::Lagged(_)) => handle.poke(lagged.clone()),
                    Err(RecvError::Closed) => return,
                },
            }
        }
    });
}

/// D6: after a run that needs another look (a refusal, or a failed run),
/// poke once more — but not again after a second such run in a row, so a
/// lasting refusal or failure can't loop.
#[derive(Default)]
struct Retry {
    repoked: bool,
}

impl Retry {
    /// Whether to poke once more after a run.
    fn after_run(&mut self, needs_retry: bool) -> bool {
        let poke = needs_retry && !self.repoked;
        self.repoked = needs_retry;
        poke
    }
}

async fn wait_for_true(receiver: &mut watch::Receiver<bool>) -> bool {
    receiver.wait_for(|done| *done).await.is_ok()
}

/// The evaluator task: waits for the D4 gates, then drains, runs, and
/// publishes, one run at a time, until the runtime stops or goes away.
async fn run_task<R: tauri::Runtime>(
    services: Weak<RuntimeServices<R>>,
    mut receiver: mpsc::Receiver<Trigger>,
    mut stop: watch::Receiver<bool>,
    (mut driver_pass, mut host_ops_pass): (watch::Receiver<bool>, watch::Receiver<bool>),
) {
    let gates =
        async { wait_for_true(&mut driver_pass).await && wait_for_true(&mut host_ops_pass).await };
    tokio::select! {
        biased;
        _ = stop.wait_for(|stopped| *stopped) => return,
        ready = gates => if !ready { return },
    }
    let mut retry = Retry::default();
    loop {
        tokio::select! {
            biased;
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() {
                    return;
                }
                continue;
            }
            trigger = receiver.recv() => if trigger.is_none() { return },
        }
        if *stop.borrow() {
            return;
        }
        let Some(services) = services.upgrade() else {
            return;
        };
        let mut drained = 1;
        while receiver.try_recv().is_ok() {
            drained += 1;
        }
        let evaluator = Arc::clone(&services.evaluator);
        evaluator.begin_run(drained);
        let result = if evaluator.take_injected_failure() {
            Err(RepositoryError::Storage(StorageError::OperationFailed))
        } else {
            run_once(&services).await
        };
        let needs_retry = match result {
            Ok(run) => {
                if evaluator.remember(&run) {
                    if let Some(app) = services.jobs.app() {
                        services
                            .queue_stream
                            .publish_eligibility(app, &run.summaries, &run.next);
                    }
                }
                if !run.refused.is_empty() {
                    evaluator.refused_runs.fetch_add(1, Ordering::SeqCst);
                }
                !run.refused.is_empty()
            }
            Err(error) => {
                crate::f3d_log!(warn, "queue.evaluatorRunFailed", error = error);
                true
            }
        };
        // D6: a refusal pokes once more, and so does a failed run, so a
        // ready Automatic entry isn't stranded until an unrelated change.
        if retry.after_run(needs_retry) {
            evaluator.repokes.fetch_add(1, Ordering::SeqCst);
            evaluator.poke(Trigger::QueueChanged);
        }
        evaluator.end_run();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::queue::{DispatchPreference, EstimateSource, MaterialEstimate, QueueEntryDisplay};
    use crate::slicing::facts::Fact;

    fn blocker(code: BlockerCode) -> Blocker {
        Blocker {
            code,
            message: "m".to_string(),
            detail: None,
            recovery: None,
            printer_ids: vec!["prn-a".to_string()],
        }
    }

    fn summary(entry_id: &str, top: Option<Blocker>) -> EligibilitySummary {
        EligibilitySummary {
            entry_id: entry_id.to_string(),
            verdict: EligibilityVerdict::Blocked,
            eligible_count: 0,
            top_blocker: top,
            candidate_printer_ids: Vec::new(),
        }
    }

    fn job(entry_id: &str, job_id: &str) -> Job {
        let mut job = crate::jobs::state::tests_support::a_job(crate::jobs::JobState::Assigned);
        job.id = job_id.to_string();
        job.queue_entry_id = entry_id.to_string();
        job
    }

    fn pass_entry(id: &str, policy: DispatchPolicy) -> PassEntry {
        let entry = QueueEntry {
            id: id.to_string(),
            revision: 1,
            slice_revision_id: "slr".to_string(),
            lineage_id: "qln".to_string(),
            copy_index: 1,
            copy_count: 1,
            origin_entry_id: None,
            origin_kind: None,
            state: QueueEntryState::Queued,
            close_reason: None,
            position: Some(1),
            policy,
            preference: DispatchPreference::LoadedFirst,
            estimate: MaterialEstimate {
                amount_mg: 1,
                source: EstimateSource::SliceEstimate,
            },
            manual_printer_id: None,
            job_id: None,
            requires_manual_printer_selection: false,
            allowed_actions: Vec::new(),
            display: QueueEntryDisplay {
                model_id: "mdl".to_string(),
                model_name: "M".to_string(),
                plate_label: None,
                target_label: "T".to_string(),
                material_family: None,
                material_other: None,
                print_seconds: None,
            },
            created_at: "t".to_string(),
            updated_at: "t".to_string(),
            closed_at: None,
        };
        let facts = SliceFacts {
            printer_profile: Fact::absent(),
            nozzle_diameter_mm: Fact::absent(),
            material_family: Fact::absent(),
            material_other: None,
            filament_diameter_mm: Fact::absent(),
        };
        PassEntry { entry, facts }
    }

    // --- the pure pass ---------------------------------------------------------

    use crate::queue::eligibility::tests as fixtures;
    use crate::queue::world::PrinterRow;
    use crate::spools::SpoolLocation;

    /// A Printer that passes gates 1-3 with Spool `spool_id` (900 g)
    /// loaded, and no durable Job.
    fn loaded_printer(id: &str, spool_id: &str) -> (PrinterRow, crate::spools::SpoolRecord) {
        let number = spool_id.trim_start_matches("spl-").parse().unwrap();
        let spool = fixtures::a_pla_spool(
            spool_id,
            number,
            900_000,
            SpoolLocation::Slot {
                slot_id: format!("slot-{id}"),
                printer_id: id.to_string(),
            },
        );
        let row = PrinterRow {
            stored: fixtures::a_stored_printer(id, id, false, "moonraker"),
            profile: fixtures::a_profile(),
            status: Some(fixtures::ready_status()),
            capabilities: fixtures::sim_capabilities(id),
            active_job: false,
            foreign_host_op: false,
            last_used_at: None,
            loaded_spool_ids: vec![spool_id.to_string()],
        };
        (row, spool)
    }

    fn world_of(printers: &[(&str, &str)]) -> World {
        let (rows, spools) = printers
            .iter()
            .map(|(id, spool)| loaded_printer(id, spool))
            .unzip();
        World::from_parts(rows, spools)
    }

    fn queued(id: &str, position: i64, policy: DispatchPolicy) -> PassEntry {
        let mut entry = fixtures::an_entry(policy, DispatchPreference::LoadedFirst, 100_000);
        entry.id = id.to_string();
        entry.position = Some(position);
        PassEntry {
            entry,
            facts: fixtures::pla_175_facts(),
        }
    }

    fn pass(
        world: &World,
        entries: &[PassEntry],
        claimed: &[&str],
        refused: &[(&str, Blocker)],
    ) -> PassResult {
        evaluate_pass(
            &PassInput {
                world,
                entries,
                now: "2026-09-27T00:00:00Z",
            },
            &claimed.iter().map(|id| id.to_string()).collect(),
            &refused
                .iter()
                .map(|(id, blocker)| (id.to_string(), blocker.clone()))
                .collect(),
        )
    }

    fn summary_of<'a>(result: &'a PassResult, entry_id: &str) -> &'a EligibilitySummary {
        result
            .summaries
            .iter()
            .find(|summary| summary.entry_id == entry_id)
            .expect("a summary")
    }

    #[test]
    fn the_pass_proposes_the_first_automatic_entry_in_order() {
        let world = world_of(&[("prn-a", "spl-1")]);
        let entries = [
            queued("qen-r", 1, DispatchPolicy::Recommended),
            queued("qen-m", 2, DispatchPolicy::Manual),
            queued("qen-1", 3, DispatchPolicy::Automatic),
            queued("qen-2", 4, DispatchPolicy::Automatic),
        ];

        let result = pass(&world, &entries, &[], &[]);

        assert_eq!(
            result.proposal,
            Some(Proposal {
                entry_id: "qen-1".to_string(),
                printer_id: "prn-a".to_string(),
                spool_id: "spl-1".to_string(),
            })
        );
        let order: Vec<&str> = result
            .summaries
            .iter()
            .map(|summary| summary.entry_id.as_str())
            .collect();
        assert_eq!(order, ["qen-r", "qen-m", "qen-1", "qen-2"]);
        assert_eq!(
            summary_of(&result, "qen-r").verdict,
            EligibilityVerdict::AwaitingOperator
        );
        assert_eq!(
            summary_of(&result, "qen-1").verdict,
            EligibilityVerdict::Ready
        );
    }

    /// No Printer here has a durable Job: only `claimed` can block it.
    #[test]
    fn a_claimed_printer_is_never_offered_again_in_the_same_run() {
        let world = world_of(&[("prn-a", "spl-1"), ("prn-b", "spl-2")]);
        let entries = [queued("qen-2", 2, DispatchPolicy::Automatic)];

        let one_left = pass(&world, &entries, &["prn-a"], &[]);
        assert_eq!(
            one_left.proposal.as_ref().map(|p| p.printer_id.as_str()),
            Some("prn-b")
        );
        assert_eq!(
            summary_of(&one_left, "qen-2").candidate_printer_ids,
            ["prn-b"]
        );

        let none_left = pass(&world, &entries, &["prn-a", "prn-b"], &[]);
        assert_eq!(none_left.proposal, None);
        let summary = summary_of(&none_left, "qen-2");
        assert_eq!(summary.verdict, EligibilityVerdict::Blocked);
        assert_eq!(summary.eligible_count, 0);
        let top = summary.top_blocker.as_ref().expect("a blocker");
        assert_eq!(top.code, BlockerCode::JobActive);
        assert_eq!(top.printer_ids, ["prn-a", "prn-b"]);
    }

    #[test]
    fn a_refused_entry_is_skipped_and_a_later_one_takes_the_unclaimed_printer() {
        let world = world_of(&[("prn-a", "spl-1")]);
        let entries = [
            queued("qen-1", 1, DispatchPolicy::Automatic),
            queued("qen-2", 2, DispatchPolicy::Automatic),
        ];
        let refusal = blocker(BlockerCode::InsufficientMaterial);

        let result = pass(&world, &entries, &[], &[("qen-1", refusal.clone())]);

        assert_eq!(
            result.proposal,
            Some(Proposal {
                entry_id: "qen-2".to_string(),
                printer_id: "prn-a".to_string(),
                spool_id: "spl-1".to_string(),
            })
        );
        let skipped = summary_of(&result, "qen-1");
        assert_eq!(skipped.verdict, EligibilityVerdict::Blocked);
        assert_eq!(skipped.eligible_count, 0);
        assert!(skipped.candidate_printer_ids.is_empty());
        assert_eq!(skipped.top_blocker.as_ref(), Some(&refusal));
    }

    #[test]
    fn a_retry_pokes_once_then_stops_until_a_clean_run() {
        let mut retry = Retry::default();
        assert!(!retry.after_run(false), "a clean run pokes nothing");
        assert!(
            retry.after_run(true),
            "the first refused or failed run pokes"
        );
        assert!(!retry.after_run(true), "the second in a row doesn't");
        assert!(!retry.after_run(true));
        assert!(!retry.after_run(false), "a clean run resets");
        assert!(retry.after_run(true), "so the next one pokes again");
    }

    #[test]
    fn next_action_waits_on_the_first_automatic_entry_left() {
        let entries = [
            pass_entry("qen-m", DispatchPolicy::Manual),
            pass_entry("qen-1", DispatchPolicy::Automatic),
            pass_entry("qen-2", DispatchPolicy::Automatic),
        ];
        let summaries = [
            summary("qen-m", None),
            summary("qen-1", Some(blocker(BlockerCode::JobActive))),
            summary("qen-2", Some(blocker(BlockerCode::SpoolNotLoaded))),
        ];
        let assigned = [job("qen-0", "job-0")];
        match next_action(&entries, &summaries, &assigned, "now") {
            NextAutomaticAction::Waiting {
                entry_id, blocker, ..
            } => {
                assert_eq!(entry_id, "qen-1");
                assert_eq!(blocker.code, BlockerCode::JobActive);
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn next_action_names_the_last_assignment_when_nothing_waits() {
        let entries = [pass_entry("qen-m", DispatchPolicy::Recommended)];
        let assigned = [job("qen-1", "job-1"), job("qen-2", "job-2")];
        assert_eq!(
            next_action(&entries, &[summary("qen-m", None)], &assigned, "now"),
            NextAutomaticAction::Assigned {
                evaluated_at: "now".to_string(),
                entry_id: "qen-2".to_string(),
                job_id: "job-2".to_string(),
                printer_id: "prn-a".to_string(),
            }
        );
        assert_eq!(
            next_action(&entries, &[], &[], "now"),
            NextAutomaticAction::NoAutomaticEntries {
                evaluated_at: "now".to_string()
            }
        );
    }

    #[test]
    fn an_automatic_entry_no_gate_explained_waits_on_setup() {
        let entries = [pass_entry("qen-1", DispatchPolicy::Automatic)];
        match next_action(&entries, &[summary("qen-1", None)], &[], "now") {
            NextAutomaticAction::Waiting { blocker, .. } => {
                assert_eq!(blocker.code, BlockerCode::SetupIncomplete);
                assert!(blocker.printer_ids.is_empty());
            }
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn refusals_map_to_the_spec_blockers() {
        let proposal = Proposal {
            entry_id: "qen-1".to_string(),
            printer_id: "prn-a".to_string(),
            spool_id: "spl-1".to_string(),
        };
        let entry = pass_entry("qen-1", DispatchPolicy::Automatic);
        let reservation = |error| RepositoryError::Reservation {
            spool_id: "spl-1".to_string(),
            spool_number: Some(1),
            reservation_id: None,
            required_mg: Some(1),
            error,
        };
        let code = |error: RepositoryError| {
            refusal_blocker(&error, &proposal, &entry).map(|blocker| blocker.code)
        };
        assert_eq!(
            code(RepositoryError::AssignmentBlocked {
                entry_id: "qen-1".to_string(),
                printer_id: "prn-a".to_string(),
                spool_id: "spl-1".to_string(),
                blockers: vec![
                    blocker(BlockerCode::PrinterNotIdle),
                    blocker(BlockerCode::SpoolNotLoaded)
                ],
            }),
            Some(BlockerCode::PrinterNotIdle)
        );
        assert_eq!(
            code(RepositoryError::JobActive {
                printer_id: "prn-a".to_string(),
                job_id: "job-1".to_string(),
            }),
            Some(BlockerCode::JobActive)
        );
        assert_eq!(
            code(reservation(ReservationError::InsufficientAvailable {
                available_mg: 5
            })),
            Some(BlockerCode::InsufficientMaterial)
        );
        assert_eq!(
            code(reservation(ReservationError::SpoolNotReservable {
                lifecycle: crate::spools::SpoolLifecycle::Empty
            })),
            Some(BlockerCode::NoCompatibleSpool)
        );
        assert_eq!(
            code(RepositoryError::Storage(StorageError::OperationFailed)),
            None
        );
    }

    #[test]
    fn a_conclusion_that_differs_only_in_time_is_not_republished() {
        let evaluator = Evaluator::default();
        let run = |at: &str| EvaluationRun {
            summaries: vec![summary("qen-1", None)],
            assigned: Vec::new(),
            next: NextAutomaticAction::NoAutomaticEntries {
                evaluated_at: at.to_string(),
            },
            refused: Vec::new(),
        };
        assert!(evaluator.remember(&run("t1")));
        assert!(!evaluator.remember(&run("t2")));
        assert_eq!(
            evaluator.last_run().unwrap().next,
            NextAutomaticAction::NoAutomaticEntries {
                evaluated_at: "t2".to_string()
            }
        );
    }

    #[test]
    fn pokes_coalesce_into_one_pending_trigger() {
        let evaluator = Evaluator::default();
        for _ in 0..100 {
            evaluator.poke(Trigger::QueueChanged);
        }
        assert_eq!(evaluator.handle.accepted.load(Ordering::SeqCst), 1);
        assert!(!evaluator.is_idle());
    }
}
