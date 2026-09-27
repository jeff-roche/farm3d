//! D9: the Start rule and the control rule, pure, over the live
//! `PrinterStatus` the `ConnectionManager` holds.
//!
//! | `OperationalState` (fresh) | Start allowed with `priorState` |
//! |---|---|
//! | `Ready` | `ready` |
//! | `Finished` | `finished` |
//! | `Cancelled` | `cancelled` |
//! | anything else, or freshness not `fresh` | never |
//!
//! Control: pause only from `Printing`, resume only from `Paused`, cancel
//! from either, all with fresh telemetry.

use crate::connections::PrinterStatus;
use crate::printers::operational::{OperationalState, TelemetryFreshness};

use super::PriorState;

/// Why `check` refused a start. Both carry what the live status showed, so
/// the command can report it (`START_NOT_ALLOWED` /
/// `START_PRECONDITION_CHANGED`).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum StartRejection {
    /// The state is not one Start is offered from, or telemetry is not
    /// fresh.
    NotAllowed {
        observed_state: OperationalState,
        freshness: TelemetryFreshness,
    },
    /// Start is offered, but not for the `priorState` the operator
    /// confirmed (the bed-clear confirmation named another state).
    PreconditionChanged {
        observed_state: OperationalState,
        freshness: TelemetryFreshness,
    },
}

/// The `priorState` Start is offered with from `state`, if any.
fn offered_prior(state: OperationalState) -> Option<PriorState> {
    match state {
        OperationalState::Ready => Some(PriorState::Ready),
        OperationalState::Finished => Some(PriorState::Finished),
        OperationalState::Cancelled => Some(PriorState::Cancelled),
        _ => None,
    }
}

/// D9's Start table.
pub fn check(status: &PrinterStatus, prior: PriorState) -> Result<(), StartRejection> {
    let observed_state = status.operational_state;
    let freshness = status.freshness;
    let offered = offered_prior(observed_state).filter(|_| freshness == TelemetryFreshness::Fresh);
    match offered {
        None => Err(StartRejection::NotAllowed {
            observed_state,
            freshness,
        }),
        Some(offered) if offered != prior => Err(StartRejection::PreconditionChanged {
            observed_state,
            freshness,
        }),
        Some(_) => Ok(()),
    }
}

/// A control verb (D9 control rule).
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum ControlVerb {
    Pause,
    Resume,
    Cancel,
}

impl ControlVerb {
    /// The wire spelling (`CONTROL_NOT_ALLOWED.details.verb`).
    pub fn as_str(self) -> &'static str {
        match self {
            ControlVerb::Pause => "pause",
            ControlVerb::Resume => "resume",
            ControlVerb::Cancel => "cancel",
        }
    }

    /// Whether the verb is offered while the Printer is in `state`.
    pub fn allows(self, state: OperationalState) -> bool {
        matches!(
            (self, state),
            (ControlVerb::Pause, OperationalState::Printing)
                | (ControlVerb::Resume, OperationalState::Paused)
                | (
                    ControlVerb::Cancel,
                    OperationalState::Printing | OperationalState::Paused
                )
        )
    }
}

/// `check_control`'s refusal: what the live status showed.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct ControlRejection {
    pub observed_state: OperationalState,
    pub freshness: TelemetryFreshness,
}

/// D9's control rule on the live status.
pub fn check_control(status: &PrinterStatus, verb: ControlVerb) -> Result<(), ControlRejection> {
    if status.freshness == TelemetryFreshness::Fresh && verb.allows(status.operational_state) {
        Ok(())
    } else {
        Err(ControlRejection {
            observed_state: status.operational_state,
            freshness: status.freshness,
        })
    }
}

/// The short state name the `START_NOT_ALLOWED` and `CONTROL_NOT_ALLOWED`
/// messages end with. Telemetry that isn't fresh is named as such, since
/// that, not the state, is the reason.
pub fn state_label(state: OperationalState, freshness: TelemetryFreshness) -> &'static str {
    if freshness != TelemetryFreshness::Fresh {
        return "its status is out of date";
    }
    match state {
        OperationalState::SetupIncomplete => "setup is incomplete",
        OperationalState::Error => "it reports an error",
        OperationalState::Offline => "it is offline",
        OperationalState::Connecting => "it is still connecting",
        OperationalState::Unknown => "its state is unknown",
        OperationalState::Printing => "it is printing",
        OperationalState::Paused => "it is paused",
        OperationalState::Busy => "it is busy",
        OperationalState::Finished => "the last print finished",
        OperationalState::Cancelled => "the last print was cancelled",
        OperationalState::Failed => "the last print failed",
        OperationalState::Ready => "it is ready",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::status_repository::ToolTemperature;
    use crate::connections::ConnectionState;

    const ALL_STATES: [OperationalState; 12] = [
        OperationalState::SetupIncomplete,
        OperationalState::Error,
        OperationalState::Offline,
        OperationalState::Connecting,
        OperationalState::Unknown,
        OperationalState::Printing,
        OperationalState::Paused,
        OperationalState::Busy,
        OperationalState::Finished,
        OperationalState::Cancelled,
        OperationalState::Failed,
        OperationalState::Ready,
    ];
    const ALL_PRIORS: [PriorState; 3] = [
        PriorState::Ready,
        PriorState::Finished,
        PriorState::Cancelled,
    ];

    fn status(state: OperationalState, freshness: TelemetryFreshness) -> PrinterStatus {
        let mut status = PrinterStatus::new(ConnectionState::Online);
        status.operational_state = state;
        status.freshness = freshness;
        status
    }

    fn fresh(state: OperationalState) -> PrinterStatus {
        status(state, TelemetryFreshness::Fresh)
    }

    fn not_allowed(state: OperationalState, freshness: TelemetryFreshness) -> StartRejection {
        StartRejection::NotAllowed {
            observed_state: state,
            freshness,
        }
    }

    #[test]
    fn ready_allows_only_prior_ready() {
        assert_eq!(
            check(&fresh(OperationalState::Ready), PriorState::Ready),
            Ok(())
        );
        for prior in [PriorState::Finished, PriorState::Cancelled] {
            assert_eq!(
                check(&fresh(OperationalState::Ready), prior),
                Err(StartRejection::PreconditionChanged {
                    observed_state: OperationalState::Ready,
                    freshness: TelemetryFreshness::Fresh,
                })
            );
        }
    }

    #[test]
    fn finished_allows_only_prior_finished() {
        assert_eq!(
            check(&fresh(OperationalState::Finished), PriorState::Finished),
            Ok(())
        );
        assert_eq!(
            check(&fresh(OperationalState::Finished), PriorState::Ready),
            Err(StartRejection::PreconditionChanged {
                observed_state: OperationalState::Finished,
                freshness: TelemetryFreshness::Fresh,
            })
        );
    }

    #[test]
    fn cancelled_allows_only_prior_cancelled() {
        assert_eq!(
            check(&fresh(OperationalState::Cancelled), PriorState::Cancelled),
            Ok(())
        );
        assert_eq!(
            check(&fresh(OperationalState::Cancelled), PriorState::Finished),
            Err(StartRejection::PreconditionChanged {
                observed_state: OperationalState::Cancelled,
                freshness: TelemetryFreshness::Fresh,
            })
        );
    }

    /// Answer 13: never after `Failed`, whatever the operator confirmed.
    #[test]
    fn failed_is_never_allowed() {
        for prior in ALL_PRIORS {
            assert_eq!(
                check(&fresh(OperationalState::Failed), prior),
                Err(not_allowed(
                    OperationalState::Failed,
                    TelemetryFreshness::Fresh
                ))
            );
        }
    }

    #[test]
    fn every_other_state_is_not_allowed() {
        for state in [
            OperationalState::Printing,
            OperationalState::Paused,
            OperationalState::Busy,
            OperationalState::Offline,
            OperationalState::Connecting,
            OperationalState::Unknown,
            OperationalState::Error,
            OperationalState::SetupIncomplete,
        ] {
            for prior in ALL_PRIORS {
                assert_eq!(
                    check(&fresh(state), prior),
                    Err(not_allowed(state, TelemetryFreshness::Fresh)),
                    "{state:?}"
                );
            }
        }
    }

    /// Stale or unavailable telemetry refuses even an offered state.
    #[test]
    fn stale_freshness_is_not_allowed_even_when_ready() {
        for freshness in [TelemetryFreshness::Stale, TelemetryFreshness::Unavailable] {
            for (state, prior) in [
                (OperationalState::Ready, PriorState::Ready),
                (OperationalState::Finished, PriorState::Finished),
                (OperationalState::Cancelled, PriorState::Cancelled),
            ] {
                assert_eq!(
                    check(&status(state, freshness), prior),
                    Err(not_allowed(state, freshness))
                );
            }
        }
    }

    /// Answer 9: a four-tool Printer's status is judged on its state alone;
    /// the tool list never collapses into one nozzle or blocks the rule.
    #[test]
    fn a_four_tool_status_follows_the_same_table() {
        let mut ready = fresh(OperationalState::Ready);
        ready.telemetry.tools = (0..4)
            .map(|index| ToolTemperature {
                index,
                temp_c: Some(25.0 + f64::from(index)),
                target_c: Some(0.0),
            })
            .collect();
        assert_eq!(check(&ready, PriorState::Ready), Ok(()));
        let mut printing = ready.clone();
        printing.operational_state = OperationalState::Printing;
        assert!(check_control(&printing, ControlVerb::Pause).is_ok());
    }

    #[test]
    fn pause_is_allowed_only_from_printing() {
        for state in ALL_STATES {
            assert_eq!(
                check_control(&fresh(state), ControlVerb::Pause).is_ok(),
                state == OperationalState::Printing,
                "{state:?}"
            );
        }
    }

    #[test]
    fn resume_is_allowed_only_from_paused() {
        for state in ALL_STATES {
            assert_eq!(
                check_control(&fresh(state), ControlVerb::Resume).is_ok(),
                state == OperationalState::Paused,
                "{state:?}"
            );
        }
    }

    #[test]
    fn cancel_is_allowed_from_printing_or_paused() {
        for state in ALL_STATES {
            assert_eq!(
                check_control(&fresh(state), ControlVerb::Cancel).is_ok(),
                matches!(state, OperationalState::Printing | OperationalState::Paused),
                "{state:?}"
            );
        }
    }

    #[test]
    fn control_needs_fresh_telemetry() {
        for verb in [ControlVerb::Pause, ControlVerb::Resume, ControlVerb::Cancel] {
            for state in [OperationalState::Printing, OperationalState::Paused] {
                assert_eq!(
                    check_control(&status(state, TelemetryFreshness::Stale), verb),
                    Err(ControlRejection {
                        observed_state: state,
                        freshness: TelemetryFreshness::Stale,
                    })
                );
            }
        }
    }
}
