//! The restart `apply_restore` (and a tier (c) reset) requests once the
//! journal is `pending` (spec D8 `apply_restore` step 11). It is injected
//! so tests never restart the process: production's [`AppRestarter`] calls
//! `AppHandle::restart()`; tests record the request.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::Duration;

/// How long after the response the restart is requested, so the response
/// reaches the frontend first.
pub const RESTART_DELAY: Duration = Duration::from_millis(500);

/// Restarts farm3d.
pub trait Restarter: Send + Sync {
    fn restart(&self);
}

/// Requests `restarter.restart()` [`RESTART_DELAY`] from now, on its own
/// thread.
pub fn request_restart(restarter: Arc<dyn Restarter>) {
    std::thread::spawn(move || {
        std::thread::sleep(RESTART_DELAY);
        restarter.restart();
    });
}

/// Production: `AppHandle::restart()`.
pub struct AppRestarter<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> AppRestarter<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: tauri::Runtime> Restarter for AppRestarter<R> {
    fn restart(&self) {
        // The log line must be on disk before the process is replaced.
        crate::diagnostics::log::flush();
        self.app.restart();
    }
}

/// Records restart requests and never restarts (tests, and the default
/// for services built without an app).
#[derive(Default)]
pub struct RecordingRestarter {
    requests: AtomicUsize,
    lock: Mutex<()>,
    requested: Condvar,
}

impl RecordingRestarter {
    /// How many restarts were requested.
    pub fn requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }

    /// Waits up to `timeout` for at least `count` requests.
    pub fn wait_for(&self, count: usize, timeout: Duration) -> bool {
        let guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let (_guard, _) = self
            .requested
            .wait_timeout_while(guard, timeout, |_| self.requests() < count)
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.requests() >= count
    }
}

impl Restarter for RecordingRestarter {
    fn restart(&self) {
        let _guard = self
            .lock
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        self.requests.fetch_add(1, Ordering::SeqCst);
        self.requested.notify_all();
    }
}
