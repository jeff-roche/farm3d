//! The Library's Tauri commands. Each validates the contract version and
//! waits for bootstrap before any side effect (opening a dialog, reading a
//! file). None accepts a filesystem path (D7): files are named by
//! `selectionId` and `fileIndex`.

use tauri::AppHandle;

use crate::bootstrap::BootstrapState;
use crate::contracts::command::{CommandError, CommandSuccess, IncomingContractVersion};
use crate::RuntimeServices;

use super::inspection::{self, ImportInspection};
use super::selection::{CancelImportSelectionData, ImportSelectionSummary, SelectionPurpose};

type Services<'a, R> = tauri::State<'a, BootstrapState<RuntimeServices<R>>>;

/// D13 step 1: opens the native picker and registers the chosen files.
/// A cancelled dialog (or an empty choice) returns `null` and registers
/// nothing.
#[tauri::command]
pub async fn pick_model_files<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    purpose: SelectionPurpose,
) -> Result<CommandSuccess<Option<ImportSelectionSummary>>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let file_io = std::sync::Arc::clone(&services.library.file_io);
    let picked = tauri::async_runtime::spawn_blocking(move || file_io.pick_files(purpose))
        .await
        .map_err(|_| CommandError::internal())??;
    let summary = picked
        .filter(|paths| !paths.is_empty())
        .map(|paths| services.library.selections.register(purpose, paths));
    Ok(CommandSuccess::new(summary))
}

/// D13 step 2: stages, hashes, and inspects every file of the selection,
/// emitting `library.import.progress`. A repeat call returns the stored
/// result. An unknown or expired selection is `SELECTION_EXPIRED`.
#[tauri::command]
pub async fn inspect_import_selection<R: tauri::Runtime>(
    app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    selection_id: String,
) -> Result<CommandSuccess<ImportInspection>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let entry = services.library.selections.get(&selection_id)?;
    inspection::inspect_selection(&app, &services.storage, &services.library, entry)
        .await
        .map(CommandSuccess::new)
}

/// D13: stops in-flight work on the selection at its next check, deletes
/// its staging, and discards it. Unknown selections are a no-op.
#[tauri::command]
pub fn cancel_import_selection<R: tauri::Runtime>(
    _app: AppHandle<R>,
    bootstrap: Services<'_, R>,
    contract_version: IncomingContractVersion,
    selection_id: String,
) -> Result<CommandSuccess<CancelImportSelectionData>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    services.library.selections.discard(&selection_id);
    Ok(CommandSuccess::new(CancelImportSelectionData {}))
}
