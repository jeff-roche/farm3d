//! D18 step 1: what may block deleting a Model. P5 registers "has Slice
//! Revisions" (spec D14), the same way P2/P3 register
//! `LifecycleBlockerSource`s for Printers (`printers::lifecycle::
//! blocker_sources`). A blocked delete is `LIFECYCLE_BLOCKED`, reusing
//! P2's `LifecycleBlocker` with `LifecycleAction::Delete`.
//!
//! P7 D8 doesn't register Queue Entries or Jobs here (a departure from
//! the plan): a Model delete is already blocked transitively through
//! `SliceRevisionsBlockModelDeletion` whenever it still has a Slice
//! Revision, and `jobs::guards::QueueOrJobBlocksRevisionDeletion` (wired
//! into `slicing::blockers::slice_revision_blocker_sources`) keeps that
//! revision from ever being deleted out from under it while a Queue
//! Entry or Job references it. See `jobs::guards` for the guards P7
//! does add.

use rusqlite::Transaction;

use crate::persistence::StorageError;
use crate::printers::lifecycle::LifecycleBlocker;

use super::StoredModel;

/// One source of reasons a Model can't be deleted, consulted inside the
/// delete's own transaction.
pub trait ModelDeletionBlocker: Send + Sync {
    fn blockers(
        &self,
        model: &StoredModel,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError>;
}

/// Every registered source, in the order their blockers are reported.
pub fn blocker_sources() -> &'static [&'static dyn ModelDeletionBlocker] {
    &[&crate::slicing::blockers::SliceRevisionsBlockModelDeletion]
}

/// Every blocker `sources` report for deleting `model`.
pub fn evaluate(
    model: &StoredModel,
    tx: &Transaction<'_>,
    sources: &[&dyn ModelDeletionBlocker],
) -> Result<Vec<LifecycleBlocker>, StorageError> {
    let mut blockers = Vec::new();
    for source in sources {
        blockers.extend(source.blockers(model, tx)?);
    }
    Ok(blockers)
}
