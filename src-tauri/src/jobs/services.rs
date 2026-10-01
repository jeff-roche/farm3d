//! P7 D7: the Job runtime — [`JobTimings`], [`JobServices`] (held in
//! `RuntimeServices.jobs`), and the dispatch driver, one tokio task per
//! process started by `start_jobs_runtime`.
//!
//! The driver selects on the host-ops change broadcast, the Printer status
//! broadcast, the inventory broadcast, its "stage now" requests, a poll
//! interval, and a stop signal:
//!
//! - **Host Operation changed** (a Job-linked row): [`dispatch::apply_host_outcome`]
//!   under the Printer lock, in its own transaction, then publish.
//! - **Status or inventory changed**: re-evaluate the Printer's
//!   `awaitingStart` Job — republish it when its `startBlockers` changed,
//!   and start it unattended when decision 1 allows.
//! - **Stage requested** (after an assignment), and on its first pass for
//!   every `assigned` Job that never staged (R2): stage it once with a
//!   `drv-*` id. A status change also stages an `assigned` Job that
//!   hasn't staged. A refusal before any row was written records
//!   `lastFailure = refused` and is never retried by the driver, except a
//!   "not yet" refusal (the Printer not reachable yet), which records
//!   nothing and is tried again (see `is_not_yet`).
//! - **`RecvError::Lagged`** or the poll interval: re-read every active
//!   Job from storage and do all of the above for it.
//! - **The tracker** (`jobs::tracker`, Task 8b): every resync polls the
//!   history of each `printing`/`paused` Job, so `JobTimings.history_poll`
//!   is the history poll. A Job entering `printing`, a succeeded cancel
//!   op, and a status that says our file ended each trigger a poll at
//!   once; a status on our file also moves progress and `printing` ⇄
//!   `paused`. When a Job's `host_unreachable_since` crosses
//!   `unreachable_declare_after`, the resync republishes it, so its
//!   `allowedActions` show `declareOutcome`.
//!
//! Every Job write takes `host_ops.printer_lock(printer_id)` first, except
//! the handoffs, which call `host_ops::api` (it takes that non-reentrant
//! lock itself).

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::AppHandle;
use tokio::sync::broadcast::error::RecvError;

use crate::contracts::command::{CommandError, ErrorCode};
use crate::host_ops::{
    repository as host_ops_repository, Clock, HostOperation, HostOperationKind, PriorState,
    SystemClock,
};
use crate::persistence::RepositoryError;
use crate::printers::now_rfc3339;
use crate::queue::{Blocker, QueueChange};
use crate::RuntimeServices;

use super::dispatch::{self, StartContext};
use super::repository::{self as jobs_repository, JobChange};
use super::{tracker, Job, JobAction, JobFailure, JobState, StartConfirmation};

/// D7's tracker and driver timings. `default()` holds the production
/// values (10 s, 3, 30 min); tests inject short ones.
#[derive(Clone, Copy, Debug)]
pub struct JobTimings {
    /// How often the driver sweeps active Jobs (and the tracker polls
    /// history).
    pub history_poll: Duration,
    /// Inconclusive history polls before `outcomeUnknown`.
    pub inconclusive_limit: u32,
    /// How long `host_unreachable_since` must be before `declareOutcome`
    /// is offered from `printing`/`paused` (D9).
    pub unreachable_declare_after: Duration,
}

impl Default for JobTimings {
    fn default() -> Self {
        Self {
            history_poll: Duration::from_secs(10),
            inconclusive_limit: 3,
            unreachable_declare_after: Duration::from_secs(30 * 60),
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The dispatch driver's state, held in `RuntimeServices.jobs`.
pub struct JobServices<R: tauri::Runtime> {
    pub timings: JobTimings,
    /// The tracker's time source: `host_unreachable_since`, and when
    /// `declareOutcome` is offered. Injectable so no test waits 30 minutes.
    clock: Arc<dyn Clock>,
    app: OnceLock<AppHandle<R>>,
    started: OnceLock<()>,
    stage_requests: tokio::sync::mpsc::UnboundedSender<String>,
    stage_receiver: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<String>>>,
    /// What `recover_after_restart` changed, published when the driver
    /// starts.
    recovered: Mutex<Vec<Job>>,
    stop: tokio::sync::watch::Sender<bool>,
    /// The `startBlockers` last published per `awaitingStart` Job.
    published_blockers: Mutex<HashMap<String, Vec<Blocker>>>,
    /// Unattended starts in flight, so one status burst starts a Job once.
    starting: Mutex<HashSet<String>>,
    /// Test hook: run once, after `start_job`'s pre-checks and before its
    /// write-ahead, so a test can change the world between the two.
    before_start_link: Mutex<Option<StartLinkHook>>,
    /// Completed resyncs (the first pass is the first), for tests.
    resyncs: std::sync::atomic::AtomicU64,
    /// `true` once the driver's first pass has finished (D4 "Recovery
    /// order": the evaluator's first run waits for it).
    first_pass: tokio::sync::watch::Sender<bool>,
    /// `printing`/`paused` Jobs last published with `declareOutcome`, so
    /// crossing the mark republishes each once.
    declare_offered: Mutex<HashSet<String>>,
    /// When a status hint last triggered a poll, per Job: at most one per
    /// `history_poll`, so a chatty status can't flood the host.
    hinted: Mutex<HashMap<String, std::time::Instant>>,
    /// Jobs a succeeded pause or resume just moved, with the state it moved
    /// them to. The cached live status can still show the state before, so
    /// status-following leaves such a Job alone until a status reports
    /// that state (see `tracker::observe_status`). Set under the Printer
    /// lock; dropped once a status reports that state, or the Job is no
    /// longer active.
    awaiting_status: Mutex<HashMap<String, JobState>>,
    /// The runtime's live tasks (the driver, and the evaluator's task and
    /// pumps), so a test can wait for a stopped runtime to finish.
    tasks: Arc<std::sync::atomic::AtomicUsize>,
    barriers: tokio::sync::mpsc::UnboundedSender<Barrier>,
    barrier_receiver: Mutex<Option<tokio::sync::mpsc::UnboundedReceiver<Barrier>>>,
}

/// A [`JobServices::barrier`] request: answered once the driver has
/// handled everything queued for it.
type Barrier = tokio::sync::oneshot::Sender<()>;

/// Counts one live runtime task until dropped (see
/// [`JobServices::running_tasks`]).
pub(crate) struct TaskGuard(Arc<std::sync::atomic::AtomicUsize>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
    }
}

/// See [`JobServices::before_start_link`].
pub type StartLinkHook = Box<dyn FnOnce(&crate::persistence::Storage) + Send>;

impl<R: tauri::Runtime> JobServices<R> {
    pub fn new(timings: JobTimings) -> Self {
        Self::with_clock(timings, Arc::new(SystemClock))
    }

    /// [`JobServices::new`] with the tracker's clock injected.
    pub fn with_clock(timings: JobTimings, clock: Arc<dyn Clock>) -> Self {
        let (stage_requests, stage_receiver) = tokio::sync::mpsc::unbounded_channel();
        let (barriers, barrier_receiver) = tokio::sync::mpsc::unbounded_channel();
        Self {
            timings,
            clock,
            app: OnceLock::new(),
            started: OnceLock::new(),
            stage_requests,
            stage_receiver: Mutex::new(Some(stage_receiver)),
            recovered: Mutex::new(Vec::new()),
            stop: tokio::sync::watch::channel(false).0,
            published_blockers: Mutex::new(HashMap::new()),
            starting: Mutex::new(HashSet::new()),
            before_start_link: Mutex::new(None),
            resyncs: std::sync::atomic::AtomicU64::new(0),
            first_pass: tokio::sync::watch::channel(false).0,
            declare_offered: Mutex::new(HashSet::new()),
            hinted: Mutex::new(HashMap::new()),
            awaiting_status: Mutex::new(HashMap::new()),
            tasks: Arc::default(),
            barriers,
            barrier_receiver: Mutex::new(Some(barrier_receiver)),
        }
    }

    /// The tracker's "now".
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// The state a succeeded pause or resume moved `job_id` to, while no
    /// live status has reported it yet (`awaiting_status`).
    pub(crate) fn awaiting_status(&self, job_id: &str) -> Option<JobState> {
        lock(&self.awaiting_status).get(job_id).copied()
    }

    /// A live status reported the state `job_id`'s pause or resume moved
    /// it to: status-following applies to it again.
    pub(crate) fn status_caught_up(&self, job_id: &str) {
        lock(&self.awaiting_status).remove(job_id);
    }

    /// Test hook: `hook` runs once, in the next `start_job` (operator or
    /// driver), after its pre-checks and before the write-ahead
    /// transaction whose link re-checks them.
    pub fn before_start_link(&self, hook: StartLinkHook) {
        *lock(&self.before_start_link) = Some(hook);
    }

    /// Test hook: how many resyncs the driver has finished. The first
    /// pass is one, so `>= 1` means it has run.
    pub fn resyncs(&self) -> u64 {
        self.resyncs.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// A receiver that reads `true` once the driver's first pass has
    /// finished (D4 "Recovery order").
    pub fn subscribe_first_pass(&self) -> tokio::sync::watch::Receiver<bool> {
        self.first_pass.subscribe()
    }

    /// A receiver of the stop signal [`JobServices::stop`] sends, so the
    /// evaluator stops with the driver, as a crash would stop both.
    pub(crate) fn subscribe_stop(&self) -> tokio::sync::watch::Receiver<bool> {
        self.stop.subscribe()
    }

    /// The app the runtime publishes to, once `start_jobs_runtime` ran.
    pub(crate) fn app(&self) -> Option<&AppHandle<R>> {
        self.app.get()
    }

    pub(crate) fn take_before_start_link(&self) -> Option<StartLinkHook> {
        lock(&self.before_start_link).take()
    }

    /// Asks the driver to stage `job_id` (D7: after each committed
    /// assignment). Queued until the driver runs; the driver's first pass
    /// would find the Job anyway.
    pub fn request_stage(&self, job_id: &str) {
        let _ = self.stage_requests.send(job_id.to_string());
    }

    /// The Jobs `recover_after_restart` changed, published once the driver
    /// starts (D4 "Recovery order", step 2).
    pub fn set_recovered(&self, jobs: Vec<Job>) {
        lock(&self.recovered).extend(jobs);
    }

    /// Test hook: stops the driver, as a crash would. Nothing it would
    /// have applied is applied; a restart's recovery catches up. The
    /// driver and the evaluator finish the step they are in (a resync
    /// stops between Jobs); [`JobServices::running_tasks`] reaches zero
    /// once they have.
    pub fn stop(&self) {
        let _ = self.stop.send(true);
    }

    /// Whether [`JobServices::stop`] was called.
    fn stopping(&self) -> bool {
        *self.stop.borrow()
    }

    /// Test hook: the runtime's tasks still running (the driver, and the
    /// evaluator's task and pumps). Zero after a stop means nothing of the
    /// stopped runtime can write any more.
    pub fn running_tasks(&self) -> usize {
        self.tasks.load(std::sync::atomic::Ordering::SeqCst)
    }

    /// Counts a runtime task from before it is spawned until it ends.
    pub(crate) fn task_guard(&self) -> TaskGuard {
        self.tasks.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        TaskGuard(Arc::clone(&self.tasks))
    }

    /// Test hook: a receiver that gets `()` once the driver has handled
    /// every host-ops change, status, inventory change, and stage request
    /// sent to it before this call. It errors when the driver stops first.
    /// Before the driver starts, nothing answers it until it does.
    pub fn barrier(&self) -> tokio::sync::oneshot::Receiver<()> {
        let (done, receiver) = tokio::sync::oneshot::channel();
        let _ = self.barriers.send(done);
        receiver
    }
}

/// Starts the dispatch driver for `services` (`start_jobs_runtime`). A
/// second call does nothing. The broadcasts are subscribed before the task
/// is spawned, so nothing published after this call is missed.
pub fn start<R: tauri::Runtime>(services: &Arc<RuntimeServices<R>>, app: &AppHandle<R>) {
    let jobs = &services.jobs;
    let _ = jobs.app.set(app.clone());
    if jobs.started.set(()).is_err() {
        return;
    }
    let driver = Driver {
        _task: jobs.task_guard(),
        services: Arc::downgrade(services),
        host_operations: services.host_ops.subscribe_changes(),
        statuses: services.manager.subscribe_status(),
        inventory: services.inventory_changes.subscribe(),
        stage_requests: lock(&jobs.stage_receiver).take(),
        barriers: lock(&jobs.barrier_receiver).take(),
        waiting: Vec::new(),
        stop: jobs.stop.subscribe(),
        poll_every: jobs.timings.history_poll,
        poll: None,
    };
    tauri::async_runtime::spawn(driver.run());
}

struct Driver<R: tauri::Runtime> {
    _task: TaskGuard,
    services: Weak<RuntimeServices<R>>,
    host_operations: tokio::sync::broadcast::Receiver<HostOperation>,
    statuses: tokio::sync::broadcast::Receiver<String>,
    inventory: tokio::sync::broadcast::Receiver<crate::spools::events::InventoryChange>,
    stage_requests: Option<tokio::sync::mpsc::UnboundedReceiver<String>>,
    barriers: Option<tokio::sync::mpsc::UnboundedReceiver<Barrier>>,
    /// Barriers not yet answered: something was still queued.
    waiting: Vec<Barrier>,
    stop: tokio::sync::watch::Receiver<bool>,
    poll_every: Duration,
    poll: Option<tokio::time::Interval>,
}

/// What one iteration of the driver's `select!` woke up for.
enum Wake {
    Stop,
    HostOperation(Box<HostOperation>),
    Printer(String),
    Inventory,
    Stage(String),
    Resync,
    Barrier(Barrier),
}

impl<R: tauri::Runtime> Driver<R> {
    async fn run(mut self) {
        {
            let Some(services) = self.services.upgrade() else {
                return;
            };
            let recovered = std::mem::take(&mut *lock(&services.jobs.recovered));
            if !recovered.is_empty() {
                let mut change = QueueChange {
                    jobs: recovered,
                    ..QueueChange::default()
                };
                change.requirements = open_requirements_of(&services, &change.jobs);
                publish(&services, change);
            }
            resync(&services).await;
            services.jobs.first_pass.send_replace(true);
        }
        // Created inside the task, on the runtime; the first tick fires at
        // once, and the first pass above already did its work.
        let mut poll = tokio::time::interval(self.poll_every);
        poll.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        poll.tick().await;
        self.poll = Some(poll);
        loop {
            let wake = self.next().await;
            // `select!` picks among ready branches at random: a poll tick
            // must never win over a stop (a crash does no more work).
            if *self.stop.borrow() {
                return;
            }
            let Some(services) = self.services.upgrade() else {
                return;
            };
            match wake {
                Wake::Stop => return,
                Wake::HostOperation(op) => on_host_operation(&services, &op).await,
                Wake::Printer(printer_id) => on_printer(&services, &printer_id).await,
                Wake::Inventory => on_awaiting_start(&services).await,
                Wake::Stage(job_id) => stage_by_driver(&services, &job_id).await,
                Wake::Resync => resync(&services).await,
                Wake::Barrier(done) => self.waiting.push(done),
            }
            self.answer_barriers();
        }
    }

    /// Answers the waiting barriers once nothing is queued for the driver:
    /// everything sent before them has been received, and so handled.
    fn answer_barriers(&mut self) {
        if self.waiting.is_empty() {
            return;
        }
        let idle = self.host_operations.is_empty()
            && self.statuses.is_empty()
            && self.inventory.is_empty()
            && self
                .stage_requests
                .as_ref()
                .is_none_or(tokio::sync::mpsc::UnboundedReceiver::is_empty);
        if idle {
            for done in self.waiting.drain(..) {
                let _ = done.send(());
            }
        }
    }

    async fn next(&mut self) -> Wake {
        let poll = self
            .poll
            .as_mut()
            .expect("the poll interval exists once running");
        let stage_requests = &mut self.stage_requests;
        let next_stage = async {
            match stage_requests {
                Some(receiver) => receiver.recv().await,
                None => std::future::pending().await,
            }
        };
        let barriers = &mut self.barriers;
        let next_barrier = async {
            match barriers {
                Some(receiver) => receiver.recv().await,
                None => std::future::pending().await,
            }
        };
        tokio::select! {
            changed = self.stop.changed() => {
                if changed.is_err() || *self.stop.borrow() {
                    Wake::Stop
                } else {
                    Wake::Resync
                }
            }
            op = self.host_operations.recv() => match op {
                Ok(op) => Wake::HostOperation(Box::new(op)),
                Err(RecvError::Lagged(_)) => Wake::Resync,
                Err(RecvError::Closed) => Wake::Stop,
            },
            printer = self.statuses.recv() => match printer {
                Ok(printer_id) => Wake::Printer(printer_id),
                Err(RecvError::Lagged(_)) => Wake::Resync,
                Err(RecvError::Closed) => Wake::Stop,
            },
            change = self.inventory.recv() => match change {
                Ok(_) => Wake::Inventory,
                Err(RecvError::Lagged(_)) => Wake::Resync,
                Err(RecvError::Closed) => Wake::Stop,
            },
            job_id = next_stage => match job_id {
                Some(job_id) => Wake::Stage(job_id),
                None => Wake::Stop,
            },
            _ = poll.tick() => Wake::Resync,
            Some(done) = next_barrier => Wake::Barrier(done),
        }
    }
}

/// Logs a failed repository step of the dispatch driver. `code` names the
/// step; the error logs its variant only.
fn log_failure(code: &'static str, error: &RepositoryError) {
    crate::f3d_log!(warn, code, error = error);
}

/// Publishes `change` on the `queue` stream, with live `startBlockers`,
/// then the Spools it touched on the inventory stream; remembers what it
/// published for each `awaitingStart` Job, and which Jobs it published
/// with `declareOutcome`.
fn publish<R: tauri::Runtime>(services: &RuntimeServices<R>, mut change: QueueChange) {
    dispatch::present_change(services, &mut change);
    {
        let mut published = lock(&services.jobs.published_blockers);
        let mut offered = lock(&services.jobs.declare_offered);
        for job in &change.jobs {
            if job.state == JobState::AwaitingStart {
                published.insert(job.id.clone(), job.start_blockers.clone());
            } else {
                published.remove(&job.id);
            }
            if job.allowed_actions.contains(&JobAction::DeclareOutcome)
                && matches!(job.state, JobState::Printing | JobState::Paused)
            {
                offered.insert(job.id.clone());
            } else {
                offered.remove(&job.id);
            }
        }
    }
    if let Some(app) = services.jobs.app.get() {
        services.queue_stream.publish(app, &change);
        crate::spools::events::publish_ids(app, services, &change.spool_ids, &[]);
    }
    // D6 `JobChanged`: a Job that ended frees its Printer (and its entry
    // left the Queue).
    if change.jobs.iter().any(|job| job.state.is_terminal()) {
        services
            .evaluator
            .poke(crate::queue::evaluator::Trigger::JobChanged);
    }
}

fn open_requirements_of<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    jobs: &[Job],
) -> Vec<super::ReconciliationRequirement> {
    services
        .storage
        .read(|connection| {
            let mut requirements = Vec::new();
            for job in jobs {
                if let Ok(rows) = jobs_repository::requirements_for_job(connection, &job.id) {
                    requirements.extend(
                        rows.into_iter()
                            .filter(|row| row.status != super::RequirementStatus::Resolved),
                    );
                }
            }
            Ok(requirements)
        })
        .unwrap_or_default()
}

fn read_active(services: &RuntimeServices<impl tauri::Runtime>) -> Vec<Job> {
    services
        .storage
        .read(|connection| Ok(jobs_repository::list_active(connection)))
        .ok()
        .and_then(Result::ok)
        .unwrap_or_default()
}

fn load_job(services: &RuntimeServices<impl tauri::Runtime>, job_id: &str) -> Option<Job> {
    services
        .storage
        .read(|connection| Ok(jobs_repository::load_job(connection, job_id)))
        .ok()
        .and_then(Result::ok)
        .flatten()
}

/// Whether the Job ever handed off a `kind` Host Operation.
fn has_handed_off(
    services: &RuntimeServices<impl tauri::Runtime>,
    job_id: &str,
    kind: HostOperationKind,
) -> bool {
    services
        .storage
        .read(|connection| {
            connection.query_row(
                "SELECT EXISTS(SELECT 1 FROM host_operations WHERE job_id = ?1 AND kind = ?2)",
                rusqlite::params![job_id, crate::spools::encode_enum(kind)],
                |row| row.get::<_, bool>(0),
            )
        })
        .unwrap_or(true)
}

/// Re-applies the Job-linked op `op_id`'s current state to its Job, under
/// the Printer lock, and publishes what changed.
async fn apply_linked<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer_id: &str,
    op_id: &str,
) {
    apply_linked_job(services, printer_id, op_id).await;
}

/// [`apply_linked`], returning the Job it changed.
async fn apply_linked_job<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer_id: &str,
    op_id: &str,
) -> Option<Job> {
    let printer_lock = services.host_ops.printer_lock(printer_id);
    let _serialized = printer_lock.lock().await;
    let now = now_rfc3339();
    let applied = services.storage.write_repo(|tx| {
        let Some(op) = host_ops_repository::load(tx, op_id)? else {
            return Ok(None);
        };
        let before = match op.job_id.as_deref() {
            Some(job_id) => jobs_repository::load_job(tx, job_id)?.map(|job| job.state),
            None => None,
        };
        Ok(dispatch::apply_host_outcome(tx, &op, &now)?.map(|applied| (op.kind, before, applied)))
    });
    match applied {
        Ok(Some((kind, before, applied))) => {
            // Still under the Printer lock, so status-following can't read
            // the moved Job before it knows to wait.
            let moved = before != Some(applied.job.state);
            if moved && matches!(kind, HostOperationKind::Pause | HostOperationKind::Resume) {
                lock(&services.jobs.awaiting_status)
                    .insert(applied.job.id.clone(), applied.job.state);
            }
            publish(services, applied.change());
            Some(applied.job)
        }
        Ok(None) => None,
        Err(error) => {
            log_failure("jobs.applyHostOperationOutcomeFailed", &error);
            None
        }
    }
}

async fn on_host_operation<R: tauri::Runtime>(services: &RuntimeServices<R>, op: &HostOperation) {
    if let Some(job_id) = &op.job_id {
        let applied = apply_linked_job(services, &op.printer_id, &op.id).await;
        // D7: a Job entering `printing` pins its history job at once (a
        // quick print may already be over), and a succeeded cancel makes
        // the tracker check history now.
        let started = applied.is_some_and(|job| job.state == JobState::Printing)
            && op.kind == HostOperationKind::Start;
        let cancelled = op.kind == HostOperationKind::Cancel
            && op.state == crate::host_ops::HostOperationState::Succeeded;
        if started || cancelled {
            track(services, job_id).await;
        }
    }
    // An op resolving can lift `HOST_OPERATION_PENDING`, and a Job may
    // just have entered `awaitingStart`.
    on_printer(services, &op.printer_id).await;
}

/// D7: the Printer's `awaitingStart` Job, re-evaluated against live
/// status — republished when its `startBlockers` changed, and started
/// unattended when decision 1 allows.
async fn on_printer<R: tauri::Runtime>(services: &RuntimeServices<R>, printer_id: &str) {
    let job = services
        .storage
        .read(|connection| {
            Ok(jobs_repository::active_job_for_printer(
                connection, printer_id,
            ))
        })
        .ok()
        .and_then(Result::ok)
        .flatten();
    let Some(job) = job else {
        return;
    };
    if matches!(job.state, JobState::Printing | JobState::Paused) {
        follow_status(services, &job).await;
        return;
    }
    if job.state == JobState::Assigned {
        // A stage deferred while the Printer wasn't reachable yet goes
        // ahead once it is.
        stage_by_driver(services, &job.id).await;
        return;
    }
    if job.state != JobState::AwaitingStart {
        return;
    }
    let context = match StartContext::read(services, &job) {
        Ok(Some(context)) => context,
        Ok(None) => return,
        Err(error) => {
            log_failure("jobs.readStartContextFailed", &error);
            return;
        }
    };
    let blockers = context.blockers(&job);
    let changed = lock(&services.jobs.published_blockers).get(&job.id) != Some(&blockers);
    if changed {
        publish(
            services,
            QueueChange {
                jobs: vec![job.clone()],
                ..QueueChange::default()
            },
        );
    }
    maybe_start_unattended(services, &job, &context).await;
}

async fn on_awaiting_start<R: tauri::Runtime>(services: &RuntimeServices<R>) {
    for job in read_active(services) {
        if job.state == JobState::AwaitingStart {
            on_printer(services, &job.printer_id).await;
        }
    }
}

/// D7: one tracker poll of `job_id`, published.
async fn track<R: tauri::Runtime>(services: &RuntimeServices<R>, job_id: &str) {
    match tracker::check(services, job_id).await {
        Ok(Some(change)) => publish(services, change),
        Ok(None) => {}
        Err(error) => log_failure("jobs.trackHistoryFailed", &error),
    }
}

/// D7: a status for a `printing`/`paused` Job's Printer — progress and
/// `printing` ⇄ `paused` on its own file, and a poll at once (at most one
/// per `history_poll`) when it says our file ended.
async fn follow_status<R: tauri::Runtime>(services: &RuntimeServices<R>, job: &Job) {
    let check_now = match tracker::observe_status(services, job).await {
        Ok((change, check_now)) => {
            if let Some(change) = change {
                publish(services, change);
            }
            check_now
        }
        Err(error) => {
            log_failure("jobs.followStatusFailed", &error);
            false
        }
    };
    if !check_now {
        return;
    }
    let due = {
        let mut hinted = lock(&services.jobs.hinted);
        let now = std::time::Instant::now();
        let due = hinted
            .get(&job.id)
            .is_none_or(|last| now.duration_since(*last) >= services.jobs.timings.history_poll);
        if due {
            hinted.insert(job.id.clone(), now);
        }
        due
    };
    if due {
        track(services, &job.id).await;
    }
}

/// D3: a `printing`/`paused` Job whose `host_unreachable_since` just
/// crossed `unreachable_declare_after` is republished, once, so its
/// `allowedActions` offer `declareOutcome`.
fn republish_declare_crossings<R: tauri::Runtime>(services: &RuntimeServices<R>, active: Vec<Job>) {
    let now = services.jobs.now();
    let after = services.jobs.timings.unreachable_declare_after;
    let ids: HashSet<&str> = active.iter().map(|job| job.id.as_str()).collect();
    lock(&services.jobs.declare_offered).retain(|id| ids.contains(id.as_str()));
    lock(&services.jobs.hinted).retain(|id, _| ids.contains(id.as_str()));
    lock(&services.jobs.awaiting_status).retain(|id, _| ids.contains(id.as_str()));
    for job in active {
        let offered = super::state::may_declare_while_unreachable(&job, now, after);
        let known = lock(&services.jobs.declare_offered).contains(&job.id);
        if offered && !known {
            publish(
                services,
                QueueChange {
                    jobs: vec![job],
                    ..QueueChange::default()
                },
            );
        } else if !offered && known {
            lock(&services.jobs.declare_offered).remove(&job.id);
        }
    }
}

fn driver_operation_id() -> String {
    format!("drv-{}", uuid::Uuid::new_v4())
}

/// Decision 1: an unattended start, once. Never after a start was handed
/// off for this Job (D6: nothing re-sends a start by itself) or after a
/// recorded failure; the operator's Start is the way on from there.
async fn maybe_start_unattended<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    job: &Job,
    context: &StartContext,
) {
    if job.last_failure.is_some() || !context.may_start_unattended(job) {
        return;
    }
    if has_handed_off(services, &job.id, HostOperationKind::Start) {
        return;
    }
    if !lock(&services.jobs.starting).insert(job.id.clone()) {
        return;
    }
    let result = dispatch::start_job(
        services,
        driver_operation_id(),
        &job.id,
        PriorState::Ready,
        StartConfirmation::Unattended,
    )
    .await;
    match result {
        Ok(handoff) if !handoff.replayed => publish(services, handoff.change()),
        Ok(_) => {}
        Err(error) if is_not_yet(&error) => {}
        Err(error) => {
            record_refusal(
                services,
                &job.id,
                JobState::AwaitingStart,
                HostOperationKind::Start,
                &error,
            )
            .await
        }
    }
    lock(&services.jobs.starting).remove(&job.id);
}

/// D7 "Staging": the driver stages an `assigned` Job once — only one that
/// has no `lastFailure` and never handed off an upload (R2).
async fn stage_by_driver<R: tauri::Runtime>(services: &RuntimeServices<R>, job_id: &str) {
    let Some(job) = load_job(services, job_id) else {
        return;
    };
    if job.state != JobState::Assigned
        || job.last_failure.is_some()
        || has_handed_off(services, job_id, HostOperationKind::Upload)
    {
        return;
    }
    match dispatch::stage_job(services, driver_operation_id(), job_id).await {
        Ok(handoff) if !handoff.replayed => publish(services, handoff.change()),
        Ok(_) => {}
        Err(error) if is_not_yet(&error) => {}
        Err(error) => {
            record_refusal(
                services,
                job_id,
                JobState::Assigned,
                HostOperationKind::Upload,
                &error,
            )
            .await
        }
    }
}

/// Which P6 refusals of a driver handoff mean "not yet" rather than "no".
///
/// A deferral records nothing: the driver tries again on the Printer's
/// next status change, the next inventory change, or the next poll. P6
/// refuses these before it writes any row, so trying again never re-sends
/// anything:
/// - `PRINTER_UNREACHABLE`: the Printer is not Online yet (still
///   `Connecting` after a restart, offline, or in error);
/// - `TIMEOUT`: a pre-check's host read timed out;
/// - `HOST_OPERATION_PENDING`: another Host Operation on this Printer
///   hasn't resolved.
///
/// Every other refusal records `lastFailure{refused}`, and the driver
/// never retries that handoff for this Job. Examples are
/// `CAPABILITY_UNSUPPORTED`, `UNSUPPORTED_ADAPTER`, `CREDENTIAL_REQUIRED`,
/// `AUTHENTICATION_FAILED`, `START_NOT_ALLOWED`,
/// `START_PRECONDITION_CHANGED`, `JOB_START_BLOCKED`,
/// `STAGED_ARTIFACT_INVALID`, and `VALIDATION`. The operator's own
/// command is how that Job moves on.
fn is_not_yet(error: &CommandError) -> bool {
    matches!(
        error.code,
        ErrorCode::PrinterUnreachable | ErrorCode::Timeout | ErrorCode::HostOperationPending
    )
}

/// D7: a driver handoff P6 refused before writing any row. Records
/// `lastFailure = { kind: "refused", code, message }` in its own
/// transaction under the Printer lock — only while the Job is still in
/// `expected`, with no `lastFailure`, and still never handed off a `kind`
/// op (a concurrent handoff that won the race is not a refusal). No state
/// change and no event row; `revision` goes up and the Job is published.
async fn record_refusal<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    job_id: &str,
    expected: JobState,
    kind: HostOperationKind,
    error: &CommandError,
) {
    let Some(job) = load_job(services, job_id) else {
        return;
    };
    let printer_lock = services.host_ops.printer_lock(&job.printer_id);
    let _serialized = printer_lock.lock().await;
    let now = now_rfc3339();
    let recorded = services.storage.write_repo(|tx| {
        let Some(job) = jobs_repository::load_job(tx, job_id)? else {
            return Ok(None);
        };
        let handed_off: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM host_operations WHERE job_id = ?1 AND kind = ?2)",
            rusqlite::params![job_id, crate::spools::encode_enum(kind)],
            |row| row.get(0),
        )?;
        if job.state != expected || job.last_failure.is_some() || handed_off {
            return Ok(None);
        }
        jobs_repository::update_columns(
            tx,
            job_id,
            JobChange {
                last_failure: Some(JobFailure::Refused {
                    at: now.clone(),
                    code: error.code,
                    message: error.message.clone(),
                }),
                ..JobChange::default()
            },
            &now,
        )
        .map(Some)
    });
    match recorded {
        Ok(Some(job)) => publish(
            services,
            QueueChange {
                jobs: vec![job],
                ..QueueChange::default()
            },
        ),
        Ok(None) => {}
        Err(error) => log_failure("jobs.recordRefusedHandoffFailed", &error),
    }
}

/// D7 `Lagged`, the poll interval, and the first pass: every active Job
/// re-read from storage. A Job whose active op resolved catches up; an
/// `assigned` Job that never staged is staged (R2); an `awaitingStart`
/// Job is re-evaluated; a `printing`/`paused` Job's history is polled.
///
/// A stopped runtime (a crash, in tests) stops between Jobs: it finishes
/// the Job in hand and does nothing more.
async fn resync<R: tauri::Runtime>(services: &RuntimeServices<R>) {
    for job in read_active(services) {
        if services.jobs.stopping() {
            return;
        }
        if let Some(op_id) = &job.active_host_operation_id {
            apply_linked(services, &job.printer_id, op_id).await;
        }
    }
    let active = read_active(services);
    for job in &active {
        if services.jobs.stopping() {
            return;
        }
        match job.state {
            JobState::Assigned => stage_by_driver(services, &job.id).await,
            JobState::AwaitingStart => on_printer(services, &job.printer_id).await,
            // The tracker's history poll (D7), and its first pass after a
            // restart (R9–R11).
            JobState::Printing | JobState::Paused => track(services, &job.id).await,
            _ => {}
        }
    }
    if services.jobs.stopping() {
        return;
    }
    // A poll can end a Job or set `host_unreachable_since`: re-read.
    let still_active = if active
        .iter()
        .any(|job| matches!(job.state, JobState::Printing | JobState::Paused))
    {
        read_active(services)
    } else {
        active
    };
    republish_declare_crossings(services, still_active);
    services
        .jobs
        .resyncs
        .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
}
