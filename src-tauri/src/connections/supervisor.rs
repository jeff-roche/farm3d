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
    PrinterStatus, MOONRAKER_KIND,
};
use crate::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use crate::printers::operational::{evaluate_operational_status, HostActivity, OperationalInput};

pub const STATUS_EVENT: &str = "farm3d-event-v1";
const MAX_BACKOFF: Duration = Duration::from_secs(60);
const STATUS_CHANNEL_DEPTH: usize = 16;
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(2);
const TELEMETRY_FRESH_FOR: ChronoDuration = ChronoDuration::seconds(30);

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

pub type PrinterStatusEvent = EventEnvelope<PrinterStatusEventType, PrinterStatusEventPayload>;

#[derive(TS)]
#[ts(export_to = "domain/PrinterStatusEvent.ts")]
pub struct PrinterStatusEventContract {
    pub event: PrinterStatusEvent,
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
    sequence: u64,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            values: HashMap::new(),
            hydrated: HashSet::new(),
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

    fn publish_removed(&self, id: &str) -> PrinterStatusEvent {
        let mut state = self.state.lock().expect("status map lock");
        state.values.remove(id);
        state.hydrated.remove(id);
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
    EventEnvelope {
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
    }
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
            // Liveness says only that the transport is alive. It must never
            // turn retained telemetry into a new observation or extend its
            // thirty-second freshness window.
            let _ = observed_at;
            status_from_parts(
                state,
                None,
                previous.telemetry,
                previous.last_observed_at.as_deref().and_then(parse_time),
                previous.fresh_until.as_deref().and_then(parse_time),
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
        let clock = Arc::new(clock);
        let statuses = Arc::new(StatusMap::default());
        match repository.list() {
            Ok(snapshots) => {
                for snapshot in snapshots {
                    statuses.seed_hydrated(snapshot, clock());
                }
            }
            Err(error) => eprintln!("farm3d: cannot hydrate telemetry cache: {error}"),
        }
        Self {
            app,
            tasks: Mutex::new(HashMap::new()),
            statuses,
            repository,
            clock,
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
        self.statuses.backfill()
    }

    pub fn reconcile_printer(&self, printer_id: &str, setup: PrinterSetupFacts) {
        let (previous, hydrated) = self.statuses.status_and_hydration(printer_id);
        let previous = previous.unwrap_or_else(|| PrinterStatus::new(ConnectionState::Offline));
        let state = if setup.has_usable_connection {
            previous.connection_state
        } else {
            ConnectionState::Offline
        };
        let next = status_from_parts(
            state,
            previous.error,
            previous.telemetry,
            previous.last_observed_at.as_deref().and_then(parse_time),
            previous.fresh_until.as_deref().and_then(parse_time),
            setup,
            hydrated,
            self.now(),
        );
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
        let live = matches!(
            &observation,
            ConnectionObservation::Telemetry(_)
                | ConnectionObservation::Health {
                    state: ConnectionState::Online,
                    ..
                }
        );
        let next = merge_status(previous.as_ref(), observation, setup, hydrated, now);
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
            }
        }
        self.publish_changed(printer_id, next, hydrated && !live);
    }

    pub fn apply_connection_error(
        &self,
        printer_id: &str,
        message: impl Into<String>,
        setup: PrinterSetupFacts,
    ) {
        let (previous, hydrated) = self.statuses.status_and_hydration(printer_id);
        let previous = previous.unwrap_or_else(|| PrinterStatus::new(ConnectionState::Error));
        let next = status_from_parts(
            ConnectionState::Error,
            Some(message.into()),
            previous.telemetry,
            previous.last_observed_at.as_deref().and_then(parse_time),
            previous.fresh_until.as_deref().and_then(parse_time),
            setup,
            hydrated,
            self.now(),
        );
        self.publish_changed(printer_id, next, hydrated);
    }

    pub fn seed(&self, printer_id: &str, status: PrinterStatus) {
        self.publish_changed(printer_id, status, false);
    }

    pub fn start(
        &self,
        printer_id: String,
        config: ConnectionConfig,
        api_key: Option<zeroize::Zeroizing<String>>,
    ) {
        self.stop_task(&printer_id);
        self.apply_observation(
            &printer_id,
            ConnectionObservation::Health {
                state: ConnectionState::Connecting,
                observed_at: format_time(self.now()),
            },
            PrinterSetupFacts::complete(),
        );
        let app = self.app.clone();
        let statuses = Arc::clone(&self.statuses);
        let repository = Arc::clone(&self.repository);
        let clock = Arc::clone(&self.clock);
        let id = printer_id.clone();
        let (stop, mut stop_requested) = tokio::sync::watch::channel(false);
        let handle = tauri::async_runtime::spawn(async move {
            let mut attempt = 0;
            loop {
                let (tx, mut rx) = tokio::sync::mpsc::channel(STATUS_CHANNEL_DEPTH);
                let connection = build(&config, api_key.clone());
                let forward = {
                    let app = app.clone();
                    let statuses = Arc::clone(&statuses);
                    let repository = Arc::clone(&repository);
                    let id = id.clone();
                    let clock = Arc::clone(&clock);
                    tauri::async_runtime::spawn(async move {
                        while let Some(observation) = rx.recv().await {
                            apply_observation_to(
                                &app,
                                &statuses,
                                &repository,
                                &id,
                                observation,
                                PrinterSetupFacts::complete(),
                                (clock)(),
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
                            PrinterSetupFacts::complete(),
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
                        PrinterSetupFacts::complete(),
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

    pub fn report_error(&self, printer_id: &str, message: impl Into<String>) {
        self.stop_task(printer_id);
        self.apply_connection_error(printer_id, message, PrinterSetupFacts::complete());
    }

    pub fn stop(&self, printer_id: &str) {
        self.stop_task(printer_id);
        self.publish_removed(printer_id);
    }

    pub async fn stop_and_wait(&self, printer_id: &str) -> bool {
        let graceful = self.stop_task_and_wait(printer_id).await;
        self.publish_removed(printer_id);
        graceful
    }

    pub async fn clear_connection(&self, printer_id: &str, profile_resolved: bool) -> bool {
        let graceful = self.stop_task_and_wait(printer_id).await;
        if let Err(error) = self.repository.delete(printer_id) {
            eprintln!("farm3d: cannot clear telemetry cache: {error}");
        }
        let now = self.now();
        let next = status_from_parts(
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
        self.publish_changed(printer_id, next, false);
        graceful
    }

    fn stop_task(&self, printer_id: &str) {
        if let Some(task) = self
            .tasks
            .lock()
            .expect("supervisor map lock")
            .remove(printer_id)
        {
            let _ = task.stop.send(true);
            task.handle.abort();
        }
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
    id: &str,
    observation: ConnectionObservation,
    setup: PrinterSetupFacts,
    now: DateTime<Utc>,
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
    let live = matches!(
        &observation,
        ConnectionObservation::Telemetry(_)
            | ConnectionObservation::Health {
                state: ConnectionState::Online,
                ..
            }
    );
    let next = merge_status(previous.as_ref(), observation, setup, hydrated, now);
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
        }
    }
    let event = statuses.publish_changed(id, next, hydrated && !live);
    let _ = app.emit(STATUS_EVENT, event);
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
    let next = status_from_parts(
        ConnectionState::Error,
        Some(message.into()),
        previous.telemetry,
        previous.last_observed_at.as_deref().and_then(parse_time),
        previous.fresh_until.as_deref().and_then(parse_time),
        setup,
        hydrated,
        now,
    );
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
        let PrinterStatusEventPayload::Changed { status } = event.payload else {
            panic!("expected changed payload")
        };
        assert_eq!(
            serde_json::to_value(status).unwrap(),
            serde_json::to_value(statuses.backfill().statuses[0].status.clone()).unwrap()
        );
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
    fn liveness_does_not_extend_the_telemetry_freshness_window() {
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
        assert_eq!(status.fresh_until, live.fresh_until);
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
}
