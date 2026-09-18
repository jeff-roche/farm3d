//! One long-lived task per connected Printer.
//!
//! The supervisor exists so no adapter implements retry. An adapter's
//! `subscribe` returning — cleanly or with an error — is this module's cue to
//! back off and reconnect. That keeps phase 3's adapters simple and keeps the
//! reconnect policy in exactly one place.

use super::moonraker::MoonrakerConnection;
use super::{ConnectionConfig, ConnectionState, PrinterConnection, PrinterStatus, MOONRAKER_KIND};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::async_runtime::JoinHandle;
use tauri::{AppHandle, Emitter};

pub const STATUS_EVENT: &str = "farm3d-event-v1";

const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Bounded so one wedged printer cannot stall the others behind a full queue.
const STATUS_CHANNEL_DEPTH: usize = 16;
const GRACEFUL_STOP_TIMEOUT: Duration = Duration::from_secs(2);

struct SupervisorTask {
    stop: tokio::sync::watch::Sender<bool>,
    handle: JoinHandle<()>,
}

/// 1s, 2s, 4s … capped at 60s. Saturating throughout: a printer that has been
/// off for days reaches attempt counts that overflow a naive shift.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

fn backoff_after_failure(failures_before_this_one: u32) -> Duration {
    backoff_delay(failures_before_this_one)
}

fn status_message(error: &super::ConnectionError) -> &'static str {
    match error {
        super::ConnectionError::Unreachable(_) => "The Printer could not be reached.",
        super::ConnectionError::Auth(_) => "The Printer rejected authentication.",
        super::ConnectionError::Protocol(_) => "The Printer returned an unexpected response.",
        super::ConnectionError::Timeout => "The Printer did not respond in time.",
    }
}

#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/PrinterStatusBackfill.ts"
)]
pub struct PrinterStatusBackfill {
    pub stream_id: String,
    pub snapshot_sequence: crate::contracts::event::JsSafeInteger,
    pub statuses: Vec<PrinterStatusRow>,
}

use serde::Serialize;

#[derive(Serialize, Clone, Debug, ts_rs::TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterStatusRow.ts")]
pub struct PrinterStatusRow {
    pub printer_id: String,
    pub status: PrinterStatus,
}

struct StatusState {
    stream_id: String,
    values: HashMap<String, PrinterStatus>,
    sequence: u64,
}

impl Default for StatusState {
    fn default() -> Self {
        Self {
            stream_id: uuid::Uuid::new_v4().to_string(),
            values: HashMap::new(),
            sequence: 0,
        }
    }
}

/// Last known status per printer, so a Connection tab opened after a printer
/// came online still renders live values instead of waiting for the next tick.
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
    pub fn set(&self, id: &str, status: PrinterStatus) {
        self.state
            .lock()
            .unwrap()
            .values
            .insert(id.to_string(), status);
    }

    pub fn forget(&self, id: &str) {
        self.state.lock().unwrap().values.remove(id);
    }

    pub fn snapshot(&self) -> HashMap<String, PrinterStatus> {
        self.state.lock().unwrap().values.clone()
    }

    pub fn backfill(&self) -> PrinterStatusBackfill {
        let state = self.state.lock().unwrap();
        let mut statuses: Vec<_> = state
            .values
            .iter()
            .map(|(printer_id, status)| PrinterStatusRow {
                printer_id: printer_id.clone(),
                status: status.clone(),
            })
            .collect();
        statuses.sort_by(|left, right| left.printer_id.as_bytes().cmp(right.printer_id.as_bytes()));
        PrinterStatusBackfill {
            stream_id: state.stream_id.clone(),
            snapshot_sequence: crate::contracts::event::JsSafeInteger::try_from(state.sequence)
                .expect("status sequence is always JS-safe"),
            statuses,
        }
    }

    fn publish(
        &self,
        id: &str,
        status: PrinterStatus,
    ) -> crate::contracts::event::EventEnvelope<String, PrinterStatus> {
        let mut state = self.state.lock().unwrap();
        if state.sequence == crate::contracts::JS_MAX_SAFE_INTEGER {
            state.stream_id = uuid::Uuid::new_v4().to_string();
            state.sequence = 0;
        }
        state.sequence += 1;
        state.values.insert(id.to_string(), status.clone());
        crate::contracts::event::EventEnvelope {
            contract_version: crate::contracts::ContractVersion::V1,
            stream_id: state.stream_id.clone(),
            sequence: crate::contracts::event::JsSafeInteger::try_from(state.sequence)
                .expect("guarded JS-safe sequence"),
            event_id: uuid::Uuid::new_v4().to_string(),
            occurred_at: crate::printers::now_rfc3339(),
            event_type: "printer.status.changed".to_string(),
            subject: crate::contracts::event::EventSubject {
                kind: "printer".to_string(),
                id: id.to_string(),
            },
            payload: status,
        }
    }
}

pub struct ConnectionManager<R: tauri::Runtime> {
    app: AppHandle<R>,
    tasks: Mutex<HashMap<String, SupervisorTask>>,
    statuses: Arc<StatusMap>,
    reconciliation: tokio::sync::Mutex<()>,
}

impl<R: tauri::Runtime> ConnectionManager<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self {
            app,
            tasks: Mutex::new(HashMap::new()),
            statuses: Arc::new(StatusMap::default()),
            reconciliation: tokio::sync::Mutex::new(()),
        }
    }

    pub fn statuses(&self) -> HashMap<String, PrinterStatus> {
        self.statuses.snapshot()
    }

    pub fn status_backfill(&self) -> PrinterStatusBackfill {
        self.statuses.backfill()
    }

    /// Idempotent: starting a printer that is already running replaces its
    /// task, which is what a config edit needs.
    pub fn start(
        &self,
        printer_id: String,
        config: ConnectionConfig,
        api_key: Option<zeroize::Zeroizing<String>>,
    ) {
        self.stop(&printer_id);

        let app = self.app.clone();
        let statuses = Arc::clone(&self.statuses);
        let id = printer_id.clone();

        let (stop, mut stop_requested) = tokio::sync::watch::channel(false);
        let handle = tauri::async_runtime::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                publish(
                    &app,
                    &statuses,
                    &id,
                    PrinterStatus::new(ConnectionState::Connecting),
                );

                let (tx, mut rx) = tokio::sync::mpsc::channel(STATUS_CHANNEL_DEPTH);
                let connection = build(&config, api_key.clone());

                let forward = {
                    let app = app.clone();
                    let statuses = Arc::clone(&statuses);
                    let id = id.clone();
                    tauri::async_runtime::spawn(async move {
                        while let Some(status) = rx.recv().await {
                            publish(&app, &statuses, &id, status);
                        }
                    })
                };

                let outcome = match connection {
                    Some(connection) => tokio::select! {
                        outcome = connection.subscribe(tx) => outcome,
                        changed = stop_requested.changed() => {
                            forward.abort();
                            if changed.is_ok() && *stop_requested.borrow() { return; }
                            return;
                        }
                    },
                    None => {
                        // An unknown `kind` is a phase-3 config read by a
                        // phase-2 build. Report it and stop — retrying a
                        // protocol we cannot speak never succeeds.
                        publish(
                            &app,
                            &statuses,
                            &id,
                            PrinterStatus::errored(
                                "This Connection kind is not supported by this build.",
                            ),
                        );
                        forward.abort();
                        return;
                    }
                };
                forward.abort();

                match outcome {
                    Ok(()) => attempt = 0,
                    Err(e) => {
                        publish(
                            &app,
                            &statuses,
                            &id,
                            PrinterStatus::errored(status_message(&e)),
                        );
                    }
                }
                tokio::select! {
                    () = tokio::time::sleep(backoff_after_failure(attempt)) => {
                        attempt = attempt.saturating_add(1);
                    }
                    changed = stop_requested.changed() => {
                        if changed.is_ok() && *stop_requested.borrow() { return; }
                        return;
                    }
                }
            }
        });

        self.tasks
            .lock()
            .unwrap()
            .insert(printer_id, SupervisorTask { stop, handle });
    }

    /// Publishes a terminal error for a printer WITHOUT starting a connection
    /// task. For failures that no amount of reconnecting can fix — today, a
    /// credential store we cannot read at launch. Starting the normal loop
    /// there would report an *auth* failure forever and point the user at
    /// their API key rather than at the real cause.
    pub fn report_error(&self, printer_id: &str, message: impl Into<String>) {
        self.stop(printer_id);
        publish(
            &self.app,
            &self.statuses,
            printer_id,
            PrinterStatus::errored(message),
        );
    }

    pub fn stop(&self, printer_id: &str) {
        if let Some(task) = self.tasks.lock().unwrap().remove(printer_id) {
            let _ = task.stop.send(true);
            task.handle.abort();
        }
        self.statuses.forget(printer_id);
    }

    pub async fn stop_and_wait(&self, printer_id: &str) -> bool {
        let task = self.tasks.lock().unwrap().remove(printer_id);
        let mut graceful = true;
        if let Some(mut task) = task {
            let _ = task.stop.send(true);
            if tokio::time::timeout(GRACEFUL_STOP_TIMEOUT, &mut task.handle)
                .await
                .is_err()
            {
                graceful = false;
                task.handle.abort();
                let _ = task.handle.await;
            }
        }
        self.statuses.forget(printer_id);
        graceful
    }

    pub async fn reconciliation_guard(&self) -> tokio::sync::MutexGuard<'_, ()> {
        self.reconciliation.lock().await
    }
}

fn build(
    config: &ConnectionConfig,
    api_key: Option<zeroize::Zeroizing<String>>,
) -> Option<Box<dyn PrinterConnection>> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Some(Box::new(MoonrakerConnection::with_zeroizing_secret(
            config.clone(),
            api_key,
        ))),
        _ => None,
    }
}

fn publish<R: tauri::Runtime>(
    app: &AppHandle<R>,
    statuses: &StatusMap,
    id: &str,
    status: PrinterStatus,
) {
    let event = statuses.publish(id, status);
    // A failed emit means the window is gone; the status map is still
    // correct, so there is nothing to recover from here.
    let _ = app.emit(STATUS_EVENT, event);
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, Ordering};

    #[test]
    fn backoff_grows_then_holds_at_a_ceiling() {
        assert_eq!(backoff_delay(0), Duration::from_secs(1));
        assert_eq!(backoff_delay(1), Duration::from_secs(2));
        assert_eq!(backoff_delay(2), Duration::from_secs(4));
        assert_eq!(backoff_delay(5), Duration::from_secs(32));
        // A printer that is simply switched off must not be retried every
        // second forever, nor drift toward never retrying at all.
        assert_eq!(backoff_delay(6), MAX_BACKOFF);
        assert_eq!(backoff_delay(99), MAX_BACKOFF);
    }

    #[test]
    fn first_reconnect_after_a_failure_waits_one_second() {
        assert_eq!(backoff_after_failure(0), Duration::from_secs(1));
        assert_eq!(backoff_after_failure(1), Duration::from_secs(2));
    }

    #[test]
    fn adapter_errors_are_sanitized_before_status_publication() {
        let raw = "wss://user:credential@example.invalid/private raw response";
        for error in [
            crate::connections::ConnectionError::Unreachable(raw.to_string()),
            crate::connections::ConnectionError::Auth(raw.to_string()),
            crate::connections::ConnectionError::Protocol(raw.to_string()),
        ] {
            let safe = status_message(&error);
            assert!(!safe.contains(raw));
            assert!(!safe.contains("credential"));
            assert!(!safe.contains("example.invalid"));
        }
    }

    #[test]
    fn backoff_never_overflows_on_a_long_outage() {
        // A printer offline for days reaches attempt counts that would panic
        // a naive `2u64.pow(attempt)` in debug builds.
        assert_eq!(backoff_delay(u32::MAX), MAX_BACKOFF);
    }

    #[test]
    fn the_status_map_starts_empty_and_records_per_printer() {
        let map = StatusMap::default();
        assert!(map.snapshot().is_empty());
        map.set("prn-1", PrinterStatus::new(ConnectionState::Online));
        map.set("prn-2", PrinterStatus::errored("nope"));
        let snapshot = map.snapshot();
        assert_eq!(snapshot["prn-1"].connection_state, ConnectionState::Online);
        assert_eq!(snapshot["prn-2"].connection_state, ConnectionState::Error);
    }

    #[test]
    fn forgetting_a_printer_drops_its_status() {
        let map = StatusMap::default();
        map.set("prn-1", PrinterStatus::new(ConnectionState::Online));
        map.forget("prn-1");
        assert!(map.snapshot().is_empty());
    }

    #[test]
    fn sequence_rollover_starts_a_new_stream_instead_of_reusing_the_maximum() {
        let map = StatusMap::default();
        let old_stream = map.backfill().stream_id;
        {
            let mut state = map.state.lock().unwrap();
            state.sequence = crate::contracts::JS_MAX_SAFE_INTEGER;
        }

        let event = map.publish("prn-1", PrinterStatus::new(ConnectionState::Online));

        assert_eq!(event.sequence.get(), 1);
        assert_ne!(event.stream_id, old_stream);
        let snapshot = map.backfill();
        assert_eq!(snapshot.stream_id, event.stream_id);
        assert_eq!(snapshot.snapshot_sequence.get(), 1);
    }

    #[test]
    fn stop_and_wait_joins_the_aborted_task() {
        struct Dropped(Arc<AtomicBool>);

        impl Drop for Dropped {
            fn drop(&mut self) {
                self.0.store(true, Ordering::SeqCst);
            }
        }

        let app = tauri::test::mock_app();
        let manager = ConnectionManager::new(app.handle().clone());
        let dropped = Arc::new(AtomicBool::new(false));
        let task_dropped = Arc::clone(&dropped);
        let (started_tx, started_rx) = std::sync::mpsc::sync_channel(1);
        let handle = tauri::async_runtime::spawn(async move {
            let _guard = Dropped(task_dropped);
            started_tx.send(()).unwrap();
            std::future::pending::<()>().await;
        });
        let (stop, _receiver) = tokio::sync::watch::channel(false);
        manager
            .tasks
            .lock()
            .unwrap()
            .insert("prn-1".to_string(), SupervisorTask { stop, handle });
        started_rx.recv().unwrap();

        assert!(!tauri::async_runtime::block_on(
            manager.stop_and_wait("prn-1")
        ));

        assert!(dropped.load(Ordering::SeqCst));
    }
}
