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

use super::{JobEventKind, JobState, Settlement};

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

#[cfg(test)]
mod tests {
    use super::*;
    use JobState::{
        Assigned, AwaitingStart, Cancelled, Completed, Failed, OutcomeUnknown, Paused, Printing,
        Staging, Starting,
    };

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
