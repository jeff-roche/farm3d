//! D10: the P3 source of Printer lifecycle blockers. Registered in
//! `printers::lifecycle::blocker_sources()`, so every archive, delete, and
//! eligibility query runs it inside the transaction it gates.

use rusqlite::Transaction;

use crate::persistence::StorageError;
use crate::printers::lifecycle::{
    LifecycleAction, LifecycleBlocker, LifecycleBlockerCode, LifecycleBlockerSource,
};
use crate::printers::StoredPrinter;

use super::repository;

/// `SPOOLS_LOADED` for `archive` while any of the Printer's slots holds a
/// Spool, and for `delete` as a safety check. An archived Printer can't
/// hold Spools (archive relocates them first, and nothing loads into an
/// archived Printer), so the delete blocker only fires if that invariant
/// were ever broken.
pub struct LoadedSpoolBlockers;

impl LifecycleBlockerSource for LoadedSpoolBlockers {
    fn blockers(
        &self,
        printer: &StoredPrinter,
        tx: &Transaction<'_>,
    ) -> Result<Vec<LifecycleBlocker>, StorageError> {
        let loaded = repository::loaded_on_printer(tx, &printer.id)?.len();
        if loaded == 0 {
            return Ok(Vec::new());
        }
        let (spools, them) = if loaded == 1 {
            ("a Spool", "it")
        } else {
            ("Spools", "them")
        };
        Ok(vec![
            LifecycleBlocker {
                action: LifecycleAction::Archive,
                code: LifecycleBlockerCode::SpoolsLoaded,
                message: format!(
                    "This Printer has {spools} loaded. Relocate {them} before archiving."
                ),
            },
            LifecycleBlocker {
                action: LifecycleAction::Delete,
                code: LifecycleBlockerCode::SpoolsLoaded,
                message: format!(
                    "This Printer has {spools} loaded. Unload {them} before deleting."
                ),
            },
        ])
    }
}
