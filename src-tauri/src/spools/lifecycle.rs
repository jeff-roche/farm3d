//! D9: Spool lifecycle actions (`markEmpty`, `reactivate`, `archive`,
//! `unarchive`) and D8's reservation guard on them. [`apply_lifecycle`] is
//! the whole action inside the caller's transaction; Task 7's
//! `set_spool_lifecycle` command wraps it. The mark-empty steps are shared
//! with archive dispositions (`spools::dispositions`), which fold the
//! unload into their own single `apply_moves` operation.

use rusqlite::{params, Transaction};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::persistence::{RepositoryError, StorageError};
use crate::printers::lifecycle::{LifecycleAction, LifecycleBlocker, LifecycleBlockerCode};
use crate::printers::now_rfc3339;

use super::ledger::{self, AmountEventKind, LedgerSnapshot};
use super::movement::{self, MoveDestination, MoveOutcome, MovementReason};
use super::operations::{self, Claim, OperationKind};
use super::repository::{self, check_and_bump_revision, StoredSpool};
use super::reservations;
use super::{encode_enum, AmountConfidence, SpoolLifecycle};

/// D9's lifecycle actions.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SpoolLifecycleAction.ts")]
pub enum SpoolLifecycleAction {
    MarkEmpty,
    Reactivate,
    Archive,
    Unarchive,
}

/// Applies one D9 action to `spool_id` after an optimistic-concurrency
/// check against `expected_revision`:
///
/// - `MarkEmpty`: `active` only, and no open reservations. Writes a
///   `markedEmpty` ledger row (0 g, measured). A loaded Spool is also
///   unloaded to storage under `storage_label` with a `consumed` movement.
///   `storage_label` is ignored for a Spool already in storage.
/// - `Reactivate`: `empty` → `active`.
/// - `Archive`: from `active` or `empty`; the Spool must be in storage and
///   have no open reservations. Records the prior state in `archived_from`.
/// - `Unarchive`: `archived` → the state in `archived_from`.
///
/// A wrong starting state is `VALIDATION` on `action`; a loaded or reserved
/// Spool is `LIFECYCLE_BLOCKED` (`SPOOLS_LOADED`/`SPOOL_RESERVED`).
///
/// Idempotent by `operation_id` the way `move_spool` is (D6): the id is
/// claimed in the operations ledger first, and a retry of the same request
/// writes nothing and returns `replayed = true` — the recorded movement
/// for a `markEmpty` that unloaded the Spool, otherwise just this Spool.
/// A replay reports the Spool's current state, not a snapshot of the first
/// call's result. The id reused for a different request is
/// [`RepositoryError::OperationIdReused`].
pub fn apply_lifecycle(
    tx: &Transaction<'_>,
    spool_id: &str,
    expected_revision: i64,
    action: SpoolLifecycleAction,
    storage_label: Option<&str>,
    operation_id: &str,
) -> Result<MoveOutcome, RepositoryError> {
    let digest = operations::digest(&SpoolLifecycleRequest {
        spool_id,
        expected_revision,
        action,
        storage_label,
    });
    if operations::claim(tx, operation_id, OperationKind::SpoolLifecycle, &digest)? == Claim::Replay
    {
        return Ok(movement::recorded_or_unmoved(tx, operation_id, spool_id)?);
    }
    if expected_revision <= 0 {
        return Err(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
    }
    let spool = repository::load_spool(tx, spool_id)?.ok_or_else(|| RepositoryError::NotFound {
        entity_id: spool_id.to_string(),
    })?;
    if spool.revision != expected_revision {
        return Err(RepositoryError::Conflict {
            entity_id: spool_id.to_string(),
            expected_revision,
            current_revision: spool.revision,
        });
    }

    match action {
        SpoolLifecycleAction::MarkEmpty => {
            ensure_can_mark_empty(tx, &spool, LifecycleAction::MarkEmpty, "action")?;
            let outcome = if spool.slot_id.is_some() {
                movement::apply_move(
                    tx,
                    operation_id,
                    spool_id,
                    expected_revision,
                    &MoveDestination::Storage {
                        storage_label: storage_label.map(str::to_string),
                    },
                    Some(MovementReason::Consumed),
                )?
            } else {
                check_and_bump_revision(tx, spool_id, expected_revision)?;
                unmoved(spool_id)
            };
            record_marked_empty(tx, spool_id)?;
            Ok(outcome)
        }
        SpoolLifecycleAction::Reactivate => {
            require_state(spool.lifecycle == SpoolLifecycle::Empty)?;
            check_and_bump_revision(tx, spool_id, expected_revision)?;
            set_lifecycle(tx, spool_id, SpoolLifecycle::Active, None)?;
            Ok(unmoved(spool_id))
        }
        SpoolLifecycleAction::Archive => {
            require_state(matches!(
                spool.lifecycle,
                SpoolLifecycle::Active | SpoolLifecycle::Empty
            ))?;
            if spool.slot_id.is_some() {
                return Err(RepositoryError::LifecycleBlocked(vec![LifecycleBlocker {
                    action: LifecycleAction::Archive,
                    code: LifecycleBlockerCode::SpoolsLoaded,
                    message: "This Spool is loaded. Unload it before archiving.".to_string(),
                }]));
            }
            ensure_unreserved(tx, spool_id, LifecycleAction::Archive)?;
            check_and_bump_revision(tx, spool_id, expected_revision)?;
            set_lifecycle(
                tx,
                spool_id,
                SpoolLifecycle::Archived,
                Some(spool.lifecycle),
            )?;
            Ok(unmoved(spool_id))
        }
        SpoolLifecycleAction::Unarchive => {
            require_state(spool.lifecycle == SpoolLifecycle::Archived)?;
            check_and_bump_revision(tx, spool_id, expected_revision)?;
            let restored = spool.archived_from.unwrap_or(SpoolLifecycle::Active);
            set_lifecycle(tx, spool_id, restored, None)?;
            Ok(unmoved(spool_id))
        }
    }
}

/// The request fields that define a `set_spool_lifecycle` operation, in a
/// fixed order for [`operations::digest`].
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SpoolLifecycleRequest<'a> {
    spool_id: &'a str,
    expected_revision: i64,
    action: SpoolLifecycleAction,
    storage_label: Option<&'a str>,
}

/// The mark-empty preconditions (D8/D9): the Spool is `active` (else
/// `VALIDATION` on `invalid_field`) and has no open reservations (else
/// `LIFECYCLE_BLOCKED`/`SPOOL_RESERVED` reported against `action`).
pub(crate) fn ensure_can_mark_empty(
    tx: &Transaction<'_>,
    spool: &StoredSpool,
    action: LifecycleAction,
    invalid_field: &'static str,
) -> Result<(), RepositoryError> {
    if spool.lifecycle != SpoolLifecycle::Active {
        return Err(RepositoryError::Validation {
            field_path: invalid_field,
        });
    }
    ensure_unreserved(tx, &spool.id, action)
}

/// The mark-empty writes other than the unload: a `markedEmpty` ledger row
/// to 0 g (measured) and the `empty` lifecycle. The caller has already
/// checked [`ensure_can_mark_empty`] and bumped the Spool's revision.
pub(crate) fn record_marked_empty(
    tx: &Transaction<'_>,
    spool_id: &str,
) -> Result<(), RepositoryError> {
    ledger::append(
        tx,
        spool_id,
        AmountEventKind::MarkedEmpty,
        0,
        AmountConfidence::Measured,
        LedgerSnapshot::default(),
    )?;
    set_lifecycle(tx, spool_id, SpoolLifecycle::Empty, None)?;
    Ok(())
}

fn ensure_unreserved(
    tx: &Transaction<'_>,
    spool_id: &str,
    action: LifecycleAction,
) -> Result<(), RepositoryError> {
    if reservations::open_reservations(tx, spool_id)?.is_empty() {
        return Ok(());
    }
    Err(RepositoryError::LifecycleBlocked(vec![LifecycleBlocker {
        action,
        code: LifecycleBlockerCode::SpoolReserved,
        message: "This Spool is reserved. Release its reservations first.".to_string(),
    }]))
}

fn require_state(allowed: bool) -> Result<(), RepositoryError> {
    if allowed {
        Ok(())
    } else {
        Err(RepositoryError::Validation {
            field_path: "action",
        })
    }
}

/// The only write of `spools.lifecycle`/`archived_from`. The revision bump
/// is the caller's (`check_and_bump_revision` or the movement it made).
fn set_lifecycle(
    tx: &Transaction<'_>,
    spool_id: &str,
    lifecycle: SpoolLifecycle,
    archived_from: Option<SpoolLifecycle>,
) -> Result<(), StorageError> {
    tx.execute(
        "UPDATE spools SET lifecycle = ?2, archived_from = ?3, updated_at = ?4 WHERE id = ?1",
        params![
            spool_id,
            encode_enum(lifecycle),
            archived_from.map(encode_enum),
            now_rfc3339()
        ],
    )?;
    Ok(())
}

/// The outcome of an action that changed a Spool without moving it.
fn unmoved(spool_id: &str) -> MoveOutcome {
    MoveOutcome {
        spool_ids: vec![spool_id.to_string()],
        printer_ids: Vec::new(),
        movements: Vec::new(),
        replayed: false,
    }
}
