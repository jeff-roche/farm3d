//! P7's six Queue commands (spec "Commands"): `list_queue`,
//! `add_to_queue`, `update_queue_entry`, `move_queue_entry`,
//! `remove_queue_entry`, and `explain_queue_entry`.
//!
//! Every mutating command is one `Storage::write_repo` transaction that
//! claims the client's `operationId` first (D4). A replay returns the
//! current rows and publishes nothing; a reused id for another request is
//! `VALIDATION` on `operationId`; a rejection rolls back and never burns
//! the id. After commit the command publishes the rows it returns on the
//! `queue` stream. None of these writes takes the Printer lock (D4).

use std::sync::Arc;

use rusqlite::{params, Transaction};
use serde::Serialize;
use sha2::{Digest, Sha256};
use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::contracts::event::JsSafeInteger;
use crate::jobs::assign::open_entries_from;
use crate::jobs::repository as jobs_repository;
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::now_rfc3339;
use crate::slicing::{repository as slicing_repository, SliceRevisionKind, SliceRevisionRecord};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::weight::grams_to_mg_round_up;
use crate::RuntimeServices;

use super::eligibility;
use super::repository::{self, NewEntries};
use super::state::EntryEvent;
use super::world::{facts_for, LiveWorld, World, WorldReader};
use super::{
    DispatchPolicy, DispatchPreference, EligibilitySummary, EstimateSource, MaterialEstimate,
    NextAutomaticAction, QueueChange, QueueEntry, QueueEntryAction, QueueEntryEligibility,
    QueueEntryState, QueueSnapshot,
};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// `add_to_queue`'s quantity bound (spec "Commands").
pub const MAX_QUANTITY: i64 = 50;
/// How many closed entries `list_queue` returns (spec "Wire types").
pub const HISTORY_LIMIT: u32 = 100;

fn ready<R: tauri::Runtime>(
    bootstrap: &Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<Arc<RuntimeServices<R>>, CommandError> {
    contract_version.validate()?;
    bootstrap.ready()
}

/// Publishes a committed change: its rows on the `queue` stream, then the
/// Spools whose reservations it changed on the inventory stream (which
/// also sends the one `InventoryChange`). Never for a replay.
pub(crate) fn publish<R: tauri::Runtime>(
    app: &AppHandle<R>,
    services: &RuntimeServices<R>,
    change: &QueueChange,
) {
    services.queue_stream.publish(app, change);
    crate::spools::events::publish_ids(app, services, &change.spool_ids, &[]);
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

fn not_found(id: &str) -> RepositoryError {
    RepositoryError::NotFound {
        entity_id: id.to_string(),
    }
}

fn load_entry(tx: &Transaction<'_>, id: &str) -> Result<QueueEntry, RepositoryError> {
    repository::load(tx, id)?.ok_or_else(|| not_found(id))
}

fn check_revision(entry: &QueueEntry, expected_revision: i64) -> Result<(), RepositoryError> {
    if entry.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: entry.id.clone(),
            expected_revision,
            current_revision: entry.revision,
        });
    }
    Ok(())
}

fn require_state(
    entry: &QueueEntry,
    allowed: &[QueueEntryState],
    action: QueueEntryAction,
) -> Result<(), RepositoryError> {
    if !allowed.contains(&entry.state) {
        return Err(RepositoryError::QueueEntryActionNotAllowed {
            entry_id: entry.id.clone(),
            action,
            state: entry.state,
        });
    }
    Ok(())
}

// --- ledger digests (D4: structs, fields in this order) -------------------

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct AddDigest<'a> {
    slice_revision_id: &'a str,
    quantity: i64,
    policy: DispatchPolicy,
    preference: DispatchPreference,
    material_estimate: Option<MaterialEstimate>,
    manual_printer_id: Option<&'a str>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct UpdateDigest<'a> {
    entry_id: &'a str,
    expected_revision: i64,
    policy: Option<DispatchPolicy>,
    preference: Option<DispatchPreference>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct MoveDigest<'a> {
    entry_id: &'a str,
    expected_revision: i64,
    to_position: i64,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct RemoveDigest<'a> {
    entry_id: &'a str,
    expected_revision: i64,
}

// --- list and explain ---------------------------------------------------------

/// The snapshot's rows, read in one transaction, under the stream
/// identity and the sequence the caller read first.
fn read_snapshot(
    tx: &Transaction<'_>,
    reader: &dyn WorldReader,
    now: &str,
    stream_id: String,
    snapshot_sequence: JsSafeInteger,
) -> Result<QueueSnapshot, RepositoryError> {
    let mut entries = repository::list_open(tx)?;
    entries.extend(repository::list_history(tx, HISTORY_LIMIT)?);

    let mut jobs = jobs_repository::list_active(tx)?;
    for entry in &entries {
        if let Some(job_id) = entry.job_id.as_deref() {
            if !jobs.iter().any(|job| job.id == job_id) {
                if let Some(job) = jobs_repository::load_job(tx, job_id)? {
                    jobs.push(job);
                }
            }
        }
    }
    let requirements = jobs_repository::open_requirements(tx)?;

    // Ruling R3: until the evaluator runs, each `queued` entry's summary
    // is computed here, synchronously, over the same rows.
    let world = World::read(tx, reader)?;
    let mut eligibility = Vec::new();
    for entry in entries
        .iter()
        .filter(|entry| entry.state == QueueEntryState::Queued)
    {
        let facts = facts_for(tx, entry)?;
        let views = world.views(entry, None);
        let evaluated = eligibility::evaluate(&world.input(entry, &facts, &views), now);
        eligibility.push(EligibilitySummary::of(&evaluated));
    }
    Ok(QueueSnapshot {
        stream_id,
        snapshot_sequence,
        entries,
        jobs,
        requirements,
        eligibility,
        // Ruling R3: the evaluator (a later task) replaces this with its
        // last run's conclusion.
        next_automatic_action: NextAutomaticAction::EvaluatorNotRunning,
    })
}

/// Spec "Commands": the backfill. The sequence is read before the rows,
/// so a change the snapshot misses carries a larger sequence.
#[tauri::command]
pub async fn list_queue<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<QueueSnapshot>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let snapshot_sequence = services.queue_stream.snapshot_sequence();
    let reader = LiveWorld::of(&services);
    let now = now_rfc3339();
    let stream_id = services.queue_stream.stream_id().to_string();
    let snapshot = services
        .storage
        .read_transaction(|tx| {
            Ok(read_snapshot(
                tx,
                &reader,
                &now,
                stream_id,
                snapshot_sequence,
            ))
        })
        .map_err(storage_error)?
        .map_err(CommandError::from_repository)?;
    let mut snapshot = snapshot;
    crate::jobs::dispatch::present_jobs(&services, &mut snapshot.jobs);
    Ok(CommandSuccess::new(snapshot))
}

/// D5: every gate for one entry, against every Printer, with ranked
/// candidates. Pure over rows read in one transaction.
#[tauri::command]
pub async fn explain_queue_entry<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    entry_id: String,
) -> Result<CommandSuccess<QueueEntryEligibility>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let reader = LiveWorld::of(&services);
    let now = now_rfc3339();
    let eligibility = services
        .storage
        .read_transaction(|tx| {
            Ok((|| {
                let entry = load_entry(tx, &entry_id)?;
                let world = World::read(tx, &reader)?;
                let facts = facts_for(tx, &entry)?;
                let views = world.views(&entry, None);
                Ok::<_, RepositoryError>(eligibility::evaluate(
                    &world.input(&entry, &facts, &views),
                    &now,
                ))
            })())
        })
        .map_err(storage_error)?
        .map_err(CommandError::from_repository)?;
    Ok(CommandSuccess::new(eligibility))
}

// --- add ---------------------------------------------------------------------------

/// Add to Queue's lineage id, derived from its `operationId` so a replay
/// finds the entries it created (P5's `derived_revision_id` pattern). It
/// keeps the `qln-` prefix and a UUID's shape.
fn derived_lineage_id(operation_id: &str) -> String {
    let hash = format!("{:x}", Sha256::digest(operation_id.as_bytes()));
    format!(
        "qln-{}-{}-{}-{}-{}",
        &hash[0..8],
        &hash[8..12],
        &hash[12..16],
        &hash[16..20],
        &hash[20..32]
    )
}

fn invalid(field_path: &'static str) -> RepositoryError {
    RepositoryError::Validation { field_path }
}

/// Decision 4 (spec "`add_to_queue` validation"): a farm3d revision with
/// `filamentGrams` fixes the estimate (`sliceEstimate`, filled in when
/// absent); otherwise the operator must supply one, either the file's
/// confirmed claim or an amount of their own.
fn resolve_estimate(
    revision: &SliceRevisionRecord,
    given: Option<MaterialEstimate>,
) -> Result<MaterialEstimate, RepositoryError> {
    let slice_grams = match revision.summary.kind {
        SliceRevisionKind::Farm3d => revision
            .summary
            .estimates
            .as_ref()
            .and_then(|estimates| estimates.filament_grams),
        SliceRevisionKind::External => None,
    };
    let estimate = match (slice_grams, given) {
        (Some(grams), given) => {
            let fixed = MaterialEstimate {
                amount_mg: grams_to_mg_round_up(grams),
                source: EstimateSource::SliceEstimate,
            };
            match given {
                None => fixed,
                Some(given) if given == fixed => given,
                Some(_) => return Err(invalid("materialEstimate")),
            }
        }
        (None, None) => return Err(invalid("materialEstimate")),
        (None, Some(given)) => match given.source {
            EstimateSource::OperatorEntered => given,
            EstimateSource::FileClaimConfirmed => {
                let claimed = revision
                    .claimed_estimates
                    .as_ref()
                    .and_then(|claimed| claimed.filament_grams)
                    .map(grams_to_mg_round_up);
                if claimed != Some(given.amount_mg) {
                    return Err(invalid("materialEstimate"));
                }
                given
            }
            EstimateSource::SliceEstimate => return Err(invalid("materialEstimate")),
        },
    };
    if estimate.amount_mg <= 0 {
        return Err(invalid("materialEstimate"));
    }
    Ok(estimate)
}

/// The Dispatch Policy rules `add_to_queue` and `update_queue_entry`
/// share: a revision with unconfirmed facts needs the Manual policy, and
/// a pinned entry stays Manual.
fn check_policy(
    policy: DispatchPolicy,
    requires_manual_printer_selection: bool,
    pinned: bool,
) -> Result<(), RepositoryError> {
    if (requires_manual_printer_selection || pinned) && policy != DispatchPolicy::Manual {
        return Err(invalid("policy"));
    }
    Ok(())
}

#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn add_to_queue<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    slice_revision_id: String,
    quantity: i64,
    policy: DispatchPolicy,
    preference: DispatchPreference,
    material_estimate: Option<MaterialEstimate>,
    manual_printer_id: Option<String>,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    if !(1..=MAX_QUANTITY).contains(&quantity) {
        return Err(CommandError::validation_at(
            "quantity",
            "Add between 1 and 50 copies.",
        ));
    }
    let digest = operations::digest(&AddDigest {
        slice_revision_id: &slice_revision_id,
        quantity,
        policy,
        preference,
        material_estimate,
        manual_printer_id: manual_printer_id.as_deref(),
    });
    let lineage_id = derived_lineage_id(&operation_id);
    let now = now_rfc3339();
    let (entries, replayed) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(tx, &operation_id, OperationKind::AddToQueue, &digest)?
                == Claim::Replay
            {
                return Ok((lineage_originals(tx, &lineage_id)?, true));
            }
            let revision = slicing_repository::load_revision(tx, &slice_revision_id)?
                .ok_or_else(|| not_found(&slice_revision_id))?;
            let estimate = resolve_estimate(&revision, material_estimate)?;
            let requires_manual = revision.summary.requires_manual_printer_selection;
            check_policy(policy, requires_manual, false)?;
            match manual_printer_id.as_deref() {
                None if requires_manual => return Err(invalid("manualPrinterId")),
                None => {}
                Some(_) if policy != DispatchPolicy::Manual => {
                    return Err(invalid("manualPrinterId"))
                }
                Some(printer_id) => {
                    let exists: bool = tx.query_row(
                        "SELECT EXISTS(SELECT 1 FROM printers WHERE id = ?1)",
                        [printer_id],
                        |row| row.get(0),
                    )?;
                    if !exists {
                        return Err(not_found(printer_id));
                    }
                }
            }
            let created = repository::create_entries(
                tx,
                &NewEntries {
                    lineage_id: Some(lineage_id.clone()),
                    slice_revision_id: slice_revision_id.clone(),
                    quantity: quantity as u8,
                    policy,
                    preference,
                    estimate,
                    manual_printer_id: manual_printer_id.clone(),
                },
                &now,
            )?;
            Ok((created, false))
        })
        .map_err(CommandError::from_repository)?;
    let change = QueueChange {
        entries,
        ..QueueChange::default()
    };
    if !replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

/// The entries one Add to Queue created: its lineage's rows with no
/// origin, in copy order.
fn lineage_originals(
    tx: &Transaction<'_>,
    lineage_id: &str,
) -> Result<Vec<QueueEntry>, RepositoryError> {
    let mut statement = tx.prepare(
        "SELECT id FROM queue_entries WHERE lineage_id = ?1 AND origin_entry_id IS NULL
         ORDER BY copy_index",
    )?;
    let ids = statement
        .query_map(params![lineage_id], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    ids.iter().map(|id| load_entry(tx, id)).collect()
}

// --- update, move, remove ------------------------------------------------------

/// D2: policy and preference, only on a `queued` entry, guarded by
/// `expectedRevision`.
#[allow(clippy::too_many_arguments)]
#[tauri::command]
pub async fn update_queue_entry<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    entry_id: String,
    expected_revision: i64,
    policy: Option<DispatchPolicy>,
    preference: Option<DispatchPreference>,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = operations::digest(&UpdateDigest {
        entry_id: &entry_id,
        expected_revision,
        policy,
        preference,
    });
    let now = now_rfc3339();
    let (entry, replayed) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(tx, &operation_id, OperationKind::UpdateQueueEntry, &digest)?
                == Claim::Replay
            {
                return Ok((load_entry(tx, &entry_id)?, true));
            }
            let entry = load_entry(tx, &entry_id)?;
            check_revision(&entry, expected_revision)?;
            require_state(&entry, &[QueueEntryState::Queued], QueueEntryAction::Update)?;
            check_policy(
                policy.unwrap_or(entry.policy),
                entry.requires_manual_printer_selection,
                entry.manual_printer_id.is_some(),
            )?;
            let updated = repository::update_entry(
                tx,
                &entry_id,
                expected_revision,
                policy,
                preference,
                &now,
            )?;
            Ok((updated, false))
        })
        .map_err(CommandError::from_repository)?;
    let change = QueueChange {
        entries: vec![entry],
        ..QueueChange::default()
    };
    if !replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

/// D2: moves an open entry to `toPosition` (1..n) and renumbers the
/// entries between, each revision + 1.
#[tauri::command]
pub async fn move_queue_entry<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    entry_id: String,
    expected_revision: i64,
    to_position: i64,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = operations::digest(&MoveDigest {
        entry_id: &entry_id,
        expected_revision,
        to_position,
    });
    let now = now_rfc3339();
    let (entries, replayed) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(tx, &operation_id, OperationKind::MoveQueueEntry, &digest)?
                == Claim::Replay
            {
                return Ok((vec![load_entry(tx, &entry_id)?], true));
            }
            let entry = load_entry(tx, &entry_id)?;
            check_revision(&entry, expected_revision)?;
            require_state(
                &entry,
                &[QueueEntryState::Queued, QueueEntryState::Assigned],
                QueueEntryAction::Move,
            )?;
            let open: i64 = tx.query_row(
                "SELECT COUNT(*) FROM queue_entries WHERE position IS NOT NULL",
                [],
                |row| row.get(0),
            )?;
            if !(1..=open).contains(&to_position) {
                return Err(invalid("toPosition"));
            }
            let moved =
                repository::move_entry(tx, &entry_id, expected_revision, to_position, &now)?;
            Ok((moved, false))
        })
        .map_err(CommandError::from_repository)?;
    let change = QueueChange {
        entries,
        ..QueueChange::default()
    };
    if !replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

/// D2: removes a `queued` entry (`closed{removed}`); every later open
/// entry moves up. An `assigned` entry's Job owns its end instead.
#[tauri::command]
pub async fn remove_queue_entry<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    entry_id: String,
    expected_revision: i64,
) -> Result<CommandSuccess<QueueChange>, CommandError> {
    let services = ready(&bootstrap, contract_version)?;
    let digest = operations::digest(&RemoveDigest {
        entry_id: &entry_id,
        expected_revision,
    });
    let now = now_rfc3339();
    let (entries, replayed) = services
        .storage
        .write_repo(|tx| {
            if operations::claim(tx, &operation_id, OperationKind::RemoveQueueEntry, &digest)?
                == Claim::Replay
            {
                return Ok((vec![load_entry(tx, &entry_id)?], true));
            }
            let entry = load_entry(tx, &entry_id)?;
            check_revision(&entry, expected_revision)?;
            require_state(&entry, &[QueueEntryState::Queued], QueueEntryAction::Remove)?;
            let freed = entry
                .position
                .ok_or(RepositoryError::Storage(StorageError::OperationFailed))?;
            let closed = repository::apply(tx, &entry_id, &EntryEvent::Remove, None, &now)?;
            let mut entries = vec![closed];
            entries.extend(open_entries_from(tx, freed)?);
            Ok((entries, false))
        })
        .map_err(CommandError::from_repository)?;
    let change = QueueChange {
        entries,
        ..QueueChange::default()
    };
    if !replayed {
        publish(&app, &services, &change);
    }
    Ok(CommandSuccess::new(change))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_derived_lineage_id_is_stable_and_uuid_shaped() {
        let id = derived_lineage_id("op-1");
        assert_eq!(id, derived_lineage_id("op-1"));
        assert_ne!(id, derived_lineage_id("op-2"));
        assert!(id.starts_with("qln-"));
        assert_eq!(id.len(), "qln-".len() + 36);
    }
}
