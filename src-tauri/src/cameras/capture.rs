//! P8 D4 "Capture triggers": what the Attention projector asks the camera
//! side to capture after a pass commits. The projector only yields these
//! (`attention::projector::AppliedChanges::capture`); handling them —
//! fetching the frame, storing it, and recording the outcome — is
//! `CameraServices`' job, on its own task, never awaited by the projector
//! (global constraint 5).

/// One best-effort capture a committed projector pass asked for. Yielded
/// only for `origin: live` rows, and only when the Printer's own toggle
/// (`snapshotOnIncident` / `snapshotOnCompletion`) is on.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureIntent {
    /// The projector opened an Incident: one `incident` frame, linked to
    /// the Incident (and its Job, if any).
    Incident {
        incident_id: String,
        printer_id: String,
        job_id: Option<String>,
    },
    /// The projector inserted `job.completed`: one `completion` frame
    /// linked to the Job, whose outcome goes to the Event's `evidence`.
    Completion {
        event_id: String,
        printer_id: String,
        job_id: String,
    },
}

impl CaptureIntent {
    pub fn printer_id(&self) -> &str {
        match self {
            CaptureIntent::Incident { printer_id, .. }
            | CaptureIntent::Completion { printer_id, .. } => printer_id,
        }
    }

    /// One key per expected outcome: at most one capture per Incident and
    /// per `job.completed` Event is ever in flight.
    fn key(&self) -> String {
        match self {
            CaptureIntent::Incident { incident_id, .. } => format!("incident:{incident_id}"),
            CaptureIntent::Completion { event_id, .. } => format!("completion:{event_id}"),
        }
    }

    fn link(&self) -> CaptureLink<'_> {
        match self {
            CaptureIntent::Incident {
                incident_id,
                job_id,
                ..
            } => CaptureLink::Incident {
                incident_id,
                job_id: job_id.as_deref(),
            },
            CaptureIntent::Completion {
                event_id, job_id, ..
            } => CaptureLink::Completion { event_id, job_id },
        }
    }
}

// --- the capture runtime ---------------------------------------------------------------

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use chrono::{DateTime, Utc};
use tauri::AppHandle;
use tokio::sync::broadcast::error::RecvError;
use tokio::sync::watch;

use crate::attention::projector::AppliedChanges;
use crate::persistence::{RepositoryError, StorageError};
use crate::RuntimeServices;

use super::media::{self, CaptureLink, MediaChanges, NewSnapshot, StoreOutcome};
use super::services::{self as camera_services, CameraFetchError};
use super::{config, CameraErrorKind, EvidenceSkipReason};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The capture runtime's state, held in `CameraServices`.
pub struct CaptureRuntime {
    started: OnceLock<()>,
    stop: watch::Sender<bool>,
    /// Keys ([`CaptureIntent::key`]) of captures running now.
    capturing: Mutex<HashSet<String>>,
    swept: Mutex<Option<MediaChanges>>,
    lags: AtomicU64,
    /// Committed changes the consumer has taken (or skipped by lagging).
    rounds: AtomicU64,
    held: watch::Sender<bool>,
    /// False while the media store is unavailable (a failed sweep).
    media_available: AtomicBool,
}

impl Default for CaptureRuntime {
    fn default() -> Self {
        Self {
            started: OnceLock::new(),
            stop: watch::channel(false).0,
            capturing: Mutex::new(HashSet::new()),
            swept: Mutex::new(None),
            lags: AtomicU64::new(0),
            rounds: AtomicU64::new(0),
            held: watch::channel(false).0,
            media_available: AtomicBool::new(true),
        }
    }
}

impl CaptureRuntime {
    pub(crate) fn set_swept(&self, changes: MediaChanges) {
        let mut swept = lock(&self.swept);
        match swept.as_mut() {
            Some(existing) => existing.extend(changes),
            None => *swept = Some(changes),
        }
    }

    pub(crate) fn stop(&self) {
        self.stop.send_replace(true);
    }

    pub(crate) fn lags(&self) -> u64 {
        self.lags.load(Ordering::SeqCst)
    }

    pub(crate) fn in_flight(&self) -> usize {
        lock(&self.capturing).len()
    }

    pub(crate) fn rounds(&self) -> u64 {
        self.rounds.load(Ordering::SeqCst)
    }

    pub(crate) fn hold(&self, held: bool) {
        self.held.send_replace(held);
    }

    pub(crate) fn media_available(&self) -> bool {
        self.media_available.load(Ordering::SeqCst)
    }

    pub(crate) fn set_media_available(&self, available: bool) {
        self.media_available.store(available, Ordering::SeqCst);
    }
}

/// Publishes a committed media change on the `attention` stream (Events,
/// then Incidents, then snapshots). Nothing for an empty change.
pub(crate) fn publish<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &AppHandle<R>,
    changes: &MediaChanges,
) {
    if !changes.is_empty() {
        services.attention.stream.publish_change(
            app,
            &changes.events,
            &changes.incidents,
            &changes.snapshots,
        );
    }
}

/// Starts the capture runtime for `services` (`start_camera_runtime`): it
/// publishes the startup sweep's changes, subscribes to the projector's
/// committed passes (so it must start **before** the Attention runtime,
/// whose first pass could otherwise open an Incident no one hears of), and
/// spawns the capture consumer and the `MediaJanitor`, whose first prune
/// pass runs at once. A second call does nothing.
///
/// Every capture runs on its own task, off the projector's transaction:
/// nothing in monitoring, slicing, assignment, or the Job path waits for a
/// camera (global constraint 5).
pub fn start<R: tauri::Runtime>(services: &Arc<RuntimeServices<R>>, app: &AppHandle<R>) {
    let runtime = &services.cameras.runtime;
    if runtime.started.set(()).is_err() {
        return;
    }
    if let Some(changes) = lock(&runtime.swept).take() {
        publish(services, app, &changes);
    }
    let applied = services.attention.subscribe_applied();
    let since = services.attention.now();
    tauri::async_runtime::spawn(consume(
        Arc::downgrade(services),
        app.clone(),
        applied,
        since,
        runtime.stop.subscribe(),
        runtime.held.subscribe(),
    ));
    tauri::async_runtime::spawn(janitor(
        Arc::downgrade(services),
        app.clone(),
        runtime.stop.subscribe(),
    ));
}

fn stopping(stop: &watch::Receiver<bool>) -> bool {
    *stop.borrow()
}

async fn consume<R: tauri::Runtime>(
    services: Weak<RuntimeServices<R>>,
    app: AppHandle<R>,
    mut applied: tokio::sync::broadcast::Receiver<Arc<AppliedChanges>>,
    since: DateTime<Utc>,
    mut stop: watch::Receiver<bool>,
    mut held: watch::Receiver<bool>,
) {
    loop {
        // A test may hold the consumer (its receiver keeps filling, and may
        // lag) until it releases it.
        while *held.borrow() {
            tokio::select! {
                changed = held.changed() => if changed.is_err() { return; },
                changed = stop.changed() => if changed.is_err() || stopping(&stop) { return; },
            }
        }
        let received = tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || stopping(&stop) {
                    return;
                }
                continue;
            }
            received = applied.recv() => received,
        };
        let Some(strong) = services.upgrade() else {
            return;
        };
        match received {
            Ok(changes) => {
                for intent in &changes.capture {
                    dispatch(&strong, &app, intent.clone());
                }
                strong.cameras.runtime.rounds.fetch_add(1, Ordering::SeqCst);
            }
            // Whatever was missed is re-derived from storage: every live
            // Incident and `job.completed` since the runtime started whose
            // outcome isn't recorded yet (in-flight ones are skipped by key).
            Err(RecvError::Lagged(skipped)) => {
                strong.cameras.runtime.lags.fetch_add(1, Ordering::SeqCst);
                match missed_intents(&strong, since) {
                    Ok(intents) => {
                        for intent in intents {
                            dispatch(&strong, &app, intent);
                        }
                    }
                    Err(error) => {
                        crate::f3d_log!(
                            warn,
                            "cameras.rederiveFailed",
                            error = error,
                            skipped = skipped
                        );
                    }
                }
                strong
                    .cameras
                    .runtime
                    .rounds
                    .fetch_add(skipped, Ordering::SeqCst);
            }
            Err(RecvError::Closed) => return,
        }
    }
}

/// Runs `intent` on its own task, unless a capture for the same Incident
/// or Event is already running.
fn dispatch<R: tauri::Runtime>(
    services: &Arc<RuntimeServices<R>>,
    app: &AppHandle<R>,
    intent: CaptureIntent,
) {
    let key = intent.key();
    if !lock(&services.cameras.runtime.capturing).insert(key.clone()) {
        return;
    }
    let services = Arc::clone(services);
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        if let Err(error) = capture(&services, &app, &intent).await {
            crate::f3d_log!(warn, "cameras.captureFailed", error = error);
        }
        lock(&services.cameras.runtime.capturing).remove(&key);
    });
}

/// D4 "Capture triggers": one best-effort frame for `intent`, stored and
/// recorded (`evidenceCaptured`, or `evidenceSkipped { cameraError |
/// diskCap }`; for a completion, the Event's `evidence`). Nothing is
/// recorded when the Printer has no camera source (or is gone): no capture
/// was expected.
pub async fn capture<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &AppHandle<R>,
    intent: &CaptureIntent,
) -> Result<(), RepositoryError> {
    let printer_id = intent.printer_id();
    let configured = services
        .storage
        .read(|conn| config::get(conn, printer_id))?
        .is_some();
    if !configured {
        return Ok(());
    }
    let link = intent.link();
    if !services.cameras.media_available() {
        // The media store is down: tried, and couldn't (no fetch).
        let changes = media::record_skip(
            &services.storage,
            &link,
            EvidenceSkipReason::Storage,
            None,
            services.attention.now(),
        )?;
        publish(services, app, &changes);
        return Ok(());
    }
    let changes = match camera_services::fetch_saved(services, app, printer_id, false).await {
        Ok(frame) => {
            let policy = services.storage.read(media::read_policy)?;
            let stored = media::store_frame(
                &services.storage,
                services.cameras.janitor(),
                policy,
                services.attention.now(),
                NewSnapshot {
                    printer_id,
                    link,
                    frame: &frame,
                },
            )
            .await;
            match stored {
                Ok(
                    StoreOutcome::Stored { changes, .. } | StoreOutcome::DiskCap { changes, .. },
                ) => changes,
                Ok(StoreOutcome::AlreadyRecorded(_)) => MediaChanges::default(),
                // The image couldn't be written (or its row committed):
                // record that the capture was tried, then report the error.
                Err(error @ RepositoryError::Storage(_)) => {
                    if let Ok(changes) = media::record_skip(
                        &services.storage,
                        &link,
                        EvidenceSkipReason::Storage,
                        None,
                        services.attention.now(),
                    ) {
                        publish(services, app, &changes);
                    }
                    return Err(error);
                }
                Err(error) => return Err(error),
            }
        }
        Err(CameraFetchError::Camera(error)) => media::record_camera_error(
            &services.storage,
            &link,
            error.kind(),
            services.attention.now(),
        )?,
        Err(CameraFetchError::NoConnection) => media::record_camera_error(
            &services.storage,
            &link,
            CameraErrorKind::WebcamListFailed,
            services.attention.now(),
        )?,
        // The source was cleared, or the Printer deleted, meanwhile.
        Err(CameraFetchError::NotConfigured | CameraFetchError::NotFound) => {
            MediaChanges::default()
        }
        Err(CameraFetchError::Storage(error)) => return Err(RepositoryError::Storage(error)),
    };
    publish(services, app, &changes);
    Ok(())
}

fn parse_time(text: &str) -> Option<DateTime<Utc>> {
    DateTime::parse_from_rfc3339(text)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// After a lag: every capture the projector asked for since `since` (live
/// only, with the Printer's toggle on now) whose outcome isn't recorded.
pub fn missed_intents<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    since: DateTime<Utc>,
) -> Result<Vec<CaptureIntent>, StorageError> {
    services.storage.read(|conn| {
        let mut intents = Vec::new();
        let mut incidents = conn.prepare(
            "SELECT i.id, i.printer_id, i.job_id, i.opened_at FROM incidents i
               JOIN incident_events o ON o.incident_id = i.id AND o.kind = 'opened'
               JOIN attention_events e ON e.id = o.attention_event_id AND e.origin = 'live'
              WHERE NOT EXISTS (SELECT 1 FROM incident_events x WHERE x.incident_id = i.id
                                  AND x.kind IN ('evidenceCaptured', 'evidenceSkipped'))
              ORDER BY i.opened_at, i.id",
        )?;
        let rows = incidents
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, Option<String>>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (incident_id, printer_id, job_id, opened_at) in rows {
            if parse_time(&opened_at).is_none_or(|at| at < since) {
                continue;
            }
            let alerts = crate::printers::alerts::get(conn, &printer_id)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            if alerts.alert_defaults.snapshot_on_incident {
                intents.push(CaptureIntent::Incident {
                    incident_id,
                    printer_id,
                    job_id,
                });
            }
        }
        let mut completions = conn.prepare(
            "SELECT id, printer_id, job_id, first_observed_at FROM attention_events
              WHERE condition = 'job.completed' AND origin = 'live' AND evidence_json IS NULL
                AND printer_id IS NOT NULL AND job_id IS NOT NULL
              ORDER BY first_observed_at, id",
        )?;
        let rows = completions
            .query_map([], |row| {
                Ok((
                    row.get::<_, String>(0)?,
                    row.get::<_, String>(1)?,
                    row.get::<_, String>(2)?,
                    row.get::<_, String>(3)?,
                ))
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        for (event_id, printer_id, job_id, observed_at) in rows {
            if parse_time(&observed_at).is_none_or(|at| at < since) {
                continue;
            }
            let alerts = crate::printers::alerts::get(conn, &printer_id)
                .map_err(|_| rusqlite::Error::InvalidQuery)?;
            if alerts.alert_defaults.snapshot_on_completion {
                intents.push(CaptureIntent::Completion {
                    event_id,
                    printer_id,
                    job_id,
                });
            }
        }
        Ok(intents)
    })
}

/// D5 "`MediaJanitor`": a prune pass at start (after the startup sweep),
/// then every `CameraTimings::janitor_every` and on every poke.
async fn janitor<R: tauri::Runtime>(
    services: Weak<RuntimeServices<R>>,
    app: AppHandle<R>,
    mut stop: watch::Receiver<bool>,
) {
    let Some(strong) = services.upgrade() else {
        return;
    };
    let wake = strong.cameras.janitor().wake();
    let every = strong.cameras.timings().janitor_every;
    drop(strong);
    loop {
        {
            let Some(services) = services.upgrade() else {
                return;
            };
            if !services.cameras.media_available() {
                retry_sweep(&services, &app).await;
            }
            if services.cameras.media_available() {
                if let Err(error) = prune_now(&services, &app).await {
                    crate::f3d_log!(warn, "cameras.prunePassFailed", error = error);
                }
            }
            services.cameras.janitor().pass_done();
        }
        tokio::select! {
            changed = stop.changed() => {
                if changed.is_err() || stopping(&stop) {
                    return;
                }
            }
            _ = wake.notified() => {}
            _ = tokio::time::sleep(every) => {}
        }
        if stopping(&stop) {
            return;
        }
    }
}

/// While the media store is unavailable, each janitor pass retries the
/// startup sweep (under the janitor lock); a success makes it available
/// again and publishes what the sweep changed.
async fn retry_sweep<R: tauri::Runtime>(services: &RuntimeServices<R>, app: &AppHandle<R>) {
    let swept = media::sweep_under_lock(
        &services.storage,
        services.cameras.janitor(),
        services.attention.now(),
    )
    .await;
    if let Ok(changes) = swept {
        services.cameras.runtime.set_media_available(true);
        publish(services, app, &changes);
    }
}

/// One janitor prune pass under the stored retention settings, published.
pub async fn prune_now<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &AppHandle<R>,
) -> Result<(), RepositoryError> {
    let policy = services.storage.read(media::read_policy)?;
    let changes = media::prune_pass(
        &services.storage,
        services.cameras.janitor(),
        policy,
        services.attention.now(),
    )
    .await?;
    publish(services, app, &changes);
    Ok(())
}
