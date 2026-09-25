//! Retains normalized observations and publishes the canonical Printer status.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use chrono::{DateTime, Duration as ChronoDuration, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};
use ts_rs::TS;

use super::moonraker::MoonrakerConnection;
use super::status_repository::{
    PrinterTelemetry, SnapshotWrite, StatusRepository, StoredTelemetrySnapshot,
};
use super::{
    ConnectionConfig, ConnectionError, ConnectionObservation, ConnectionState, PrinterConnection,
    PrinterStatus, StatusCacheWarning, StatusCacheWarningOperation, MOONRAKER_KIND,
};
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::printers::operational::{evaluate_operational_status, HostActivity, OperationalInput};

pub const STATUS_EVENT: &str = "farm3d-event-v1";
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const STATUS_CHANNEL_DEPTH: usize = 16;
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const TELEMETRY_FRESH_FOR: ChronoDuration = ChronoDuration::seconds(30);

type ConnectionFactory = dyn Fn(&ConnectionConfig, Option<zeroize::Zeroizing<String>>) -> Option<Box<dyn PrinterConnection>>
    + Send
    + Sync;

struct SupervisorTask {
    stop: tokio::sync::watch::Sender<bool>,
    handle: JoinHandle<()>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterSetupFacts.ts")]
pub struct PrinterSetupFacts {
    pub has_usable_connection: bool,
    pub profile_resolved: bool,
}

impl PrinterSetupFacts {
    pub const fn complete() -> Self {
        Self {
            has_usable_connection: true,
            profile_resolved: true,
        }
    }

    fn setup_complete(self) -> bool {
        self.has_usable_connection && self.profile_resolved
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[ts(export_to = "domain/PrinterStatusEventType.ts")]
pub enum PrinterStatusEventType {
    #[serde(rename = "printer.status.changed")]
    #[ts(rename = "printer.status.changed")]
    Changed,
    #[serde(rename = "printer.status.removed")]
    #[ts(rename = "printer.status.removed")]
    Removed,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "type", rename_all = "camelCase")]
#[ts(
    tag = "type",
    rename_all = "camelCase",
    export_to = "domain/PrinterStatusEventPayload.ts"
)]
pub enum PrinterStatusEventPayload {
    Changed { status: Box<PrinterStatus> },
    Removed,
}

/// The exact envelope emitted on [`STATUS_EVENT`]. The transparent newtype
/// gives ts-rs a named generated contract without changing the wire shape.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(transparent)]
#[ts(export_to = "domain/PrinterStatusEvent.ts")]
pub struct PrinterStatusEvent(pub EventEnvelope<PrinterStatusEventType, PrinterStatusEventPayload>);

impl std::ops::Deref for PrinterStatusEvent {
    type Target = EventEnvelope<PrinterStatusEventType, PrinterStatusEventPayload>;

    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

#[derive(Serialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/PrinterStatusBackfill.ts"
)]
pub struct PrinterStatusBackfill {
    pub stream_id: String,
    pub snapshot_sequence: JsSafeInteger,
    pub statuses: Vec<PrinterStatusRow>,
    pub cache_warnings: Vec<StatusCacheWarning>,
}

#[derive(Serialize, Clone, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterStatusRow.ts")]
pub struct PrinterStatusRow {
    pub printer_id: String,
    pub status: PrinterStatus,
}

struct StatusState {
    stream_id: String,
    values: HashMap<String, PrinterStatus>,
    hydrated: HashSet<String>,
    epochs: HashMap<String, u64>,
    sequence: u64,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            values: HashMap::new(),
            hydrated: HashSet::new(),
            epochs: HashMap::new(),
            sequence: 0,
        }
    }
}

/// Shared status map. Publication mutates the map before emitting, retaining
/// listener-before-backfill semantics when an event races a backfill request.
pub struct StatusMap {
    state: Mutex<StatusState>,
}

impl Default for StatusMap {
    fn default() -> Self {
        Self {
            state: Mutex::new(StatusState::default()),
        }
    }
}

impl StatusMap {
    fn begin_supervision(&self, id: &str) -> u64 {
        let mut state = self.state.lock().expect("status map lock");
        let epoch = state.epochs.entry(id.to_string()).or_default();
        *epoch = epoch.saturating_add(1);
        *epoch
    }

    fn snapshot(&self) -> HashMap<String, PrinterStatus> {
        self.state.lock().expect("status map lock").values.clone()
    }

    fn status_and_hydration(&self, id: &str) -> (Option<PrinterStatus>, bool) {
        let state = self.state.lock().expect("status map lock");
        (state.values.get(id).cloned(), state.hydrated.contains(id))
    }

    fn seed_hydrated(&self, snapshot: StoredTelemetrySnapshot, now: DateTime<Utc>) {
        let observed_at = parse_time(&snapshot.last_observed_at).unwrap_or_else(Utc::now);
        let status = status_from_parts(
            ConnectionState::Offline,
            None,
            snapshot.telemetry,
            Some(observed_at),
            Some(observed_at + TELEMETRY_FRESH_FOR),
            PrinterSetupFacts::complete(),
            true,
            now,
        );
        let mut state = self.state.lock().expect("status map lock");
        state.hydrated.insert(snapshot.printer_id.clone());
        state.values.insert(snapshot.printer_id, status);
    }

    fn backfill(&self) -> PrinterStatusBackfill {
        let state = self.state.lock().expect("status map lock");
        let mut statuses = state
            .values
            .iter()
            .map(|(printer_id, status)| PrinterStatusRow {
                printer_id: printer_id.clone(),
                status: status.clone(),
            })
            .collect::<Vec<_>>();
        statuses.sort_by(|left, right| left.printer_id.as_bytes().cmp(right.printer_id.as_bytes()));
        PrinterStatusBackfill {
            stream_id: state.stream_id.clone(),
            snapshot_sequence: JsSafeInteger::try_from(state.sequence)
                .expect("status sequence is JS-safe"),
            statuses,
            cache_warnings: Vec::new(),
        }
    }

    fn publish_changed(
        &self,
        id: &str,
        status: PrinterStatus,
        hydrated: bool,
    ) -> PrinterStatusEvent {
        let mut state = self.state.lock().expect("status map lock");
        let envelope = next_envelope(
            &mut state,
            id,
            PrinterStatusEventType::Changed,
            PrinterStatusEventPayload::Changed {
                status: Box::new(status.clone()),
            },
        );
        state.values.insert(id.to_string(), status);
        if hydrated {
            state.hydrated.insert(id.to_string());
        } else {
            state.hydrated.remove(id);
        }
        envelope
    }

    fn publish_changed_if_current(
        &self,
        id: &str,
        epoch: u64,
        status: PrinterStatus,
        hydrated: bool,
    ) -> Option<PrinterStatusEvent> {
        let mut state = self.state.lock().expect("status map lock");
        if state.epochs.get(id).copied() != Some(epoch) {
            return None;
        }
        let envelope = next_envelope(
            &mut state,
            id,
            PrinterStatusEventType::Changed,
            PrinterStatusEventPayload::Changed {
                status: Box::new(status.clone()),
            },
        );
        state.values.insert(id.to_string(), status);
        if hydrated {
            state.hydrated.insert(id.to_string());
        } else {
            state.hydrated.remove(id);
        }
        Some(envelope)
    }

    /// Drops any retained status/hydration entry, bumps the epoch, and
    /// builds the removal envelope — all under one lock, so no concurrent
    /// publish can land between the drop and the epoch bump.
    fn publish_removed(&self, id: &str) -> PrinterStatusEvent {
        let mut state = self.state.lock().expect("status map lock");
        state.values.remove(id);
        state.hydrated.remove(id);
        let epoch = state.epochs.entry(id.to_string()).or_default();
        *epoch = epoch.saturating_add(1);
        next_envelope(
            &mut state,
            id,
            PrinterStatusEventType::Removed,
            PrinterStatusEventPayload::Removed,
        )
    }
}

fn next_envelope(
    state: &mut StatusState,
    id: &str,
    event_type: PrinterStatusEventType,
    payload: PrinterStatusEventPayload,
) -> PrinterStatusEvent {
    if state.sequence == crate::contracts::JS_MAX_SAFE_INTEGER {
        state.stream_id = uuid::Uuid::new_v4().to_string();
        state.sequence = 0;
    }
    state.sequence += 1;
    PrinterStatusEvent(EventEnvelope {
        contract_version: crate::contracts::ContractVersion::V1,
        stream_id: state.stream_id.clone(),
        sequence: JsSafeInteger::try_from(state.sequence).expect("guarded JS-safe sequence"),
        event_id: uuid::Uuid::new_v4().to_string(),
        occurred_at: Utc::now().to_rfc3339_opts(SecondsFormat::AutoSi, true),
        event_type,
        subject: EventSubject {
            kind: "printer".to_string(),
            id: id.to_string(),
        },
        payload,
    })
}

pub fn backoff_delay(attempt: u32) -> Duration {
    Duration::from_secs(1u64.checked_shl(attempt).unwrap_or(u64::MAX)).min(MAX_BACKOFF)
}

fn status_message(error: &ConnectionError) -> &'static str {
    match error {
        ConnectionError::Unreachable(_) => "The Printer could not be reached.",
        ConnectionError::Auth(_) => "The Printer rejected authentication.",
        ConnectionError::Protocol(_) => "The Printer returned an unexpected response.",
        ConnectionError::Timeout => "The Printer did not respond in time.",
    }
}

fn empty_telemetry() -> PrinterTelemetry {
    PrinterTelemetry {
        host_activity: HostActivity::Unknown,
        host_activity_name: None,
        job_name: None,
        progress: None,
        nozzle_temp_c: None,
        nozzle_target_c: None,
        bed_temp_c: None,
        bed_target_c: None,
        print_duration_s: None,
        tools: Vec::new(),
    }
}

fn parse_time(value: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(value)
        .ok()
        .map(|value| value.with_timezone(&Utc))
}

fn format_time(value: DateTime<Utc>) -> String {
    value.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn record_cache_warning_in(
    warnings: &Mutex<Vec<StatusCacheWarning>>,
    printer_id: Option<&str>,
    operation: StatusCacheWarningOperation,
) {
    let warning = StatusCacheWarning {
        printer_id: printer_id.map(str::to_string),
        operation,
    };
    let mut warnings = warnings.lock().expect("cache warning lock");
    if !warnings.contains(&warning) {
        warnings.push(warning);
    }
}

fn clear_cache_warning_in(
    warnings: &Mutex<Vec<StatusCacheWarning>>,
    printer_id: Option<&str>,
    operation: StatusCacheWarningOperation,
) {
    warnings
        .lock()
        .expect("cache warning lock")
        .retain(|warning| {
            warning.printer_id.as_deref() != printer_id || warning.operation != operation
        });
}

fn clear_cache_warnings_for_in(warnings: &Mutex<Vec<StatusCacheWarning>>, printer_id: &str) {
    warnings
        .lock()
        .expect("cache warning lock")
        .retain(|warning| warning.printer_id.as_deref() != Some(printer_id));
}

fn cache_warnings_for(
    warnings: &Mutex<Vec<StatusCacheWarning>>,
    printer_id: &str,
) -> Vec<StatusCacheWarning> {
    warnings
        .lock()
        .expect("cache warning lock")
        .iter()
        .filter(|warning| warning.printer_id.as_deref() == Some(printer_id))
        .cloned()
        .collect()
}

// The merger deliberately lists every policy input instead of hiding state in
// a builder; each caller makes its retained/observed provenance explicit.
#[expect(clippy::too_many_arguments, reason = "central status merge boundary")]
fn status_from_parts(
    connection_state: ConnectionState,
    error: Option<String>,
    telemetry: PrinterTelemetry,
    last_observed_at: Option<DateTime<Utc>>,
    fresh_until: Option<DateTime<Utc>>,
    setup: PrinterSetupFacts,
    hydrated_from_cache: bool,
    now: DateTime<Utc>,
) -> PrinterStatus {
    let result = evaluate_operational_status(
        &OperationalInput {
            setup_complete: setup.setup_complete(),
            connection_state,
            connection_error: error.is_some(),
            host_activity: telemetry.host_activity,
            last_observed_at,
            fresh_until,
            hydrated_from_cache,
        },
        now,
    );
    PrinterStatus {
        connection_state,
        error,
        telemetry,
        last_observed_at: last_observed_at.map(format_time),
        fresh_until: fresh_until.map(format_time),
        operational_state: result.operational_state,
        readiness: result.readiness,
        freshness: result.freshness,
        cache_warnings: Vec::new(),
        updated_at: format_time(now),
    }
}

/// Merges one observation with retained state without allowing health changes
/// to erase partial telemetry.
pub fn merge_status(
    previous: Option<&PrinterStatus>,
    observation: ConnectionObservation,
    setup: PrinterSetupFacts,
    hydrated_from_cache: bool,
    now: DateTime<Utc>,
) -> PrinterStatus {
    let previous = previous
        .cloned()
        .unwrap_or_else(|| PrinterStatus::new(ConnectionState::Offline));
    match observation {
        ConnectionObservation::Telemetry(telemetry) => status_from_parts(
            ConnectionState::Online,
            None,
            telemetry,
            Some(now),
            Some(now + TELEMETRY_FRESH_FOR),
            setup,
            false,
            now,
        ),
        ConnectionObservation::Health { state, observed_at } => {
            // Liveness cannot validate hydrated cache readings. Once a live
            // telemetry frame has established provenance, however, an online
            // health observation keeps that live reading fresh.
            let _ = observed_at;
            let fresh_until = (!hydrated_from_cache && state == ConnectionState::Online)
                .then_some(now + TELEMETRY_FRESH_FOR)
                .or_else(|| previous.fresh_until.as_deref().and_then(parse_time));
            status_from_parts(
                state,
                None,
                previous.telemetry,
                previous.last_observed_at.as_deref().and_then(parse_time),
                fresh_until,
                setup,
                hydrated_from_cache,
                now,
            )
        }
    }
}

pub struct ConnectionManager<R: tauri::Runtime> {
    app: AppHandle<R>,
    tasks: Mutex<HashMap<String, SupervisorTask>>,
    statuses: Arc<StatusMap>,
    repository: Arc<StatusRepository>,
    clock: Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>,
    connection_factory: Arc<ConnectionFactory>,
    cache_warnings: Arc<Mutex<Vec<StatusCacheWarning>>>,
    reconciliation: tokio::sync::Mutex<()>,
}

impl<R: tauri::Runtime> ConnectionManager<R> {
    pub fn new(app: AppHandle<R>, repository: Arc<StatusRepository>) -> Self {
        Self::with_clock(app, repository, Utc::now)
    }

    pub fn with_clock(
        app: AppHandle<R>,
        repository: Arc<StatusRepository>,
        clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
    ) -> Self {
        Self::with_clock_and_factory(app, repository, clock, build)
    }

    pub fn with_clock_and_factory(
        app: AppHandle<R>,
        repository: Arc<StatusRepository>,
        clock: impl Fn() -> DateTime<Utc> + Send + Sync + 'static,
        connection_factory: impl Fn(
                &ConnectionConfig,
                Option<zeroize::Zeroizing<String>>,
            ) -> Option<Box<dyn PrinterConnection>>
            + Send
            + Sync
            + 'static,
    ) -> Self {
        let clock = Arc::new(clock);
        let statuses = Arc::new(StatusMap::default());
        let cache_warnings = Arc::new(Mutex::new(Vec::new()));
        match repository.list() {
            Ok(snapshots) => {
                for snapshot in snapshots {
                    statuses.seed_hydrated(snapshot, clock());
                }
            }
            Err(error) => {
                eprintln!("farm3d: cannot hydrate telemetry cache: {error}");
                record_cache_warning_in(
                    &cache_warnings,
                    None,
                    StatusCacheWarningOperation::Hydrate,
                );
            }
        }
        Self {
            app,
            tasks: Mutex::new(HashMap::new()),
            statuses,
            repository,
            clock,
            connection_factory: Arc::new(connection_factory),
            cache_warnings,
            reconciliation: tokio::sync::Mutex::new(()),
        }
    }

    fn now(&self) -> DateTime<Utc> {
        (self.clock)()
    }
    pub fn statuses(&self) -> HashMap<String, PrinterStatus> {
        self.statuses.snapshot()
    }
    pub fn status_backfill(&self) -> PrinterStatusBackfill {
        let mut backfill = self.statuses.backfill();
        backfill.cache_warnings = self.cache_warnings();
        backfill
    }

    pub fn cache_warnings(&self) -> Vec<StatusCacheWarning> {
        self.cache_warnings
            .lock()
            .expect("cache warning lock")
            .clone()
    }

    fn record_cache_warning(
        &self,
        printer_id: Option<&str>,
        operation: StatusCacheWarningOperation,
    ) {
        record_cache_warning_in(&self.cache_warnings, printer_id, operation);
    }

    fn clear_cache_warning(
        &self,
        printer_id: Option<&str>,
        operation: StatusCacheWarningOperation,
    ) {
        clear_cache_warning_in(&self.cache_warnings, printer_id, operation);
    }

    fn clear_cache_warnings_for(&self, printer_id: &str) {
        clear_cache_warnings_for_in(&self.cache_warnings, printer_id);
    }

    fn cache_warnings_for(&self, printer_id: &str) -> Vec<StatusCacheWarning> {
        self.cache_warnings()
            .into_iter()
            .filter(|warning| warning.printer_id.as_deref() == Some(printer_id))
            .collect()
    }

    pub fn reconcile_printer(&self, printer_id: &str, setup: PrinterSetupFacts) {
        let (previous, hydrated) = self.statuses.status_and_hydration(printer_id);
        let previous = previous.unwrap_or_else(|| PrinterStatus::new(ConnectionState::Offline));
        let cache_warnings = previous.cache_warnings.clone();
        let state = if setup.has_usable_connection {
            previous.connection_state
        } else {
            ConnectionState::Offline
        };
        let mut next = status_from_parts(
            state,
            previous.error,
            previous.telemetry,
            previous.last_observed_at.as_deref().and_then(parse_time),
            previous.fresh_until.as_deref().and_then(parse_time),
            setup,
            hydrated,
            self.now(),
        );
        next.cache_warnings = cache_warnings;
        self.publish_changed(printer_id, next, hydrated);
    }

    pub fn apply_observation(
        &self,
        printer_id: &str,
        observation: ConnectionObservation,
        setup: PrinterSetupFacts,
    ) {
        let (previous, hydrated) = self.statuses.status_and_hydration(printer_id);
        let now = self.now();
        let write = match (&previous, &observation) {
            (Some(status), ConnectionObservation::Telemetry(telemetry))
                if status.telemetry.host_activity != telemetry.host_activity =>
            {
                Some(SnapshotWrite::ActivityTransition)
            }
            (_, ConnectionObservation::Telemetry(_)) => Some(SnapshotWrite::Periodic),
            _ => None,
        };
        let telemetry_observed = matches!(&observation, ConnectionObservation::Telemetry(_));
        let mut next = merge_status(previous.as_ref(), observation, setup, hydrated, now);
        if let Some(write) = write {
            let snapshot = StoredTelemetrySnapshot {
                printer_id: printer_id.to_string(),
                telemetry: next.telemetry.clone(),
                last_observed_at: next
                    .last_observed_at
                    .clone()
                    .unwrap_or_else(|| format_time(now)),
            };
            if let Err(error) = self.repository.save_if_due(&snapshot, write) {
                eprintln!("farm3d: cannot cache Printer telemetry: {error}");
                self.record_cache_warning(Some(printer_id), StatusCacheWarningOperation::Save);
            } else {
                self.clear_cache_warning(Some(printer_id), StatusCacheWarningOperation::Save);
            }
        }
        next.cache_warnings = self.cache_warnings_for(printer_id);
        self.publish_changed(printer_id, next, hydrated && !telemetry_observed);
    }

    pub fn apply_connection_error(
        &self,
        printer_id: &str,
        message: impl Into<String>,
        setup: PrinterSetupFacts,
    ) {
        let (previous, hydrated) = self.statuses.status_and_hydration(printer_id);
        let previous = previous.unwrap_or_else(|| PrinterStatus::new(ConnectionState::Error));
        let cache_warnings = previous.cache_warnings.clone();
        let mut next = status_from_parts(
            ConnectionState::Error,
            Some(message.into()),
            previous.telemetry,
            previous.last_observed_at.as_deref().and_then(parse_time),
            previous.fresh_until.as_deref().and_then(parse_time),
            setup,
            hydrated,
            self.now(),
        );
        next.cache_warnings = cache_warnings;
        self.publish_changed(printer_id, next, hydrated);
    }

    pub fn seed(&self, printer_id: &str, status: PrinterStatus) {
        self.publish_changed(printer_id, status, false);
    }

    pub async fn start(
        &self,
        printer_id: String,
        config: ConnectionConfig,
        api_key: Option<zeroize::Zeroizing<String>>,
        setup: PrinterSetupFacts,
    ) {
        let _ = self.stop_task_and_wait(&printer_id).await;
        let epoch = self.statuses.begin_supervision(&printer_id);
        self.apply_observation(
            &printer_id,
            ConnectionObservation::Health {
                state: ConnectionState::Connecting,
                observed_at: format_time(self.now()),
            },
            setup,
        );
        let app = self.app.clone();
        let statuses = Arc::clone(&self.statuses);
        let repository = Arc::clone(&self.repository);
        let clock = Arc::clone(&self.clock);
        let connection_factory = Arc::clone(&self.connection_factory);
        let cache_warnings = Arc::clone(&self.cache_warnings);
        let id = printer_id.clone();
        let (stop, mut stop_requested) = tokio::sync::watch::channel(false);
        let handle = tauri::async_runtime::spawn(async move {
            let mut attempt = 0;
            loop {
                let (tx, mut rx) = tokio::sync::mpsc::channel(STATUS_CHANNEL_DEPTH);
                let connection = connection_factory(&config, api_key.clone());
                let forward = {
                    let app = app.clone();
                    let statuses = Arc::clone(&statuses);
                    let repository = Arc::clone(&repository);
                    let id = id.clone();
                    let clock = Arc::clone(&clock);
                    let cache_warnings = Arc::clone(&cache_warnings);
                    tauri::async_runtime::spawn(async move {
                        while let Some(observation) = rx.recv().await {
                            apply_observation_to(
                                &app,
                                &statuses,
                                &repository,
                                &cache_warnings,
                                &id,
                                observation,
                                setup,
                                (clock)(),
                                epoch,
                            );
                        }
                    })
                };
                let outcome = match connection {
                    Some(connection) => tokio::select! {
                        outcome = connection.subscribe(tx) => Some(outcome),
                        _ = stop_requested.changed() => None,
                    },
                    None => {
                        apply_error_to(
                            &app,
                            &statuses,
                            &id,
                            "This Connection kind is not supported by this build.",
                            setup,
                            (clock)(),
                        );
                        None
                    }
                };
                let _ = forward.await;
                if outcome.is_none() || *stop_requested.borrow() {
                    return;
                }
                match outcome.expect("checked") {
                    Ok(()) => attempt = 0,
                    Err(error) => apply_error_to(
                        &app,
                        &statuses,
                        &id,
                        status_message(&error),
                        setup,
                        (clock)(),
                    ),
                }
                tokio::select! { _ = tokio::time::sleep(backoff_delay(attempt)) => attempt = attempt.saturating_add(1), _ = stop_requested.changed() => return }
            }
        });
        self.tasks
            .lock()
            .expect("supervisor map lock")
            .insert(printer_id, SupervisorTask { stop, handle });
    }

    pub async fn report_error(
        &self,
        printer_id: &str,
        message: impl Into<String>,
        setup: PrinterSetupFacts,
    ) {
        let _ = self.stop_task_and_wait(printer_id).await;
        self.apply_connection_error(printer_id, message, setup);
    }

    pub async fn stop(&self, printer_id: &str) -> bool {
        let graceful = self.stop_task_and_wait(printer_id).await;
        self.clear_cache_warnings_for(printer_id);
        self.publish_removed(printer_id);
        graceful
    }

    /// Removes a Printer's live status and publishes `printer.status.removed`
    /// without stopping a supervisor task or touching the telemetry
    /// snapshot row. For a Printer under active supervision, `stop` is the
    /// right call; `forget` is for a status that was never backed by a
    /// running task (e.g. a probe's transient status).
    pub fn forget(&self, printer_id: &str) {
        self.publish_removed(printer_id);
    }

    /// Builds a `PrinterConnection` for `config` through the manager's
    /// (possibly test-injected) connection factory, without starting any
    /// supervision around it. Exposed so a probe can reuse the same factory
    /// the supervisor uses instead of constructing its own adapter.
    pub fn connection_for(
        &self,
        config: &ConnectionConfig,
        api_key: Option<zeroize::Zeroizing<String>>,
    ) -> Option<Box<dyn PrinterConnection>> {
        (self.connection_factory)(config, api_key)
    }

    pub async fn stop_and_wait(&self, printer_id: &str) -> bool {
        self.stop(printer_id).await
    }

    fn delete_telemetry_snapshot(&self, printer_id: &str) {
        if let Err(error) = self.repository.delete(printer_id) {
            eprintln!("farm3d: cannot clear telemetry cache: {error}");
            self.record_cache_warning(Some(printer_id), StatusCacheWarningOperation::Delete);
        } else {
            self.clear_cache_warning(Some(printer_id), StatusCacheWarningOperation::Delete);
        }
    }

    /// `clear_connection` for an archived Printer (D6): stops any task and
    /// drops the telemetry snapshot like `clear_connection`, but publishes
    /// removal instead of a live Offline/Setup-incomplete status.
    pub async fn discard_connection(&self, printer_id: &str) -> bool {
        let graceful = self.stop_task_and_wait(printer_id).await;
        self.delete_telemetry_snapshot(printer_id);
        self.clear_cache_warnings_for(printer_id);
        self.publish_removed(printer_id);
        graceful
    }

    pub async fn clear_connection(&self, printer_id: &str, profile_resolved: bool) -> bool {
        let graceful = self.stop_task_and_wait(printer_id).await;
        self.delete_telemetry_snapshot(printer_id);
        let now = self.now();
        let mut next = status_from_parts(
            ConnectionState::Offline,
            None,
            empty_telemetry(),
            None,
            None,
            PrinterSetupFacts {
                has_usable_connection: false,
                profile_resolved,
            },
            false,
            now,
        );
        next.cache_warnings = self.cache_warnings_for(printer_id);
        self.publish_changed(printer_id, next, false);
        graceful
    }

    async fn stop_task_and_wait(&self, printer_id: &str) -> bool {
        let task = self
            .tasks
            .lock()
            .expect("supervisor map lock")
            .remove(printer_id);
        let Some(mut task) = task else {
            return true;
        };
        let _ = task.stop.send(true);
        if tokio::time::timeout(GRACEFUL_STOP_TIMEOUT, &mut task.handle)
            .await
            .is_ok()
        {
            true
        } else {
            task.handle.abort();
            let _ = task.handle.await;
            false
        }
    }
    pub async fn reconciliation_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.reconciliation.lock().await
    }

    fn publish_changed(&self, id: &str, status: PrinterStatus, hydrated: bool) {
        let event = self.statuses.publish_changed(id, status, hydrated);
        let _ = self.app.emit(STATUS_EVENT, event);
    }
    fn publish_removed(&self, id: &str) {
        let event = self.statuses.publish_removed(id);
        let _ = self.app.emit(STATUS_EVENT, event);
    }
}

fn apply_observation_to<R: tauri::Runtime>(
    app: &AppHandle<R>,
    statuses: &StatusMap,
    repository: &StatusRepository,
    cache_warnings: &Mutex<Vec<StatusCacheWarning>>,
    id: &str,
    observation: ConnectionObservation,
    setup: PrinterSetupFacts,
    now: DateTime<Utc>,
    epoch: u64,
) {
    let (previous, hydrated) = statuses.status_and_hydration(id);
    let write = match (&previous, &observation) {
        (Some(status), ConnectionObservation::Telemetry(telemetry))
            if status.telemetry.host_activity != telemetry.host_activity =>
        {
            Some(SnapshotWrite::ActivityTransition)
        }
        (_, ConnectionObservation::Telemetry(_)) => Some(SnapshotWrite::Periodic),
        _ => None,
    };
    let telemetry_observed = matches!(&observation, ConnectionObservation::Telemetry(_));
    let mut next = merge_status(previous.as_ref(), observation, setup, hydrated, now);
    if let Some(write) = write {
        let snapshot = StoredTelemetrySnapshot {
            printer_id: id.to_string(),
            telemetry: next.telemetry.clone(),
            last_observed_at: next
                .last_observed_at
                .clone()
                .unwrap_or_else(|| format_time(now)),
        };
        if let Err(error) = repository.save_if_due(&snapshot, write) {
            eprintln!("farm3d: cannot cache Printer telemetry: {error}");
            record_cache_warning_in(cache_warnings, Some(id), StatusCacheWarningOperation::Save);
        } else {
            clear_cache_warning_in(cache_warnings, Some(id), StatusCacheWarningOperation::Save);
        }
    }
    next.cache_warnings = cache_warnings_for(cache_warnings, id);
    if let Some(event) =
        statuses.publish_changed_if_current(id, epoch, next, hydrated && !telemetry_observed)
    {
        let _ = app.emit(STATUS_EVENT, event);
    }
}

fn apply_error_to<R: tauri::Runtime>(
    app: &AppHandle<R>,
    statuses: &StatusMap,
    id: &str,
    message: impl Into<String>,
    setup: PrinterSetupFacts,
    now: DateTime<Utc>,
) {
    let (previous, hydrated) = statuses.status_and_hydration(id);
    let previous = previous.unwrap_or_else(|| PrinterStatus::new(ConnectionState::Error));
    let cache_warnings = previous.cache_warnings.clone();
    let mut next = status_from_parts(
        ConnectionState::Error,
        Some(message.into()),
        previous.telemetry,
        previous.last_observed_at.as_deref().and_then(parse_time),
        previous.fresh_until.as_deref().and_then(parse_time),
        setup,
        hydrated,
        now,
    );
    next.cache_warnings = cache_warnings;
    let event = statuses.publish_changed(id, next, hydrated);
    let _ = app.emit(STATUS_EVENT, event);
}

fn build(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    (config.kind == MOONRAKER_KIND).then(|| {
        Box::new(MoonrakerConnection::with_zeroizing_secret(
            config.clone(),
            api_key,
        )) as Box<dyn PrinterConnection>
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::printers::operational::{OperationalState, TelemetryFreshness};
    use crate::printers::repository::PrinterRepository;
    use crate::printers::StoredPrinter;
    use chrono::{TimeZone, Utc};
    use tauri::Listener;

    #[test]
    fn telemetry_observation_sets_a_thirty_second_freshness_window() {
        let now = Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 0).unwrap();
        let status = merge_status(
            None,
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Idle,
                host_activity_name: Some("standby".to_string()),
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
            false,
            now,
        );
        assert_eq!(
            status.last_observed_at.as_deref(),
            Some("2026-09-18T12:00:00Z")
        );
        assert_eq!(status.fresh_until.as_deref(), Some("2026-09-18T12:00:30Z"));
    }

    #[test]
    fn error_transition_retains_telemetry_and_marks_it_stale() {
        let now = Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 0).unwrap();
        let live = merge_status(
            None,
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Printing,
                host_activity_name: Some("printing".to_string()),
                job_name: None,
                progress: Some(0.5),
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
            false,
            now,
        );
        let error = status_from_parts(
            ConnectionState::Error,
            Some("unreachable".to_string()),
            live.telemetry,
            live.last_observed_at.as_deref().and_then(parse_time),
            live.fresh_until.as_deref().and_then(parse_time),
            PrinterSetupFacts::complete(),
            false,
            now,
        );
        assert_eq!(error.telemetry.nozzle_temp_c, Some(215.0));
        assert_eq!(error.freshness, TelemetryFreshness::Stale);
        assert_eq!(error.operational_state, OperationalState::Error);
    }

    #[test]
    fn changed_event_payload_uses_the_backfill_status_shape() {
        let statuses = StatusMap::default();
        let status = PrinterStatus::new(ConnectionState::Online);
        let event = statuses.publish_changed("prn-1", status, false);
        let PrinterStatusEventPayload::Changed { ref status } = event.payload else {
            panic!("expected changed payload")
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::to_value(statuses.backfill().statuses[0].status.clone()).unwrap()
        );
    }

    #[test]
    fn generated_status_event_is_the_emitted_envelope_not_a_wrapper() {
        let event = StatusMap::default().publish_changed(
            "prn-1",
            PrinterStatus::new(ConnectionState::Online),
            false,
        );
        let value = serde_json::to_value(event).unwrap();

        assert!(value.get("event").is_none());
        assert_eq!(value["type"], "printer.status.changed");
        assert_eq!(value["payload"]["type"], "changed");
    }

    #[test]
    fn terminal_removal_fences_an_accepted_buffered_observation() {
        let statuses = StatusMap::default();
        let epoch = statuses.begin_supervision("prn-1");
        statuses.publish_removed("prn-1");

        let republished = statuses.publish_changed_if_current(
            "prn-1",
            epoch,
            PrinterStatus::new(ConnectionState::Online),
            false,
        );

        assert!(republished.is_none());
        assert!(statuses.backfill().statuses.is_empty());
    }

    #[test]
    fn backoff_grows_then_holds_at_a_ceiling() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(6), MAX_BACKOFF);
        assert_eq!(backoff_delay(u32::MAX), MAX_BACKOFF);
    }

    #[tokio::test]
    async fn liveness_health_observation_is_emitted_without_telemetry() {
        let (tx, mut rx) = tokio::sync::mpsc::channel(1);

        assert!(!crate::connections::moonraker::send_health(&tx, ConnectionState::Online).await);

        assert!(matches!(
            rx.recv().await,
            Some(ConnectionObservation::Health {
                state: ConnectionState::Online,
                ..
            })
        ));
    }

    #[test]
    fn liveness_extends_freshness_only_after_live_telemetry() {
        let observed_at = Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 0).unwrap();
        let telemetry = PrinterTelemetry {
            host_activity: HostActivity::Idle,
            host_activity_name: Some("standby".to_string()),
            job_name: None,
            progress: None,
            nozzle_temp_c: Some(215.0),
            nozzle_target_c: None,
            bed_temp_c: None,
            bed_target_c: None,
            print_duration_s: None,
            tools: Vec::new(),
        };
        let live = merge_status(
            None,
            ConnectionObservation::Telemetry(telemetry),
            PrinterSetupFacts::complete(),
            false,
            observed_at,
        );

        let status = merge_status(
            Some(&live),
            ConnectionObservation::Health {
                state: ConnectionState::Online,
                observed_at: format_time(observed_at + ChronoDuration::seconds(10)),
            },
            PrinterSetupFacts::complete(),
            false,
            observed_at + ChronoDuration::seconds(31),
        );

        assert_eq!(status.last_observed_at, live.last_observed_at);
        assert_ne!(status.fresh_until, live.fresh_until);
        assert_eq!(status.freshness, TelemetryFreshness::Fresh);
    }

    #[test]
    fn online_health_keeps_hydrated_cache_telemetry_stale() {
        let now = Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 31).unwrap();
        let cached = merge_status(
            None,
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Idle,
                host_activity_name: Some("standby".to_string()),
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
            false,
            now - ChronoDuration::seconds(31),
        );
        let status = merge_status(
            Some(&cached),
            ConnectionObservation::Health {
                state: ConnectionState::Online,
                observed_at: format_time(now),
            },
            PrinterSetupFacts::complete(),
            true,
            now,
        );

        assert_eq!(status.telemetry.nozzle_temp_c, Some(215.0));
        assert_eq!(status.freshness, TelemetryFreshness::Stale);
    }

    #[test]
    fn hydrated_cache_status_starts_stale() {
        let (_root, _lease, storage) = crate::test_storage();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-1".to_string(),
                name: "Cached Printer".to_string(),
                ..Default::default()
            })
            .unwrap();
        let repository = Arc::new(StatusRepository::new(Arc::clone(&storage)));
        repository
            .save_if_due(
                &StoredTelemetrySnapshot {
                    printer_id: "prn-1".to_string(),
                    telemetry: PrinterTelemetry {
                        host_activity: HostActivity::Printing,
                        host_activity_name: Some("printing".to_string()),
                        job_name: None,
                        progress: Some(0.5),
                        nozzle_temp_c: Some(215.0),
                        nozzle_target_c: None,
                        bed_temp_c: None,
                        bed_target_c: None,
                        print_duration_s: None,
                        tools: Vec::new(),
                    },
                    last_observed_at: "2026-09-18T12:00:00Z".to_string(),
                },
                SnapshotWrite::Periodic,
            )
            .unwrap();
        let app = tauri::test::mock_app();

        let manager = ConnectionManager::new(app.handle().clone(), repository);
        let status = manager.statuses().remove("prn-1").unwrap();

        assert_eq!(status.telemetry.nozzle_temp_c, Some(215.0));
        assert_eq!(status.freshness, TelemetryFreshness::Stale);
    }

    #[tokio::test]
    async fn supervisor_transitions_preserve_setup_incomplete_facts() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let manager = ConnectionManager::with_clock_and_factory(
            app.handle().clone(),
            Arc::new(StatusRepository::new(storage)),
            || Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 0).unwrap(),
            |_config, _key| None,
        );

        manager
            .start(
                "prn-1".to_string(),
                ConnectionConfig {
                    kind: "unsupported".to_string(),
                    host: "printer.local".to_string(),
                    port: 7125,
                    use_tls: false,
                    credential_ref: None,
                },
                None,
                PrinterSetupFacts {
                    has_usable_connection: true,
                    profile_resolved: false,
                },
            )
            .await;
        tokio::task::yield_now().await;

        assert_eq!(
            manager.statuses()["prn-1"].operational_state,
            OperationalState::SetupIncomplete
        );
    }

    #[tokio::test]
    async fn manager_error_retains_live_readings_as_stale() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let manager = ConnectionManager::new(
            app.handle().clone(),
            Arc::new(StatusRepository::new(Arc::clone(&storage))),
        );
        manager.apply_observation(
            "prn-1",
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Printing,
                host_activity_name: Some("printing".to_string()),
                job_name: None,
                progress: Some(0.5),
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
        );
        manager
            .report_error("prn-1", "unreachable", PrinterSetupFacts::complete())
            .await;

        let status = manager.statuses()["prn-1"].clone();
        assert_eq!(status.telemetry.nozzle_temp_c, Some(215.0));
        assert_eq!(status.operational_state, OperationalState::Error);
        assert_eq!(status.freshness, TelemetryFreshness::Stale);
    }

    #[test]
    fn cache_write_failure_is_reported_without_replacing_live_status() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let manager = ConnectionManager::new(
            app.handle().clone(),
            Arc::new(StatusRepository::new(Arc::clone(&storage))),
        );
        manager.apply_observation(
            "prn-1",
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Idle,
                host_activity_name: None,
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
        );

        assert_eq!(
            manager.statuses()["prn-1"].connection_state,
            ConnectionState::Online
        );
        assert_eq!(
            manager.cache_warnings()[0].operation,
            StatusCacheWarningOperation::Save
        );
        assert_eq!(
            manager.statuses()["prn-1"].cache_warnings,
            manager.cache_warnings()
        );

        PrinterRepository::new(storage)
            .create(StoredPrinter {
                id: "prn-1".to_string(),
                name: "Recovered cache".to_string(),
                ..Default::default()
            })
            .unwrap();
        manager.apply_observation(
            "prn-1",
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Idle,
                host_activity_name: None,
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
        );

        assert!(manager.cache_warnings().is_empty());
        assert!(manager.statuses()["prn-1"].cache_warnings.is_empty());
    }

    #[test]
    fn cache_read_failure_is_reported_without_blocking_manager_startup() {
        let (_root, _lease, storage) = crate::test_storage();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-1".to_string(),
                name: "Corrupt cache".to_string(),
                ..Default::default()
            })
            .unwrap();
        storage
            .write(|transaction| {
                transaction.execute(
                    "INSERT INTO printer_status_snapshots (printer_id, telemetry_json, last_observed_at, persisted_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params!["prn-1", "{}", "2026-09-18T12:00:00Z", "2026-09-18T12:00:00Z"],
                )?;
                Ok(())
            })
            .unwrap();
        let app = tauri::test::mock_app();

        let manager = ConnectionManager::new(
            app.handle().clone(),
            Arc::new(StatusRepository::new(storage)),
        );

        assert!(manager.statuses().is_empty());
        assert_eq!(
            manager.cache_warnings()[0].operation,
            StatusCacheWarningOperation::Hydrate
        );
    }

    #[test]
    fn durable_printer_without_a_connection_gets_setup_incomplete_without_a_task() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let manager = ConnectionManager::new(
            app.handle().clone(),
            Arc::new(StatusRepository::new(storage)),
        );

        manager.reconcile_printer(
            "prn-1",
            PrinterSetupFacts {
                has_usable_connection: false,
                profile_resolved: true,
            },
        );

        assert_eq!(
            manager.statuses()["prn-1"].operational_state,
            OperationalState::SetupIncomplete
        );
        assert_eq!(
            manager.statuses()["prn-1"].freshness,
            TelemetryFreshness::Unavailable
        );
        assert!(manager.tasks.lock().unwrap().is_empty());
    }

    #[test]
    fn removal_publishes_a_tombstone_and_deletes_the_backfill_entry() {
        let statuses = StatusMap::default();
        statuses.publish_changed("prn-1", PrinterStatus::new(ConnectionState::Online), false);

        let event = statuses.publish_removed("prn-1");

        assert!(matches!(event.payload, PrinterStatusEventPayload::Removed));
        assert!(statuses.backfill().statuses.is_empty());
    }

    #[tokio::test]
    async fn removing_a_printer_clears_only_its_cache_warnings_and_keeps_backfill_in_sequence() {
        let (_root, _lease, storage) = crate::test_storage();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-hydrate".to_string(),
                name: "Malformed cache".to_string(),
                ..Default::default()
            })
            .unwrap();
        storage
            .write(|transaction| {
                transaction.execute(
                    "INSERT INTO printer_status_snapshots (printer_id, telemetry_json, last_observed_at, persisted_at) VALUES (?1, ?2, ?3, ?4)",
                    rusqlite::params!["prn-hydrate", "{}", "2026-09-18T12:00:00Z", "2026-09-18T12:00:00Z"],
                )?;
                Ok(())
            })
            .unwrap();
        let app = tauri::test::mock_app();
        let manager = ConnectionManager::new(
            app.handle().clone(),
            Arc::new(StatusRepository::new(storage)),
        );
        let (events_tx, events_rx) = std::sync::mpsc::channel();
        app.listen(STATUS_EVENT, move |event| {
            events_tx.send(event.payload().to_string()).unwrap();
        });
        manager.apply_observation(
            "prn-1",
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Idle,
                host_activity_name: None,
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
        );
        assert_eq!(
            manager.statuses()["prn-1"].cache_warnings[0].operation,
            StatusCacheWarningOperation::Save
        );
        let _: serde_json::Value = serde_json::from_str(&events_rx.recv().unwrap()).unwrap();
        manager.apply_observation(
            "prn-2",
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Idle,
                host_activity_name: None,
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
        );
        assert_eq!(
            manager.statuses()["prn-2"].cache_warnings,
            vec![StatusCacheWarning {
                printer_id: Some("prn-2".to_string()),
                operation: StatusCacheWarningOperation::Save,
            }]
        );
        let _: serde_json::Value = serde_json::from_str(&events_rx.recv().unwrap()).unwrap();

        manager.stop_and_wait("prn-1").await;

        let tombstone: serde_json::Value =
            serde_json::from_str(&events_rx.recv().unwrap()).unwrap();
        let backfill = manager.status_backfill();
        assert_eq!(tombstone["type"], "printer.status.removed");
        assert_eq!(tombstone["subject"]["id"], "prn-1");
        assert!(!manager.statuses().contains_key("prn-1"));
        assert!(manager.statuses().contains_key("prn-2"));
        assert!(backfill
            .statuses
            .iter()
            .all(|row| row.printer_id != "prn-1"));
        assert_eq!(backfill.statuses.len(), 1);
        assert_eq!(backfill.statuses[0].printer_id, "prn-2");
        assert_eq!(
            serde_json::to_value(&backfill).unwrap()["snapshotSequence"],
            tombstone["sequence"]
        );
        assert_eq!(
            backfill.cache_warnings,
            vec![
                StatusCacheWarning {
                    printer_id: None,
                    operation: StatusCacheWarningOperation::Hydrate,
                },
                StatusCacheWarning {
                    printer_id: Some("prn-2".to_string()),
                    operation: StatusCacheWarningOperation::Save,
                },
            ]
        );
    }

    #[tokio::test]
    async fn clearing_a_connection_drops_cached_telemetry_and_publishes_setup_incomplete() {
        let (_root, _lease, storage) = crate::test_storage();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-1".to_string(),
                name: "Clear connection".to_string(),
                ..Default::default()
            })
            .unwrap();
        let repository = Arc::new(StatusRepository::new(Arc::clone(&storage)));
        let app = tauri::test::mock_app();
        let manager = ConnectionManager::new(app.handle().clone(), Arc::clone(&repository));
        manager.apply_observation(
            "prn-1",
            ConnectionObservation::Telemetry(PrinterTelemetry {
                host_activity: HostActivity::Idle,
                host_activity_name: Some("standby".to_string()),
                job_name: None,
                progress: None,
                nozzle_temp_c: Some(215.0),
                nozzle_target_c: None,
                bed_temp_c: None,
                bed_target_c: None,
                print_duration_s: None,
                tools: Vec::new(),
            }),
            PrinterSetupFacts::complete(),
        );

        manager.clear_connection("prn-1", true).await;

        let status = manager.statuses()["prn-1"].clone();
        assert_eq!(status.operational_state, OperationalState::SetupIncomplete);
        assert_eq!(status.freshness, TelemetryFreshness::Unavailable);
        assert_eq!(status.telemetry.nozzle_temp_c, None);
        assert!(repository.get("prn-1").unwrap().is_none());
    }

    #[test]
    fn forget_removes_the_status_and_publishes_a_tombstone_without_touching_the_snapshot_row() {
        let (_root, _lease, storage) = crate::test_storage();
        PrinterRepository::new(Arc::clone(&storage))
            .create(StoredPrinter {
                id: "prn-1".to_string(),
                name: "Forget me".to_string(),
                ..Default::default()
            })
            .unwrap();
        let repository = Arc::new(StatusRepository::new(Arc::clone(&storage)));
        repository
            .save_if_due(
                &StoredTelemetrySnapshot {
                    printer_id: "prn-1".to_string(),
                    telemetry: PrinterTelemetry {
                        host_activity: HostActivity::Idle,
                        host_activity_name: None,
                        job_name: None,
                        progress: None,
                        nozzle_temp_c: Some(200.0),
                        nozzle_target_c: None,
                        bed_temp_c: None,
                        bed_target_c: None,
                        print_duration_s: None,
                        tools: Vec::new(),
                    },
                    last_observed_at: "2026-09-18T12:00:00Z".to_string(),
                },
                SnapshotWrite::Periodic,
            )
            .unwrap();
        let app = tauri::test::mock_app();
        let manager = ConnectionManager::new(app.handle().clone(), Arc::clone(&repository));
        // Startup hydration already seeded prn-1 into statuses() from the
        // snapshot saved above.
        assert!(manager.statuses().contains_key("prn-1"));

        manager.forget("prn-1");

        assert!(!manager.statuses().contains_key("prn-1"));
        assert!(
            repository.get("prn-1").unwrap().is_some(),
            "forget must not touch the snapshot row"
        );
        let backfill = manager.status_backfill();
        assert!(backfill
            .statuses
            .iter()
            .all(|row| row.printer_id != "prn-1"));
    }

    #[test]
    fn connection_for_calls_the_injected_factory() {
        let (_root, _lease, storage) = crate::test_storage();
        let app = tauri::test::mock_app();
        let calls = Arc::new(Mutex::new(0));
        let counter = Arc::clone(&calls);
        let manager = ConnectionManager::with_clock_and_factory(
            app.handle().clone(),
            Arc::new(StatusRepository::new(storage)),
            Utc::now,
            move |_config, _key| {
                *counter.lock().expect("call counter lock") += 1;
                None
            },
        );
        let config = ConnectionConfig {
            kind: MOONRAKER_KIND.to_string(),
            host: "printer.local".to_string(),
            port: 7125,
            use_tls: false,
            credential_ref: None,
        };

        let connection = manager.connection_for(&config, None);

        assert!(connection.is_none());
        assert_eq!(*calls.lock().expect("call counter lock"), 1);
    }
}
