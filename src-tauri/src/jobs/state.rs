//! P7 D3: the Job state machine, as a pure function so the repository can
//! reject an illegal move before it ever reaches SQL. See the design
//! spec's D3 "Events" table — this module's `SPEC_TABLE` test fixture is
//! a direct copy of it (`Assigned` excluded: it's the Job's insert event,
//! carries no `from_state`, and is never passed to [`transition`]).
//!
//! This module only checks state *legality*. Which `cancelReason`,
//! `settlement`, event `detail`, or Reconciliation Requirement a legal
//! move also writes is `jobs::repository`'s job (a later task); this
//! module's [`settlement_after`] gives only the `Settlement` half of
//! that, since D3's settlement table is keyed on the event alone.

use std::time::Duration;

use chrono::{DateTime, Utc};

use super::{CancelReason, Job, JobAction, JobEventKind, JobState, Settlement};

/// `transition`'s rejection: `event` never legally fires from `from`. The
/// design spec's D3 splits the code an illegal pair maps to by who raised
/// it (a user command: `JOB_ACTION_NOT_ALLOWED`; the driver, tracker, or
/// `apply_host_outcome`: dropped as an already-applied idempotent no-op) —
/// that mapping is a later task's caller, not this module's job.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IllegalTransition {
    pub from: JobState,
    pub event: JobEventKind,
}

/// Whether D3 lets `event` fire while a Job is `from`. `Ok(to)` on a legal
/// move (`to` may equal `from`: several D3 events — `HostJobPinned`,
/// `PauseHandedOff`, `ResumeHandedOff`, `CancelHandedOff`, `ControlFailed`,
/// `MaterialSettled`, `MaterialDeferred`, `MaterialCorrected` — leave the
/// state unchanged); [`IllegalTransition`] otherwise. `event ==
/// JobEventKind::Assigned` is illegal from every state: it is the Job's
/// insert event, not a transition.
pub fn transition(from: JobState, event: JobEventKind) -> Result<JobState, IllegalTransition> {
    use JobEventKind as Event;
    use JobState::{
        Assigned, AwaitingStart, Cancelled, Completed, Failed, OutcomeUnknown, Paused, Printing,
        Staging, Starting,
    };

    let to = match (from, event) {
        (Assigned, Event::StageHandedOff) => Some(Staging),
        (AwaitingStart, Event::StageHandedOff) => Some(Staging),
        (Staging, Event::StageSucceeded) => Some(AwaitingStart),
        (Staging, Event::StageFailed) => Some(Assigned),
        (AwaitingStart, Event::StartHandedOff) => Some(Starting),
        (Starting, Event::StartSucceeded) => Some(Printing),
        (Starting, Event::StartFailed) => Some(AwaitingStart),
        (Starting, Event::StartAbandoned) => Some(OutcomeUnknown),
        (Printing, Event::HostJobPinned) => Some(Printing),
        (Paused, Event::HostJobPinned) => Some(Paused),
        (Printing, Event::PauseHandedOff) => Some(Printing),
        (Paused, Event::ResumeHandedOff) => Some(Paused),
        (Printing, Event::CancelHandedOff) => Some(Printing),
        (Paused, Event::CancelHandedOff) => Some(Paused),
        (Printing, Event::ControlFailed) => Some(Printing),
        (Paused, Event::ControlFailed) => Some(Paused),
        (Printing, Event::Paused) => Some(Paused),
        (Paused, Event::Resumed) => Some(Printing),
        (Printing, Event::Completed) => Some(Completed),
        (Paused, Event::Completed) => Some(Completed),
        (Printing, Event::Failed) => Some(Failed),
        (Paused, Event::Failed) => Some(Failed),
        (Printing, Event::Cancelled) => Some(Cancelled),
        (Paused, Event::Cancelled) => Some(Cancelled),
        (Printing, Event::OutcomeUnknown) => Some(OutcomeUnknown),
        (Paused, Event::OutcomeUnknown) => Some(OutcomeUnknown),
        (OutcomeUnknown, Event::DeclaredCompleted) => Some(Completed),
        (Printing, Event::DeclaredCompleted) => Some(Completed),
        (Paused, Event::DeclaredCompleted) => Some(Completed),
        (OutcomeUnknown, Event::DeclaredFailed) => Some(Failed),
        (Printing, Event::DeclaredFailed) => Some(Failed),
        (Paused, Event::DeclaredFailed) => Some(Failed),
        (OutcomeUnknown, Event::DeclaredCancelled) => Some(Cancelled),
        (Printing, Event::DeclaredCancelled) => Some(Cancelled),
        (Paused, Event::DeclaredCancelled) => Some(Cancelled),
        (Assigned, Event::Released) => Some(Cancelled),
        (AwaitingStart, Event::Released) => Some(Cancelled),
        (Assigned, Event::CancelledBeforeStart) => Some(Cancelled),
        (AwaitingStart, Event::CancelledBeforeStart) => Some(Cancelled),
        (Failed, Event::MaterialSettled) => Some(Failed),
        (Cancelled, Event::MaterialSettled) => Some(Cancelled),
        (Failed, Event::MaterialDeferred) => Some(Failed),
        (Cancelled, Event::MaterialDeferred) => Some(Cancelled),
        (Completed, Event::MaterialCorrected) => Some(Completed),
        _ => None,
    };

    to.ok_or(IllegalTransition { from, event })
}

/// D3's settlement table, keyed on the event alone: the `Settlement` a
/// state-changing event sets, or `None` for an event that leaves it
/// unchanged (including every illegal event, and legal events with no
/// settlement effect — `StageHandedOff`, `HostJobPinned`, and so on).
/// `Settlement::Open` (the Job insert) has no event and is never
/// returned here.
pub fn settlement_after(event: JobEventKind) -> Option<Settlement> {
    match event {
        JobEventKind::Released | JobEventKind::CancelledBeforeStart => {
            Some(Settlement::NotRequired)
        }
        JobEventKind::Completed
        | JobEventKind::DeclaredCompleted
        | JobEventKind::MaterialSettled => Some(Settlement::Settled),
        JobEventKind::Failed
        | JobEventKind::Cancelled
        | JobEventKind::DeclaredFailed
        | JobEventKind::DeclaredCancelled => Some(Settlement::Pending),
        JobEventKind::MaterialDeferred => Some(Settlement::Deferred),
        _ => None,
    }
}

/// D3's "Job actions by state" table: what `Job.allowedActions` lists, in
/// `JobAction` order. `has_successor` is whether the Job's Queue Entry
/// already has a retry (or release replacement); `now` and
/// `declare_after` decide the "unreachable ≥ 30 min" rows (D7, D9).
pub fn allowed_actions(
    job: &Job,
    has_successor: bool,
    now: DateTime<Utc>,
    declare_after: Duration,
) -> Vec<JobAction> {
    use JobAction::*;

    let settleable = matches!(job.settlement, Settlement::Pending | Settlement::Deferred);

    let mut actions = match job.state {
        // D3: in `assigned`, `stage` is offered while `lastFailure` is set
        // or no stage was handed off yet. Every failed or refused stage
        // sets `lastFailure`, and only `StageSucceeded` (which leaves
        // `assigned`) clears it, so an `assigned` Job always qualifies.
        JobState::Assigned => vec![Stage, Cancel, Release],
        JobState::Staging | JobState::Starting => vec![],
        JobState::AwaitingStart => vec![Stage, Start, Cancel, Release],
        JobState::Printing => vec![Pause, Cancel],
        JobState::Paused => vec![Resume, Cancel],
        JobState::OutcomeUnknown => vec![DeclareOutcome],
        JobState::Completed => {
            let mut actions = Vec::new();
            if !has_successor {
                actions.push(Retry);
            }
            if !job.corrected {
                actions.push(CorrectMaterial);
            }
            actions
        }
        JobState::Failed | JobState::Cancelled => {
            let mut actions = Vec::new();
            let released = job.cancel_reason == Some(CancelReason::ReleasedBeforeStart);
            if !has_successor && !released {
                actions.push(Retry);
            }
            if settleable {
                actions.push(SettleMaterial);
            }
            actions
        }
    };
    if may_declare_while_unreachable(job, now, declare_after) {
        actions.push(DeclareOutcome);
    }
    actions
}

/// D9 (ruling R5): a `printing` or `paused` Job whose host has been
/// unreachable for at least `declare_after` at `now` may have its end
/// declared. An unparseable `host_unreachable_since` never qualifies
/// (fail-safe).
pub fn may_declare_while_unreachable(
    job: &Job,
    now: DateTime<Utc>,
    declare_after: Duration,
) -> bool {
    matches!(job.state, JobState::Printing | JobState::Paused)
        && job
            .host_unreachable_since
            .as_deref()
            .and_then(|since| DateTime::parse_from_rfc3339(since).ok())
            .and_then(|since| {
                chrono::Duration::from_std(declare_after)
                    .ok()
                    .map(|after| since.with_timezone(&Utc) + after <= now)
            })
            .unwrap_or(false)
}

/// Fixtures shared by this module's tests and `jobs::dispatch`'s.
#[cfg(test)]
pub(crate) mod tests_support {
    use crate::catalog::{BedShape, PrinterProfile};

    use super::super::{Job, JobState, Settlement};

    pub fn a_profile() -> PrinterProfile {
        PrinterProfile {
            bed_shape: BedShape::Rectangular {
                width_mm: 256.0,
                depth_mm: 256.0,
                origin_x_mm: 0.0,
                origin_y_mm: 0.0,
            },
            printable_height_mm: 256.0,
            bed_exclude_areas: Vec::new(),
            default_bed_type: "PEI".to_string(),
            nozzle_diameter_mm: vec![0.4],
            nozzle_type: "hardened_steel".to_string(),
            gcode_flavor: "klipper".to_string(),
            has_auxiliary_fan: false,
            supports_air_filtration: false,
            supports_multi_filament: false,
            suggested_host_type: None,
            suggested_port: None,
        }
    }

    pub fn a_job(state: JobState) -> Job {
        Job {
            id: "job-a".to_string(),
            revision: 1,
            queue_entry_id: "qen-a".to_string(),
            slice_revision_id: "slr-a".to_string(),
            printer_id: "prn-a".to_string(),
            printer_snapshot: crate::jobs::PrinterSnapshot {
                name: "A".to_string(),
                location: None,
                catalog_ref: None,
                adapter_kind: None,
                profile: a_profile(),
            },
            spool_id: "spl-a".to_string(),
            reservation_id: "rsv-a".to_string(),
            estimate_mg: 1000,
            state,
            cancel_reason: None,
            settlement: if state.is_terminal() {
                Settlement::Pending
            } else {
                Settlement::Open
            },
            settlement_method: None,
            settlement_preview: None,
            corrected: false,
            assigned_by: crate::jobs::AssignedBy::Operator,
            start_confirmation: None,
            upload_host_operation_id: None,
            active_host_operation_id: None,
            max_progress_pct: 0,
            host_unreachable_since: None,
            host_path: None,
            last_failure: None,
            start_blockers: Vec::new(),
            allowed_actions: Vec::new(),
            created_at: String::new(),
            updated_at: String::new(),
            started_at: None,
            ended_at: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::a_job;
    use super::*;
    use JobState::{
        Assigned, AwaitingStart, Cancelled, Completed, Failed, OutcomeUnknown, Paused, Printing,
        Staging, Starting,
    };

    const THIRTY_MINUTES: Duration = Duration::from_secs(30 * 60);

    fn now() -> DateTime<Utc> {
        "2026-09-27T12:00:00Z".parse().unwrap()
    }

    fn actions(job: &Job) -> Vec<JobAction> {
        allowed_actions(job, false, now(), THIRTY_MINUTES)
    }

    /// D3's table, row by row, for a Job with nothing special about it
    /// (reachable host, not retried, settlement as its state implies).
    #[test]
    fn allowed_actions_follow_d3s_table_by_state() {
        use JobAction::*;
        let table: [(JobState, &[JobAction]); 10] = [
            (Assigned, &[Stage, Cancel, Release]),
            (Staging, &[]),
            (AwaitingStart, &[Stage, Start, Cancel, Release]),
            (Starting, &[]),
            (Printing, &[Pause, Cancel]),
            (Paused, &[Resume, Cancel]),
            (OutcomeUnknown, &[DeclareOutcome]),
            (Completed, &[Retry, CorrectMaterial]),
            (Failed, &[Retry, SettleMaterial]),
            (Cancelled, &[Retry, SettleMaterial]),
        ];
        for (state, expected) in table {
            let mut job = a_job(state);
            if state == Completed {
                job.settlement = Settlement::Settled;
            }
            if state == Cancelled {
                job.cancel_reason = Some(CancelReason::HostCancelled);
            }
            assert_eq!(actions(&job), expected, "{state:?}");
        }
    }

    #[test]
    fn retry_disappears_once_retried_and_never_offered_after_release() {
        let mut failed = a_job(Failed);
        assert!(allowed_actions(&failed, false, now(), THIRTY_MINUTES).contains(&JobAction::Retry));
        assert!(!allowed_actions(&failed, true, now(), THIRTY_MINUTES).contains(&JobAction::Retry));
        failed.state = Cancelled;
        failed.cancel_reason = Some(CancelReason::ReleasedBeforeStart);
        failed.settlement = Settlement::NotRequired;
        assert_eq!(actions(&failed), Vec::<JobAction>::new());
    }

    #[test]
    fn settle_is_offered_only_while_pending_or_deferred() {
        for (settlement, offered) in [
            (Settlement::Pending, true),
            (Settlement::Deferred, true),
            (Settlement::Settled, false),
            (Settlement::NotRequired, false),
        ] {
            let mut job = a_job(Cancelled);
            job.cancel_reason = Some(CancelReason::CancelledByOperator);
            job.settlement = settlement;
            assert_eq!(
                actions(&job).contains(&JobAction::SettleMaterial),
                offered,
                "{settlement:?}"
            );
        }
    }

    #[test]
    fn correct_material_is_offered_until_corrected() {
        let mut job = a_job(Completed);
        job.settlement = Settlement::Settled;
        assert!(actions(&job).contains(&JobAction::CorrectMaterial));
        job.corrected = true;
        assert!(!actions(&job).contains(&JobAction::CorrectMaterial));
    }

    /// D3/D9: `declareOutcome` appears in `printing`/`paused` only once
    /// the host has been unreachable for at least 30 minutes.
    #[test]
    fn declare_outcome_appears_after_thirty_minutes_unreachable() {
        for state in [Printing, Paused] {
            let mut job = a_job(state);
            job.host_unreachable_since = Some("2026-09-27T11:31:00Z".to_string());
            assert!(
                !actions(&job).contains(&JobAction::DeclareOutcome),
                "{state:?} at 29 min"
            );
            job.host_unreachable_since = Some("2026-09-27T11:30:00Z".to_string());
            assert!(
                actions(&job).contains(&JobAction::DeclareOutcome),
                "{state:?} at 30 min"
            );
            // Unparseable timestamps never offer it (fail-safe).
            job.host_unreachable_since = Some("garbage".to_string());
            assert!(
                !actions(&job).contains(&JobAction::DeclareOutcome),
                "{state:?}"
            );
        }
    }

    /// D3's event table, copied verbatim as `(from, event, to)` triples.
    /// `Assigned` (the insert event) is deliberately absent: every pair
    /// naming it must resolve to `None` (illegal) below.
    const SPEC_TABLE: [(JobState, JobEventKind, JobState); 44] = [
        (Assigned, JobEventKind::StageHandedOff, Staging),
        (AwaitingStart, JobEventKind::StageHandedOff, Staging),
        (Staging, JobEventKind::StageSucceeded, AwaitingStart),
        (Staging, JobEventKind::StageFailed, Assigned),
        (AwaitingStart, JobEventKind::StartHandedOff, Starting),
        (Starting, JobEventKind::StartSucceeded, Printing),
        (Starting, JobEventKind::StartFailed, AwaitingStart),
        (Starting, JobEventKind::StartAbandoned, OutcomeUnknown),
        (Printing, JobEventKind::HostJobPinned, Printing),
        (Paused, JobEventKind::HostJobPinned, Paused),
        (Printing, JobEventKind::PauseHandedOff, Printing),
        (Paused, JobEventKind::ResumeHandedOff, Paused),
        (Printing, JobEventKind::CancelHandedOff, Printing),
        (Paused, JobEventKind::CancelHandedOff, Paused),
        (Printing, JobEventKind::ControlFailed, Printing),
        (Paused, JobEventKind::ControlFailed, Paused),
        (Printing, JobEventKind::Paused, Paused),
        (Paused, JobEventKind::Resumed, Printing),
        (Printing, JobEventKind::Completed, Completed),
        (Paused, JobEventKind::Completed, Completed),
        (Printing, JobEventKind::Failed, Failed),
        (Paused, JobEventKind::Failed, Failed),
        (Printing, JobEventKind::Cancelled, Cancelled),
        (Paused, JobEventKind::Cancelled, Cancelled),
        (Printing, JobEventKind::OutcomeUnknown, OutcomeUnknown),
        (Paused, JobEventKind::OutcomeUnknown, OutcomeUnknown),
        (OutcomeUnknown, JobEventKind::DeclaredCompleted, Completed),
        (Printing, JobEventKind::DeclaredCompleted, Completed),
        (Paused, JobEventKind::DeclaredCompleted, Completed),
        (OutcomeUnknown, JobEventKind::DeclaredFailed, Failed),
        (Printing, JobEventKind::DeclaredFailed, Failed),
        (Paused, JobEventKind::DeclaredFailed, Failed),
        (OutcomeUnknown, JobEventKind::DeclaredCancelled, Cancelled),
        (Printing, JobEventKind::DeclaredCancelled, Cancelled),
        (Paused, JobEventKind::DeclaredCancelled, Cancelled),
        (Assigned, JobEventKind::Released, Cancelled),
        (AwaitingStart, JobEventKind::Released, Cancelled),
        (Assigned, JobEventKind::CancelledBeforeStart, Cancelled),
        (AwaitingStart, JobEventKind::CancelledBeforeStart, Cancelled),
        (Failed, JobEventKind::MaterialSettled, Failed),
        (Cancelled, JobEventKind::MaterialSettled, Cancelled),
        (Failed, JobEventKind::MaterialDeferred, Failed),
        (Cancelled, JobEventKind::MaterialDeferred, Cancelled),
        (Completed, JobEventKind::MaterialCorrected, Completed),
    ];

    /// Every `(JobState, JobEventKind)` pair, 10 states x 27 events,
    /// exhaustively matched against `SPEC_TABLE` — legal and illegal
    /// pairs included.
    #[test]
    fn every_job_transition_pair_matches_the_spec_table() {
        for from in JobState::ALL {
            for event in JobEventKind::ALL {
                let expected = SPEC_TABLE
                    .iter()
                    .find(|(f, e, _)| *f == from && *e == event)
                    .map(|(_, _, to)| *to);
                assert_eq!(
                    transition(from, event).ok(),
                    expected,
                    "{from:?} + {event:?}"
                );
            }
        }
    }

    /// `Assigned` is the insert event, not a transition: illegal from
    /// every state.
    #[test]
    fn assigned_is_illegal_from_every_state() {
        for from in JobState::ALL {
            assert!(
                transition(from, JobEventKind::Assigned).is_err(),
                "Assigned must be illegal from {from:?}"
            );
        }
    }

    /// A terminal `JobState` (`completed`, `failed`, `cancelled`) accepts
    /// only its material events, and each one leaves the state unchanged:
    /// `MaterialSettled`/`MaterialDeferred` from `failed`/`cancelled`,
    /// `MaterialCorrected` from `completed`.
    #[test]
    fn terminal_states_accept_only_their_material_events_unchanged() {
        for (terminal, legal_events) in [
            (
                Failed,
                &[
                    JobEventKind::MaterialSettled,
                    JobEventKind::MaterialDeferred,
                ][..],
            ),
            (
                Cancelled,
                &[
                    JobEventKind::MaterialSettled,
                    JobEventKind::MaterialDeferred,
                ][..],
            ),
            (Completed, &[JobEventKind::MaterialCorrected][..]),
        ] {
            for event in JobEventKind::ALL {
                let result = transition(terminal, event);
                if legal_events.contains(&event) {
                    assert_eq!(
                        result,
                        Ok(terminal),
                        "{event:?} must leave {terminal:?} unchanged"
                    );
                } else {
                    assert!(
                        result.is_err(),
                        "{event:?} must be illegal from terminal state {terminal:?}"
                    );
                }
            }
        }
    }

    /// The three `Declared*` events are legal exactly from
    /// `outcomeUnknown`, `printing`, and `paused` (D3: `declare_job_outcome`
    /// may declare from `printing`/`paused` only while the host has been
    /// unreachable for 30 minutes — D9's own guard, not part of this pure
    /// state machine, which only knows the three states are legal).
    #[test]
    fn declared_events_are_legal_only_from_outcome_unknown_printing_or_paused() {
        let legal_from = [OutcomeUnknown, Printing, Paused];
        for event in [
            JobEventKind::DeclaredCompleted,
            JobEventKind::DeclaredFailed,
            JobEventKind::DeclaredCancelled,
        ] {
            for from in JobState::ALL {
                let result = transition(from, event);
                if legal_from.contains(&from) {
                    assert!(result.is_ok(), "{event:?} must be legal from {from:?}");
                } else {
                    assert!(result.is_err(), "{event:?} must be illegal from {from:?}");
                }
            }
        }
    }

    #[test]
    fn settlement_after_matches_d3s_settlement_table() {
        assert_eq!(
            settlement_after(JobEventKind::Released),
            Some(Settlement::NotRequired)
        );
        assert_eq!(
            settlement_after(JobEventKind::CancelledBeforeStart),
            Some(Settlement::NotRequired)
        );
        assert_eq!(
            settlement_after(JobEventKind::Completed),
            Some(Settlement::Settled)
        );
        assert_eq!(
            settlement_after(JobEventKind::DeclaredCompleted),
            Some(Settlement::Settled)
        );
        assert_eq!(
            settlement_after(JobEventKind::MaterialSettled),
            Some(Settlement::Settled)
        );
        assert_eq!(
            settlement_after(JobEventKind::Failed),
            Some(Settlement::Pending)
        );
        assert_eq!(
            settlement_after(JobEventKind::Cancelled),
            Some(Settlement::Pending)
        );
        assert_eq!(
            settlement_after(JobEventKind::DeclaredFailed),
            Some(Settlement::Pending)
        );
        assert_eq!(
            settlement_after(JobEventKind::DeclaredCancelled),
            Some(Settlement::Pending)
        );
        assert_eq!(
            settlement_after(JobEventKind::MaterialDeferred),
            Some(Settlement::Deferred)
        );
        // Everything else leaves settlement unchanged.
        for event in JobEventKind::ALL {
            if ![
                JobEventKind::Released,
                JobEventKind::CancelledBeforeStart,
                JobEventKind::Completed,
                JobEventKind::DeclaredCompleted,
                JobEventKind::MaterialSettled,
                JobEventKind::Failed,
                JobEventKind::Cancelled,
                JobEventKind::DeclaredFailed,
                JobEventKind::DeclaredCancelled,
                JobEventKind::MaterialDeferred,
            ]
            .contains(&event)
            {
                assert_eq!(settlement_after(event), None, "{event:?} must be unchanged");
            }
        }
    }
}
