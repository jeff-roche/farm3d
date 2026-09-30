//! P8 D6: the [`NotificationService`], held in
//! `RuntimeServices.notifications`.
//!
//! - **Candidates.** `start` subscribes to the Attention projector's
//!   committed passes (`AttentionServices::subscribe_applied`) and runs
//!   each live insert's [`NotifyCandidate`] through
//!   [`policy::decide`](super::policy::decide) with the stored class
//!   settings, the Event's Printer's alert defaults, the [`Focus`], and
//!   one [`RateLimiter`]. The broadcast's buffer is the candidate channel's
//!   bound: a receiver that lagged drops what it missed (a count is
//!   logged) and never shows a stale Event late; the center has it.
//! - **Showing.** After a show succeeds it remembers the notification in
//!   `outstanding` (bounded at [`OUTSTANDING_LIMIT`], oldest evicted
//!   first: a server may never report an expired one closed) and writes
//!   `notified_at` on the Events it covered (no revision bump, no event).
//! - **Signals.** The sink's signals are handled one at a time, in order:
//!   `ActivationToken` is kept for its id for `token_ttl`;
//!   `ActionInvoked` (`default` or `Open`) on a known id raises the
//!   window, emits `farm3d-navigate-v1`, and marks the Event read
//!   ([`activation`](super::activation)); `NotificationClosed` forgets
//!   the id. Ids farm3d didn't show are ignored.
//!
//! Nothing it logs names a Printer, a host, a path, or a credential:
//! only counts and error kinds.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock, Weak};
use std::time::{Duration, Instant};

use tauri::AppHandle;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::{mpsc, watch};

use crate::attention::deep_link;
use crate::attention::repository as attention_repository;
use crate::contracts::navigation::NavigationTarget;
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::alerts::{self, AlertDefaults};
use crate::RuntimeServices;

use super::activation::{self, MainWindowControl, WindowControl};
use super::focus::Focus;
use super::null::NullNotificationSink;
use super::policy::{self, NotifyCandidate, RateLimiter};
use super::{
    NavigateRequest, Notification, NotificationClassSettings, NotificationHandle,
    NotificationSink, NotifierStatus, NotifyError, SinkSignal,
};

/// The most notifications remembered for a click (D6, step 1).
pub const OUTSTANDING_LIMIT: usize = 256;
/// The most recent log lines kept for [`NotificationService::log_lines`].
const LOG_LINES: usize = 64;

/// D6's waits. `default()` holds the production values.
#[derive(Clone, Copy, Debug)]
pub struct NotificationTimings {
    /// How long a raise step waits for `Focused(true)` (500 ms).
    pub raise_wait: Duration,
    /// How long an `ActivationToken` is kept for its click (10 s).
    pub token_ttl: Duration,
}

impl Default for NotificationTimings {
    fn default() -> Self {
        Self {
            raise_wait: Duration::from_millis(500),
            token_ttl: Duration::from_secs(10),
        }
    }
}

/// What a click on a shown notification opens.
#[derive(Clone, PartialEq, Debug)]
pub struct OutstandingNotification {
    /// The target when shown. An Event's is evaluated again at click time.
    pub target: NavigationTarget,
    /// The one Event a click marks read.
    pub event_id: Option<String>,
    pub open_attention_center: bool,
}

/// `outstanding`: id → notification, oldest first.
#[derive(Default)]
struct Outstanding {
    entries: HashMap<u32, OutstandingNotification>,
    order: VecDeque<u32>,
}

impl Outstanding {
    fn insert(&mut self, id: u32, entry: OutstandingNotification) {
        // A summary updated in place keeps its id and its place.
        if self.entries.insert(id, entry).is_none() {
            self.order.push_back(id);
        }
        while self.order.len() > OUTSTANDING_LIMIT {
            if let Some(oldest) = self.order.pop_front() {
                self.entries.remove(&oldest);
            }
        }
    }

    fn remove(&mut self, id: u32) {
        if self.entries.remove(&id).is_some() {
            self.order.retain(|kept| *kept != id);
        }
    }
}

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The runtime a started service works against.
struct Runtime<R: tauri::Runtime> {
    app: AppHandle<R>,
    services: Weak<RuntimeServices<R>>,
}

/// The notification runtime's state.
pub struct NotificationService<R: tauri::Runtime> {
    sink: RwLock<Arc<dyn NotificationSink>>,
    focus: Focus,
    timings: NotificationTimings,
    limiter: Mutex<RateLimiter>,
    outstanding: Mutex<Outstanding>,
    tokens: Mutex<HashMap<u32, (String, Instant)>>,
    control: OnceLock<Arc<dyn WindowControl>>,
    runtime: OnceLock<Runtime<R>>,
    started: OnceLock<()>,
    signals: mpsc::UnboundedSender<SinkSignal>,
    signal_receiver: Mutex<Option<mpsc::UnboundedReceiver<SinkSignal>>>,
    held: watch::Sender<bool>,
    stop: watch::Sender<bool>,
    log: Mutex<VecDeque<String>>,
    candidates_handled: AtomicU64,
    signals_handled: AtomicU64,
    lagged: AtomicU64,
}

impl<R: tauri::Runtime> Default for NotificationService<R> {
    /// The null sink: tests never reach a desktop unless they ask to.
    fn default() -> Self {
        Self::new(
            Arc::new(NullNotificationSink),
            Focus::default(),
            NotificationTimings::default(),
        )
    }
}

impl<R: tauri::Runtime> NotificationService<R> {
    pub fn new(sink: Arc<dyn NotificationSink>, focus: Focus, timings: NotificationTimings) -> Self {
        let (signals, signal_receiver) = mpsc::unbounded_channel();
        let service = Self {
            sink: RwLock::new(Arc::clone(&sink)),
            focus,
            timings,
            limiter: Mutex::new(RateLimiter::default()),
            outstanding: Mutex::new(Outstanding::default()),
            tokens: Mutex::new(HashMap::new()),
            control: OnceLock::new(),
            runtime: OnceLock::new(),
            started: OnceLock::new(),
            signals,
            signal_receiver: Mutex::new(Some(signal_receiver)),
            held: watch::channel(false).0,
            stop: watch::channel(false).0,
            log: Mutex::new(VecDeque::new()),
            candidates_handled: AtomicU64::new(0),
            signals_handled: AtomicU64::new(0),
            lagged: AtomicU64::new(0),
        };
        service.attach(&sink);
        service
    }

    /// The platform's sink: farm3d's own D-Bus client on Linux (connected
    /// lazily, so a missing bus is `unavailable`, never a failed start),
    /// the null sink elsewhere. `icon` is D6's `app_icon`.
    pub fn platform(focus: Focus, icon: String) -> Self {
        #[cfg(target_os = "linux")]
        let sink: Arc<dyn NotificationSink> = Arc::new(super::dbus::DbusNotificationSink::new(icon));
        #[cfg(not(target_os = "linux"))]
        let sink: Arc<dyn NotificationSink> = {
            let _ = icon;
            Arc::new(NullNotificationSink)
        };
        Self::new(sink, focus, NotificationTimings::default())
    }

    fn attach(&self, sink: &Arc<dyn NotificationSink>) {
        let signals = self.signals.clone();
        sink.set_signal_handler(Arc::new(move |signal| {
            let _ = signals.send(signal);
        }));
    }

    fn sink(&self) -> Arc<dyn NotificationSink> {
        Arc::clone(&self.sink.read().unwrap_or_else(|poisoned| poisoned.into_inner()))
    }

    /// Test hook: replaces the sink (before `start`).
    #[doc(hidden)]
    pub fn set_sink(&self, sink: Arc<dyn NotificationSink>) {
        self.attach(&sink);
        *self.sink.write().unwrap_or_else(|poisoned| poisoned.into_inner()) = sink;
    }

    /// Where a click's raise goes; the main window unless set before
    /// `start` (tests record the steps instead).
    pub fn set_window_control(&self, control: Arc<dyn WindowControl>) {
        let _ = self.control.set(control);
    }

    pub fn focus(&self) -> &Focus {
        &self.focus
    }

    /// `notification_status`: the sink's status, retrying the connection
    /// first when it isn't `available`.
    pub async fn status(&self) -> NotifierStatus {
        let sink = self.sink();
        match sink.status() {
            available @ NotifierStatus::Available { .. } => available,
            _ => sink.refresh().await,
        }
    }

    /// `send_test_notification`: one notification whatever the focus and
    /// classes, targeting the Monitor. The status when it can't be shown.
    pub async fn send_test(&self) -> Result<NotificationHandle, NotifierStatus> {
        let status = self.status().await;
        if !matches!(status, NotifierStatus::Available { .. }) {
            self.log_line(
                "notifications.testNotShown",
                format!(
                    "farm3d: notifications: the test notification was not shown ({})",
                    status_kind(&status)
                ),
            );
            return Err(status);
        }
        let notification = Notification::test();
        match self.sink().show(&notification).await {
            Ok(handle) => {
                self.remember(handle, &notification);
                Ok(handle)
            }
            Err(error) => {
                self.log_line(
                    "notifications.testNotShown",
                    format!(
                        "farm3d: notifications: the test notification was not shown ({})",
                        error_kind(error)
                    ),
                );
                Err(error.status())
            }
        }
    }

    /// Decides one candidate and, when it notifies, shows it. `None` when
    /// it isn't shown, or the service hasn't started.
    pub async fn consider(&self, candidate: &NotifyCandidate) -> Option<NotificationHandle> {
        let services = self.runtime.get()?.services.upgrade()?;
        let now = services.attention.now();
        let printer_id = candidate.event.printer_id.clone();
        let inputs = services
            .storage
            .read(|connection| {
                Ok((|| -> Result<_, StorageError> {
                    let classes =
                        crate::settings::repository::load_notification_classes(connection)?;
                    let alerts = match &printer_id {
                        Some(printer_id) => alerts::get(connection, printer_id)?.alert_defaults,
                        None => AlertDefaults::default(),
                    };
                    Ok((classes, alerts))
                })())
            })
            .and_then(|inputs| inputs);
        let (classes, alerts): (NotificationClassSettings, AlertDefaults) = match inputs {
            Ok(inputs) => inputs,
            Err(_) => {
                self.log_line(
                    "notifications.settingsUnreadable",
                    "farm3d: notifications: could not read the settings; skipped one candidate",
                );
                return None;
            }
        };
        let notification = {
            let mut limiter = lock(&self.limiter);
            policy::decide(
                candidate,
                &classes,
                &alerts,
                self.focus.is_focused(),
                &mut limiter,
                now,
            )
        }?;
        let handle = match self.sink().show(&notification).await {
            Ok(handle) => handle,
            Err(error) => {
                self.log_line(
                    "notifications.notShown",
                    format!(
                        "farm3d: notifications: a notification was not shown ({})",
                        error_kind(error)
                    ),
                );
                return None;
            }
        };
        // Only a notification the sink showed counts toward the limits.
        lock(&self.limiter).record(&notification, handle.id, now);
        self.remember(handle, &notification);
        let marked = services.storage.write_repo(|tx| -> Result<(), RepositoryError> {
            for event_id in &notification.event_ids {
                attention_repository::mark_notified(tx, event_id, now)?;
            }
            Ok(())
        });
        if marked.is_err() {
            self.log_line(
                "notifications.notifiedAtFailed",
                "farm3d: notifications: could not record notified_at",
            );
        }
        Some(handle)
    }

    fn remember(&self, handle: NotificationHandle, notification: &Notification) {
        lock(&self.outstanding).insert(
            handle.id,
            OutstandingNotification {
                target: notification.target.clone(),
                event_id: notification.event_id.clone(),
                open_attention_center: notification.open_attention_center,
            },
        );
    }

    /// One sink signal (D6 "Click activation").
    async fn handle_signal(&self, signal: SinkSignal) {
        match signal {
            SinkSignal::ActivationToken { id, token } => {
                if lock(&self.outstanding).entries.contains_key(&id) {
                    let mut tokens = lock(&self.tokens);
                    let ttl = self.timings.token_ttl;
                    tokens.retain(|_, (_, at)| at.elapsed() < ttl);
                    tokens.insert(id, (token, Instant::now()));
                }
            }
            SinkSignal::ActionInvoked { id, action } => {
                if action != "default" && action != "Open" {
                    return;
                }
                let Some(entry) = lock(&self.outstanding).entries.get(&id).cloned() else {
                    return;
                };
                let token = lock(&self.tokens)
                    .remove(&id)
                    .filter(|(_, at)| at.elapsed() < self.timings.token_ttl)
                    .map(|(token, _)| token);
                self.activate(entry, token);
            }
            SinkSignal::Closed { id, .. } => {
                lock(&self.outstanding).remove(id);
                lock(&self.tokens).remove(&id);
            }
        }
    }

    /// Steps 3–5: raise, navigate, mark read.
    fn activate(&self, entry: OutstandingNotification, token: Option<String>) {
        let Some(runtime) = self.runtime.get() else {
            return;
        };
        if let Some(control) = self.control.get() {
            activation::raise(
                Arc::clone(control),
                self.focus.clone(),
                token,
                self.timings.raise_wait,
            );
        }
        let services = runtime.services.upgrade();
        let target = match (&entry.event_id, &services) {
            (Some(event_id), Some(services)) => services
                .storage
                .read(|connection| {
                    Ok((|| -> Result<_, StorageError> {
                        let Some(event) = attention_repository::load_event(connection, event_id)?
                        else {
                            return Ok(None);
                        };
                        let exists = deep_link::source_exists(connection, &event)?;
                        Ok(Some(deep_link::target_for(&event, exists)))
                    })())
                })
                .and_then(|target| target)
                .ok()
                .flatten()
                .unwrap_or_else(|| entry.target.clone()),
            _ => entry.target.clone(),
        };
        activation::navigate(
            &runtime.app,
            &NavigateRequest::new(target, entry.open_attention_center),
        );
        if let (Some(event_id), Some(services)) = (&entry.event_id, &services) {
            self.mark_read(services, &runtime.app, event_id);
        }
    }

    /// Step 5: `mark_read` (what `mark_attention_read` uses, without an
    /// `operationId`), published when it changed the Event.
    fn mark_read(&self, services: &RuntimeServices<R>, app: &AppHandle<R>, event_id: &str) {
        let now = services.attention.now();
        let ids = [event_id.to_string()];
        let changed = services.storage.write_repo(|tx| {
            let was_unread = attention_repository::load_event(tx, event_id)?
                .is_some_and(|event| event.read_at.is_none());
            if !was_unread {
                return Ok(Vec::new());
            }
            attention_repository::mark_read(tx, &ids, now)
        });
        match changed {
            Ok(events) if !events.is_empty() => {
                services
                    .attention
                    .stream
                    .publish_change(app, &events, &[], &[]);
            }
            Ok(_) => {}
            Err(_) => self.log_line(
                "notifications.markReadFailed",
                "farm3d: notifications: could not mark a clicked Event read",
            ),
        }
    }

    /// Keeps `line` (a fixed message) in the in-memory buffer and logs its
    /// `code` to the diagnostics log. The line itself never reaches the file.
    fn log_line(&self, code: &'static str, line: impl Into<String>) {
        let line = line.into();
        crate::f3d_log!(warn, code);
        let mut log = lock(&self.log);
        log.push_back(line);
        while log.len() > LOG_LINES {
            log.pop_front();
        }
    }

    /// Test hook: the service's most recent log lines.
    pub fn log_lines(&self) -> Vec<String> {
        lock(&self.log).iter().cloned().collect()
    }

    /// Test hook: the notification remembered for `id`.
    pub fn outstanding(&self, id: u32) -> Option<OutstandingNotification> {
        lock(&self.outstanding).entries.get(&id).cloned()
    }

    /// Test hook: how many notifications are remembered.
    pub fn outstanding_len(&self) -> usize {
        lock(&self.outstanding).entries.len()
    }

    /// Test hook: candidates the runtime has handled.
    pub fn candidates_handled(&self) -> u64 {
        self.candidates_handled.load(Ordering::SeqCst)
    }

    /// Test hook: sink signals the runtime has handled.
    pub fn signals_handled(&self) -> u64 {
        self.signals_handled.load(Ordering::SeqCst)
    }

    /// Test hook: how many times the candidate receiver lagged.
    pub fn lagged(&self) -> u64 {
        self.lagged.load(Ordering::SeqCst)
    }

    /// Test hook: the candidate task waits (its receiver keeps filling,
    /// and may lag) until [`release`](Self::release).
    pub fn hold(&self) {
        self.held.send_replace(true);
    }

    /// Test hook: see [`hold`](Self::hold).
    pub fn release(&self) {
        self.held.send_replace(false);
    }

    /// Stops the runtime's tasks.
    pub fn stop(&self) {
        self.stop.send_replace(true);
    }
}

fn status_kind(status: &NotifierStatus) -> &'static str {
    match status {
        NotifierStatus::Available { .. } => "available",
        NotifierStatus::Unavailable { reason } => error_kind(NotifyError::Unavailable(*reason)),
        NotifierStatus::Unsupported => "unsupported",
    }
}

fn error_kind(error: NotifyError) -> &'static str {
    match error {
        NotifyError::Unsupported => "unsupported",
        NotifyError::Unavailable(super::NotifierUnavailableReason::NoSessionBus) => "noSessionBus",
        NotifyError::Unavailable(super::NotifierUnavailableReason::NoNotificationServer) => {
            "noNotificationServer"
        }
        NotifyError::Unavailable(super::NotifierUnavailableReason::CallFailed) => "callFailed",
    }
}

/// Starts the notification runtime for `services`
/// (`start_notification_runtime`). A second call does nothing. It
/// subscribes to the projector's committed passes, so it must start
/// before `start_attention_runtime` to hear the first live pass. The sink
/// connects in the background (D6: `GetServerInformation` and
/// `GetCapabilities` at startup).
pub fn start<R: tauri::Runtime>(services: &Arc<RuntimeServices<R>>, app: &AppHandle<R>) {
    let service = Arc::clone(&services.notifications);
    let _ = service.runtime.set(Runtime {
        app: app.clone(),
        services: Arc::downgrade(services),
    });
    if service.started.set(()).is_err() {
        return;
    }
    let _ = service
        .control
        .set(Arc::new(MainWindowControl::new(app.clone())));

    let sink = service.sink();
    tauri::async_runtime::spawn(async move {
        let _ = sink.refresh().await;
    });

    let candidates = services.attention.subscribe_applied();
    tauri::async_runtime::spawn(run_candidates(Arc::downgrade(&service), candidates));
    let signals = lock(&service.signal_receiver).take();
    if let Some(signals) = signals {
        tauri::async_runtime::spawn(run_signals(Arc::downgrade(&service), signals));
    }
}

async fn run_candidates<R: tauri::Runtime>(
    service: Weak<NotificationService<R>>,
    mut candidates: tokio::sync::broadcast::Receiver<
        Arc<crate::attention::projector::AppliedChanges>,
    >,
) {
    let (mut held, mut stop) = match service.upgrade() {
        Some(service) => (service.held.subscribe(), service.stop.subscribe()),
        None => return,
    };
    loop {
        while *held.borrow() {
            tokio::select! {
                changed = held.changed() => if changed.is_err() { return; },
                changed = stop.changed() => if changed.is_err() || *stop.borrow() { return; },
            }
        }
        if *stop.borrow() {
            return;
        }
        let received = tokio::select! {
            received = candidates.recv() => received,
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { return; }
                continue;
            }
        };
        let Some(service) = service.upgrade() else {
            return;
        };
        match received {
            Ok(changes) => {
                for candidate in &changes.notify {
                    service.consider(candidate).await;
                }
                service
                    .candidates_handled
                    .fetch_add(changes.notify.len() as u64, Ordering::SeqCst);
            }
            // Never replay: what the receiver missed stays in the
            // Attention center.
            Err(RecvError::Lagged(missed)) => {
                service.lagged.fetch_add(1, Ordering::SeqCst);
                service.log_line(
                    "notifications.passesDropped",
                    format!("farm3d: notifications: dropped {missed} missed projector passes"),
                );
            }
            Err(RecvError::Closed) => return,
        }
    }
}

async fn run_signals<R: tauri::Runtime>(
    service: Weak<NotificationService<R>>,
    mut signals: mpsc::UnboundedReceiver<SinkSignal>,
) {
    let mut stop = match service.upgrade() {
        Some(service) => service.stop.subscribe(),
        None => return,
    };
    loop {
        let signal = tokio::select! {
            signal = signals.recv() => match signal {
                Some(signal) => signal,
                None => return,
            },
            changed = stop.changed() => {
                if changed.is_err() || *stop.borrow() { return; }
                continue;
            }
        };
        let Some(service) = service.upgrade() else {
            return;
        };
        service.handle_signal(signal).await;
        service.signals_handled.fetch_add(1, Ordering::SeqCst);
    }
}
