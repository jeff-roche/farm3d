//! P7 D2: the Queue Entry state machine, as a pure function so the
//! repository can reject an illegal move before it ever reaches SQL. See
//! the design spec's D2 "Legal and illegal transitions" table — this
//! module's `SPEC_TABLE` test fixture is a direct copy of it.
//!
//! ```text
//! queued   ─ Assign ──────────────────> assigned
//! queued   ─ Remove ──────────────────> closed
//! assigned ─ JobTerminal(completed) ───> closed
//! assigned ─ JobTerminal(failed) ──────> closed
//! assigned ─ JobTerminal(cancelled) ───> closed
//! assigned ─ Release ──────────────────> closed
//! ```
//!
//! Every other `(state, event)` pair is illegal. This module only checks
//! state *legality*; which `closeReason`/`position` a legal move writes is
//! `queue::repository`'s job (a later task).

use super::{CloseReason, QueueEntryState};

/// D2's events. `JobTerminal` only ever carries `Completed`, `Failed`, or
/// `Cancelled` — a Job reaches a terminal state through one of those three
/// reasons; `Released` and `Removed` are their own direct events
/// ([`EntryEvent::Release`], [`EntryEvent::Remove`]), never a
/// `JobTerminal` payload. Retry is not an entry event: it creates a new
/// entry and leaves its origin untouched (D1).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EntryEvent {
    /// `queued -> assigned`: the entry is assigned to a Printer.
    Assign,
    /// `queued -> closed{removed}`: the operator removes an unassigned
    /// entry.
    Remove,
    /// `assigned -> closed{<reason>}`: the entry's Job reached a terminal
    /// state.
    JobTerminal(CloseReason),
    /// `assigned -> closed{released}`: the entry's Job is released before
    /// start.
    Release,
}

impl EntryEvent {
    pub const ALL: [EntryEvent; 6] = [
        EntryEvent::Assign,
        EntryEvent::Remove,
        EntryEvent::JobTerminal(CloseReason::Completed),
        EntryEvent::JobTerminal(CloseReason::Failed),
        EntryEvent::JobTerminal(CloseReason::Cancelled),
        EntryEvent::Release,
    ];
}

/// `transition`'s rejection: `event` never legally fires from `from`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IllegalTransition {
    pub from: QueueEntryState,
    pub event: EntryEvent,
}

/// Whether D2 lets `event` fire while an entry is `from`. `Ok(to)` on a
/// legal move; [`IllegalTransition`] otherwise (an illegal pair reached
/// from a user command maps to `QUEUE_ENTRY_ACTION_NOT_ALLOWED`, and one
/// reached from farm3d's own code to `INTERNAL` — the design spec's D2
/// table, applied by a later task's callers).
pub fn transition(
    from: QueueEntryState,
    event: &EntryEvent,
) -> Result<QueueEntryState, IllegalTransition> {
    use QueueEntryState::{Assigned, Closed, Queued};

    let to = match (from, event) {
        (Queued, EntryEvent::Assign) => Some(Assigned),
        (Queued, EntryEvent::Remove) => Some(Closed),
        (Assigned, EntryEvent::JobTerminal(_)) => Some(Closed),
        (Assigned, EntryEvent::Release) => Some(Closed),
        _ => None,
    };

    to.ok_or(IllegalTransition {
        from,
        event: *event,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use QueueEntryState::{Assigned, Closed, Queued};

    /// D2's table, copied verbatim: every legal `(from, event) -> to`
    /// triple. Every pair not listed here is illegal.
    const SPEC_TABLE: [(QueueEntryState, EntryEvent, QueueEntryState); 6] = [
        (Queued, EntryEvent::Assign, Assigned),
        (Queued, EntryEvent::Remove, Closed),
        (
            Assigned,
            EntryEvent::JobTerminal(CloseReason::Completed),
            Closed,
        ),
        (
            Assigned,
            EntryEvent::JobTerminal(CloseReason::Failed),
            Closed,
        ),
        (
            Assigned,
            EntryEvent::JobTerminal(CloseReason::Cancelled),
            Closed,
        ),
        (Assigned, EntryEvent::Release, Closed),
    ];

    /// Every `(QueueEntryState, EntryEvent)` pair, 3 states x 6 events,
    /// exhaustively matched against `SPEC_TABLE` — legal and illegal pairs
    /// included.
    #[test]
    fn every_queue_entry_transition_pair_matches_the_spec_table() {
        for from in QueueEntryState::ALL {
            for event in EntryEvent::ALL {
                let expected = SPEC_TABLE
                    .iter()
                    .find(|(f, e, _)| *f == from && *e == event)
                    .map(|(_, _, to)| *to);
                assert_eq!(
                    transition(from, &event).ok(),
                    expected,
                    "{from:?} + {event:?}"
                );
            }
        }
    }

    #[test]
    fn illegal_transitions_report_the_state_and_event_attempted() {
        let error = transition(Queued, &EntryEvent::Release).unwrap_err();
        assert_eq!(
            error,
            IllegalTransition {
                from: Queued,
                event: EntryEvent::Release,
            }
        );
    }

    #[test]
    fn closed_is_terminal_and_rejects_every_event() {
        for event in EntryEvent::ALL {
            assert!(
                transition(Closed, &event).is_err(),
                "{event:?} must be illegal from closed"
            );
        }
    }
}
