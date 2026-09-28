//! P8 D2 "Planner rules": the pure planner. `plan` diffs the
//! [`ObservedConditions`] against the open Attention Events and returns
//! the actions the projector applies in one transaction.

use std::collections::HashMap;

use super::observe::{Observation, ObservedConditions};
use super::{
    summary, AttentionDetail, AttentionEvent, AttentionResolution, AttentionSeverity, Condition,
    ResolutionMode,
};

/// One step of a pass's plan, applied in order by the projector (D2
/// "Apply").
/// Short-lived per pass, so the `Condition` stays unboxed.
#[allow(clippy::large_enum_variant)]
#[derive(Clone, PartialEq, Debug)]
pub enum PlannedAction {
    /// A new open Event for `condition` (`acknowledged` by the system
    /// when `condition.acknowledge`).
    Insert {
        condition: Condition,
        recurrence_of: Option<String>,
        acknowledged: bool,
    },
    /// A `Present` observation of an open Event: always `last_observed_at`
    /// and `observation_count + 1`; when `changed`, also `detail`,
    /// `severity`, and `summary` (re-derived from the Event's own subject
    /// and the new detail) with `revision + 1`. Never the evidence, the
    /// Incident link, or the lifecycle columns.
    Amend {
        event_id: String,
        detail: AttentionDetail,
        severity: AttentionSeverity,
        summary: String,
        changed: bool,
    },
    /// A system acknowledgement (a deferred material requirement).
    Acknowledge { event_id: String },
    /// A system resolution: `conditionCleared` or `actionCompleted`.
    Resolve {
        event_id: String,
        resolution: AttentionResolution,
    },
}

/// D2 "Planner rules": walks `observed` in key order. For each key, with
/// `e` the open Event for the key and `r` the latest resolved one:
///
/// | Observation | `e` exists | no `e`, `r` exists | neither |
/// |---|---|---|---|
/// | `Present(c)` | `Amend`, then `Acknowledge` if `c.acknowledge` and `e` is unacknowledged | recurring: `Insert` with `recurrenceOf: r`; once per source: nothing | `Insert` |
/// | `Absent` | by mode: `auto` resolves `conditionCleared`, `action` `actionCompleted`, `manual` nothing | nothing | nothing |
/// | `Unknown` | nothing | nothing | nothing |
///
/// An open Event whose key isn't observed gets nothing. Pure, and a fixed
/// point: applying the result and planning again yields only
/// `Amend { changed: false }`.
pub fn plan(
    open: &[AttentionEvent],
    latest_resolved: &HashMap<String, String>,
    observed: &ObservedConditions,
) -> Vec<PlannedAction> {
    let open_by_key: HashMap<&str, &AttentionEvent> = open
        .iter()
        .filter(|e| e.resolved_at.is_none())
        .map(|e| (e.dedup_key.as_str(), e))
        .collect();
    let mut actions = Vec::new();
    for (key, observation) in observed {
        let open_event = open_by_key.get(key.as_str()).copied();
        match (observation, open_event) {
            (Observation::Present(condition), Some(e)) => {
                debug_assert_matching(condition);
                debug_assert_eq!(
                    e.condition, condition.kind,
                    "an open Event's key names its kind"
                );
                let severity = condition.spec().severity;
                let detail = condition.detail.clone();
                let changed = detail != e.detail || severity != e.severity;
                actions.push(PlannedAction::Amend {
                    event_id: e.id.clone(),
                    summary: summary(e.condition, &e.subject, &detail),
                    detail,
                    severity,
                    changed,
                });
                if condition.acknowledge && e.acknowledged_at.is_none() {
                    actions.push(PlannedAction::Acknowledge {
                        event_id: e.id.clone(),
                    });
                }
            }
            (Observation::Present(condition), None) => {
                debug_assert_matching(condition);
                let recurrence_of = match latest_resolved.get(key) {
                    // Once per source: never a second Event for the key.
                    Some(_) if !condition.spec().recurs => continue,
                    previous => previous.cloned(),
                };
                actions.push(PlannedAction::Insert {
                    condition: condition.clone(),
                    recurrence_of,
                    acknowledged: condition.acknowledge,
                });
            }
            (Observation::Absent, Some(e)) => {
                let resolution = match e.resolution_mode {
                    ResolutionMode::Auto => Some(AttentionResolution::ConditionCleared),
                    ResolutionMode::Action => Some(AttentionResolution::ActionCompleted),
                    ResolutionMode::Manual => None,
                };
                if let Some(resolution) = resolution {
                    actions.push(PlannedAction::Resolve {
                        event_id: e.id.clone(),
                        resolution,
                    });
                }
            }
            (Observation::Absent, None) | (Observation::Unknown, _) => {}
        }
    }
    actions
}

/// Debug builds: a Condition's detail is its own kind's variant.
fn debug_assert_matching(condition: &Condition) {
    debug_assert_eq!(
        condition.detail.condition_kind(),
        condition.kind,
        "a Condition's detail must be its own kind's variant"
    );
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::attention::{dedup_key, AttentionOrigin, AttentionSubject, ConditionKind};

    fn subject() -> AttentionSubject {
        AttentionSubject {
            printer_name: Some("Voron".into()),
            printer_location: None,
            job_label: None,
            spool_number: Some(12),
            spool_label: None,
        }
    }

    fn low(mg: i64) -> AttentionDetail {
        AttentionDetail::SpoolLow {
            current_mg: mg,
            low_threshold_mg: 100_000,
        }
    }

    fn condition(kind: ConditionKind, detail: AttentionDetail) -> Condition {
        Condition {
            kind,
            source_id: "src-1".into(),
            printer_id: None,
            job_id: None,
            spool_id: None,
            requirement_id: None,
            subject: subject(),
            detail,
            acknowledge: false,
        }
    }

    fn event(id: &str, kind: ConditionKind, detail: AttentionDetail) -> AttentionEvent {
        let spec = kind.spec();
        AttentionEvent {
            id: id.into(),
            revision: 1,
            dedup_key: dedup_key(kind, "src-1"),
            condition: kind,
            severity: spec.severity,
            requires_action: spec.requires_action,
            resolution_mode: spec.resolution_mode,
            notification_class: spec.notification_class,
            source: crate::attention::AttentionSource {
                kind: spec.source_kind,
                id: "src-1".into(),
            },
            printer_id: None,
            job_id: None,
            spool_id: None,
            requirement_id: None,
            incident_id: None,
            summary: summary(kind, &subject(), &detail),
            subject: subject(),
            detail,
            origin: AttentionOrigin::Live,
            first_observed_at: "2026-09-27T11:00:00Z".into(),
            last_observed_at: "2026-09-27T11:00:00Z".into(),
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

    fn observed(kind: ConditionKind, observation: Observation) -> ObservedConditions {
        ObservedConditions::from([(dedup_key(kind, "src-1"), observation)])
    }

    fn resolved_map(kind: ConditionKind) -> HashMap<String, String> {
        HashMap::from([(dedup_key(kind, "src-1"), "att-prev".to_string())])
    }

    /// A detail of `kind`'s own variant.
    fn detail_for(kind: ConditionKind) -> AttentionDetail {
        let ended_at = || "2026-09-27T11:30:00Z".to_string();
        match kind {
            ConditionKind::PrinterOffline => AttentionDetail::PrinterOffline {
                unreachable_since: "2026-09-27T11:50:00Z".into(),
            },
            ConditionKind::PrinterConnectionError => AttentionDetail::PrinterConnectionError {
                cause: crate::attention::PrinterConnectionErrorCause::Auth,
            },
            ConditionKind::PrinterHostFailed => AttentionDetail::PrinterHostFailed,
            ConditionKind::JobStartConfirmation => AttentionDetail::JobStartConfirmation {
                awaiting_material: false,
            },
            ConditionKind::JobFailed => AttentionDetail::JobFailed {
                ended_at: ended_at(),
            },
            ConditionKind::JobHostCancelled => AttentionDetail::JobHostCancelled {
                ended_at: ended_at(),
            },
            ConditionKind::RequirementMaterialReconciliation => {
                AttentionDetail::RequirementMaterialReconciliation {
                    requirement_status: crate::attention::MaterialReconciliationStatus::Pending,
                    spool_id: "spl-1".into(),
                }
            }
            ConditionKind::RequirementJobOutcomeUnknown => {
                AttentionDetail::RequirementJobOutcomeUnknown
            }
            ConditionKind::SpoolLow => low(80_000),
            ConditionKind::JobCompleted => AttentionDetail::JobCompleted {
                ended_at: ended_at(),
            },
        }
    }

    #[test]
    #[cfg(debug_assertions)]
    #[should_panic(expected = "its own kind's variant")]
    fn a_condition_with_another_kinds_detail_is_caught_in_debug_builds() {
        let kind = ConditionKind::JobFailed;
        let wrong = condition(kind, low(80_000));
        plan(
            &[],
            &HashMap::new(),
            &observed(kind, Observation::Present(wrong)),
        );
    }

    #[test]
    fn detail_for_gives_every_kind_its_own_variant() {
        for kind in ConditionKind::ALL {
            assert_eq!(detail_for(kind).condition_kind(), kind);
        }
    }

    #[test]
    fn present_with_an_open_event_amends_and_reports_change() {
        let kind = ConditionKind::SpoolLow;
        let open = [event("att-1", kind, low(90_000))];
        let same = plan(
            &open,
            &HashMap::new(),
            &observed(kind, Observation::Present(condition(kind, low(90_000)))),
        );
        assert_eq!(
            same,
            [PlannedAction::Amend {
                event_id: "att-1".into(),
                detail: low(90_000),
                severity: AttentionSeverity::Warning,
                summary: "Spool #12 is low (90 g left).".into(),
                changed: false,
            }]
        );
        let changed = plan(
            &open,
            &HashMap::new(),
            &observed(kind, Observation::Present(condition(kind, low(80_000)))),
        );
        assert_eq!(
            changed,
            [PlannedAction::Amend {
                event_id: "att-1".into(),
                detail: low(80_000),
                severity: AttentionSeverity::Warning,
                summary: "Spool #12 is low (80 g left).".into(),
                changed: true,
            }]
        );
    }

    #[test]
    fn a_severity_drift_counts_as_a_change() {
        let kind = ConditionKind::SpoolLow;
        let mut stale = event("att-1", kind, low(90_000));
        stale.severity = AttentionSeverity::Info;
        let actions = plan(
            &[stale],
            &HashMap::new(),
            &observed(kind, Observation::Present(condition(kind, low(90_000)))),
        );
        assert!(matches!(
            actions.as_slice(),
            [PlannedAction::Amend {
                changed: true,
                severity: AttentionSeverity::Warning,
                ..
            }]
        ));
    }

    #[test]
    fn the_amend_summary_uses_the_events_own_subject() {
        let kind = ConditionKind::SpoolLow;
        let open = [event("att-1", kind, low(90_000))];
        let mut renamed = condition(kind, low(90_000));
        renamed.subject.spool_number = Some(99);
        let actions = plan(
            &open,
            &HashMap::new(),
            &observed(kind, Observation::Present(renamed)),
        );
        assert!(matches!(
            &actions[0],
            PlannedAction::Amend { summary, .. } if summary == "Spool #12 is low (90 g left)."
        ));
    }

    #[test]
    fn acknowledge_follows_amend_only_when_asked_and_unacknowledged() {
        let kind = ConditionKind::RequirementMaterialReconciliation;
        let detail = AttentionDetail::RequirementMaterialReconciliation {
            requirement_status: crate::attention::MaterialReconciliationStatus::Deferred,
            spool_id: "spl-1".into(),
        };
        let mut c = condition(kind, detail.clone());
        c.acknowledge = true;
        let unacked = [event("att-1", kind, detail.clone())];
        let actions = plan(
            &unacked,
            &HashMap::new(),
            &observed(kind, Observation::Present(c.clone())),
        );
        assert_eq!(actions.len(), 2);
        assert_eq!(
            actions[1],
            PlannedAction::Acknowledge {
                event_id: "att-1".into()
            }
        );
        let mut acked = event("att-1", kind, detail.clone());
        acked.read_at = Some("2026-09-27T11:10:00Z".into());
        acked.acknowledged_at = Some("2026-09-27T11:10:00Z".into());
        let actions = plan(
            &[acked],
            &HashMap::new(),
            &observed(kind, Observation::Present(c.clone())),
        );
        assert!(matches!(
            actions.as_slice(),
            [PlannedAction::Amend { changed: false, .. }]
        ));
        // Not asked: no Acknowledge even when unacknowledged.
        c.acknowledge = false;
        let actions = plan(
            &unacked,
            &HashMap::new(),
            &observed(kind, Observation::Present(c)),
        );
        assert_eq!(actions.len(), 1);
    }

    #[test]
    fn present_inserts_with_or_without_recurrence_by_catalogue() {
        for kind in ConditionKind::ALL {
            let c = condition(kind, detail_for(kind));
            let fresh = plan(
                &[],
                &HashMap::new(),
                &observed(kind, Observation::Present(c.clone())),
            );
            assert_eq!(
                fresh,
                [PlannedAction::Insert {
                    condition: c.clone(),
                    recurrence_of: None,
                    acknowledged: false
                }],
                "{kind:?}"
            );
            let again = plan(
                &[],
                &resolved_map(kind),
                &observed(kind, Observation::Present(c.clone())),
            );
            if kind.spec().recurs {
                assert_eq!(
                    again,
                    [PlannedAction::Insert {
                        condition: c,
                        recurrence_of: Some("att-prev".into()),
                        acknowledged: false
                    }],
                    "{kind:?}"
                );
            } else {
                assert_eq!(again, [], "{kind:?} is once per source");
            }
        }
    }

    #[test]
    fn insert_carries_the_conditions_acknowledge() {
        let kind = ConditionKind::RequirementMaterialReconciliation;
        let mut c = condition(kind, detail_for(kind));
        c.acknowledge = true;
        let actions = plan(
            &[],
            &HashMap::new(),
            &observed(kind, Observation::Present(c)),
        );
        assert!(matches!(
            actions.as_slice(),
            [PlannedAction::Insert {
                acknowledged: true,
                ..
            }]
        ));
    }

    #[test]
    fn absent_resolves_by_mode_and_never_a_manual_event() {
        for kind in ConditionKind::ALL {
            let open = [event("att-1", kind, detail_for(kind))];
            let actions = plan(&open, &HashMap::new(), &observed(kind, Observation::Absent));
            let expected: Vec<PlannedAction> = match kind.spec().resolution_mode {
                ResolutionMode::Auto => vec![PlannedAction::Resolve {
                    event_id: "att-1".into(),
                    resolution: AttentionResolution::ConditionCleared,
                }],
                ResolutionMode::Action => vec![PlannedAction::Resolve {
                    event_id: "att-1".into(),
                    resolution: AttentionResolution::ActionCompleted,
                }],
                ResolutionMode::Manual => vec![],
            };
            assert_eq!(actions, expected, "{kind:?}");
            // No open Event: nothing, resolved before or not.
            assert_eq!(
                plan(
                    &[],
                    &resolved_map(kind),
                    &observed(kind, Observation::Absent)
                ),
                []
            );
            assert_eq!(
                plan(&[], &HashMap::new(), &observed(kind, Observation::Absent)),
                []
            );
        }
    }

    #[test]
    fn unknown_and_unobserved_keys_never_act() {
        for kind in ConditionKind::ALL {
            let open = [event("att-1", kind, detail_for(kind))];
            assert_eq!(
                plan(
                    &open,
                    &HashMap::new(),
                    &observed(kind, Observation::Unknown)
                ),
                []
            );
            assert_eq!(
                plan(
                    &[],
                    &resolved_map(kind),
                    &observed(kind, Observation::Unknown)
                ),
                []
            );
            assert_eq!(
                plan(&[], &HashMap::new(), &observed(kind, Observation::Unknown)),
                []
            );
            assert_eq!(
                plan(&open, &resolved_map(kind), &ObservedConditions::new()),
                []
            );
        }
    }

    #[test]
    fn a_resolved_event_passed_as_open_is_ignored() {
        let kind = ConditionKind::SpoolLow;
        let mut done = event("att-1", kind, low(90_000));
        done.read_at = Some("2026-09-27T11:10:00Z".into());
        done.resolved_at = Some("2026-09-27T11:10:00Z".into());
        done.resolution = Some(AttentionResolution::ConditionCleared);
        let c = condition(kind, low(80_000));
        let actions = plan(
            &[done],
            &resolved_map(kind),
            &observed(kind, Observation::Present(c)),
        );
        assert!(matches!(
            actions.as_slice(),
            [PlannedAction::Insert { recurrence_of: Some(prev), .. }] if prev == "att-prev"
        ));
    }

    #[test]
    fn actions_follow_key_order() {
        let a = condition(ConditionKind::SpoolLow, low(1));
        let b = condition(
            ConditionKind::PrinterHostFailed,
            AttentionDetail::PrinterHostFailed,
        );
        let observed = ObservedConditions::from([
            (a.dedup_key(), Observation::Present(a.clone())),
            (b.dedup_key(), Observation::Present(b.clone())),
        ]);
        let kinds: Vec<_> = plan(&[], &HashMap::new(), &observed)
            .into_iter()
            .map(|action| match action {
                PlannedAction::Insert { condition, .. } => condition.kind,
                other => panic!("{other:?}"),
            })
            .collect();
        // "printer.hostFailed:…" sorts before "spool.low:…".
        assert_eq!(
            kinds,
            [ConditionKind::PrinterHostFailed, ConditionKind::SpoolLow]
        );
    }
}
