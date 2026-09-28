//! P8 D6 "Policy": whether an Attention Event notifies, and what the
//! notification says.
//!
//! The Attention projector yields one [`NotifyCandidate`] per Event it
//! inserted with `origin: live` (`attention::projector::AppliedChanges::
//! notify`). [`decide`] is pure: it shows a candidate only when it is a
//! live insert (new or recurred), its class is enabled, its Printer isn't
//! muted, the window is unfocused, and the [`RateLimiter`] allows it.
//! Amendments, acknowledgements, resolutions, reads, and backfilled
//! inserts never notify, whatever reaches it.

use std::collections::{HashMap, VecDeque};

use chrono::{DateTime, Duration, Utc};

use crate::attention::deep_link;
use crate::attention::projector::EventChange;
use crate::attention::{AttentionEvent, AttentionOrigin, AttentionSeverity, ConditionKind};
use crate::printers::alerts::{AlertDefaults, NotificationMode};

use super::{monitor_target, Notification, NotificationClassSettings, NotificationKind, Urgency};

/// An Event the projector just changed, live, and how.
#[derive(Clone, Debug, PartialEq)]
pub struct NotifyCandidate {
    pub event: AttentionEvent,
    /// The projector hands on `Inserted` only; [`decide`] still refuses
    /// every other change.
    pub change: EventChange,
}

/// A dedup key notifies at most once per this long.
pub const PER_KEY_INTERVAL: Duration = Duration::minutes(10);
/// The burst window.
pub const BURST_WINDOW: Duration = Duration::seconds(10);
/// How many notifications a burst window shows by themselves; the next
/// one joins a summary.
pub const BURST_LIMIT: usize = 3;

/// D6 "Rate limiter". `last_shown` per dedup key (a candidate folded into
/// a summary counts as shown), the individual notifications of the last
/// [`BURST_WINDOW`], and the live summary. [`decide`] only reads it (and
/// forgets what has aged out); the caller [`records`](RateLimiter::record)
/// a notification once the sink has shown it, so a failed show never uses
/// up a key's 10 minutes or a burst slot.
#[derive(Debug, Default)]
pub struct RateLimiter {
    last_shown: HashMap<String, DateTime<Utc>>,
    recent: VecDeque<DateTime<Utc>>,
    summary: Option<SummaryWindow>,
}

#[derive(Debug)]
struct SummaryWindow {
    started: DateTime<Utc>,
    count: u32,
    worst: AttentionSeverity,
    /// The server's id; later candidates replace it in place.
    id: u32,
}

impl RateLimiter {
    /// Records `notification`, shown by the sink as `id` at `now`: its
    /// Events' keys, an individual notification toward the burst, or the
    /// summary (so the next candidate in its window replaces it in place).
    pub fn record(&mut self, notification: &Notification, id: u32, now: DateTime<Utc>) {
        for key in &notification.dedup_keys {
            self.last_shown.insert(key.clone(), now);
        }
        match notification.kind {
            NotificationKind::Event => self.recent.push_back(now),
            NotificationKind::Summary { count } => {
                let worst = severity_of(notification.urgency);
                match self.summary.as_mut() {
                    Some(summary) => {
                        summary.count = count;
                        summary.worst = worse(summary.worst, worst);
                        summary.id = id;
                    }
                    None => {
                        self.summary = Some(SummaryWindow {
                            started: now,
                            count,
                            worst,
                            id,
                        })
                    }
                }
            }
            NotificationKind::Test => {}
        }
    }

    fn forget_old(&mut self, now: DateTime<Utc>) {
        self.last_shown
            .retain(|_, shown| now - *shown < PER_KEY_INTERVAL);
        while self
            .recent
            .front()
            .is_some_and(|shown| now - *shown >= BURST_WINDOW)
        {
            self.recent.pop_front();
        }
        if self
            .summary
            .as_ref()
            .is_some_and(|summary| now - summary.started >= BURST_WINDOW)
        {
            self.summary = None;
        }
    }
}

/// Whether `candidate` notifies, and its content. Pure: the inputs are the
/// settings' classes, the Event's Printer's alert defaults (the defaults
/// when it has none), the window's focus, the limiter, and `now`. It
/// records nothing: the caller calls [`RateLimiter::record`] after a
/// successful show.
pub fn decide(
    candidate: &NotifyCandidate,
    classes: &NotificationClassSettings,
    alerts: &AlertDefaults,
    focused: bool,
    limiter: &mut RateLimiter,
    now: DateTime<Utc>,
) -> Option<Notification> {
    let event = &candidate.event;
    if !matches!(candidate.change, EventChange::Inserted { .. })
        || event.origin != AttentionOrigin::Live
        || !classes.enabled(event.notification_class)
    {
        return None;
    }
    // Job and requirement Events carry their Job's Printer; `spool.low`
    // has none and is never muted.
    if event.printer_id.is_some() && alerts.notifications == NotificationMode::Muted {
        return None;
    }
    if focused {
        return None;
    }

    limiter.forget_old(now);
    if limiter.last_shown.contains_key(&event.dedup_key) {
        return None;
    }
    if let Some(summary) = limiter.summary.as_ref() {
        return Some(summary_notification(
            summary.count + 1,
            worse(summary.worst, event.severity),
            summary.id,
            event,
        ));
    }
    if limiter.recent.len() >= BURST_LIMIT {
        return Some(summary_notification(1, event.severity, 0, event));
    }
    Some(event_notification(event))
}

fn rank(severity: AttentionSeverity) -> u8 {
    match severity {
        AttentionSeverity::Fatal => 2,
        AttentionSeverity::Warning => 1,
        AttentionSeverity::Info => 0,
    }
}

fn worse(left: AttentionSeverity, right: AttentionSeverity) -> AttentionSeverity {
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

fn severity_of(urgency: Urgency) -> AttentionSeverity {
    match urgency {
        Urgency::Critical => AttentionSeverity::Fatal,
        Urgency::Normal => AttentionSeverity::Warning,
        Urgency::Low => AttentionSeverity::Info,
    }
}

/// One Event's notification (D6 "Content"): the Condition's label as the
/// title; the body is the severity word, a colon, and the Event's own
/// summary, exactly as the Attention center shows it. The summary is
/// host-free by construction (`attention::summary` reads only the
/// subject's names and labels, never a Connection, credential, or camera).
pub fn event_notification(event: &AttentionEvent) -> Notification {
    Notification {
        summary: condition_label(event.condition).to_string(),
        body: format!("{}: {}", severity_word(event.severity), event.summary),
        urgency: Urgency::of(event.severity),
        replaces_id: 0,
        target: deep_link::source_target(event),
        open_attention_center: false,
        event_id: Some(event.id.clone()),
        event_ids: vec![event.id.clone()],
        dedup_keys: vec![event.dedup_key.clone()],
        kind: NotificationKind::Event,
    }
}

fn summary_notification(
    count: u32,
    worst: AttentionSeverity,
    replaces_id: u32,
    event: &AttentionEvent,
) -> Notification {
    let title = if count == 1 {
        "1 new Attention Event".to_string()
    } else {
        format!("{count} new Attention Events")
    };
    Notification {
        summary: title,
        body: format!(
            "{}: open the Attention center to review them.",
            severity_word(worst)
        ),
        urgency: Urgency::of(worst),
        replaces_id,
        target: monitor_target(),
        open_attention_center: true,
        event_id: None,
        event_ids: vec![event.id.clone()],
        dedup_keys: vec![event.dedup_key.clone()],
        kind: NotificationKind::Summary { count },
    }
}

/// The body's first word: never only the icon.
pub fn severity_word(severity: AttentionSeverity) -> &'static str {
    match severity {
        AttentionSeverity::Fatal => "Fatal",
        AttentionSeverity::Warning => "Warning",
        AttentionSeverity::Info => "Info",
    }
}

/// The notification title per Condition.
pub fn condition_label(kind: ConditionKind) -> &'static str {
    match kind {
        ConditionKind::PrinterOffline => "Printer offline",
        ConditionKind::PrinterConnectionError => "Printer connection error",
        ConditionKind::PrinterHostFailed => "Printer reported a failed print",
        ConditionKind::JobStartConfirmation => "Job waiting to start",
        ConditionKind::JobFailed => "Job failed",
        ConditionKind::JobHostCancelled => "Job cancelled by the host",
        ConditionKind::RequirementMaterialReconciliation => "Material to settle",
        ConditionKind::RequirementJobOutcomeUnknown => "Job outcome unknown",
        ConditionKind::SpoolLow => "Spool low",
        ConditionKind::JobCompleted => "Job completed",
    }
}
