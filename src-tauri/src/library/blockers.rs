//! D18 step 1: what may block deleting a Model. P4 registers no source; P5
//! and P7 add "referenced by a Slice Revision, Queue Entry, or Job" here,
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

/// Every registered source, in the order their blockers are reported. Empty
/// in P4.
pub fn blocker_sources() -> &'static [&'static dyn ModelDeletionBlocker] {
    &[]
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
