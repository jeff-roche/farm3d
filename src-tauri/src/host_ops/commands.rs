//! P6's Host Operation commands (spec "Commands", D8, D9).
//!
//! Every write command runs under the Printer's lock, in D9's order:
//! replay check, the Printer exists and is not archived, the capability is
//! supported (`CAPABILITY_UNSUPPORTED`), no unresolved row
//! (`HOST_OPERATION_PENDING`), then its own pre-checks. Any rejection
//! writes no row and never burns the operation id. The write-ahead commit
//! re-checks the Printer, its Connection, the unresolved row, and the
//! Slice Revision inside its own transaction, then the command returns the
//! `dispatching` row and the executor carries on in the background;
//! outcomes arrive as events.

use std::sync::Arc;

use rusqlite::OptionalExtension;
use serde::Serialize;
use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::connections::capabilities::{
    CapabilityKey, HistoryQuery, HostJobState, HostStateQuery, KlippyState, LocateOutcome,
    PrintStatsState,
};
use crate::connections::{ConnectionConfig, ConnectionError, ConnectionState, PrinterStatus};
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::RepositoryError;
use crate::printers::create::probe_error;
use crate::printers::operational::{OperationalState, TelemetryFreshness};
use crate::printers::StoredPrinter;
use crate::spools::encode_enum;
use crate::spools::operations::{self, OperationKind};
use crate::RuntimeServices;

use super::repository::{self, NewHostOperation, Outcome};
use super::start_rule::{self, ControlVerb, StartRejection};
use super::{
    executor, reconciler, repository_error, wire, HostOperation, HostOperationEndpoint,
    HostOperationKind, HostOperationServices, HostOperationState, HostOperationsSnapshot,
    PriorState,
};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// D8: the only acknowledgement `abandon_host_operation` accepts.
pub const ABANDON_ACKNOWLEDGEMENT: &str = "hostStateUnknown";
/// D8: the longest abandon note, in characters.
pub const ABANDON_NOTE_MAX_CHARS: usize = 500;

fn ready<R: tauri::Runtime>(
    app: &AppHandle<R>,
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<HostOperationServices<R>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    services.host_ops.attach(app, &services.credentials);
    Ok(Arc::clone(&services.host_ops))
}

// --- ledger digests (spec "Commands": structs, fields in this order) ------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StageDigest<'a> {
    printer_id: &'a str,
    slice_revision_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct StartDigest<'a> {
    printer_id: &'a str,
    host_operation_id: &'a str,
    prior_state: PriorState,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct ControlDigest<'a> {
    printer_id: &'a str,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AbandonDigest<'a> {
    host_operation_id: &'a str,
    acknowledgement: &'a str,
    note: Option<&'a str>,
}

/// D2 replay check, read-only: `true` when `operation_id` already recorded
/// this exact request. A reused id for a different request is
/// `OperationIdReused`.
fn is_replay<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    operation_id: &str,
    kind: OperationKind,
    digest: &str,
) -> Result<bool, CommandError> {
    if operation_id.trim().is_empty() {
        return Err(repository_error(RepositoryError::Validation {
            field_path: "operationId",
        }));
    }
    let recorded: Option<(String, String)> = services
        .storage
        .read(|connection| {
            connection
                .query_row(
                    "SELECT kind, request_digest FROM operations WHERE id = ?1",
                    [operation_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
        .map_err(|error| repository_error(RepositoryError::Storage(error)))?;
    match recorded {
        None => Ok(false),
        Some((recorded_kind, recorded_digest))
            if recorded_kind == encode_enum(kind) && recorded_digest == digest =>
        {
            Ok(true)
        }
        Some(_) => Err(repository_error(RepositoryError::OperationIdReused)),
    }
}

/// The row a replayed write command created.
fn replayed_row<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    operation_id: &str,
) -> Result<HostOperation, CommandError> {
    services
        .storage
        .read(|connection| Ok(repository::load_by_operation_id(connection, operation_id)))
        .map_err(|error| repository_error(RepositoryError::Storage(error)))?
        .map_err(repository_error)?
        .ok_or_else(|| CommandError::not_found(operation_id))
}

fn archived_error() -> CommandError {
    CommandError::validation_at("printerId", "Unarchive this Printer first.")
}

/// D9 steps 2–4: the Printer exists and is not archived, `capability` is
/// supported, and it has no unresolved row.
fn writable_printer<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    printer_id: &str,
    capability: CapabilityKey,
) -> Result<StoredPrinter, CommandError> {
    let printer = services
        .load_printer(printer_id)
        .map_err(repository_error)?
        .ok_or_else(|| CommandError::not_found(printer_id))?;
    if printer.archived_at.is_some() {
        return Err(archived_error());
    }
    if let Some(error) = services.unsupported(&printer, capability) {
        return Err(error);
    }
    let pending = services
        .storage
        .read(|connection| Ok(repository::list_unresolved(connection, Some(printer_id))))
        .map_err(|error| repository_error(RepositoryError::Storage(error)))?
        .map_err(repository_error)?;
    if let Some(row) = pending.first() {
        return Err(CommandError::host_operation_pending(
            &[printer_id.to_string()],
            std::slice::from_ref(&row.id),
        ));
    }
    Ok(printer)
}

fn connection_of(printer: &StoredPrinter) -> Result<&ConnectionConfig, CommandError> {
    // `writable_printer`'s capability check already refused a Printer with
    // no Connection (D6 rule 1).
    printer
        .connection
        .as_ref()
        .ok_or_else(CommandError::internal)
}

/// A pre-check read that failed: the existing network errors (D9).
fn network_error(
    error: ConnectionError,
    config: &ConnectionConfig,
    printer_id: &str,
) -> CommandError {
    probe_error(error, &config.kind, Some(printer_id))
}

/// A host-state query on the Printer's current Connection.
fn host_state_for<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    printer: &StoredPrinter,
) -> Result<Box<dyn HostStateQuery>, CommandError> {
    let config = connection_of(printer)?;
    let key = services
        .credential(printer)
        .map_err(|()| CommandError::credential_required(&printer.id))?;
    services
        .factory
        .host_state(config, key)
        .ok_or_else(|| CommandError::unsupported_adapter(&config.kind))
}

/// The refusal of a stage or control command whose Printer's Connection
/// changed during its pre-checks (Start refuses with
/// `START_PRECONDITION_CHANGED` instead).
fn connection_changed_error() -> CommandError {
    CommandError::validation_at("printerId", "The Printer's Connection changed. Try again.")
}

/// D2/D3 write-ahead: re-checks, inside the transaction, that the Printer
/// still exists, is not archived, still has exactly the Connection the
/// pre-checks used (`kind`, `host`, `port`, `useTls`, and credential
/// reference; else `connection_changed`), has no unresolved row, and that
/// the row's Slice Revision still exists (`NOT_FOUND`); then claims the
/// operation id and inserts the `dispatching` row. After commit it
/// publishes the row and starts the executor.
///
/// The Connection commands don't take the host-ops lock, so a Connection
/// edit can land while the pre-checks talk to the host. Without the
/// compare, the row would name the old endpoint while the executor loads
/// the new credential (D7: never take the endpoint or credential out from
/// under a write).
fn write_ahead<R: tauri::Runtime>(
    services: &Arc<HostOperationServices<R>>,
    checked: &ConnectionConfig,
    new_operation: NewHostOperation,
    connection_changed: impl FnOnce() -> CommandError,
) -> Result<HostOperation, CommandError> {
    let printer_id = new_operation.printer_id.clone();
    services.run_before_write_ahead();
    let outcome = services
        .storage
        .write_repo(|tx| {
            let printer: Option<(Option<String>, Option<String>)> = tx
                .query_row(
                    "SELECT archived_at, connection_json FROM printers WHERE id = ?1",
                    [&printer_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()?;
            let connection_json = match printer {
                None => {
                    return Err(RepositoryError::NotFound {
                        entity_id: printer_id.clone(),
                    })
                }
                Some((Some(_), _)) => {
                    return Err(RepositoryError::Validation {
                        field_path: "printerId",
                    })
                }
                Some((None, connection_json)) => connection_json,
            };
            // An unreadable Connection is treated as changed (fail-safe).
            let current = connection_json
                .and_then(|json| serde_json::from_str::<ConnectionConfig>(&json).ok());
            if current.as_ref() != Some(checked) {
                // Nothing written yet: this commits an empty transaction.
                return Ok(None);
            }
            if let Some(pending) = repository::list_unresolved(tx, Some(&printer_id))?.first() {
                return Err(RepositoryError::HostOperationsPending {
                    printer_ids: vec![printer_id.clone()],
                    host_operation_ids: vec![pending.id.clone()],
                });
            }
            if let Some(slice_revision_id) = &new_operation.slice_revision_id {
                let exists = tx
                    .query_row(
                        "SELECT 1 FROM slice_revisions WHERE id = ?1",
                        [slice_revision_id],
                        |_| Ok(()),
                    )
                    .optional()?
                    .is_some();
                if !exists {
                    return Err(RepositoryError::NotFound {
                        entity_id: slice_revision_id.clone(),
                    });
                }
            }
            repository::insert_dispatching(tx, &new_operation).map(Some)
        })
        .map_err(|error| match error {
            RepositoryError::Validation {
                field_path: "printerId",
            } => archived_error(),
            other => repository_error(other),
        })?;
    // `None`: the Printer's Connection is no longer the one the
    // pre-checks used.
    let Some(row) = outcome else {
        return Err(connection_changed());
    };
    services.publish(std::slice::from_ref(&row));
    executor::spawn(services, row.id.clone());
    Ok(row)
}

fn live_status<R: tauri::Runtime>(
    services: &HostOperationServices<R>,
    printer_id: &str,
) -> PrinterStatus {
    services
        .manager
        .statuses()
        .remove(printer_id)
        .unwrap_or_else(|| PrinterStatus::new(ConnectionState::Offline))
}

// --- stage_slice_revision -------------------------------------------------------

/// D9 "`stage_slice_revision` order": steps 1–4 (`upload`), the Slice
/// Revision exists, the Printer is online, then write-ahead. The executor
/// uploads to `farm3d/<slice-revision-id>.gcode` and verifies it.
#[tauri::command]
pub async fn stage_slice_revision<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
    slice_revision_id: String,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    let printer_lock = services.printer_lock(&printer_id);
    let _serialized = printer_lock.lock().await;

    let digest = operations::digest(&StageDigest {
        printer_id: &printer_id,
        slice_revision_id: &slice_revision_id,
    });
    if is_replay(
        &services,
        &operation_id,
        OperationKind::StageSliceRevision,
        &digest,
    )? {
        return replayed_row(&services, &operation_id).map(CommandSuccess::new);
    }
    let printer = writable_printer(&services, &printer_id, CapabilityKey::Upload)?;
    let config = connection_of(&printer)?;
    let revision: Option<(String, i64)> = services
        .storage
        .read(|connection| {
            connection
                .query_row(
                    "SELECT gcode_sha256, gcode_size FROM slice_revisions WHERE id = ?1",
                    [&slice_revision_id],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
        })
        .map_err(|error| repository_error(RepositoryError::Storage(error)))?;
    let (gcode_sha256, gcode_size) =
        revision.ok_or_else(|| CommandError::not_found(&slice_revision_id))?;
    if !services.is_online(&printer_id) {
        return Err(network_error(
            ConnectionError::Unreachable("the Printer is not online".to_string()),
            config,
            &printer_id,
        ));
    }
    let row = write_ahead(
        &services,
        config,
        NewHostOperation {
            operation_id,
            operation_kind: OperationKind::StageSliceRevision,
            request_digest: digest,
            printer_id,
            kind: HostOperationKind::Upload,
            host_path: format!("farm3d/{slice_revision_id}.gcode"),
            slice_revision_id: Some(slice_revision_id),
            source_host_operation_id: None,
            gcode_sha256: Some(gcode_sha256),
            gcode_size: Some(gcode_size),
            history_mark: None,
            endpoint: HostOperationEndpoint::of(config),
        },
        connection_changed_error,
    )?;
    Ok(CommandSuccess::new(row))
}

// --- start_staged_artifact --------------------------------------------------

fn start_rejection(printer_id: &str, prior: PriorState, rejection: StartRejection) -> CommandError {
    match rejection {
        StartRejection::NotAllowed {
            observed_state,
            freshness,
        } => CommandError::start_not_allowed(
            printer_id,
            &wire(observed_state),
            &wire(freshness),
            start_rule::state_label(observed_state, freshness),
        ),
        StartRejection::PreconditionChanged {
            observed_state,
            freshness,
        } => CommandError::start_precondition_changed(
            printer_id,
            &wire(observed_state),
            &wire(freshness),
            &wire(prior),
        ),
    }
}

/// Spec "Error codes": a host re-read's print state as an
/// `OperationalState` (Klipper not ready is `error`).
fn host_observed_state(state: &HostJobState) -> OperationalState {
    if state.klippy_state != KlippyState::Ready {
        return OperationalState::Error;
    }
    match state.print.as_ref().map(|print| &print.state) {
        Some(PrintStatsState::Printing) => OperationalState::Printing,
        Some(PrintStatsState::Paused) => OperationalState::Paused,
        Some(PrintStatsState::Standby) => OperationalState::Ready,
        Some(PrintStatsState::Complete) => OperationalState::Finished,
        Some(PrintStatsState::Cancelled) => OperationalState::Cancelled,
        Some(PrintStatsState::Error) => OperationalState::Failed,
        Some(PrintStatsState::Other(_)) | None => OperationalState::Unknown,
    }
}

/// D9 step 7: the host re-read states a Start may go ahead from, the
/// Start table's offered set. Everything else refuses: printing, paused,
/// Klipper not ready (`error`), the last print failed (ruling R21, owner
/// decision 13, even if the live status hasn't caught up), and, fail-safe,
/// a print state farm3d doesn't know or no `print_stats` at all
/// (`unknown`).
fn host_allows_start(observed: OperationalState) -> bool {
    matches!(
        observed,
        OperationalState::Ready | OperationalState::Finished | OperationalState::Cancelled
    )
}

fn host_start_not_allowed(printer_id: &str, observed: OperationalState) -> CommandError {
    CommandError::start_not_allowed(
        printer_id,
        &wire(observed),
        &wire(TelemetryFreshness::Fresh),
        start_rule::state_label(observed, TelemetryFreshness::Fresh),
    )
}

/// D9 "`start_staged_artifact` order". Writes no row on any rejection.
#[tauri::command]
pub async fn start_staged_artifact<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
    host_operation_id: String,
    prior_state: PriorState,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    let printer_lock = services.printer_lock(&printer_id);
    let _serialized = printer_lock.lock().await;

    // 1. Replay.
    let digest = operations::digest(&StartDigest {
        printer_id: &printer_id,
        host_operation_id: &host_operation_id,
        prior_state,
    });
    if is_replay(
        &services,
        &operation_id,
        OperationKind::StartStagedArtifact,
        &digest,
    )? {
        return replayed_row(&services, &operation_id).map(CommandSuccess::new);
    }
    // 2–4.
    let printer = writable_printer(&services, &printer_id, CapabilityKey::Start)?;
    let config = connection_of(&printer)?;
    // 5. A succeeded upload of this Printer.
    let upload = services
        .load(&host_operation_id)
        .map_err(repository_error)?
        .ok_or_else(|| CommandError::not_found(&host_operation_id))?;
    if upload.kind != HostOperationKind::Upload
        || upload.printer_id != printer_id
        || upload.state != HostOperationState::Succeeded
    {
        return Err(CommandError::validation_at(
            "hostOperationId",
            "Choose a file staged on this Printer.",
        ));
    }
    // A succeeded upload row always has its hash and size (D2 CHECK).
    let artifact = reconciler::staged_artifact(&upload).ok_or_else(CommandError::internal)?;
    // 6. The Start rule on the live status.
    start_rule::check(&live_status(&services, &printer_id), prior_state)
        .map_err(|rejection| start_rejection(&printer_id, prior_state, rejection))?;
    // 7. Host re-read: the live status can lag, and Klipper accepts a
    // start from Paused (spike 8).
    let host_state = host_state_for(&services, &printer)?;
    match host_state.host_job_state().await {
        Ok(state) => {
            let observed = host_observed_state(&state);
            if !host_allows_start(observed) {
                return Err(host_start_not_allowed(&printer_id, observed));
            }
        }
        Err(ConnectionError::HostNotReady) => {
            return Err(host_start_not_allowed(&printer_id, OperationalState::Error))
        }
        Err(error) => return Err(network_error(error, config, &printer_id)),
    }
    // 8. The history high-water mark: the maximum parsed id over the page.
    let jobs = host_state
        .job_history(HistoryQuery {
            since_epoch_s: None,
            limit: services.timings.history_query_limit,
        })
        .await
        .map_err(|error| network_error(error, config, &printer_id))?;
    let history_mark = jobs
        .iter()
        .map(|job| job.job_id)
        .max()
        .map_or(0, |max| i64::try_from(max).unwrap_or(i64::MAX));
    // 9. The staged bytes, verified now.
    let key = services
        .credential(&printer)
        .map_err(|()| CommandError::credential_required(&printer_id))?;
    let staging = services
        .factory
        .staging(config, key)
        .ok_or_else(|| CommandError::unsupported_adapter(&config.kind))?;
    match staging.locate(&artifact).await {
        Ok(LocateOutcome::Matches) => {}
        Ok(LocateOutcome::Absent) => {
            return Err(CommandError::staged_artifact_invalid(
                &host_operation_id,
                "absent",
            ))
        }
        Ok(LocateOutcome::Differs { .. }) => {
            return Err(CommandError::staged_artifact_invalid(
                &host_operation_id,
                "differs",
            ))
        }
        Err(error) => return Err(network_error(error, config, &printer_id)),
    }
    // 10. The Start rule again: steps 7–9 can take seconds.
    start_rule::check(&live_status(&services, &printer_id), prior_state)
        .map_err(|rejection| start_rejection(&printer_id, prior_state, rejection))?;
    // 11. Write-ahead.
    let row = write_ahead(
        &services,
        config,
        NewHostOperation {
            operation_id,
            operation_kind: OperationKind::StartStagedArtifact,
            request_digest: digest,
            printer_id: printer_id.clone(),
            kind: HostOperationKind::Start,
            slice_revision_id: upload.slice_revision_id.clone(),
            source_host_operation_id: Some(upload.id.clone()),
            gcode_sha256: upload.gcode_sha256.clone(),
            gcode_size: upload.gcode_size,
            host_path: upload.host_path.clone(),
            history_mark: Some(history_mark),
            endpoint: HostOperationEndpoint::of(config),
        },
        || {
            // The bed-clear confirmation named a Printer whose Connection
            // is now different: confirm again against the current one.
            let status = live_status(&services, &printer_id);
            CommandError::start_precondition_changed(
                &printer_id,
                &wire(status.operational_state),
                &wire(status.freshness),
                &wire(prior_state),
            )
        },
    )?;
    Ok(CommandSuccess::new(row))
}

// --- pause / resume / cancel ------------------------------------------------------

fn control_rejection(
    printer_id: &str,
    verb: ControlVerb,
    observed: OperationalState,
    freshness: TelemetryFreshness,
) -> CommandError {
    CommandError::control_not_allowed(
        printer_id,
        verb.as_str(),
        &wire(observed),
        &wire(freshness),
        start_rule::state_label(observed, freshness),
    )
}

async fn control_command<R: tauri::Runtime>(
    services: Arc<HostOperationServices<R>>,
    operation_id: String,
    printer_id: String,
    verb: ControlVerb,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let (kind, operation_kind, capability) = match verb {
        ControlVerb::Pause => (
            HostOperationKind::Pause,
            OperationKind::PauseHostPrint,
            CapabilityKey::Pause,
        ),
        ControlVerb::Resume => (
            HostOperationKind::Resume,
            OperationKind::ResumeHostPrint,
            CapabilityKey::Resume,
        ),
        ControlVerb::Cancel => (
            HostOperationKind::Cancel,
            OperationKind::CancelHostPrint,
            CapabilityKey::Cancel,
        ),
    };
    let printer_lock = services.printer_lock(&printer_id);
    let _serialized = printer_lock.lock().await;

    let digest = operations::digest(&ControlDigest {
        printer_id: &printer_id,
    });
    if is_replay(&services, &operation_id, operation_kind, &digest)? {
        return replayed_row(&services, &operation_id).map(CommandSuccess::new);
    }
    let printer = writable_printer(&services, &printer_id, capability)?;
    let config = connection_of(&printer)?;
    // The control rule on the live status...
    start_rule::check_control(&live_status(&services, &printer_id), verb).map_err(|rejection| {
        control_rejection(
            &printer_id,
            verb,
            rejection.observed_state,
            rejection.freshness,
        )
    })?;
    // ...and on a host re-read (spike 9: pause from idle sets `is_paused`).
    let host_state = host_state_for(&services, &printer)?;
    let state = match host_state.host_job_state().await {
        Ok(state) => state,
        Err(ConnectionError::HostNotReady) => {
            return Err(control_rejection(
                &printer_id,
                verb,
                OperationalState::Error,
                TelemetryFreshness::Fresh,
            ))
        }
        Err(error) => return Err(network_error(error, config, &printer_id)),
    };
    let observed = host_observed_state(&state);
    let filename = state
        .print
        .as_ref()
        .and_then(|print| print.filename.clone());
    let host_path = match filename {
        Some(filename) if verb.allows(observed) => filename,
        _ => {
            return Err(control_rejection(
                &printer_id,
                verb,
                observed,
                TelemetryFreshness::Fresh,
            ))
        }
    };
    let row = write_ahead(
        &services,
        config,
        NewHostOperation {
            operation_id,
            operation_kind,
            request_digest: digest,
            printer_id,
            kind,
            slice_revision_id: None,
            source_host_operation_id: None,
            gcode_sha256: None,
            gcode_size: None,
            host_path,
            history_mark: None,
            endpoint: HostOperationEndpoint::of(config),
        },
        connection_changed_error,
    )?;
    Ok(CommandSuccess::new(row))
}

/// D9: pause, only while printing.
#[tauri::command]
pub async fn pause_host_print<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    control_command(services, operation_id, printer_id, ControlVerb::Pause).await
}

/// D9: resume, only while paused.
#[tauri::command]
pub async fn resume_host_print<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    control_command(services, operation_id, printer_id, ControlVerb::Resume).await
}

/// D9: cancel, while printing or paused.
#[tauri::command]
pub async fn cancel_host_print<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    printer_id: String,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    control_command(services, operation_id, printer_id, ControlVerb::Cancel).await
}

// --- reconcile, abandon, list -----------------------------------------------------

/// D5 "Check again": one attempt now, with the backoff reset. A row that
/// is not `uncertain` is returned unchanged.
#[tauri::command]
pub async fn reconcile_host_operation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    host_operation_id: String,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    let row = services
        .load(&host_operation_id)
        .map_err(repository_error)?
        .ok_or_else(|| CommandError::not_found(&host_operation_id))?;
    if row.state != HostOperationState::Uncertain {
        return Ok(CommandSuccess::new(row));
    }
    services.reset_backoff(&row.printer_id);
    reconciler::attempt(&services, &host_operation_id)
        .await
        .map(CommandSuccess::new)
}

/// D8: the operator stops the checks. Allowed only from `uncertain`, after
/// at least one attempt, or with none when the row is structurally
/// unreconcilable. Nothing is sent to the host.
#[tauri::command]
pub async fn abandon_host_operation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    host_operation_id: String,
    acknowledgement: String,
    note: Option<String>,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    if acknowledgement != ABANDON_ACKNOWLEDGEMENT {
        return Err(CommandError::validation_at(
            "acknowledgement",
            "Confirm that the printer's state is unknown.",
        ));
    }
    let note = note
        .map(|note| note.trim().to_string())
        .filter(|note| !note.is_empty());
    if note
        .as_ref()
        .is_some_and(|note| note.chars().count() > ABANDON_NOTE_MAX_CHARS)
    {
        return Err(CommandError::validation_at(
            "note",
            "The note can be at most 500 characters.",
        ));
    }
    let row = services
        .load(&host_operation_id)
        .map_err(repository_error)?
        .ok_or_else(|| CommandError::not_found(&host_operation_id))?;
    let printer_lock = services.printer_lock(&row.printer_id);
    let _serialized = printer_lock.lock().await;

    let digest = operations::digest(&AbandonDigest {
        host_operation_id: &host_operation_id,
        acknowledgement: &acknowledgement,
        note: note.as_deref(),
    });
    if is_replay(
        &services,
        &operation_id,
        OperationKind::AbandonHostOperation,
        &digest,
    )? {
        return services
            .load(&host_operation_id)
            .map_err(repository_error)?
            .ok_or_else(|| CommandError::not_found(&host_operation_id))
            .map(CommandSuccess::new);
    }
    let row = services
        .load(&host_operation_id)
        .map_err(repository_error)?
        .ok_or_else(|| CommandError::not_found(&host_operation_id))?;
    let unreconcilable = || {
        services
            .load_printer(&row.printer_id)
            .ok()
            .flatten()
            .is_some_and(|printer| {
                services
                    .unsupported(&printer, reconciler::needed_capability(row.kind))
                    .is_some()
            })
    };
    let abandonable =
        row.state == HostOperationState::Uncertain && (row.attempts >= 1 || unreconcilable());
    if !abandonable {
        return Err(CommandError::host_operation_not_abandonable(
            &host_operation_id,
            &wire(row.state),
            row.attempts,
        ));
    }
    // The replay check above ran under this Printer's lock, which every
    // abandon of this row holds, so the claim here is always fresh; the
    // claim still guards the ledger, and a replay can't reach this point
    // to publish.
    let abandoned = services
        .storage
        .write_repo(|tx| {
            operations::claim(
                tx,
                &operation_id,
                OperationKind::AbandonHostOperation,
                &digest,
            )?;
            repository::transition(
                tx,
                &host_operation_id,
                Outcome::Abandoned { note: note.clone() },
            )
        })
        .map_err(repository_error)?;
    services.cancel_retries(&abandoned.printer_id);
    services.publish(std::slice::from_ref(&abandoned));
    Ok(CommandSuccess::new(abandoned))
}

/// Spec "Commands": the backfill. The sequence is read before the rows, so
/// a change the snapshot misses carries a larger sequence.
#[tauri::command]
pub async fn list_host_operations<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    printer_id: Option<String>,
) -> Result<CommandSuccess<HostOperationsSnapshot>, CommandError> {
    let services = ready(&app, &bootstrap, contract_version)?;
    let snapshot_sequence = services.stream.snapshot_sequence();
    let operations = services
        .storage
        .read(|connection| Ok(repository::snapshot(connection, printer_id.as_deref())))
        .map_err(|error| repository_error(RepositoryError::Storage(error)))?
        .map_err(repository_error)?;
    Ok(CommandSuccess::new(HostOperationsSnapshot {
        stream_id: services.stream.stream_id().to_string(),
        snapshot_sequence,
        operations,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::connections::moonraker::files::not_ready_job_state;

    #[test]
    fn a_host_reread_without_print_stats_never_allows_a_start() {
        // Klipper ready, but no `print_stats` in the answer: `Unknown`.
        let state = not_ready_job_state(KlippyState::Ready);
        let observed = host_observed_state(&state);
        assert_eq!(observed, OperationalState::Unknown);
        assert!(!host_allows_start(observed));
    }

    #[test]
    fn only_ready_finished_and_cancelled_hosts_allow_a_start() {
        for (state, allowed) in [
            (OperationalState::Ready, true),
            (OperationalState::Finished, true),
            (OperationalState::Cancelled, true),
            (OperationalState::Unknown, false),
            (OperationalState::Failed, false),
            (OperationalState::Error, false),
            (OperationalState::Printing, false),
            (OperationalState::Paused, false),
        ] {
            assert_eq!(host_allows_start(state), allowed, "{state:?}");
        }
    }
}
