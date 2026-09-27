//! P6's Host Operation commands (spec "Commands", D8, D9).
//!
//! The write commands (`stage_slice_revision`, `start_staged_artifact`,
//! pause, resume, and cancel) are thin wrappers over [`super::api`], which
//! holds their D9 order and the write-ahead; they pass no Job link, so a
//! Printer with an active Job refuses them with `JOB_ACTIVE` (P7 D9).
//! Reconcile and abandon stay here, unguarded: they are the operator's exit
//! for a Job-linked row stuck `uncertain` (P7 D3).

use std::sync::Arc;

use serde::Serialize;
use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::RepositoryError;
use crate::spools::operations::{self, OperationKind};
use crate::RuntimeServices;

use super::api::{self, is_replay};
use super::repository::{self, Outcome};
use super::start_rule::ControlVerb;
use super::{
    reconciler, repository_error, wire, HostOperation, HostOperationServices, HostOperationState,
    HostOperationsSnapshot, PriorState,
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

// --- ledger digest (spec "Commands": fields in this order) ------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AbandonDigest<'a> {
    host_operation_id: &'a str,
    acknowledgement: &'a str,
    note: Option<&'a str>,
}

// --- stage, start, pause, resume, cancel (raw P6 writes) --------------------

/// D9 "`stage_slice_revision` order" ([`api::stage`]).
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
    api::stage(&services, operation_id, printer_id, slice_revision_id, None)
        .await
        .map(CommandSuccess::new)
}

/// D9 "`start_staged_artifact` order" ([`api::start`]). Writes no row on
/// any rejection.
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
    api::start(
        &services,
        operation_id,
        printer_id,
        host_operation_id,
        prior_state,
        None,
    )
    .await
    .map(CommandSuccess::new)
}

async fn control_command<R: tauri::Runtime>(
    services: Arc<HostOperationServices<R>>,
    operation_id: String,
    printer_id: String,
    verb: ControlVerb,
) -> Result<CommandSuccess<HostOperation>, CommandError> {
    api::control(&services, operation_id, printer_id, verb, None)
        .await
        .map(CommandSuccess::new)
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
