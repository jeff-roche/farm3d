//! P8 spec "Condition fixtures", copied as table-driven tests: one
//! `#[test]` per fixture id, named `fixture_<id>` with `-` as `_`. Each
//! builds a `FarmView` from the spec's baseline world plus the row's
//! changes, builds the prior Events, runs `observe` then `plan`, and
//! asserts the planned actions for the named Condition's key (the other
//! keys the view produces follow their own rows).
//!
//! Beyond the spec's tables:
//! - `fixture_x*` pin choices the spec leaves implicit (a durable-fact
//!   `Absent` beats the backfill's `Unknown`, and a few reach cells);
//! - `plan_is_a_fixed_point_for_every_fixture` applies `plan`'s output to
//!   an in-memory store and plans again over every catalogue and
//!   supplementary fixture, expecting only `Amend { changed: false }`.

use std::collections::HashMap;

use chrono::{DateTime, SecondsFormat, Utc};
use farm3d_lib::attention::lifecycle::{self, AckBy, LifecycleOp};
use farm3d_lib::attention::observe::{
    next_deadline, observe, FarmView, JobEndedBy, JobFacts, LatestJob, PrinterFacts, SpoolFacts,
    StatusFacts,
};
use farm3d_lib::attention::plan::{plan, PlannedAction};
use farm3d_lib::attention::{
    dedup_key, summary, AttentionAction, AttentionDetail, AttentionEvent, AttentionOrigin,
    AttentionResolution, AttentionSubject, ConditionKind, EvidenceOutcome,
    MaterialReconciliationStatus, PrinterConnectionErrorCause, ResolutionMode,
};
use farm3d_lib::connections::{
    ConnectionErrorCause, ConnectionState, PrinterStatus, PrinterStatusFacts,
};
use farm3d_lib::jobs::{
    CancelReason, JobState, ReconciliationRequirement, RequirementKind, RequirementStatus,
};
use farm3d_lib::printers::alerts::{AlertDefaults, OfflineAlertMinutes};
use farm3d_lib::printers::operational::OperationalState;
use farm3d_lib::printers::StartSafety;
use farm3d_lib::spools::SpoolLifecycle;

use AttentionResolution::{ActionCompleted, ConditionCleared};
use ConditionKind as K;

// ---------------------------------------------------------------------
// Baseline world
// ---------------------------------------------------------------------

fn t(hms: &str) -> DateTime<Utc> {
    format!("2026-09-27T{hms}Z").parse().unwrap()
}

fn text(at: DateTime<Utc>) -> String {
    at.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

fn now() -> DateTime<Utc> {
    t("12:00:00")
}

const PRN: &str = "prn-1";
const JOB: &str = "job-1";
const RRQ: &str = "rrq-1";
const SPL: &str = "spl-1";
const JOB_ENDED_AT: &str = "11:30:00";

fn status(
    connection: ConnectionState,
    cause: Option<ConnectionErrorCause>,
    operational: OperationalState,
    file: Option<&str>,
) -> StatusFacts {
    let mut status = PrinterStatus::new(connection);
    status.operational_state = operational;
    status.telemetry.job_name = file.map(str::to_string);
    StatusFacts::from_facts(&PrinterStatusFacts { status, cause })
}

fn printer(id: &str, name: &str) -> PrinterFacts {
    PrinterFacts {
        id: id.into(),
        name: name.into(),
        location: Some("Bay A".into()),
        archived: false,
        setup_incomplete: false,
        start_safety: StartSafety::ConfirmBedClear,
        status: Some(status(
            ConnectionState::Online,
            None,
            OperationalState::Ready,
            None,
        )),
        unreachable_since: None,
        alert_defaults: AlertDefaults::default(),
        active_job_id: None,
        latest_job: None,
    }
}

/// `NOW` 12:00, `STARTED` 11:00, `EPOCH` 10:00; `prn-1` online and
/// ready; `spl-1` active, 500 g, loaded on `prn-1`.
fn baseline() -> FarmView {
    FarmView {
        printers: vec![printer(PRN, "Voron")],
        jobs: vec![],
        requirements: vec![],
        spools: vec![SpoolFacts {
            id: SPL.into(),
            number: 12,
            label: "Polymaker PLA".into(),
            lifecycle: SpoolLifecycle::Active,
            low: false,
            current_mg: 500_000,
            low_threshold_mg: 100_000,
            loaded_on: Some(PRN.into()),
        }],
        supervisors_started_at: Some(t("11:00:00")),
        attention_epoch: t("10:00:00"),
    }
}

fn prn(view: &mut FarmView) -> &mut PrinterFacts {
    view.printers.iter_mut().find(|p| p.id == PRN).unwrap()
}

fn job(view: &mut FarmView) -> &mut JobFacts {
    view.jobs.iter_mut().find(|j| j.id == JOB).unwrap()
}

fn set_status(
    view: &mut FarmView,
    connection: ConnectionState,
    cause: Option<ConnectionErrorCause>,
    operational: OperationalState,
    file: Option<&str>,
) {
    prn(view).status = Some(status(connection, cause, operational, file));
}

/// `prn-1` `offline` with the watch's `unreachable_since`.
fn offline(view: &mut FarmView, since: &str) {
    set_status(
        view,
        ConnectionState::Offline,
        None,
        OperationalState::Offline,
        None,
    );
    prn(view).unreachable_since = Some(t(since));
}

/// Adds `job-1` on `prn-1` (`cube.gcode`, `spl-1`, "Cube — Plate 1"): the
/// Printer's latest Job, its active Job while non-terminal, and ended at
/// 11:30 when terminal.
fn with_job(view: &mut FarmView, state: JobState, ended_by: Option<JobEndedBy>) {
    let terminal = state.is_terminal();
    let started = matches!(
        state,
        JobState::Starting | JobState::Printing | JobState::Paused | JobState::OutcomeUnknown
    ) || matches!(ended_by, Some(JobEndedBy::Tracker | JobEndedBy::Declared));
    view.jobs.push(JobFacts {
        id: JOB.into(),
        printer_id: PRN.into(),
        state,
        cancel_reason: None,
        ended_by: if terminal { ended_by } else { None },
        ended_at: terminal.then(|| t(JOB_ENDED_AT)),
        host_path: Some("cube.gcode".into()),
        started,
        spool_id: SPL.into(),
        has_last_failure: false,
        label: "Cube — Plate 1".into(),
    });
    let printer = prn(view);
    printer.latest_job = Some(LatestJob {
        id: JOB.into(),
        host_path: Some("cube.gcode".into()),
    });
    printer.active_job_id = (!terminal).then(|| JOB.to_string());
}

fn with_requirement(view: &mut FarmView, kind: RequirementKind, status: RequirementStatus) {
    let material = kind == RequirementKind::MaterialReconciliation;
    view.requirements.push(ReconciliationRequirement {
        id: RRQ.into(),
        job_id: JOB.into(),
        kind,
        status,
        spool_id: material.then(|| SPL.to_string()),
        reservation_id: material.then(|| "rsv-1".to_string()),
        opened_at: text(t(JOB_ENDED_AT)),
        deferred_at: (status == RequirementStatus::Deferred).then(|| text(t("11:40:00"))),
        resolved_at: (status == RequirementStatus::Resolved).then(|| text(t("11:50:00"))),
        resolution: None,
    });
}

fn set_spool_mg(view: &mut FarmView, current_mg: i64) {
    let spool = &mut view.spools[0];
    spool.current_mg = current_mg;
    spool.low = spool.lifecycle == SpoolLifecycle::Active && current_mg <= spool.low_threshold_mg;
}

fn backfill(view: &mut FarmView) {
    view.supervisors_started_at = None;
    for printer in &mut view.printers {
        printer.status = None;
    }
}

// ---------------------------------------------------------------------
// Row worlds (the catalogue's "Facts" column)
// ---------------------------------------------------------------------

fn world(f: impl FnOnce(&mut FarmView)) -> FarmView {
    let mut view = baseline();
    f(&mut view);
    view
}

fn c1_p() -> FarmView {
    world(|v| offline(v, "11:50:00"))
}
fn c1_u() -> FarmView {
    world(|v| offline(v, "11:57:00"))
}
fn c2_p() -> FarmView {
    world(|v| {
        set_status(
            v,
            ConnectionState::Error,
            Some(ConnectionErrorCause::Auth),
            OperationalState::Error,
            None,
        )
    })
}
fn c2_u() -> FarmView {
    world(|v| prn(v).status = None)
}
fn c3_p() -> FarmView {
    world(|v| {
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Failed,
            None,
        )
    })
}
fn c3_u() -> FarmView {
    world(|v| offline(v, "11:59:00"))
}
fn c4_p() -> FarmView {
    world(|v| with_job(v, JobState::AwaitingStart, None))
}
fn c4_a() -> FarmView {
    world(|v| with_job(v, JobState::Starting, None))
}
fn c5_p() -> FarmView {
    world(|v| with_job(v, JobState::Failed, Some(JobEndedBy::Tracker)))
}
fn c5_a() -> FarmView {
    world(|v| with_job(v, JobState::Failed, Some(JobEndedBy::Declared)))
}
fn c6_p() -> FarmView {
    world(|v| {
        with_job(v, JobState::Cancelled, Some(JobEndedBy::Tracker));
        job(v).cancel_reason = Some(CancelReason::HostCancelled);
    })
}
fn c6_a() -> FarmView {
    world(|v| {
        with_job(v, JobState::Cancelled, Some(JobEndedBy::Tracker));
        job(v).cancel_reason = Some(CancelReason::CancelledByOperator);
    })
}
fn c7_p() -> FarmView {
    world(|v| {
        with_job(v, JobState::Failed, Some(JobEndedBy::Declared));
        with_requirement(
            v,
            RequirementKind::MaterialReconciliation,
            RequirementStatus::Pending,
        );
    })
}
fn c7_a() -> FarmView {
    world(|v| {
        with_job(v, JobState::Failed, Some(JobEndedBy::Declared));
        with_requirement(
            v,
            RequirementKind::MaterialReconciliation,
            RequirementStatus::Resolved,
        );
    })
}
fn c7_u() -> FarmView {
    world(|v| with_job(v, JobState::Failed, Some(JobEndedBy::Declared)))
}
fn c8_p() -> FarmView {
    world(|v| {
        with_job(v, JobState::OutcomeUnknown, None);
        with_requirement(
            v,
            RequirementKind::JobOutcomeUnknown,
            RequirementStatus::Pending,
        );
    })
}
fn c8_a() -> FarmView {
    world(|v| {
        with_job(v, JobState::Failed, Some(JobEndedBy::Declared));
        with_requirement(
            v,
            RequirementKind::JobOutcomeUnknown,
            RequirementStatus::Resolved,
        );
    })
}
fn c8_u() -> FarmView {
    world(|v| with_job(v, JobState::OutcomeUnknown, None))
}
fn c9_p() -> FarmView {
    world(|v| set_spool_mg(v, 80_000))
}
fn c9_u() -> FarmView {
    world(|v| v.spools.clear())
}
fn c10_p() -> FarmView {
    world(|v| {
        with_job(v, JobState::Completed, Some(JobEndedBy::Tracker));
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Finished,
            Some("cube.gcode"),
        );
    })
}
fn c10_a() -> FarmView {
    let mut v = c10_p();
    set_status(
        &mut v,
        ConnectionState::Online,
        None,
        OperationalState::Ready,
        Some("cube.gcode"),
    );
    v
}
fn c10_u() -> FarmView {
    let mut v = c10_p();
    offline(&mut v, "11:59:00");
    v
}

// ---------------------------------------------------------------------
// Keys, details, and priors
// ---------------------------------------------------------------------

fn source_of(kind: ConditionKind) -> &'static str {
    match kind {
        K::PrinterOffline | K::PrinterConnectionError | K::PrinterHostFailed => PRN,
        K::JobStartConfirmation | K::JobFailed | K::JobHostCancelled | K::JobCompleted => JOB,
        K::RequirementMaterialReconciliation | K::RequirementJobOutcomeUnknown => RRQ,
        K::SpoolLow => SPL,
    }
}

fn key(kind: ConditionKind) -> String {
    dedup_key(kind, source_of(kind))
}

/// The `detail` each Condition's `-p` row observes.
fn p_detail(kind: ConditionKind) -> AttentionDetail {
    match kind {
        K::PrinterOffline => AttentionDetail::PrinterOffline {
            unreachable_since: "2026-09-27T11:50:00Z".into(),
        },
        K::PrinterConnectionError => AttentionDetail::PrinterConnectionError {
            cause: PrinterConnectionErrorCause::Auth,
        },
        K::PrinterHostFailed => AttentionDetail::PrinterHostFailed,
        K::JobStartConfirmation => AttentionDetail::JobStartConfirmation {
            awaiting_material: false,
        },
        K::JobFailed => AttentionDetail::JobFailed {
            ended_at: "2026-09-27T11:30:00Z".into(),
        },
        K::JobHostCancelled => AttentionDetail::JobHostCancelled {
            ended_at: "2026-09-27T11:30:00Z".into(),
        },
        K::RequirementMaterialReconciliation => {
            AttentionDetail::RequirementMaterialReconciliation {
                requirement_status: MaterialReconciliationStatus::Pending,
                spool_id: SPL.into(),
            }
        }
        K::RequirementJobOutcomeUnknown => AttentionDetail::RequirementJobOutcomeUnknown,
        K::SpoolLow => AttentionDetail::SpoolLow {
            current_mg: 80_000,
            low_threshold_mg: 100_000,
        },
        K::JobCompleted => AttentionDetail::JobCompleted {
            ended_at: "2026-09-27T11:30:00Z".into(),
        },
    }
}

fn subject() -> AttentionSubject {
    AttentionSubject {
        printer_name: Some("Voron".into()),
        printer_location: Some("Bay A".into()),
        job_label: None,
        spool_number: None,
        spool_label: None,
    }
}

/// An Attention Event row for `kind`'s key with the catalogue's values:
/// open, unread, and unacknowledged.
fn event(id: &str, kind: ConditionKind, detail: AttentionDetail) -> AttentionEvent {
    let spec = kind.spec();
    let source_id = source_of(kind);
    let subject = subject();
    AttentionEvent {
        id: id.into(),
        revision: 1,
        dedup_key: dedup_key(kind, source_id),
        condition: kind,
        severity: spec.severity,
        requires_action: spec.requires_action,
        resolution_mode: spec.resolution_mode,
        notification_class: spec.notification_class,
        source: farm3d_lib::attention::AttentionSource {
            kind: spec.source_kind,
            id: source_id.into(),
        },
        printer_id: Some(PRN.into()),
        job_id: None,
        spool_id: None,
        requirement_id: None,
        incident_id: None,
        summary: summary(kind, &subject, &detail),
        subject,
        detail,
        origin: AttentionOrigin::Live,
        first_observed_at: text(t("11:55:00")),
        last_observed_at: text(t("11:55:00")),
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

/// `att-prev` (or another id): a resolved Event for `kind`'s key.
fn resolved_event(id: &str, kind: ConditionKind, resolved_at: &str) -> AttentionEvent {
    let mut e = event(id, kind, p_detail(kind));
    e.read_at = Some(text(t(resolved_at)));
    e.resolved_at = Some(text(t(resolved_at)));
    e.resolution = Some(match kind.spec().resolution_mode {
        ResolutionMode::Auto => ConditionCleared,
        ResolutionMode::Action => ActionCompleted,
        ResolutionMode::Manual => AttentionResolution::OperatorResolved,
    });
    e
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Prior {
    N,
    O,
    R,
}

/// The fixture's `N`/`O`/`R` prior for `kind`'s key. `O`'s detail is the
/// one the row's `P` observation produces.
fn prior(kind: ConditionKind, prior: Prior) -> Store {
    let mut store = Store::default();
    match prior {
        Prior::N => {}
        Prior::O => store.events.push(event("att-open", kind, p_detail(kind))),
        Prior::R => store
            .events
            .push(resolved_event("att-prev", kind, "11:45:00")),
    }
    store
}

// ---------------------------------------------------------------------
// An in-memory Attention store (the projector's apply, minus SQL)
// ---------------------------------------------------------------------

#[derive(Clone, Default, Debug)]
struct Store {
    events: Vec<AttentionEvent>,
    inserted: u32,
}

impl Store {
    fn open(&self) -> Vec<AttentionEvent> {
        self.events
            .iter()
            .filter(|e| e.resolved_at.is_none())
            .cloned()
            .collect()
    }

    /// The newest resolved Event id per key (ties: the later row).
    fn latest_resolved(&self) -> HashMap<String, String> {
        let mut newest: HashMap<String, &AttentionEvent> = HashMap::new();
        for e in self.events.iter().filter(|e| e.resolved_at.is_some()) {
            match newest.get(&e.dedup_key) {
                Some(seen) if seen.resolved_at > e.resolved_at => {}
                _ => {
                    newest.insert(e.dedup_key.clone(), e);
                }
            }
        }
        newest.into_iter().map(|(k, e)| (k, e.id.clone())).collect()
    }

    fn find(&mut self, id: &str) -> &mut AttentionEvent {
        self.events.iter_mut().find(|e| e.id == id).unwrap()
    }

    /// D2 "Apply", without Incidents: what each action writes.
    fn apply(&mut self, actions: &[PlannedAction], at: DateTime<Utc>) {
        for action in actions {
            match action {
                PlannedAction::Insert {
                    condition,
                    recurrence_of,
                    acknowledged,
                } => {
                    self.inserted += 1;
                    let spec = condition.spec();
                    let now = text(at);
                    self.events.push(AttentionEvent {
                        id: format!("att-new-{}", self.inserted),
                        revision: 1,
                        dedup_key: condition.dedup_key(),
                        condition: condition.kind,
                        severity: spec.severity,
                        requires_action: spec.requires_action,
                        resolution_mode: spec.resolution_mode,
                        notification_class: spec.notification_class,
                        source: condition.source(),
                        printer_id: condition.printer_id.clone(),
                        job_id: condition.job_id.clone(),
                        spool_id: condition.spool_id.clone(),
                        requirement_id: condition.requirement_id.clone(),
                        incident_id: None,
                        subject: condition.subject.clone(),
                        detail: condition.detail.clone(),
                        summary: condition.summary(),
                        origin: AttentionOrigin::Live,
                        first_observed_at: now.clone(),
                        last_observed_at: now.clone(),
                        observation_count: 1,
                        recurrence_of: recurrence_of.clone(),
                        read_at: acknowledged.then(|| now.clone()),
                        acknowledged_at: acknowledged.then(|| now.clone()),
                        resolved_at: None,
                        resolution: None,
                        notified_at: None,
                        evidence: None,
                        allowed_actions: vec![AttentionAction::MarkRead],
                    });
                }
                PlannedAction::Amend {
                    event_id,
                    detail,
                    severity,
                    summary,
                    changed,
                } => {
                    let e = self.find(event_id);
                    e.last_observed_at = text(at);
                    e.observation_count += 1;
                    if *changed {
                        e.detail = detail.clone();
                        e.severity = *severity;
                        e.summary = summary.clone();
                        e.revision += 1;
                    }
                }
                PlannedAction::Acknowledge { event_id } => {
                    let e = self.find(event_id);
                    let change =
                        lifecycle::apply(e, LifecycleOp::Acknowledge { by: AckBy::System }, at)
                            .unwrap();
                    assert!(
                        change.changed,
                        "a planned Acknowledge must change the Event"
                    );
                    e.read_at = change.read_at;
                    e.acknowledged_at = change.acknowledged_at;
                    e.revision += 1;
                }
                PlannedAction::Resolve {
                    event_id,
                    resolution,
                } => {
                    let e = self.find(event_id);
                    let change =
                        lifecycle::apply(e, LifecycleOp::Resolve(*resolution), at).unwrap();
                    assert!(change.changed, "a planned Resolve must change the Event");
                    e.read_at = change.read_at;
                    e.resolved_at = change.resolved_at;
                    e.resolution = change.resolution;
                    e.revision += 1;
                }
            }
        }
    }
}

// ---------------------------------------------------------------------
// Running and asserting
// ---------------------------------------------------------------------

fn run(view: &FarmView, store: &Store) -> Vec<PlannedAction> {
    plan(
        &store.open(),
        &store.latest_resolved(),
        &observe(view, now()),
    )
}

/// A planned action with the parts every fixture asserts. An `Insert`'s
/// catalogue values (severity, mode, class) come from its kind; its
/// subject and summary are asserted by `insert_carries_the_subject…`.
#[derive(Clone, PartialEq, Debug)]
enum Act {
    Insert {
        key: String,
        detail: AttentionDetail,
        recurrence_of: Option<String>,
        acknowledged: bool,
    },
    Amend {
        event_id: String,
        detail: AttentionDetail,
        changed: bool,
    },
    Acknowledge(String),
    Resolve(String, AttentionResolution),
}

/// The actions for `key` only, normalized to [`Act`]. Also checks every
/// Amend against D2: the catalogue severity, `changed` exactly when the
/// detail or severity differs, and a summary re-derived from the Event's
/// subject and the new detail.
fn actions_for(key: &str, actions: &[PlannedAction], store: &Store) -> Vec<Act> {
    let event_key = |id: &str| {
        store
            .events
            .iter()
            .find(|e| e.id == id)
            .map(|e| e.dedup_key.clone())
            .unwrap_or_default()
    };
    actions
        .iter()
        .filter_map(|action| match action {
            PlannedAction::Insert {
                condition,
                recurrence_of,
                acknowledged,
            } => (condition.dedup_key() == key).then(|| Act::Insert {
                key: condition.dedup_key(),
                detail: condition.detail.clone(),
                recurrence_of: recurrence_of.clone(),
                acknowledged: *acknowledged,
            }),
            PlannedAction::Amend {
                event_id,
                detail,
                severity,
                summary: amended_summary,
                changed,
            } => {
                if event_key(event_id) != key {
                    return None;
                }
                let e = store.events.iter().find(|e| &e.id == event_id).unwrap();
                assert_eq!(*severity, e.condition.spec().severity);
                assert_eq!(*changed, *detail != e.detail || *severity != e.severity);
                assert_eq!(*amended_summary, summary(e.condition, &e.subject, detail));
                Some(Act::Amend {
                    event_id: event_id.clone(),
                    detail: detail.clone(),
                    changed: *changed,
                })
            }
            PlannedAction::Acknowledge { event_id } => {
                (event_key(event_id) == key).then(|| Act::Acknowledge(event_id.clone()))
            }
            PlannedAction::Resolve {
                event_id,
                resolution,
            } => (event_key(event_id) == key).then(|| Act::Resolve(event_id.clone(), *resolution)),
        })
        .collect()
}

fn assert_actions(
    id: &str,
    view: &FarmView,
    store: &Store,
    kind: ConditionKind,
    expected: Vec<Act>,
) {
    let actions = run(view, store);
    assert_eq!(
        actions_for(&key(kind), &actions, store),
        expected,
        "fixture {id}: {} (all actions: {actions:#?})",
        key(kind)
    );
}

fn insert(kind: ConditionKind, detail: AttentionDetail) -> Act {
    Act::Insert {
        key: key(kind),
        detail,
        recurrence_of: None,
        acknowledged: false,
    }
}

fn insert_again(kind: ConditionKind, detail: AttentionDetail) -> Act {
    Act::Insert {
        key: key(kind),
        detail,
        recurrence_of: Some("att-prev".into()),
        acknowledged: false,
    }
}

fn amend_eq(kind: ConditionKind) -> Act {
    Act::Amend {
        event_id: "att-open".into(),
        detail: p_detail(kind),
        changed: false,
    }
}

fn resolve(resolution: AttentionResolution) -> Act {
    Act::Resolve("att-open".into(), resolution)
}

/// Applies `plan` to `store`, plans again, and expects only
/// `Amend { changed: false }` (D2 "Fixed point").
fn assert_fixed_point(id: &str, view: &FarmView, mut store: Store) {
    let first = run(view, &store);
    store.apply(&first, now());
    let second = run(view, &store);
    assert!(
        second
            .iter()
            .all(|a| matches!(a, PlannedAction::Amend { changed: false, .. })),
        "fixture {id}: plan is not a fixed point; second pass: {second:#?}"
    );
    let open_keys: Vec<_> = store.open().into_iter().map(|e| e.id).collect();
    let amended: Vec<_> = second
        .iter()
        .filter_map(|a| match a {
            PlannedAction::Amend { event_id, .. } => Some(event_id.clone()),
            _ => None,
        })
        .collect();
    for amended_id in &amended {
        assert!(
            open_keys.contains(amended_id),
            "fixture {id}: amended a closed Event"
        );
    }
    store.apply(&second, now());
    let third = run(view, &store);
    assert_eq!(
        third, second,
        "fixture {id}: the second and third passes differ"
    );
}

// ---------------------------------------------------------------------
// Catalogue fixtures (30 observations × 3 priors = 90)
// ---------------------------------------------------------------------

#[derive(Clone, Copy, Debug)]
enum Cell {
    Nothing,
    Insert,
    InsertAgain,
    AmendEq,
    Resolve(AttentionResolution),
}

struct CatalogueRow {
    id: &'static str,
    kind: ConditionKind,
    world: fn() -> FarmView,
    /// `N`, `O`, `R`.
    cells: [Cell; 3],
}

fn catalogue() -> Vec<CatalogueRow> {
    use Cell::*;
    let row = |id, kind, world, cells| CatalogueRow {
        id,
        kind,
        world,
        cells,
    };
    let p = [Insert, AmendEq, InsertAgain];
    let once = [Insert, AmendEq, Nothing];
    let unknown = [Nothing, Nothing, Nothing];
    vec![
        row("c1-p", K::PrinterOffline, c1_p as fn() -> FarmView, p),
        row(
            "c1-a",
            K::PrinterOffline,
            baseline,
            [Nothing, Resolve(ConditionCleared), Nothing],
        ),
        row("c1-u", K::PrinterOffline, c1_u, unknown),
        row("c2-p", K::PrinterConnectionError, c2_p, p),
        row(
            "c2-a",
            K::PrinterConnectionError,
            baseline,
            [Nothing, Resolve(ConditionCleared), Nothing],
        ),
        row("c2-u", K::PrinterConnectionError, c2_u, unknown),
        row("c3-p", K::PrinterHostFailed, c3_p, p),
        row(
            "c3-a",
            K::PrinterHostFailed,
            baseline,
            [Nothing, Resolve(ConditionCleared), Nothing],
        ),
        row("c3-u", K::PrinterHostFailed, c3_u, unknown),
        row("c4-p", K::JobStartConfirmation, c4_p, p),
        row(
            "c4-a",
            K::JobStartConfirmation,
            c4_a,
            [Nothing, Resolve(ActionCompleted), Nothing],
        ),
        row("c4-u", K::JobStartConfirmation, baseline, unknown),
        row("c5-p", K::JobFailed, c5_p, once),
        row("c5-a", K::JobFailed, c5_a, unknown),
        row("c5-u", K::JobFailed, baseline, unknown),
        row("c6-p", K::JobHostCancelled, c6_p, once),
        row("c6-a", K::JobHostCancelled, c6_a, unknown),
        row("c6-u", K::JobHostCancelled, baseline, unknown),
        row("c7-p", K::RequirementMaterialReconciliation, c7_p, once),
        row(
            "c7-a",
            K::RequirementMaterialReconciliation,
            c7_a,
            [Nothing, Resolve(ActionCompleted), Nothing],
        ),
        row("c7-u", K::RequirementMaterialReconciliation, c7_u, unknown),
        row("c8-p", K::RequirementJobOutcomeUnknown, c8_p, once),
        row(
            "c8-a",
            K::RequirementJobOutcomeUnknown,
            c8_a,
            [Nothing, Resolve(ActionCompleted), Nothing],
        ),
        row("c8-u", K::RequirementJobOutcomeUnknown, c8_u, unknown),
        row("c9-p", K::SpoolLow, c9_p, p),
        row(
            "c9-a",
            K::SpoolLow,
            baseline,
            [Nothing, Resolve(ConditionCleared), Nothing],
        ),
        row("c9-u", K::SpoolLow, c9_u, unknown),
        row("c10-p", K::JobCompleted, c10_p, once),
        row(
            "c10-a",
            K::JobCompleted,
            c10_a,
            [Nothing, Resolve(ConditionCleared), Nothing],
        ),
        row("c10-u", K::JobCompleted, c10_u, unknown),
    ]
}

fn catalogue_row(id: &str) -> CatalogueRow {
    catalogue().into_iter().find(|r| r.id == id).unwrap()
}

fn expected(kind: ConditionKind, cell: Cell) -> Vec<Act> {
    match cell {
        Cell::Nothing => vec![],
        Cell::Insert => vec![insert(kind, p_detail(kind))],
        Cell::InsertAgain => vec![insert_again(kind, p_detail(kind))],
        Cell::AmendEq => vec![amend_eq(kind)],
        Cell::Resolve(resolution) => vec![resolve(resolution)],
    }
}

fn run_catalogue(id: &str, which: Prior) {
    let row = catalogue_row(id);
    let cell = row.cells[which as usize];
    let fixture = format!("{id}-{}", format!("{which:?}").to_lowercase());
    assert_actions(
        &fixture,
        &(row.world)(),
        &prior(row.kind, which),
        row.kind,
        expected(row.kind, cell),
    );
}

macro_rules! catalogue_fixtures {
    ($($name:ident => $id:literal, $prior:ident;)*) => {
        $(
            #[test]
            fn $name() {
                run_catalogue($id, Prior::$prior);
            }
        )*
    };
}

catalogue_fixtures! {
    fixture_c1_p_n => "c1-p", N; fixture_c1_p_o => "c1-p", O; fixture_c1_p_r => "c1-p", R;
    fixture_c1_a_n => "c1-a", N; fixture_c1_a_o => "c1-a", O; fixture_c1_a_r => "c1-a", R;
    fixture_c1_u_n => "c1-u", N; fixture_c1_u_o => "c1-u", O; fixture_c1_u_r => "c1-u", R;
    fixture_c2_p_n => "c2-p", N; fixture_c2_p_o => "c2-p", O; fixture_c2_p_r => "c2-p", R;
    fixture_c2_a_n => "c2-a", N; fixture_c2_a_o => "c2-a", O; fixture_c2_a_r => "c2-a", R;
    fixture_c2_u_n => "c2-u", N; fixture_c2_u_o => "c2-u", O; fixture_c2_u_r => "c2-u", R;
    fixture_c3_p_n => "c3-p", N; fixture_c3_p_o => "c3-p", O; fixture_c3_p_r => "c3-p", R;
    fixture_c3_a_n => "c3-a", N; fixture_c3_a_o => "c3-a", O; fixture_c3_a_r => "c3-a", R;
    fixture_c3_u_n => "c3-u", N; fixture_c3_u_o => "c3-u", O; fixture_c3_u_r => "c3-u", R;
    fixture_c4_p_n => "c4-p", N; fixture_c4_p_o => "c4-p", O; fixture_c4_p_r => "c4-p", R;
    fixture_c4_a_n => "c4-a", N; fixture_c4_a_o => "c4-a", O; fixture_c4_a_r => "c4-a", R;
    fixture_c4_u_n => "c4-u", N; fixture_c4_u_o => "c4-u", O; fixture_c4_u_r => "c4-u", R;
    fixture_c5_p_n => "c5-p", N; fixture_c5_p_o => "c5-p", O; fixture_c5_p_r => "c5-p", R;
    fixture_c5_a_n => "c5-a", N; fixture_c5_a_o => "c5-a", O; fixture_c5_a_r => "c5-a", R;
    fixture_c5_u_n => "c5-u", N; fixture_c5_u_o => "c5-u", O; fixture_c5_u_r => "c5-u", R;
    fixture_c6_p_n => "c6-p", N; fixture_c6_p_o => "c6-p", O; fixture_c6_p_r => "c6-p", R;
    fixture_c6_a_n => "c6-a", N; fixture_c6_a_o => "c6-a", O; fixture_c6_a_r => "c6-a", R;
    fixture_c6_u_n => "c6-u", N; fixture_c6_u_o => "c6-u", O; fixture_c6_u_r => "c6-u", R;
    fixture_c7_p_n => "c7-p", N; fixture_c7_p_o => "c7-p", O; fixture_c7_p_r => "c7-p", R;
    fixture_c7_a_n => "c7-a", N; fixture_c7_a_o => "c7-a", O; fixture_c7_a_r => "c7-a", R;
    fixture_c7_u_n => "c7-u", N; fixture_c7_u_o => "c7-u", O; fixture_c7_u_r => "c7-u", R;
    fixture_c8_p_n => "c8-p", N; fixture_c8_p_o => "c8-p", O; fixture_c8_p_r => "c8-p", R;
    fixture_c8_a_n => "c8-a", N; fixture_c8_a_o => "c8-a", O; fixture_c8_a_r => "c8-a", R;
    fixture_c8_u_n => "c8-u", N; fixture_c8_u_o => "c8-u", O; fixture_c8_u_r => "c8-u", R;
    fixture_c9_p_n => "c9-p", N; fixture_c9_p_o => "c9-p", O; fixture_c9_p_r => "c9-p", R;
    fixture_c9_a_n => "c9-a", N; fixture_c9_a_o => "c9-a", O; fixture_c9_a_r => "c9-a", R;
    fixture_c9_u_n => "c9-u", N; fixture_c9_u_o => "c9-u", O; fixture_c9_u_r => "c9-u", R;
    fixture_c10_p_n => "c10-p", N; fixture_c10_p_o => "c10-p", O; fixture_c10_p_r => "c10-p", R;
    fixture_c10_a_n => "c10-a", N; fixture_c10_a_o => "c10-a", O; fixture_c10_a_r => "c10-a", R;
    fixture_c10_u_n => "c10-u", N; fixture_c10_u_o => "c10-u", O; fixture_c10_u_r => "c10-u", R;
}

#[test]
fn the_catalogue_table_has_thirty_rows_covering_every_condition_three_ways() {
    let rows = catalogue();
    assert_eq!(rows.len(), 30);
    for kind in ConditionKind::ALL {
        let ids: Vec<_> = rows
            .iter()
            .filter(|r| r.kind == kind)
            .map(|r| &r.id[r.id.len() - 1..])
            .collect();
        assert_eq!(ids, ["p", "a", "u"], "{kind:?}");
    }
}

// ---------------------------------------------------------------------
// Supplementary fixtures
// ---------------------------------------------------------------------

/// One supplementary world with one prior (a row with `O` / `N` is two
/// cases).
struct Case {
    id: &'static str,
    view: FarmView,
    store: Store,
}

fn case(id: &'static str, view: FarmView, store: Store) -> Case {
    Case { id, view, store }
}

fn s1() -> Case {
    let mut v = world(|v| offline(v, "11:50:00"));
    v.supervisors_started_at = Some(t("11:58:00"));
    case("s1", v, prior(K::PrinterOffline, Prior::N))
}
fn s2() -> Case {
    let mut v = baseline();
    backfill(&mut v);
    case("s2", v, prior(K::PrinterOffline, Prior::O))
}
fn s3() -> Case {
    let v = world(|v| {
        set_status(
            v,
            ConnectionState::Error,
            Some(ConnectionErrorCause::Timeout),
            OperationalState::Error,
            None,
        );
        prn(v).unreachable_since = Some(t("11:50:00"));
    });
    case("s3", v, Store::default())
}
fn s4() -> Case {
    let v = world(|v| {
        offline(v, "11:00:00");
        prn(v).archived = true;
    });
    case("s4", v, prior(K::PrinterOffline, Prior::O))
}
fn s5() -> Case {
    let v = world(|v| prn(v).setup_incomplete = true);
    case("s5", v, prior(K::PrinterConnectionError, Prior::O))
}
fn s6_world() -> FarmView {
    world(|v| {
        offline(v, "11:00:00");
        prn(v).alert_defaults.offline_after_minutes = None;
    })
}
fn s6_o() -> Case {
    case("s6-o", s6_world(), prior(K::PrinterOffline, Prior::O))
}
fn s6_n() -> Case {
    case("s6-n", s6_world(), prior(K::PrinterOffline, Prior::N))
}
fn s7_world() -> FarmView {
    world(|v| {
        with_job(v, JobState::Printing, None);
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Failed,
            Some("cube.gcode"),
        );
    })
}
fn s7_n() -> Case {
    case("s7-n", s7_world(), prior(K::PrinterHostFailed, Prior::N))
}
fn s7_o() -> Case {
    case("s7-o", s7_world(), prior(K::PrinterHostFailed, Prior::O))
}
fn s8() -> Case {
    let v = world(|v| {
        with_job(v, JobState::Failed, Some(JobEndedBy::Tracker));
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Failed,
            Some("cube.gcode"),
        );
    });
    case("s8", v, Store::default())
}
fn deferred_material() -> FarmView {
    world(|v| {
        with_job(v, JobState::Failed, Some(JobEndedBy::Declared));
        with_requirement(
            v,
            RequirementKind::MaterialReconciliation,
            RequirementStatus::Deferred,
        );
    })
}
fn deferred_detail() -> AttentionDetail {
    AttentionDetail::RequirementMaterialReconciliation {
        requirement_status: MaterialReconciliationStatus::Deferred,
        spool_id: SPL.into(),
    }
}
fn s9() -> Case {
    case(
        "s9",
        deferred_material(),
        prior(K::RequirementMaterialReconciliation, Prior::N),
    )
}
fn s10() -> Case {
    case(
        "s10",
        deferred_material(),
        prior(K::RequirementMaterialReconciliation, Prior::O),
    )
}
fn s11() -> Case {
    let mut store = Store::default();
    let mut e = event(
        "att-open",
        K::RequirementMaterialReconciliation,
        deferred_detail(),
    );
    e.read_at = Some(text(t("11:41:00")));
    e.acknowledged_at = Some(text(t("11:41:00")));
    store.events.push(e);
    case("s11", deferred_material(), store)
}
fn s12() -> Case {
    let mut store = Store::default();
    store.events.push(event(
        "att-open",
        K::SpoolLow,
        AttentionDetail::SpoolLow {
            current_mg: 90_000,
            low_threshold_mg: 100_000,
        },
    ));
    case("s12", world(|v| set_spool_mg(v, 80_000)), store)
}
fn s13() -> Case {
    let v = world(|v| {
        with_job(v, JobState::Failed, Some(JobEndedBy::Tracker));
        job(v).ended_at = Some(t("09:00:00"));
    });
    case("s13", v, prior(K::JobFailed, Prior::N))
}
fn s14_world() -> FarmView {
    world(|v| {
        with_job(v, JobState::AwaitingStart, None);
        prn(v).start_safety = StartSafety::Unattended;
    })
}
fn s14_n() -> Case {
    case(
        "s14-n",
        s14_world(),
        prior(K::JobStartConfirmation, Prior::N),
    )
}
fn s14_o() -> Case {
    case(
        "s14-o",
        s14_world(),
        prior(K::JobStartConfirmation, Prior::O),
    )
}
fn s15() -> Case {
    let mut v = s14_world();
    job(&mut v).has_last_failure = true;
    case("s15", v, prior(K::JobStartConfirmation, Prior::N))
}
fn s16_world() -> FarmView {
    world(|v| {
        with_job(v, JobState::AwaitingStart, None);
        v.spools[0].loaded_on = None;
    })
}
fn s16_n() -> Case {
    case(
        "s16-n",
        s16_world(),
        prior(K::JobStartConfirmation, Prior::N),
    )
}
fn s16_o() -> Case {
    case(
        "s16-o",
        s16_world(),
        prior(K::JobStartConfirmation, Prior::O),
    )
}
fn s17_world() -> FarmView {
    let mut v = c10_p();
    v.jobs.push(JobFacts {
        id: "job-2".into(),
        printer_id: PRN.into(),
        state: JobState::Assigned,
        cancel_reason: None,
        ended_by: None,
        ended_at: None,
        host_path: None,
        started: false,
        spool_id: SPL.into(),
        has_last_failure: false,
        label: "Cube — Plate 2".into(),
    });
    let printer = prn(&mut v);
    printer.latest_job = Some(LatestJob {
        id: "job-2".into(),
        host_path: None,
    });
    printer.active_job_id = Some("job-2".into());
    v
}
fn s17_n() -> Case {
    case("s17-n", s17_world(), prior(K::JobCompleted, Prior::N))
}
fn s17_o() -> Case {
    case("s17-o", s17_world(), prior(K::JobCompleted, Prior::O))
}
fn s18() -> Case {
    let mut v = c10_p();
    job(&mut v).ended_by = Some(JobEndedBy::Declared);
    case("s18", v, prior(K::JobCompleted, Prior::N))
}
fn s19() -> Case {
    let mut v = c10_p();
    prn(&mut v).archived = true;
    case("s19", v, prior(K::JobCompleted, Prior::O))
}
fn s20() -> Case {
    let v = world(|v| {
        set_status(
            v,
            ConnectionState::Error,
            None,
            OperationalState::Error,
            None,
        )
    });
    case("s20", v, prior(K::PrinterConnectionError, Prior::O))
}
fn s21() -> Case {
    let mut store = Store::default();
    let mut e = event("att-open", K::PrinterOffline, p_detail(K::PrinterOffline));
    e.dedup_key = dedup_key(K::PrinterOffline, "prn-9");
    e.source.id = "prn-9".into();
    e.printer_id = Some("prn-9".into());
    store.events.push(e);
    case("s21", baseline(), store)
}
fn s22() -> Case {
    case("s22", c1_p(), Store::default())
}
fn s23() -> Case {
    let v = world(|v| {
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Unknown,
            None,
        )
    });
    case("s23", v, prior(K::PrinterHostFailed, Prior::O))
}
fn s24() -> Case {
    let mut store = Store::default();
    // Listed newest-last so a key-order or first-seen shortcut would pick
    // the wrong one.
    store.events.push(resolved_event(
        "att-prev",
        K::JobStartConfirmation,
        "11:45:00",
    ));
    store.events.push(resolved_event(
        "att-older",
        K::JobStartConfirmation,
        "11:10:00",
    ));
    case("s24", c4_p(), store)
}
fn auth_prior() -> Store {
    // `O` for `printer.connectionError` (cause `auth`): `p_detail`'s.
    prior(K::PrinterConnectionError, Prior::O)
}
fn s25() -> Case {
    let v = world(|v| {
        set_status(
            v,
            ConnectionState::Connecting,
            None,
            OperationalState::Connecting,
            None,
        );
        prn(v).unreachable_since = Some(t("11:59:30"));
    });
    case("s25", v, auth_prior())
}
fn s26() -> Case {
    let mut v = baseline();
    // The hydrated startup status: `offline`, before any live observation.
    set_status(
        &mut v,
        ConnectionState::Offline,
        None,
        OperationalState::Offline,
        None,
    );
    prn(&mut v).unreachable_since = Some(t("11:59:59"));
    v.supervisors_started_at = Some(t("11:59:59"));
    case("s26", v, auth_prior())
}
fn s27() -> Case {
    let mut store = prior(K::JobCompleted, Prior::O);
    store.events[0].evidence = Some(EvidenceOutcome::Captured {
        snapshot_id: "snp-1".into(),
    });
    case("s27", c10_p(), store)
}
fn s28() -> Case {
    let v = world(|v| {
        with_job(v, JobState::Assigned, None);
        job(v).host_path = None;
        prn(v).latest_job.as_mut().unwrap().host_path = None;
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Failed,
            Some("other.gcode"),
        );
    });
    case("s28", v, prior(K::PrinterHostFailed, Prior::N))
}
fn s29() -> Case {
    let v = world(|v| {
        with_job(v, JobState::AwaitingStart, None);
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Failed,
            Some("cube.gcode"),
        );
    });
    case("s29", v, prior(K::PrinterHostFailed, Prior::N))
}

fn supplementary() -> Vec<Case> {
    vec![
        s1(),
        s2(),
        s3(),
        s4(),
        s5(),
        s6_o(),
        s6_n(),
        s7_n(),
        s7_o(),
        s8(),
        s9(),
        s10(),
        s11(),
        s12(),
        s13(),
        s14_n(),
        s14_o(),
        s15(),
        s16_n(),
        s16_o(),
        s17_n(),
        s17_o(),
        s18(),
        s19(),
        s20(),
        s21(),
        s22(),
        s23(),
        s24(),
        s25(),
        s26(),
        s27(),
        s28(),
        s29(),
    ]
}

fn check(c: Case, kind: ConditionKind, expected: Vec<Act>) {
    assert_actions(c.id, &c.view, &c.store, kind, expected);
}

#[test]
fn fixture_s1() {
    // max(11:50, 11:58) + 5 min = 12:03 > NOW.
    check(s1(), K::PrinterOffline, vec![]);
}

#[test]
fn fixture_s2() {
    check(s2(), K::PrinterOffline, vec![]);
}

#[test]
fn fixture_s3() {
    check(
        s3(),
        K::PrinterOffline,
        vec![insert(K::PrinterOffline, p_detail(K::PrinterOffline))],
    );
    check(s3(), K::PrinterConnectionError, vec![]);
}

#[test]
fn fixture_s4() {
    check(s4(), K::PrinterOffline, vec![resolve(ConditionCleared)]);
}

#[test]
fn fixture_s5() {
    check(
        s5(),
        K::PrinterConnectionError,
        vec![resolve(ConditionCleared)],
    );
}

#[test]
fn fixture_s6() {
    check(s6_o(), K::PrinterOffline, vec![resolve(ConditionCleared)]);
    check(s6_n(), K::PrinterOffline, vec![]);
}

#[test]
fn fixture_s7() {
    check(s7_n(), K::PrinterHostFailed, vec![]);
    check(
        s7_o(),
        K::PrinterHostFailed,
        vec![resolve(ConditionCleared)],
    );
}

#[test]
fn fixture_s8() {
    check(s8(), K::PrinterHostFailed, vec![]);
    check(
        s8(),
        K::JobFailed,
        vec![insert(K::JobFailed, p_detail(K::JobFailed))],
    );
}

#[test]
fn fixture_s9() {
    check(
        s9(),
        K::RequirementMaterialReconciliation,
        vec![Act::Insert {
            key: key(K::RequirementMaterialReconciliation),
            detail: deferred_detail(),
            recurrence_of: None,
            acknowledged: true,
        }],
    );
}

#[test]
fn fixture_s10() {
    check(
        s10(),
        K::RequirementMaterialReconciliation,
        vec![
            Act::Amend {
                event_id: "att-open".into(),
                detail: deferred_detail(),
                changed: true,
            },
            Act::Acknowledge("att-open".into()),
        ],
    );
}

#[test]
fn fixture_s11() {
    check(
        s11(),
        K::RequirementMaterialReconciliation,
        vec![Act::Amend {
            event_id: "att-open".into(),
            detail: deferred_detail(),
            changed: false,
        }],
    );
}

#[test]
fn fixture_s12() {
    check(
        s12(),
        K::SpoolLow,
        vec![Act::Amend {
            event_id: "att-open".into(),
            detail: p_detail(K::SpoolLow),
            changed: true,
        }],
    );
}

#[test]
fn fixture_s12_rederives_the_summary_from_the_new_detail() {
    let c = s12();
    let mut store = c.store.clone();
    // Give the prior Event the Spool's real subject, as its insert did.
    store.events[0].subject.spool_number = Some(12);
    store.events[0].summary = summary(
        K::SpoolLow,
        &store.events[0].subject,
        &store.events[0].detail,
    );
    assert_eq!(store.events[0].summary, "Spool #12 is low (90 g left).");
    let actions = run(&c.view, &store);
    store.apply(&actions, now());
    assert_eq!(store.events[0].summary, "Spool #12 is low (80 g left).");
}

#[test]
fn fixture_s13() {
    check(s13(), K::JobFailed, vec![]);
}

#[test]
fn fixture_s14() {
    check(s14_n(), K::JobStartConfirmation, vec![]);
    check(
        s14_o(),
        K::JobStartConfirmation,
        vec![resolve(ActionCompleted)],
    );
}

#[test]
fn fixture_s15() {
    check(
        s15(),
        K::JobStartConfirmation,
        vec![insert(
            K::JobStartConfirmation,
            p_detail(K::JobStartConfirmation),
        )],
    );
}

#[test]
fn fixture_s16() {
    let awaiting = AttentionDetail::JobStartConfirmation {
        awaiting_material: true,
    };
    check(
        s16_n(),
        K::JobStartConfirmation,
        vec![insert(K::JobStartConfirmation, awaiting.clone())],
    );
    check(
        s16_o(),
        K::JobStartConfirmation,
        vec![Act::Amend {
            event_id: "att-open".into(),
            detail: awaiting,
            changed: true,
        }],
    );
}

#[test]
fn fixture_s17() {
    check(s17_n(), K::JobCompleted, vec![]);
    check(s17_o(), K::JobCompleted, vec![resolve(ConditionCleared)]);
}

#[test]
fn fixture_s18() {
    check(s18(), K::JobCompleted, vec![]);
}

#[test]
fn fixture_s19() {
    check(s19(), K::JobCompleted, vec![resolve(ConditionCleared)]);
}

#[test]
fn fixture_s20() {
    check(s20(), K::PrinterConnectionError, vec![]);
}

#[test]
fn fixture_s21() {
    let c = s21();
    let actions = run(&c.view, &c.store);
    assert!(
        actions.iter().all(|a| match a {
            PlannedAction::Insert { .. } => true,
            PlannedAction::Amend { event_id, .. }
            | PlannedAction::Acknowledge { event_id }
            | PlannedAction::Resolve { event_id, .. } => event_id != "att-open",
        }),
        "fixture s21: an Event whose source isn't in the view must get no action: {actions:#?}"
    );
}

#[test]
fn fixture_s22() {
    let c = s22();
    let mut store = c.store.clone();
    let first = run(&c.view, &store);
    assert!(
        first
            .iter()
            .any(|a| matches!(a, PlannedAction::Insert { condition, .. } if condition.kind == K::PrinterOffline)),
        "fixture s22: the first pass inserts printer.offline"
    );
    store.apply(&first, now());
    let second = run(&c.view, &store);
    assert!(!second.is_empty(), "fixture s22: the second pass amends");
    assert!(
        second
            .iter()
            .all(|a| matches!(a, PlannedAction::Amend { changed: false, .. })),
        "fixture s22: only Amend= on the second pass: {second:#?}"
    );
}

#[test]
fn fixture_s23() {
    check(s23(), K::PrinterHostFailed, vec![]);
}

#[test]
fn fixture_s24() {
    check(
        s24(),
        K::JobStartConfirmation,
        vec![insert_again(
            K::JobStartConfirmation,
            p_detail(K::JobStartConfirmation),
        )],
    );
}

#[test]
fn fixture_s25() {
    check(s25(), K::PrinterConnectionError, vec![]);
}

#[test]
fn fixture_s26() {
    check(s26(), K::PrinterConnectionError, vec![]);
}

#[test]
fn fixture_s27() {
    let c = s27();
    check(s27(), K::JobCompleted, vec![amend_eq(K::JobCompleted)]);
    let mut store = c.store.clone();
    let actions = run(&c.view, &store);
    store.apply(&actions, now());
    let row = store.events.iter().find(|e| e.id == "att-open").unwrap();
    assert_eq!(
        row.evidence,
        Some(EvidenceOutcome::Captured {
            snapshot_id: "snp-1".into()
        }),
        "fixture s27: the applied row keeps its evidence"
    );
    assert_eq!(row.revision, 1, "fixture s27: Amend= bumps no revision");
    assert_eq!(row.observation_count, 2);
}

#[test]
fn fixture_s28() {
    check(
        s28(),
        K::PrinterHostFailed,
        vec![insert(
            K::PrinterHostFailed,
            AttentionDetail::PrinterHostFailed,
        )],
    );
}

#[test]
fn fixture_s29() {
    check(
        s29(),
        K::PrinterHostFailed,
        vec![insert(
            K::PrinterHostFailed,
            AttentionDetail::PrinterHostFailed,
        )],
    );
}

// ---------------------------------------------------------------------
// Beyond the tables: durable facts during the backfill, and reach cells
// ---------------------------------------------------------------------

/// Controller ruling (the spec's table puts durable `Absent` before
/// "`Unknown` otherwise, including backfill"): an archived Printer's
/// `printer.*` resolve even in the backfill.
#[test]
fn fixture_x1_backfill_archived_printer_resolves_its_printer_events() {
    for kind in [
        K::PrinterOffline,
        K::PrinterConnectionError,
        K::PrinterHostFailed,
    ] {
        let mut v = baseline();
        backfill(&mut v);
        prn(&mut v).archived = true;
        assert_actions(
            "x1",
            &v,
            &prior(kind, Prior::O),
            kind,
            vec![resolve(ConditionCleared)],
        );
    }
}

#[test]
fn fixture_x2_backfill_setup_incomplete_resolves_connection_error() {
    let mut v = baseline();
    backfill(&mut v);
    prn(&mut v).setup_incomplete = true;
    assert_actions(
        "x2",
        &v,
        &prior(K::PrinterConnectionError, Prior::O),
        K::PrinterConnectionError,
        vec![resolve(ConditionCleared)],
    );
}

#[test]
fn fixture_x3_backfill_offline_alerts_off_resolves_offline() {
    let mut v = baseline();
    backfill(&mut v);
    prn(&mut v).alert_defaults.offline_after_minutes = None;
    assert_actions(
        "x3",
        &v,
        &prior(K::PrinterOffline, Prior::O),
        K::PrinterOffline,
        vec![resolve(ConditionCleared)],
    );
}

#[test]
fn fixture_x4_backfill_job_completed_absent_by_durable_facts_but_unknown_by_status() {
    // Not the latest Job any more: durable, resolves.
    let mut v = s17_world();
    backfill(&mut v);
    assert_actions(
        "x4-not-latest",
        &v,
        &prior(K::JobCompleted, Prior::O),
        K::JobCompleted,
        vec![resolve(ConditionCleared)],
    );
    // Still the latest Job: only the live status could tell, so unknown.
    let mut v = c10_p();
    backfill(&mut v);
    assert_actions(
        "x4-latest",
        &v,
        &prior(K::JobCompleted, Prior::O),
        K::JobCompleted,
        vec![],
    );
    assert_actions(
        "x4-latest-n",
        &v,
        &prior(K::JobCompleted, Prior::N),
        K::JobCompleted,
        vec![],
    );
}

#[test]
fn fixture_x5_backfill_still_projects_durable_families() {
    let mut v = c7_p();
    set_spool_mg(&mut v, 80_000);
    backfill(&mut v);
    assert_actions(
        "x5-requirement",
        &v,
        &Store::default(),
        K::RequirementMaterialReconciliation,
        vec![insert(
            K::RequirementMaterialReconciliation,
            p_detail(K::RequirementMaterialReconciliation),
        )],
    );
    assert_actions(
        "x5-spool",
        &v,
        &Store::default(),
        K::SpoolLow,
        vec![insert(K::SpoolLow, p_detail(K::SpoolLow))],
    );
}

#[test]
fn fixture_x6_protocol_error_is_a_connection_error() {
    let v = world(|v| {
        set_status(
            v,
            ConnectionState::Error,
            Some(ConnectionErrorCause::Protocol),
            OperationalState::Error,
            None,
        )
    });
    assert_actions(
        "x6",
        &v,
        &Store::default(),
        K::PrinterConnectionError,
        vec![insert(
            K::PrinterConnectionError,
            AttentionDetail::PrinterConnectionError {
                cause: PrinterConnectionErrorCause::Protocol,
            },
        )],
    );
    // Misconfigured is `Absent` for printer.offline.
    assert_actions(
        "x6-offline",
        &v,
        &prior(K::PrinterOffline, Prior::O),
        K::PrinterOffline,
        vec![resolve(ConditionCleared)],
    );
}

#[test]
fn fixture_x7_unreachable_error_is_offline_and_unknown_for_connection_error() {
    let v = world(|v| {
        set_status(
            v,
            ConnectionState::Error,
            Some(ConnectionErrorCause::Unreachable),
            OperationalState::Error,
            None,
        );
        prn(v).unreachable_since = Some(t("11:50:00"));
    });
    assert_actions(
        "x7-offline",
        &v,
        &Store::default(),
        K::PrinterOffline,
        vec![insert(K::PrinterOffline, p_detail(K::PrinterOffline))],
    );
    assert_actions(
        "x7-connection-error",
        &v,
        &auth_prior(),
        K::PrinterConnectionError,
        vec![],
    );
}

#[test]
fn fixture_x8_grace_is_measured_from_startup_and_ends_exactly_at_the_deadline() {
    // STARTED 11:55, unreachable since 11:00: max(11:00, 11:55) + 5 = 12:00 = NOW.
    let mut v = world(|v| offline(v, "11:00:00"));
    v.supervisors_started_at = Some(t("11:55:00"));
    let detail = AttentionDetail::PrinterOffline {
        unreachable_since: "2026-09-27T11:00:00Z".into(),
    };
    assert_actions(
        "x8",
        &v,
        &Store::default(),
        K::PrinterOffline,
        vec![insert(K::PrinterOffline, detail)],
    );
}

#[test]
fn fixture_x9_reachable_but_not_failed_is_absent_for_host_failed_even_with_a_matching_file() {
    let v = world(|v| {
        with_job(v, JobState::Printing, None);
        set_status(
            v,
            ConnectionState::Online,
            None,
            OperationalState::Printing,
            Some("cube.gcode"),
        );
    });
    assert_actions(
        "x9",
        &v,
        &prior(K::PrinterHostFailed, Prior::O),
        K::PrinterHostFailed,
        vec![resolve(ConditionCleared)],
    );
}

#[test]
fn fixture_x10_a_failed_host_file_for_an_older_started_job_is_not_covered() {
    // job-1 started and ended; a newer, unstarted job-2 is the latest Job,
    // so job-1's file no longer covers the failure.
    let mut v = s17_world();
    set_status(
        &mut v,
        ConnectionState::Online,
        None,
        OperationalState::Failed,
        Some("cube.gcode"),
    );
    assert_actions(
        "x10",
        &v,
        &Store::default(),
        K::PrinterHostFailed,
        vec![insert(
            K::PrinterHostFailed,
            AttentionDetail::PrinterHostFailed,
        )],
    );
}

/// The backfill's `supervisors_started_at: None` alone makes every
/// live-status judgement unknown, even with a (hydrated) status present.
#[test]
fn fixture_x11_no_supervisors_started_at_is_unknown_even_with_a_status() {
    let rows: [(&str, fn() -> FarmView, ConditionKind); 4] = [
        ("x11-offline", c1_p, K::PrinterOffline),
        ("x11-connection-error", c2_p, K::PrinterConnectionError),
        ("x11-host-failed", c3_p, K::PrinterHostFailed),
        ("x11-completed", c10_p, K::JobCompleted),
    ];
    for (id, world_of, kind) in rows {
        let mut v = world_of();
        v.supervisors_started_at = None;
        assert_actions(id, &v, &prior(kind, Prior::N), kind, vec![]);
        assert_actions(id, &v, &prior(kind, Prior::O), kind, vec![]);
    }
    // And with the Condition gone (baseline, ready): still no resolve.
    let mut v = baseline();
    v.supervisors_started_at = None;
    for kind in [
        K::PrinterOffline,
        K::PrinterConnectionError,
        K::PrinterHostFailed,
    ] {
        assert_actions("x11-baseline", &v, &prior(kind, Prior::O), kind, vec![]);
    }
}

// ---------------------------------------------------------------------
// The Insert's subject and ids
// ---------------------------------------------------------------------

fn inserted(view: &FarmView, kind: ConditionKind) -> farm3d_lib::attention::Condition {
    run(view, &Store::default())
        .into_iter()
        .find_map(|a| match a {
            PlannedAction::Insert { condition, .. } if condition.kind == kind => Some(condition),
            _ => None,
        })
        .unwrap_or_else(|| panic!("no {kind:?} insert"))
}

#[test]
fn insert_carries_the_subject_the_ids_and_the_summary() {
    let c = inserted(&c1_p(), K::PrinterOffline);
    assert_eq!(c.subject, subject());
    assert_eq!(
        (
            c.printer_id.as_deref(),
            c.job_id.as_deref(),
            c.spool_id.as_deref(),
            c.requirement_id.as_deref()
        ),
        (Some(PRN), None, None, None)
    );
    assert_eq!(c.summary(), "Voron (Bay A) is offline.");

    let c = inserted(&c5_p(), K::JobFailed);
    assert_eq!(
        c.subject,
        AttentionSubject {
            printer_name: Some("Voron".into()),
            printer_location: Some("Bay A".into()),
            job_label: Some("Cube — Plate 1".into()),
            spool_number: Some(12),
            spool_label: Some("Polymaker PLA".into()),
        }
    );
    assert_eq!(
        (
            c.printer_id.as_deref(),
            c.job_id.as_deref(),
            c.spool_id.as_deref(),
            c.requirement_id.as_deref()
        ),
        (Some(PRN), Some(JOB), None, None)
    );
    assert_eq!(c.summary(), "Cube — Plate 1 failed on Voron.");

    let c = inserted(&c7_p(), K::RequirementMaterialReconciliation);
    assert_eq!(
        (
            c.printer_id.as_deref(),
            c.job_id.as_deref(),
            c.spool_id.as_deref(),
            c.requirement_id.as_deref()
        ),
        (Some(PRN), Some(JOB), Some(SPL), Some(RRQ))
    );
    assert_eq!(c.subject.job_label.as_deref(), Some("Cube — Plate 1"));

    let c = inserted(&c8_p(), K::RequirementJobOutcomeUnknown);
    assert_eq!(
        (
            c.printer_id.as_deref(),
            c.job_id.as_deref(),
            c.spool_id.as_deref(),
            c.requirement_id.as_deref()
        ),
        (Some(PRN), Some(JOB), None, Some(RRQ))
    );
    assert_eq!(c.subject.spool_number, Some(12));

    let c = inserted(&c9_p(), K::SpoolLow);
    assert_eq!(
        (
            c.printer_id.as_deref(),
            c.job_id.as_deref(),
            c.spool_id.as_deref()
        ),
        (None, None, Some(SPL))
    );
    assert_eq!(c.subject.spool_number, Some(12));
    assert_eq!(c.subject.spool_label.as_deref(), Some("Polymaker PLA"));
    assert_eq!(c.summary(), "Spool #12 is low (80 g left).");
}

// ---------------------------------------------------------------------
// Fixed point (D2): over every catalogue and supplementary fixture
// ---------------------------------------------------------------------

#[test]
fn plan_is_a_fixed_point_for_every_fixture() {
    let mut count = 0;
    for row in catalogue() {
        for which in [Prior::N, Prior::O, Prior::R] {
            let id = format!("{}-{which:?}", row.id);
            assert_fixed_point(&id, &(row.world)(), prior(row.kind, which));
            count += 1;
        }
    }
    for c in supplementary() {
        assert_fixed_point(c.id, &c.view, c.store);
        count += 1;
    }
    assert_eq!(count, 90 + 34);
}

// ---------------------------------------------------------------------
// next_deadline fixtures
// ---------------------------------------------------------------------

#[test]
fn fixture_d1() {
    let v = world(|v| offline(v, "11:58:00"));
    assert_eq!(next_deadline(&v, now()), Some(t("12:03:00")), "fixture d1");
}

#[test]
fn fixture_d2() {
    let mut v = world(|v| offline(v, "11:58:00"));
    v.supervisors_started_at = Some(t("11:59:00"));
    assert_eq!(next_deadline(&v, now()), Some(t("12:04:00")), "fixture d2");
}

#[test]
fn fixture_d3() {
    let v = world(|v| {
        offline(v, "11:58:00");
        prn(v).alert_defaults.offline_after_minutes = None;
    });
    assert_eq!(next_deadline(&v, now()), None, "fixture d3");
}

#[test]
fn fixture_d4() {
    let v = world(|v| offline(v, "11:50:00"));
    assert_eq!(next_deadline(&v, now()), None, "fixture d4");
}

#[test]
fn fixture_d5() {
    let v = world(|v| {
        offline(v, "11:58:00");
        let mut second = printer("prn-2", "Prusa");
        second.status = Some(status(
            ConnectionState::Offline,
            None,
            OperationalState::Offline,
            None,
        ));
        second.unreachable_since = Some(t("11:56:00"));
        v.printers.push(second);
    });
    assert_eq!(next_deadline(&v, now()), Some(t("12:01:00")), "fixture d5");
}

#[test]
fn next_deadline_ignores_the_backfill_and_other_grace_lengths_count() {
    let mut v = world(|v| offline(v, "11:58:00"));
    backfill(&mut v);
    prn(&mut v).unreachable_since = Some(t("11:58:00"));
    assert_eq!(next_deadline(&v, now()), None);

    let v = world(|v| {
        offline(v, "11:58:00");
        prn(v).alert_defaults.offline_after_minutes = Some(OfflineAlertMinutes::Fifteen);
    });
    assert_eq!(next_deadline(&v, now()), Some(t("12:13:00")));
}
