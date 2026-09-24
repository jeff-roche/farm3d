//! D13 step 2: staging and inspecting a selection's files.
//!
//! Each file passes the path policy (D6), is copied into
//! `staging/<selectionId>/<fileIndex>.part` with its hash (D4), and is
//! detected and inspected from that staged copy, never the live file
//! (D8-D12). An embedded thumbnail is staged beside it as
//! `<fileIndex>.thumb.png` (never `.part`, P9). At most two files are in
//! flight, each on a blocking thread. The result is stored on the
//! [`SelectionEntry`], so asking again returns it without re-reading.

use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures_util::stream::{self, StreamExt};
use rusqlite::Connection;
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Runtime};
use ts_rs::TS;

use crate::contracts::command::CommandError;
use crate::persistence::{normalize_absolute, Storage, StorageError, StoragePaths};

use super::content::{CancelFlag, ContentError, ContentStore, StagedFile};
use super::events::{self, LibraryStream};
use super::formats::{self, InspectError, InspectOutcome, Inspection, InspectionSummary};
use super::formats::{ThumbnailBytes, UnsupportedEntry};
use super::selection::{ImportProgress, SelectedFile, SelectionEntry};
use super::{repository, ImportWarning, LibraryServices, ModelFormat};

/// D13: files staged and inspected at once, per selection.
const FILES_IN_FLIGHT: usize = 2;

/// D13: at most one `library.import.progress` per file this often, apart
/// from the first and last (S5a).
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);

/// Spec §Commands: why one import item was rejected.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "command/ImportItemErrorCode.ts"
)]
pub enum ImportItemErrorCode {
    Validation,
    NotFound,
    Conflict,
    UnsupportedFormat,
    InvalidContent,
    TooLarge,
    NotAFile,
    PathNotAllowed,
    SourceUnreadable,
    SourceChangedDuringRead,
    DuplicateDecisionRequired,
    UnsupportedNotAcknowledged,
    PersistenceUnavailable,
    Cancelled,
}

/// D14: an existing Model holding the same bytes. `revisionId` and
/// `sequence` name that Model's latest revision with this hash, and
/// `isCurrent` says whether it is the Model's current revision.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/DuplicateMatch.ts")]
pub struct DuplicateMatch {
    pub model_id: String,
    pub model_name: String,
    pub project_ids: Vec<String>,
    pub revision_id: String,
    #[ts(type = "number")]
    pub sequence: i64,
    pub is_current: bool,
}

/// D13: one file's inspection result as the frontend sees it.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "command/ImportCandidate.ts"
)]
pub enum ImportCandidate {
    Ready {
        file_index: u32,
        file_name: String,
        format: ModelFormat,
        #[ts(type = "number")]
        size_bytes: u64,
        sha256: String,
        summary: InspectionSummary,
        /// D10: 3MF content farm3d keeps but won't use. Empty otherwise.
        unsupported: Vec<UnsupportedEntry>,
        warnings: Vec<ImportWarning>,
        duplicates: Vec<DuplicateMatch>,
    },
    Rejected {
        file_index: u32,
        file_name: String,
        code: ImportItemErrorCode,
        message: String,
    },
}

/// `inspect_import_selection`'s result.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/ImportInspection.ts")]
pub struct ImportInspection {
    pub selection_id: String,
    pub items: Vec<ImportCandidate>,
}

/// The staged embedded thumbnail of a ready file. Its bytes live only in
/// the staged file; `origin_part` is the 3MF part or G-code line it came
/// from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct StagedThumbnail {
    pub staged: StagedFile,
    pub origin_part: String,
    pub width: u32,
    pub height: u32,
}

/// A staged, inspected file, ready for `import_models` (Task 6).
/// `outcome.thumbnail` is always `None` here: the thumbnail was moved to
/// `thumbnail` when it was staged.
#[derive(Clone, Debug, PartialEq)]
pub struct ReadyItem {
    pub staged: StagedFile,
    pub format: ModelFormat,
    pub outcome: InspectOutcome,
    pub thumbnail: Option<StagedThumbnail>,
    pub duplicates: Vec<DuplicateMatch>,
}

/// One file's stored inspection result.
#[derive(Clone, Debug, PartialEq)]
pub enum InspectedItem {
    Ready(Box<ReadyItem>),
    Rejected {
        code: ImportItemErrorCode,
        message: String,
    },
}

impl InspectedItem {
    fn rejected(code: ImportItemErrorCode, message: impl Into<String>) -> Self {
        Self::Rejected {
            code,
            message: message.into(),
        }
    }

    fn cancelled() -> Self {
        Self::rejected(ImportItemErrorCode::Cancelled, "The import was cancelled.")
    }

    fn candidate(&self, file_index: u32, file: &SelectedFile) -> ImportCandidate {
        let file_name = file.file_name.clone();
        match self {
            Self::Ready(item) => ImportCandidate::Ready {
                file_index,
                file_name,
                format: item.format,
                size_bytes: item.staged.size,
                sha256: item.staged.sha256.clone(),
                summary: item.outcome.summary.clone(),
                unsupported: match &item.outcome.inspection {
                    Inspection::ThreeMf(model) => model.unsupported.clone(),
                    Inspection::Stl(_) | Inspection::Gcode(_) => Vec::new(),
                },
                warnings: item.outcome.warnings.clone(),
                duplicates: item.duplicates.clone(),
            },
            Self::Rejected { code, message } => ImportCandidate::Rejected {
                file_index,
                file_name,
                code: *code,
                message: message.clone(),
            },
        }
    }
}

fn wire(entry: &SelectionEntry, items: &[InspectedItem]) -> ImportInspection {
    ImportInspection {
        selection_id: entry.id.clone(),
        items: items
            .iter()
            .zip(&entry.files)
            .enumerate()
            .map(|(index, (item, file))| item.candidate(index as u32, file))
            .collect(),
    }
}

/// D13 step 2 for `entry`. The first call stages and inspects every file
/// (S5c: an async stream of blocking jobs, two at a time); later calls
/// return the stored result. If the selection is cancelled meanwhile, the
/// unfinished files come back `CANCELLED`, the result is not stored, and
/// the selection's staging is deleted.
pub async fn inspect_selection<R: Runtime>(
    app: &AppHandle<R>,
    storage: &Arc<Storage>,
    library: &LibraryServices<R>,
    entry: Arc<SelectionEntry>,
) -> Result<ImportInspection, CommandError> {
    let mut stored = entry.inspection.lock().await;
    if let Some(items) = stored.as_ref() {
        return Ok(wire(&entry, items));
    }

    let cancel = entry.cancel_flag();
    // Collected up front: a lazy iterator borrowing `entry` would make the
    // command's future not `Send`-provable (a higher-ranked closure).
    let jobs: Vec<StageJob<R>> = entry
        .files
        .iter()
        .enumerate()
        .map(|(index, file)| StageJob {
            app: app.clone(),
            storage: Arc::clone(storage),
            content: Arc::clone(&library.content),
            stream: Arc::clone(&library.stream),
            selection_id: entry.id.clone(),
            file_index: index,
            file: file.clone(),
            cancel: cancel.clone(),
        })
        .collect();
    let finished: Vec<(usize, InspectedItem)> = stream::iter(jobs)
        .map(|job| async move {
            let index = job.file_index;
            tauri::async_runtime::spawn_blocking(move || job.run())
                .await
                .map(|item| (index, item))
        })
        .buffer_unordered(FILES_IN_FLIGHT)
        .collect::<Vec<_>>()
        .await
        .into_iter()
        .collect::<Result<_, _>>()
        .map_err(|_| CommandError::internal())?;

    let mut items: Vec<InspectedItem> = (0..entry.files.len())
        .map(|_| InspectedItem::cancelled())
        .collect();
    for (index, item) in finished {
        items[index] = item;
    }
    let result = wire(&entry, &items);
    if entry.is_cancelled() {
        // The cancel already discarded the staging, but a job that was
        // mid-copy may have recreated the directory before it stopped.
        library.content.discard_staging(&entry.id);
    } else {
        *stored = Some(Arc::new(items));
    }
    Ok(result)
}

/// One file's blocking stage-and-inspect.
struct StageJob<R: Runtime> {
    app: AppHandle<R>,
    storage: Arc<Storage>,
    content: Arc<ContentStore>,
    stream: Arc<LibraryStream>,
    selection_id: String,
    file_index: usize,
    file: SelectedFile,
    cancel: CancelFlag,
}

impl<R: Runtime> StageJob<R> {
    fn run(self) -> InspectedItem {
        if self.cancel.is_cancelled() {
            return InspectedItem::cancelled();
        }
        if let Err(rejection) = check_path_policy(&self.file.path, self.storage.paths()) {
            return rejection;
        }
        let staged = match self.stage() {
            Ok(staged) => staged,
            Err(error) => return content_rejection(error),
        };
        match self.inspect(&staged) {
            Ok(item) => InspectedItem::Ready(Box::new(item)),
            Err(rejection) => {
                // A rejected file keeps nothing in staging.
                let _ = fs::remove_file(&staged.path);
                if let Some(directory) = staged.path.parent() {
                    let _ = fs::remove_file(directory.join(self.thumbnail_name()));
                }
                rejection
            }
        }
    }

    fn stage(&self) -> Result<StagedFile, ContentError> {
        let mut throttle = ProgressThrottle::default();
        let file_index = self.file_index as u32;
        self.content.stage_from_path(
            &self.file.path,
            &self.selection_id,
            self.file_index,
            &self.cancel,
            &mut |bytes_done, bytes_total| {
                if throttle.admit(bytes_done, bytes_total, Instant::now()) {
                    events::publish_import_progress(
                        &self.app,
                        &self.stream,
                        &self.selection_id,
                        ImportProgress {
                            file_index,
                            bytes_done,
                            bytes_total,
                        },
                    );
                }
            },
        )
    }

    fn inspect(&self, staged: &StagedFile) -> Result<ReadyItem, InspectedItem> {
        let detected =
            formats::detect_named(&staged.path, &self.file.file_name).map_err(inspect_rejection)?;
        let mut outcome =
            formats::inspect(&staged.path, &detected, &self.cancel).map_err(inspect_rejection)?;
        let thumbnail = outcome
            .thumbnail
            .take()
            .map(|thumbnail| self.stage_thumbnail(thumbnail))
            .transpose()?;
        let duplicates = find_duplicates(&self.storage, &staged.sha256).map_err(|_| {
            InspectedItem::rejected(
                ImportItemErrorCode::PersistenceUnavailable,
                "farm3d couldn't check the Library for this file.",
            )
        })?;
        Ok(ReadyItem {
            staged: staged.clone(),
            format: detected.format,
            outcome,
            thumbnail,
            duplicates,
        })
    }

    fn stage_thumbnail(&self, thumbnail: ThumbnailBytes) -> Result<StagedThumbnail, InspectedItem> {
        let staged = self
            .content
            .stage_bytes(&thumbnail.bytes, &self.selection_id, &self.thumbnail_name())
            .map_err(content_rejection)?;
        Ok(StagedThumbnail {
            staged,
            origin_part: thumbnail.origin_part,
            width: thumbnail.width,
            height: thumbnail.height,
        })
    }

    fn thumbnail_name(&self) -> String {
        format!("{}.thumb.png", self.file_index)
    }
}

/// S5a: the first and final progress for a file always pass; the ones in
/// between pass at most once per [`PROGRESS_INTERVAL`].
#[derive(Default)]
struct ProgressThrottle {
    last: Option<Instant>,
}

impl ProgressThrottle {
    fn admit(&mut self, bytes_done: u64, bytes_total: u64, now: Instant) -> bool {
        let due = match self.last {
            None => true,
            Some(last) => {
                bytes_done >= bytes_total
                    || now.saturating_duration_since(last) >= PROGRESS_INTERVAL
            }
        };
        if due {
            self.last = Some(now);
        }
        due
    }
}

/// D6: a file may be imported only from an absolute, lexically normal
/// path whose target (following symlinks) is a regular file outside
/// farm3d's own metadata and content trees.
pub fn check_path_policy(path: &Path, paths: &StoragePaths) -> Result<(), InspectedItem> {
    let not_allowed = || {
        InspectedItem::rejected(
            ImportItemErrorCode::PathNotAllowed,
            "farm3d can't import files from this location.",
        )
    };
    if normalize_absolute(path).ok().as_deref() != Some(path) {
        return Err(not_allowed());
    }
    let metadata = fs::metadata(path).map_err(|_| unreadable())?;
    if !metadata.is_file() {
        return Err(content_rejection(ContentError::NotAFile));
    }
    let canonical = path.canonicalize().map_err(|_| unreadable())?;
    if canonical.starts_with(paths.metadata_root()) || canonical.starts_with(paths.content_root()) {
        return Err(not_allowed());
    }
    Ok(())
}

fn unreadable() -> InspectedItem {
    InspectedItem::rejected(
        ImportItemErrorCode::SourceUnreadable,
        "farm3d couldn't read the file.",
    )
}

/// Per-file staging failures are item outcomes, not command errors.
fn content_rejection(error: ContentError) -> InspectedItem {
    match error {
        ContentError::TooLarge => InspectedItem::rejected(
            ImportItemErrorCode::TooLarge,
            "The file is larger than the 1 GiB import limit.",
        ),
        ContentError::NotAFile => InspectedItem::rejected(
            ImportItemErrorCode::NotAFile,
            "This isn't a file. Folders aren't imported.",
        ),
        ContentError::Unreadable(_) => unreadable(),
        ContentError::ChangedDuringRead => InspectedItem::rejected(
            ImportItemErrorCode::SourceChangedDuringRead,
            "The file changed while farm3d was reading it. Try again.",
        ),
        ContentError::Cancelled => InspectedItem::cancelled(),
        ContentError::HashMismatch
        | ContentError::Io
        | ContentError::Storage(_)
        | ContentError::Repository(_) => InspectedItem::rejected(
            ImportItemErrorCode::PersistenceUnavailable,
            "farm3d couldn't store its copy of the file.",
        ),
    }
}

fn inspect_rejection(error: InspectError) -> InspectedItem {
    let code = match &error {
        InspectError::UnsupportedFormat { .. } => ImportItemErrorCode::UnsupportedFormat,
        InspectError::InvalidContent(_) => ImportItemErrorCode::InvalidContent,
        InspectError::Cancelled => ImportItemErrorCode::Cancelled,
        InspectError::Io(_) => ImportItemErrorCode::SourceUnreadable,
    };
    InspectedItem::rejected(code, error.message())
}

/// D14: every Model with any revision of `sha256`, by name.
fn find_duplicates(storage: &Storage, sha256: &str) -> Result<Vec<DuplicateMatch>, StorageError> {
    storage
        .read(|connection| Ok(duplicates_in(connection, sha256)))
        .and_then(|inner| inner)
}

fn duplicates_in(
    connection: &Connection,
    sha256: &str,
) -> Result<Vec<DuplicateMatch>, StorageError> {
    let mut statement = connection.prepare(
        "SELECT r.model_id, m.name, r.id, r.sequence,
                r.sequence = (SELECT MAX(c.sequence) FROM model_source_revisions c
                              WHERE c.model_id = r.model_id)
         FROM model_source_revisions r
         JOIN library_models m ON m.id = r.model_id
         WHERE r.content_sha256 = ?1
           AND r.sequence = (SELECT MAX(s.sequence) FROM model_source_revisions s
                             WHERE s.model_id = r.model_id AND s.content_sha256 = ?1)
         ORDER BY lower(m.name), m.id",
    )?;
    let rows = statement
        .query_map([sha256], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, i64>(3)?,
                row.get::<_, bool>(4)?,
            ))
        })?
        .collect::<rusqlite::Result<Vec<_>>>()?;
    rows.into_iter()
        .map(
            |(model_id, model_name, revision_id, sequence, is_current)| {
                Ok(DuplicateMatch {
                    project_ids: repository::project_ids_for(connection, &model_id)?,
                    model_id,
                    model_name,
                    revision_id,
                    sequence,
                    is_current,
                })
            },
        )
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_passes_first_and_last_and_throttles_the_middle() {
        let start = Instant::now();
        let mut throttle = ProgressThrottle::default();
        assert!(throttle.admit(1, 10, start));
        assert!(!throttle.admit(2, 10, start + Duration::from_millis(100)));
        assert!(!throttle.admit(3, 10, start + Duration::from_millis(249)));
        assert!(throttle.admit(4, 10, start + Duration::from_millis(250)));
        assert!(!throttle.admit(5, 10, start + Duration::from_millis(300)));
        assert!(throttle.admit(10, 10, start + Duration::from_millis(301)));
    }

    #[test]
    fn staging_failures_map_to_import_item_codes() {
        let code = |error| match content_rejection(error) {
            InspectedItem::Rejected { code, .. } => code,
            InspectedItem::Ready(_) => unreachable!(),
        };
        assert_eq!(code(ContentError::TooLarge), ImportItemErrorCode::TooLarge);
        assert_eq!(code(ContentError::NotAFile), ImportItemErrorCode::NotAFile);
        assert_eq!(
            code(ContentError::Unreadable(
                std::io::ErrorKind::PermissionDenied
            )),
            ImportItemErrorCode::SourceUnreadable
        );
        assert_eq!(
            code(ContentError::ChangedDuringRead),
            ImportItemErrorCode::SourceChangedDuringRead
        );
        assert_eq!(
            code(ContentError::Cancelled),
            ImportItemErrorCode::Cancelled
        );
        assert_eq!(
            code(ContentError::Io),
            ImportItemErrorCode::PersistenceUnavailable
        );
    }

    #[test]
    fn item_error_codes_use_the_spec_spellings() {
        use ImportItemErrorCode::*;
        assert_eq!(
            serde_json::to_value([
                Validation,
                NotFound,
                Conflict,
                UnsupportedFormat,
                InvalidContent,
                TooLarge,
                NotAFile,
                PathNotAllowed,
                SourceUnreadable,
                SourceChangedDuringRead,
                DuplicateDecisionRequired,
                UnsupportedNotAcknowledged,
                PersistenceUnavailable,
                Cancelled,
            ])
            .unwrap(),
            serde_json::json!([
                "VALIDATION",
                "NOT_FOUND",
                "CONFLICT",
                "UNSUPPORTED_FORMAT",
                "INVALID_CONTENT",
                "TOO_LARGE",
                "NOT_A_FILE",
                "PATH_NOT_ALLOWED",
                "SOURCE_UNREADABLE",
                "SOURCE_CHANGED_DURING_READ",
                "DUPLICATE_DECISION_REQUIRED",
                "UNSUPPORTED_NOT_ACKNOWLEDGED",
                "PERSISTENCE_UNAVAILABLE",
                "CANCELLED",
            ])
        );
    }
}
