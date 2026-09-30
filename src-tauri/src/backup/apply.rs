//! `apply_restore`'s core without Tauri (spec D8 `apply_restore` steps 6
//! and 8–10): the journal checks, the carry, and the `pending` journal.
//! `backup::commands::apply_restore` wraps it with the confirmation, the
//! ledger, the lease, the blocker re-check, the safety backup, and the
//! restart.

use std::collections::{BTreeMap, BTreeSet};

use chrono::{DateTime, SecondsFormat, Utc};
use rusqlite::OptionalExtension;

use super::inventory::table_counts;
use super::journal::{
    self, CarriedCredentialCleanup, CarriedSlicerRuntime, Carry, JournalPhase, RestoreJournal,
};
use super::preview;
use super::staging::StagedCandidate;
use crate::contracts::command::CommandError;
use crate::persistence::{Storage, StoragePaths};

/// The exact confirmation phrase (no trim, no case folding).
pub const CONFIRMATION: &str = "restore";

/// `RESTART_PENDING` while a `pending`, `installing`, or `installed`
/// journal waits for the restart (D8 step 6). A journal that can't be read
/// refuses too: startup stops on it, so nothing else may start.
pub fn refuse_if_restart_pending(paths: &StoragePaths) -> Result<(), CommandError> {
    match journal::read(paths) {
        Ok(None) => Ok(()),
        Ok(Some(current)) if current.phase.is_finished() => Ok(()),
        Ok(Some(current)) => Err(CommandError::restart_pending(
            &current.id,
            current.kind.as_str(),
        )),
        Err(_) => Err(CommandError::persistence_unavailable()),
    }
}

/// D8 step 6: a finished (`done` or `failed`) journal is acknowledged
/// (deleted with its directory) before a new one is written.
pub fn acknowledge_finished(paths: &StoragePaths) -> Result<(), CommandError> {
    refuse_if_restart_pending(paths)?;
    if let Ok(Some(finished)) = journal::read(paths) {
        journal::remove(paths, &finished.id)
            .map_err(|_| CommandError::persistence_unavailable())?;
    }
    Ok(())
}

/// What D8 step 8 reads from the live Farm in one read transaction.
struct LocalState {
    slicer_runtime: Option<CarriedSlicerRuntime>,
    pending: Vec<CarriedCredentialCleanup>,
    refs: BTreeSet<String>,
}

fn local_state(storage: &Storage) -> Result<LocalState, CommandError> {
    storage
        .read_transaction(|tx| {
            let slicer_runtime = tx
                .query_row(
                    "SELECT engine_path, preset_source_path FROM slicer_runtime_config
                      WHERE singleton_id = 1",
                    [],
                    |row| {
                        Ok(CarriedSlicerRuntime {
                            engine_path: row.get(0)?,
                            preset_source_path: row.get(1)?,
                        })
                    },
                )
                .optional()?;
            let pending = tx
                .prepare(
                    "SELECT credential_ref, printer_id, reason, attempt_count, last_error_code,
                            created_at, last_attempt_at
                       FROM pending_credential_cleanup ORDER BY credential_ref",
                )?
                .query_map([], |row| {
                    Ok(CarriedCredentialCleanup {
                        credential_ref: row.get(0)?,
                        printer_id: row.get(1)?,
                        reason: row.get(2)?,
                        attempt_count: row.get(3)?,
                        last_error_code: row.get(4)?,
                        created_at: row.get(5)?,
                        last_attempt_at: row.get(6)?,
                    })
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            let refs = preview::local_refs(tx)?;
            Ok(LocalState {
                slicer_runtime,
                pending,
                refs,
            })
        })
        .map_err(|_| CommandError::persistence_unavailable())
}

/// What the candidate holds that the journal needs: its Printers' refs and
/// its per-table counts.
fn candidate_facts(
    candidate: &StagedCandidate,
) -> Result<(BTreeSet<String>, BTreeMap<String, i64>), CommandError> {
    let connection =
        preview::open_candidate(candidate).map_err(|_| CommandError::persistence_unavailable())?;
    let refs = preview::printer_refs(&connection)
        .map_err(|_| CommandError::persistence_unavailable())?
        .into_iter()
        .map(|(_, reference)| reference)
        .collect();
    let counts = table_counts(&connection).map_err(|_| CommandError::persistence_unavailable())?;
    Ok((refs, counts))
}

/// D8 steps 8–10: reads the carry, computes `expectedCounts`, and writes
/// the `pending` journal atomically. The caller holds the lease and has
/// written the safety backup.
pub fn write_pending_journal(
    storage: &Storage,
    candidate: &StagedCandidate,
    safety_backup_id: &str,
    now: DateTime<Utc>,
) -> Result<RestoreJournal, CommandError> {
    write_pending_journal_with(storage, candidate, safety_backup_id, now, journal::write)
}

/// [`write_pending_journal`] with the journal write injected (tests fail
/// it after its rename). See [`journal::write_confirmed`].
pub fn write_pending_journal_with(
    storage: &Storage,
    candidate: &StagedCandidate,
    safety_backup_id: &str,
    now: DateTime<Utc>,
    write: impl FnOnce(&StoragePaths, &RestoreJournal) -> std::io::Result<()>,
) -> Result<RestoreJournal, CommandError> {
    let local = local_state(storage)?;
    let (candidate_refs, expected_counts) = candidate_facts(candidate)?;
    // A local row whose ref the restored Printers use again is dropped: an
    // automatic reason would let F1's startup retry delete it.
    let pending = local
        .pending
        .into_iter()
        .filter(|row| !candidate_refs.contains(&row.credential_ref))
        .collect();
    let orphans = local
        .refs
        .into_iter()
        .filter(|reference| !candidate_refs.contains(reference))
        .collect();
    let restore = RestoreJournal::new_restore(
        journal::new_restore_id(),
        now.to_rfc3339_opts(SecondsFormat::Millis, true),
        candidate.staging_id.clone(),
        safety_backup_id.to_string(),
        expected_counts,
        Carry {
            slicer_runtime: local.slicer_runtime,
            pending_credential_cleanup: pending,
        },
        orphans,
    );
    debug_assert_eq!(restore.phase, JournalPhase::Pending);
    journal::write_confirmed(storage.paths(), &restore, write)
        .map_err(|_| CommandError::persistence_unavailable())?;
    Ok(restore)
}
