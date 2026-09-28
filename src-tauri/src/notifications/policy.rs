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
/// [`BURST_WINDOW`], and the live summary.
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
    /// The server's id once shown; later candidates replace it in place.
    id: Option<u32>,
}

impl RateLimiter {
    /// Records the server id the sink returned for the current summary, so
    /// the next candidate in its window replaces it in place.
    pub fn summary_shown(&mut self, id: u32) {
        if let Some(summary) = self.summary.as_mut() {
            summary.id = Some(id);
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
/// when it has none), the window's focus, the limiter, and `now`.
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
    limiter.last_shown.insert(event.dedup_key.clone(), now);

    if let Some(summary) = limiter.summary.as_mut() {
        summary.count += 1;
        summary.worst = worse(summary.worst, event.severity);
        return Some(summary_notification(summary, event));
    }
    if limiter.recent.len() >= BURST_LIMIT {
        let summary = limiter.summary.insert(SummaryWindow {
            started: now,
            count: 1,
            worst: event.severity,
            id: None,
        });
        return Some(summary_notification(summary, event));
    }
    limiter.recent.push_back(now);
    Some(event_notification(event))
}

fn worse(left: AttentionSeverity, right: AttentionSeverity) -> AttentionSeverity {
    let rank = |severity| match severity {
        AttentionSeverity::Fatal => 2,
        AttentionSeverity::Warning => 1,
        AttentionSeverity::Info => 0,
    };
    if rank(right) > rank(left) {
        right
    } else {
        left
    }
}

/// One Event's notification: the Condition's label, then "<Severity>: " and
/// the Event's summary, scrubbed of anything address-shaped.
pub fn event_notification(event: &AttentionEvent) -> Notification {
    Notification {
        summary: condition_label(event.condition).to_string(),
        body: redact(&format!("{}: {}", severity_word(event.severity), event.summary)),
        urgency: Urgency::of(event.severity),
        replaces_id: 0,
        target: deep_link::source_target(event),
        open_attention_center: false,
        event_id: Some(event.id.clone()),
        event_ids: vec![event.id.clone()],
        kind: NotificationKind::Event,
    }
}

fn summary_notification(summary: &SummaryWindow, event: &AttentionEvent) -> Notification {
    let title = if summary.count == 1 {
        "1 new Attention Event".to_string()
    } else {
        format!("{} new Attention Events", summary.count)
    };
    Notification {
        summary: title,
        body: format!(
            "{}: open the Attention center to review them.",
            severity_word(summary.worst)
        ),
        urgency: Urgency::of(summary.worst),
        replaces_id: summary.id.unwrap_or(0),
        target: monitor_target(),
        open_attention_center: true,
        event_id: None,
        event_ids: vec![event.id.clone()],
        kind: NotificationKind::Summary {
            count: summary.count,
        },
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

/// What replaces an address- or credential-shaped word.
const REDACTED: &str = "[hidden]";

/// Global constraint 3: a notification body never carries a host, URL,
/// or credential. An Event's summary holds operator text (a Printer's
/// name and location, a Job's label), which an operator could fill with
/// one, so every whitespace-separated word shaped like one is replaced: a
/// URL (`://`), userinfo or an e-mail (`@`), a query or assignment (`=`,
/// `?`), an IPv4 or IPv6 address, or `a:b` (a `host:port` or a
/// `user:password`; a time such as `10:30` is kept). Everything else is
/// kept as written.
pub fn redact(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut word = String::new();
    for character in text.chars() {
        if character.is_whitespace() {
            flush_word(&mut out, &mut word);
            out.push(character);
        } else {
            word.push(character);
        }
    }
    flush_word(&mut out, &mut word);
    out
}

fn flush_word(out: &mut String, word: &mut String) {
    if word.is_empty() {
        return;
    }
    // Keep sentence punctuation around the word ("(…)", "….").
    let core = word.trim_matches(|c: char| matches!(c, '(' | ')' | '.' | ',' | ';' | '"' | '\''));
    if !core.is_empty() && looks_like_an_address(core) {
        let start = word.find(core).unwrap_or(0);
        out.push_str(&word[..start]);
        out.push_str(REDACTED);
        out.push_str(&word[start + core.len()..]);
    } else {
        out.push_str(word);
    }
    word.clear();
}

fn looks_like_an_address(word: &str) -> bool {
    if ["://", "@", "=", "?", "::"]
        .iter()
        .any(|marker| word.contains(marker))
    {
        return true;
    }
    if word.contains(':') {
        let parts: Vec<&str> = word.split(':').collect();
        let is_time = parts
            .iter()
            .all(|part| !part.is_empty() && part.chars().all(|c| c.is_ascii_digit()));
        let joined = parts
            .windows(2)
            .any(|pair| !pair[0].is_empty() && !pair[1].is_empty());
        if joined && !is_time {
            return true;
        }
    }
    word.split(|c: char| !(c.is_ascii_digit() || c == '.'))
        .any(is_ipv4)
}

fn is_ipv4(candidate: &str) -> bool {
    let parts: Vec<&str> = candidate.trim_matches('.').split('.').collect();
    parts.len() == 4
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.len() <= 3 && part.parse::<u8>().is_ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn redact_replaces_address_shaped_words_and_keeps_the_rest() {
        assert_eq!(
            redact("Voron http://u:p@192.0.2.10:8080/x?token=t (Bay 192.0.2.4:7125)."),
            "Voron [hidden] (Bay [hidden])."
        );
        assert_eq!(redact("Cube — Plate 1 failed on Voron."), "Cube — Plate 1 failed on Voron.");
        assert_eq!(redact("Voron 2.4 at 10:30"), "Voron 2.4 at 10:30");
        assert_eq!(redact("voron.local:7125 and voron:80"), "[hidden] and [hidden]");
        assert_eq!(redact("(operator:hunter2) Note: fine"), "([hidden]) Note: fine");
        assert_eq!(redact("user:secret@host"), "[hidden]");
        assert_eq!(redact("a=b fe80::1"), "[hidden] [hidden]");
        assert_eq!(redact("Spool #7 is low (80 g left)."), "Spool #7 is low (80 g left).");
    }
}
