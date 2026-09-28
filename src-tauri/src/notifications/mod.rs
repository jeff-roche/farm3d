//! P8 D6: the notification domain's pure wire types — the notifier's
//! reported status and the click-activation navigation payload. See the
//! P8 design spec's D6 ("Notifications") and "Backend model" module
//! layout table.
//!
//! This module (Task 3) is wire types only. The `NotificationSink` trait,
//! `Notification` (a sink call's content), `NotificationHandle`, and
//! `NotifyError` are Rust-internal (never serialized to the frontend) and,
//! along with the policy (`policy.rs`), focus (`focus.rs`), the D-Bus and
//! null sinks, activation, services, and commands, are later tasks (see
//! the module layout table in the design spec).

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contracts::navigation::NavigationTarget;
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
