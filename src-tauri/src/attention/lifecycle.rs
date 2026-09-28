//! P8 D2 "Lifecycle rules": the Attention Event lifecycle, as a pure
//! function so the repository can never write an illegal combination of
//! `read_at`/`acknowledged_at`/`resolved_at`. See the design spec's D2
//! "Lifecycle rules" table — this module's `SPEC_TABLE` test fixture is a
//! direct copy of it.
//!
//! An Attention Event's three dimensions (`read_at`, `acknowledged_at`,
//! `resolved_at`) are independent columns (D1). Nothing ever clears one
//! once set; acknowledging or resolving sets `read_at` if it is `NULL`
//! (the umbrella's "resolving implies read"). The migration's CHECKs make
//! "unread, acknowledged" and "unread, resolved" impossible, so this
//! module only ever sees one of five states.

use chrono::{DateTime, SecondsFormat, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use super::{AttentionEvent, AttentionResolution, ResolutionMode};

/// D2 "Incident rule"/`IncidentEntryDetail::EventAcknowledged`: who
/// acknowledged an Event — the operator, from the command, or the
/// projector on the Event's behalf (`Insert { acknowledged: true }`,
/// D2's planner rule for a `deferred` Reconciliation Requirement).
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/AckBy.ts")]
pub enum AckBy {
    Operator,
    System,
}

/// The three commands [`apply`] handles (`mark_attention_read`,
/// `acknowledge_attention_event`, `resolve_attention_event`), plus the
/// projector's own system acknowledge/resolve.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LifecycleOp {
    MarkRead,
    Acknowledge { by: AckBy },
    Resolve(AttentionResolution),
}

/// [`apply`]'s result: the new value of each lifecycle column, and
/// whether anything changed. A `changed: false` result means the caller
/// writes nothing, bumps no `revision`, and emits no event (D2
/// "Lifecycle rules": "a no-op returns `changed: false`").
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct LifecycleChange {
    pub read_at: Option<String>,
    pub acknowledged_at: Option<String>,
    pub resolved_at: Option<String>,
    pub resolution: Option<AttentionResolution>,
    pub changed: bool,
}

/// [`apply`]'s only rejection: `Resolve(operatorResolved)` on an Event
/// whose `resolution_mode` isn't `manual`. Checked before the resolved
/// check, so the answer for a given Condition never depends on timing
/// (maps to `ErrorCode::AttentionNotManual`, a later task's concern).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum LifecycleError {
    NotManual,
}

/// D2 "Lifecycle rules": applies `op` to `event`'s current lifecycle
/// state and returns the new column values. Pure — the caller persists
/// the result and, on `changed: true`, bumps `revision` and emits
/// `attention.event.changed`.
///
/// Acknowledging or resolving sets `read_at` if it was `NULL` (a system
/// resolution too). `Resolve(operatorResolved)` on a non-`manual` Event
/// is always [`LifecycleError::NotManual`], whether the Event is open or
/// already resolved. `Acknowledge` on a resolved Event, or a second
/// `MarkRead`/`Acknowledge`/`Resolve` that changes nothing, is a no-op
/// success (`changed: false`), never an error — the planner never
/// produces a system `Resolve` for an already-resolved Event, but this
/// function stays total.
pub fn apply(
    event: &AttentionEvent,
    op: LifecycleOp,
    now: DateTime<Utc>,
) -> Result<LifecycleChange, LifecycleError> {
    let is_resolved = event.resolved_at.is_some();

    match op {
        LifecycleOp::MarkRead => {
            if event.read_at.is_some() {
                return Ok(no_op(event));
            }
            Ok(LifecycleChange {
                read_at: Some(rfc3339(now)),
                acknowledged_at: event.acknowledged_at.clone(),
                resolved_at: event.resolved_at.clone(),
                resolution: event.resolution,
                changed: true,
            })
        }
        LifecycleOp::Acknowledge { .. } => {
            if is_resolved || event.acknowledged_at.is_some() {
                return Ok(no_op(event));
            }
            Ok(LifecycleChange {
                read_at: Some(event.read_at.clone().unwrap_or_else(|| rfc3339(now))),
                acknowledged_at: Some(rfc3339(now)),
                resolved_at: event.resolved_at.clone(),
                resolution: event.resolution,
                changed: true,
            })
        }
        LifecycleOp::Resolve(resolution) => {
            if resolution == AttentionResolution::OperatorResolved
                && event.resolution_mode != ResolutionMode::Manual
            {
                return Err(LifecycleError::NotManual);
            }
            if is_resolved {
                return Ok(no_op(event));
            }
            Ok(LifecycleChange {
                read_at: Some(event.read_at.clone().unwrap_or_else(|| rfc3339(now))),
                acknowledged_at: event.acknowledged_at.clone(),
                resolved_at: Some(rfc3339(now)),
                resolution: Some(resolution),
                changed: true,
            })
        }
    }
}

fn no_op(event: &AttentionEvent) -> LifecycleChange {
    LifecycleChange {
        read_at: event.read_at.clone(),
        acknowledged_at: event.acknowledged_at.clone(),
        resolved_at: event.resolved_at.clone(),
        resolution: event.resolution,
        changed: false,
    }
}

fn rfc3339(now: DateTime<Utc>) -> String {
    now.to_rfc3339_opts(SecondsFormat::AutoSi, true)
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::super::{
        AttentionAction, AttentionDetail, AttentionOrigin, AttentionSeverity, AttentionSource,
        AttentionSourceKind, AttentionSubject, ConditionKind, NotificationClass,
    };
    use super::*;

    /// A bare Attention Event of `mode`, with `read_at`/`acknowledged_at`/
    /// `resolved_at` set from the three flags (the migration's CHECKs
    /// make every other combination impossible, so callers only ever
    /// pass one of the five legal states).
    pub fn an_event(
        mode: ResolutionMode,
        read: bool,
        acknowledged: bool,
        resolved: bool,
    ) -> AttentionEvent {
        assert!(!acknowledged || read, "acknowledged implies read");
        assert!(!resolved || read, "resolved implies read");
        AttentionEvent {
            id: "att-a".to_string(),
            revision: 1,
            dedup_key: "printer.hostFailed:printer:prn-a".to_string(),
            condition: ConditionKind::PrinterHostFailed,
            severity: AttentionSeverity::Fatal,
            requires_action: true,
            resolution_mode: mode,
            notification_class: NotificationClass::Fatal,
            source: AttentionSource {
                kind: AttentionSourceKind::Printer,
                id: "prn-a".to_string(),
            },
            printer_id: Some("prn-a".to_string()),
            job_id: None,
            spool_id: None,
            requirement_id: None,
            incident_id: None,
            subject: AttentionSubject {
                printer_name: Some("Voron".to_string()),
                printer_location: None,
                job_label: None,
                spool_number: None,
                spool_label: None,
            },
            detail: AttentionDetail::PrinterHostFailed,
            summary: "Voron failed.".to_string(),
            origin: AttentionOrigin::Live,
            first_observed_at: "2026-09-27T12:00:00Z".to_string(),
            last_observed_at: "2026-09-27T12:00:00Z".to_string(),
            observation_count: 1,
            recurrence_of: None,
            read_at: read.then(|| "2026-09-27T11:00:00Z".to_string()),
            acknowledged_at: acknowledged.then(|| "2026-09-27T11:05:00Z".to_string()),
            resolved_at: resolved.then(|| "2026-09-27T11:10:00Z".to_string()),
            resolution: resolved.then_some(AttentionResolution::ConditionCleared),
            notified_at: None,
            evidence: None,
            allowed_actions: Vec::<AttentionAction>::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::an_event;
    use super::*;
    use ResolutionMode::{Action, Auto, Manual};

    fn now() -> DateTime<Utc> {
        "2026-09-27T12:00:00Z".parse().unwrap()
    }

    /// D2's lifecycle table, row by row, for a `manual` Event (the only
    /// mode `Resolve(operatorResolved)` accepts). `(read, acknowledged,
    /// resolved)` names each of the five legal states in the table's row
    /// order.
    const STATES: [(bool, bool, bool); 5] = [
        (false, false, false), // unread, unacknowledged, open
        (true, false, false),  // read, unacknowledged, open
        (true, true, false),   // read, acknowledged, open
        (true, false, true),   // read, unacknowledged, resolved
        (true, true, true),    // read, acknowledged, resolved
    ];

    #[test]
    fn mark_read_follows_the_table() {
        // read, sets read_at only when it was unset; a no-op everywhere
        // else (every other state is already read).
        for (i, &(read, acknowledged, resolved)) in STATES.iter().enumerate() {
            let event = an_event(Manual, read, acknowledged, resolved);
            let change = apply(&event, LifecycleOp::MarkRead, now()).expect("MarkRead never errors");
            if i == 0 {
                assert!(change.changed, "row {i}");
                assert_eq!(change.read_at, Some(rfc3339(now())));
            } else {
                assert!(!change.changed, "row {i}");
                assert_eq!(change.read_at, event.read_at);
            }
            assert_eq!(change.acknowledged_at, event.acknowledged_at);
            assert_eq!(change.resolved_at, event.resolved_at);
        }
    }

    #[test]
    fn acknowledge_follows_the_table() {
        // (unread, open) -> read+acknowledged; (read, unacked, open) ->
        // acknowledged; (read, acked, open) -> no-op (already
        // acknowledged); both resolved rows -> no-op success, never an
        // error.
        let by = AckBy::Operator;
        for (i, &(read, acknowledged, resolved)) in STATES.iter().enumerate() {
            let event = an_event(Manual, read, acknowledged, resolved);
            let change = apply(&event, LifecycleOp::Acknowledge { by }, now())
                .expect("Acknowledge never errors");
            match i {
                0 => {
                    assert!(change.changed, "row {i}");
                    assert_eq!(change.read_at, Some(rfc3339(now())));
                    assert_eq!(change.acknowledged_at, Some(rfc3339(now())));
                }
                1 => {
                    assert!(change.changed, "row {i}");
                    assert_eq!(change.read_at, event.read_at, "read_at stays put once set");
                    assert_eq!(change.acknowledged_at, Some(rfc3339(now())));
                }
                _ => {
                    assert!(!change.changed, "row {i} is a no-op");
                    assert_eq!(change.acknowledged_at, event.acknowledged_at);
                    assert_eq!(change.resolved_at, event.resolved_at);
                }
            }
        }
    }

    #[test]
    fn resolve_operator_resolved_follows_the_table_for_a_manual_event() {
        for (i, &(read, acknowledged, resolved)) in STATES.iter().enumerate() {
            let event = an_event(Manual, read, acknowledged, resolved);
            let change = apply(
                &event,
                LifecycleOp::Resolve(AttentionResolution::OperatorResolved),
                now(),
            )
            .expect("manual resolve never errors");
            if resolved {
                assert!(!change.changed, "row {i} is already resolved: no-op");
                assert_eq!(change.resolved_at, event.resolved_at);
                assert_eq!(change.resolution, event.resolution);
            } else {
                assert!(change.changed, "row {i}");
                assert_eq!(change.resolved_at, Some(rfc3339(now())));
                assert_eq!(change.resolution, Some(AttentionResolution::OperatorResolved));
                assert_eq!(
                    change.read_at,
                    Some(event.read_at.clone().unwrap_or_else(|| rfc3339(now()))),
                );
            }
        }
    }

    /// `Resolve(operatorResolved)` is `NotManual` for every non-manual
    /// mode, in every state, checked before the resolved check (so an
    /// already-resolved `auto`/`action` Event still errors, not no-ops).
    #[test]
    fn resolve_operator_resolved_is_not_manual_for_every_other_mode() {
        for mode in [Auto, Action] {
            for &(read, acknowledged, resolved) in &STATES {
                let event = an_event(mode, read, acknowledged, resolved);
                let error = apply(
                    &event,
                    LifecycleOp::Resolve(AttentionResolution::OperatorResolved),
                    now(),
                )
                .expect_err("non-manual operatorResolved must be rejected");
                assert_eq!(error, LifecycleError::NotManual);
            }
        }
    }

    /// A system resolution (`conditionCleared`/`actionCompleted`/
    /// `sourceRemoved`) is accepted regardless of `resolution_mode`, sets
    /// `read_at` if unset, and no-ops once already resolved.
    #[test]
    fn resolve_system_reasons_follow_the_table_for_every_mode() {
        for mode in [Auto, Action, Manual] {
            for reason in [
                AttentionResolution::ConditionCleared,
                AttentionResolution::ActionCompleted,
                AttentionResolution::SourceRemoved,
            ] {
                for (i, &(read, acknowledged, resolved)) in STATES.iter().enumerate() {
                    let event = an_event(mode, read, acknowledged, resolved);
                    let change = apply(&event, LifecycleOp::Resolve(reason), now())
                        .expect("a system resolution never errors");
                    if resolved {
                        assert!(!change.changed, "{mode:?} row {i} is a no-op");
                        assert_eq!(change.resolution, event.resolution);
                    } else {
                        assert!(change.changed, "{mode:?} row {i}");
                        assert_eq!(change.resolved_at, Some(rfc3339(now())));
                        assert_eq!(change.resolution, Some(reason));
                        assert_eq!(
                            change.read_at,
                            Some(event.read_at.clone().unwrap_or_else(|| rfc3339(now()))),
                            "resolving implies read"
                        );
                    }
                }
            }
        }
    }

    /// D2's full table, transcribed as `(from, op, expected)` triples,
    /// where `expected` is `Ok(changed)` or `Err(NotManual)`. Mirrors the
    /// spec's D2 table exactly, one row per (state x op) cell.
    #[test]
    fn spec_table_every_state_and_op_cell() {
        #[derive(Clone, Copy)]
        enum Expect {
            Changed,
            NoOp,
            NotManual,
        }

        // Row order matches STATES: unread/open, read-unacked/open,
        // read-acked/open, read-unacked/resolved, read-acked/resolved.
        let auto_and_action_table: [[Expect; 4]; 5] = [
            [Expect::Changed, Expect::Changed, Expect::NotManual, Expect::Changed],
            [Expect::NoOp, Expect::Changed, Expect::NotManual, Expect::Changed],
            [Expect::NoOp, Expect::NoOp, Expect::NotManual, Expect::Changed],
            [Expect::NoOp, Expect::NoOp, Expect::NotManual, Expect::NoOp],
            [Expect::NoOp, Expect::NoOp, Expect::NotManual, Expect::NoOp],
        ];
        let manual_table: [[Expect; 4]; 5] = [
            [Expect::Changed, Expect::Changed, Expect::Changed, Expect::Changed],
            [Expect::NoOp, Expect::Changed, Expect::Changed, Expect::Changed],
            [Expect::NoOp, Expect::NoOp, Expect::Changed, Expect::Changed],
            [Expect::NoOp, Expect::NoOp, Expect::NoOp, Expect::NoOp],
            [Expect::NoOp, Expect::NoOp, Expect::NoOp, Expect::NoOp],
        ];

        for (mode, table) in [
            (Auto, &auto_and_action_table),
            (Action, &auto_and_action_table),
            (Manual, &manual_table),
        ] {
            for (row, &(read, acknowledged, resolved)) in STATES.iter().enumerate() {
                let event = an_event(mode, read, acknowledged, resolved);
                let ops = [
                    LifecycleOp::MarkRead,
                    LifecycleOp::Acknowledge { by: AckBy::Operator },
                    LifecycleOp::Resolve(AttentionResolution::OperatorResolved),
                    LifecycleOp::Resolve(AttentionResolution::ConditionCleared),
                ];
                for (col, op) in ops.into_iter().enumerate() {
                    let result = apply(&event, op, now());
                    match table[row][col] {
                        Expect::Changed => assert!(
                            result.expect("must not error").changed,
                            "{mode:?} row {row} col {col} must change"
                        ),
                        Expect::NoOp => assert!(
                            !result.expect("must not error").changed,
                            "{mode:?} row {row} col {col} must be a no-op"
                        ),
                        Expect::NotManual => assert_eq!(
                            result.expect_err("must reject"),
                            LifecycleError::NotManual,
                            "{mode:?} row {row} col {col} must be NotManual"
                        ),
                    }
                }
            }
        }
    }
}
