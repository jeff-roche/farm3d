//! D5's inputs, read. [`World::read`] loads every durable fact
//! `eligibility::evaluate`/`check_assignment` need from the caller's
//! transaction — Printers with their resolved profiles, Spools, which
//! Printers have an active Job or an unresolved Host Operation of their
//! own, and each Printer's `lastUsedAt` — and joins in the live facts only
//! memory holds (statuses and capabilities) through a [`WorldReader`].
//!
//! The assign transaction reads its `World` inside the same `write_repo`
//! transaction that commits the Job, so its re-check sees exactly the rows
//! it is about to commit against (D4, D5). `list_queue` and
//! `explain_queue_entry` read theirs inside one read transaction.

use std::collections::{BTreeMap, BTreeSet, HashMap};

use rusqlite::Transaction;

use crate::catalog::resolve::resolve_printer;
use crate::catalog::{Catalog, PrinterProfile};
use crate::connections::capabilities::PrinterCapabilities;
use crate::connections::PrinterStatus;
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::repository as printers_repository;
use crate::printers::StoredPrinter;
use crate::slicing::facts::SliceFacts;
use crate::slicing::repository as slicing_repository;
use crate::spools::{repository as spools_repository, SpoolLocation, SpoolRecord};

use super::eligibility::{EligibilityInput, PrinterView};
use super::QueueEntry;

/// The live, in-memory half of D5's input: what the Connection supervisor
/// last observed, what the host-ops capability rules say now, and the
/// catalog Printers resolve their profiles against.
pub trait WorldReader {
    fn status(&self, printer_id: &str) -> Option<PrinterStatus>;
    fn capabilities(&self, printer: &StoredPrinter) -> PrinterCapabilities;
    fn catalog(&self) -> &Catalog;
}

/// The app's [`WorldReader`]: one snapshot of the supervisor's statuses,
/// with capabilities from the host-ops services (the same D6 rules the P6
/// commands apply).
pub struct LiveWorld<'a, R: tauri::Runtime> {
    statuses: HashMap<String, PrinterStatus>,
    host_ops: &'a crate::host_ops::HostOperationServices<R>,
    catalog: &'a Catalog,
}

impl<'a, R: tauri::Runtime> LiveWorld<'a, R> {
    pub fn of(services: &'a crate::RuntimeServices<R>) -> Self {
        Self {
            statuses: services.manager.statuses(),
            host_ops: &services.host_ops,
            catalog: &services.catalog,
        }
    }
}

impl<R: tauri::Runtime> WorldReader for LiveWorld<'_, R> {
    fn status(&self, printer_id: &str) -> Option<PrinterStatus> {
        self.statuses.get(printer_id).cloned()
    }

    fn capabilities(&self, printer: &StoredPrinter) -> PrinterCapabilities {
        self.host_ops.capabilities(printer)
    }

    fn catalog(&self) -> &Catalog {
        self.catalog
    }
}

/// One Printer, joined: the stored row, its resolved profile, its live
/// status and capabilities, and the durable facts D5 gate 1 and ranking
/// read.
pub struct PrinterRow {
    pub stored: StoredPrinter,
    pub profile: PrinterProfile,
    pub status: Option<PrinterStatus>,
    pub capabilities: PrinterCapabilities,
    pub active_job: bool,
    pub foreign_host_op: bool,
    pub last_used_at: Option<String>,
    pub loaded_spool_ids: Vec<String>,
}

/// Every Printer (archived ones too; [`World::views`] filters) and every
/// Spool, as one transaction saw them.
pub struct World {
    pub printers: Vec<PrinterRow>,
    pub spools: Vec<SpoolRecord>,
    claimed: BTreeSet<String>,
}

fn string_set(tx: &Transaction<'_>, sql: &str) -> Result<BTreeSet<String>, StorageError> {
    let mut statement = tx.prepare(sql)?;
    let rows = statement
        .query_map([], |row| row.get::<_, String>(0))?
        .collect::<rusqlite::Result<BTreeSet<_>>>()?;
    Ok(rows)
}

impl World {
    pub fn read(tx: &Transaction<'_>, reader: &dyn WorldReader) -> Result<World, RepositoryError> {
        let spools = spools_repository::list_spools(tx)?;
        let active_jobs = string_set(
            tx,
            "SELECT printer_id FROM jobs WHERE state NOT IN ('completed','failed','cancelled')",
        )?;
        // D5 gate 1's `HOST_OPERATION_PENDING` is a Printer's own unresolved
        // write; one linked to a Job is that Job's, and `JOB_ACTIVE` covers
        // it.
        let foreign_host_ops = string_set(
            tx,
            "SELECT DISTINCT printer_id FROM host_operations
             WHERE state IN ('dispatching','uncertain','reconciling') AND job_id IS NULL",
        )?;
        let last_used: BTreeMap<String, String> = {
            let mut statement = tx.prepare(
                "SELECT printer_id, MAX(ended_at) FROM jobs
                 WHERE state IN ('completed','failed','cancelled') GROUP BY printer_id",
            )?;
            let rows = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, String>(1)?))
                })?
                .collect::<rusqlite::Result<BTreeMap<_, _>>>()?;
            rows
        };
        let printer_ids: Vec<String> = {
            let mut statement = tx.prepare("SELECT id FROM printers ORDER BY CAST(id AS BLOB)")?;
            let ids = statement
                .query_map([], |row| row.get::<_, String>(0))?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            ids
        };

        let mut printers = Vec::with_capacity(printer_ids.len());
        for id in printer_ids {
            let stored = printers_repository::load_in(tx, &id)?;
            let profile = resolve_printer(reader.catalog(), &stored)
                .profile_resolution
                .profile;
            let loaded_spool_ids = spools
                .iter()
                .filter(|spool| {
                    matches!(&spool.location, SpoolLocation::Slot { printer_id, .. } if *printer_id == id)
                })
                .map(|spool| spool.id.clone())
                .collect();
            printers.push(PrinterRow {
                status: reader.status(&id),
                capabilities: reader.capabilities(&stored),
                active_job: active_jobs.contains(&id),
                foreign_host_op: foreign_host_ops.contains(&id),
                last_used_at: last_used.get(&id).cloned(),
                loaded_spool_ids,
                profile,
                stored,
            });
        }
        Ok(World {
            printers,
            spools,
            claimed: BTreeSet::new(),
        })
    }

    /// D5's Printer set for `entry`: the unarchived Printers, plus the
    /// entry's `manualPrinterId` even if archived (gate 0), plus `also`
    /// even if archived — the assign re-check's target, so gate 1 reports
    /// `PRINTER_ARCHIVED` for it rather than the Printer going missing.
    pub fn views(&self, entry: &QueueEntry, also: Option<&str>) -> Vec<PrinterView<'_>> {
        let pin = entry.manual_printer_id.as_deref();
        self.printers
            .iter()
            .filter(|row| {
                row.stored.archived_at.is_none()
                    || Some(row.stored.id.as_str()) == pin
                    || Some(row.stored.id.as_str()) == also
            })
            .map(|row| PrinterView {
                printer: &row.stored,
                profile: &row.profile,
                status: row.status.as_ref(),
                capabilities: &row.capabilities,
                active_job: row.active_job,
                foreign_host_op: row.foreign_host_op,
                last_used_at: row.last_used_at.as_deref(),
                loaded_spool_ids: &row.loaded_spool_ids,
            })
            .collect()
    }

    /// The `EligibilityInput` for `entry` over `views` (from
    /// [`World::views`]) and the entry's Slice facts.
    pub fn input<'a>(
        &'a self,
        entry: &'a QueueEntry,
        facts: &'a SliceFacts,
        views: &'a [PrinterView<'a>],
    ) -> EligibilityInput<'a> {
        EligibilityInput {
            entry,
            facts,
            printers: views,
            spools: &self.spools,
            claimed_printers: &self.claimed,
        }
    }

    pub fn spool(&self, spool_id: &str) -> Option<&SpoolRecord> {
        self.spools.iter().find(|spool| spool.id == spool_id)
    }
}

/// The Slice facts of `entry`'s Slice Revision. The revision is
/// `ON DELETE RESTRICT` from `queue_entries`, so a missing one is
/// corruption.
pub fn facts_for(tx: &Transaction<'_>, entry: &QueueEntry) -> Result<SliceFacts, StorageError> {
    slicing_repository::load_revision(tx, &entry.slice_revision_id)?
        .map(|record| record.summary.facts)
        .ok_or(StorageError::CorruptData {
            source_name: "queue_entries.slice_revision_id",
            source_sha256: None,
        })
}
