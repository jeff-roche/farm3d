//! P8 D6 "Sinks": the test doubles. [`RecordingSink`] records every
//! notification and lets a test deliver the signals a server would send
//! (a token, a click, a close); [`RecordingWindowControl`] records the
//! raise steps a click asks for. Nothing here touches a desktop.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Mutex;

use async_trait::async_trait;

use super::activation::{RaiseStep, WindowControl};
use super::{
    Notification, NotificationHandle, NotificationSink, NotifierStatus, NotifyError, SignalHandler,
    SinkSignal,
};

fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A sink that records what it is asked to show. Ids start at 1; a
/// notification with a `replaces_id` keeps that id, as a freedesktop
/// server does.
pub struct RecordingSink {
    shown: Mutex<Vec<(u32, Notification)>>,
    next_id: AtomicU32,
    status: Mutex<NotifierStatus>,
    handler: Mutex<Option<SignalHandler>>,
}

impl RecordingSink {
    /// An `available` notifier.
    pub fn available() -> Self {
        Self::with_status(NotifierStatus::Available {
            server_name: "Recording".into(),
            server_vendor: "farm3d tests".into(),
            server_version: "1".into(),
            spec_version: "1.2".into(),
            actions: true,
            body_markup: true,
        })
    }

    /// A notifier in `status`; `show` fails unless it is `available`.
    pub fn with_status(status: NotifierStatus) -> Self {
        Self {
            shown: Mutex::new(Vec::new()),
            next_id: AtomicU32::new(1),
            status: Mutex::new(status),
            handler: Mutex::new(None),
        }
    }

    /// Every notification shown so far, with its id.
    pub fn shown(&self) -> Vec<(u32, Notification)> {
        lock(&self.shown).clone()
    }

    pub fn status_now(&self) -> NotifierStatus {
        lock(&self.status).clone()
    }

    pub fn set_status(&self, status: NotifierStatus) {
        *lock(&self.status) = status;
    }

    fn deliver(&self, signal: SinkSignal) {
        let handler = lock(&self.handler).clone();
        if let Some(handler) = handler {
            handler(signal);
        }
    }

    /// The server's `ActivationToken(id, token)`.
    pub fn token(&self, id: u32, token: &str) {
        self.deliver(SinkSignal::ActivationToken {
            id,
            token: token.to_string(),
        });
    }

    /// The server's `ActionInvoked(id, action)`: a click.
    pub fn click(&self, id: u32, action: &str) {
        self.deliver(SinkSignal::ActionInvoked {
            id,
            action: action.to_string(),
        });
    }

    /// The server's `NotificationClosed(id, reason)`.
    pub fn close(&self, id: u32, reason: u32) {
        self.deliver(SinkSignal::Closed { id, reason });
    }
}

#[async_trait]
impl NotificationSink for RecordingSink {
    async fn show(&self, notification: &Notification) -> Result<NotificationHandle, NotifyError> {
        match self.status_now() {
            NotifierStatus::Available { .. } => {}
            NotifierStatus::Unavailable { reason } => return Err(NotifyError::Unavailable(reason)),
            NotifierStatus::Unsupported => return Err(NotifyError::Unsupported),
        }
        let id = if notification.replaces_id != 0 {
            notification.replaces_id
        } else {
            self.next_id.fetch_add(1, Ordering::SeqCst)
        };
        lock(&self.shown).push((id, notification.clone()));
        Ok(NotificationHandle { id })
    }

    fn status(&self) -> NotifierStatus {
        self.status_now()
    }

    fn set_signal_handler(&self, handler: SignalHandler) {
        *lock(&self.handler) = Some(handler);
    }
}

/// Records the raise steps a click asks for, in order.
#[derive(Default)]
pub struct RecordingWindowControl {
    steps: Mutex<Vec<RaiseStep>>,
}

impl RecordingWindowControl {
    pub fn steps(&self) -> Vec<RaiseStep> {
        lock(&self.steps).clone()
    }
}

impl WindowControl for RecordingWindowControl {
    fn present(&self, token: Option<String>) {
        lock(&self.steps).push(RaiseStep::Present { token });
    }

    fn remap(&self) {
        lock(&self.steps).push(RaiseStep::Remap);
    }

    fn request_attention(&self) {
        lock(&self.steps).push(RaiseStep::RequestAttention);
    }
}
