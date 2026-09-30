//! P8 D7 "Commands": the four Attention commands — `list_attention`,
//! `mark_attention_read`, `acknowledge_attention_event`, and
//! `resolve_attention_event`.
//!
//! Every mutating command claims its `operationId` through
//! `spools::operations::claim` in its own IMMEDIATE transaction (global
//! constraint 10): a replay returns the current rows with no side effect
//! and publishes nothing; a reused id with a different request is
//! `VALIDATION` on `operationId`; a rejected command rolls back and never
//! burns its id. After commit, a command publishes its changed rows on the
//! `attention` stream; a no-op publishes nothing.
//!
//! Reading, acknowledging, and resolving are three separate dimensions
//! (`attention::lifecycle`): acknowledging never resolves, and only a
//! `manual` Event is the operator's to resolve (`ATTENTION_NOT_MANUAL`).

use std::collections::HashSet;
use std::sync::Arc;

use serde::Serialize;
use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::incidents::repository as incidents_repository;
use crate::incidents::{Incident, IncidentEntryDetail};
use crate::persistence::{RepositoryError, StorageError};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::RuntimeServices;

use super::lifecycle::AckBy;
use super::repository as attention_repository;
use super::{
    AttentionBackfill, AttentionChange, AttentionCursor, AttentionEvent, AttentionResolution,
};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// `list_attention`'s default and largest resolved page.
const MAX_PAGE: i64 = 200;
/// `mark_attention_read`'s largest batch.
const MAX_READ_BATCH: usize = 200;

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

/// A `limit` of `1..=200`, defaulting to 200.
pub(crate) fn page_limit(limit: Option<i64>) -> Result<i64, CommandError> {
    match limit {
        None => Ok(MAX_PAGE),
        Some(limit) if (1..=MAX_PAGE).contains(&limit) => Ok(limit),
        Some(_) => Err(CommandError::validation_at(
            "limit",
            "limit must be between 1 and 200",
        )),
    }
}

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
}

fn load_event(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<AttentionEvent, RepositoryError> {
    attention_repository::load_event(tx, id)?.ok_or_else(|| not_found(id))
}

fn load_incident(tx: &rusqlite::Transaction<'_>, id: &str) -> Result<Incident, RepositoryError> {
    incidents_repository::load_incident(tx, id)?.ok_or_else(|| not_found(id))
}

/// The Event's current row and, when it is linked, its Incident's.
fn with_incident(
    tx: &rusqlite::Transaction<'_>,
    event: AttentionEvent,
) -> Result<AttentionChange, RepositoryError> {
    let incidents = match &event.incident_id {
        Some(incident_id) => vec![load_incident(tx, incident_id)?],
        None => Vec::new(),
    };
    Ok(AttentionChange {
        events: vec![event],
        incidents,
    })
}

fn publish<R: tauri::Runtime>(
    app: &AppHandle<R>,
    services: &RuntimeServices<R>,
    change: &AttentionChange,
) {
    if change.events.is_empty() && change.incidents.is_empty() {
        return;
    }
    services
        .attention
        .stream
        .publish_change(app, &change.events, &change.incidents, &[]);
}

/// `list_attention`: without `resolvedBefore`, every open Event, the first
/// resolved page, the open Incidents, and camera health (in memory, one
/// per Printer with a camera source); with it, only the
/// next resolved page. The stream's sequence is read before the rows
/// (listen before backfill).
#[tauri::command]
pub async fn list_attention<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    resolved_before: Option<AttentionCursor>,
    limit: Option<i64>,
) -> Result<CommandSuccess<AttentionBackfill>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let limit = page_limit(limit)?;
    if let Some(cursor) = &resolved_before {
        if !attention_repository::is_well_formed_cursor(cursor) {
            return Err(CommandError::validation_at(
                "resolvedBefore",
                "resolvedBefore must be a cursor list_attention returned",
            ));
        }
    }
    let snapshot_sequence = services.attention.stream.snapshot_sequence();
    let stream_id = services.attention.stream.stream_id().to_string();
    let first_page = resolved_before.is_none();
    let (page, open_incidents, camera_sources) = services
        .storage
        .read_transaction(|tx| {
            Ok((|| -> Result<_, StorageError> {
                let page =
                    attention_repository::list_attention(tx, resolved_before.as_ref(), limit)?;
                let (incidents, camera_sources) = if first_page {
                    (
                        incidents_repository::list_open(tx)?,
                        crate::cameras::config::list_sources(tx)?,
                    )
                } else {
                    (Vec::new(), Vec::new())
                };
                Ok((page, incidents, camera_sources))
            })())
        })
        .map_err(storage_error)?
        .map_err(storage_error)?;
    Ok(CommandSuccess::new(AttentionBackfill {
        stream_id,
        snapshot_sequence: snapshot_sequence.get() as i64,
        open: if first_page { page.open } else { Vec::new() },
        resolved: page.resolved,
        resolved_cursor: page.resolved_cursor,
        open_incidents,
        // One per Printer with a camera source (none on a later page).
        camera_health: services.cameras.health_for(&camera_sources),
    }))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MarkReadDigest<'a> {
    event_ids: &'a [String],
}

/// `mark_attention_read`: 1..=200 distinct ids, all or nothing. Returns
/// the Events that changed (a replay: every named Event's current row).
#[tauri::command]
pub async fn mark_attention_read<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    event_ids: Vec<String>,
) -> Result<CommandSuccess<AttentionChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let distinct: HashSet<&String> = event_ids.iter().collect();
    if event_ids.is_empty() || event_ids.len() > MAX_READ_BATCH || distinct.len() != event_ids.len()
    {
        return Err(CommandError::validation_at(
            "eventIds",
            "eventIds must name 1 to 200 distinct Attention Events",
        ));
    }
    let digest = operations::digest(&MarkReadDigest {
        event_ids: &event_ids,
    });
    let now = services.attention.now();
    let (change, replayed) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(tx, &operation_id, OperationKind::MarkAttentionRead, &digest)?
                == Claim::Replay
            {
                let events = event_ids
                    .iter()
                    .map(|id| load_event(tx, id))
                    .collect::<Result<Vec<_>, _>>()?;
                return Ok((
                    AttentionChange {
                        events,
                        incidents: Vec::new(),
                    },
                    true,
                ));
            }
            let unread: HashSet<String> = event_ids
                .iter()
                .map(|id| load_event(tx, id))
                .collect::<Result<Vec<_>, _>>()?
                .into_iter()
                .filter(|event| event.read_at.is_none())
                .map(|event| event.id)
                .collect();
            let events = attention_repository::mark_read(tx, &event_ids, now)?
                .into_iter()
                .filter(|event| unread.contains(&event.id))
                .collect();
            Ok((
                AttentionChange {
                    events,
                    incidents: Vec::new(),
                },
                false,
            ))
        })
        .map_err(CommandError::from_repository)?;
    if !replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct EventDigest<'a> {
    event_id: &'a str,
}

/// `acknowledge_attention_event`: the Event stays open and actionable
/// until it resolves. A linked Incident gains an `eventAcknowledged {
/// by: operator }` entry. Returns the Event and its Incident if linked.
#[tauri::command]
pub async fn acknowledge_attention_event<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    event_id: String,
) -> Result<CommandSuccess<AttentionChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = operations::digest(&EventDigest {
        event_id: &event_id,
    });
    let now = services.attention.now();
    let (change, publishes) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(
                tx,
                &operation_id,
                OperationKind::AcknowledgeAttention,
                &digest,
            )? == Claim::Replay
            {
                return Ok((with_incident(tx, load_event(tx, &event_id)?)?, false));
            }
            let (event, changed) =
                attention_repository::acknowledge(tx, &event_id, AckBy::Operator, now)?;
            if changed {
                if let Some(incident_id) = &event.incident_id {
                    incidents_repository::append_entry(
                        tx,
                        incident_id,
                        &IncidentEntryDetail::EventAcknowledged {
                            event_id: event_id.clone(),
                            by: AckBy::Operator,
                        },
                        Some(&operation_id),
                        now,
                    )?;
                }
            }
            Ok((with_incident(tx, event)?, changed))
        })
        .map_err(CommandError::from_repository)?;
    if publishes {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

/// `resolve_attention_event`: only a `manual` Event (`job.failed`,
/// `job.hostCancelled`) is the operator's to resolve; any other is
/// `ATTENTION_NOT_MANUAL`, whether open or resolved. A linked Incident
/// gains an `eventResolved` entry and closes when this was its last open
/// actionable Event.
#[tauri::command]
pub async fn resolve_attention_event<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    event_id: String,
) -> Result<CommandSuccess<AttentionChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = operations::digest(&EventDigest {
        event_id: &event_id,
    });
    let now = services.attention.now();
    let (change, publishes) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(tx, &operation_id, OperationKind::ResolveAttention, &digest)?
                == Claim::Replay
            {
                return Ok((with_incident(tx, load_event(tx, &event_id)?)?, false));
            }
            let (event, changed) = attention_repository::resolve(
                tx,
                &event_id,
                AttentionResolution::OperatorResolved,
                now,
            )?;
            if changed {
                if let Some(incident_id) = &event.incident_id {
                    incidents_repository::append_entry(
                        tx,
                        incident_id,
                        &IncidentEntryDetail::EventResolved {
                            event_id: event_id.clone(),
                            resolution: AttentionResolution::OperatorResolved,
                        },
                        Some(&operation_id),
                        now,
                    )?;
                    incidents_repository::close_if_settled(tx, incident_id, now)?;
                }
            }
            Ok((with_incident(tx, event)?, changed))
        })
        .map_err(CommandError::from_repository)?;
    if publishes {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}
