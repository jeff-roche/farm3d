//! P8 D7 "Commands": the three Incident commands — `list_incidents`,
//! `get_incident`, and `add_incident_note`.
//!
//! Incidents are opened, linked, and closed only by the projector's pass
//! and `resolve_attention_event` (D3); the operator's one write here is a
//! note, appended to the timeline under a claimed `operationId` (global
//! constraint 10). After commit, the note's Incident is published once on
//! the `attention` stream with its final row; a replay publishes nothing.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::AppHandle;

use crate::attention::commands::page_limit;
use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::persistence::{RepositoryError, StorageError};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::RuntimeServices;

use super::repository as incidents_repository;
use super::{IncidentDetail, IncidentEntryDetail, IncidentPage, IncidentState};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// The longest note, in characters after trimming.
const NOTE_MAX_CHARS: usize = 2000;

fn ready<R: tauri::Runtime>(
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<RuntimeServices<R>>, CommandError> {
    contract_version.validate()?;
    bootstrap.ready()
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// `list_incidents`' `state` filter (default `all`).
#[derive(Deserialize, Clone, Copy, PartialEq, Eq, Debug)]
#[serde(rename_all = "camelCase")]
pub enum IncidentStateFilter {
    Open,
    Closed,
    All,
}

/// `list_incidents`: `openedAt` descending, then id, filtered by state and
/// Printer, paginated by the opaque `before` cursor.
#[tauri::command]
pub async fn list_incidents<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    state: Option<IncidentStateFilter>,
    printer_id: Option<String>,
    before: Option<String>,
    limit: Option<i64>,
) -> Result<CommandSuccess<IncidentPage>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let limit = page_limit(limit)?;
    if let Some(cursor) = &before {
        if !incidents_repository::is_well_formed_cursor(cursor) {
            return Err(CommandError::validation_at(
                "before",
                "before must be a cursor list_incidents returned",
            ));
        }
    }
    let state = match state.unwrap_or(IncidentStateFilter::All) {
        IncidentStateFilter::Open => Some(IncidentState::Open),
        IncidentStateFilter::Closed => Some(IncidentState::Closed),
        IncidentStateFilter::All => None,
    };
    let page = services
        .storage
        .read_transaction(|tx| {
            Ok(incidents_repository::list(
                tx,
                state,
                printer_id.as_deref(),
                before.as_deref(),
                limit,
            ))
        })
        .map_err(storage_error)?
        .map_err(storage_error)?;
    Ok(CommandSuccess::new(page))
}

/// `get_incident`: the Incident, its merged timeline, its linked Events,
/// and its snapshots.
#[tauri::command]
pub async fn get_incident<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    incident_id: String,
) -> Result<CommandSuccess<IncidentDetail>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    services
        .storage
        .read_transaction(|tx| Ok(incidents_repository::detail(tx, &incident_id)))
        .map_err(storage_error)?
        .map_err(storage_error)?
        .map(CommandSuccess::new)
        .ok_or_else(|| CommandError::not_found(&incident_id))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct NoteDigest<'a> {
    incident_id: &'a str,
    text: &'a str,
}

/// `add_incident_note`: 1-2000 characters after trimming, stored verbatim
/// as a `noteAdded` entry. Returns the Incident's detail.
#[tauri::command]
pub async fn add_incident_note<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    incident_id: String,
    text: String,
) -> Result<CommandSuccess<IncidentDetail>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let trimmed = text.trim().chars().count();
    if trimmed == 0 || trimmed > NOTE_MAX_CHARS {
        return Err(CommandError::validation_at(
            "text",
            "A note must be 1 to 2000 characters.",
        ));
    }
    let digest = operations::digest(&NoteDigest {
        incident_id: &incident_id,
        text: &text,
    });
    let now = services.attention.now();
    let (detail, replayed) = services
        .storage
        .write_repo(|tx| {
            let not_found = || RepositoryError::NotFound {
                entity_id: incident_id.clone(),
            };
            let replayed =
                operations::claim(tx, &operation_id, OperationKind::AddIncidentNote, &digest)?
                    == Claim::Replay;
            if !replayed {
                incidents_repository::get(tx, &incident_id)?.ok_or_else(not_found)?;
                incidents_repository::append_entry(
                    tx,
                    &incident_id,
                    &IncidentEntryDetail::NoteAdded { text: text.clone() },
                    Some(&operation_id),
                    now,
                )?;
            }
            let detail = incidents_repository::detail(tx, &incident_id)?.ok_or_else(not_found)?;
            Ok((detail, replayed))
        })
        .map_err(CommandError::from_repository)?;
    if !replayed {
        services.attention.stream.publish_change(
            &app,
            &[],
            std::slice::from_ref(&detail.incident),
            &[],
        );
    }
    Ok(CommandSuccess::new(detail))
}
