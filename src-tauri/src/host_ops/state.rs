//! P6 D3: the Host Operation state machine, as a pure function so the
//! repository can reject an illegal move before it ever reaches SQL.
//!
//! ```text
//! dispatching ─ definitive success ────────────> succeeded
//! dispatching ─ definitive failure ────────────> failed
//! dispatching ─ indeterminate/timeout/panic ───> uncertain
//! uncertain ─ reconcile attempt begins ────────> reconciling
//! reconciling ─ proved applied ────────────────> succeeded
//! reconciling ─ proved not applied (upload) ───> failed
//! reconciling ─ inconclusive/startup ──────────> uncertain
//! uncertain ─ operator abandons ────────────────> abandoned
//! ```
//!
//! Every other `(from, to)` pair is illegal. This module only checks state
//! *legality* — which companion columns a move writes (`resolution_json`,
//! `uncertain_since`, `attempts`, ...) is `repository.rs`'s job.

use super::HostOperationState;

/// `transition`'s rejection: `from` never legally moves to `to`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct IllegalTransition {
    pub from: HostOperationState,
    pub to: HostOperationState,
}

/// Whether D3 allows `from -> to`. `Ok(to)` on a legal move (returned
/// unchanged, so callers can chain this straight into their own result);
/// [`IllegalTransition`] otherwise.
pub fn transition(
    from: HostOperationState,
    to: HostOperationState,
) -> Result<HostOperationState, IllegalTransition> {
    use HostOperationState::{Abandoned, Dispatching, Failed, Reconciling, Succeeded, Uncertain};

    let legal = matches!(
        (from, to),
        (Dispatching, Succeeded)
            | (Dispatching, Failed)
            | (Dispatching, Uncertain)
            | (Uncertain, Reconciling)
            | (Uncertain, Abandoned)
            | (Reconciling, Succeeded)
            | (Reconciling, Failed)
            | (Reconciling, Uncertain)
    );

    if legal {
        Ok(to)
    } else {
        Err(IllegalTransition { from, to })
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

    /// D3's table, exhaustively: every legal `(from, to)` pair the diagram
    /// draws an arrow for, and nothing else.
    const LEGAL_EDGES: [(HostOperationState, HostOperationState); 8] = [
        (Dispatching, Succeeded),
        (Dispatching, Failed),
        (Dispatching, Uncertain),
        (Uncertain, Reconciling),
        (Uncertain, Abandoned),
        (Reconciling, Succeeded),
        (Reconciling, Failed),
        (Reconciling, Uncertain),
    ];

    #[test]
    fn every_legal_edge_succeeds_and_returns_its_target() {
        for (from, to) in LEGAL_EDGES {
            assert_eq!(
                transition(from, to),
                Ok(to),
                "{from:?} -> {to:?} must be legal"
            );
        }
    }

    #[test]
    fn every_other_pair_in_the_full_cross_product_is_illegal() {
        let mut checked = 0;
        for from in ALL_STATES {
            for to in ALL_STATES {
                if LEGAL_EDGES.contains(&(from, to)) {
                    continue;
                }
                checked += 1;
                assert_eq!(
                    transition(from, to),
                    Err(IllegalTransition { from, to }),
                    "{from:?} -> {to:?} must be illegal"
                );
            }
        }
        // 6 states x 6 states minus the 8 legal edges.
        assert_eq!(checked, 36 - 8);
    }

    #[test]
    fn terminal_states_never_move_anywhere_including_themselves() {
        for terminal in [Succeeded, Failed, Abandoned] {
            for to in ALL_STATES {
                assert!(transition(terminal, to).is_err());
            }
        }
    }
}
