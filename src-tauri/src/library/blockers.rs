//! D18 step 1: what may block deleting a Model. P5 registers "has Slice
//! Revisions" (spec D14); P7 adds Queue Entries and Jobs here,
//! the same way P2/P3 register `LifecycleBlockerSource`s for Printers
//! (`printers::lifecycle::blocker_sources`). A blocked delete is
//! `LIFECYCLE_BLOCKED`, reusing P2's `LifecycleBlocker` with
//! `LifecycleAction::Delete`.

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
