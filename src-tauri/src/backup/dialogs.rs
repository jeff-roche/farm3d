//! D18 "Dialogs": the native file dialogs P9 owns, injected so tests never
//! open one. The frontend never passes a path, and a result never returns
//! a full path (a basename at most). Closing a dialog is `Ok(None)`.

use std::path::PathBuf;

use tauri_plugin_dialog::DialogExt;

use crate::contracts::command::CommandError;

/// The backup file extension, without the dot.
pub const BACKUP_EXTENSION: &str = "farm3d-backup";

pub trait PortabilityDialogs: Send + Sync {
    /// Open a `.farm3d-backup` (restore preview).
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError>;
    /// Save a `.farm3d-backup`, suggesting `suggested_name`.
    fn save_backup(&self, suggested_name: &str) -> Result<Option<PathBuf>, CommandError>;
    /// Save a diagnostics `.zip`, suggesting `suggested_name`.
    fn save_diagnostics(&self, suggested_name: &str) -> Result<Option<PathBuf>, CommandError>;
}

/// Every dialog closes at once (`RuntimeServices::for_test`'s default).
pub struct CancelledPortabilityDialogs;

impl PortabilityDialogs for CancelledPortabilityDialogs {
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_backup(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
    fn save_diagnostics(&self, _suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        Ok(None)
    }
}

pub struct NativePortabilityDialogs<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> NativePortabilityDialogs<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

fn into_path(
    selected: Option<tauri_plugin_dialog::FilePath>,
    message: &str,
) -> Result<Option<PathBuf>, CommandError> {
    selected
        .map(|selected| selected.into_path())
        .transpose()
        .map_err(|_| CommandError::validation(message))
}

impl<R: tauri::Runtime> PortabilityDialogs for NativePortabilityDialogs<R> {
    fn open_backup(&self) -> Result<Option<PathBuf>, CommandError> {
        into_path(
            self.app
                .dialog()
                .file()
                .add_filter("farm3d backup", &[BACKUP_EXTENSION])
                .blocking_pick_file(),
            "Select a local farm3d backup.",
        )
    }

    fn save_backup(&self, suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        into_path(
            self.app
                .dialog()
                .file()
                .add_filter("farm3d backup", &[BACKUP_EXTENSION])
                .set_file_name(suggested_name)
                .blocking_save_file(),
            "Select a local destination for the backup.",
        )
    }

    fn save_diagnostics(&self, suggested_name: &str) -> Result<Option<PathBuf>, CommandError> {
        into_path(
            self.app
                .dialog()
                .file()
                .add_filter("Zip archive", &["zip"])
                .set_file_name(suggested_name)
                .blocking_save_file(),
            "Select a local destination for the diagnostics bundle.",
        )
    }
}
