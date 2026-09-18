use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::connections::ConnectionState;

/// The normalized activity vocabulary supplied by a host adapter.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/HostActivity.ts")]
pub enum HostActivity {
    Idle,
    Printing,
    Paused,
    Busy,
    Unknown,
}

/// The single operational state a consumer renders for a printer.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/OperationalState.ts")]
pub enum OperationalState {
    SetupIncomplete,
    Error,
    Offline,
    Connecting,
    Unknown,
    Printing,
    Paused,
    Busy,
    Ready,
}

/// Whether the printer may receive operational work.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ReadinessState.ts")]
pub enum ReadinessState {
    Ready,
    NotReady,
}

/// The policy reason for a non-ready printer.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/ReadinessReason.ts")]
pub enum ReadinessReason {
    SetupIncomplete,
    ConnectionError,
    Offline,
    Refreshing,
    TelemetryUnavailable,
    TelemetryStale,
    PrinterBusy,
}

/// The readiness state and its explanatory reason, when one exists.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/PrinterReadiness.ts")]
pub struct PrinterReadiness {
    pub state: ReadinessState,
    pub reason: Option<ReadinessReason>,
}

/// How safely a host's latest telemetry can be used.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/TelemetryFreshness.ts")]
pub enum TelemetryFreshness {
    Fresh,
    Stale,
    Unavailable,
}

/// Normalized, protocol-independent facts evaluated by the operational policy.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/OperationalInput.ts")]
pub struct OperationalInput {
    pub setup_complete: bool,
    pub connection_state: ConnectionState,
    pub connection_error: bool,
    pub host_activity: HostActivity,
    #[ts(type = "string | null")]
    pub last_observed_at: Option<DateTime<Utc>>,
    #[ts(type = "string | null")]
    pub fresh_until: Option<DateTime<Utc>>,
    pub hydrated_from_cache: bool,
}

/// The canonical operational status derived from normalized input facts.
#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/OperationalResult.ts")]
pub struct OperationalResult {
    pub operational_state: OperationalState,
    pub readiness: PrinterReadiness,
    pub freshness: TelemetryFreshness,
}

/// Evaluates the operational state, readiness, and telemetry freshness at `now`.
pub fn evaluate_operational_status(
    input: &OperationalInput,
    now: DateTime<Utc>,
) -> OperationalResult {
    let freshness = telemetry_freshness(input, now);
    let (operational_state, reason) = if !input.setup_complete {
        (
            OperationalState::SetupIncomplete,
            Some(ReadinessReason::SetupIncomplete),
        )
    } else if input.connection_error || input.connection_state == ConnectionState::Error {
        (
            OperationalState::Error,
            Some(ReadinessReason::ConnectionError),
        )
    } else if input.connection_state == ConnectionState::Offline {
        (OperationalState::Offline, Some(ReadinessReason::Offline))
    } else if input.connection_state == ConnectionState::Connecting {
        (
            OperationalState::Connecting,
            Some(ReadinessReason::Refreshing),
        )
    } else if freshness == TelemetryFreshness::Unavailable {
        (
            OperationalState::Unknown,
            Some(ReadinessReason::TelemetryUnavailable),
        )
    } else if freshness == TelemetryFreshness::Stale {
        (
            OperationalState::Unknown,
            Some(ReadinessReason::TelemetryStale),
        )
    } else {
        match input.host_activity {
            HostActivity::Unknown => (
                OperationalState::Unknown,
                Some(ReadinessReason::TelemetryUnavailable),
            ),
            HostActivity::Printing => (
                OperationalState::Printing,
                Some(ReadinessReason::PrinterBusy),
            ),
            HostActivity::Paused => (OperationalState::Paused, Some(ReadinessReason::PrinterBusy)),
            HostActivity::Busy => (OperationalState::Busy, Some(ReadinessReason::PrinterBusy)),
            HostActivity::Idle => (OperationalState::Ready, None),
        }
    };
    let readiness = PrinterReadiness {
        state: if operational_state == OperationalState::Ready {
            ReadinessState::Ready
        } else {
            ReadinessState::NotReady
        },
        reason,
    };

    OperationalResult {
        operational_state,
        readiness,
        freshness,
    }
}

fn telemetry_freshness(input: &OperationalInput, now: DateTime<Utc>) -> TelemetryFreshness {
    if input.hydrated_from_cache {
        TelemetryFreshness::Stale
    } else if input.last_observed_at.is_none() {
        TelemetryFreshness::Unavailable
    } else if input.connection_error
        || matches!(
            input.connection_state,
            ConnectionState::Error | ConnectionState::Offline
        )
    {
        TelemetryFreshness::Stale
    } else if input
        .fresh_until
        .is_some_and(|fresh_until| now <= fresh_until)
    {
        TelemetryFreshness::Fresh
    } else {
        TelemetryFreshness::Stale
    }
}

#[cfg(test)]
mod tests {
    use chrono::{TimeZone, Utc};

    use super::*;
    use crate::connections::ConnectionState;

    fn input(activity: HostActivity) -> OperationalInput {
        OperationalInput {
            setup_complete: true,
            connection_state: ConnectionState::Online,
            connection_error: false,
            host_activity: activity,
            last_observed_at: Some(Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 0).unwrap()),
            fresh_until: Some(Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 30).unwrap()),
            hydrated_from_cache: false,
        }
    }

    #[test]
    fn evaluates_operational_precedence_and_readiness() {
        let now = Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 30).unwrap();
        let mut setup_incomplete = input(HostActivity::Idle);
        setup_incomplete.setup_complete = false;
        let mut connection_state_error = input(HostActivity::Idle);
        connection_state_error.connection_state = ConnectionState::Error;
        let mut reported_connection_error = input(HostActivity::Idle);
        reported_connection_error.connection_error = true;
        let mut offline = input(HostActivity::Idle);
        offline.connection_state = ConnectionState::Offline;
        let mut connecting = input(HostActivity::Idle);
        connecting.connection_state = ConnectionState::Connecting;

        let cases = [
            (
                setup_incomplete,
                OperationalState::SetupIncomplete,
                Some(ReadinessReason::SetupIncomplete),
            ),
            (
                connection_state_error,
                OperationalState::Error,
                Some(ReadinessReason::ConnectionError),
            ),
            (
                reported_connection_error,
                OperationalState::Error,
                Some(ReadinessReason::ConnectionError),
            ),
            (
                offline,
                OperationalState::Offline,
                Some(ReadinessReason::Offline),
            ),
            (
                connecting,
                OperationalState::Connecting,
                Some(ReadinessReason::Refreshing),
            ),
            (
                input(HostActivity::Unknown),
                OperationalState::Unknown,
                Some(ReadinessReason::TelemetryUnavailable),
            ),
            (
                input(HostActivity::Printing),
                OperationalState::Printing,
                Some(ReadinessReason::PrinterBusy),
            ),
            (
                input(HostActivity::Paused),
                OperationalState::Paused,
                Some(ReadinessReason::PrinterBusy),
            ),
            (
                input(HostActivity::Busy),
                OperationalState::Busy,
                Some(ReadinessReason::PrinterBusy),
            ),
            (input(HostActivity::Idle), OperationalState::Ready, None),
        ];

        for (input, operational_state, reason) in cases {
            let result = evaluate_operational_status(&input, now);

            assert_eq!(result.operational_state, operational_state);
            assert_eq!(result.readiness.reason, reason);
            assert_eq!(
                result.readiness.state,
                if operational_state == OperationalState::Ready {
                    ReadinessState::Ready
                } else {
                    ReadinessState::NotReady
                }
            );
        }
    }

    #[test]
    fn evaluates_freshness_at_the_boundary_and_for_cached_or_missing_telemetry() {
        let now = Utc.with_ymd_and_hms(2026, 9, 18, 12, 0, 30).unwrap();

        let fresh = input(HostActivity::Idle);
        let mut stale = input(HostActivity::Idle);
        stale.fresh_until = Some(now - chrono::Duration::seconds(1));
        let mut cached = input(HostActivity::Idle);
        cached.hydrated_from_cache = true;
        let mut unavailable = input(HostActivity::Idle);
        unavailable.last_observed_at = None;
        let mut connection_error = input(HostActivity::Idle);
        connection_error.connection_error = true;

        for (input, freshness, operational_state, reason) in [
            (
                fresh,
                TelemetryFreshness::Fresh,
                OperationalState::Ready,
                None,
            ),
            (
                stale,
                TelemetryFreshness::Stale,
                OperationalState::Unknown,
                Some(ReadinessReason::TelemetryStale),
            ),
            (
                cached,
                TelemetryFreshness::Stale,
                OperationalState::Unknown,
                Some(ReadinessReason::TelemetryStale),
            ),
            (
                unavailable,
                TelemetryFreshness::Unavailable,
                OperationalState::Unknown,
                Some(ReadinessReason::TelemetryUnavailable),
            ),
            (
                connection_error,
                TelemetryFreshness::Stale,
                OperationalState::Error,
                Some(ReadinessReason::ConnectionError),
            ),
        ] {
            let result = evaluate_operational_status(&input, now);

            assert_eq!(result.freshness, freshness);
            assert_eq!(result.operational_state, operational_state);
            assert_eq!(result.readiness.reason, reason);
        }
    }
}
