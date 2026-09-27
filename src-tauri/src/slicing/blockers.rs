//! D14 deletion guards.
//!
//! - [`SliceRevisionDeletionBlocker`]: what may block deleting a Slice
//!   Revision. P6 registers
//!   `host_ops::guards::UnresolvedHostOperationBlocksRevisionDeletion`
//!   (an unresolved upload or start uses it); P7 registers Queue Entries
//!   and Jobs here.
//! - [`SliceRevisionsBlockModelDeletion`]: the P4 `ModelDeletionBlocker`
//!   P5 registers, because a Model's Slice Revisions `RESTRICT` its
//!   deletion.

use rusqlite::Transaction;

use crate::library::blockers::ModelDeletionBlocker;
use crate::library::StoredModel;
use crate::persistence::StorageError;
use crate::printers::lifecycle::{LifecycleAction, LifecycleBlocker, LifecycleBlockerCode};

/// One source of reasons a Slice Revision can't be deleted, consulted
/// inside the delete's own transaction.
pub trait SliceRevisionDeletionBlocker: Send + Sync {
    fn blockers(
        &self,
        slice_revision_id: &str,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError>;
}

/// Every registered source, in the order their blockers are reported.
pub fn slice_revision_blocker_sources() -> &'static [&'static dyn SliceRevisionDeletionBlocker] {
    &[&crate::host_ops::guards::UnresolvedHostOperationBlocksRevisionDeletion]
}

/// Every blocker `sources` report for deleting `slice_revision_id`.
pub fn evaluate_slice_revision_deletion(
    slice_revision_id: &str,
    tx: &Transaction<'_>,
    sources: &[&dyn SliceRevisionDeletionBlocker],
) -> Result<Vec<LifecycleBlocker>, StorageError> {
    let mut blockers = Vec::new();
    for source in sources {
        blockers.extend(source.blockers(slice_revision_id, tx)?);
    }
    Ok(blockers)
}

/// Blocks deleting a Model that still has Slice Revisions.
pub struct SliceRevisionsBlockModelDeletion;

impl ModelDeletionBlocker for SliceRevisionsBlockModelDeletion {
    fn blockers(
        &self,
        model: &StoredModel,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError> {
        let count: i64 = tx.query_row(
            "SELECT COUNT(*) FROM slice_revisions WHERE model_id = ?1",
            [&model.id],
            |row| row.get(0),
        )?;
        Ok(if count == 0 {
            Vec::new()
        } else {
            vec![LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::SliceRevisionsExist,
                message: if count == 1 {
                    "Delete this Model's 1 Slice Revision first.".to_string()
                } else {
                    format!("Delete this Model's {count} Slice Revisions first.")
                },
            }]
        })
    }
}
