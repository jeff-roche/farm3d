//! P6: the durable Host Operation record — one row per write-ahead commit
//! of an upload, start, pause, resume, or cancel command (D2), its pure
//! state machine ([`state`], D3), and its SQL ([`repository`], D2/D3/D5).
//!
//! This module holds the ts-rs domain types the wire shares with the
//! frontend (`HostOperation` and friends); the executor, reconciler, and
//! guards ([`guards`], D7) build on top of [`repository`]'s functions.

pub mod commands;
pub mod events;
pub mod executor;
pub mod guards;
pub mod reconciler;
pub mod repository;
pub mod start_rule;
pub mod state;

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{Receiver, SyncSender};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use ts_rs::TS;
use zeroize::Zeroizing;

use crate::connections::capabilities::{
    capabilities_for, ArtifactStaging, CapabilityKey, CapabilityState, HostFacts,
    HostOperationFailureCode, HostStateQuery, InconclusiveReason, PrintControl,
    PrinterCapabilities,
};
use crate::connections::credentials::CredentialStore;
use crate::connections::supervisor::ConnectionManager;
use crate::connections::{ConnectionConfig, ConnectionState};
use crate::contracts::command::CommandError;
use crate::contracts::event::JsSafeInteger;
use crate::library::content::ContentStore;
use crate::persistence::{RepositoryError, Storage, StorageError};
use crate::printers::repository::PrinterRepository;
use crate::printers::StoredPrinter;

/// The `hop-<uuid v4>` id prefix (D2).
pub const HOST_OPERATION_ID_PREFIX: &str = "hop";

pub fn new_host_operation_id() -> String {
    crate::library::new_id(HOST_OPERATION_ID_PREFIX)
}

/// D2/D3: what a Host Operation row is for.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperationKind.ts")]
pub enum HostOperationKind {
    Upload,
    Start,
    Pause,
    Resume,
    Cancel,
}

/// D3: a Host Operation's state. `succeeded`, `failed`, and `abandoned`
/// are terminal (the migration's `BEFORE UPDATE` trigger enforces it).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperationState.ts")]
pub enum HostOperationState {
    Dispatching,
    Uncertain,
    Reconciling,
    Succeeded,
    Failed,
    Abandoned,
}

/// D9: the Printer state `start_staged_artifact` was offered from, so the
/// bed-clear confirmation can name it.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PriorState.ts")]
pub enum PriorState {
    Ready,
    Finished,
    Cancelled,
}

/// A Host Operation's `endpoint_json`: the Connection farm3d dispatched
/// to, frozen at write-ahead time. Never a credential or `credentialRef`
/// (D2).
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationEndpoint.ts"
)]
pub struct HostOperationEndpoint {
    pub kind: String,
    pub host: String,
    pub port: u16,
}

/// D11: a failed row's `failure_json`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperationFailure.ts")]
pub struct HostOperationFailure {
    pub code: HostOperationFailureCode,
    pub message: String,
}

impl HostOperationFailure {
    /// D11's fixed message for `code` (the table's "Message" column,
    /// reproduced verbatim). The one place that text lives, so
    /// `repository::recover_after_restart` and a later task's executor
    /// raise the exact same wording for the same code.
    pub fn for_code(code: HostOperationFailureCode) -> Self {
        use HostOperationFailureCode as Code;
        let message = match code {
            Code::NeverSent => "farm3d closed before sending this. Nothing reached the printer.",
            Code::HostUnreachable => "farm3d couldn't connect to the printer. Nothing was sent.",
            Code::AuthRejected => "The printer rejected farm3d's API key.",
            Code::ChecksumRejected => "The printer found the upload damaged and discarded it.",
            Code::FileLoaded => {
                "The printer is using a file with this name, so it refused the upload."
            }
            Code::HostBusy => "The printer is busy with another print.",
            Code::FileMissing => "The printer couldn't find the staged file.",
            Code::HostRejected => "The printer refused the request.",
            Code::HostNotReady => "Klipper isn't running on the printer.",
            Code::NotApplied => {
                "The file isn't on the printer, and it didn't appear within a minute."
            }
            Code::HostFileDiffers => {
                "A different file is at farm3d's path on the printer. Staging again replaces it."
            }
        };
        Self {
            code,
            message: message.to_string(),
        }
    }
}

/// D11 `HostOperationResolution.startObserved.source`: where the start
/// evidence came from.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/StartEvidenceSource.ts")]
pub enum StartEvidenceSource {
    PrintStats,
    History,
}

/// D11 `HostOperationResolution.stateObserved.observedState`: the
/// `print_stats.state` reconciliation read to prove pause/resume/cancel.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationObservedState.ts"
)]
pub enum HostOperationObservedState {
    Printing,
    Paused,
    Complete,
    Cancelled,
}

/// D11: a succeeded row's `resolution_json`, tagged on `kind`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "kind",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/HostOperationResolution.ts"
)]
pub enum HostOperationResolution {
    ArtifactVerified {
        reconciled: bool,
    },
    /// A 200 `{"result":"ok"}` at dispatch (D5: definitive for `start`).
    StartAccepted,
    StartObserved {
        source: StartEvidenceSource,
        /// Moonraker's hex id string, for display only.
        history_job_id: Option<String>,
        interrupted: bool,
    },
    StateObserved {
        observed_state: HostOperationObservedState,
        reconciled: bool,
    },
}

/// `HostOperation.lastAttempt`: the most recent reconciliation attempt
/// that left the row `uncertain`.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationLastAttempt.ts"
)]
pub struct HostOperationLastAttempt {
    pub at: String,
    pub reason: InconclusiveReason,
}

/// A `host_operations` row, on the wire. `history_mark` is backend-only
/// (D2/"Wire types"): present here for the reconciler and executor, never
/// serialized or exported.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostOperation.ts")]
pub struct HostOperation {
    pub id: String,
    pub printer_id: String,
    pub kind: HostOperationKind,
    pub state: HostOperationState,
    pub slice_revision_id: Option<String>,
    pub source_host_operation_id: Option<String>,
    pub gcode_sha256: Option<String>,
    #[ts(type = "number | null")]
    pub gcode_size: Option<i64>,
    pub host_path: String,
    /// Start only: the newest history `job_id` before dispatch. Backend-
    /// only — not on the wire.
    #[serde(skip)]
    #[ts(skip)]
    pub history_mark: Option<i64>,
    pub endpoint: HostOperationEndpoint,
    pub failure: Option<HostOperationFailure>,
    pub resolution: Option<HostOperationResolution>,
    #[ts(type = "number")]
    pub attempts: i64,
    pub last_attempt: Option<HostOperationLastAttempt>,
    pub no_longer_pending: bool,
    pub abandoned_at: Option<String>,
    pub abandon_note: Option<String>,
    pub created_at: String,
    pub dispatched_at: Option<String>,
    pub uncertain_since: Option<String>,
    pub resolved_at: Option<String>,
}

/// `list_host_operations`' result: the stream identity a later `hostOperations`
/// event carries, plus the backfill (spec "Commands").
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/HostOperationsSnapshot.ts"
)]
pub struct HostOperationsSnapshot {
    pub stream_id: String,
    pub snapshot_sequence: JsSafeInteger,
    pub operations: Vec<HostOperation>,
}

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

/// Timestamps are stored at whole-second precision (`now_rfc3339`).
pub(crate) fn format_time(time: DateTime<Utc>) -> String {
    time.format("%Y-%m-%dT%H:%M:%SZ").to_string()
}

pub(crate) fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|time| time.with_timezone(&Utc))
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
    host_facts: Mutex<HashMap<String, (HostFacts, String)>>,
    retries: Mutex<HashMap<String, RetryState>>,
    faults: Mutex<Vec<(FaultPoint, FaultAction, SyncSender<()>)>>,
    mark_sent_fault: Mutex<Option<(MarkSentFault, SyncSender<()>)>>,
    /// How many timer-scheduled attempts actually ran (diagnostics, tests).
    retry_attempts_run: AtomicUsize,
    /// How many retry timers were scheduled (diagnostics, tests).
    retry_timers_scheduled: AtomicUsize,
}

/// A repository error as the command error every host-ops command returns.
pub(crate) fn repository_error(error: RepositoryError) -> CommandError {
    CommandError::from_repository(error)
}

/// Logs a commit that failed after the host may have been contacted. The
/// row keeps its last committed state, and startup recovery is the
/// backstop. Rows and repository errors carry no credential (D2), so
/// neither does this line.
pub(crate) fn log_commit_failure(id: &str, what: &str, error: &RepositoryError) {
    eprintln!("farm3d: host operation {id}: could not commit {what}: {error:?}");
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
        let cached = lock(&self.host_facts).get(&printer.id).cloned();
        let mut capabilities = self
            .factory
            .capabilities(printer, cached.as_ref().map(|(facts, _)| facts));
        if capabilities.host_facts.is_some() {
            capabilities.observed_at = cached.map(|(_, observed_at)| observed_at);
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
    /// the Printer's current credential reference.
    pub(crate) fn endpoint_config(
        &self,
        endpoint: &HostOperationEndpoint,
        printer: &StoredPrinter,
    ) -> ConnectionConfig {
        ConnectionConfig {
            kind: endpoint.kind.clone(),
            host: endpoint.host.clone(),
            port: endpoint.port,
            use_tls: false,
            credential_ref: printer
                .connection
                .as_ref()
                .and_then(|connection| connection.credential_ref.clone()),
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
        let row = match self
            .storage
            .write_repo(|tx| repository::transition(tx, id, outcome))
        {
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
                (facts, format_time(self.clock.now())),
            );
        }
    }
}

/// A camelCase enum's wire spelling.
pub(crate) fn wire(value: impl Serialize) -> String {
    serde_json::to_value(value)
        .ok()
        .and_then(|value| value.as_str().map(str::to_string))
        .unwrap_or_default()
}

/// D3 startup recovery, run by `build_runtime_services` before any command
/// is served (and by tests that simulate a restart). `now` stamps
/// `uncertain_since` for rows that may have been sent.
pub fn recover_after_restart(
    storage: &Storage,
    now: DateTime<Utc>,
) -> Result<Vec<HostOperation>, StorageError> {
    storage
        .write_repo(|tx| repository::recover_after_restart(tx, &format_time(now)))
        .map_err(|error| match error {
            RepositoryError::Storage(error) => error,
            _ => StorageError::Database,
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_host_operation_id_uses_the_hop_prefix() {
        assert!(new_host_operation_id().starts_with("hop-"));
    }

    /// `for_code`'s match has no wildcard arm, so a code the compiler
    /// doesn't cover here fails to build; this spot-checks a few against
    /// D11's table text verbatim.
    #[test]
    fn for_code_reproduces_d11s_message_verbatim() {
        assert_eq!(
            HostOperationFailure::for_code(HostOperationFailureCode::NeverSent).message,
            "farm3d closed before sending this. Nothing reached the printer."
        );
        assert_eq!(
            HostOperationFailure::for_code(HostOperationFailureCode::HostFileDiffers).message,
            "A different file is at farm3d's path on the printer. Staging again replaces it."
        );
        assert_eq!(
            HostOperationFailure::for_code(HostOperationFailureCode::AuthRejected).code,
            HostOperationFailureCode::AuthRejected
        );
    }

    /// The migration's `kind`/`state` `CHECK` lists spell each stored enum
    /// exactly as serde does.
    #[test]
    fn stored_enums_serialize_to_the_d2_check_spellings() {
        let wire = |value: serde_json::Value| value.as_str().unwrap().to_string();
        let kinds = [
            HostOperationKind::Upload,
            HostOperationKind::Start,
            HostOperationKind::Pause,
            HostOperationKind::Resume,
            HostOperationKind::Cancel,
        ]
        .map(|kind| wire(serde_json::to_value(kind).unwrap()));
        assert_eq!(kinds, ["upload", "start", "pause", "resume", "cancel"]);

        let states = [
            HostOperationState::Dispatching,
            HostOperationState::Uncertain,
            HostOperationState::Reconciling,
            HostOperationState::Succeeded,
            HostOperationState::Failed,
            HostOperationState::Abandoned,
        ]
        .map(|state| wire(serde_json::to_value(state).unwrap()));
        assert_eq!(
            states,
            [
                "dispatching",
                "uncertain",
                "reconciling",
                "succeeded",
                "failed",
                "abandoned"
            ]
        );
    }

    /// D11's tagged resolution shapes, verbatim.
    #[test]
    fn resolution_uses_its_d11_tagged_wire_shape() {
        assert_eq!(
            serde_json::to_value(HostOperationResolution::ArtifactVerified { reconciled: true })
                .unwrap(),
            serde_json::json!({ "kind": "artifactVerified", "reconciled": true })
        );
        assert_eq!(
            serde_json::to_value(HostOperationResolution::StartAccepted).unwrap(),
            serde_json::json!({ "kind": "startAccepted" })
        );
        assert_eq!(
            serde_json::to_value(HostOperationResolution::StartObserved {
                source: StartEvidenceSource::History,
                history_job_id: Some("0000A1".to_string()),
                interrupted: true,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "startObserved",
                "source": "history",
                "historyJobId": "0000A1",
                "interrupted": true,
            })
        );
        assert_eq!(
            serde_json::to_value(HostOperationResolution::StateObserved {
                observed_state: HostOperationObservedState::Paused,
                reconciled: false,
            })
            .unwrap(),
            serde_json::json!({
                "kind": "stateObserved",
                "observedState": "paused",
                "reconciled": false,
            })
        );
    }

    /// `history_mark` never reaches the wire, even when it's set.
    #[test]
    fn history_mark_is_never_serialized() {
        let operation = sample_operation();
        let value = serde_json::to_value(&operation).unwrap();
        assert!(value.get("historyMark").is_none());
        assert!(value.get("history_mark").is_none());
    }

    fn sample_operation() -> HostOperation {
        HostOperation {
            id: "hop-a".to_string(),
            printer_id: "prn-a".to_string(),
            kind: HostOperationKind::Start,
            state: HostOperationState::Dispatching,
            slice_revision_id: Some("slr-a".to_string()),
            source_host_operation_id: None,
            gcode_sha256: Some("a".repeat(64)),
            gcode_size: Some(100),
            host_path: "farm3d/slr-a.gcode".to_string(),
            history_mark: Some(3),
            endpoint: HostOperationEndpoint {
                kind: "moonraker".to_string(),
                host: "192.0.2.1".to_string(),
                port: 7125,
            },
            failure: None,
            resolution: None,
            attempts: 0,
            last_attempt: None,
            no_longer_pending: false,
            abandoned_at: None,
            abandon_note: None,
            created_at: "2026-01-01T00:00:00.000Z".to_string(),
            dispatched_at: None,
            uncertain_since: None,
            resolved_at: None,
        }
    }
}
