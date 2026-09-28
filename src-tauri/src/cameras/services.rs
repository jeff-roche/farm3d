//! P8 D4 "Health and preview": `CameraServices` keeps one in-memory
//! `CameraHealth` per Printer, the last preview frame per Printer, and runs
//! at most one fetch per Printer at a time.
//!
//! - Every fetch of a Printer's **saved** source (a preview, a
//!   `test_camera` of the saved source, or — Task 8 — a capture) updates
//!   its health. A `test_camera` of an unsaved (draft) source never does.
//! - `camera.health.changed` is published only when `state`, `sourceKind`,
//!   or `lastFailureKind` changes; timestamps alone never publish.
//! - A preview request while a fetch for that Printer is running waits for
//!   it and shares its result. A preview within
//!   `CameraTimings::preview_min_interval` of the last successful preview
//!   frame gets that frame again from memory.
//! - Setting or clearing a source ([`CameraServices::source_changed`])
//!   drops the Printer's last frame and resets its health, and a fetch
//!   still running for the old source can't write its result back.
//! - Deleting a Printer drops everything held for it
//!   ([`CameraServices::forget_printer`]).
//!
//! Nothing here is persisted: health starts `unknown` after a restart.

use std::collections::HashMap;
use std::marker::PhantomData;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use chrono::{SecondsFormat, Utc};
use tauri::AppHandle;
use zeroize::Zeroizing;

use crate::attention::events::AttentionStream;
use crate::persistence::StorageError;
use crate::printers::repository::PrinterRepository;
use crate::printers::StoredPrinter;
use crate::RuntimeServices;

use super::fetch::{CameraError, Frame, FrameFetcher};
use super::resolve::{self, WebcamHost};
use super::{
    config, CameraErrorKind, CameraHealth, CameraHealthState, CameraSource, CameraSourceKind,
};

/// D4's camera timings. `default()` holds the production values; tests
/// inject shorter ones.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct CameraTimings {
    /// One frame fetch's whole budget (connect plus body), and the bound on
    /// a host-webcam lookup.
    pub fetch: Duration,
    /// A preview within this of the last successful preview frame gets
    /// that frame again from memory.
    pub preview_min_interval: Duration,
}

impl Default for CameraTimings {
    fn default() -> Self {
        Self {
            fetch: Duration::from_secs(5),
            preview_min_interval: Duration::from_secs(1),
        }
    }
}

/// Why a Printer-scoped fetch produced no frame.
#[derive(Debug)]
pub enum CameraFetchError {
    /// No such Printer.
    NotFound,
    /// The Printer has no camera source (`CAMERA_NOT_CONFIGURED`).
    NotConfigured,
    /// A `hostWebcam` source on a Printer with no Connection.
    NoConnection,
    /// The fetch itself failed.
    Camera(CameraError),
    Storage(StorageError),
}

impl From<StorageError> for CameraFetchError {
    fn from(error: StorageError) -> Self {
        CameraFetchError::Storage(error)
    }
}

/// A fetch's outcome, as the Printers waiting on it share it.
type Shared = Result<Arc<Frame>, CameraError>;

struct LastFetch {
    /// The entry's `completed` count this result was recorded at.
    completed: u64,
    epoch: u64,
    result: Shared,
}

/// Everything held in memory for one Printer.
struct Entry {
    /// `None` until the first saved-source fetch or source change.
    health: Option<CameraHealth>,
    /// Changes whenever the source changes (and is unique per entry), so a
    /// fetch that started before can't write its result back.
    epoch: u64,
    /// Saved-source fetches finished so far.
    completed: u64,
    last: Option<LastFetch>,
    /// The last successful preview frame, when it was fetched, and for
    /// which epoch.
    preview: Option<(Instant, u64, Arc<Frame>)>,
    /// Callers waiting on or running a fetch for this Printer.
    in_flight: usize,
}

pub struct CameraServices<R: tauri::Runtime> {
    fetcher: FrameFetcher,
    timings: CameraTimings,
    entries: Mutex<HashMap<String, Entry>>,
    /// One fetch per Printer at a time.
    locks: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    epochs: AtomicU64,
    _runtime: PhantomData<fn() -> R>,
}

impl<R: tauri::Runtime> Default for CameraServices<R> {
    fn default() -> Self {
        Self::new(CameraTimings::default())
    }
}

fn now_text() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

/// A Printer's health before any fetch: `unknown` with a source,
/// `notConfigured` without one.
fn fresh_health(printer_id: &str, source_kind: Option<CameraSourceKind>) -> CameraHealth {
    CameraHealth {
        printer_id: printer_id.to_string(),
        state: if source_kind.is_some() {
            CameraHealthState::Unknown
        } else {
            CameraHealthState::NotConfigured
        },
        source_kind,
        last_success_at: None,
        last_failure_at: None,
        last_failure_kind: None,
    }
}

/// The health after one saved-source fetch.
fn next_health(previous: &CameraHealth, kind: CameraSourceKind, result: &Shared) -> CameraHealth {
    let mut next = previous.clone();
    next.source_kind = Some(kind);
    match result {
        Ok(_) => {
            next.state = CameraHealthState::Ok;
            next.last_success_at = Some(now_text());
        }
        Err(error) => {
            next.state = if error.kind() == CameraErrorKind::UnsupportedAdapter {
                CameraHealthState::Unsupported
            } else {
                CameraHealthState::Failing
            };
            next.last_failure_at = Some(now_text());
            next.last_failure_kind = Some(error.kind());
        }
    }
    next
}

/// D4: only these three fields publish; timestamps alone never do.
fn publishes(previous: &CameraHealth, next: &CameraHealth) -> bool {
    previous.state != next.state
        || previous.source_kind != next.source_kind
        || previous.last_failure_kind != next.last_failure_kind
}

impl<R: tauri::Runtime> CameraServices<R> {
    pub fn new(timings: CameraTimings) -> Self {
        Self {
            fetcher: FrameFetcher::new(timings.fetch),
            timings,
            entries: Mutex::new(HashMap::new()),
            locks: Mutex::new(HashMap::new()),
            epochs: AtomicU64::new(0),
            _runtime: PhantomData,
        }
    }

    pub fn timings(&self) -> CameraTimings {
        self.timings
    }

    pub fn fetcher(&self) -> &FrameFetcher {
        &self.fetcher
    }

    fn entries(&self) -> MutexGuard<'_, HashMap<String, Entry>> {
        self.entries
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn next_epoch(&self) -> u64 {
        self.epochs.fetch_add(1, Ordering::SeqCst) + 1
    }

    fn entry<'a>(
        &self,
        entries: &'a mut HashMap<String, Entry>,
        printer_id: &str,
    ) -> &'a mut Entry {
        entries
            .entry(printer_id.to_string())
            .or_insert_with(|| Entry {
                health: None,
                epoch: self.next_epoch(),
                completed: 0,
                last: None,
                preview: None,
                in_flight: 0,
            })
    }

    /// The Printer's in-memory health, if anything has set it since start.
    pub fn health(&self, printer_id: &str) -> Option<CameraHealth> {
        self.entries()
            .get(printer_id)
            .and_then(|entry| entry.health.clone())
    }

    /// `list_attention`'s `cameraHealth`: one per Printer with a source
    /// (`sources`, read with the Events), from memory when it is for that
    /// source kind, else `unknown`.
    pub fn health_for(&self, sources: &[(String, CameraSourceKind)]) -> Vec<CameraHealth> {
        let entries = self.entries();
        sources
            .iter()
            .map(|(printer_id, kind)| {
                entries
                    .get(printer_id)
                    .and_then(|entry| entry.health.clone())
                    .filter(|health| health.source_kind == Some(*kind))
                    .unwrap_or_else(|| fresh_health(printer_id, Some(*kind)))
            })
            .collect()
    }

    /// Whether a last preview frame is held for the Printer.
    pub fn has_preview_frame(&self, printer_id: &str) -> bool {
        self.entries()
            .get(printer_id)
            .is_some_and(|entry| entry.preview.is_some())
    }

    /// Callers waiting on or running a fetch for the Printer (a test seam
    /// for the coalescing rule).
    pub fn in_flight(&self, printer_id: &str) -> usize {
        self.entries()
            .get(printer_id)
            .map_or(0, |entry| entry.in_flight)
    }

    /// The Printer's source was set (`Some(kind)`) or cleared (`None`)
    /// and committed: drop its last frame and shared result, reset its
    /// health to `unknown` / `notConfigured`, and publish that. Task 10's
    /// import calls this too when it replaces a Printer's source.
    pub fn source_changed(
        &self,
        app: &AppHandle<R>,
        stream: &AttentionStream,
        printer_id: &str,
        source_kind: Option<CameraSourceKind>,
    ) {
        let mut entries = self.entries();
        let epoch = self.next_epoch();
        let entry = self.entry(&mut entries, printer_id);
        entry.epoch = epoch;
        entry.last = None;
        entry.preview = None;
        let health = fresh_health(printer_id, source_kind);
        entry.health = Some(health.clone());
        stream.publish_camera_health(app, &health);
    }

    /// The Printer was deleted: drop its health, last frame, and fetch
    /// lock. A fetch still running for it records nothing.
    pub fn forget_printer(&self, printer_id: &str) {
        self.entries().remove(printer_id);
        self.locks
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(printer_id);
    }

    fn lock_for(&self, printer_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(
            self.locks
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .entry(printer_id.to_string())
                .or_default(),
        )
    }

    /// Joins the Printer's queue: counts the caller in flight and returns
    /// how many saved-source fetches had finished when it arrived.
    fn enter(&self, printer_id: &str) -> InFlight<'_, R> {
        let mut entries = self.entries();
        let entry = self.entry(&mut entries, printer_id);
        entry.in_flight += 1;
        InFlight {
            services: self,
            printer_id: printer_id.to_string(),
            seen: entry.completed,
        }
    }

    fn epoch(&self, printer_id: &str) -> u64 {
        let mut entries = self.entries();
        self.entry(&mut entries, printer_id).epoch
    }

    /// A preview's answer from memory: the last preview frame within the
    /// interval, or the result of a fetch that finished while it waited.
    fn reusable(&self, printer_id: &str, seen: u64) -> Option<Shared> {
        let entries = self.entries();
        let entry = entries.get(printer_id)?;
        if let Some((at, epoch, frame)) = &entry.preview {
            if *epoch == entry.epoch && at.elapsed() < self.timings.preview_min_interval {
                return Some(Ok(Arc::clone(frame)));
            }
        }
        entry
            .last
            .as_ref()
            .filter(|last| last.epoch == entry.epoch && last.completed > seen)
            .map(|last| last.result.clone())
    }

    /// Records a saved-source fetch of `epoch`'s source: shares it with
    /// waiting previews, keeps a preview frame, updates the health, and
    /// publishes a change. Nothing is recorded when the source changed or
    /// the Printer was forgotten meanwhile.
    fn record(
        &self,
        app: &AppHandle<R>,
        stream: &AttentionStream,
        fetch: SavedFetch<'_>,
        result: Result<Frame, CameraError>,
    ) -> Shared {
        let SavedFetch {
            printer_id,
            epoch,
            kind,
            preview,
        } = fetch;
        let result: Shared = result.map(Arc::new);
        let mut entries = self.entries();
        let Some(entry) = entries.get_mut(printer_id) else {
            return result;
        };
        if entry.epoch != epoch {
            return result;
        }
        entry.completed += 1;
        entry.last = Some(LastFetch {
            completed: entry.completed,
            epoch,
            result: result.clone(),
        });
        if let (true, Ok(frame)) = (preview, &result) {
            entry.preview = Some((Instant::now(), epoch, Arc::clone(frame)));
        }
        let previous = entry
            .health
            .clone()
            .filter(|health| health.source_kind == Some(kind))
            .unwrap_or_else(|| fresh_health(printer_id, Some(kind)));
        let next = next_health(&previous, kind, &result);
        entry.health = Some(next.clone());
        if publishes(&previous, &next) {
            // Under the entries lock, so one Printer's changes publish in
            // the order they were recorded.
            stream.publish_camera_health(app, &next);
        }
        result
    }
}

/// Which saved-source fetch a result belongs to.
struct SavedFetch<'a> {
    printer_id: &'a str,
    /// The entry's epoch when the source was read.
    epoch: u64,
    kind: CameraSourceKind,
    /// A preview keeps a successful frame for reuse.
    preview: bool,
}

/// A caller's place in one Printer's queue; leaving it (by any path)
/// uncounts it.
struct InFlight<'a, R: tauri::Runtime> {
    services: &'a CameraServices<R>,
    printer_id: String,
    seen: u64,
}

impl<R: tauri::Runtime> Drop for InFlight<'_, R> {
    fn drop(&mut self) {
        let mut entries = self.services.entries();
        if let Some(entry) = entries.get_mut(&self.printer_id) {
            entry.in_flight = entry.in_flight.saturating_sub(1);
            // A Printer that never had a source or a fetch keeps nothing.
            if entry.in_flight == 0 && entry.health.is_none() && entry.last.is_none() {
                entries.remove(&self.printer_id);
            }
        }
    }
}

// --- the Printer-scoped fetches ----------------------------------------------

fn load_printer<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer_id: &str,
) -> Result<StoredPrinter, CameraFetchError> {
    PrinterRepository::new(Arc::clone(&services.storage))
        .get(printer_id)?
        .ok_or(CameraFetchError::NotFound)
}

fn load_source<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer_id: &str,
) -> Result<Option<CameraSource>, CameraFetchError> {
    Ok(services
        .storage
        .read(|connection| config::get(connection, printer_id))?
        .map(|camera| camera.source))
}

/// The Printer's Connection and credential, for a `hostWebcam` lookup
/// only (a `snapshotUrl` fetch never touches the credential store). A
/// credential that can't be read is sent as none: the lookup then fails
/// `webcamListFailed`, never a credential error.
pub(crate) fn printer_webcam_host<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer: &StoredPrinter,
) -> Option<WebcamHost> {
    let config = printer.connection.clone()?;
    let api_key = config
        .credential_ref
        .as_deref()
        .and_then(|reference| services.credentials.get(reference).ok().flatten())
        .map(Zeroizing::new);
    Some(WebcamHost { config, api_key })
}

/// Resolves and fetches `source` once, bounded by the fetch budget (the
/// host-webcam lookup and the frame each get it).
pub async fn fetch_source<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    source: &CameraSource,
    host: Option<&WebcamHost>,
) -> Result<Frame, CameraError> {
    let cameras = &services.cameras;
    let url = resolve::resolve(source, host, cameras.timings.fetch).await?;
    cameras.fetcher.fetch(&url).await
}

/// Loads the Printer, its saved source, and (for a host webcam) its
/// Connection.
fn saved_target<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer_id: &str,
) -> Result<(StoredPrinter, Option<CameraSource>), CameraFetchError> {
    let printer = load_printer(services, printer_id)?;
    let source = load_source(services, printer_id)?;
    Ok((printer, source))
}

fn webcam_host_for<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    printer: &StoredPrinter,
    source: &CameraSource,
) -> Result<Option<WebcamHost>, CameraFetchError> {
    match source {
        CameraSource::HostWebcam { .. } => printer_webcam_host(services, printer)
            .map(Some)
            .ok_or(CameraFetchError::NoConnection),
        CameraSource::SnapshotUrl { .. } => Ok(None),
    }
}

/// `camera_preview_frame`: the Printer's saved source, coalesced (D4). A
/// preview never stores anything.
pub async fn preview_frame<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &AppHandle<R>,
    printer_id: &str,
) -> Result<Arc<Frame>, CameraFetchError> {
    fetch_saved(services, app, printer_id, true).await
}

/// A fresh fetch of the Printer's saved source (a `test_camera` of it, or
/// a capture), serialized with every other fetch for the Printer, updating
/// its health. The seam Task 8's capture path uses.
pub async fn fetch_saved<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &AppHandle<R>,
    printer_id: &str,
    preview: bool,
) -> Result<Arc<Frame>, CameraFetchError> {
    let cameras = &services.cameras;
    let queued = cameras.enter(printer_id);
    let lock = cameras.lock_for(printer_id);
    let _one_at_a_time = lock.lock().await;
    if preview {
        if let Some(shared) = cameras.reusable(printer_id, queued.seen) {
            return shared.map_err(CameraFetchError::Camera);
        }
    }
    // The epoch before the source is read: a source change committed after
    // this read bumps it, and this fetch's result is then dropped.
    let epoch = cameras.epoch(printer_id);
    let (printer, source) = saved_target(services, printer_id)?;
    let source = source.ok_or(CameraFetchError::NotConfigured)?;
    let host = webcam_host_for(services, &printer, &source)?;
    let result = fetch_source(services, &source, host.as_ref()).await;
    let fetch = SavedFetch {
        printer_id,
        epoch,
        kind: source.kind(),
        preview,
    };
    cameras
        .record(app, &services.attention.stream, fetch, result)
        .map_err(CameraFetchError::Camera)
}

/// `test_camera` with a `printerId`: a test of the Printer's saved source
/// is a saved-source fetch (it updates the health); any other source is a
/// draft, fetched through the Printer's Connection with no effect on its
/// health (D4; carry: a draft never touches the saved source's health).
/// Serialized with every other fetch for the Printer either way.
pub async fn test_printer_source<R: tauri::Runtime>(
    services: &RuntimeServices<R>,
    app: &AppHandle<R>,
    printer_id: &str,
    source: &CameraSource,
) -> Result<Arc<Frame>, CameraFetchError> {
    let cameras = &services.cameras;
    let _queued = cameras.enter(printer_id);
    let lock = cameras.lock_for(printer_id);
    let _one_at_a_time = lock.lock().await;
    let epoch = cameras.epoch(printer_id);
    let (printer, saved) = saved_target(services, printer_id)?;
    let host = webcam_host_for(services, &printer, source)?;
    let result = fetch_source(services, source, host.as_ref()).await;
    if saved.as_ref() == Some(source) {
        let fetch = SavedFetch {
            printer_id,
            epoch,
            kind: source.kind(),
            preview: false,
        };
        cameras
            .record(app, &services.attention.stream, fetch, result)
            .map_err(CameraFetchError::Camera)
    } else {
        result.map(Arc::new).map_err(CameraFetchError::Camera)
    }
}
