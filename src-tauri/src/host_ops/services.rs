//! P6's Host Operation services (spec "Module layout"): the clock and
//! timings, the capability factory, the test fault points, and
//! [`HostOperationServices`] itself. Re-exported from [`super`], so every
//! public path is unchanged.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use tauri::AppHandle;
use zeroize::Zeroizing;

use crate::connections::capabilities::{
    capabilities_for, ArtifactStaging, CapabilityKey, CapabilityState, HostFacts, HostStateQuery,
    InconclusiveReason, PrintControl, PrinterCapabilities,
};
use crate::connections::credentials::CredentialStore;
use crate::connections::supervisor::ConnectionManager;
use crate::connections::{ConnectionConfig, ConnectionState};
use crate::contracts::command::CommandError;
use crate::library::content::ContentStore;
use crate::persistence::{RepositoryError, Storage};
use crate::printers::repository::PrinterRepository;
use crate::printers::StoredPrinter;

use super::{
    events, format_time, log_commit_failure, reconciler, repository, wire, HostOperation,
    HostOperationEndpoint, HostOperationState,
};

// --- D5 "When reconciliation runs": clock and timings --------------------

/// The time source the reconciler compares against (the settle period) and
/// that startup recovery stamps. Injectable so no test waits in real time.
pub trait Clock: Send + Sync {
    fn now(&self) -> DateTime<Utc>;
}

pub struct SystemClock;

impl Clock for SystemClock {
    fn now(&self) -> DateTime<Utc> {
        Utc::now()
    }
}

/// D5/D10's host-ops constants. `default()` holds the production values;
/// tests inject short ones. The executor and reconciler read the
/// verification window from here, never from the adapter.
#[derive(Clone, Copy, Debug)]
pub struct HostOpsTimings {
    /// `SETTLE_PERIOD`, counted from `uncertain_since`.
    pub settle_period: Duration,
    /// `START_SKEW_TOLERANCE`.
    pub start_skew_tolerance: Duration,
    /// `HISTORY_QUERY_LIMIT`.
    pub history_query_limit: u32,
    /// `CONTROL_VERIFY_WINDOW` and how often it polls `host_job_state`.
    pub verify_window: Duration,
    pub verify_poll_interval: Duration,
    /// The retry delay after `step` inconclusive automatic attempts.
    pub backoff: fn(u32) -> Duration,
}

impl Default for HostOpsTimings {
    fn default() -> Self {
        Self {
            settle_period: Duration::from_secs(60),
            start_skew_tolerance: Duration::from_secs(30),
            history_query_limit: 50,
            verify_window: Duration::from_secs(10),
            verify_poll_interval: Duration::from_millis(500),
            backoff: crate::connections::supervisor::backoff_delay,
        }
    }
}

// --- capability objects -----------------------------------------------------

/// Where the executor and reconciler get their capability objects and the
/// capability matrix they gate on. Production uses the adapter registry's
/// builders (default timings) and `capabilities_for`; tests inject one with
/// short `MoonrakerTimings`.
pub trait CapabilityFactory: Send + Sync {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities;
    fn staging(
        &self,
        config: &ConnectionConfig,
        key: Option<Zeroizing<String>>,
    ) -> Option<Box<dyn ArtifactStaging>>;
    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>>;
    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>>;
}

/// The production factory: the adapter registry (D1, D6).
pub struct RegistryCapabilityFactory;

impl CapabilityFactory for RegistryCapabilityFactory {
    fn capabilities(
        &self,
        printer: &StoredPrinter,
        host_facts: Option<&HostFacts>,
    ) -> PrinterCapabilities {
        capabilities_for(printer, host_facts)
    }

    fn staging(
        &self,
        config: &ConnectionConfig,
        key: Option<Zeroizing<String>>,
    ) -> Option<Box<dyn ArtifactStaging>> {
        crate::connections::adapters::descriptor(&config.kind)
            .and_then(|descriptor| descriptor.staging)
            .map(|build| build(config, key))
    }

    fn control(
        &self,
        config: &ConnectionConfig,
        key: Option<Zeroizing<String>>,
    ) -> Option<Box<dyn PrintControl>> {
        crate::connections::adapters::descriptor(&config.kind)
            .and_then(|descriptor| descriptor.control)
            .map(|build| build(config, key))
    }

    fn host_state(
        &self,
        config: &ConnectionConfig,
        key: Option<Zeroizing<String>>,
    ) -> Option<Box<dyn HostStateQuery>> {
        crate::connections::adapters::descriptor(&config.kind)
            .and_then(|descriptor| descriptor.host_state)
            .map(|build| build(config, key))
    }
}

// --- test fault points (spec "Module layout", `executor.rs`) ---------------

/// Where the executor can be stopped, as a crash or panic would stop it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaultPoint {
    BeforeMarkSent,
    AfterMarkSentBeforeSend,
    AfterSend,
    AfterResponseBeforeCommit,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum FaultAction {
    /// The executor stops without committing anything more (a crash; a
    /// restart then recovers the row).
    Crash,
    /// The executor panics.
    Panic,
}

/// A test hook run once, just before the next write-ahead transaction.
type WriteAheadHook = Box<dyn FnOnce() + Send>;

/// Makes the next `mark_sent` fail.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MarkSentFault {
    /// The executor then commits `failed { neverSent }`.
    Fails,
    /// And that commit fails too, so the row is left for startup recovery.
    FailsAndFailureCommitFails,
}

/// Retry scheduling for one Printer: a newer `generation` cancels every
/// older timer; `step` is the backoff index.
///
/// Ruling R20: the spec says the retry comes "after
/// `supervisor::backoff_delay(attempts)`" and also that "Check again"
/// (`reconcile_host_operation`) "resets the backoff". The persisted
/// `attempts` can't be reset (D8 reads it), so the backoff index is this
/// in-memory per-Printer `step` instead: it counts automatic retries,
/// and resets to 0 on "Check again" and whenever the Printer comes Online.
#[derive(Default)]
struct RetryState {
    generation: u64,
    step: u32,
}

/// A Printer's host facts as last read, with the endpoint they came from
/// and when (`observedAt`).
#[derive(Clone)]
struct CachedHostFacts {
    endpoint: HostOperationEndpoint,
    facts: HostFacts,
    observed_at: String,
}

/// P6's Host Operation services (spec "Module layout"): the repository's
/// callers, the executor, the reconciler, the `hostOperations` stream, the
/// host-facts cache, the per-Printer locks, and the clock.
pub struct HostOperationServices<R: tauri::Runtime> {
    pub stream: events::HostOperationsStream,
    pub(crate) storage: Arc<Storage>,
    pub(crate) content: Arc<ContentStore>,
    pub(crate) manager: Arc<ConnectionManager<R>>,
    pub(crate) factory: Arc<dyn CapabilityFactory>,
    pub(crate) clock: Arc<dyn Clock>,
    pub(crate) timings: HostOpsTimings,
    app: OnceLock<AppHandle<R>>,
    credentials: OnceLock<Arc<CredentialStore>>,
    started: OnceLock<()>,
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Keyed by Printer id. Each entry carries the endpoint it was read
    /// from, and applies only while the Printer still names that endpoint.
    host_facts: Mutex<HashMap<String, CachedHostFacts>>,
    retries: Mutex<HashMap<String, RetryState>>,
    faults: Mutex<Vec<(FaultPoint, FaultAction, SyncSender<()>)>>,
    mark_sent_fault: Mutex<Option<(MarkSentFault, SyncSender<()>)>>,
    before_write_ahead: Mutex<Option<WriteAheadHook>>,
    /// How many timer-scheduled attempts actually ran (diagnostics, tests).
    retry_attempts_run: AtomicUsize,
    /// How many retry timers were scheduled (diagnostics, tests).
    retry_timers_scheduled: AtomicUsize,
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn signal() -> (SyncSender<()>, Receiver<()>) {
    std::sync::mpsc::sync_channel(1)
}

impl<R: tauri::Runtime> HostOperationServices<R> {
    pub fn new(
        storage: Arc<Storage>,
        content: Arc<ContentStore>,
        manager: Arc<ConnectionManager<R>>,
        factory: Arc<dyn CapabilityFactory>,
        clock: Arc<dyn Clock>,
        timings: HostOpsTimings,
    ) -> Self {
        Self {
            stream: events::HostOperationsStream::default(),
            storage,
            content,
            manager,
            factory,
            clock,
            timings,
            app: OnceLock::new(),
            credentials: OnceLock::new(),
            started: OnceLock::new(),
            locks: Mutex::new(HashMap::new()),
            host_facts: Mutex::new(HashMap::new()),
            retries: Mutex::new(HashMap::new()),
            faults: Mutex::new(Vec::new()),
            mark_sent_fault: Mutex::new(None),
            before_write_ahead: Mutex::new(None),
            retry_attempts_run: AtomicUsize::new(0),
            retry_timers_scheduled: AtomicUsize::new(0),
        }
    }

    /// The production services: the registry, the system clock, and the
    /// default timings.
    pub fn production(
        storage: Arc<Storage>,
        content: Arc<ContentStore>,
        manager: Arc<ConnectionManager<R>>,
    ) -> Self {
        Self::new(
            storage,
            content,
            manager,
            Arc::new(RegistryCapabilityFactory),
            Arc::new(SystemClock),
            HostOpsTimings::default(),
        )
    }

    /// Attaches the app (for events) and the credential store. The first
    /// call wins; later calls do nothing.
    pub fn attach(&self, app: &AppHandle<R>, credentials: &Arc<CredentialStore>) {
        let _ = self.app.set(app.clone());
        let _ = self.credentials.set(Arc::clone(credentials));
    }

    /// Startup (spec "Module layout": after `restore_persisted_connections`):
    /// attaches, installs the Online hook, refreshes the host facts of every
    /// Printer already Online, and makes one attempt per `uncertain` row. A
    /// second call does nothing.
    pub fn start(self: &Arc<Self>, app: &AppHandle<R>, credentials: &Arc<CredentialStore>) {
        self.attach(app, credentials);
        if self.started.set(()).is_err() {
            return;
        }
        let weak: Weak<Self> = Arc::downgrade(self);
        self.manager
            .set_online_hook(Arc::new(move |printer_id: &str| {
                if let Some(services) = weak.upgrade() {
                    services.on_online(printer_id.to_string());
                }
            }));
        let online: Vec<String> = self
            .manager
            .statuses()
            .into_iter()
            .filter(|(_, status)| status.connection_state == ConnectionState::Online)
            .map(|(id, _)| id)
            .collect();
        let uncertain = self
            .storage
            .read(|connection| Ok(repository::list_unresolved(connection, None)))
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default()
            .into_iter()
            .filter(|row| row.state == HostOperationState::Uncertain)
            .map(|row| row.id)
            .collect::<Vec<_>>();
        let services = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            for printer_id in online {
                services.refresh_host_facts(&printer_id).await;
            }
            for id in uncertain {
                let _ = reconciler::attempt(&services, &id).await;
            }
        });
    }

    /// Emits one event per row. Call only after the rows' changes committed.
    pub(crate) fn publish(&self, rows: &[HostOperation]) {
        if let Some(app) = self.app.get() {
            self.stream.publish(app, rows);
        }
    }

    /// D5 "Serialization": the one lock per Printer that write commands and
    /// reconcile attempts share.
    pub(crate) fn printer_lock(&self, printer_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(lock(&self.locks).entry(printer_id.to_string()).or_default())
    }

    pub(crate) fn load(&self, id: &str) -> Result<Option<HostOperation>, RepositoryError> {
        self.storage
            .read(|connection| Ok(repository::load(connection, id)))
            .map_err(RepositoryError::Storage)?
    }

    pub(crate) fn load_printer(
        &self,
        printer_id: &str,
    ) -> Result<Option<StoredPrinter>, RepositoryError> {
        PrinterRepository::new(Arc::clone(&self.storage))
            .get(printer_id)
            .map_err(RepositoryError::Storage)
    }

    /// D6 over the cached host facts, with their read time.
    pub fn capabilities(&self, printer: &StoredPrinter) -> PrinterCapabilities {
        // Facts read from another endpoint (the Connection changed since,
        // or a Printer came back under the same id) describe another host.
        let endpoint = printer.connection.as_ref().map(HostOperationEndpoint::of);
        let cached = lock(&self.host_facts)
            .get(&printer.id)
            .filter(|cached| Some(&cached.endpoint) == endpoint.as_ref())
            .cloned();
        let mut capabilities = self
            .factory
            .capabilities(printer, cached.as_ref().map(|cached| &cached.facts));
        if capabilities.host_facts.is_some() {
            capabilities.observed_at = cached.map(|cached| cached.observed_at);
        }
        capabilities
    }

    /// `CAPABILITY_UNSUPPORTED` when `key` is unsupported for `printer`.
    pub(crate) fn unsupported(
        &self,
        printer: &StoredPrinter,
        key: CapabilityKey,
    ) -> Option<CommandError> {
        match &self.capabilities(printer).capabilities[key] {
            CapabilityState::Supported { .. } => None,
            CapabilityState::Unsupported { reason, detail } => {
                Some(CommandError::capability_unsupported(
                    &printer.id,
                    &wire(key),
                    &wire(*reason),
                    detail,
                ))
            }
        }
    }

    /// The Printer's current credential. `Err` when it names one the store
    /// can't produce.
    pub(crate) fn credential(
        &self,
        printer: &StoredPrinter,
    ) -> Result<Option<Zeroizing<String>>, ()> {
        let Some(reference) = printer
            .connection
            .as_ref()
            .and_then(|connection| connection.credential_ref.as_deref())
        else {
            return Ok(None);
        };
        let store = self.credentials.get().ok_or(())?;
        match store.get(reference) {
            Ok(Some(secret)) => Ok(Some(Zeroizing::new(secret))),
            _ => Err(()),
        }
    }

    /// A row's recorded endpoint (D5: never the Printer's current one) with
    /// the Printer's current credential reference and `use_tls`. The
    /// endpoint record carries no TLS flag; taking it from the Connection
    /// means a TLS Connection (refused everywhere today) is refused by the
    /// adapter, never spoken to over plain HTTP.
    pub(crate) fn endpoint_config(
        &self,
        endpoint: &HostOperationEndpoint,
        printer: &StoredPrinter,
    ) -> ConnectionConfig {
        let connection = printer.connection.as_ref();
        ConnectionConfig {
            kind: endpoint.kind.clone(),
            host: endpoint.host.clone(),
            port: endpoint.port,
            use_tls: connection.is_some_and(|connection| connection.use_tls),
            credential_ref: connection.and_then(|connection| connection.credential_ref.clone()),
        }
    }

    /// How many timer-scheduled retries have run an attempt so far.
    pub fn retry_attempts_run(&self) -> usize {
        self.retry_attempts_run.load(Ordering::SeqCst)
    }

    /// How many retry timers have been scheduled so far.
    pub fn retry_timers_scheduled(&self) -> usize {
        self.retry_timers_scheduled.load(Ordering::SeqCst)
    }

    pub(crate) fn is_online(&self, printer_id: &str) -> bool {
        self.manager
            .statuses()
            .get(printer_id)
            .is_some_and(|status| status.connection_state == ConnectionState::Online)
    }

    // --- faults -------------------------------------------------------------

    /// Test hook: stops the next executor to reach `point` with `action`.
    /// The receiver gets a message when the point is reached.
    pub fn inject_fault(&self, point: FaultPoint, action: FaultAction) -> Receiver<()> {
        let (fired, receiver) = signal();
        lock(&self.faults).push((point, action, fired));
        receiver
    }

    /// Test hook: makes the next `mark_sent` fail. The receiver gets a
    /// message once the executor has handled the failure.
    pub fn inject_mark_sent_fault(&self, fault: MarkSentFault) -> Receiver<()> {
        let (fired, receiver) = signal();
        *lock(&self.mark_sent_fault) = Some((fault, fired));
        receiver
    }

    /// Whether the executor must stop at `point`. Panics for
    /// `FaultAction::Panic` (without running the panic hook, so tests stay
    /// quiet).
    pub(crate) fn hit(&self, point: FaultPoint) -> bool {
        let fault = {
            let mut faults = lock(&self.faults);
            faults
                .iter()
                .position(|(at, _, _)| *at == point)
                .map(|index| faults.remove(index))
        };
        let Some((_, action, fired)) = fault else {
            return false;
        };
        let _ = fired.try_send(());
        match action {
            FaultAction::Crash => true,
            FaultAction::Panic => std::panic::resume_unwind(Box::new("injected executor panic")),
        }
    }

    /// Test hook: runs `hook` once, after the next write command's
    /// pre-checks and just before its write-ahead transaction, so a test
    /// can change the Printer in that window.
    pub fn inject_before_write_ahead(&self, hook: impl FnOnce() + Send + 'static) {
        *lock(&self.before_write_ahead) = Some(Box::new(hook));
    }

    pub(crate) fn run_before_write_ahead(&self) {
        let hook = lock(&self.before_write_ahead).take();
        if let Some(hook) = hook {
            hook();
        }
    }

    pub(crate) fn take_mark_sent_fault(&self) -> Option<(MarkSentFault, SyncSender<()>)> {
        lock(&self.mark_sent_fault).take()
    }

    // --- commits --------------------------------------------------------------

    /// Commits `outcome` for row `id`, publishes it, and schedules a retry
    /// when it left the row `uncertain`. `None` when the commit failed,
    /// which is logged; the row then waits for startup recovery.
    pub(crate) fn commit_outcome(
        self: &Arc<Self>,
        id: &str,
        outcome: repository::Outcome,
    ) -> Option<HostOperation> {
        let row = match self.storage.write_repo(|tx| {
            repository::transition_at(tx, id, outcome, &format_time(self.clock.now()))
        }) {
            Ok(row) => row,
            Err(error) => {
                log_commit_failure(id, "its dispatch outcome", &error);
                return None;
            }
        };
        self.publish(std::slice::from_ref(&row));
        if row.state == HostOperationState::Uncertain {
            self.schedule_retry(&row);
        }
        Some(row)
    }

    // --- retries and the Online hook -----------------------------------------

    /// "Check again" and Online: the next retry uses the first backoff
    /// step, and every pending timer is cancelled.
    pub(crate) fn reset_backoff(&self, printer_id: &str) {
        let mut retries = lock(&self.retries);
        let retry = retries.entry(printer_id.to_string()).or_default();
        retry.generation += 1;
        retry.step = 0;
    }

    /// Cancels every pending timer for the Printer (abandon).
    pub(crate) fn cancel_retries(&self, printer_id: &str) {
        lock(&self.retries)
            .entry(printer_id.to_string())
            .or_default()
            .generation += 1;
    }

    fn retry_is_current(&self, printer_id: &str, generation: u64) -> bool {
        lock(&self.retries)
            .get(printer_id)
            .is_some_and(|retry| retry.generation == generation)
    }

    /// D5 "Retry": while the Printer is Online, one attempt after
    /// `backoff(step)`, and for `uploadSettling` also at the end of the
    /// settle period. Nothing while it is not Online (the Online hook
    /// resumes).
    pub(crate) fn schedule_retry(self: &Arc<Self>, row: &HostOperation) {
        if row.state != HostOperationState::Uncertain || !self.is_online(&row.printer_id) {
            return;
        }
        let (generation, step) = {
            let mut retries = lock(&self.retries);
            let retry = retries.entry(row.printer_id.clone()).or_default();
            retry.generation += 1;
            let step = retry.step;
            retry.step = retry.step.saturating_add(1);
            (retry.generation, step)
        };
        let mut delays = vec![(self.timings.backoff)(step)];
        let settling = row
            .last_attempt
            .as_ref()
            .is_some_and(|attempt| attempt.reason == InconclusiveReason::UploadSettling);
        if settling {
            if let Some(deadline) = reconciler::settle_deadline(row, &self.timings) {
                delays.push((deadline - self.clock.now()).to_std().unwrap_or_default());
            }
        }
        for delay in delays {
            self.retry_timers_scheduled.fetch_add(1, Ordering::SeqCst);
            let weak = Arc::downgrade(self);
            let id = row.id.clone();
            let printer_id = row.printer_id.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(delay).await;
                let Some(services) = weak.upgrade() else {
                    return;
                };
                if services.retry_is_current(&printer_id, generation)
                    && services.is_online(&printer_id)
                {
                    services.retry_attempts_run.fetch_add(1, Ordering::SeqCst);
                    let _ = reconciler::attempt(&services, &id).await;
                }
            });
        }
    }

    /// D5/D6: the Printer came Online. Refresh its host facts, then one
    /// attempt for its `uncertain` row.
    fn on_online(self: &Arc<Self>, printer_id: String) {
        let services = Arc::clone(self);
        tauri::async_runtime::spawn(async move {
            services.refresh_host_facts(&printer_id).await;
            services.reset_backoff(&printer_id);
            let rows = services
                .storage
                .read(|connection| Ok(repository::list_unresolved(connection, Some(&printer_id))))
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            for row in rows {
                if row.state == HostOperationState::Uncertain {
                    let _ = reconciler::attempt(&services, &row.id).await;
                }
            }
        });
    }

    /// Reads and caches the Printer's host facts (never persisted). A read
    /// that fails keeps the previous facts.
    pub(crate) async fn refresh_host_facts(&self, printer_id: &str) {
        let Ok(Some(printer)) = self.load_printer(printer_id) else {
            return;
        };
        let Some(config) = printer.connection.clone() else {
            return;
        };
        let key = self.credential(&printer).unwrap_or(None);
        let Some(host_state) = self.factory.host_state(&config, key) else {
            return;
        };
        if let Ok(facts) = host_state.host_facts().await {
            lock(&self.host_facts).insert(
                printer_id.to_string(),
                CachedHostFacts {
                    endpoint: HostOperationEndpoint::of(&config),
                    facts,
                    observed_at: format_time(self.clock.now()),
                },
            );
        }
    }
}
