//! Injectable native-dialog and document-I/O boundary for editable imports and
//! exports. Tests provide deterministic selections and failures; production
//! owns the four native JSON dialogs here rather than accepting frontend paths.

use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};

use tauri_plugin_dialog::DialogExt;

use crate::contracts::command::CommandError;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DocumentKind {
    Settings,
    Printers,
}

pub trait DocumentIo: Send + Sync {
    fn open_json(&self, kind: DocumentKind) -> Result<Option<PathBuf>, CommandError>;
    fn save_json(&self, kind: DocumentKind) -> Result<Option<PathBuf>, CommandError>;
    fn read(&self, path: &Path) -> Result<Vec<u8>, CommandError>;
    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<(), CommandError>;

    /// Deterministic race hook used by command tests after snapshot acceptance
    /// and before the final preconditioned transaction. Production is a no-op.
    fn after_snapshot(&self, _kind: DocumentKind) -> Result<(), CommandError> {
        Ok(())
    }

    /// Deterministic test hook for failures after a Printers replacement has
    /// committed. Production is a no-op.
    fn after_printers_commit(&self) {}
}

pub struct NativeDocumentIo<R: tauri::Runtime> {
    app: tauri::AppHandle<R>,
}

impl<R: tauri::Runtime> NativeDocumentIo<R> {
    pub fn new(app: tauri::AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: tauri::Runtime> DocumentIo for NativeDocumentIo<R> {
    fn open_json(&self, _kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        self.app
            .dialog()
            .file()
            .add_filter("JSON", &["json"])
            .blocking_pick_file()
            .map(|selected| selected.into_path())
            .transpose()
            .map_err(|_| CommandError::validation("Select a local JSON document."))
    }

    fn save_json(&self, kind: DocumentKind) -> Result<Option<PathBuf>, CommandError> {
        let name = match kind {
            DocumentKind::Settings => "farm3d-settings.json",
            DocumentKind::Printers => "farm3d-printers.json",
        };
        self.app
            .dialog()
            .file()
            .add_filter("JSON", &["json"])
            .set_file_name(name)
            .blocking_save_file()
            .map(|selected| selected.into_path())
            .transpose()
            .map_err(|_| CommandError::validation("Select a local JSON destination."))
    }

    fn read(&self, path: &Path) -> Result<Vec<u8>, CommandError> {
        fs::read(path).map_err(|_| CommandError::persistence_unavailable())
    }

    fn atomic_write(&self, path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
        atomic_write(path, bytes)
    }
}

pub fn atomic_write(path: &Path, bytes: &[u8]) -> Result<(), CommandError> {
    let parent = path.parent().ok_or_else(CommandError::internal)?;
    let temporary = parent.join(format!(".farm3d-export-{}.tmp", uuid::Uuid::new_v4()));
    let result = (|| {
        let mut file = fs::OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temporary)
            .map_err(|_| CommandError::persistence_unavailable())?;
        file.write_all(bytes)
            .and_then(|()| file.sync_all())
            .map_err(|_| CommandError::persistence_unavailable())?;
        atomic_replace(&temporary, path).map_err(|_| CommandError::persistence_unavailable())?;
        #[cfg(unix)]
        fs::File::open(parent)
            .and_then(|directory| directory.sync_all())
            .map_err(|_| CommandError::persistence_unavailable())?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}

#[cfg(unix)]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    fs::rename(source, destination)
}

#[cfg(windows)]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::Storage::FileSystem::{
        MoveFileExW, ReplaceFileW, MOVEFILE_WRITE_THROUGH, REPLACEFILE_WRITE_THROUGH,
    };

    let destination_exists = destination.exists();
    let source = source
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let destination = destination
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<_>>();
    let success = unsafe {
        if destination_exists {
            ReplaceFileW(
                destination.as_ptr(),
                source.as_ptr(),
                std::ptr::null(),
                REPLACEFILE_WRITE_THROUGH,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        } else {
            MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_WRITE_THROUGH,
            )
        }
    };
    if success == 0 {
        Err(std::io::Error::last_os_error())
    } else {
        Ok(())
    }
}

#[cfg(not(any(unix, windows)))]
pub(crate) fn atomic_replace(source: &Path, destination: &Path) -> std::io::Result<()> {
    if destination.exists() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Unsupported,
            "atomic replacement is unavailable",
        ));
    }
    fs::rename(source, destination)
}
