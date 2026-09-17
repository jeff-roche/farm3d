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

/// Frontend event name. The payload is `{ id, status }`.
pub const STATUS_EVENT: &str = "printer-status";

const MAX_BACKOFF: Duration = Duration::from_secs(60);
/// Bounded so one wedged printer cannot stall the others behind a full queue.
const STATUS_CHANNEL_DEPTH: usize = 16;

/// 1s, 2s, 4s … capped at 60s. Saturating throughout: a printer that has been
/// off for days reaches attempt counts that overflow a naive shift.
pub fn backoff_delay(attempt: u32) -> Duration {
    let secs = 1u64.checked_shl(attempt).unwrap_or(u64::MAX);
    Duration::from_secs(secs).min(MAX_BACKOFF)
}

#[derive(Serialize, Clone)]
#[serde(rename_all = "camelCase")]
struct StatusEvent {
    id: String,
    status: PrinterStatus,
}

use serde::Serialize;

/// Last known status per printer, so a Connection tab opened after a printer
/// came online still renders live values instead of waiting for the next tick.
#[derive(Default)]
pub struct StatusMap(Mutex<HashMap<String, PrinterStatus>>);

impl StatusMap {
    pub fn set(&self, id: &str, status: PrinterStatus) {
        self.0.lock().unwrap().insert(id.to_string(), status);
    }

    pub fn forget(&self, id: &str) {
        self.0.lock().unwrap().remove(id);
    }

    pub fn snapshot(&self) -> HashMap<String, PrinterStatus> {
        self.0.lock().unwrap().clone()
    }
}

pub struct ConnectionManager<R: tauri::Runtime> {
    app: AppHandle<R>,
    tasks: Mutex<HashMap<String, JoinHandle<()>>>,
    statuses: Arc<StatusMap>,
}

impl<R: tauri::Runtime> ConnectionManager<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app, tasks: Mutex::new(HashMap::new()), statuses: Arc::new(StatusMap::default()) }
    }

    pub fn statuses(&self) -> HashMap<String, PrinterStatus> {
        self.statuses.snapshot()
    }

    /// Idempotent: starting a printer that is already running replaces its
    /// task, which is what a config edit needs.
    pub fn start(&self, printer_id: String, config: ConnectionConfig, api_key: Option<String>) {
        self.stop(&printer_id);

        let app = self.app.clone();
        let statuses = Arc::clone(&self.statuses);
        let id = printer_id.clone();

        let handle = tauri::async_runtime::spawn(async move {
            let mut attempt: u32 = 0;
            loop {
                publish(&app, &statuses, &id, PrinterStatus::new(ConnectionState::Connecting));

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
                    Some(connection) => connection.subscribe(tx).await,
                    None => {
                        // An unknown `kind` is a phase-3 config read by a
                        // phase-2 build. Report it and stop — retrying a
                        // protocol we cannot speak never succeeds.
                        publish(
                            &app,
                            &statuses,
                            &id,
                            PrinterStatus::errored(format!(
                                "This build cannot speak `{}` connections",
                                config.kind
                            )),
                        );
                        forward.abort();
                        return;
                    }
                };
                forward.abort();

                match outcome {
                    Ok(()) => attempt = 0,
                    Err(e) => {
                        publish(&app, &statuses, &id, PrinterStatus::errored(e.to_string()));
                        attempt = attempt.saturating_add(1);
                    }
                }
                tokio::time::sleep(backoff_delay(attempt)).await;
            }
        });

        self.tasks.lock().unwrap().insert(printer_id, handle);
    }

    /// Publishes a terminal error for a printer WITHOUT starting a connection
    /// task. For failures that no amount of reconnecting can fix — today, a
    /// credential store we cannot read at launch. Starting the normal loop
    /// there would report an *auth* failure forever and point the user at
    /// their API key rather than at the real cause.
    pub fn report_error(&self, printer_id: &str, message: impl Into<String>) {
        self.stop(printer_id);
        publish(&self.app, &self.statuses, printer_id, PrinterStatus::errored(message));
    }

    pub fn stop(&self, printer_id: &str) {
        if let Some(handle) = self.tasks.lock().unwrap().remove(printer_id) {
            handle.abort();
        }
        self.statuses.forget(printer_id);
    }

    pub async fn stop_and_wait(&self, printer_id: &str) {
        let handle = self.tasks.lock().unwrap().remove(printer_id);
        if let Some(handle) = handle {
            handle.abort();
            let _ = handle.await;
        }
        self.statuses.forget(printer_id);
    }
}

fn build(config: &ConnectionConfig, api_key: Option<String>) -> Option<Box<dyn PrinterConnection>> {
    match config.kind.as_str() {
        MOONRAKER_KIND => Some(Box::new(MoonrakerConnection::new(config.clone(), api_key))),
        _ => None,
    }
}

fn publish<R: tauri::Runtime>(
    app: &AppHandle<R>,
    statuses: &StatusMap,
    id: &str,
    status: PrinterStatus,
) {
    statuses.set(id, status.clone());
    // A failed emit means the window is gone; the status map is still
    // correct, so there is nothing to recover from here.
    let _ = app.emit(STATUS_EVENT, StatusEvent { id: id.to_string(), status });
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
        manager.tasks.lock().unwrap().insert("prn-1".to_string(), handle);
        started_rx.recv().unwrap();

        tauri::async_runtime::block_on(manager.stop_and_wait("prn-1"));

        assert!(dropped.load(Ordering::SeqCst));
    }
}
