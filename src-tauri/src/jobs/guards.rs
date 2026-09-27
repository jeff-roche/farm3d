//! P7 D8: the lifecycle guards that keep a Printer archive/delete/import,
//! a Connection change, and a Slice Revision delete from ever orphaning a
//! Queue Entry, a Job, a reservation, or a history record. Every check
//! here runs inside the caller's own write transaction, alongside the
//! rest of that mutation's guards (`printers::lifecycle`,
//! `slicing::blockers`).
//!
//! - [`JobBlockers`]: the Printer lifecycle blocker source (archive,
//!   delete). Archive is blocked by an *active* Job (any non-terminal
//!   state, including `outcomeUnknown`); delete is blocked by *any* Job
//!   history (owner decision 7: archive it instead) or by an open Queue
//!   Entry still pinned to the Printer.
//! - [`QueueOrJobBlocksRevisionDeletion`]: the Slice Revision deletion
//!   blocker source (any Queue Entry, in any state, or any Job).
//! - [`check_import`]: `PrinterRepository::replace_all` (`JOBS_EXIST`) --
//!   any Job at all, or any open Queue Entry pinned to a Printer, rejects
//!   the whole import before anything is written.
//!
//! The Connection-change guard needs no new code here: P6's
//! `host_ops::guards::check_connection_change` already only looks at
//! unresolved Host Operations, which is exactly what ruling R5(b) wants
//! (an active Job alone never blocks it). That check's `RepositoryError::
//! ConnectionInUse` now carries the unresolved operation's own `job_id`,
//! so a Job-linked block gets the Job-specific `CONNECTION_IN_USE`
//! message and `recovery` (`contracts::command::CommandError::
//! connection_in_use`).
//!
//! `spools::lifecycle::ensure_unreserved` already blocks a Spool archive
//! or mark-empty on any open reservation regardless of its holder, so a
//! Job's reservation (`holder_kind = 'job'`) blocks it unchanged (P3,
//! D8) -- nothing here registers anything for it.

use rusqlite::Transaction;

use crate::persistence::RepositoryError;
use crate::printers::lifecycle::{
    LifecycleAction, LifecycleBlocker, LifecycleBlockerCode, LifecycleBlockerSource,
};
use crate::printers::StoredPrinter;
use crate::slicing::blockers::SliceRevisionDeletionBlocker;

/// D8: Archive is blocked while the Printer has an active (non-terminal)
/// Job. Delete is blocked while any Job (terminal or not) references the
/// Printer -- `jobs.printer_id` is `ON DELETE RESTRICT`, and decision 7
/// keeps Job history through an archive, not a delete -- or while an open
/// Queue Entry is still pinned to it through `manual_printer_id`.
pub struct JobBlockers;

impl LifecycleBlockerSource for JobBlockers {
    fn blockers(
        &self,
        printer: &StoredPrinter,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, crate::persistence::StorageError> {
        let mut blockers = Vec::new();

        let active: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM jobs
                 WHERE printer_id = ?1 AND state NOT IN ('completed','failed','cancelled')
             )",
            [&printer.id],
            |row| row.get(0),
        )?;
        if active {
            blockers.push(LifecycleBlocker {
                action: LifecycleAction::Archive,
                code: LifecycleBlockerCode::JobActive,
                message: "Finish, cancel, or release this Printer's Job before archiving."
                    .to_string(),
            });
        }

        let has_history: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM jobs WHERE printer_id = ?1)",
            [&printer.id],
            |row| row.get(0),
        )?;
        if has_history {
            blockers.push(LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::JobHistoryExists,
                message: "This Printer has Job history. Archive it instead.".to_string(),
            });
        }

        let pinned: bool = tx.query_row(
            "SELECT EXISTS(
                 SELECT 1 FROM queue_entries
                 WHERE manual_printer_id = ?1 AND state <> 'closed'
             )",
            [&printer.id],
            |row| row.get(0),
        )?;
        if pinned {
            blockers.push(LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::QueueEntryPinned,
                message: "A Queue Entry is pinned to this Printer. Remove it or change its \
                          Printer first."
                    .to_string(),
            });
        }

        Ok(blockers)
    }
}

/// D8 `QUEUE_REFERENCES_REVISION`: blocks deleting a Slice Revision that
/// any Queue Entry (any state) or any Job still references --
/// `queue_entries.slice_revision_id` and `jobs.slice_revision_id` are both
/// `ON DELETE RESTRICT`.
pub struct QueueOrJobBlocksRevisionDeletion;

impl SliceRevisionDeletionBlocker for QueueOrJobBlocksRevisionDeletion {
    fn blockers(
        &self,
        slice_revision_id: &str,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, crate::persistence::StorageError> {
        let referenced: bool = tx.query_row(
            "SELECT EXISTS(SELECT 1 FROM queue_entries WHERE slice_revision_id = ?1)
                OR EXISTS(SELECT 1 FROM jobs WHERE slice_revision_id = ?1)",
            [slice_revision_id],
            |row| row.get(0),
        )?;
        Ok(if referenced {
            vec![LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::QueueReferencesRevision,
                message: "Queue Entries or Jobs use this Slice Revision.".to_string(),
            }]
        } else {
            Vec::new()
        })
    }
}

/// D8 `JOBS_EXIST` for `import_printers` (`PrinterRepository::
/// replace_all`): rejects the whole import, nothing written, while any
/// Job exists anywhere (any state -- `replace_all` deletes every Printer,
/// and `jobs.printer_id` is `ON DELETE RESTRICT`, so even a finished Job
/// would make the delete fail mid-import; decision 7 keeps Job history,
/// and P9 owns pruning, not an import) or any open Queue Entry is pinned
/// to a Printer. Each returned list is capped at 20.
pub fn check_import(tx: &Transaction<'_>) -> Result<(), RepositoryError> {
    let mut printer_ids = std::collections::BTreeSet::new();
    let mut job_ids = Vec::new();
    {
        let mut statement = tx.prepare("SELECT id, printer_id FROM jobs ORDER BY created_at, id")?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (job_id, printer_id) = row?;
            job_ids.push(job_id);
            printer_ids.insert(printer_id);
        }
    }
    let mut queue_entry_ids = Vec::new();
    {
        let mut statement = tx.prepare(
            "SELECT id, manual_printer_id FROM queue_entries
             WHERE state <> 'closed' AND manual_printer_id IS NOT NULL
             ORDER BY created_at, id",
        )?;
        let rows = statement.query_map([], |row| {
            Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
        })?;
        for row in rows {
            let (entry_id, printer_id) = row?;
            queue_entry_ids.push(entry_id);
            printer_ids.insert(printer_id);
        }
    }
    if job_ids.is_empty() && queue_entry_ids.is_empty() {
        return Ok(());
    }
    Err(RepositoryError::JobsExist {
        printer_ids: printer_ids.into_iter().take(20).collect(),
        job_ids: job_ids.into_iter().take(20).collect(),
        queue_entry_ids: queue_entry_ids.into_iter().take(20).collect(),
    })
}
