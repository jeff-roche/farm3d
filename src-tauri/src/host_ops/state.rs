//! P6 D3: the Host Operation state machine, as a pure function so the
//! repository can reject an illegal move before it ever reaches SQL.
//!
//! ```text
//! dispatching ─ definitive success ────────────> succeeded         (DefinitiveSuccess)
//! dispatching ─ definitive failure ────────────> failed            (DefinitiveFailure)
//! dispatching ─ indeterminate/timeout/panic ───> uncertain         (Indeterminate)
//! dispatching ─ startup, never sent ───────────> failed{neverSent} (StartupNeverSent)
//! dispatching ─ startup, sent ─────────────────> uncertain         (StartupSent)
//! uncertain ─ reconcile attempt begins ────────> reconciling       (AttemptBegins)
//! uncertain ─ operator abandons ────────────────> abandoned        (Abandon)
//! reconciling ─ proved applied ────────────────> succeeded         (ProvedApplied)
//! reconciling ─ proved not applied (upload) ───> failed            (ProvedNotApplied)
//! reconciling ─ inconclusive ──────────────────> uncertain         (Inconclusive)
//! reconciling ─ startup ───────────────────────> uncertain         (StartupReconciling)
//! ```
//!
//! The spec binds `transition(from, event)`, not `transition(from, to)`:
//! each event names exactly one D3 edge (its own fixed source and target
//! state), so `reconciling -> uncertain` and `dispatching -> uncertain`
//! are different events even though they share a target. That matters
//! because they aren't interchangeable — a reconciler's inconclusive
//! attempt ([`Event::Inconclusive`]) counts an attempt and leaves
//! `uncertain_since` alone, while the executor's indeterminate dispatch
//! result ([`Event::Indeterminate`]) is `dispatching`-only and is the one
//! and only place `uncertain_since` is written (D2). A `(from, to)`-keyed
//! table couldn't tell those apart — see `repository.rs`'s
//! `record_attempt` vs. its `transition`/`Outcome::Uncertain`.
//!
//! Every other `(from, event)` pair is illegal. This module only checks
//! state *legality* — which companion columns a move writes
//! (`resolution_json`, `uncertain_since`, `attempts`, ...), and any rule
//! beyond state (e.g. D3's "proved not applied" is upload-only) is
//! `repository.rs`'s job.

use super::HostOperationState;

/// D3's events, each legal from exactly one state. `AttemptBegins` and
/// `Abandon` are `uncertain`'s two exits; every other event is
/// `dispatching`'s or `reconciling`'s.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Event {
    /// `dispatching -> succeeded`: the executor's dispatch answer proved
    /// the write applied.
    DefinitiveSuccess,
    /// `dispatching -> failed`: the executor's dispatch answer proved the
    /// write did not apply (or a local precondition failed before it sent
    /// anything).
    DefinitiveFailure,
    /// `dispatching -> uncertain`: the executor's dispatch answer proved
    /// neither (timeout, lost response, panic, ...).
    Indeterminate,
    /// `uncertain -> reconciling`: a reconcile attempt begins.
    AttemptBegins,
    /// `reconciling -> succeeded`: the reconcile read proved the write
    /// applied.
    ProvedApplied,
    /// `reconciling -> failed`: the reconcile read proved the write did
    /// not apply. D3/repository rule: only ever raised for an `upload`
    /// row (`repository::transition` enforces this; this module doesn't
    /// know about `kind`).
    ProvedNotApplied,
    /// `reconciling -> uncertain`: the reconcile read proved neither.
    /// Counts an attempt (`repository::record_attempt`).
    Inconclusive,
    /// `dispatching -> failed{neverSent}`: startup found the row unsent.
    StartupNeverSent,
    /// `dispatching -> uncertain`: startup found the row sent, but with no
    /// answer recorded.
    StartupSent,
    /// `reconciling -> uncertain`: startup found a reconcile attempt in
    /// progress (a crash mid-attempt). Never counts an attempt.
    StartupReconciling,
    /// `uncertain -> abandoned`: the operator abandons (D8).
    Abandon,
}

impl Event {
    /// The one `(from, to)` edge this event is legal for.
    fn edge(self) -> (HostOperationState, HostOperationState) {
        use HostOperationState::{
            Abandoned, Dispatching, Failed, Reconciling, Succeeded, Uncertain,
        };
        match self {
            Event::DefinitiveSuccess => (Dispatching, Succeeded),
            Event::DefinitiveFailure => (Dispatching, Failed),
            Event::Indeterminate => (Dispatching, Uncertain),
            Event::StartupNeverSent => (Dispatching, Failed),
            Event::StartupSent => (Dispatching, Uncertain),
            Event::AttemptBegins => (Uncertain, Reconciling),
            Event::Abandon => (Uncertain, Abandoned),
            Event::ProvedApplied => (Reconciling, Succeeded),
            Event::ProvedNotApplied => (Reconciling, Failed),
            Event::Inconclusive => (Reconciling, Uncertain),
            Event::StartupReconciling => (Reconciling, Uncertain),
        }
    }
}

/// `transition`'s rejection: `event` never legally fires from `from`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IllegalTransition {
    pub from: HostOperationState,
    pub event: Event,
}

impl IllegalTransition {
    /// The state `event` would have moved to, had it been legal — for a
    /// caller (the repository) that wants to report *what* was attempted,
    /// not just that it failed.
    pub fn attempted_target(&self) -> HostOperationState {
        self.event.edge().1
    }
}

/// Whether D3 lets `event` fire while a row is `from`. `Ok(to)` on a
/// legal move (the event's fixed target, returned so callers can chain
/// this straight into their own result); [`IllegalTransition`] otherwise.
pub fn transition(
    from: HostOperationState,
    event: Event,
) -> Result<HostOperationState, IllegalTransition> {
    let (required_from, to) = event.edge();
    if from == required_from {
        Ok(to)
    } else {
        Err(IllegalTransition { from, event })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use HostOperationState::{Abandoned, Dispatching, Failed, Reconciling, Succeeded, Uncertain};

    const ALL_STATES: [HostOperationState; 6] = [
        Dispatching,
        Uncertain,
        Reconciling,
        Succeeded,
        Failed,
        Abandoned,
    ];

    const ALL_EVENTS: [Event; 11] = [
        Event::DefinitiveSuccess,
        Event::DefinitiveFailure,
        Event::Indeterminate,
        Event::StartupNeverSent,
        Event::StartupSent,
        Event::AttemptBegins,
        Event::Abandon,
        Event::ProvedApplied,
        Event::ProvedNotApplied,
        Event::Inconclusive,
        Event::StartupReconciling,
    ];

    /// Every event, from every state: legal only from the one state its
    /// `edge()` names, and always returns that edge's target there.
    #[test]
    fn every_event_is_legal_only_from_its_one_required_state() {
        for event in ALL_EVENTS {
            let (required_from, to) = event.edge();
            for from in ALL_STATES {
                let result = transition(from, event);
                if from == required_from {
                    assert_eq!(result, Ok(to), "{event:?} must be legal from {from:?}");
                } else {
                    assert_eq!(
                        result,
                        Err(IllegalTransition { from, event }),
                        "{event:?} must be illegal from {from:?}"
                    );
                }
            }
        }
    }

    /// D3's two same-target-different-source pairs stay distinct events,
    /// so a `reconciling` row can never take the `dispatching`-only
    /// `Indeterminate`/`StartupSent`/`StartupNeverSent` route to
    /// `uncertain`/`failed` — only its own `Inconclusive`/
    /// `StartupReconciling` (this is the fix for the bug where a
    /// `(from, to)`-keyed table let `reconciling -> uncertain` through
    /// the executor's event and reset `uncertain_since`).
    #[test]
    fn reconciling_never_takes_a_dispatching_only_event_to_uncertain_or_failed() {
        for event in [
            Event::Indeterminate,
            Event::StartupSent,
            Event::StartupNeverSent,
            Event::DefinitiveSuccess,
            Event::DefinitiveFailure,
        ] {
            assert!(
                transition(Reconciling, event).is_err(),
                "{event:?} must be illegal from reconciling"
            );
        }
    }

    #[test]
    fn terminal_states_reject_every_event() {
        for terminal in [Succeeded, Failed, Abandoned] {
            for event in ALL_EVENTS {
                assert!(
                    transition(terminal, event).is_err(),
                    "{event:?} must be illegal from terminal state {terminal:?}"
                );
            }
        }
    }

    #[test]
    fn attempted_target_reports_what_the_illegal_event_would_have_reached() {
        let error = transition(Dispatching, Event::AttemptBegins).unwrap_err();
        assert_eq!(error.attempted_target(), Reconciling);
    }
}
