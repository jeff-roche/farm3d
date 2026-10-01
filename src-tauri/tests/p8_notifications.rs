//! Task 9 (P8 D6, ADR-0015): desktop notifications.
//!
//! - Step 1, pure: `decide`, tabled over the class setting × the
//!   Printer's `muted` × focus × origin × change, then the per-key and
//!   burst limits, and the content (label, severity word, no host).
//! - Step 2, the `NotificationService` over a `RecordingSink`: focus,
//!   the decision-8 target, a simulated click (`farm3d-navigate-v1` and
//!   the Event read), closes, the 256 bound, and the raise fallbacks.
//! - Step 4, settings: the new columns on `save_settings`, export schema
//!   3, imports of schemas 1–3, the janitor poke, and the commands.
//!
//! No test here sends a real desktop notification: every service uses a
//! `RecordingSink` (or the null sink).

mod common;

use chrono::{DateTime, Duration as ChronoDuration, Utc};

use farm3d_lib::attention::projector::EventChange;
use farm3d_lib::attention::{
    AttentionDetail, AttentionEvent, AttentionOrigin, AttentionSeverity, AttentionSource,
    AttentionSubject, ConditionKind, MaterialReconciliationStatus, NotificationClass,
    PrinterConnectionErrorCause,
};
use farm3d_lib::contracts::navigation::{
    NavigationDestination, NavigationSelection, NavigationSelectionKind, NavigationTarget,
};
use farm3d_lib::notifications::policy::{decide, NotifyCandidate, RateLimiter};
use farm3d_lib::notifications::{NotificationClassSettings, NotificationKind, Urgency};
use farm3d_lib::printers::alerts::{AlertDefaults, NotificationMode};

const NOW_TEXT: &str = "2026-09-28T09:00:00Z";

fn now() -> DateTime<Utc> {
    NOW_TEXT.parse().unwrap()
}

/// The seeded-secret corpus (global constraint 3): a credential, a URL
/// with userinfo and a query token, and an RFC 5737 LAN-style host.
const SECRET_URL: &str = "http://operator:s3cr3t-P8N@192.0.2.10:8080/webcam?token=tok-P8N-77";
const CORPUS: [&str; 5] = [
    SECRET_URL,
    "s3cr3t-P8N",
    "operator:s3cr3t-P8N",
    "tok-P8N-77",
    "192.0.2.10",
];

fn assert_no_corpus(what: &str, text: &str) {
    for secret in CORPUS {
        assert!(!text.contains(secret), "{what} leaked {secret:?}: {text}");
    }
}

fn detail_for(kind: ConditionKind) -> AttentionDetail {
    match kind {
        ConditionKind::PrinterOffline => AttentionDetail::PrinterOffline {
            unreachable_since: NOW_TEXT.into(),
        },
        ConditionKind::PrinterConnectionError => AttentionDetail::PrinterConnectionError {
            cause: PrinterConnectionErrorCause::Auth,
        },
        ConditionKind::PrinterHostFailed => AttentionDetail::PrinterHostFailed,
        ConditionKind::JobStartConfirmation => AttentionDetail::JobStartConfirmation {
            awaiting_material: false,
        },
        ConditionKind::JobFailed => AttentionDetail::JobFailed {
            ended_at: NOW_TEXT.into(),
        },
        ConditionKind::JobHostCancelled => AttentionDetail::JobHostCancelled {
            ended_at: NOW_TEXT.into(),
        },
        ConditionKind::RequirementMaterialReconciliation => {
            AttentionDetail::RequirementMaterialReconciliation {
                requirement_status: MaterialReconciliationStatus::Pending,
                spool_id: "spl-1".into(),
            }
        }
        ConditionKind::RequirementJobOutcomeUnknown => {
            AttentionDetail::RequirementJobOutcomeUnknown
        }
        ConditionKind::SpoolLow => AttentionDetail::SpoolLow {
            current_mg: 80_000,
            low_threshold_mg: 100_000,
        },
        ConditionKind::JobCompleted => AttentionDetail::JobCompleted {
            ended_at: NOW_TEXT.into(),
        },
    }
}

/// A committed Event row of `kind`, as the projector hands it on. Its ids
/// follow D2 (the Job's Printer for Job and requirement Events; no
/// Printer for `spool.low`).
fn event_of(kind: ConditionKind, origin: AttentionOrigin, source_id: &str) -> AttentionEvent {
    event_with_subject(
        kind,
        origin,
        source_id,
        AttentionSubject {
            printer_name: Some("Voron".into()),
            printer_location: Some("Bay A".into()),
            job_label: Some("Cube — Plate 1".into()),
            spool_number: Some(7),
            spool_label: Some("Polymaker PLA".into()),
        },
    )
}

fn event_with_subject(
    kind: ConditionKind,
    origin: AttentionOrigin,
    source_id: &str,
    subject: AttentionSubject,
) -> AttentionEvent {
    let spec = kind.spec();
    let detail = detail_for(kind);
    let (printer_id, job_id, spool_id, requirement_id) = match spec.source_kind {
        farm3d_lib::attention::AttentionSourceKind::Printer => {
            (Some(source_id.to_string()), None, None, None)
        }
        farm3d_lib::attention::AttentionSourceKind::Job => (
            Some("prn-voron".to_string()),
            Some(source_id.to_string()),
            None,
            None,
        ),
        farm3d_lib::attention::AttentionSourceKind::ReconciliationRequirement => (
            Some("prn-voron".to_string()),
            Some("job-of-requirement".to_string()),
            Some("spl-1".to_string()),
            Some(source_id.to_string()),
        ),
        farm3d_lib::attention::AttentionSourceKind::Spool => {
            (None, None, Some(source_id.to_string()), None)
        }
    };
    AttentionEvent {
        id: format!("att-{}-{source_id}", kind.as_str()),
        revision: 1,
        dedup_key: farm3d_lib::attention::dedup_key(kind, source_id),
        condition: kind,
        severity: spec.severity,
        requires_action: spec.requires_action,
        resolution_mode: spec.resolution_mode,
        notification_class: spec.notification_class,
        source: AttentionSource {
            kind: spec.source_kind,
            id: source_id.to_string(),
        },
        printer_id,
        job_id,
        spool_id,
        requirement_id,
        incident_id: None,
        summary: farm3d_lib::attention::summary(kind, &subject, &detail),
        subject,
        detail,
        origin,
        first_observed_at: NOW_TEXT.into(),
        last_observed_at: NOW_TEXT.into(),
        observation_count: 1,
        recurrence_of: None,
        read_at: None,
        acknowledged_at: None,
        resolved_at: None,
        resolution: None,
        notified_at: None,
        evidence: None,
        allowed_actions: vec![],
    }
}

fn candidate(kind: ConditionKind, origin: AttentionOrigin, change: EventChange) -> NotifyCandidate {
    NotifyCandidate {
        event: event_of(kind, origin, "src-1"),
        change,
    }
}

fn all_on() -> NotificationClassSettings {
    NotificationClassSettings {
        fatal: true,
        confirmation: true,
        completion: true,
        reconciliation: true,
        connectivity: true,
        inventory: true,
    }
}

fn only_off(class: NotificationClass) -> NotificationClassSettings {
    let mut classes = all_on();
    match class {
        NotificationClass::Fatal => classes.fatal = false,
        NotificationClass::Confirmation => classes.confirmation = false,
        NotificationClass::Completion => classes.completion = false,
        NotificationClass::Reconciliation => classes.reconciliation = false,
        NotificationClass::Connectivity => classes.connectivity = false,
        NotificationClass::Inventory => classes.inventory = false,
    }
    classes
}

fn alerts(mode: NotificationMode) -> AlertDefaults {
    AlertDefaults {
        notifications: mode,
        ..AlertDefaults::default()
    }
}

// --- Step 1: decide ---------------------------------------------------------------

#[test]
fn the_defaults_match_decision_7() {
    assert_eq!(
        NotificationClassSettings::default(),
        NotificationClassSettings {
            fatal: true,
            confirmation: true,
            completion: true,
            reconciliation: false,
            connectivity: false,
            inventory: false,
        }
    );
}

/// The full table: every Condition × class on/off × follow/muted × focus
/// × origin × change. A fresh limiter per row, so only the gates decide.
#[test]
fn decide_notifies_only_a_live_insert_of_an_enabled_unmuted_class_while_unfocused() {
    let changes = [
        EventChange::Inserted { recurred: false },
        EventChange::Inserted { recurred: true },
        EventChange::Amended,
        EventChange::Acknowledged,
        EventChange::Resolved,
        EventChange::Read,
    ];
    let mut rows = 0;
    for kind in ConditionKind::ALL {
        let class = kind.spec().notification_class;
        for class_on in [true, false] {
            let classes = if class_on { all_on() } else { only_off(class) };
            for mode in NotificationMode::ALL {
                for focused in [false, true] {
                    for origin in [AttentionOrigin::Live, AttentionOrigin::Backfill] {
                        for change in changes {
                            rows += 1;
                            let mut limiter = RateLimiter::default();
                            let shown = decide(
                                &candidate(kind, origin, change),
                                &classes,
                                &alerts(mode),
                                focused,
                                &mut limiter,
                                now(),
                            );
                            // `spool.low` has no Printer, so it is never muted.
                            let muted =
                                mode == NotificationMode::Muted && kind != ConditionKind::SpoolLow;
                            let expected = class_on
                                && !muted
                                && !focused
                                && origin == AttentionOrigin::Live
                                && matches!(change, EventChange::Inserted { .. });
                            assert_eq!(
                                shown.is_some(),
                                expected,
                                "{kind:?} class_on={class_on} {mode:?} focused={focused} {origin:?} {change:?}"
                            );
                        }
                    }
                }
            }
        }
    }
    assert_eq!(rows, 10 * 2 * 2 * 2 * 2 * 6);
}

#[test]
fn a_shown_event_carries_its_label_the_severity_word_and_the_decision_8_target() {
    let cases = [
        (
            ConditionKind::JobFailed,
            "Job failed",
            "Fatal: Cube — Plate 1 failed on Voron.",
            Urgency::Critical,
            NavigationDestination::Queue,
            NavigationSelectionKind::Job,
            "src-1",
        ),
        (
            ConditionKind::PrinterOffline,
            "Printer offline",
            "Warning: Voron (Bay A) is offline.",
            Urgency::Normal,
            NavigationDestination::Monitor,
            NavigationSelectionKind::Printer,
            "src-1",
        ),
        (
            ConditionKind::JobCompleted,
            "Job completed",
            "Info: Cube — Plate 1 finished on Voron.",
            Urgency::Low,
            NavigationDestination::Queue,
            NavigationSelectionKind::Job,
            "src-1",
        ),
        (
            ConditionKind::SpoolLow,
            "Spool low",
            "Warning: Spool #7 is low (80 g left).",
            Urgency::Normal,
            NavigationDestination::Spools,
            NavigationSelectionKind::Spool,
            "src-1",
        ),
        (
            // A requirement opens its Job.
            ConditionKind::RequirementJobOutcomeUnknown,
            "Job outcome unknown",
            "Fatal: Cube — Plate 1 has an unknown outcome on Voron.",
            Urgency::Critical,
            NavigationDestination::Queue,
            NavigationSelectionKind::Job,
            "job-of-requirement",
        ),
    ];
    for (kind, label, body, urgency, destination, selection, selected) in cases {
        let mut limiter = RateLimiter::default();
        let shown = decide(
            &candidate(
                kind,
                AttentionOrigin::Live,
                EventChange::Inserted { recurred: false },
            ),
            &all_on(),
            &alerts(NotificationMode::Follow),
            false,
            &mut limiter,
            now(),
        )
        .unwrap_or_else(|| panic!("{kind:?} should notify"));
        assert_eq!(shown.summary, label);
        assert_eq!(shown.body, body);
        assert_eq!(shown.urgency, urgency);
        assert_eq!(shown.replaces_id, 0);
        assert_eq!(shown.kind, NotificationKind::Event);
        assert!(!shown.open_attention_center);
        assert_eq!(
            shown.event_id.as_deref(),
            Some(format!("att-{}-src-1", kind.as_str()).as_str())
        );
        assert_eq!(
            shown.target,
            NavigationTarget {
                version: Default::default(),
                destination,
                selection: Some(NavigationSelection {
                    kind: selection,
                    id: selected.to_string(),
                }),
            }
        );
    }
}

#[test]
fn every_condition_has_a_label_and_every_body_starts_with_its_severity_word() {
    for kind in ConditionKind::ALL {
        let mut limiter = RateLimiter::default();
        let shown = decide(
            &candidate(
                kind,
                AttentionOrigin::Live,
                EventChange::Inserted { recurred: false },
            ),
            &all_on(),
            &alerts(NotificationMode::Follow),
            false,
            &mut limiter,
            now(),
        )
        .unwrap();
        assert!(!shown.summary.is_empty(), "{kind:?}");
        let word = match kind.spec().severity {
            AttentionSeverity::Fatal => "Fatal: ",
            AttentionSeverity::Warning => "Warning: ",
            AttentionSeverity::Info => "Info: ",
        };
        assert!(shown.body.starts_with(word), "{kind:?}: {}", shown.body);
    }
}

/// `decide`, then — as the service does after the sink showed it —
/// `record` with a stand-in server id.
fn show(
    candidate: &NotifyCandidate,
    limiter: &mut RateLimiter,
    at: DateTime<Utc>,
    id: u32,
) -> Option<farm3d_lib::notifications::Notification> {
    let shown = decide(
        candidate,
        &all_on(),
        &alerts(NotificationMode::Follow),
        false,
        limiter,
        at,
    )?;
    limiter.record(&shown, id, at);
    Some(shown)
}

#[test]
fn the_same_key_notifies_at_most_once_in_ten_minutes() {
    let mut limiter = RateLimiter::default();
    let insert = candidate(
        ConditionKind::PrinterOffline,
        AttentionOrigin::Live,
        EventChange::Inserted { recurred: false },
    );
    let recur = candidate(
        ConditionKind::PrinterOffline,
        AttentionOrigin::Live,
        EventChange::Inserted { recurred: true },
    );
    let at = |minutes: i64| now() + ChronoDuration::minutes(minutes);
    assert!(show(&insert, &mut limiter, at(0), 1).is_some());
    // A recurrence of the same key 9 minutes later: suppressed.
    assert!(show(&recur, &mut limiter, at(9), 2).is_none());
    // A suppressed candidate doesn't restart the 10 minutes.
    assert!(show(&recur, &mut limiter, at(10), 3).is_some());
    // Another key is unaffected.
    let other = NotifyCandidate {
        event: event_of(
            ConditionKind::PrinterOffline,
            AttentionOrigin::Live,
            "src-2",
        ),
        change: EventChange::Inserted { recurred: false },
    };
    assert!(show(&other, &mut limiter, at(10), 4).is_some());
}

#[test]
fn a_notification_the_sink_never_showed_uses_up_neither_its_key_nor_a_burst_slot() {
    let mut limiter = RateLimiter::default();
    let classes = all_on();
    let follow = alerts(NotificationMode::Follow);
    let nth = |n: usize| NotifyCandidate {
        event: event_of(
            ConditionKind::PrinterOffline,
            AttentionOrigin::Live,
            &format!("src-{n}"),
        ),
        change: EventChange::Inserted { recurred: false },
    };
    // Five failed shows (decided, never recorded): no slot is used.
    for n in 0..5 {
        let decided = decide(&nth(n), &classes, &follow, false, &mut limiter, now()).unwrap();
        assert_eq!(decided.kind, NotificationKind::Event);
    }
    // The same key right after a failed show: still shown by itself.
    let retried = show(&nth(0), &mut limiter, now(), 1).unwrap();
    assert_eq!(retried.kind, NotificationKind::Event);
}

#[test]
fn a_fourth_notification_within_ten_seconds_becomes_one_summary_that_opens_the_attention_center() {
    let mut limiter = RateLimiter::default();
    let nth = |n: usize, kind: ConditionKind| NotifyCandidate {
        event: event_of(kind, AttentionOrigin::Live, &format!("src-{n}")),
        change: EventChange::Inserted { recurred: false },
    };
    let at = |seconds: i64| now() + ChronoDuration::seconds(seconds);
    for n in 0..3 {
        let shown = show(
            &nth(n, ConditionKind::PrinterOffline),
            &mut limiter,
            at(n as i64),
            n as u32 + 1,
        )
        .unwrap();
        assert_eq!(
            shown.kind,
            NotificationKind::Event,
            "#{n} is shown by itself"
        );
    }
    // The 4th, 3 s after the first: one summary, a new notification; the
    // sink returns id 41 for it.
    let summary = show(
        &nth(3, ConditionKind::JobCompleted),
        &mut limiter,
        at(3),
        41,
    )
    .unwrap();
    assert_eq!(summary.kind, NotificationKind::Summary { count: 1 });
    assert_eq!(summary.summary, "1 new Attention Event");
    assert!(summary.open_attention_center);
    assert_eq!(summary.event_id, None);
    assert_eq!(
        summary.event_ids,
        vec!["att-job.completed-src-3".to_string()]
    );
    assert_eq!(summary.replaces_id, 0);
    assert_eq!(
        summary.target,
        NavigationTarget {
            version: Default::default(),
            destination: NavigationDestination::Monitor,
            selection: None,
        }
    );
    assert_eq!(summary.urgency, Urgency::Low);
    // The 5th in the same window replaces it in place, with the new count
    // and the worst severity so far.
    let replaced = show(&nth(4, ConditionKind::JobFailed), &mut limiter, at(8), 41).unwrap();
    assert_eq!(replaced.kind, NotificationKind::Summary { count: 2 });
    assert_eq!(replaced.summary, "2 new Attention Events");
    assert_eq!(replaced.replaces_id, 41);
    assert_eq!(replaced.urgency, Urgency::Critical);
    assert!(replaced.body.starts_with("Fatal: "), "{}", replaced.body);
    // The 6th keeps the worst severity seen in the window.
    let third = show(
        &nth(6, ConditionKind::JobCompleted),
        &mut limiter,
        at(9),
        41,
    )
    .unwrap();
    assert_eq!(third.kind, NotificationKind::Summary { count: 3 });
    assert_eq!(third.urgency, Urgency::Critical);
    // A folded key counts as shown for the per-key rule.
    assert!(show(&nth(4, ConditionKind::JobFailed), &mut limiter, at(9), 41).is_none());
    // After the window, a candidate is shown by itself again.
    let later = show(
        &nth(5, ConditionKind::PrinterOffline),
        &mut limiter,
        at(14),
        50,
    )
    .unwrap();
    assert_eq!(later.kind, NotificationKind::Event);
}

#[test]
fn a_gated_candidate_never_touches_the_limiter() {
    let mut limiter = RateLimiter::default();
    let classes = all_on();
    let follow = alerts(NotificationMode::Follow);
    let offline = candidate(
        ConditionKind::PrinterOffline,
        AttentionOrigin::Live,
        EventChange::Inserted { recurred: false },
    );
    // Focused: not shown, and not remembered for the per-key rule.
    assert!(decide(&offline, &classes, &follow, true, &mut limiter, now()).is_none());
    assert!(show(&offline, &mut limiter, now(), 1).is_some());
}

/// D6 "Content" (controller ruling, fix round 1): the body is the severity
/// word, a colon, and the Event's summary exactly as the Attention center
/// shows it — operator punctuation included.
#[test]
fn the_body_is_the_severity_word_and_the_event_summary_verbatim() {
    let subject = AttentionSubject {
        printer_name: Some("PLA:Black rig@bay".into()),
        printer_location: Some("shelf=2 (which?)".into()),
        job_label: Some("Cube v2: final? a=b @home".into()),
        spool_number: Some(3),
        spool_label: Some("PLA:Black".into()),
    };
    for kind in ConditionKind::ALL {
        let event = event_with_subject(kind, AttentionOrigin::Live, "src-1", subject.clone());
        let shown = show(
            &NotifyCandidate {
                event: event.clone(),
                change: EventChange::Inserted { recurred: false },
            },
            &mut RateLimiter::default(),
            now(),
            1,
        )
        .unwrap();
        let word = match event.severity {
            AttentionSeverity::Fatal => "Fatal",
            AttentionSeverity::Warning => "Warning",
            AttentionSeverity::Info => "Info",
        };
        assert_eq!(shown.body, format!("{word}: {}", event.summary), "{kind:?}");
    }
    let offline = event_with_subject(
        ConditionKind::PrinterOffline,
        AttentionOrigin::Live,
        "src-1",
        subject,
    );
    assert_eq!(
        event_notification_body(&offline),
        "Warning: PLA:Black rig@bay (shelf=2 (which?)) is offline."
    );
}

fn event_notification_body(event: &AttentionEvent) -> String {
    farm3d_lib::notifications::policy::event_notification(event).body
}

#[test]
fn markup_is_escaped_only_by_the_escape_helper() {
    assert_eq!(
        farm3d_lib::notifications::escape_body_markup("A & B <i>x</i> > y"),
        "A &amp; B &lt;i&gt;x&lt;/i&gt; &gt; y"
    );
}

// --- Step 2: the NotificationService over a RecordingSink ---------------------------

use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use farm3d_lib::attention::repository as attention_repo;
use farm3d_lib::attention::Condition;
use farm3d_lib::connections::supervisor::STATUS_EVENT;
use farm3d_lib::notifications::activation::RaiseStep;
use farm3d_lib::notifications::recording::{RecordingSink, RecordingWindowControl};
use farm3d_lib::notifications::services::{
    NotificationService, NotificationTimings, OUTSTANDING_LIMIT,
};
use farm3d_lib::notifications::{
    Notification, NotifierStatus, NotifierUnavailableReason, NAVIGATE_EVENT,
};
use farm3d_lib::persistence::{RepositoryError, Storage};
use farm3d_lib::RuntimeServices;
use serde_json::{json, Value};
use tauri::test::MockRuntime;
use tauri::Listener;

const PRINTER: &str = "prn-voron";
const RAISE_WAIT: Duration = Duration::from_millis(60);
const WAIT: Duration = Duration::from_secs(10);

struct Rig {
    _temp: tempfile::TempDir,
    _lease: farm3d_lib::persistence::MetadataRootLease,
    _app: tauri::App<MockRuntime>,
    webview: tauri::WebviewWindow<MockRuntime>,
    services: Arc<RuntimeServices<MockRuntime>>,
    storage: Arc<Storage>,
    sink: Arc<RecordingSink>,
    control: Arc<RecordingWindowControl>,
    navigations: Arc<Mutex<Vec<Value>>>,
    stream: Arc<Mutex<Vec<Value>>>,
    documents: Arc<Documents>,
}

impl Drop for Rig {
    fn drop(&mut self) {
        self.services.notifications.stop();
    }
}

/// A Settings document picker/reader/writer that never opens a dialog.
#[derive(Default)]
struct Documents {
    bytes: Mutex<Option<Vec<u8>>>,
    writes: Mutex<Vec<Vec<u8>>>,
}

impl farm3d_lib::document_io::DocumentIo for Documents {
    fn open_json(
        &self,
        _kind: farm3d_lib::document_io::DocumentKind,
    ) -> Result<Option<std::path::PathBuf>, farm3d_lib::contracts::command::CommandError> {
        Ok(Some("settings.json".into()))
    }
    fn save_json(
        &self,
        _kind: farm3d_lib::document_io::DocumentKind,
    ) -> Result<Option<std::path::PathBuf>, farm3d_lib::contracts::command::CommandError> {
        Ok(Some("settings.json".into()))
    }
    fn read(
        &self,
        _path: &std::path::Path,
    ) -> Result<Vec<u8>, farm3d_lib::contracts::command::CommandError> {
        Ok(self.bytes.lock().unwrap().clone().unwrap_or_default())
    }
    fn atomic_write(
        &self,
        _path: &std::path::Path,
        bytes: &[u8],
    ) -> Result<(), farm3d_lib::contracts::command::CommandError> {
        self.writes.lock().unwrap().push(bytes.to_vec());
        Ok(())
    }
}

fn rig_with(sink: RecordingSink) -> Rig {
    let (temp, lease, storage, _database) = common::storage();
    farm3d_lib::settings::repository::SettingsRepository::new(Arc::clone(&storage))
        .ensure_default()
        .unwrap();
    let sink = Arc::new(sink);
    let control = Arc::new(RecordingWindowControl::default());
    let documents = Arc::new(Documents::default());
    let service_sink = Arc::clone(&sink);
    let service_documents = Arc::clone(&documents);
    let credentials = temp.path().join("credentials");
    let (app, webview, _manager, services) = common::runtime_with(
        tauri::generate_handler![
            farm3d_lib::notifications::commands::notification_status,
            farm3d_lib::notifications::commands::send_test_notification,
            farm3d_lib::printers::alerts::get_printer_alert_defaults,
            farm3d_lib::printers::alerts::set_printer_alert_defaults,
            farm3d_lib::settings::commands::load_settings,
            farm3d_lib::settings::commands::save_settings,
            farm3d_lib::settings::commands::export_settings,
            farm3d_lib::settings::commands::import_settings,
        ],
        Arc::clone(&storage),
        Arc::new(common::a_catalog()),
        credentials,
        |_, _| None,
        move |services| {
            services.notifications = Arc::new(NotificationService::new(
                service_sink,
                Default::default(),
                NotificationTimings {
                    raise_wait: RAISE_WAIT,
                    ..NotificationTimings::default()
                },
            ));
            services.documents = service_documents;
        },
    );
    services
        .notifications
        .set_window_control(Arc::clone(&control) as _);
    let navigations = Arc::new(Mutex::new(Vec::new()));
    let sink_navigations = Arc::clone(&navigations);
    app.listen(NAVIGATE_EVENT, move |event| {
        sink_navigations
            .lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap());
    });
    let stream = Arc::new(Mutex::new(Vec::new()));
    let sink_stream = Arc::clone(&stream);
    app.listen(STATUS_EVENT, move |event| {
        sink_stream
            .lock()
            .unwrap()
            .push(serde_json::from_str(event.payload()).unwrap());
    });
    farm3d_lib::start_notification_runtime(&services, app.handle());
    storage
        .write(|tx| {
            tx.execute_batch(&format!(
                "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                   catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                   overrides_json, created_at, updated_at)
                 VALUES ('{PRINTER}', 1, 'Voron', '', '', '', '', '', '', '{{}}',
                         '{NOW_TEXT}', '{NOW_TEXT}');"
            ))?;
            Ok(())
        })
        .unwrap();
    Rig {
        _temp: temp,
        _lease: lease,
        _app: app,
        webview,
        services,
        storage,
        sink,
        control,
        navigations,
        stream,
        documents,
    }
}

fn rig() -> Rig {
    rig_with(RecordingSink::available())
}

fn wait_until(what: &str, done: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !done() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        std::thread::sleep(Duration::from_millis(5));
    }
}

impl Rig {
    fn call(&self, command: &str, mut body: Value) -> Result<Value, Value> {
        body["contractVersion"] = json!(1);
        common::invoke(&self.webview, command, body).map(|success| success["data"].clone())
    }

    fn ok(&self, command: &str, body: Value) -> Value {
        self.call(command, body)
            .unwrap_or_else(|error| panic!("{command} failed: {error}"))
    }

    /// Inserts a live `printer.hostFailed` (fatal, on by default) for the
    /// Printer, with `subject`, and returns its row.
    fn insert_host_failed(&self, printer_id: &str, subject: AttentionSubject) -> AttentionEvent {
        self.storage
            .write_repo(|tx| -> Result<AttentionEvent, RepositoryError> {
                attention_repo::insert(
                    tx,
                    &Condition {
                        kind: ConditionKind::PrinterHostFailed,
                        source_id: printer_id.to_string(),
                        printer_id: Some(printer_id.to_string()),
                        job_id: None,
                        spool_id: None,
                        requirement_id: None,
                        subject,
                        detail: AttentionDetail::PrinterHostFailed,
                        acknowledge: false,
                    },
                    None,
                    false,
                    AttentionOrigin::Live,
                    now(),
                )
            })
            .unwrap()
    }

    fn voron_failed(&self) -> AttentionEvent {
        self.insert_host_failed(
            PRINTER,
            AttentionSubject {
                printer_name: Some("Voron".into()),
                printer_location: Some("Bay A".into()),
                job_label: None,
                spool_number: None,
                spool_label: None,
            },
        )
    }

    fn consider(&self, event: &AttentionEvent) -> Option<u32> {
        tauri::async_runtime::block_on(self.services.notifications.consider(&NotifyCandidate {
            event: event.clone(),
            change: EventChange::Inserted { recurred: false },
        }))
        .map(|handle| handle.id)
    }

    fn event(&self, id: &str) -> AttentionEvent {
        self.storage
            .read(|connection| Ok(attention_repo::load_event(connection, id)))
            .unwrap()
            .unwrap()
            .unwrap()
    }

    /// Delivers `signal` through the sink and waits until the service has
    /// handled it.
    fn signal(&self, send: impl FnOnce(&RecordingSink)) {
        let before = self.services.notifications.signals_handled();
        send(&self.sink);
        wait_until("the signal to be handled", || {
            self.services.notifications.signals_handled() > before
        });
    }
}

fn printer_target(id: &str) -> Value {
    json!({"version": 1, "destination": "monitor", "selection": {"kind": "printer", "id": id}})
}

#[test]
fn nothing_is_shown_while_farm3d_has_focus_and_it_starts_focused() {
    let rig = rig();
    assert!(
        rig.services.notifications.focus().is_focused(),
        "Focus seeds true"
    );
    let event = rig.voron_failed();
    assert_eq!(rig.consider(&event), None);
    assert!(rig.sink.shown().is_empty());
    assert_eq!(rig.event(&event.id).notified_at, None);
}

#[test]
fn unfocused_a_live_insert_is_shown_once_with_its_target_and_marked_notified() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    let event = rig.voron_failed();
    let id = rig.consider(&event).expect("shown");
    let shown = rig.sink.shown();
    assert_eq!(shown.len(), 1);
    let (shown_id, notification): &(u32, Notification) = &shown[0];
    assert_eq!(*shown_id, id);
    assert_eq!(notification.summary, "Printer reported a failed print");
    assert_eq!(
        notification.body,
        "Fatal: Voron (Bay A) reported a failed print."
    );
    assert_eq!(
        serde_json::to_value(&notification.target).unwrap(),
        printer_target(PRINTER)
    );
    let outstanding = rig.services.notifications.outstanding(id).unwrap();
    assert_eq!(outstanding.event_id.as_deref(), Some(event.id.as_str()));
    assert!(!outstanding.open_attention_center);
    // notified_at is written, with no revision bump and no stream event.
    let stored = rig.event(&event.id);
    assert!(stored.notified_at.is_some());
    assert_eq!(stored.revision, event.revision);
    assert!(rig.stream.lock().unwrap().is_empty());
    // The same Event again (a stale re-delivery): the per-key limit holds.
    assert_eq!(rig.consider(&event), None);
}

#[test]
fn a_muted_printer_or_a_disabled_class_shows_nothing() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    rig.ok(
        "set_printer_alert_defaults",
        json!({
            "operationId": "op-mute",
            "printerId": PRINTER,
            "alertDefaults": {
                "offlineAfterMinutes": 5, "notifications": "muted",
                "snapshotOnIncident": true, "snapshotOnCompletion": true,
            },
        }),
    );
    assert_eq!(rig.consider(&rig.voron_failed()), None);

    let settings = rig.ok("load_settings", json!({}));
    rig.ok(
        "set_printer_alert_defaults",
        json!({
            "operationId": "op-follow",
            "printerId": PRINTER,
            "alertDefaults": {
                "offlineAfterMinutes": 5, "notifications": "follow",
                "snapshotOnIncident": true, "snapshotOnCompletion": true,
            },
        }),
    );
    rig.ok(
        "save_settings",
        json!({
            "expectedRevision": settings["revision"],
            "themeMode": "system",
            "monitorSection": "printerModel",
            "monitorDensity": "comfortable",
            "notifications": {
                "fatal": false, "confirmation": true, "completion": true,
                "reconciliation": false, "connectivity": false, "inventory": false,
            },
        }),
    );
    // Another Printer (following), with the fatal class now off.
    rig.storage
        .write(|tx| {
            tx.execute_batch(&format!(
                "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                   catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                   overrides_json, created_at, updated_at)
                 VALUES ('prn-other', 1, 'Other', '', '', '', '', '', '', '{{}}',
                         '{NOW_TEXT}', '{NOW_TEXT}');"
            ))?;
            Ok(())
        })
        .unwrap();
    let other = rig.insert_host_failed(
        "prn-other",
        AttentionSubject {
            printer_name: Some("Other".into()),
            printer_location: None,
            job_label: None,
            spool_number: None,
            spool_label: None,
        },
    );
    assert_eq!(rig.consider(&other), None);
    assert!(rig.sink.shown().is_empty());
}

#[test]
fn a_click_raises_the_window_navigates_to_the_target_and_marks_the_event_read() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    let event = rig.voron_failed();
    let id = rig.consider(&event).unwrap();
    // A token for the id arrives first, then the click.
    rig.signal(|sink| sink.token(id, "activation-token-1"));
    rig.signal(|sink| sink.click(id, "default"));
    wait_until("navigation", || !rig.navigations.lock().unwrap().is_empty());
    assert_eq!(
        rig.navigations.lock().unwrap()[0],
        json!({
            "contractVersion": 1,
            "target": printer_target(PRINTER),
            "openAttentionCenter": false,
        })
    );
    // The window was presented with the token first.
    assert_eq!(
        rig.control.steps()[0],
        RaiseStep::Present {
            token: Some("activation-token-1".into())
        }
    );
    // Marked read, and published on the attention stream.
    let stored = rig.event(&event.id);
    assert!(stored.read_at.is_some());
    wait_until("the read to be published", || {
        rig.stream.lock().unwrap().iter().any(|published| {
            published["type"] == "attention.event.changed"
                && published["subject"]["id"] == event.id.as_str()
                && !published["payload"]["event"]["readAt"].is_null()
        })
    });
}

#[test]
fn without_focus_after_the_raise_the_window_is_remapped_then_flagged() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    let id = rig.consider(&rig.voron_failed()).unwrap();
    rig.signal(|sink| sink.click(id, "default"));
    wait_until("the fallbacks", || rig.control.steps().len() == 3);
    assert_eq!(
        rig.control.steps(),
        vec![
            RaiseStep::Present { token: None },
            RaiseStep::Remap,
            RaiseStep::RequestAttention
        ]
    );
}

#[test]
fn a_focus_gained_after_the_raise_stops_the_fallbacks() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    let id = rig.consider(&rig.voron_failed()).unwrap();
    rig.signal(|sink| sink.click(id, "Open"));
    wait_until("the raise", || !rig.control.steps().is_empty());
    rig.services.notifications.focus().set(true);
    std::thread::sleep(RAISE_WAIT * 4);
    assert_eq!(
        rig.control.steps(),
        vec![RaiseStep::Present { token: None }]
    );
}

#[test]
fn a_click_on_a_deleted_source_opens_the_event_itself() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    rig.storage
        .write(|tx| {
            tx.execute_batch(&format!(
                "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                   catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                   overrides_json, created_at, updated_at)
                 VALUES ('prn-gone', 1, 'Gone', '', '', '', '', '', '', '{{}}',
                         '{NOW_TEXT}', '{NOW_TEXT}');"
            ))?;
            Ok(())
        })
        .unwrap();
    let event = rig.insert_host_failed(
        "prn-gone",
        AttentionSubject {
            printer_name: Some("Gone".into()),
            printer_location: None,
            job_label: None,
            spool_number: None,
            spool_label: None,
        },
    );
    let id = rig.consider(&event).unwrap();
    rig.storage
        .write(|tx| {
            tx.execute("DELETE FROM printers WHERE id = 'prn-gone'", [])?;
            Ok(())
        })
        .unwrap();
    rig.signal(|sink| sink.click(id, "default"));
    wait_until("navigation", || !rig.navigations.lock().unwrap().is_empty());
    assert_eq!(
        rig.navigations.lock().unwrap()[0]["target"],
        json!({"version": 1, "destination": "monitor", "selection": {"kind": "attention", "id": event.id}})
    );
}

#[test]
fn unknown_ids_and_other_actions_are_ignored_and_a_closed_id_is_forgotten() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    let id = rig.consider(&rig.voron_failed()).unwrap();
    // Another client's notification, and an unknown action on ours.
    rig.signal(|sink| sink.click(id + 1000, "default"));
    rig.signal(|sink| sink.click(id, "inline-reply"));
    rig.signal(|sink| sink.token(id + 1000, "not-ours"));
    assert!(rig.navigations.lock().unwrap().is_empty());
    assert!(rig.control.steps().is_empty());
    // Closed: forgotten, and a later click does nothing.
    rig.signal(|sink| sink.close(id, 3));
    assert_eq!(rig.services.notifications.outstanding(id), None);
    rig.signal(|sink| sink.click(id, "default"));
    std::thread::sleep(Duration::from_millis(50));
    assert!(rig.navigations.lock().unwrap().is_empty());
}

#[test]
fn outstanding_notifications_are_bounded_at_256_oldest_first() {
    let rig = rig();
    let mut ids = Vec::new();
    for _ in 0..(OUTSTANDING_LIMIT + 4) {
        let id = tauri::async_runtime::block_on(rig.services.notifications.send_test())
            .expect("the test notification is shown");
        ids.push(id.id);
    }
    assert_eq!(
        rig.services.notifications.outstanding_len(),
        OUTSTANDING_LIMIT
    );
    for evicted in &ids[..4] {
        assert_eq!(rig.services.notifications.outstanding(*evicted), None);
    }
    assert!(rig.services.notifications.outstanding(ids[4]).is_some());
    assert!(rig
        .services
        .notifications
        .outstanding(*ids.last().unwrap())
        .is_some());
}

#[test]
fn a_burst_summary_is_replaced_in_place_and_its_click_opens_the_attention_center() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    let mut ids = Vec::new();
    for n in 0..5 {
        let printer = format!("prn-burst-{n}");
        rig.storage
            .write(|tx| {
                tx.execute_batch(&format!(
                    "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                       catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                       overrides_json, created_at, updated_at)
                     VALUES ('{printer}', 1, 'Burst {n}', '', '', '', '', '', '', '{{}}',
                             '{NOW_TEXT}', '{NOW_TEXT}');"
                ))?;
                Ok(())
            })
            .unwrap();
        let event = rig.insert_host_failed(
            &printer,
            AttentionSubject {
                printer_name: Some(format!("Burst {n}")),
                printer_location: None,
                job_label: None,
                spool_number: None,
                spool_label: None,
            },
        );
        ids.push(rig.consider(&event).unwrap());
    }
    // Three by themselves, then one summary updated in place.
    assert_eq!(ids[3], ids[4], "the summary keeps its id");
    let shown = rig.sink.shown();
    assert_eq!(shown.len(), 5);
    assert_eq!(shown[3].1.summary, "1 new Attention Event");
    assert_eq!(shown[4].1.summary, "2 new Attention Events");
    assert_eq!(shown[4].1.replaces_id, ids[3]);
    rig.signal(|sink| sink.click(ids[4], "default"));
    wait_until("navigation", || !rig.navigations.lock().unwrap().is_empty());
    assert_eq!(
        rig.navigations.lock().unwrap()[0],
        json!({"contractVersion": 1, "target": {"version": 1, "destination": "monitor"}, "openAttentionCenter": true})
    );
}

#[test]
fn a_candidate_from_the_projector_reaches_the_sink_but_a_lagged_receiver_replays_nothing() {
    let rig = rig();
    rig.services.notifications.focus().set(false);
    let event = rig.voron_failed();
    let changes = farm3d_lib::attention::projector::AppliedChanges {
        notify: vec![NotifyCandidate {
            event: event.clone(),
            change: EventChange::Inserted { recurred: false },
        }],
        ..Default::default()
    };
    let before = rig.services.notifications.candidates_handled();
    rig.services.attention.hand_on_for_test(changes);
    wait_until("the candidate", || {
        rig.services.notifications.candidates_handled() > before
    });
    assert_eq!(rig.sink.shown().len(), 1);

    // Overflow the hand-off while the service is held: the lagged
    // passes are dropped, never shown late.
    rig.services.notifications.hold();
    for n in 0..80 {
        let printer = format!("prn-lag-{n}");
        rig.storage
            .write(|tx| {
                tx.execute_batch(&format!(
                    "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model,
                       catalog_variant, catalog_model_id, catalog_printer_variant, notes,
                       overrides_json, created_at, updated_at)
                     VALUES ('{printer}', 1, 'Lag', '', '', '', '', '', '', '{{}}',
                             '{NOW_TEXT}', '{NOW_TEXT}');"
                ))?;
                Ok(())
            })
            .unwrap();
        let lagged = rig.insert_host_failed(
            &printer,
            AttentionSubject {
                printer_name: Some("Lag".into()),
                printer_location: None,
                job_label: None,
                spool_number: None,
                spool_label: None,
            },
        );
        rig.services
            .attention
            .hand_on_for_test(farm3d_lib::attention::projector::AppliedChanges {
                notify: vec![NotifyCandidate {
                    event: lagged,
                    change: EventChange::Inserted { recurred: false },
                }],
                ..Default::default()
            });
    }
    let before = rig.services.notifications.candidates_handled();
    rig.services.notifications.release();
    wait_until("the backlog", || {
        rig.services.notifications.candidates_handled() >= before + 64
    });
    std::thread::sleep(Duration::from_millis(50));
    assert!(rig.services.notifications.lagged() > 0);
    // At most the channel's capacity was seen at all.
    assert!(rig.services.notifications.candidates_handled() - before <= 64);
    assert!(
        rig.services
            .notifications
            .log_lines()
            .iter()
            .any(|line| line.contains("dropped") && !line.contains('/')),
        "{:?}",
        rig.services.notifications.log_lines()
    );
}

#[test]
fn an_unavailable_notifier_never_panics_and_the_commands_say_so() {
    let rig = rig_with(RecordingSink::with_status(NotifierStatus::Unavailable {
        reason: NotifierUnavailableReason::NoSessionBus,
    }));
    assert_eq!(
        rig.ok("notification_status", json!({})),
        json!({"state": "unavailable", "reason": "noSessionBus"})
    );
    let error = rig.call("send_test_notification", json!({})).unwrap_err();
    assert_eq!(error["code"], "NOTIFICATIONS_UNAVAILABLE");
    assert_eq!(
        error["details"]["status"],
        json!({"state": "unavailable", "reason": "noSessionBus"})
    );
    // A candidate while unavailable is dropped quietly.
    rig.services.notifications.focus().set(false);
    let event = rig.voron_failed();
    assert_eq!(rig.consider(&event), None);
    // The next status call retries: the server came back.
    rig.sink.set_status(RecordingSink::available().status_now());
    assert_eq!(
        rig.ok("notification_status", json!({}))["state"],
        "available"
    );
    // The failed show didn't use up the key's 10 minutes.
    assert!(rig.consider(&event).is_some());
    assert_eq!(rig.sink.shown().len(), 1);
}

#[test]
fn send_test_notification_shows_one_whatever_the_focus_and_classes() {
    let rig = rig();
    assert!(rig.services.notifications.focus().is_focused());
    let settings = rig.ok("load_settings", json!({}));
    rig.ok(
        "save_settings",
        json!({
            "expectedRevision": settings["revision"],
            "themeMode": "system",
            "monitorSection": "printerModel",
            "monitorDensity": "comfortable",
            "notifications": {
                "fatal": false, "confirmation": false, "completion": false,
                "reconciliation": false, "connectivity": false, "inventory": false,
            },
        }),
    );
    assert_eq!(
        rig.ok("send_test_notification", json!({})),
        json!({"sent": true})
    );
    let shown = rig.sink.shown();
    assert_eq!(shown.len(), 1);
    assert_eq!(shown[0].1.summary, "farm3d test notification");
    assert_eq!(
        serde_json::to_value(&shown[0].1.target).unwrap(),
        json!({"version": 1, "destination": "monitor"})
    );
    assert_eq!(
        rig.ok("notification_status", json!({}))["state"],
        "available"
    );
}

#[test]
fn the_null_sink_reports_unsupported() {
    use farm3d_lib::notifications::null::NullNotificationSink;
    use farm3d_lib::notifications::{NotificationSink, NotifyError};
    let sink = NullNotificationSink;
    assert_eq!(sink.status(), NotifierStatus::Unsupported);
    assert_eq!(
        tauri::async_runtime::block_on(sink.show(&Notification::test())),
        Err(NotifyError::Unsupported)
    );
    assert_eq!(
        tauri::async_runtime::block_on(sink.refresh()),
        NotifierStatus::Unsupported
    );
}

// --- Step 4: settings, alert defaults, and the seeded secret ------------------------

fn save(rig: &Rig, extra: Value) -> Result<Value, Value> {
    let current = rig.ok("load_settings", json!({}));
    let mut body = json!({
        "expectedRevision": current["revision"],
        "themeMode": current["themeMode"],
        "monitorSection": current["monitorSection"],
        "monitorDensity": current["monitorDensity"],
    });
    for (key, value) in extra.as_object().unwrap() {
        body[key] = value.clone();
    }
    rig.call("save_settings", body)
}

fn default_classes() -> Value {
    json!({
        "fatal": true, "confirmation": true, "completion": true,
        "reconciliation": false, "connectivity": false, "inventory": false,
    })
}

#[test]
fn settings_carry_the_class_and_retention_defaults_and_keep_them_when_absent() {
    let rig = rig();
    let loaded = rig.ok("load_settings", json!({}));
    assert_eq!(loaded["notifications"], default_classes());
    assert_eq!(
        loaded["snapshotRetention"],
        json!({"retentionDays": 30, "diskCapMb": 2048})
    );

    let classes = json!({
        "fatal": false, "confirmation": true, "completion": false,
        "reconciliation": true, "connectivity": true, "inventory": true,
    });
    let saved = save(
        &rig,
        json!({"notifications": classes, "snapshotRetention": {"retentionDays": 7, "diskCapMb": 512}}),
    )
    .unwrap();
    assert_eq!(saved["notifications"], classes);
    assert_eq!(
        saved["snapshotRetention"],
        json!({"retentionDays": 7, "diskCapMb": 512})
    );
    assert_eq!(saved["revision"], loaded["revision"].as_i64().unwrap() + 1);

    // The theme and Monitor callers send neither object: both are kept.
    let themed = save(&rig, json!({"themeMode": "farm3d-dark"})).unwrap();
    assert_eq!(themed["themeMode"], "farm3d-dark");
    assert_eq!(themed["notifications"], classes);
    assert_eq!(
        themed["snapshotRetention"],
        json!({"retentionDays": 7, "diskCapMb": 512})
    );
    assert_eq!(rig.ok("load_settings", json!({})), themed);
}

#[test]
fn a_stale_revision_conflicts_and_out_of_range_retention_is_a_field_validation() {
    let rig = rig();
    let loaded = rig.ok("load_settings", json!({}));
    save(&rig, json!({"notifications": default_classes()})).unwrap();
    let stale = rig
        .call(
            "save_settings",
            json!({
                "expectedRevision": loaded["revision"],
                "themeMode": "system",
                "monitorSection": "printerModel",
                "monitorDensity": "comfortable",
                "snapshotRetention": {"retentionDays": 9, "diskCapMb": 2048},
            }),
        )
        .unwrap_err();
    assert_eq!(stale["code"], "CONFLICT");

    for (retention, field) in [
        (
            json!({"retentionDays": 0, "diskCapMb": 2048}),
            "snapshotRetention.retentionDays",
        ),
        (
            json!({"retentionDays": 366, "diskCapMb": 2048}),
            "snapshotRetention.retentionDays",
        ),
        (
            json!({"retentionDays": 30, "diskCapMb": 99}),
            "snapshotRetention.diskCapMb",
        ),
        (
            json!({"retentionDays": 30, "diskCapMb": 102401}),
            "snapshotRetention.diskCapMb",
        ),
    ] {
        let error = save(&rig, json!({"snapshotRetention": retention})).unwrap_err();
        assert_eq!(error["code"], "VALIDATION", "{retention}");
        assert_eq!(error["details"]["fieldPath"], field, "{error}");
    }
    // The boundaries are accepted.
    save(
        &rig,
        json!({"snapshotRetention": {"retentionDays": 1, "diskCapMb": 100}}),
    )
    .unwrap();
    save(
        &rig,
        json!({"snapshotRetention": {"retentionDays": 365, "diskCapMb": 102400}}),
    )
    .unwrap();
}

#[test]
fn a_retention_change_by_save_or_import_pokes_the_media_janitor() {
    let rig = rig();
    let janitor = || rig.services.cameras.janitor().pokes();
    let before = janitor();
    // Only the classes: no poke.
    save(&rig, json!({"notifications": default_classes()})).unwrap();
    assert_eq!(janitor(), before);
    // The same retention again: no poke.
    save(
        &rig,
        json!({"snapshotRetention": {"retentionDays": 30, "diskCapMb": 2048}}),
    )
    .unwrap();
    assert_eq!(janitor(), before);
    // A change: one poke.
    save(
        &rig,
        json!({"snapshotRetention": {"retentionDays": 3, "diskCapMb": 2048}}),
    )
    .unwrap();
    assert_eq!(janitor(), before + 1);

    // import_settings: a v3 document with a new cap pokes.
    *rig.documents.bytes.lock().unwrap() = Some(
        serde_json::to_vec(&json!({
            "schemaVersion": 3,
            "exportedAt": NOW_TEXT,
            "settings": {
                "themeMode": "system",
                "snapshotRetention": {"retentionDays": 3, "diskCapMb": 4096},
            },
        }))
        .unwrap(),
    );
    let revision = rig.ok("load_settings", json!({}))["revision"].clone();
    let imported = rig.ok("import_settings", json!({"expectedRevision": revision}));
    assert_eq!(imported["status"], "applied");
    assert_eq!(
        imported["settings"]["snapshotRetention"],
        json!({"retentionDays": 3, "diskCapMb": 4096})
    );
    assert_eq!(janitor(), before + 2);
    // A v2 document resets retention to the defaults: a change, a poke.
    *rig.documents.bytes.lock().unwrap() = Some(
        serde_json::to_vec(&json!({
            "schemaVersion": 2,
            "exportedAt": NOW_TEXT,
            "settings": {"themeMode": "system", "monitorSection": "none", "monitorDensity": "compact"},
        }))
        .unwrap(),
    );
    let revision = rig.ok("load_settings", json!({}))["revision"].clone();
    let imported = rig.ok("import_settings", json!({"expectedRevision": revision}));
    assert_eq!(imported["settings"]["notifications"], default_classes());
    assert_eq!(
        imported["settings"]["snapshotRetention"],
        json!({"retentionDays": 30, "diskCapMb": 2048})
    );
    assert_eq!(imported["settings"]["monitorSection"], "none");
    assert_eq!(janitor(), before + 3);
}

#[test]
fn export_writes_schema_3_with_the_new_fields_and_it_imports_back() {
    let rig = rig();
    let classes = json!({
        "fatal": true, "confirmation": false, "completion": true,
        "reconciliation": true, "connectivity": false, "inventory": true,
    });
    save(
        &rig,
        json!({"notifications": classes, "snapshotRetention": {"retentionDays": 14, "diskCapMb": 1000}}),
    )
    .unwrap();
    assert_eq!(rig.ok("export_settings", json!({}))["status"], "exported");
    let written: Value =
        serde_json::from_slice(rig.documents.writes.lock().unwrap().last().unwrap()).unwrap();
    assert_eq!(written["schemaVersion"], 3);
    assert_eq!(written["settings"]["notifications"], classes);
    assert_eq!(
        written["settings"]["snapshotRetention"],
        json!({"retentionDays": 14, "diskCapMb": 1000})
    );

    // Reset, then import the exported document: every field comes back.
    save(
        &rig,
        json!({"notifications": default_classes(), "snapshotRetention": {"retentionDays": 30, "diskCapMb": 2048}}),
    )
    .unwrap();
    *rig.documents.bytes.lock().unwrap() = Some(serde_json::to_vec(&written).unwrap());
    let revision = rig.ok("load_settings", json!({}))["revision"].clone();
    let imported = rig.ok("import_settings", json!({"expectedRevision": revision}));
    assert_eq!(imported["settings"]["notifications"], classes);
    assert_eq!(
        imported["settings"]["snapshotRetention"],
        json!({"retentionDays": 14, "diskCapMb": 1000})
    );
    // A schema 4 document is from a newer farm3d.
    *rig.documents.bytes.lock().unwrap() = Some(
        serde_json::to_vec(&json!({"schemaVersion": 4, "exportedAt": NOW_TEXT, "settings": {"themeMode": "system"}}))
            .unwrap(),
    );
    let revision = rig.ok("load_settings", json!({}))["revision"].clone();
    let error = rig
        .call("import_settings", json!({"expectedRevision": revision}))
        .unwrap_err();
    assert_eq!(error["code"], "UNSUPPORTED_SCHEMA_VERSION");
}

fn alert_body(operation: &str, printer: &str, minutes: Value, mode: &str) -> Value {
    json!({
        "operationId": operation,
        "printerId": printer,
        "alertDefaults": {
            "offlineAfterMinutes": minutes, "notifications": mode,
            "snapshotOnIncident": true, "snapshotOnCompletion": false,
        },
    })
}

#[test]
fn alert_defaults_get_the_defaults_set_is_idempotent_and_a_change_pokes_the_projector() {
    let rig = rig();
    assert_eq!(
        rig.ok("get_printer_alert_defaults", json!({"printerId": PRINTER})),
        json!({
            "printerId": PRINTER, "revision": null, "updatedAt": null,
            "alertDefaults": {
                "offlineAfterMinutes": 5, "notifications": "follow",
                "snapshotOnIncident": true, "snapshotOnCompletion": true,
            },
        })
    );
    let pokes = || rig.services.attention.pokes();
    let before = pokes();
    let set = rig.ok(
        "set_printer_alert_defaults",
        alert_body("op-1", PRINTER, json!(15), "muted"),
    );
    assert_eq!(set["revision"], 1);
    assert_eq!(set["alertDefaults"]["offlineAfterMinutes"], 15);
    assert_eq!(
        pokes(),
        before + 1,
        "a new offline grace takes effect at once"
    );
    // A replay: the same row, no poke.
    assert_eq!(
        rig.ok(
            "set_printer_alert_defaults",
            alert_body("op-1", PRINTER, json!(15), "muted")
        ),
        set
    );
    assert_eq!(pokes(), before + 1);
    // The same values under a new id: a no-op, no revision bump, no poke.
    assert_eq!(
        rig.ok(
            "set_printer_alert_defaults",
            alert_body("op-2", PRINTER, json!(15), "muted")
        ),
        set
    );
    assert_eq!(pokes(), before + 1);
    // A reused id with another request.
    let reused = rig
        .call(
            "set_printer_alert_defaults",
            alert_body("op-1", PRINTER, json!(1), "muted"),
        )
        .unwrap_err();
    assert_eq!(reused["code"], "VALIDATION");
    assert_eq!(reused["details"]["fieldPath"], "operationId", "{reused}");
    // Off, then a missing Printer.
    let off = rig.ok(
        "set_printer_alert_defaults",
        alert_body("op-3", PRINTER, Value::Null, "follow"),
    );
    assert_eq!(off["revision"], 2);
    assert_eq!(off["alertDefaults"]["offlineAfterMinutes"], Value::Null);
    assert_eq!(
        rig.ok("get_printer_alert_defaults", json!({"printerId": PRINTER})),
        off
    );
    for command in ["get_printer_alert_defaults", "set_printer_alert_defaults"] {
        let error = rig
            .call(
                command,
                alert_body("op-4", "prn-missing", json!(5), "follow"),
            )
            .unwrap_err();
        assert_eq!(error["code"], "NOT_FOUND", "{command}");
    }
    // A bad grace is refused by the wire type.
    rig.call(
        "set_printer_alert_defaults",
        alert_body("op-5", PRINTER, json!(10), "follow"),
    )
    .unwrap_err();
}

/// Global constraint 3: the corpus in the real secret-bearing inputs — the
/// Printer's Connection host, its stored credential, and a manual camera
/// URL — never reaches a notification, the navigate event, a command
/// response, or a log line. The Printer has an ordinary name.
#[test]
fn the_seeded_secret_never_reaches_a_notification_navigation_response_or_log_line() {
    let rig = rig();
    const CREDENTIAL_REF: &str = "cred-p8n-notifications";
    rig.services
        .credentials
        .set(CREDENTIAL_REF, "s3cr3t-P8N")
        .unwrap();
    rig.storage
        .write(|tx| {
            tx.execute(
                "UPDATE printers SET location = 'Bay A', connection_json = ?1 WHERE id = ?2",
                rusqlite::params![
                    json!({
                        "kind": "moonraker",
                        "host": "192.0.2.10",
                        "port": 8080,
                        "useTls": false,
                        "credentialRef": CREDENTIAL_REF,
                    })
                    .to_string(),
                    PRINTER
                ],
            )?;
            tx.execute(
                "INSERT INTO printer_cameras(printer_id, source_kind, snapshot_url, updated_at)
                 VALUES (?1, 'snapshotUrl', ?2, ?3)",
                rusqlite::params![PRINTER, SECRET_URL, NOW_TEXT],
            )?;
            Ok(())
        })
        .unwrap();
    // The corpus really is in place (the scan below isn't vacuous).
    assert_eq!(
        rig.services.credentials.get(CREDENTIAL_REF).unwrap(),
        Some("s3cr3t-P8N".to_string())
    );

    // Insert -> show -> token -> click.
    rig.services.notifications.focus().set(false);
    let event = rig.voron_failed();
    let id = rig.consider(&event).expect("shown");
    rig.signal(|sink| sink.token(id, "activation-token"));
    rig.signal(|sink| sink.click(id, "default"));
    wait_until("navigation", || !rig.navigations.lock().unwrap().is_empty());
    wait_until("the read to be published", || {
        !rig.stream.lock().unwrap().is_empty()
    });

    let shown = rig.sink.shown();
    assert!(!shown.is_empty());
    for (_, notification) in shown {
        assert_no_corpus("summary", &notification.summary);
        assert_no_corpus("body", &notification.body);
        assert_no_corpus(
            "target",
            &serde_json::to_string(&notification.target).unwrap(),
        );
    }
    for navigation in rig.navigations.lock().unwrap().iter() {
        assert_no_corpus("navigate event", &navigation.to_string());
    }
    for published in rig.stream.lock().unwrap().iter() {
        assert_no_corpus("attention stream event", &published.to_string());
    }
    for response in [
        rig.ok("notification_status", json!({})),
        rig.ok("send_test_notification", json!({})),
        rig.ok("get_printer_alert_defaults", json!({"printerId": PRINTER})),
        rig.ok(
            "set_printer_alert_defaults",
            alert_body("op-secret", PRINTER, json!(5), "follow"),
        ),
        rig.ok("load_settings", json!({})),
    ] {
        assert_no_corpus("command response", &response.to_string());
    }
    // Failing shows and refused sends log; so does a lagged hand-off.
    rig.sink.set_status(NotifierStatus::Unavailable {
        reason: NotifierUnavailableReason::CallFailed,
    });
    let error = rig.call("send_test_notification", json!({})).unwrap_err();
    assert_no_corpus("command error", &error.to_string());
    rig.services.notifications.hold();
    for _ in 0..70 {
        rig.services
            .attention
            .hand_on_for_test(farm3d_lib::attention::projector::AppliedChanges {
                notify: vec![NotifyCandidate {
                    event: event.clone(),
                    change: EventChange::Inserted { recurred: false },
                }],
                ..Default::default()
            });
    }
    rig.services.notifications.release();
    wait_until("the lagged hand-off", || {
        rig.services.notifications.lagged() > 0
    });
    let lines = rig.services.notifications.log_lines();
    assert!(lines.len() >= 2, "{lines:?}");
    for line in lines {
        assert_no_corpus("log line", &line);
    }
}
