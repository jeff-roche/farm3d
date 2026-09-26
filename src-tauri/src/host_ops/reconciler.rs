//! D5 "Reconciliation rules": proves an `uncertain` row's outcome by
//! **reading** the host. It never issues a write. It dispatches to the
//! row's recorded `endpoint_json` with the Printer's current credential.
//!
//! | Kind | Proved applied | Proved not applied |
//! |---|---|---|
//! | upload | `locate` = `Matches` | `Absent`/`Differs`, only once the settle period has passed |
//! | start | (a) printing/paused our file, or (b) a qualifying history job | never |
//! | pause | `paused`, our file | never |
//! | resume | `printing` or `complete`, our file | never |
//! | cancel | `cancelled`, our file | never |
//!
//! Anything else goes back to `uncertain` with `attempts + 1`. Seeing
//! Klipper not ready sets `no_longer_pending`, which never changes the
//! state.

use std::panic::AssertUnwindSafe;
use std::sync::Arc;
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::FutureExt;

use crate::connections::capabilities::{
    ArtifactStaging, CapabilityKey, HistoryJob, HistoryQuery, HostOperationFailureCode,
    HostStateQuery, InconclusiveReason, KlippyState, LocateOutcome, PrintStatsState,
    StagedArtifact,
};
use crate::connections::ConnectionError;
use crate::contracts::command::CommandError;

use super::repository::{self, Outcome};
use super::{
    log_commit_failure, parse_time, repository_error, HostOperation, HostOperationFailure,
    HostOperationKind, HostOperationObservedState, HostOperationResolution, HostOperationServices,
    HostOperationState, HostOpsTimings, StartEvidenceSource,
};

/// What one read of the host proved.
#[derive(Clone, Debug, PartialEq)]
pub(crate) enum Decision {
    Applied(HostOperationResolution),
    /// Upload only.
    NotApplied(HostOperationFailureCode),
    Inconclusive {
        reason: InconclusiveReason,
        no_longer_pending: bool,
    },
}

fn inconclusive(reason: InconclusiveReason) -> Decision {
    Decision::Inconclusive {
        reason,
        no_longer_pending: false,
    }
}

fn not_ready() -> Decision {
    Decision::Inconclusive {
        reason: InconclusiveReason::HostNotReady,
        no_longer_pending: true,
    }
}

/// A host-state read that failed: never a proof either way.
fn read_failure(error: &ConnectionError) -> Decision {
    match error {
        ConnectionError::HostNotReady => not_ready(),
        ConnectionError::Auth(_) => inconclusive(InconclusiveReason::AuthRejected),
        ConnectionError::Unreachable(_) | ConnectionError::Timeout => {
            inconclusive(InconclusiveReason::HostUnreachable)
        }
        ConnectionError::Protocol(_) => inconclusive(InconclusiveReason::UnexpectedResponse),
    }
}

/// D5 "Capability gate": what an attempt on a row of `kind` needs.
pub(crate) fn needed_capability(kind: HostOperationKind) -> CapabilityKey {
    match kind {
        HostOperationKind::Upload => CapabilityKey::ArtifactIdentity,
        _ => CapabilityKey::HostState,
    }
}

/// Timestamps are stored at whole-second precision, so a stored instant
/// may be up to a second earlier than the real one. Every bound here counts
/// from the end of the stored second, which only ever makes a proof harder.
const STORED_PRECISION: Duration = Duration::from_secs(1);

fn chrono(duration: Duration) -> chrono::Duration {
    chrono::Duration::from_std(duration).unwrap_or(chrono::Duration::MAX)
}

/// When an upload's absence starts to count: `uncertain_since +
/// SETTLE_PERIOD` (from the end of the stored second). `None` when the row
/// has no `uncertain_since`, which never settles.
pub(crate) fn settle_deadline(
    row: &HostOperation,
    timings: &HostOpsTimings,
) -> Option<DateTime<Utc>> {
    let since = parse_time(row.uncertain_since.as_deref()?)?;
    Some(since + chrono(STORED_PRECISION) + chrono(timings.settle_period))
}

/// The staged artifact an upload or start row names: its host path, and
/// the G-code hash and size copied at creation. `None` for a row without
/// them (pause, resume, cancel).
pub(crate) fn staged_artifact(row: &HostOperation) -> Option<StagedArtifact> {
    Some(StagedArtifact {
        host_path: row.host_path.clone(),
        sha256: row.gcode_sha256.clone()?,
        size: u64::try_from(row.gcode_size?).ok()?,
    })
}

/// The upload row.
pub(crate) async fn decide_upload(
    staging: &dyn ArtifactStaging,
    row: &HostOperation,
    now: DateTime<Utc>,
    timings: &HostOpsTimings,
) -> Decision {
    let Some(artifact) = staged_artifact(row) else {
        return inconclusive(InconclusiveReason::IdentityCheckFailed);
    };
    let settled = settle_deadline(row, timings).is_some_and(|deadline| now >= deadline);
    match staging.locate(&artifact).await {
        Ok(LocateOutcome::Matches) => {
            Decision::Applied(HostOperationResolution::ArtifactVerified { reconciled: true })
        }
        Ok(LocateOutcome::Absent) if settled => {
            Decision::NotApplied(HostOperationFailureCode::NotApplied)
        }
        Ok(LocateOutcome::Differs { .. }) if settled => {
            Decision::NotApplied(HostOperationFailureCode::HostFileDiffers)
        }
        Ok(LocateOutcome::Absent | LocateOutcome::Differs { .. }) => {
            inconclusive(InconclusiveReason::UploadSettling)
        }
        Err(ConnectionError::Auth(_)) => inconclusive(InconclusiveReason::AuthRejected),
        Err(ConnectionError::Unreachable(_) | ConnectionError::Timeout) => {
            inconclusive(InconclusiveReason::HostUnreachable)
        }
        Err(_) => inconclusive(InconclusiveReason::IdentityCheckFailed),
    }
}

/// Rule (b)'s job statuses that mean the print ran and was cut short.
fn is_interrupted(job: &HistoryJob) -> bool {
    matches!(
        job.status.as_str(),
        "klippy_disconnect" | "klippy_shutdown" | "server_exit"
    )
}

fn epoch_seconds(time: DateTime<Utc>) -> f64 {
    time.timestamp() as f64 + f64::from(time.timestamp_subsec_millis()) / 1000.0
}

/// The start row. Never "not applied": a start can still be queued.
pub(crate) async fn decide_start(
    host_state: &dyn HostStateQuery,
    row: &HostOperation,
    timings: &HostOpsTimings,
) -> Decision {
    let host_path = row.host_path.as_str();
    let mut klipper_not_ready = false;
    let mut different_file = false;
    let mut failure: Option<Decision> = None;

    // Rule (a): printing or paused, our file.
    match host_state.host_job_state().await {
        Ok(state) => {
            if state.klippy_state != KlippyState::Ready {
                klipper_not_ready = true;
            }
            if let Some(print) = state.print {
                if matches!(
                    print.state,
                    PrintStatsState::Printing | PrintStatsState::Paused
                ) {
                    if print.filename.as_deref() == Some(host_path) {
                        return Decision::Applied(HostOperationResolution::StartObserved {
                            source: StartEvidenceSource::PrintStats,
                            history_job_id: None,
                            interrupted: false,
                        });
                    }
                    different_file = true;
                }
            }
        }
        Err(ConnectionError::HostNotReady) => klipper_not_ready = true,
        Err(error) => failure = Some(read_failure(&error)),
    }

    // Rule (b): a job for our file above the high-water mark that started
    // after dispatch (less the skew tolerance), whatever its status. Every
    // returned job is re-checked; `since` and `order` are not trusted.
    let dispatched_at = row.dispatched_at.as_deref().and_then(parse_time);
    if let (Some(dispatched_at), Some(mark)) = (dispatched_at, row.history_mark) {
        let skew = chrono(timings.start_skew_tolerance);
        let query_since = epoch_seconds(dispatched_at - skew);
        let earliest = epoch_seconds(dispatched_at + chrono(STORED_PRECISION) - skew);
        let mark = u64::try_from(mark).unwrap_or(u64::MAX);
        let query = HistoryQuery {
            since_epoch_s: Some(query_since),
            limit: timings.history_query_limit,
        };
        match host_state.job_history(query).await {
            Ok(jobs) => {
                let qualifying = jobs
                    .iter()
                    .filter(|job| {
                        job.filename == host_path
                            && job.job_id > mark
                            && job.start_time_epoch_s >= earliest
                    })
                    .max_by_key(|job| job.job_id);
                if let Some(job) = qualifying {
                    return Decision::Applied(HostOperationResolution::StartObserved {
                        source: StartEvidenceSource::History,
                        history_job_id: Some(format!("{:06X}", job.job_id)),
                        interrupted: is_interrupted(job),
                    });
                }
            }
            Err(error) => {
                failure.get_or_insert_with(|| read_failure(&error));
            }
        }
    }

    if klipper_not_ready {
        return not_ready();
    }
    if let Some(failure) = failure {
        return failure;
    }
    inconclusive(if different_file {
        InconclusiveReason::DifferentFileOnHost
    } else {
        InconclusiveReason::NoStartEvidence
    })
}

/// Pause, resume, and cancel: the verb's effect on our file. Also the
/// executor's verification window (`reconciled: false`).
pub(crate) async fn observe_control(
    host_state: &dyn HostStateQuery,
    kind: HostOperationKind,
    host_path: &str,
    reconciled: bool,
) -> Decision {
    let state = match host_state.host_job_state().await {
        Ok(state) => state,
        Err(error) => return read_failure(&error),
    };
    if state.klippy_state != KlippyState::Ready {
        return not_ready();
    }
    let Some(print) = state.print else {
        return inconclusive(InconclusiveReason::EffectNotObserved);
    };
    let ours = print.filename.as_deref() == Some(host_path);
    let observed = match (kind, &print.state) {
        (HostOperationKind::Pause, PrintStatsState::Paused) => {
            Some(HostOperationObservedState::Paused)
        }
        (HostOperationKind::Resume, PrintStatsState::Printing) => {
            Some(HostOperationObservedState::Printing)
        }
        (HostOperationKind::Resume, PrintStatsState::Complete) => {
            Some(HostOperationObservedState::Complete)
        }
        (HostOperationKind::Cancel, PrintStatsState::Cancelled) => {
            Some(HostOperationObservedState::Cancelled)
        }
        _ => None,
    };
    match observed {
        Some(observed_state) if ours => Decision::Applied(HostOperationResolution::StateObserved {
            observed_state,
            reconciled,
        }),
        _ if !ours
            && print.filename.is_some()
            && matches!(
                print.state,
                PrintStatsState::Printing | PrintStatsState::Paused
            ) =>
        {
            inconclusive(InconclusiveReason::DifferentFileOnHost)
        }
        _ => inconclusive(InconclusiveReason::EffectNotObserved),
    }
}

/// One attempt on row `id`, under the Printer's lock. A row that is not
/// `uncertain` is returned unchanged. When the capability the row's kind
/// needs is unsupported, no attempt runs (`CAPABILITY_UNSUPPORTED`; the
/// automatic triggers ignore it, which is how they skip the row).
pub(crate) async fn attempt<R: tauri::Runtime>(
    services: &Arc<HostOperationServices<R>>,
    id: &str,
) -> Result<HostOperation, CommandError> {
    let not_found = || CommandError::not_found(id);
    let row = services
        .load(id)
        .map_err(repository_error)?
        .ok_or_else(not_found)?;
    let printer_lock = services.printer_lock(&row.printer_id);
    let _serialized = printer_lock.lock().await;
    let row = services
        .load(id)
        .map_err(repository_error)?
        .ok_or_else(not_found)?;
    if row.state != HostOperationState::Uncertain {
        return Ok(row);
    }
    let printer = services
        .load_printer(&row.printer_id)
        .map_err(repository_error)?
        .ok_or_else(|| CommandError::not_found(&row.printer_id))?;
    let needed = needed_capability(row.kind);
    if let Some(error) = services.unsupported(&printer, needed) {
        return Err(error);
    }
    let config = services.endpoint_config(&row.endpoint, &printer);
    // A read with no key is harmless: the host answers 401, which is
    // inconclusive (`authRejected`).
    let key = services.credential(&printer).unwrap_or(None);
    enum Reader {
        Staging(Box<dyn ArtifactStaging>),
        HostState(Box<dyn HostStateQuery>),
    }
    let reader = match row.kind {
        HostOperationKind::Upload => services.factory.staging(&config, key).map(Reader::Staging),
        _ => services
            .factory
            .host_state(&config, key)
            .map(Reader::HostState),
    };
    let Some(reader) = reader else {
        // No adapter for the recorded endpoint's kind: structurally
        // unreconcilable, like an unsupported capability.
        return Err(CommandError::capability_unsupported(
            &printer.id,
            &super::wire(needed),
            "adapter",
            "farm3d can't use this Connection type.",
        ));
    };

    let reconciling = services
        .storage
        .write_repo(|tx| repository::transition(tx, id, Outcome::Reconciling))
        .map_err(repository_error)?;
    services.publish(std::slice::from_ref(&reconciling));

    let decide = async {
        match (&reader, row.kind) {
            (Reader::Staging(staging), _) => {
                decide_upload(
                    staging.as_ref(),
                    &row,
                    services.clock.now(),
                    &services.timings,
                )
                .await
            }
            (Reader::HostState(host_state), HostOperationKind::Start) => {
                decide_start(host_state.as_ref(), &row, &services.timings).await
            }
            (Reader::HostState(host_state), kind) => {
                observe_control(host_state.as_ref(), kind, &row.host_path, true).await
            }
        }
    };
    let decision = AssertUnwindSafe(decide)
        .catch_unwind()
        .await
        .unwrap_or_else(|_| inconclusive(InconclusiveReason::UnexpectedResponse));

    let committed = services
        .storage
        .write_repo(|tx| match &decision {
            Decision::Applied(resolution) => repository::transition(
                tx,
                id,
                Outcome::Succeeded {
                    resolution: resolution.clone(),
                },
            ),
            Decision::NotApplied(code) => repository::transition(
                tx,
                id,
                Outcome::Failed {
                    failure: HostOperationFailure::for_code(*code),
                },
            ),
            Decision::Inconclusive {
                reason,
                no_longer_pending,
            } => {
                if *no_longer_pending {
                    repository::set_no_longer_pending(tx, id)?;
                }
                repository::record_attempt(tx, id, *reason)
            }
        })
        .map_err(|error| {
            // The row stays `reconciling`; startup recovery returns it to
            // `uncertain` without counting the attempt.
            log_commit_failure(id, "its reconcile outcome", &error);
            repository_error(error)
        })?;
    services.publish(std::slice::from_ref(&committed));
    if committed.state == HostOperationState::Uncertain {
        services.schedule_retry(&committed);
    }
    Ok(committed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::capabilities::{HostFacts, HostJobState, PrintSnapshot};
    use crate::host_ops::HostOperationEndpoint;

    struct Host {
        state: Result<HostJobState, ConnectionError>,
        history: Result<Vec<HistoryJob>, ConnectionError>,
    }

    #[async_trait::async_trait]
    impl HostStateQuery for Host {
        async fn host_facts(&self) -> Result<HostFacts, ConnectionError> {
            Err(ConnectionError::Timeout)
        }
        async fn host_job_state(&self) -> Result<HostJobState, ConnectionError> {
            self.state.clone()
        }
        async fn job_history(
            &self,
            _query: HistoryQuery,
        ) -> Result<Vec<HistoryJob>, ConnectionError> {
            self.history.clone()
        }
    }

    const PATH: &str = "farm3d/slr-a.gcode";

    fn job_state(state: PrintStatsState, filename: Option<&str>) -> HostJobState {
        HostJobState {
            klippy_state: KlippyState::Ready,
            print: Some(PrintSnapshot {
                state,
                filename: filename.map(str::to_string),
                is_paused: false,
            }),
            tools: Vec::new(),
            bed: None,
        }
    }

    fn start_row(dispatched_at: DateTime<Utc>, mark: i64) -> HostOperation {
        HostOperation {
            id: "hop-a".to_string(),
            printer_id: "prn-a".to_string(),
            kind: HostOperationKind::Start,
            state: HostOperationState::Reconciling,
            slice_revision_id: None,
            source_host_operation_id: None,
            gcode_sha256: Some("a".repeat(64)),
            gcode_size: Some(10),
            host_path: PATH.to_string(),
            history_mark: Some(mark),
            endpoint: HostOperationEndpoint {
                kind: "moonraker".to_string(),
                host: "192.0.2.1".to_string(),
                port: 7125,
            },
            failure: None,
            resolution: None,
            attempts: 0,
            last_attempt: None,
            no_longer_pending: false,
            abandoned_at: None,
            abandon_note: None,
            created_at: super::super::format_time(dispatched_at),
            dispatched_at: Some(super::super::format_time(dispatched_at)),
            uncertain_since: Some(super::super::format_time(dispatched_at)),
            resolved_at: None,
        }
    }

    fn job(id: u64, filename: &str, status: &str, start: DateTime<Utc>) -> HistoryJob {
        HistoryJob {
            job_id: id,
            filename: filename.to_string(),
            status: status.to_string(),
            start_time_epoch_s: epoch_seconds(start),
        }
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn job_ids_and_start_times_are_compared_as_numbers() {
        let dispatched = Utc::now();
        let timings = HostOpsTimings::default();
        // 0x10 = 16 > mark 9, though "000010" < "9" as a string.
        let host = Host {
            state: Ok(job_state(PrintStatsState::Standby, None)),
            history: Ok(vec![job(0x10, PATH, "completed", dispatched)]),
        };
        let decision = block_on(decide_start(&host, &start_row(dispatched, 9), &timings));
        assert_eq!(
            decision,
            Decision::Applied(HostOperationResolution::StartObserved {
                source: StartEvidenceSource::History,
                history_job_id: Some("000010".to_string()),
                interrupted: false,
            })
        );
    }

    #[test]
    fn a_job_at_the_mark_or_before_the_skew_bound_never_counts() {
        let dispatched = Utc::now();
        let timings = HostOpsTimings::default();
        for jobs in [
            vec![job(9, PATH, "completed", dispatched)],
            vec![job(
                10,
                PATH,
                "completed",
                dispatched - chrono::Duration::seconds(31),
            )],
            vec![job(10, "farm3d/other.gcode", "completed", dispatched)],
        ] {
            let host = Host {
                state: Ok(job_state(PrintStatsState::Complete, Some(PATH))),
                history: Ok(jobs),
            };
            assert_eq!(
                block_on(decide_start(&host, &start_row(dispatched, 9), &timings)),
                inconclusive(InconclusiveReason::NoStartEvidence)
            );
        }
    }

    #[test]
    fn an_interrupted_job_is_applied_and_marked_interrupted() {
        let dispatched = Utc::now();
        for status in ["klippy_disconnect", "klippy_shutdown", "server_exit"] {
            let host = Host {
                state: Ok(job_state(PrintStatsState::Standby, None)),
                history: Ok(vec![job(3, PATH, status, dispatched)]),
            };
            match block_on(decide_start(
                &host,
                &start_row(dispatched, 0),
                &HostOpsTimings::default(),
            )) {
                Decision::Applied(HostOperationResolution::StartObserved {
                    interrupted, ..
                }) => {
                    assert!(interrupted, "{status}")
                }
                other => panic!("{status}: {other:?}"),
            }
        }
    }

    #[test]
    fn klipper_not_ready_on_a_start_read_sets_no_longer_pending() {
        let host = Host {
            state: Err(ConnectionError::HostNotReady),
            history: Ok(Vec::new()),
        };
        assert_eq!(
            block_on(decide_start(
                &host,
                &start_row(Utc::now(), 0),
                &HostOpsTimings::default()
            )),
            not_ready()
        );
    }

    #[test]
    fn control_is_proved_only_by_its_own_effect_on_our_file() {
        let cases = [
            (HostOperationKind::Pause, PrintStatsState::Paused, true),
            (HostOperationKind::Pause, PrintStatsState::Printing, false),
            (HostOperationKind::Resume, PrintStatsState::Printing, true),
            (HostOperationKind::Resume, PrintStatsState::Complete, true),
            (HostOperationKind::Resume, PrintStatsState::Paused, false),
            (HostOperationKind::Cancel, PrintStatsState::Cancelled, true),
            (HostOperationKind::Cancel, PrintStatsState::Standby, false),
        ];
        for (kind, state, proved) in cases {
            let host = Host {
                state: Ok(job_state(state.clone(), Some(PATH))),
                history: Ok(Vec::new()),
            };
            let decision = block_on(observe_control(&host, kind, PATH, true));
            assert_eq!(
                matches!(decision, Decision::Applied(_)),
                proved,
                "{kind:?} {state:?}: {decision:?}"
            );
        }
        // The same state on another file proves nothing.
        let host = Host {
            state: Ok(job_state(PrintStatsState::Paused, Some("other.gcode"))),
            history: Ok(Vec::new()),
        };
        assert_eq!(
            block_on(observe_control(&host, HostOperationKind::Pause, PATH, true)),
            inconclusive(InconclusiveReason::DifferentFileOnHost)
        );
    }

    #[test]
    fn the_settle_deadline_counts_from_the_end_of_the_stored_second() {
        let row = start_row(Utc::now(), 0);
        let since = parse_time(row.uncertain_since.as_deref().unwrap()).unwrap();
        assert_eq!(
            settle_deadline(&row, &HostOpsTimings::default()),
            Some(since + chrono::Duration::seconds(61))
        );
    }
}
