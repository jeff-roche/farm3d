use std::sync::Arc;

use rusqlite::{params, OptionalExtension};

use crate::persistence::{RepositoryError, Storage, StorageError};
use crate::spools::dispositions::{apply_dispositions, SpoolDispositionInput};
use crate::spools::movement::{self, MoveOutcome};
use crate::spools::operations::{self, Claim, OperationKind};
use crate::spools::slots::{self, InitialLoad, SlotSpec};

use super::create::PrinterSetupExtras;
use super::host_identity::canonical_host_identity;
use super::lifecycle::{evaluate, LifecycleAction};
use super::{StartSafety, StoredPrinter};
use crate::connections::ConnectionConfig;
use crate::host_ops::repository as host_operations;

/// Shared column list for every `SELECT ... FROM printers` — keeps the four
/// P2 columns (`location`, `start_safety`, `archived_at`, `host_identity`)
/// in lockstep with `decode` across `list`/`get`/`update`/`delete`/
/// `set_connection` instead of four separately hand-maintained strings.
const PRINTER_COLUMNS: &str = "id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, location, start_safety, archived_at, host_identity, created_at, updated_at";

pub struct PrinterRepository {
    storage: Arc<Storage>,
}

/// What a committed [`PrinterRepository::delete_with_outcome`] changed.
#[derive(Debug)]
pub struct PrinterDeletion {
    /// The deleted row, as it was.
    pub printer: StoredPrinter,
    /// P8 D8: the Printer's open Events, now resolved `sourceRemoved`
    /// (and any Incident that changed with them), to publish.
    pub resolved: crate::incidents::guards::ResolvedPrinterEvents,
}

/// What a committed [`PrinterRepository::replace_all_with_setup`] changed.
#[derive(Debug)]
pub struct ReplaceAllOutcome {
    /// The imported Printers, sorted by id.
    pub stored: Vec<StoredPrinter>,
    /// Every camera source stored before the import, so the caller can
    /// tell which Printers' sources changed.
    pub previous_cameras: std::collections::HashMap<String, crate::cameras::CameraSource>,
    /// P8 D8: every replaced Printer's open Events, now resolved
    /// `sourceRemoved`, to publish.
    pub resolved_events: Vec<crate::attention::AttentionEvent>,
}

impl PrinterRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn generate_id() -> String {
        format!("prn-{}", uuid::Uuid::new_v4())
    }

    pub fn enqueue_credential_cleanup(
        &self,
        reference: &str,
        printer_id: Option<&str>,
        reason: &str,
    ) -> Result<(), StorageError> {
        self.storage.write(|transaction| {
            enqueue_credential_cleanup(transaction, reference, printer_id, reason)
        })
    }

    /// D4/D12 (global constraints clarification 1): one extra query beyond
    /// the Printer rows themselves — `slots::live_slots_by_printer` loads
    /// every Printer's live slots in a single batched query, grouped by
    /// `printer_id`, rather than one extra query per row.
    pub fn list(&self) -> Result<Vec<StoredPrinter>, StorageError> {
        self.storage
            .read(|connection| {
                let mut statement = connection.prepare(&format!(
                    "SELECT {PRINTER_COLUMNS} FROM printers ORDER BY CAST(id AS BLOB)"
                ))?;
                let printers = statement
                    .query_map([], decode)?
                    .collect::<rusqlite::Result<Vec<_>>>()?;
                Ok(
                    slots::live_slots_by_printer(connection).map(|mut by_printer| {
                        printers
                            .into_iter()
                            .map(|mut printer| {
                                printer.material_slots =
                                    by_printer.remove(&printer.id).unwrap_or_default();
                                printer
                            })
                            .collect::<Vec<_>>()
                    }),
                )
            })
            .and_then(|inner| inner)
    }

    /// D4/D12: fills `material_slots` with one extra query, scoped to this
    /// Printer (global constraints clarification 1).
    pub fn get(&self, id: &str) -> Result<Option<StoredPrinter>, StorageError> {
        self.storage
            .read(|connection| {
                let printer = connection
                    .query_row(
                        &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                        [id],
                        decode,
                    )
                    .optional()?;
                Ok(match printer {
                    None => Ok(None),
                    Some(mut printer) => slots::live_slots(connection, id).map(|found| {
                        printer.material_slots = found;
                        Some(printer)
                    }),
                })
            })
            .and_then(|inner| inner)
    }

    /// `printer_lifecycle_eligibility`'s read path: loads the Printer and
    /// evaluates its lifecycle eligibility inside the same read transaction,
    /// so a caller never observes a state in between.
    pub fn lifecycle_eligibility(
        &self,
        id: &str,
    ) -> Result<Option<super::lifecycle::LifecycleEligibility>, StorageError> {
        self.storage
            .read_transaction(|transaction| {
                let printer = transaction
                    .query_row(
                        &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                        [id],
                        decode,
                    )
                    .optional()?;
                Ok(match printer {
                    None => Ok(None),
                    Some(printer) => evaluate(&printer, transaction).map(Some),
                })
            })
            .and_then(|inner| inner)
    }

    /// The active (non-archived) Printer, if any, currently holding `identity`
    /// — the repository-side half of D3's duplicate-host enforcement.
    /// `excluding` is the printer being written, so it never conflicts with
    /// its own not-yet-committed row.
    pub fn find_active_by_host_identity(
        &self,
        identity: &str,
        excluding: Option<&str>,
    ) -> Result<Option<String>, StorageError> {
        self.storage
            .read(|connection| active_printer_with_identity(connection, identity, excluding))
    }

    pub fn create(&self, mut printer: StoredPrinter) -> Result<StoredPrinter, RepositoryError> {
        validate_id(&printer.id).map_err(|_| RepositoryError::Validation { field_path: "id" })?;
        let now = crate::printers::now_rfc3339();
        printer.revision = 1;
        printer.created_at = now.clone();
        printer.updated_at = now;
        self.storage
            .write(|transaction| {
                precheck_duplicate_host(transaction, &printer)?;
                insert(transaction, &printer)?;
                printer.material_slots = slots::live_slots(transaction, &printer.id)?;
                Ok(printer.clone())
            })
            .map_err(duplicate_host_or_storage)
    }

    /// `create`'s sibling for the credential-provisioning path
    /// (`create_printer_with`): inserts the Printer and, in the SAME
    /// transaction, deletes `provisional_reference`'s
    /// `pending_credential_cleanup` row — mirroring how `set_connection`
    /// removes a provisional row on a successful commit. On failure the
    /// provisional row is left in place (its secret is orphaned, and the
    /// caller retries cleanup for it) rather than deleted here, since the
    /// insert itself rolled back.
    pub fn create_in(
        &self,
        mut printer: StoredPrinter,
        provisional_reference: Option<&str>,
    ) -> Result<StoredPrinter, RepositoryError> {
        validate_id(&printer.id).map_err(|_| RepositoryError::Validation { field_path: "id" })?;
        let now = crate::printers::now_rfc3339();
        printer.revision = 1;
        printer.created_at = now.clone();
        printer.updated_at = now;
        self.storage
            .write(|transaction| {
                precheck_duplicate_host(transaction, &printer)?;
                insert(transaction, &printer)?;
                if let Some(reference) = provisional_reference {
                    transaction.execute(
                        "DELETE FROM pending_credential_cleanup WHERE credential_ref=?1",
                        [reference],
                    )?;
                }
                printer.material_slots = slots::live_slots(transaction, &printer.id)?;
                Ok(printer.clone())
            })
            .map_err(duplicate_host_or_storage)
    }

    /// `create_in`'s sibling for P3's `create_printer`/batch create (Task
    /// 5, D4/D12): inserts the Printer, its Material Slot layout
    /// (`slots::insert_layout`), and every initial load
    /// (`movement::apply_moves`, sharing one generated `operationId`) in
    /// the SAME transaction — so a rejected initial load (an unloadable
    /// slot, a stale `expectedSpoolRevision`) rolls back the whole create,
    /// Printer row included. `apply_moves` bumps the Printer's own revision
    /// once per occupied slot, so the row is re-read after the loads rather
    /// than left at the just-inserted revision 1. Returns the Printer with
    /// `material_slots` already populated (each occupied by its initial
    /// load, if any).
    pub fn create_with_layout(
        &self,
        printer: StoredPrinter,
        provisional_reference: Option<&str>,
        slot_layout: &[SlotSpec],
        initial_loads: &[InitialLoad],
    ) -> Result<StoredPrinter, RepositoryError> {
        self.create_with_setup(
            printer,
            provisional_reference,
            slot_layout,
            initial_loads,
            &PrinterSetupExtras::default(),
        )
    }

    /// [`create_with_layout`](Self::create_with_layout), plus P8's setup
    /// rows: the camera source and alert defaults in `setup` (already
    /// validated) are written right after the Printer row, in the same
    /// transaction, so anything that rolls the create back (an invalid
    /// layout, a rejected initial load) takes them with it.
    pub fn create_with_setup(
        &self,
        mut printer: StoredPrinter,
        provisional_reference: Option<&str>,
        slot_layout: &[SlotSpec],
        initial_loads: &[InitialLoad],
        setup: &PrinterSetupExtras,
    ) -> Result<StoredPrinter, RepositoryError> {
        validate_id(&printer.id).map_err(|_| RepositoryError::Validation { field_path: "id" })?;
        let now = crate::printers::now_rfc3339();
        printer.revision = 1;
        printer.created_at = now.clone();
        printer.updated_at = now;
        self.storage.write_repo(|transaction| {
            precheck_duplicate_host(transaction, &printer)?;
            insert(transaction, &printer)?;
            if let Some(reference) = provisional_reference {
                transaction.execute(
                    "DELETE FROM pending_credential_cleanup WHERE credential_ref=?1",
                    [reference],
                )?;
            }
            write_setup(transaction, &printer.id, setup, &printer.created_at)?;
            let created_slots = slots::insert_layout(transaction, &printer.id, slot_layout)?;
            if let Some((revision, updated_at)) = crate::spools::initial_loads::apply_initial_loads(
                transaction,
                &printer.id,
                &created_slots,
                initial_loads,
            )? {
                printer.revision = revision;
                printer.updated_at = updated_at;
            }
            printer.material_slots = slots::live_slots(transaction, &printer.id)?;
            Ok(printer.clone())
        })
    }

    pub fn update(
        &self,
        id: &str,
        expected_revision: i64,
        mutate: impl FnOnce(&mut StoredPrinter),
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let result = self.storage.write(|transaction| {
            let mut printer = transaction
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision || expected_revision == 0 {
                return Err(StorageError::OperationFailed);
            }
            mutate(&mut printer);
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            precheck_duplicate_host(transaction, &printer)?;
            replace(transaction, &printer)?;
            printer.material_slots = slots::live_slots(transaction, id)?;
            Ok(printer)
        });
        result.map_err(|error| classify_entity_write(error, self, id, expected_revision))
    }

    /// [`delete_with_outcome`](Self::delete_with_outcome) at the current
    /// time, returning only the deleted row.
    pub fn delete(
        &self,
        id: &str,
        expected_revision: i64,
    ) -> Result<StoredPrinter, RepositoryError> {
        self.delete_with_outcome(id, expected_revision, chrono::Utc::now())
            .map(|deletion| deletion.printer)
    }

    /// D6: deletes an archived Printer, in one transaction: the revision
    /// check, the lifecycle guards (P7's Job history, P8 D8's Incident
    /// history and pinned evidence, ...), the Printer's terminal Host
    /// Operation rows (P6 owner decision 5), then P8 D8's delete-time
    /// steps -- its open Attention Events resolve `sourceRemoved` (at
    /// `now`), its unattached `manual` snapshot rows that are unpinned or
    /// pruned are removed -- and finally the row itself.
    /// `attention_events.printer_id` becomes NULL (`ON DELETE SET NULL`;
    /// each Event keeps its subject), and `printer_cameras` and
    /// `printer_alert_defaults` cascade. After commit, the removed
    /// snapshots' files are unlinked; the caller publishes the resolved
    /// Events and pokes the projector.
    pub fn delete_with_outcome(
        &self,
        id: &str,
        expected_revision: i64,
        now: chrono::DateTime<chrono::Utc>,
    ) -> Result<PrinterDeletion, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let (deletion, rel_paths) = self.storage.write_repo(|transaction| {
            let printer = load_for_write(transaction, id)?;
            if printer.revision != expected_revision {
                return Err(RepositoryError::Conflict {
                    entity_id: id.to_string(),
                    expected_revision,
                    current_revision: printer.revision,
                });
            }
            let eligibility = evaluate(&printer, transaction)?;
            if !eligibility.can_delete {
                return Err(RepositoryError::LifecycleBlocked(blockers_for(
                    eligibility,
                    LifecycleAction::Delete,
                )));
            }
            // P6 owner decision 5: the Printer's terminal Host Operation
            // rows go with it. The blocker check above has just proved
            // none is unresolved (`HostOperationBlockers`), and
            // `ON DELETE RESTRICT` backs that up.
            host_operations::delete_terminal_for_printer(transaction, id)?;
            // P8 D8 steps 1 and 2. The guards above have proved no
            // Incident, no Job, and no pinned unpruned unattached snapshot
            // references the Printer, so these leave nothing behind that
            // `ON DELETE RESTRICT` would refuse.
            let resolved = crate::incidents::guards::resolve_printer_events(transaction, id, now)?;
            let rel_paths = crate::cameras::media::remove_unattached_manual(transaction, id)?;
            transaction.execute("DELETE FROM printers WHERE id = ?1", [id])?;
            if let Some(reference) = printer
                .connection
                .as_ref()
                .and_then(|connection| connection.credential_ref.as_deref())
            {
                enqueue_credential_cleanup(transaction, reference, Some(id), "printer_deleted")?;
            }
            Ok((PrinterDeletion { printer, resolved }, rel_paths))
        })?;
        // P8 D8 step 4: the removed rows' files, after commit. One that
        // can't be removed now has no row left, so the next startup sweep
        // deletes it.
        let media = crate::cameras::media::MediaStore::for_storage(&self.storage);
        for rel_path in &rel_paths {
            media.unlink(rel_path);
        }
        Ok(deletion)
    }

    /// D6/P3 D10: moves a Printer into the archived state, relocating its
    /// loaded Spools first. One transaction: the revision check, every
    /// disposition (`spools::dispositions::apply_dispositions`, one
    /// movement operation under `operation_id`), the eligibility re-check,
    /// and `archived_at`. Any failure rolls all of it back. Blocked when the
    /// Printer is already archived, or still holds a Spool that
    /// `dispositions` doesn't relocate (`SPOOLS_LOADED`).
    pub fn archive(
        &self,
        id: &str,
        expected_revision: i64,
        operation_id: &str,
        dispositions: &[SpoolDispositionInput],
    ) -> Result<StoredPrinter, RepositoryError> {
        self.archive_with_outcome(id, expected_revision, operation_id, dispositions)
            .map(|(printer, _)| printer)
    }

    /// [`archive`](Self::archive), also returning the dispositions' movement
    /// outcome, so `archive_printer` can publish inventory events for every
    /// Spool and Printer the relocation touched (D11).
    ///
    /// Idempotent by `operation_id` (D10): the id is claimed in the
    /// operations ledger before the revision check, so a retry of the same
    /// request writes nothing and returns the Printer's current state with
    /// the recorded relocation (`replayed = true`) — a replay, not a
    /// `CONFLICT`, even when the archive moved no Spools. The id reused for
    /// a different request is [`RepositoryError::OperationIdReused`].
    pub fn archive_with_outcome(
        &self,
        id: &str,
        expected_revision: i64,
        operation_id: &str,
        dispositions: &[SpoolDispositionInput],
    ) -> Result<(StoredPrinter, MoveOutcome), RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let digest = operations::digest(&ArchivePrinterRequest {
            printer_id: id,
            expected_revision,
            spool_dispositions: dispositions,
        });
        self.storage.write_repo(|transaction| {
            if operations::claim(
                transaction,
                operation_id,
                OperationKind::ArchivePrinter,
                &digest,
            )? == Claim::Replay
            {
                let printer = load_in(transaction, id)?;
                let outcome = movement::find_operation(transaction, operation_id)?
                    .unwrap_or_else(|| MoveOutcome::empty(true));
                return Ok((printer, outcome));
            }
            let printer = load_for_write(transaction, id)?;
            if printer.revision != expected_revision {
                return Err(RepositoryError::Conflict {
                    entity_id: id.to_string(),
                    expected_revision,
                    current_revision: printer.revision,
                });
            }
            let outcome = apply_dispositions(transaction, id, operation_id, dispositions)?;
            // The dispositions bumped this row's revision once per slot
            // they emptied, so re-read it rather than reuse `printer`.
            let mut printer = load_for_write(transaction, id)?;
            let eligibility = evaluate(&printer, transaction)?;
            if !eligibility.can_archive {
                return Err(RepositoryError::LifecycleBlocked(blockers_for(
                    eligibility,
                    LifecycleAction::Archive,
                )));
            }
            printer.archived_at = Some(crate::printers::now_rfc3339());
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            replace(transaction, &printer)?;
            printer.material_slots = slots::live_slots(transaction, id)?;
            Ok((printer, outcome))
        })
    }

    /// D6: moves an archived Printer back to active, with the empty slots
    /// it was archived with. Goes through the same duplicate-host-identity
    /// precheck as any other write (`update`'s), so a host another active
    /// Printer has since claimed surfaces as `RepositoryError::DuplicateHost`,
    /// not a silent takeover. Blocked when the Printer isn't archived.
    pub fn unarchive(
        &self,
        id: &str,
        expected_revision: i64,
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let mut blocked: Option<Vec<super::lifecycle::LifecycleBlocker>> = None;
        let result = self.storage.write(|transaction| {
            let mut printer = transaction
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision {
                return Err(StorageError::OperationFailed);
            }
            let eligibility = evaluate(&printer, transaction)?;
            if !eligibility.can_unarchive {
                blocked = Some(blockers_for(eligibility, LifecycleAction::Unarchive));
                return Err(StorageError::OperationFailed);
            }
            printer.archived_at = None;
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            precheck_duplicate_host(transaction, &printer)?;
            replace(transaction, &printer)?;
            printer.material_slots = slots::live_slots(transaction, id)?;
            Ok(printer)
        });
        if let Some(blockers) = blocked {
            return Err(RepositoryError::LifecycleBlocked(blockers));
        }
        result.map_err(|error| classify_entity_write(error, self, id, expected_revision))
    }

    pub fn set_connection(
        &self,
        id: &str,
        expected_revision: i64,
        connection: Option<ConnectionConfig>,
        provisional_reference: Option<&str>,
        removed_reason: &str,
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        let mut in_use: Option<RepositoryError> = None;
        let result = self.storage.write(|transaction| {
            let mut printer = transaction
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()?
                .ok_or(StorageError::OperationFailed)?;
            if printer.revision != expected_revision || expected_revision <= 0 {
                return Err(StorageError::OperationFailed);
            }
            // P6 D7: never take the endpoint or credential out from under
            // an unresolved Host Operation.
            if let Err(error) = crate::host_ops::guards::check_connection_change(
                transaction,
                id,
                printer.connection.as_ref(),
                connection.as_ref(),
            ) {
                return Err(match error {
                    RepositoryError::Storage(storage) => storage,
                    refused => {
                        in_use = Some(refused);
                        StorageError::OperationFailed
                    }
                });
            }
            let old_reference = printer
                .connection
                .as_ref()
                .and_then(|value| value.credential_ref.clone());
            let new_reference = connection
                .as_ref()
                .and_then(|value| value.credential_ref.clone());
            printer.connection = connection;
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            precheck_duplicate_host(transaction, &printer)?;
            replace(transaction, &printer)?;
            if let Some(reference) = provisional_reference {
                transaction.execute(
                    "DELETE FROM pending_credential_cleanup WHERE credential_ref=?1",
                    [reference],
                )?;
            }
            if old_reference != new_reference {
                if let Some(reference) = old_reference {
                    enqueue_credential_cleanup(transaction, &reference, Some(id), removed_reason)?;
                }
            }
            printer.material_slots = slots::live_slots(transaction, id)?;
            Ok(printer)
        });
        if let Some(error) = in_use {
            return Err(error);
        }
        result.map_err(|error| classify_entity_write(error, self, id, expected_revision))
    }

    /// D4/D12: sets `id`'s Material Slot layout. The
    /// `set_material_slot_layout` command in `printers::commands` calls this.
    /// Bumps the Printer's revision like any other mutation, so concurrent
    /// layout edits are still guarded by `expectedRevision`.
    pub fn set_material_slot_layout(
        &self,
        id: &str,
        expected_revision: i64,
        layout: &[SlotSpec],
    ) -> Result<StoredPrinter, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        self.storage.write_repo(|transaction| {
            let mut printer = transaction
                .query_row(
                    &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
                    [id],
                    decode,
                )
                .optional()?
                .ok_or_else(|| RepositoryError::NotFound {
                    entity_id: id.to_string(),
                })?;
            if printer.revision != expected_revision {
                return Err(RepositoryError::Conflict {
                    entity_id: id.to_string(),
                    expected_revision,
                    current_revision: printer.revision,
                });
            }
            let updated_slots = slots::set_layout(transaction, id, layout)?;
            printer.revision += 1;
            printer.updated_at = crate::printers::now_rfc3339();
            transaction.execute(
                "UPDATE printers SET revision = ?2, updated_at = ?3 WHERE id = ?1",
                params![id, printer.revision, printer.updated_at],
            )?;
            printer.material_slots = updated_slots;
            Ok(printer)
        })
    }

    /// The Printers import write path (`import_printers`). Unlike
    /// `create`/`update`/`transition`/`set_connection`, this doesn't call
    /// `precheck_duplicate_host` before each insert — it relies solely on
    /// the partial unique index (`printers_active_host_identity`) and
    /// `insert`'s `map_write_error` backstop to turn a violation into
    /// `StorageError::DuplicateHost`. That's safe only because the whole
    /// table is deleted first (so `imported` never collides with a
    /// pre-existing row) and because `import_printers` runs
    /// `host_identity::archive_duplicates` over `imported` before calling
    /// this, so no two active rows in `imported` share a host identity by
    /// the time they reach `insert`.
    /// `layouts` is each imported Printer's Material Slot layout, keyed by
    /// its (possibly reused) id — schemaVersion 3's own `materialSlots`, or
    /// `slots::default_layout()` for a 1/2 document or a v3 Printer that
    /// omitted it (Task 5, D12: "importing v3 recreates the layout with new
    /// ids").
    ///
    /// Every Spool must be unloaded (`slot_id IS NULL`) before an import can
    /// run — checked below and rejected as `RepositoryError::Validation`
    /// before the delete. `spools.slot_id` has no `ON DELETE` action (unlike
    /// `spool_movements.from_slot_id`/`to_slot_id`, which cascade), so
    /// without this check the `DELETE FROM printers` below would fail its
    /// own foreign-key check the moment a loaded Spool's Material Slot was
    /// deleted out from under it, surfacing as an opaque
    /// `PERSISTENCE_UNAVAILABLE` instead of a real validation error.
    ///
    /// P6 D7: likewise, any unresolved Host Operation rejects the whole
    /// import (`RepositoryError::HostOperationsPending`) before anything is
    /// written, and every replaced Printer's terminal Host Operation rows
    /// are deleted with it (owner decision 5), since `host_operations.
    /// printer_id` is `ON DELETE RESTRICT`.
    ///
    /// With no Spool loaded, the `DELETE FROM printers` cascades away every
    /// REPLACED Printer's Material Slot rows AND its `spool_movements`
    /// history (the migration's `ON DELETE CASCADE` on both
    /// `material_slots.printer_id` and `spool_movements.from_slot_id`/
    /// `to_slot_id`) — that history loss is accepted, not a side effect to
    /// work around, since every imported Printer's layout is inserted fresh
    /// via `slots::insert_layout` with brand-new slot ids regardless (never
    /// copied from whatever it had before the import).
    pub fn replace_all(
        &self,
        expected: &[(String, i64)],
        imported: Vec<StoredPrinter>,
        layouts: &std::collections::HashMap<String, Vec<SlotSpec>>,
    ) -> Result<Vec<StoredPrinter>, RepositoryError> {
        self.replace_all_with_setup(
            expected,
            imported,
            layouts,
            &Default::default(),
            chrono::Utc::now(),
        )
        .map(|outcome| outcome.stored)
    }

    /// [`replace_all`](Self::replace_all), plus P8's setup rows (schema 4):
    /// the replaced Printers' `printer_cameras` and `printer_alert_defaults`
    /// rows cascade with the `DELETE FROM printers`, and each imported
    /// Printer's `setups` entry (already validated) is written after its
    /// row. Also returns every camera source stored before the import, read
    /// in the same transaction, so the caller can tell which Printers'
    /// sources changed.
    ///
    /// P8 D8: any Incident or any `camera_snapshots` row rejects the whole
    /// import (`EVIDENCE_EXISTS`) before anything is written, and every
    /// replaced Printer's open Events resolve `sourceRemoved` before the
    /// `DELETE FROM printers` NULLs their `printer_id` (at `resolved_at`).
    pub fn replace_all_with_setup(
        &self,
        expected: &[(String, i64)],
        mut imported: Vec<StoredPrinter>,
        layouts: &std::collections::HashMap<String, Vec<SlotSpec>>,
        setups: &std::collections::HashMap<String, PrinterSetupExtras>,
        resolved_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<ReplaceAllOutcome, RepositoryError> {
        self.storage.write_repo(|transaction| {
            let mut statement = transaction
                .prepare("SELECT id, revision FROM printers ORDER BY CAST(id AS BLOB)")?;
            let current = statement
                .query_map([], |row| {
                    Ok((row.get::<_, String>(0)?, row.get::<_, i64>(1)?))
                })?
                .collect::<rusqlite::Result<Vec<_>>>()?;
            drop(statement);
            if current != expected {
                return Err(RepositoryError::SetConflict {
                    expected_count: expected.len(),
                    current_count: current.len(),
                });
            }
            if crate::spools::repository::any_loaded(transaction)? {
                return Err(RepositoryError::SpoolsLoadedForImport);
            }
            // P7 D8: any Job at all, or any open Queue Entry pinned to a
            // Printer, rejects the whole import up front (`JOBS_EXIST`) --
            // `jobs.printer_id` is `ON DELETE RESTRICT`, so even a
            // finished Job would otherwise make the delete below fail
            // mid-import. Checked before the (narrower) unresolved Host
            // Operation guard, since this one is decisive on its own.
            crate::jobs::guards::check_import(transaction)?;
            // P8 D8: likewise any Incident or camera snapshot
            // (`EVIDENCE_EXISTS`): both reference their Printer
            // `ON DELETE RESTRICT`.
            crate::incidents::guards::check_import(transaction)?;
            // P6 D7: an unresolved Host Operation anywhere rejects the whole
            // import. Terminal rows go with the Printers the import
            // replaces (owner decision 5), before `printer_id`'s
            // `ON DELETE RESTRICT` would refuse the delete below.
            crate::host_ops::guards::check_import(transaction)?;
            let mut resolved_events = Vec::new();
            for (printer_id, _) in &current {
                host_operations::delete_terminal_for_printer(transaction, printer_id)?;
                // P8 D8: every replaced Printer's open Events resolve
                // `sourceRemoved`. None is linked to an Incident, since
                // any Incident rejected the import above.
                resolved_events.extend(
                    crate::incidents::guards::resolve_printer_events(
                        transaction,
                        printer_id,
                        resolved_at,
                    )?
                    .events,
                );
            }
            let revisions: std::collections::HashMap<_, _> = current.into_iter().collect();
            let old_references = transaction.prepare("SELECT DISTINCT json_extract(connection_json, '$.credentialRef') FROM printers WHERE json_extract(connection_json, '$.credentialRef') IS NOT NULL")?
                .query_map([], |row| row.get::<_, String>(0))?.collect::<rusqlite::Result<std::collections::HashSet<_>>>()?;
            let imported_references = imported.iter().filter_map(|printer| printer.connection.as_ref()?.credential_ref.clone()).collect::<std::collections::HashSet<_>>();
            let previous_cameras = crate::cameras::config::list_all(transaction)?
                .into_iter()
                .map(|camera| (camera.printer_id, camera.source))
                .collect();
            transaction.execute("DELETE FROM printers", [])?;
            let now = crate::printers::now_rfc3339();
            for printer in &mut imported {
                validate_id(&printer.id)?;
                printer.revision = revisions
                    .get(&printer.id)
                    .map_or(1, |revision| revision + 1);
                if printer.created_at.is_empty() {
                    printer.created_at = now.clone();
                }
                printer.updated_at = now.clone();
                insert(transaction, printer)?;
                let default_layout = slots::default_layout();
                let layout = layouts.get(&printer.id).unwrap_or(&default_layout);
                printer.material_slots = slots::insert_layout(transaction, &printer.id, layout)?;
                if let Some(setup) = setups.get(&printer.id) {
                    write_setup(transaction, &printer.id, setup, &now)?;
                }
            }
            for reference in old_references.difference(&imported_references) {
                enqueue_credential_cleanup(
                    transaction,
                    reference,
                    None,
                    "import_orphan",
                )?;
            }
            imported.sort_by(|left, right| left.id.as_bytes().cmp(right.id.as_bytes()));
            Ok(ReplaceAllOutcome {
                stored: imported,
                previous_cameras,
                resolved_events,
            })
        })
    }
}

/// Writes a new (or just re-inserted) Printer's P8 setup rows inside the
/// caller's transaction: its camera source and its alert defaults, each at
/// revision 1. Nothing for a `None`.
fn write_setup(
    transaction: &rusqlite::Transaction<'_>,
    printer_id: &str,
    setup: &PrinterSetupExtras,
    now: &str,
) -> Result<(), RepositoryError> {
    if let Some(camera) = &setup.camera {
        crate::cameras::config::put(transaction, printer_id, camera, now)?;
    }
    if let Some(defaults) = &setup.alert_defaults {
        super::alerts::set(transaction, printer_id, defaults, now)?;
    }
    Ok(())
}

fn classify_entity_write(
    error: StorageError,
    repository: &PrinterRepository,
    id: &str,
    expected_revision: i64,
) -> RepositoryError {
    if let StorageError::DuplicateHost(conflicting_printer_id) = error {
        return RepositoryError::DuplicateHost {
            conflicting_printer_id,
        };
    }
    if !matches!(error, StorageError::OperationFailed) {
        return RepositoryError::Storage(error);
    }
    match repository.get(id) {
        Ok(None) => RepositoryError::NotFound {
            entity_id: id.to_string(),
        },
        Ok(Some(current)) => RepositoryError::Conflict {
            entity_id: id.to_string(),
            expected_revision,
            current_revision: current.revision,
        },
        Err(storage) => RepositoryError::Storage(storage),
    }
}

/// `create`'s error path has no expected revision to classify a conflict
/// against, so it only needs to single out `DuplicateHost` before falling
/// back to a plain storage error.
fn duplicate_host_or_storage(error: StorageError) -> RepositoryError {
    match error {
        StorageError::DuplicateHost(conflicting_printer_id) => RepositoryError::DuplicateHost {
            conflicting_printer_id,
        },
        other => RepositoryError::Storage(other),
    }
}

/// The Printer `id` with its live Material Slots, read inside the caller's
/// transaction, or `NotFound`. The inventory commands use it to return
/// every Printer whose occupancy their write changed.
pub fn load_in(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<StoredPrinter, RepositoryError> {
    let mut printer = load_for_write(transaction, id)?;
    printer.material_slots = slots::live_slots(transaction, id)?;
    Ok(printer)
}

/// The request fields that define an `archive_printer` operation, in a
/// fixed order for [`operations::digest`].
#[derive(serde::Serialize)]
#[serde(rename_all = "camelCase")]
struct ArchivePrinterRequest<'a> {
    printer_id: &'a str,
    expected_revision: i64,
    spool_dispositions: &'a [SpoolDispositionInput],
}

/// The Printer row `id` inside a `write_repo` transaction, or `NotFound`.
fn load_for_write(
    transaction: &rusqlite::Transaction<'_>,
    id: &str,
) -> Result<StoredPrinter, RepositoryError> {
    transaction
        .query_row(
            &format!("SELECT {PRINTER_COLUMNS} FROM printers WHERE id = ?1"),
            [id],
            decode,
        )
        .optional()?
        .ok_or_else(|| RepositoryError::NotFound {
            entity_id: id.to_string(),
        })
}

/// The blockers relevant to `action` alone — a caller attempting one action
/// (e.g. delete) shouldn't be told about blockers for a different one (e.g.
/// unarchive) that happens to also be blocked right now.
fn blockers_for(
    eligibility: super::lifecycle::LifecycleEligibility,
    action: LifecycleAction,
) -> Vec<super::lifecycle::LifecycleBlocker> {
    eligibility
        .blockers
        .into_iter()
        .filter(|blocker| blocker.action == action)
        .collect()
}

/// D3: no two non-archived Printers may share a host identity. Checked
/// inside the write transaction before every insert/replace; the partial
/// unique index created by migration 0003 is the backstop (see
/// `is_host_identity_violation`/`map_write_error`) for anything that skips
/// this precheck.
fn precheck_duplicate_host(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    if printer.archived_at.is_some() {
        return Ok(());
    }
    let Some(identity) = connection_host_identity(printer) else {
        return Ok(());
    };
    if let Some(conflicting_id) =
        active_printer_with_identity(transaction, &identity, Some(&printer.id))?
    {
        return Err(StorageError::DuplicateHost(conflicting_id));
    }
    Ok(())
}

fn connection_host_identity(printer: &StoredPrinter) -> Option<String> {
    printer
        .connection
        .as_ref()
        .and_then(|connection| canonical_host_identity(&connection.host, connection.port))
}

fn active_printer_with_identity(
    connection: &rusqlite::Connection,
    identity: &str,
    excluding: Option<&str>,
) -> rusqlite::Result<Option<String>> {
    connection
        .query_row(
            "SELECT id FROM printers
             WHERE host_identity = ?1 AND archived_at IS NULL AND (?2 IS NULL OR id != ?2)
             LIMIT 1",
            params![identity, excluding],
            |row| row.get::<_, String>(0),
        )
        .optional()
}

/// Whether `error` is the partial unique index (`printers_active_host_identity`,
/// migration 0003) rejecting a write — SQLite's own message for it names the
/// column, not the (partial) index, so matching on `host_identity` is what
/// distinguishes it from any other constraint failure (e.g. the `id` primary
/// key, or the `location`/`start_safety` CHECK constraints).
fn is_host_identity_violation(error: &rusqlite::Error) -> bool {
    matches!(
        error,
        rusqlite::Error::SqliteFailure(code, Some(message))
            if code.code == rusqlite::ErrorCode::ConstraintViolation
                && message.contains("host_identity")
    )
}

/// Backstop for `precheck_duplicate_host`: if a write still hits the
/// partial unique index (e.g. a caller that skips the precheck, such as
/// `replace_all`'s import path), look up who holds the identity in the same
/// transaction rather than surface a bare constraint error. Per ruling R1,
/// this reuses `StorageError::DuplicateHost` rather than adding a separate
/// constraint-error variant.
fn map_write_error(
    transaction: &rusqlite::Transaction<'_>,
    error: rusqlite::Error,
    identity: Option<&str>,
    excluding_id: &str,
) -> StorageError {
    if is_host_identity_violation(&error) {
        if let Some(identity) = identity {
            let conflicting_id =
                active_printer_with_identity(transaction, identity, Some(excluding_id))
                    .ok()
                    .flatten()
                    .unwrap_or_default();
            return StorageError::DuplicateHost(conflicting_id);
        }
    }
    StorageError::from(error)
}

fn cleanup_precedence(reason: &str) -> Option<u8> {
    match reason {
        "import_orphan" => Some(0),
        "reset" => Some(1),
        "provisional" => Some(2),
        "replaced" => Some(3),
        "cleared" => Some(4),
        "printer_deleted" => Some(5),
        _ => None,
    }
}

fn enqueue_credential_cleanup(
    transaction: &rusqlite::Transaction<'_>,
    reference: &str,
    printer_id: Option<&str>,
    reason: &str,
) -> Result<(), StorageError> {
    let incoming_precedence = cleanup_precedence(reason).ok_or(StorageError::OperationFailed)?;
    let current = transaction
        .query_row(
            "SELECT reason, printer_id FROM pending_credential_cleanup WHERE credential_ref=?1",
            [reference],
            |row| Ok((row.get::<_, String>(0)?, row.get::<_, Option<String>>(1)?)),
        )
        .optional()?;
    if let Some((current_reason, current_printer)) = current {
        let current_precedence =
            cleanup_precedence(&current_reason).ok_or(StorageError::OperationFailed)?;
        let incoming_wins = incoming_precedence > current_precedence
            || (incoming_precedence == current_precedence
                && match (printer_id, current_printer.as_deref()) {
                    (Some(_), None) => true,
                    (Some(incoming), Some(current)) => incoming.as_bytes() < current.as_bytes(),
                    (None, _) => false,
                });
        if incoming_wins {
            // P6 D7: `retry_pending_credential_cleanup` keeps a credential
            // while the Printer its row names has an unresolved Host
            // Operation, so never move the row off such a Printer (to
            // another Printer, or to none) until that row is terminal.
            let keep_printer = match current_printer.as_deref() {
                Some(current) => crate::host_ops::repository::has_unresolved(transaction, current)
                    .map_err(crate::host_ops::guards::storage_error)?,
                None => false,
            };
            let printer_id = if keep_printer {
                current_printer.as_deref()
            } else {
                printer_id
            };
            transaction.execute(
                "UPDATE pending_credential_cleanup SET reason=?2, printer_id=?3 WHERE credential_ref=?1",
                params![reference, reason, printer_id],
            )?;
        }
    } else {
        transaction.execute(
            "INSERT INTO pending_credential_cleanup(credential_ref, printer_id, reason, created_at) VALUES (?1, ?2, ?3, ?4)",
            params![reference, printer_id, reason, crate::printers::now_rfc3339()],
        )?;
    }
    Ok(())
}

fn validate_id(id: &str) -> Result<(), StorageError> {
    if id.is_empty() || id.len() > 512 || id.chars().any(char::is_control) {
        Err(StorageError::OperationFailed)
    } else {
        Ok(())
    }
}

/// Column order matches `PRINTER_COLUMNS`. `host_identity` (index 15) is
/// selected for consistency with `insert`/`replace` but never decoded onto
/// `StoredPrinter` — it's derived from `connection` on every write, never a
/// field callers set directly.
fn decode(row: &rusqlite::Row<'_>) -> rusqlite::Result<StoredPrinter> {
    let overrides: String = row.get(9)?;
    let last_known_good: Option<String> = row.get(10)?;
    let connection: Option<String> = row.get(11)?;
    let start_safety: String = row.get(13)?;
    Ok(StoredPrinter {
        id: row.get(0)?,
        revision: row.get(1)?,
        name: row.get(2)?,
        catalog_ref: super::CatalogRef {
            vendor: row.get(3)?,
            model: row.get(4)?,
            variant: row.get(5)?,
            model_id: row.get(6)?,
            printer_variant: row.get(7)?,
        },
        notes: row.get(8)?,
        overrides: serde_json::from_str(&overrides).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                9,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        last_known_good: last_known_good
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    10,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        connection: connection
            .map(|value| serde_json::from_str(&value))
            .transpose()
            .map_err(|error| {
                rusqlite::Error::FromSqlConversionFailure(
                    11,
                    rusqlite::types::Type::Text,
                    Box::new(error),
                )
            })?,
        location: row.get(12)?,
        start_safety: decode_start_safety(&start_safety).map_err(|error| {
            rusqlite::Error::FromSqlConversionFailure(
                13,
                rusqlite::types::Type::Text,
                Box::new(error),
            )
        })?,
        archived_at: row.get(14)?,
        // Never a `printers` column — every caller (`list`/`get`/every
        // write path) fills it separately, right before returning.
        material_slots: Vec::new(),
        created_at: row.get(16)?,
        updated_at: row.get(17)?,
    })
}

/// `start_safety` round-trips through `StoredPrinter`/`StartSafety`'s own
/// serde camelCase mapping rather than a hand-maintained string match, so
/// the DB text and the wire representation can never drift apart.
fn decode_start_safety(text: &str) -> Result<StartSafety, serde_json::Error> {
    serde_json::from_value(serde_json::Value::String(text.to_string()))
}

fn encode_start_safety(value: StartSafety) -> String {
    match serde_json::to_value(value).expect("StartSafety always serializes") {
        serde_json::Value::String(text) => text,
        other => unreachable!("StartSafety serializes to a string, got {other:?}"),
    }
}

struct PrinterColumnValues {
    overrides: String,
    last_known_good: Option<String>,
    connection: Option<String>,
    host_identity: Option<String>,
    start_safety: String,
}

fn values(printer: &StoredPrinter) -> Result<PrinterColumnValues, StorageError> {
    Ok(PrinterColumnValues {
        overrides: serde_json::to_string(&printer.overrides)
            .map_err(|_| StorageError::OperationFailed)?,
        last_known_good: printer
            .last_known_good
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::OperationFailed)?,
        connection: printer
            .connection
            .as_ref()
            .map(serde_json::to_string)
            .transpose()
            .map_err(|_| StorageError::OperationFailed)?,
        host_identity: connection_host_identity(printer),
        start_safety: encode_start_safety(printer.start_safety),
    })
}

fn insert(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    let columns = values(printer)?;
    let result = transaction.execute(
        "INSERT INTO printers(id, revision, name, catalog_vendor, catalog_model, catalog_variant, catalog_model_id, catalog_printer_variant, notes, overrides_json, last_known_good_json, connection_json, location, start_safety, archived_at, host_identity, created_at, updated_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18)",
        params![
            printer.id,
            printer.revision,
            printer.name,
            printer.catalog_ref.vendor,
            printer.catalog_ref.model,
            printer.catalog_ref.variant,
            printer.catalog_ref.model_id,
            printer.catalog_ref.printer_variant,
            printer.notes,
            columns.overrides,
            columns.last_known_good,
            columns.connection,
            printer.location,
            columns.start_safety,
            printer.archived_at,
            columns.host_identity,
            printer.created_at,
            printer.updated_at,
        ],
    );
    result.map(|_| ()).map_err(|error| {
        map_write_error(
            transaction,
            error,
            columns.host_identity.as_deref(),
            &printer.id,
        )
    })
}

fn replace(
    transaction: &rusqlite::Transaction<'_>,
    printer: &StoredPrinter,
) -> Result<(), StorageError> {
    let columns = values(printer)?;
    let result = transaction.execute(
        "UPDATE printers SET revision=?2, name=?3, catalog_vendor=?4, catalog_model=?5, catalog_variant=?6, catalog_model_id=?7, catalog_printer_variant=?8, notes=?9, overrides_json=?10, last_known_good_json=?11, connection_json=?12, location=?13, start_safety=?14, archived_at=?15, host_identity=?16, updated_at=?17 WHERE id=?1",
        params![
            printer.id,
            printer.revision,
            printer.name,
            printer.catalog_ref.vendor,
            printer.catalog_ref.model,
            printer.catalog_ref.variant,
            printer.catalog_ref.model_id,
            printer.catalog_ref.printer_variant,
            printer.notes,
            columns.overrides,
            columns.last_known_good,
            columns.connection,
            printer.location,
            columns.start_safety,
            printer.archived_at,
            columns.host_identity,
            printer.updated_at,
        ],
    );
    result.map(|_| ()).map_err(|error| {
        map_write_error(
            transaction,
            error,
            columns.host_identity.as_deref(),
            &printer.id,
        )
    })
}

#[cfg(test)]
mod cleanup_precedence_tests {
    use super::cleanup_precedence;

    /// P9 D15: `reset` ranks between `import_orphan` and `provisional`.
    #[test]
    fn precedence_orders_reset_between_import_orphan_and_provisional() {
        let order = [
            "import_orphan",
            "reset",
            "provisional",
            "replaced",
            "cleared",
            "printer_deleted",
        ];
        let ranks: Vec<u8> = order
            .iter()
            .map(|reason| cleanup_precedence(reason).expect("known reason"))
            .collect();
        assert!(ranks.windows(2).all(|pair| pair[0] < pair[1]), "{ranks:?}");
        assert_eq!(cleanup_precedence("bogus"), None);
    }
}
