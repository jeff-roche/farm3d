//! P8 D2 "Runtime and wakes": the Attention runtime — [`AttentionTimings`],
//! [`AttentionServices`] (held in `RuntimeServices.attention`), and the
//! projector task `start_attention_runtime` spawns.
//!
//! The task wakes on the Printer status broadcast, the queue broadcast
//! (`QueueStream::subscribe_changes`), the inventory broadcast, a
//! [`AttentionServices::poke`] (Printer lifecycle and Connection changes,
//! alert defaults), the next offline-grace deadline, and a safety tick.
//! Every wake means one full pass ([`run_pass`]); `RecvError::Lagged` is
//! just another wake. It drains every ready wake before a pass, and runs
//! passes at most once per `pass_min_interval`, so a burst of wakes costs
//! one trailing pass.
//!
//! After each committed pass it publishes the changed rows on the
//! `attention` stream and hands the pass's capture intents and notify
//! candidates on ([`AttentionServices::subscribe_applied`]); it never
//! waits for either.

use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::AppHandle;
use tokio::sync::broadcast::error::{RecvError, TryRecvError};
use tokio::sync::{broadcast, oneshot, watch, Notify};

use crate::host_ops::{Clock, SystemClock};
use crate::persistence::RepositoryError;
use crate::RuntimeServices;

use super::events::AttentionStream;
use super::observe::PrinterWatch;
use super::projector::{self, AppliedChanges, LiveInputs};
use super::AttentionOrigin;

/// D2 "Coalescing". `default()` holds the production values (1 s, 60 s);
/// tests inject a zero interval.
#[derive(Clone, Copy, Debug)]
pub struct AttentionTimings {
    /// The least time between the starts of two passes.
    pub pass_min_interval: Duration,
    /// A full pass at least this often, whatever else wakes the task.
    pub safety_tick: Duration,
}

impl Default for AttentionTimings {
    fn default() -> Self {
        Self {
            pass_min_interval: Duration::from_secs(1),
            safety_tick: Duration::from_secs(60),
        }
    }
}

/// How many committed passes' hand-offs a slow subscriber may fall behind.
const APPLIED_CAPACITY: usize = 64;

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Counts one live runtime task until dropped.
struct TaskGuard(Arc<AtomicUsize>);

impl Drop for TaskGuard {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// The Attention runtime's state, held in `RuntimeServices.attention`.
pub struct AttentionServices<R: tauri::Runtime> {
    pub timings: AttentionTimings,
    /// The projector's time source: the offline grace, and every P8
    /// timestamp. Injectable so no test waits out a grace.
    clock: Arc<dyn Clock>,
    /// The `attention` event stream's id and sequence.
    pub stream: AttentionStream,
    app: OnceLock<AppHandle<R>>,
    started: OnceLock<()>,
    /// D2: when the supervisors started (`start_attention_runtime`);
    /// `None` until then, as in the backfill.
    supervisors_started_at: OnceLock<DateTime<Utc>>,
    wake: Arc<Notify>,
    /// The offline watch (D2 "The offline watch"). Never persisted.
    watch: Mutex<PrinterWatch>,
    /// Passes never overlap.
    pass_lock: Mutex<()>,
    next_deadline: Mutex<Option<DateTime<Utc>>>,
    /// The startup backfill's changes, published when the runtime starts.
    backfilled: Mutex<Option<AppliedChanges>>,
    applied: broadcast::Sender<Arc<AppliedChanges>>,
    barriers: Mutex<Vec<oneshot::Sender<()>>>,
    held: watch::Sender<bool>,
    stop: watch::Sender<bool>,
    passes: AtomicU64,
    lagged: AtomicU64,
    tasks: Arc<AtomicUsize>,
}

impl<R: tauri::Runtime> Default for AttentionServices<R> {
    fn default() -> Self {
        Self::new(AttentionTimings::default())
    }
}

impl<R: tauri::Runtime> AttentionServices<R> {
    pub fn new(timings: AttentionTimings) -> Self {
        Self::with_clock(timings, Arc::new(SystemClock))
    }

    /// [`AttentionServices::new`] with the projector's clock injected.
    pub fn with_clock(timings: AttentionTimings, clock: Arc<dyn Clock>) -> Self {
        Self {
            timings,
            clock,
            stream: AttentionStream::default(),
            app: OnceLock::new(),
            started: OnceLock::new(),
            supervisors_started_at: OnceLock::new(),
            wake: Arc::new(Notify::new()),
            watch: Mutex::new(PrinterWatch::default()),
            pass_lock: Mutex::new(()),
            next_deadline: Mutex::new(None),
            backfilled: Mutex::new(None),
            applied: broadcast::channel(APPLIED_CAPACITY).0,
            barriers: Mutex::new(Vec::new()),
            held: watch::channel(false).0,
            stop: watch::channel(false).0,
            passes: AtomicU64::new(0),
            lagged: AtomicU64::new(0),
            tasks: Arc::default(),
        }
    }

    /// The projector's "now" (every P8 timestamp).
    pub fn now(&self) -> DateTime<Utc> {
        self.clock.now()
    }

    /// The app the runtime publishes to, once `start_attention_runtime`
    /// ran.
    pub(crate) fn app(&self) -> Option<&AppHandle<R>> {
        self.app.get()
    }

    /// Asks for a full pass (D2 "Wakes": Printer create, edit, archive,
    /// unarchive, delete, import, Connection changes, alert defaults).
    /// Pokes coalesce; one before the runtime starts is kept for it.
    pub fn poke(&self) {
        self.wake.notify_one();
    }

    /// The startup backfill's changes, published once the runtime starts.
    pub fn set_backfilled(&self, changes: AppliedChanges) {
        let mut backfilled = lock(&self.backfilled);
        match backfilled.as_mut() {
            Some(existing) => {
                existing.events.extend(changes.events);
                existing.incidents.extend(changes.incidents);
            }
            None => *backfilled = Some(changes),
        }
    }

    /// Every committed live pass's changes, after it published them (the
    /// camera and notification hand-offs subscribe here).
    pub fn subscribe_applied(&self) -> broadcast::Receiver<Arc<AppliedChanges>> {
        self.applied.subscribe()
    }

    /// Test hook: a receiver that gets `()` once the projector task has
    /// finished a whole pass that started after this call. It errors if the
    /// runtime stops first.
    pub fn pass_barrier(&self) -> oneshot::Receiver<()> {
        let (done, receiver) = oneshot::channel();
        lock(&self.barriers).push(done);
        self.poke();
        receiver
    }

    /// Test hook: the task finishes the step it is in, then waits (its
    /// receivers keep filling, and may lag) until [`release`](Self::release).
    pub fn hold(&self) {
        self.held.send_replace(true);
    }

    /// Test hook: see [`hold`](Self::hold).
    pub fn release(&self) {
        self.held.send_replace(false);
    }

    /// Test hook: how many passes have committed.
    pub fn passes(&self) -> u64 {
        self.passes.load(Ordering::SeqCst)
    }

    /// Test hook: how many times a wake receiver reported `Lagged`.
    pub fn lagged(&self) -> u64 {
        self.lagged.load(Ordering::SeqCst)
    }

    /// Test hook: stops the projector task, as a crash would.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }

    /// Test hook: the runtime's live tasks.
    pub fn running_tasks(&self) -> usize {
        self.tasks.load(Ordering::SeqCst)
    }

    fn task_guard(&self) -> TaskGuard {
        self.tasks.fetch_add(1, Ordering::SeqCst);
        TaskGuard(Arc::clone(&self.tasks))
    }

    /// Publishes a committed change on the `attention` stream (Events,
    /// then Incidents), and hands it on. Nothing when there is no app yet.
    pub(crate) fn publish(&self, changes: &AppliedChanges) {
        if changes.is_empty() {
            return;
        }
        if let Some(app) = self.app.get() {
            self.stream
                .publish_change(app, &changes.event_rows(), &changes.incidents, &[]);
        }
        // An error only means nobody is subscribed yet.
        let _ = self.applied.send(Arc::new(changes.clone()));
    }
}

/// One full live pass for `services` (D2 "The pass"): the supervisor's
/// statuses, the watch, then one IMMEDIATE transaction; after commit, the
/// changes are published. Passes never overlap. The task calls it on every
/// wake; tests may call it directly.
pub fn run_pass<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
) -> Result<AppliedChanges, RepositoryError> {
    let attention = &services.attention;
    let _serialized = lock(&attention.pass_lock);
    let statuses = services.manager.status_facts();
    let now = attention.now();
    let live = LiveInputs {
        statuses: &statuses,
        supervisors_started_at: attention.supervisors_started_at.get().copied(),
        catalog: Some(&services.catalog),
    };
    let outcome = {
        let mut watch = lock(&attention.watch);
        projector::run(&services.storage, &live, &mut watch, AttentionOrigin::Live, now)?
    };
    *lock(&attention.next_deadline) = outcome.next_deadline;
    attention.passes.fetch_add(1, Ordering::SeqCst);
    attention.publish(&outcome.changes);
    Ok(outcome.changes)
}

/// Starts the projector for `services` (`start_attention_runtime`). A
/// second call does nothing. It records `supervisors_started_at`,
/// subscribes to every wake source, then spawns the task, whose first
/// action is a full pass: anything published before it subscribed is
/// covered by that pass.
pub fn start<R: tauri::Runtime>(services: &Arc<RuntimeServices<R>>, app: &AppHandle<R>) {
    let attention = &services.attention;
    let _ = attention.app.set(app.clone());
    if attention.started.set(()).is_err() {
        return;
    }
    let _ = attention.supervisors_started_at.set(attention.now());
    let task = Projector {
        _task: attention.task_guard(),
        services: Arc::downgrade(services),
        wake: Arc::clone(&attention.wake),
        statuses: services.manager.subscribe_status(),
        queue: services.queue_stream.subscribe_changes(),
        inventory: services.inventory_changes.subscribe(),
        held: attention.held.subscribe(),
        stop: attention.stop.subscribe(),
        min_interval: attention.timings.pass_min_interval,
        safety_tick: attention.timings.safety_tick,
    };
    tauri::async_runtime::spawn(task.run());
}

struct Projector<R: tauri::Runtime> {
    _task: TaskGuard,
    services: Weak<RuntimeServices<R>>,
    wake: Arc<Notify>,
    statuses: broadcast::Receiver<String>,
    queue: broadcast::Receiver<crate::queue::events::QueueChangeIds>,
    inventory: broadcast::Receiver<crate::spools::events::InventoryChange>,
    held: watch::Receiver<bool>,
    stop: watch::Receiver<bool>,
    min_interval: Duration,
    safety_tick: Duration,
}

/// Drains a broadcast receiver's ready messages. Returns whether it
/// lagged, and whether its sender is gone.
fn drain<T: Clone>(receiver: &mut broadcast::Receiver<T>) -> (bool, bool) {
    let mut lagged = false;
    loop {
        match receiver.try_recv() {
            Ok(_) => {}
            Err(TryRecvError::Lagged(_)) => lagged = true,
            Err(TryRecvError::Empty) => return (lagged, false),
            Err(TryRecvError::Closed) => return (lagged, true),
        }
    }
}

enum Wake {
    Pass,
    Stop,
}

impl<R: tauri::Runtime> Projector<R> {
    fn stopping(&self) -> bool {
        *self.stop.borrow()
    }

    async fn run(mut self) {
        {
            let Some(services) = self.services.upgrade() else {
                return;
            };
            let backfilled = lock(&services.attention.backfilled).take();
            if let Some(changes) = backfilled {
                if let Some(app) = services.attention.app() {
                    services
                        .attention
                        .stream
                        .publish_change(app, &changes.event_rows(), &changes.incidents, &[]);
                }
            }
        }
        let mut last_pass: Option<tokio::time::Instant> = None;
        loop {
            if !self.wait_while_held().await || !self.pace(last_pass).await {
                return;
            }
            let lagged = self.drain_ready();
            let Some(services) = self.services.upgrade() else {
                return;
            };
            if lagged > 0 {
                services.attention.lagged.fetch_add(lagged, Ordering::SeqCst);
            }
            let waiting = std::mem::take(&mut *lock(&services.attention.barriers));
            last_pass = Some(tokio::time::Instant::now());
            if let Err(error) = run_pass(&services) {
                // Repository errors carry no credential, so neither does this.
                eprintln!("farm3d: attention projector: a pass failed: {error:?}");
            }
            for done in waiting {
                let _ = done.send(());
            }
            let sleep = self.until_next_timer(&services);
            drop(services);
            match self.next_wake(sleep).await {
                Wake::Pass => {}
                Wake::Stop => return,
            }
        }
    }

    /// Waits while a test holds the task. `false` when it should stop.
    async fn wait_while_held(&mut self) -> bool {
        while *self.held.borrow() {
            tokio::select! {
                changed = self.held.changed() => if changed.is_err() { return false; },
                changed = self.stop.changed() => if changed.is_err() || self.stopping() { return false; },
            }
        }
        !self.stopping()
    }

    /// Waits out the rest of `pass_min_interval` since the last pass.
    /// `false` when it should stop.
    async fn pace(&mut self, last_pass: Option<tokio::time::Instant>) -> bool {
        let Some(last) = last_pass else {
            return !self.stopping();
        };
        let next = last + self.min_interval;
        if tokio::time::Instant::now() < next {
            tokio::select! {
                _ = tokio::time::sleep_until(next) => {}
                changed = self.stop.changed() => if changed.is_err() || self.stopping() { return false; },
            }
        }
        !self.stopping()
    }

    /// Drains every ready wake (a burst costs one pass). Returns how many
    /// receivers lagged.
    fn drain_ready(&mut self) -> u64 {
        let mut lagged = 0;
        for (did_lag, _) in [
            drain(&mut self.statuses),
            drain(&mut self.queue),
            drain(&mut self.inventory),
        ] {
            if did_lag {
                lagged += 1;
            }
        }
        lagged
    }

    /// The sleep before the next timer wake: the next offline-grace
    /// deadline, or the safety tick, whichever is sooner.
    fn until_next_timer(&self, services: &RuntimeServices<R>) -> Duration {
        let deadline = *lock(&services.attention.next_deadline);
        let now = services.attention.now();
        deadline
            .map(|deadline| (deadline - now).to_std().unwrap_or(Duration::ZERO))
            .map_or(self.safety_tick, |until| until.min(self.safety_tick))
    }

    async fn next_wake(&mut self, sleep: Duration) -> Wake {
        let timer = tokio::time::sleep(sleep);
        tokio::pin!(timer);
        loop {
            tokio::select! {
                changed = self.stop.changed() => {
                    if changed.is_err() || self.stopping() {
                        return Wake::Stop;
                    }
                }
                status = self.statuses.recv() => return self.on_recv(status),
                change = self.queue.recv() => return self.on_recv(change),
                change = self.inventory.recv() => return self.on_recv(change),
                _ = self.wake.notified() => return Wake::Pass,
                _ = &mut timer => return Wake::Pass,
            }
        }
    }

    fn on_recv<T>(&self, received: Result<T, RecvError>) -> Wake {
        match received {
            Ok(_) => Wake::Pass,
            Err(RecvError::Lagged(_)) => {
                if let Some(services) = self.services.upgrade() {
                    services.attention.lagged.fetch_add(1, Ordering::SeqCst);
                }
                Wake::Pass
            }
            // A wake source went away with the runtime.
            Err(RecvError::Closed) => Wake::Stop,
        }
    }
}
