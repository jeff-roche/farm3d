//! D13: the inventory commands. Each one validates the contract version,
//! waits for bootstrap, runs ONE `Storage::write_repo` (or a read), and
//! only after that returns `Ok` publishes the inventory events and the
//! `InventoryChange` broadcast (D11). Nothing is emitted inside a
//! transaction, for a failed write, or for an idempotent replay.
//!
//! The records a command returns are read inside its own transaction, so
//! the result and the events carry exactly the committed state.

use rusqlite::Transaction;
use serde::Serialize;
use tauri::AppHandle;
use ts_rs::TS;

use crate::bootstrap::BootstrapState;
use crate::catalog::resolve::{resolve_printer, ResolvedPrinter};
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::contracts::event::JsSafeInteger;
use crate::persistence::{RepositoryError, StorageError};
use crate::printers::commands::OperationWarning;
use crate::printers::StoredPrinter;
use crate::RuntimeServices;

use super::events::{self, PrinterSlots};
use super::ledger::{self, AmountEntry, AmountEvent, AmountEventKind};
use super::lifecycle::{self, SpoolLifecycleAction};
use super::movement::{self, MoveDestination, SpoolMovement};
use super::reservations::{self, Reservation};
use super::tares::{self, Tare};
use super::{repository, AmountConfidence, SpoolFields, SpoolRecord};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// `create_spool`/`update_spool`/`record_spool_amount`/`set_spool_lifecycle`.
/// `printers` holds every Printer whose occupancy the command changed.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/SpoolMutationResult.ts")]
pub struct SpoolMutationResult {
    pub spool: SpoolRecord,
    pub printers: Vec<ResolvedPrinter>,
    pub warnings: Vec<OperationWarning>,
}

/// D6: the moved Spool, then the Spool it displaced (when there was one);
/// each Printer whose slots changed; and the movement rows in write order.
/// Named `MoveSpoolData` in TypeScript, because `MoveSpoolResult` there is
/// the command's `CommandSuccess` wrapper (as with `DeletePrinterData`).
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename = "MoveSpoolData",
    rename_all = "camelCase",
    export_to = "command/MoveSpoolData.ts"
)]
pub struct MoveSpoolResult {
    pub spools: Vec<SpoolRecord>,
    pub printers: Vec<ResolvedPrinter>,
    pub movements: Vec<SpoolMovement>,
}

/// `create_tare`/`update_tare`/`delete_tare`. For a delete, `tare` is the
/// tare as it was just before deletion.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/TareMutationResult.ts")]
pub struct TareMutationResult {
    pub tare: Tare,
}

/// `spool_history`: every movement, ledger row, and reservation of one
/// Spool, oldest first.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/SpoolHistory.ts")]
pub struct SpoolHistory {
    pub movements: Vec<SpoolMovement>,
    pub amount_events: Vec<AmountEvent>,
    pub reservations: Vec<Reservation>,
}

/// D11's backfill: the inventory stream's identity, the sequence the
/// snapshot covers, and every Spool and tare.
#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/InventorySnapshot.ts")]
pub struct InventorySnapshot {
    pub stream_id: String,
    pub snapshot_sequence: JsSafeInteger,
    pub spools: Vec<SpoolRecord>,
    pub tares: Vec<Tare>,
}

/// The committed records one write touched, read in its transaction.
struct Committed {
    spools: Vec<SpoolRecord>,
    printers: Vec<StoredPrinter>,
}

impl Committed {
    fn read(
        tx: &Transaction<'_>,
        spool_ids: &[String],
        printer_ids: &[String],
    ) -> Result<Self, RepositoryError> {
        let spools = spool_ids
            .iter()
            .map(|id| {
                repository::load_record(tx, id)?.ok_or_else(|| RepositoryError::NotFound {
                    entity_id: id.clone(),
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let printers = printer_ids
            .iter()
            .map(|id| crate::printers::repository::load_in(tx, id))
            .collect::<Result<Vec<_>, _>>()?;
        Ok(Self { spools, printers })
    }

    /// Publishes this write's events and broadcast. Call after commit only.
    fn publish<R: tauri::Runtime>(&self, app: &AppHandle<R>, services: &RuntimeServices<R>) {
        let printers: Vec<PrinterSlots> = self.printers.iter().map(PrinterSlots::from).collect();
        events::publish(app, services, &self.spools, &printers);
    }

    fn resolved_printers<R: tauri::Runtime>(
        &self,
        services: &RuntimeServices<R>,
    ) -> Vec<ResolvedPrinter> {
        self.printers
            .iter()
            .map(|printer| resolve_printer(&services.catalog, printer))
            .collect()
    }

    /// The first Spool, which each single-Spool command reads first.
    fn into_mutation<R: tauri::Runtime>(
        mut self,
        services: &RuntimeServices<R>,
    ) -> SpoolMutationResult {
        let printers = self.resolved_printers(services);
        SpoolMutationResult {
            spool: self.spools.swap_remove(0),
            printers,
            warnings: Vec::new(),
        }
    }
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// Reads through a read-only transaction; `operation` returns the
/// repository modules' `StorageError`, which `read_transaction` doesn't.
fn read<R: tauri::Runtime, T>(
    services: &RuntimeServices<R>,
    operation: impl FnOnce(&Transaction<'_>) -> Result<T, RepositoryError>,
) -> Result<T, CommandError> {
    services
        .storage
        .read_transaction(|tx| Ok(operation(tx)))
        .map_err(storage_error)?
        .map_err(CommandError::from_repository)
}

/// A single-Spool write: runs `operation` (which returns the id of every
/// Spool and Printer it touched, the subject Spool first), reads their
/// committed records, then publishes after commit.
fn mutate_spool<R: tauri::Runtime>(
    app: &AppHandle<R>,
    services: &RuntimeServices<R>,
    operation: impl FnOnce(&Transaction<'_>) -> Result<(Vec<String>, Vec<String>), RepositoryError>,
) -> Result<CommandSuccess<SpoolMutationResult>, CommandError> {
    let committed = services
        .storage
        .write_repo(|tx| {
            let (spool_ids, printer_ids) = operation(tx)?;
            Committed::read(tx, &spool_ids, &printer_ids)
        })
        .map_err(CommandError::from_repository)?;
    committed.publish(app, services);
    Ok(CommandSuccess::new(committed.into_mutation(services)))
}

/// D11: the backfill. The sequence is read BEFORE the Spools, so a change
/// this snapshot misses always has a larger sequence than
/// `snapshotSequence`.
#[tauri::command]
pub fn list_spools<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<InventorySnapshot>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let snapshot_sequence = services.inventory_stream.snapshot_sequence();
    let (spools, tares) = read(&services, |tx| {
        Ok((repository::list_spools(tx)?, tares::list(tx)?))
    })?;
    Ok(CommandSuccess::new(InventorySnapshot {
        stream_id: services.inventory_stream.stream_id().to_string(),
        snapshot_sequence,
        spools,
        tares,
    }))
}

#[tauri::command]
pub fn spool_history<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    spool_id: String,
) -> Result<CommandSuccess<SpoolHistory>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let history = read(&services, |tx| {
        if repository::load_spool(tx, &spool_id)?.is_none() {
            return Err(RepositoryError::NotFound {
                entity_id: spool_id.clone(),
            });
        }
        Ok(SpoolHistory {
            movements: movement::history(tx, &spool_id)?,
            amount_events: ledger::history(tx, &spool_id)?,
            reservations: reservations::history(tx, &spool_id)?,
        })
    })?;
    Ok(CommandSuccess::new(history))
}

/// D1-D3/D7: a new active Spool in storage, with its `initial` ledger row.
#[tauri::command]
pub fn create_spool<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    fields: SpoolFields,
    initial_amount: AmountEntry,
    storage_label: Option<String>,
) -> Result<CommandSuccess<SpoolMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    mutate_spool(&app, &services, |tx| {
        let spool =
            repository::insert_spool(tx, &fields, &initial_amount, storage_label.as_deref())?;
        Ok((vec![spool.id], Vec::new()))
    })
}

/// D2: replaces every user-editable field.
#[tauri::command]
pub fn update_spool<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
    patch: SpoolFields,
) -> Result<CommandSuccess<SpoolMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    mutate_spool(&app, &services, |tx| {
        repository::update_spool_fields(tx, &id, expected_revision, &patch)?;
        Ok((vec![id.clone()], Vec::new()))
    })
}

/// D3/D7: appends a `measurement` (a scale entry or a measured net) or an
/// `estimate` (an estimated net) ledger row. The entry is validated before
/// the revision check, matching `update_spool`.
#[tauri::command]
pub fn record_spool_amount<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
    entry: AmountEntry,
    note: Option<String>,
) -> Result<CommandSuccess<SpoolMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    mutate_spool(&app, &services, |tx| {
        let (after_mg, confidence, mut snapshot) = ledger::resolve_entry(tx, &entry)?;
        repository::check_and_bump_revision(tx, &id, expected_revision)?;
        snapshot.note = note
            .as_deref()
            .map(str::trim)
            .filter(|text| !text.is_empty())
            .map(str::to_string);
        let kind = match confidence {
            AmountConfidence::Measured => AmountEventKind::Measurement,
            AmountConfidence::Estimated => AmountEventKind::Estimate,
        };
        ledger::append(tx, &id, kind, after_mg, confidence, snapshot)?;
        Ok((vec![id.clone()], Vec::new()))
    })
}

/// D6: load, unload, swap, relocate, or move between Printers. Idempotent
/// by `operationId` through the operations ledger: a retry of the same
/// request returns the current state of the recorded Spools and Printers
/// and emits nothing. The same `operationId` for a different request is
/// `VALIDATION` on `operationId`.
#[tauri::command]
pub fn move_spool<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    spool_id: String,
    expected_spool_revision: i64,
    destination: MoveDestination,
) -> Result<CommandSuccess<MoveSpoolResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let (committed, outcome) = services
        .storage
        .write_repo(|tx| {
            let outcome = movement::move_spool(
                tx,
                &operation_id,
                &spool_id,
                expected_spool_revision,
                &destination,
            )?;
            let committed = Committed::read(tx, &outcome.spool_ids, &outcome.printer_ids)?;
            Ok((committed, outcome))
        })
        .map_err(CommandError::from_repository)?;
    if !outcome.replayed {
        committed.publish(&app, &services);
    }
    Ok(CommandSuccess::new(MoveSpoolResult {
        printers: committed.resolved_printers(&services),
        spools: committed.spools,
        movements: outcome.movements,
    }))
}

/// D9: `markEmpty`, `reactivate`, `archive`, or `unarchive`. A `markEmpty`
/// of a loaded Spool also unloads it to `storageLabel`, so `printers` then
/// holds the Printer it left. Idempotent by `operationId`, like
/// `move_spool`: a retry of the same request returns the Spool's current
/// state and emits nothing.
#[tauri::command]
pub fn set_spool_lifecycle<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    operation_id: String,
    id: String,
    expected_revision: i64,
    action: SpoolLifecycleAction,
    storage_label: Option<String>,
) -> Result<CommandSuccess<SpoolMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let (committed, replayed) = services
        .storage
        .write_repo(|tx| {
            let outcome = lifecycle::apply_lifecycle(
                tx,
                &id,
                expected_revision,
                action,
                storage_label.as_deref(),
                &operation_id,
            )?;
            // `outcome.spool_ids` is just this Spool: a lifecycle unload
            // never displaces another.
            let committed = Committed::read(tx, std::slice::from_ref(&id), &outcome.printer_ids)?;
            Ok((committed, outcome.replayed))
        })
        .map_err(CommandError::from_repository)?;
    if !replayed {
        committed.publish(&app, &services);
    }
    Ok(CommandSuccess::new(committed.into_mutation(&services)))
}

#[tauri::command]
pub fn create_tare<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    name: String,
    weight_mg: i64,
) -> Result<CommandSuccess<TareMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let tare = services
        .storage
        .write_repo(|tx| tares::create(tx, &name, weight_mg))
        .map_err(CommandError::from_repository)?;
    Ok(CommandSuccess::new(TareMutationResult { tare }))
}

#[tauri::command]
pub fn update_tare<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
    name: String,
    weight_mg: i64,
) -> Result<CommandSuccess<TareMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let tare = services
        .storage
        .write_repo(|tx| tares::update(tx, &id, expected_revision, &name, weight_mg))
        .map_err(CommandError::from_repository)?;
    Ok(CommandSuccess::new(TareMutationResult { tare }))
}

/// D3: deletes a tare. Every Spool whose default tare it was loses its
/// `tareId` and gets a `spool.changed` event.
#[tauri::command]
pub fn delete_tare<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    id: String,
    expected_revision: i64,
) -> Result<CommandSuccess<TareMutationResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let (tare, committed) = services
        .storage
        .write_repo(|tx| {
            let (tare, cleared_spool_ids) = tares::delete(tx, &id, expected_revision)?;
            Ok((tare, Committed::read(tx, &cleared_spool_ids, &[])?))
        })
        .map_err(CommandError::from_repository)?;
    committed.publish(&app, &services);
    Ok(CommandSuccess::new(TareMutationResult { tare }))
}

/// D8's demo aid: seeds one reservation so the `reserved` facet can be
/// checked by hand. Debug builds only, and deliberately outside
/// `COMMAND_NAMES`/`COMMAND_CONTRACTS`: P3 exposes no reservation command.
#[cfg(debug_assertions)]
#[tauri::command]
pub fn debug_seed_reservation<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    spool_id: String,
    amount_mg: i64,
) -> Result<CommandSuccess<SpoolMutationResult>, CommandError> {
    use reservations::{ReservationError, ReservationHolder};

    contract_version.validate()?;
    let services = bootstrap.ready()?;
    if amount_mg <= 0 {
        return Err(CommandError::validation_at(
            "amountMg",
            "The submitted value is invalid.",
        ));
    }
    let holder = ReservationHolder {
        kind: "debug".to_string(),
        id: "fixture".to_string(),
    };
    let operation_id = format!("op-{}", uuid::Uuid::new_v4());
    mutate_spool(&app, &services, |tx| {
        reservations::reserve(tx, &spool_id, &holder, amount_mg, &operation_id).map_err(
            |error| match error {
                ReservationError::InsufficientAvailable { .. }
                | ReservationError::InvalidAmount => RepositoryError::Validation {
                    field_path: "amountMg",
                },
                ReservationError::SpoolNotReservable { .. } => RepositoryError::Validation {
                    field_path: "spoolId",
                },
                ReservationError::NotFound => RepositoryError::NotFound {
                    entity_id: spool_id.clone(),
                },
                ReservationError::InvalidTransition { .. } => {
                    RepositoryError::Storage(StorageError::OperationFailed)
                }
                ReservationError::Storage(error) => RepositoryError::Storage(error),
            },
        )?;
        Ok((vec![spool_id.clone()], Vec::new()))
    })
}
