//! P8 D6 (ADR-0015): desktop notifications.
//!
//! Rust decides whether to notify ([`policy::decide`], pure), sends through
//! a platform [`NotificationSink`] (farm3d's own `org.freedesktop.
//! Notifications` client on Linux, [`dbus::DbusNotificationSink`]; the
//! [`null::NullNotificationSink`] elsewhere), and on a click raises the
//! window, emits [`NAVIGATE_EVENT`], and marks the Event read
//! ([`services::NotificationService`], [`activation`]). Focus is seeded
//! once from the shown window's `is_focused()`, then follows
//! `WindowEvent::Focused` ([`focus::Focus`]).
//!
//! This file holds the wire types (the notifier's status, the navigation
//! payload, the class settings) and the Rust-internal sink contract: a
//! [`Notification`]'s content, its [`NotificationHandle`], a
//! [`NotifyError`], and the [`SinkSignal`]s a sink reports.

pub mod activation;
pub mod commands;
#[cfg(target_os = "linux")]
pub mod dbus;
pub mod focus;
pub mod null;
pub mod policy;
pub mod recording;
pub mod services;

use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::attention::{AttentionSeverity, NotificationClass};
use crate::contracts::navigation::{NavigationDestination, NavigationTarget};
use crate::contracts::ContractVersion;

/// The unsequenced event name `notifications::activation` emits a click's
/// [`NavigateRequest`] on (D7 "Events"). No backfill.
pub const NAVIGATE_EVENT: &str = "farm3d-navigate-v1";

/// D6 "Sinks": why the platform notifier can't be used.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/NotifierUnavailableReason.ts"
)]
pub enum NotifierUnavailableReason {
    NoSessionBus,
    NoNotificationServer,
    CallFailed,
}

impl NotifierUnavailableReason {
    pub const ALL: [NotifierUnavailableReason; 3] = [
        NotifierUnavailableReason::NoSessionBus,
        NotifierUnavailableReason::NoNotificationServer,
        NotifierUnavailableReason::CallFailed,
    ];
}

/// `notification_status`'s result (D6 "Sinks"). `available` only on
/// Linux, once the D-Bus sink has confirmed a reachable
/// `org.freedesktop.Notifications`; `unsupported` on every other target
/// (the null sink).
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(tag = "state", rename_all = "camelCase", rename_all_fields = "camelCase")]
#[ts(
    tag = "state",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "domain/NotifierStatus.ts"
)]
pub enum NotifierStatus {
    Available {
        server_name: String,
        server_vendor: String,
        server_version: String,
        spec_version: String,
        actions: bool,
        body_markup: bool,
    },
    Unavailable {
        reason: NotifierUnavailableReason,
    },
    Unsupported,
}

/// D8 "Deep-link targets": a notification click's activation payload.
/// Opening from a notification marks the Event read; when the target no
/// longer exists, it opens the Event itself.
#[derive(Serialize, Deserialize, Clone, PartialEq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/NavigateRequest.ts")]
pub struct NavigateRequest {
    #[ts(type = "1")]
    pub contract_version: ContractVersion,
    pub target: NavigationTarget,
    pub open_attention_center: bool,
}

impl NavigateRequest {
    pub fn new(target: NavigationTarget, open_attention_center: bool) -> Self {
        Self {
            contract_version: ContractVersion::V1,
            target,
            open_attention_center,
        }
    }
}

/// The six `settings.notify_*` toggles (D6 "Classes and defaults"): which
/// notification classes may notify at all. `SettingsRecord.notifications`
/// on the wire.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(
    rename_all = "camelCase",
    export_to = "domain/NotificationClassSettings.ts"
)]
pub struct NotificationClassSettings {
    pub fatal: bool,
    pub confirmation: bool,
    pub completion: bool,
    pub reconciliation: bool,
    pub connectivity: bool,
    pub inventory: bool,
}

impl Default for NotificationClassSettings {
    /// Decision 7: fatal failures, confirmations, and completions on;
    /// reconciliation, connectivity, and inventory off.
    fn default() -> Self {
        Self {
            fatal: true,
            confirmation: true,
            completion: true,
            reconciliation: false,
            connectivity: false,
            inventory: false,
        }
    }
}

impl NotificationClassSettings {
    pub fn enabled(&self, class: NotificationClass) -> bool {
        match class {
            NotificationClass::Fatal => self.fatal,
            NotificationClass::Confirmation => self.confirmation,
            NotificationClass::Completion => self.completion,
            NotificationClass::Reconciliation => self.reconciliation,
            NotificationClass::Connectivity => self.connectivity,
            NotificationClass::Inventory => self.inventory,
        }
    }
}

/// The freedesktop `urgency` hint: `fatal` 2, `warning` 1, `info` 0.
#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Urgency {
    Low,
    Normal,
    Critical,
}

impl Urgency {
    pub fn of(severity: AttentionSeverity) -> Self {
        match severity {
            AttentionSeverity::Fatal => Urgency::Critical,
            AttentionSeverity::Warning => Urgency::Normal,
            AttentionSeverity::Info => Urgency::Low,
        }
    }

    /// The hint's byte.
    pub fn byte(self) -> u8 {
        match self {
            Urgency::Low => 0,
            Urgency::Normal => 1,
            Urgency::Critical => 2,
        }
    }
}

/// What a [`Notification`] stands for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotificationKind {
    /// One Attention Event.
    Event,
    /// D6 "Burst": `count` Events folded into one notification that opens
    /// the Attention center.
    Summary { count: u32 },
    /// `send_test_notification`.
    Test,
}

/// One notification to show (Rust-internal; never serialized). `summary`
/// is the freedesktop title and `body` its text: plain text, never a
/// host, URL, camera text, or credential. A sink escapes the body itself
/// when its server takes markup.
#[derive(Clone, PartialEq, Debug)]
pub struct Notification {
    pub summary: String,
    pub body: String,
    pub urgency: Urgency,
    /// freedesktop `replaces_id`: 0 for a new notification, or the live
    /// summary's id to update it in place.
    pub replaces_id: u32,
    /// Where a click goes (decision 8). An Event's target is evaluated
    /// again at click time (`attention::deep_link::target_for`).
    pub target: NavigationTarget,
    pub open_attention_center: bool,
    /// The one Event a click marks read (`Event` only).
    pub event_id: Option<String>,
    /// Every Event whose `notified_at` a successful show writes.
    pub event_ids: Vec<String>,
    /// The dedup keys a successful show records in the rate limiter.
    pub dedup_keys: Vec<String>,
    pub kind: NotificationKind,
}

impl Notification {
    /// `send_test_notification`'s notification: whatever the focus and
    /// classes, targeting the Monitor.
    pub fn test() -> Self {
        Self {
            summary: "farm3d test notification".to_string(),
            body: "Info: Desktop notifications from farm3d work.".to_string(),
            urgency: Urgency::Low,
            replaces_id: 0,
            target: monitor_target(),
            open_attention_center: false,
            event_id: None,
            event_ids: Vec::new(),
            dedup_keys: Vec::new(),
            kind: NotificationKind::Test,
        }
    }
}

/// `{ destination: "monitor" }`: the test notification's target, and a
/// summary's (with the Attention center open).
pub fn monitor_target() -> NavigationTarget {
    NavigationTarget {
        version: ContractVersion::V1,
        destination: NavigationDestination::Monitor,
        selection: None,
    }
}

/// A shown notification's server id.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct NotificationHandle {
    pub id: u32,
}

/// Why a sink could not show a notification.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum NotifyError {
    /// This platform has no sink (the null sink).
    Unsupported,
    /// No session bus, no notification server, or a failed call.
    Unavailable(NotifierUnavailableReason),
}

impl NotifyError {
    /// The status a failed show leaves (`send_test_notification`'s
    /// `NOTIFICATIONS_UNAVAILABLE` details).
    pub fn status(self) -> NotifierStatus {
        match self {
            NotifyError::Unsupported => NotifierStatus::Unsupported,
            NotifyError::Unavailable(reason) => NotifierStatus::Unavailable { reason },
        }
    }
}

/// What a sink reports about the notifications it showed (D6 "Click
/// activation"). The service ignores ids it doesn't know.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SinkSignal {
    /// `ActivationToken(id, token)`: a Wayland activation token for the
    /// click that follows.
    ActivationToken { id: u32, token: String },
    /// `ActionInvoked(id, action)`: a click on the body (`default`) or a
    /// button.
    ActionInvoked { id: u32, action: String },
    /// `NotificationClosed(id, reason)`.
    Closed { id: u32, reason: u32 },
}

/// Where a sink delivers its [`SinkSignal`]s, in the order they arrived.
pub type SignalHandler = Arc<dyn Fn(SinkSignal) + Send + Sync>;

/// A platform notifier (D6 "Sinks"). Async, since zbus is.
#[async_trait]
pub trait NotificationSink: Send + Sync {
    /// Shows `notification`; its id on success.
    async fn show(&self, notification: &Notification) -> Result<NotificationHandle, NotifyError>;

    /// The last known status, without touching the bus.
    fn status(&self) -> NotifierStatus;

    /// Retries the connection when it isn't `available`, then reports the
    /// status (`notification_status`, `send_test_notification`).
    async fn refresh(&self) -> NotifierStatus {
        self.status()
    }

    /// Where signals for this sink's notifications go. A sink without
    /// signals ignores it.
    fn set_signal_handler(&self, _handler: SignalHandler) {}
}

/// Escapes `&`, `<`, and `>` for a server that advertises `body-markup`
/// (D6 "Content"). Plain text otherwise.
pub fn escape_body_markup(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    for character in text.chars() {
        match character {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            other => out.push(other),
        }
    }
    out
}
