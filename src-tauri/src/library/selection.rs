//! D7: how files enter the Library. There is no command that takes a path:
//! files arrive only through the Rust-owned native picker
//! ([`ModelFileIo`]) or a Rust-observed window drop ([`handle_drop`]). Both
//! register the chosen paths in the [`SelectionRegistry`] and hand the
//! frontend an [`ImportSelectionSummary`], which names files by index and
//! basename only. Later commands take `selectionId` plus `fileIndex`.

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;
use ts_rs::TS;

use crate::contracts::command::CommandError;

use super::content::{CancelFlag, ContentStore};
use super::events;
use super::import::ImportLedger;
use super::inspection::InspectedItem;
use super::LibraryServices;

/// D7: a selection lives this long after its last use.
pub const SELECTION_TTL: Duration = Duration::from_secs(30 * 60);

/// D7: the most files one selection (a pick or a drop) holds. Extra paths
/// are ignored.
pub const MAX_SELECTION_FILES: usize = 100;

/// D7's picker filter for imports.
const MODEL_FILE_EXTENSIONS: [&str; 5] = ["stl", "3mf", "gcode", "gco", "g"];

/// What a selection is for: a new import, or Locate for a missing linked
/// source (D16).
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/SelectionPurpose.ts")]
pub enum SelectionPurpose {
    Import,
    Locate,
}

/// The native file dialog boundary. Production is [`NativeModelFileIo`];
/// tests use deterministic fakes.
pub trait ModelFileIo: Send + Sync {
    /// `Ok(None)` when the user cancels the dialog.
    fn pick_files(&self, purpose: SelectionPurpose) -> Result<Option<Vec<PathBuf>>, CommandError>;
}

/// A picker that is always cancelled: the default for services built
/// without a window (`RuntimeServices::for_test`).
pub struct CancelledModelFileIo;

impl ModelFileIo for CancelledModelFileIo {
    fn pick_files(&self, _purpose: SelectionPurpose) -> Result<Option<Vec<PathBuf>>, CommandError> {
        Ok(None)
    }
}

/// D7: `tauri-plugin-dialog`'s native open dialog. Import picks several
/// files; Locate picks one. Both filter to the formats farm3d reads.
pub struct NativeModelFileIo<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> NativeModelFileIo<R> {
    pub fn new(app: AppHandle<R>) -> Self {
        Self { app }
    }
}

impl<R: Runtime> ModelFileIo for NativeModelFileIo<R> {
    fn pick_files(&self, purpose: SelectionPurpose) -> Result<Option<Vec<PathBuf>>, CommandError> {
        let dialog = self
            .app
            .dialog()
            .file()
            .add_filter("3D models and G-code", &MODEL_FILE_EXTENSIONS);
        let picked = match purpose {
            SelectionPurpose::Import => dialog.blocking_pick_files(),
            SelectionPurpose::Locate => dialog.blocking_pick_file().map(|file| vec![file]),
        };
        picked
            .map(|files| {
                files
                    .into_iter()
                    .map(|file| file.into_path())
                    .collect::<Result<Vec<_>, _>>()
            })
            .transpose()
            .map_err(|_| CommandError::validation("Select files on this computer."))
    }
}

/// One file of a selection as the frontend sees it. `sizeBytes` is `null`
/// when the path isn't a readable regular file at registration.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportSelectionFile.ts")]
pub struct ImportSelectionFile {
    pub file_index: u32,
    pub file_name: String,
    #[ts(type = "number | null")]
    pub size_bytes: Option<u64>,
}

/// D13 step 1: what `pick_model_files` returns and
/// `library.selection.dropped` carries.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "command/ImportSelectionSummary.ts"
)]
pub struct ImportSelectionSummary {
    pub selection_id: String,
    pub purpose: SelectionPurpose,
    pub files: Vec<ImportSelectionFile>,
}

/// The `library.import.progress` payload for one file.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportProgress.ts")]
pub struct ImportProgress {
    pub file_index: u32,
    #[ts(type = "number")]
    pub bytes_done: u64,
    #[ts(type = "number")]
    pub bytes_total: u64,
}

/// `cancel_import_selection`'s (empty) result.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, TS)]
#[ts(export_to = "command/CancelImportSelectionData.ts")]
pub struct CancelImportSelectionData {}

/// One selected file. `path` is the absolute, lexically normalised path the
/// user chose (D6: symlinks are not resolved), or the path as given when it
/// can't be normalised; the path policy rejects that at inspection.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SelectedFile {
    pub path: PathBuf,
    pub file_name: String,
    pub size_bytes: Option<u64>,
}

/// A registered selection. `inspection` holds D13's stored inspection
/// result once `inspect_import_selection` has run; its lock is held for the
/// whole inspection, so a concurrent second call waits for the first.
/// `imports` records each committed item's outcome per `operationId` (D13).
pub struct SelectionEntry {
    pub id: String,
    pub purpose: SelectionPurpose,
    pub files: Vec<SelectedFile>,
    pub inspection: tokio::sync::Mutex<Option<Arc<Vec<InspectedItem>>>>,
    pub imports: ImportLedger,
    cancel: tokio::sync::watch::Sender<bool>,
    last_used: Mutex<Instant>,
}

impl SelectionEntry {
    /// The signal `cancel_import_selection` raises for in-flight work.
    pub fn cancel_flag(&self) -> CancelFlag {
        CancelFlag::new(self.cancel.subscribe())
    }

    pub fn is_cancelled(&self) -> bool {
        *self.cancel.borrow()
    }

    pub fn summary(&self) -> ImportSelectionSummary {
        ImportSelectionSummary {
            selection_id: self.id.clone(),
            purpose: self.purpose,
            files: self
                .files
                .iter()
                .enumerate()
                .map(|(index, file)| ImportSelectionFile {
                    file_index: index as u32,
                    file_name: file.file_name.clone(),
                    size_bytes: file.size_bytes,
                })
                .collect(),
        }
    }

    fn touch(&self, now: Instant) {
        *lock(&self.last_used) = now;
    }

    fn expired_at(&self, now: Instant) -> bool {
        now.saturating_duration_since(*lock(&self.last_used)) >= SELECTION_TTL
    }
}

/// D7: the in-memory selection registry. Selections are discarded on
/// cancel, after a complete import, on expiry, and (being in memory) at
/// restart. Discarding one also deletes its staging directory.
pub struct SelectionRegistry {
    entries: Mutex<HashMap<String, Arc<SelectionEntry>>>,
    content: Arc<ContentStore>,
}

impl SelectionRegistry {
    pub fn new(content: Arc<ContentStore>) -> Self {
        Self {
            entries: Mutex::new(HashMap::new()),
            content,
        }
    }

    /// Registers up to [`MAX_SELECTION_FILES`] of `paths` as a new
    /// selection. The native picker and the drop handler both call this;
    /// no command passes frontend input to it (clarification 2).
    pub fn register(
        &self,
        purpose: SelectionPurpose,
        paths: Vec<PathBuf>,
    ) -> ImportSelectionSummary {
        let now = Instant::now();
        self.sweep_expired(now);
        let files = paths
            .into_iter()
            .take(MAX_SELECTION_FILES)
            .map(selected_file)
            .collect();
        let entry = Arc::new(SelectionEntry {
            id: super::new_id("sel"),
            purpose,
            files,
            inspection: tokio::sync::Mutex::new(None),
            imports: ImportLedger::default(),
            cancel: tokio::sync::watch::channel(false).0,
            last_used: Mutex::new(now),
        });
        let summary = entry.summary();
        lock(&self.entries).insert(entry.id.clone(), entry);
        summary
    }

    /// The live selection `id`, marking it used. An unknown or expired id
    /// is `SELECTION_EXPIRED`.
    pub fn get(&self, id: &str) -> Result<Arc<SelectionEntry>, CommandError> {
        let now = Instant::now();
        let entry = lock(&self.entries).get(id).cloned();
        match entry {
            Some(entry) if !entry.expired_at(now) => {
                entry.touch(now);
                Ok(entry)
            }
            Some(_) => {
                self.discard(id);
                Err(CommandError::selection_expired())
            }
            None => Err(CommandError::selection_expired()),
        }
    }

    /// Cancels any in-flight work on `id`, forgets it, and deletes its
    /// staging. Unknown ids are a no-op.
    pub fn discard(&self, id: &str) {
        let removed = lock(&self.entries).remove(id);
        if let Some(entry) = removed {
            entry.cancel.send_replace(true);
            self.content.discard_staging(&entry.id);
        }
    }

    /// Discards every selection unused for [`SELECTION_TTL`] as of `now`.
    pub fn sweep_expired(&self, now: Instant) {
        let expired: Vec<String> = lock(&self.entries)
            .values()
            .filter(|entry| entry.expired_at(now))
            .map(|entry| entry.id.clone())
            .collect();
        for id in expired {
            self.discard(&id);
        }
    }

    pub fn len(&self) -> usize {
        lock(&self.entries).len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// D7: a window drop. Registers the dropped paths as an import selection
/// and emits `library.selection.dropped`. `run()`'s `on_webview_event`
/// calls this once bootstrap is ready.
pub fn handle_drop<R: Runtime>(
    app: &AppHandle<R>,
    library: &LibraryServices<R>,
    paths: Vec<PathBuf>,
) -> ImportSelectionSummary {
    let summary = library.selections.register(SelectionPurpose::Import, paths);
    events::publish_selection_dropped(app, &library.stream, &summary);
    summary
}

fn selected_file(path: PathBuf) -> SelectedFile {
    let path = crate::persistence::normalize_absolute(&path).unwrap_or(path);
    let file_name = basename(&path);
    let size_bytes = fs::metadata(&path)
        .ok()
        .filter(fs::Metadata::is_file)
        .map(|metadata| metadata.len());
    SelectedFile {
        path,
        file_name,
        size_bytes,
    }
}

/// The last component of `path`, for display. Never the full path.
fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "(unnamed)".to_string())
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // Neither map nor timestamp holds an invariant a panicking holder
    // could break.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> (tempfile::TempDir, SelectionRegistry) {
        let temp = tempfile::tempdir().unwrap();
        let root = temp.path().canonicalize().unwrap();
        let content = Arc::new(ContentStore::open(&root).unwrap());
        (temp, SelectionRegistry::new(content))
    }

    #[test]
    fn registration_normalises_paths_lexically_and_names_files_by_basename() {
        let (temp, registry) = registry();
        let file = temp.path().join("cube.stl");
        fs::write(&file, b"abc").unwrap();
        let dotted = temp
            .path()
            .join("sub")
            .join("..")
            .join(".")
            .join("cube.stl");

        let summary = registry.register(SelectionPurpose::Import, vec![dotted]);
        let entry = registry.get(&summary.selection_id).unwrap();
        assert_eq!(entry.files[0].path, file);
        assert_eq!(summary.files[0].file_name, "cube.stl");
        assert_eq!(summary.files[0].size_bytes, Some(3));
        assert!(summary.selection_id.starts_with("sel-"));
    }

    #[test]
    fn using_a_selection_restarts_its_expiry_clock() {
        let (_temp, registry) = registry();
        let summary = registry.register(SelectionPurpose::Locate, Vec::new());
        let entry = registry.get(&summary.selection_id).unwrap();
        entry.touch(Instant::now() + Duration::from_secs(20 * 60));

        registry.sweep_expired(Instant::now() + Duration::from_secs(31 * 60));
        assert!(registry.get(&summary.selection_id).is_ok());
    }

    #[test]
    fn discarding_cancels_in_flight_work() {
        let (_temp, registry) = registry();
        let summary = registry.register(SelectionPurpose::Import, Vec::new());
        let entry = registry.get(&summary.selection_id).unwrap();
        let flag = entry.cancel_flag();
        assert!(!flag.is_cancelled());

        registry.discard(&summary.selection_id);
        assert!(flag.is_cancelled());
        assert!(registry.is_empty());
        registry.discard(&summary.selection_id);
    }
}
