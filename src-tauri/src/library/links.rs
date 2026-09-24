//! D15 and D16: following linked Model sources.
//!
//! The watcher is only a hint. The authority is [`check_linked_source`], a
//! stat-then-hash check that runs from four triggers: the startup pass
//! ([`LinkSupervisor::reconcile_all`]), a debounced watch event for the
//! Model's parent directory, `check_linked_sources`, and a retry timer for a
//! source that was changing while it was read.
//!
//! [`LinkSupervisor`] owns the watches: one `NonRecursive` watch per
//! distinct parent directory, shared and reference-counted across Models.
//! A parent directory that does not exist is covered by a watch on its
//! nearest existing ancestor until it reappears; so is a watched directory
//! that is removed, because its native watch does not survive (Task 1,
//! Gate C). Native watches go through `notify-debouncer-full` (750 ms). A
//! directory whose native watch cannot be registered falls back to a
//! `PollWatcher` at 10 s.
//!
//! Checks run one at a time per Model. A watch event for a Model that
//! already has a check queued is absorbed into it, so one save produces one
//! check. Everything a check writes goes through one transaction, and its
//! events are published after commit.
//!
//! [`locate_source`] is D16's Locate: relinking a Model to a file the user
//! chose, with a `relocate` revision when its content differs.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::{Duration, Instant};

use notify::event::{AccessKind, AccessMode};
use notify::{EventKind, PollWatcher, RecommendedWatcher, RecursiveMode};
use notify_debouncer_full::{
    new_debouncer, new_debouncer_opt, DebounceEventResult, Debouncer, NoCache, RecommendedCache,
};
use rusqlite::Transaction;
use tauri::{AppHandle, Runtime};
use tokio::sync::{mpsc, watch};

use crate::contracts::command::CommandError;
use crate::persistence::{RepositoryError, Storage, StorageError};

use super::content::{
    CancelFlag, ContentError, ContentStore, SourceStat, StagedFile, MAX_SOURCE_BYTES,
};
use super::events::{self, LibraryEventSpec};
use super::formats::{self, Detected, InspectError, InspectOutcome};
use super::inspection::{check_path_policy, ImportItemErrorCode, InspectedItem, StagedThumbnail};
use super::repository::{self, RevisionSource};
use super::selection::SelectedFile;
use super::{
    new_id, LibraryServices, ModelFormat, ModelRecord, ModelSourceRevisionSummary, RevisionOrigin,
    SourceState, StorageMode, StoredModel, WatchMode,
};

/// D15: the debounce for native watch events (Task 1, Gate C).
pub const NATIVE_DEBOUNCE: Duration = Duration::from_millis(750);

/// D15: how often a directory whose native watch failed is polled.
pub const POLL_FALLBACK_INTERVAL: Duration = Duration::from_secs(10);

/// D15: when a `changing` source is checked again, counted from the check
/// before. After the last one it stays `changing` until the next trigger.
pub const CHANGING_RETRY_DELAYS: [Duration; 3] = [
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];

/// D7: how often the supervisor discards expired import selections, which
/// are otherwise reclaimed only when the registry is next used.
const SELECTION_SWEEP_INTERVAL: Duration = Duration::from_secs(60);

/// The staged name of a captured revision's thumbnail (never `.part`, P9).
const THUMBNAIL_NAME: &str = "0.thumb.png";

/// How the supervisor follows linked sources. Production uses
/// [`WatchPolicy::native`] (Gate C passed). `PollOnly` is the spike's
/// fallback for a host where native watching fails; tests also use it with
/// a long interval to take the watcher out of the picture.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WatchPolicy {
    /// Native watches with this debounce. A directory whose watch can't be
    /// registered is polled every [`POLL_FALLBACK_INTERVAL`].
    Native { debounce: Duration },
    /// Every directory is polled at `interval`.
    PollOnly { interval: Duration },
}

impl WatchPolicy {
    pub fn native() -> Self {
        Self::Native {
            debounce: NATIVE_DEBOUNCE,
        }
    }
}

/// What a linked path is, following symlinks (D6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FileKind {
    File,
    Directory,
    Other,
}

/// A linked path's `metadata`, as [`LinkFs`] reports it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkStat {
    pub kind: FileKind,
    pub stat: SourceStat,
}

/// The filesystem as [`check_linked_source`] sees it. Production is
/// [`NativeLinkFs`]; unit tests supply stats and bytes from memory.
pub trait LinkFs {
    /// `metadata(path)`, following symlinks.
    fn metadata(&self, path: &Path) -> io::Result<LinkStat>;
    /// D4 steps 1-2: copies the source into `staging/<staging_key>/` while
    /// hashing, failing with `ChangedDuringRead` if it moved meanwhile.
    fn stage(
        &self,
        content: &ContentStore,
        path: &Path,
        staging_key: &str,
    ) -> Result<StagedFile, ContentError>;
}

/// The real filesystem.
pub struct NativeLinkFs;

impl LinkFs for NativeLinkFs {
    fn metadata(&self, path: &Path) -> io::Result<LinkStat> {
        let metadata = fs::metadata(path)?;
        let kind = if metadata.is_file() {
            FileKind::File
        } else if metadata.is_dir() {
            FileKind::Directory
        } else {
            FileKind::Other
        };
        Ok(LinkStat {
            kind,
            stat: SourceStat::of(&metadata),
        })
    }

    fn stage(
        &self,
        content: &ContentStore,
        path: &Path,
        staging_key: &str,
    ) -> Result<StagedFile, ContentError> {
        content.stage_from_path(path, staging_key, 0, &CancelFlag::never(), &mut |_, _| {})
    }
}

/// A linked Model as a check sees it: its stored link and the hash of its
/// current revision.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkSnapshot {
    pub model_id: String,
    pub model_revision: i64,
    pub path: PathBuf,
    pub format: ModelFormat,
    pub state: SourceState,
    pub observed_size: Option<i64>,
    pub observed_mtime_ns: Option<i64>,
    pub observed_file_id: Option<String>,
    pub current_sha256: String,
}

impl LinkSnapshot {
    /// The linked Model `model_id`, or `None` when there is no such Model
    /// or it is managed.
    pub fn load(
        connection: &rusqlite::Connection,
        model_id: &str,
    ) -> Result<Option<Self>, StorageError> {
        let Some(model) = repository::load_model(connection, model_id)? else {
            return Ok(None);
        };
        let (Some(path), Some(state)) = (model.linked_path, model.link_state) else {
            return Ok(None);
        };
        let Some(current_sha256) = repository::current_revision_sha256(connection, model_id)?
        else {
            return Ok(None);
        };
        Ok(Some(Self {
            model_id: model.id,
            model_revision: model.revision,
            path: PathBuf::from(path),
            format: model.format,
            state,
            observed_size: model.link_observed_size,
            observed_mtime_ns: model.link_observed_mtime_ns,
            observed_file_id: model.link_observed_file_id,
            current_sha256,
        }))
    }

    /// The source's basename, the only part events and errors show (D6).
    pub fn file_name(&self) -> String {
        basename(&self.path)
    }

    /// D6: whether `stat` is what was last observed (size, modification
    /// time in nanoseconds, and file id).
    fn observed(&self, stat: &SourceStat) -> bool {
        same_observation(
            self.observed_size,
            self.observed_mtime_ns,
            self.observed_file_id.as_deref(),
            stat,
        )
    }
}

fn same_observation(
    size: Option<i64>,
    mtime_ns: Option<i64>,
    file_id: Option<&str>,
    stat: &SourceStat,
) -> bool {
    size.is_some()
        && size == i64::try_from(stat.size).ok()
        && mtime_ns.is_some()
        && mtime_ns == stat.modified_ns()
        && file_id == stat.file_id.as_deref()
}

/// What [`check_linked_source`] found.
#[derive(Debug)]
pub enum CheckOutcome {
    /// D15 step 2: the source is `ok` and its stat is unchanged. Nothing to
    /// write.
    Unchanged,
    /// A state that creates no revision. `stat` is the source's observed
    /// stat, when there was a readable file to observe.
    State {
        state: SourceState,
        stat: Option<SourceStat>,
    },
    /// D15 step 6: new content that inspected cleanly as the Model's
    /// format, staged and ready to capture as a revision.
    NewContent(Box<CapturedContent>),
}

impl CheckOutcome {
    fn state(state: SourceState) -> Self {
        Self::State { state, stat: None }
    }
}

/// New linked content, staged and inspected.
#[derive(Debug)]
pub struct CapturedContent {
    pub staged: StagedFile,
    pub outcome: InspectOutcome,
    pub thumbnail: Option<StagedThumbnail>,
}

/// D15's `check_linked_source`, steps 1-5; [`apply_outcome`] is step 6.
/// Staged bytes that aren't captured are deleted before it returns. `Err`
/// is farm3d's own failure (its store), never the source's.
pub fn check_linked_source(
    fs: &dyn LinkFs,
    content: &ContentStore,
    link: &LinkSnapshot,
    staging_key: &str,
) -> Result<CheckOutcome, CommandError> {
    // Step 1.
    let found = match fs.metadata(&link.path) {
        Ok(found) => found,
        Err(error) => return Ok(CheckOutcome::state(state_for_error(error.kind()))),
    };
    if found.kind != FileKind::File {
        return Ok(CheckOutcome::state(SourceState::NotAFile));
    }
    if found.stat.size > MAX_SOURCE_BYTES {
        return Ok(CheckOutcome::State {
            state: SourceState::Unreadable,
            stat: Some(found.stat),
        });
    }
    // Step 2: the common case costs one stat.
    if link.state == SourceState::Ok && link.observed(&found.stat) {
        return Ok(CheckOutcome::Unchanged);
    }
    // Step 3.
    let staged = match fs.stage(content, &link.path, staging_key) {
        Ok(staged) => staged,
        Err(error) => return staging_failure(error),
    };
    let stat = staged.source.clone();
    // Step 4.
    if staged.sha256 == link.current_sha256 {
        remove_staged(&staged);
        return Ok(CheckOutcome::State {
            state: SourceState::Ok,
            stat,
        });
    }
    // Step 5: inspect the staged bytes as the Model's format.
    match inspect_as(
        content,
        &staged,
        link.format,
        &link.file_name(),
        staging_key,
    ) {
        Ok(Some((outcome, thumbnail))) => Ok(CheckOutcome::NewContent(Box::new(CapturedContent {
            staged,
            outcome,
            thumbnail,
        }))),
        Ok(None) => {
            remove_staged(&staged);
            Ok(CheckOutcome::State {
                state: SourceState::InvalidContent,
                stat,
            })
        }
        Err(error) => {
            remove_staged(&staged);
            Err(error)
        }
    }
}

/// D15 step 1: `NotFound` (or a path component that is no longer a
/// directory) is `missing`; anything else that stops the read is
/// `unreadable`.
fn state_for_error(kind: io::ErrorKind) -> SourceState {
    match kind {
        io::ErrorKind::NotFound | io::ErrorKind::NotADirectory => SourceState::Missing,
        _ => SourceState::Unreadable,
    }
}

fn staging_failure(error: ContentError) -> Result<CheckOutcome, CommandError> {
    let state = match error {
        ContentError::ChangedDuringRead => SourceState::Changing,
        ContentError::NotAFile => SourceState::NotAFile,
        ContentError::TooLarge => SourceState::Unreadable,
        ContentError::Unreadable(kind) => state_for_error(kind),
        ContentError::Cancelled
        | ContentError::HashMismatch
        | ContentError::Io
        | ContentError::Storage(_)
        | ContentError::Repository(_) => return Err(error.into()),
    };
    Ok(CheckOutcome::state(state))
}

/// Detects and inspects staged bytes as `format`. `Ok(None)` means the
/// bytes are not a readable `format` file; `Err` is farm3d's own failure.
/// The embedded thumbnail, if any, is staged beside them.
fn inspect_as(
    content: &ContentStore,
    staged: &StagedFile,
    format: ModelFormat,
    file_name: &str,
    staging_key: &str,
) -> Result<Option<(InspectOutcome, Option<StagedThumbnail>)>, CommandError> {
    let detected = match formats::detect_named(&staged.path, file_name) {
        Ok(detected) if detected.format == format => detected,
        Ok(_) => return Ok(None),
        Err(error) => return unreadable_content(error),
    };
    let mut outcome = match formats::inspect(&staged.path, &detected, &CancelFlag::never()) {
        Ok(outcome) => outcome,
        Err(error) => return unreadable_content(error),
    };
    let thumbnail = stage_thumbnail(content, &mut outcome, staging_key)?;
    Ok(Some((outcome, thumbnail)))
}

fn unreadable_content<T>(error: InspectError) -> Result<Option<T>, CommandError> {
    match error {
        InspectError::UnsupportedFormat { .. } | InspectError::InvalidContent(_) => Ok(None),
        InspectError::Io(_) | InspectError::Cancelled => {
            Err(CommandError::persistence_unavailable())
        }
    }
}

/// D12: moves an inspection's thumbnail bytes into staging.
fn stage_thumbnail(
    content: &ContentStore,
    outcome: &mut InspectOutcome,
    staging_key: &str,
) -> Result<Option<StagedThumbnail>, CommandError> {
    let Some(thumbnail) = outcome.thumbnail.take() else {
        return Ok(None);
    };
    let staged = content.stage_bytes(&thumbnail.bytes, staging_key, THUMBNAIL_NAME)?;
    Ok(Some(StagedThumbnail {
        staged,
        origin_part: thumbnail.origin_part,
        width: thumbnail.width,
        height: thumbnail.height,
    }))
}

fn remove_staged(staged: &StagedFile) {
    let _ = fs::remove_file(&staged.path);
}

/// The result of applying a check: the source state it left (`None` when
/// the Model is no longer linked as the check saw it), and, when anything
/// was written, the committed record and any revision it created.
#[derive(Debug, Default)]
pub struct Applied {
    pub state: Option<SourceState>,
    pub record: Option<ModelRecord>,
    pub revision: Option<ModelSourceRevisionSummary>,
}

impl Applied {
    /// `library.model.changed`, then `library.revision.created` when a
    /// revision was added. Empty when nothing was written.
    pub fn events(&self) -> Vec<LibraryEventSpec> {
        let Some(record) = &self.record else {
            return Vec::new();
        };
        std::iter::once(events::model_changed(record))
            .chain(self.revision.as_ref().map(events::revision_created))
            .collect()
    }
}

/// D15 step 6, and the writes for states that create no revision. Commits
/// `outcome` for `link` in one transaction and bumps the Model's revision.
/// A state that is already stored (with the same stat) writes nothing.
/// If the Model's link changed since `link` was read (relinked, converted,
/// deleted, or given another revision), nothing is written: the check is
/// stale, and whatever changed the link has its own check.
pub fn apply_outcome<R: Runtime>(
    library: &LibraryServices<R>,
    storage: &Storage,
    link: &LinkSnapshot,
    outcome: CheckOutcome,
) -> Result<Applied, CommandError> {
    let committed = match outcome {
        CheckOutcome::Unchanged => {
            return Ok(Applied {
                state: Some(link.state),
                ..Applied::default()
            })
        }
        CheckOutcome::State { state, stat } => storage
            .write_repo(|tx| {
                let stored = linked_as_seen(tx, link)?;
                let stat_changed = stat.as_ref().is_some_and(|stat| {
                    !same_observation(
                        stored.link_observed_size,
                        stored.link_observed_mtime_ns,
                        stored.link_observed_file_id.as_deref(),
                        stat,
                    )
                });
                if stored.link_state == Some(state) && !stat_changed {
                    return Ok(Applied {
                        state: Some(state),
                        ..Applied::default()
                    });
                }
                repository::record_link_check(tx, &link.model_id, state, stat.as_ref())?;
                Ok(Applied {
                    state: Some(state),
                    record: library.record_for(tx, &link.model_id)?,
                    revision: None,
                })
            })
            .map_err(ContentError::Repository),
        CheckOutcome::NewContent(captured) => capture(library, storage, link, &captured),
    };
    match committed {
        Ok(applied) => Ok(applied),
        Err(ContentError::Repository(RepositoryError::Conflict { .. })) => Ok(Applied::default()),
        Err(error) => Err(error.into()),
    }
}

/// D15 step 6: places the captured blob(s) and, in one transaction, adds
/// the `linkedChange` revision, records the observed stat, and sets `ok`.
fn capture<R: Runtime>(
    library: &LibraryServices<R>,
    storage: &Storage,
    link: &LinkSnapshot,
    captured: &CapturedContent,
) -> Result<Applied, ContentError> {
    let staged: Vec<&StagedFile> = std::iter::once(&captured.staged)
        .chain(
            captured
                .thumbnail
                .as_ref()
                .map(|thumbnail| &thumbnail.staged),
        )
        .collect();
    let source_path = link.path.to_string_lossy();
    let file_name = link.file_name();
    let mtime = captured
        .staged
        .source
        .as_ref()
        .and_then(SourceStat::modified_rfc3339);
    library.content.place_and_commit(storage, &staged, |tx| {
        linked_as_seen(tx, link)?;
        add_revision(
            tx,
            &link.model_id,
            captured,
            RevisionOrigin::LinkedChange,
            &RevisionSource {
                path: &source_path,
                file_name: &file_name,
                mtime: mtime.as_deref(),
            },
        )?;
        repository::record_link_check(
            tx,
            &link.model_id,
            SourceState::Ok,
            captured.staged.source.as_ref(),
        )?;
        let record = committed_record(library, tx, &link.model_id)?;
        Ok(Applied {
            state: Some(SourceState::Ok),
            revision: Some(record.current_revision.clone()),
            record: Some(record),
        })
    })
}

fn add_revision(
    tx: &Transaction<'_>,
    model_id: &str,
    captured: &CapturedContent,
    origin: RevisionOrigin,
    source: &RevisionSource<'_>,
) -> Result<(), RepositoryError> {
    let revision = repository::insert_revision(
        tx,
        model_id,
        &captured.staged,
        &captured.outcome,
        origin,
        source,
    )?;
    if let Some(thumbnail) = &captured.thumbnail {
        repository::insert_thumbnail(tx, &revision.id, thumbnail)?;
    }
    Ok(())
}

fn committed_record<R: Runtime>(
    library: &LibraryServices<R>,
    tx: &Transaction<'_>,
    model_id: &str,
) -> Result<ModelRecord, RepositoryError> {
    library
        .record_for(tx, model_id)?
        .ok_or_else(|| RepositoryError::NotFound {
            entity_id: model_id.to_string(),
        })
}

/// Inside a check's transaction: the Model is still linked to the same path
/// with the same current revision as when the check read it. Otherwise the
/// check is stale (`Conflict`).
fn linked_as_seen(
    tx: &Transaction<'_>,
    link: &LinkSnapshot,
) -> Result<StoredModel, RepositoryError> {
    let stale = |current_revision| RepositoryError::Conflict {
        entity_id: link.model_id.clone(),
        expected_revision: link.model_revision,
        current_revision,
    };
    let stored = repository::load_model(tx, &link.model_id)?.ok_or_else(|| stale(0))?;
    let current = repository::current_revision_sha256(tx, &link.model_id)?;
    let same_link = stored.storage_mode == StorageMode::Linked
        && stored.linked_path.as_deref().map(Path::new) == Some(link.path.as_path())
        && current.as_deref() == Some(link.current_sha256.as_str());
    if !same_link {
        return Err(stale(stored.revision));
    }
    Ok(stored)
}

/// D15's retry timer for a `changing` source: runs `check` after each of
/// [`CHANGING_RETRY_DELAYS`] while it keeps reporting `changing`, then
/// stops.
pub async fn retry_while_changing<F, Fut>(mut check: F)
where
    F: FnMut() -> Fut,
    Fut: Future<Output = Option<SourceState>>,
{
    for delay in CHANGING_RETRY_DELAYS {
        tokio::time::sleep(delay).await;
        if check().await != Some(SourceState::Changing) {
            return;
        }
    }
}

/// One whole check of Model `model_id` (blocking): load its link, check it,
/// and commit what changed. A Model that isn't linked is left alone.
pub fn check_model<R: Runtime>(
    library: &LibraryServices<R>,
    storage: &Storage,
    model_id: &str,
) -> Result<Applied, CommandError> {
    let link = storage
        .read(|connection| Ok(LinkSnapshot::load(connection, model_id)))
        .and_then(|loaded| loaded)
        .map_err(storage_error)?;
    let Some(link) = link else {
        return Ok(Applied::default());
    };
    let staging_key = new_id("lnk");
    let applied = check_linked_source(&NativeLinkFs, &library.content, &link, &staging_key)
        .and_then(|outcome| apply_outcome(library, storage, &link, outcome));
    library.content.discard_staging(&staging_key);
    applied
}

fn storage_error(error: StorageError) -> CommandError {
    CommandError::from_repository(RepositoryError::Storage(error))
}

/// D16's Locate, committed: the relinked Model and, when the located file
/// had different content, the `relocate` revision it added.
#[derive(Debug)]
pub struct Located {
    pub record: ModelRecord,
    pub revision: Option<ModelSourceRevisionSummary>,
}

/// D16: relinks linked Model `model_id` to `file`, the file the user chose.
/// Blocking. The file is staged, hashed, and inspected, and must have the
/// Model's format. The same content as the current revision relinks with no
/// revision; different content is `SOURCE_CONTENT_DIFFERS` unless
/// `accept_different_content`, which relinks and adds a `relocate` revision.
/// The caller starts following the new path after this returns.
pub fn locate_source<R: Runtime>(
    library: &LibraryServices<R>,
    storage: &Storage,
    model_id: &str,
    expected_revision: i64,
    file: &SelectedFile,
    accept_different_content: bool,
) -> Result<Located, CommandError> {
    let model = storage
        .read(|connection| Ok(checked_linked(connection, model_id, expected_revision)))
        .map_err(storage_error)?
        .map_err(locate_error)?;
    if let Err(InspectedItem::Rejected { code, .. }) =
        check_path_policy(&file.path, storage.paths())
    {
        return Err(match code {
            ImportItemErrorCode::PathNotAllowed => CommandError::validation_at(
                "fileIndex",
                "farm3d can't link to files in this location.",
            ),
            _ => CommandError::source_unavailable(&file.file_name),
        });
    }
    let path = file.path.to_str().ok_or_else(|| {
        CommandError::validation_at("fileIndex", "farm3d can't link to this file's location.")
    })?;
    let staging_key = new_id("loc");
    let located = Relocation {
        library,
        storage,
        model: &model,
        expected_revision,
        file,
        path,
        staging_key: &staging_key,
    }
    .run(accept_different_content);
    library.content.discard_staging(&staging_key);
    located
}

/// P2's revision check for Model `id`, which must be linked.
fn checked_linked(
    connection: &rusqlite::Connection,
    id: &str,
    expected_revision: i64,
) -> Result<StoredModel, RepositoryError> {
    let model = repository::checked_model(connection, id, expected_revision)?;
    if model.storage_mode != StorageMode::Linked {
        return Err(RepositoryError::Validation {
            field_path: "modelId",
        });
    }
    Ok(model)
}

fn locate_error(error: RepositoryError) -> CommandError {
    match error {
        RepositoryError::Validation {
            field_path: "modelId",
        } => CommandError::validation_at("modelId", "Only a linked Model has a source to locate."),
        other => CommandError::from_repository(other),
    }
}

struct Relocation<'a, R: Runtime> {
    library: &'a LibraryServices<R>,
    storage: &'a Storage,
    model: &'a StoredModel,
    expected_revision: i64,
    file: &'a SelectedFile,
    path: &'a str,
    staging_key: &'a str,
}

impl<R: Runtime> Relocation<'_, R> {
    fn run(&self, accept_different_content: bool) -> Result<Located, CommandError> {
        let content = &self.library.content;
        let staged = content
            .stage_from_path(
                &self.file.path,
                self.staging_key,
                0,
                &CancelFlag::never(),
                &mut |_, _| {},
            )
            .map_err(|error| match error {
                ContentError::HashMismatch
                | ContentError::Io
                | ContentError::Storage(_)
                | ContentError::Repository(_) => CommandError::from(error),
                _ => CommandError::source_unavailable(&self.file.file_name),
            })?;
        let stat = staged.source.clone().ok_or_else(CommandError::internal)?;
        let detected = self.detect(&staged)?;
        let current = self
            .storage
            .read(|connection| {
                Ok(repository::current_revision_sha256(
                    connection,
                    &self.model.id,
                ))
            })
            .and_then(|current| current)
            .map_err(storage_error)?
            .ok_or_else(CommandError::internal)?;
        if staged.sha256 == current {
            let record = self
                .storage
                .write_repo(|tx| {
                    checked_linked(tx, &self.model.id, self.expected_revision)?;
                    repository::relink_model(tx, &self.model.id, self.path, &stat)?;
                    committed_record(self.library, tx, &self.model.id)
                })
                .map_err(locate_error)?;
            return Ok(Located {
                record,
                revision: None,
            });
        }
        if !accept_different_content {
            return Err(CommandError::source_content_differs(
                &current,
                &staged.sha256,
                &self.file.file_name,
            ));
        }
        let mut outcome = formats::inspect(&staged.path, &detected, &CancelFlag::never())
            .map_err(|error| self.inspect_error(error))?;
        let thumbnail = stage_thumbnail(content, &mut outcome, self.staging_key)?;
        let captured = CapturedContent {
            staged,
            outcome,
            thumbnail,
        };
        let placed: Vec<&StagedFile> = std::iter::once(&captured.staged)
            .chain(
                captured
                    .thumbnail
                    .as_ref()
                    .map(|thumbnail| &thumbnail.staged),
            )
            .collect();
        let mtime = stat.modified_rfc3339();
        let record = content
            .place_and_commit(self.storage, &placed, |tx| {
                checked_linked(tx, &self.model.id, self.expected_revision)?;
                add_revision(
                    tx,
                    &self.model.id,
                    &captured,
                    RevisionOrigin::Relocate,
                    &RevisionSource {
                        path: self.path,
                        file_name: &self.file.file_name,
                        mtime: mtime.as_deref(),
                    },
                )?;
                repository::relink_model(tx, &self.model.id, self.path, &stat)?;
                committed_record(self.library, tx, &self.model.id)
            })
            .map_err(|error| match error {
                ContentError::Repository(error) => locate_error(error),
                other => other.into(),
            })?;
        Ok(Located {
            revision: Some(record.current_revision.clone()),
            record,
        })
    }

    /// D16: a located file of another format is `VALIDATION`.
    fn detect(&self, staged: &StagedFile) -> Result<Detected, CommandError> {
        let detected = formats::detect_named(&staged.path, &self.file.file_name)
            .map_err(|error| self.inspect_error(error))?;
        if detected.format != self.model.format {
            return Err(CommandError::validation_at(
                "fileIndex",
                format!(
                    "The chosen file is {}, but this Model is {}.",
                    format_label(detected.format),
                    format_label(self.model.format)
                ),
            ));
        }
        Ok(detected)
    }

    fn inspect_error(&self, error: InspectError) -> CommandError {
        match error {
            InspectError::UnsupportedFormat { reason, extensions } => {
                CommandError::unsupported_format(&reason, &extensions)
            }
            InspectError::InvalidContent(reason) => {
                CommandError::validation_at("fileIndex", reason)
            }
            InspectError::Io(_) | InspectError::Cancelled => {
                CommandError::source_unavailable(&self.file.file_name)
            }
        }
    }
}

fn format_label(format: ModelFormat) -> &'static str {
    match format {
        ModelFormat::Stl => "an STL file",
        ModelFormat::ThreeMf => "a 3MF file",
        ModelFormat::Gcode => "G-code",
    }
}

fn basename(path: &Path) -> String {
    path.file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_default()
}

type NativeDebouncer = Debouncer<RecommendedWatcher, RecommendedCache>;
type PollDebouncer = Debouncer<PollWatcher, NoCache>;

/// What the debouncer threads hand the supervisor's task.
#[derive(Debug, PartialEq, Eq)]
enum Signal {
    /// A debounced batch of events, by path.
    Paths(Vec<PathBuf>),
    /// The watcher reported an error (for example, a queue overflow), so
    /// events may have been lost: check everything.
    Everything,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Backend {
    Native,
    Poll,
    /// Neither backend could watch the directory.
    None,
}

/// One watched directory, shared by every Model whose source it covers.
struct WatchEntry {
    refcount: usize,
    mode: WatchMode,
    backend: Backend,
}

/// A followed Model: its source's parent directory, and the directory
/// actually watched for it (the parent, or its nearest existing ancestor
/// while the parent is missing).
struct Registration {
    parent: PathBuf,
    watched: PathBuf,
}

#[derive(Default)]
struct Watches {
    models: HashMap<String, Registration>,
    directories: HashMap<PathBuf, WatchEntry>,
    native: Option<NativeDebouncer>,
    poll: Option<PollDebouncer>,
    stopped: bool,
}

/// D15's supervisor: the watches, the per-Model check queue, and the retry
/// timers. Started by `start_library_runtime` and held in
/// `LibraryServices::links`. It holds a `Weak` reference to the
/// `LibraryServices` that hold it.
pub struct LinkSupervisor<R: Runtime> {
    this: Weak<Self>,
    services: Weak<LibraryServices<R>>,
    storage: Arc<Storage>,
    app: AppHandle<R>,
    policy: WatchPolicy,
    watches: Mutex<Watches>,
    signals: mpsc::UnboundedSender<Signal>,
    stop: watch::Sender<bool>,
    /// Models with a check waiting to start. A watch event for one of them
    /// is absorbed into that check.
    queued: Mutex<HashSet<String>>,
    /// Models whose `changing` retries are running.
    retrying: Mutex<HashSet<String>>,
    /// One lock per Model, so its checks run one at a time.
    gates: Mutex<HashMap<String, Arc<tokio::sync::Mutex<()>>>>,
    /// Checks spawned by [`Self::schedule`] that have not finished.
    scheduled: AtomicUsize,
    fail_next_native_watch: AtomicBool,
    reconciled: AtomicBool,
}

impl<R: Runtime> LinkSupervisor<R> {
    /// Starts the supervisor's event task and its selection sweep. Watches
    /// are added as Models are registered; [`Self::reconcile_all`] registers
    /// every linked Model and checks it.
    pub fn start(
        services: Weak<LibraryServices<R>>,
        storage: Arc<Storage>,
        app: AppHandle<R>,
        policy: WatchPolicy,
    ) -> Arc<Self> {
        let (signals, mut received) = mpsc::unbounded_channel();
        let supervisor = Arc::new_cyclic(|this| Self {
            this: this.clone(),
            services: services.clone(),
            storage,
            app,
            policy,
            watches: Mutex::default(),
            signals,
            stop: watch::channel(false).0,
            queued: Mutex::default(),
            retrying: Mutex::default(),
            gates: Mutex::default(),
            scheduled: AtomicUsize::new(0),
            fail_next_native_watch: AtomicBool::new(false),
            reconciled: AtomicBool::new(false),
        });

        let this = Arc::downgrade(&supervisor);
        let mut stopped = supervisor.stop.subscribe();
        tauri::async_runtime::spawn(async move {
            loop {
                tokio::select! {
                    signal = received.recv() => {
                        let (Some(signal), Some(supervisor)) = (signal, this.upgrade()) else {
                            break;
                        };
                        supervisor.handle(signal);
                    }
                    _ = stopped.wait_for(|stopped| *stopped) => break,
                }
            }
        });

        let mut stopped = supervisor.stop.subscribe();
        tauri::async_runtime::spawn(async move {
            let mut ticks = tokio::time::interval(SELECTION_SWEEP_INTERVAL);
            ticks.tick().await;
            loop {
                tokio::select! {
                    _ = ticks.tick() => {
                        let Some(services) = services.upgrade() else { break };
                        services.selections.sweep_expired(Instant::now());
                    }
                    _ = stopped.wait_for(|stopped| *stopped) => break,
                }
            }
        });
        supervisor
    }

    /// Starts following `linked_path` for `model_id`, replacing any earlier
    /// registration, and returns how its directory is watched (P14).
    pub fn register(&self, model_id: &str, linked_path: &Path) -> WatchMode {
        let Some(parent) = linked_path.parent().map(Path::to_path_buf) else {
            return WatchMode::NotWatched;
        };
        let mut watches = lock(&self.watches);
        if watches.stopped {
            return WatchMode::NotWatched;
        }
        if let Some(old) = watches.models.remove(model_id) {
            self.release(&mut watches, &old.watched);
        }
        let watched = nearest_existing_directory(&parent);
        let mode = self.acquire(&mut watches, &watched);
        watches
            .models
            .insert(model_id.to_string(), Registration { parent, watched });
        mode
    }

    /// Stops following `model_id`'s source (D5, D18). Its directory's watch
    /// goes when no other Model needs it.
    pub fn unregister(&self, model_id: &str) {
        let mut watches = lock(&self.watches);
        if let Some(old) = watches.models.remove(model_id) {
            self.release(&mut watches, &old.watched);
        }
    }

    /// How `model_id`'s source is being followed right now.
    pub fn watch_mode(&self, model_id: &str) -> WatchMode {
        let watches = lock(&self.watches);
        watches
            .models
            .get(model_id)
            .and_then(|registration| watches.directories.get(&registration.watched))
            .map_or(WatchMode::NotWatched, |entry| entry.mode)
    }

    /// Whether every directory is polled by policy, in which case polling
    /// is not a degraded mode worth a warning.
    pub fn is_poll_only(&self) -> bool {
        matches!(self.policy, WatchPolicy::PollOnly { .. })
    }

    /// `check_linked_sources`: checks `model_ids` (every linked Model when
    /// `None`) now, one at a time, and returns the records of those that
    /// changed. Unknown and managed Models are skipped.
    ///
    /// A check that fails on farm3d's side is logged by Model id and the
    /// rest still run. The result lists only changed records, so it has no
    /// room for failures: they are returned as the error only when every
    /// check failed, since then the list would falsely read as "unchanged".
    pub async fn check(
        &self,
        model_ids: Option<Vec<String>>,
    ) -> Result<Vec<ModelRecord>, CommandError> {
        let ids: Vec<String> = match model_ids {
            Some(ids) => {
                let mut seen = HashSet::new();
                ids.into_iter()
                    .filter(|id| seen.insert(id.clone()))
                    .collect()
            }
            None => self
                .linked_models()?
                .into_iter()
                .map(|(id, _)| id)
                .collect(),
        };
        let checked = ids.len();
        let mut changed = Vec::new();
        let mut failures = Vec::new();
        for id in ids {
            match self.run_check(&id).await {
                Ok(applied) => changed.extend(applied.record),
                Err(error) => {
                    eprintln!(
                        "farm3d: checking linked Model {id} failed: {}",
                        error.message
                    );
                    failures.push(error);
                }
            }
        }
        if failures.len() == checked {
            if let Some(error) = failures.into_iter().next() {
                return Err(error);
            }
        }
        Ok(changed)
    }

    /// D15 trigger 1: registers every linked Model, then checks each one.
    /// Runs in the background after startup and never blocks it.
    pub async fn reconcile_all(&self) {
        let linked = self.linked_models().unwrap_or_else(|error| {
            eprintln!(
                "farm3d: linked sources could not be listed: {}",
                error.message
            );
            Vec::new()
        });
        for (id, path) in &linked {
            self.register(id, Path::new(path));
        }
        for (id, _) in &linked {
            if let Err(error) = self.run_check(id).await {
                eprintln!(
                    "farm3d: checking linked Model {id} failed: {}",
                    error.message
                );
            }
        }
        self.reconciled.store(true, Ordering::SeqCst);
    }

    /// Whether [`Self::reconcile_all`] has finished.
    pub fn has_reconciled(&self) -> bool {
        self.reconciled.load(Ordering::SeqCst)
    }

    /// Stops watching and ends the supervisor's tasks. Queued checks that
    /// have not started are skipped. Registrations after this are ignored.
    pub fn shutdown(&self) {
        self.stop.send_replace(true);
        let mut watches = lock(&self.watches);
        watches.stopped = true;
        watches.models.clear();
        watches.directories.clear();
        watches.native = None;
        watches.poll = None;
    }

    /// Test hook: the next native `watch()` fails, as it does at the inotify
    /// watch limit, so that directory falls back to polling.
    #[doc(hidden)]
    pub fn fail_next_native_watch(&self) {
        self.fail_next_native_watch.store(true, Ordering::SeqCst);
    }

    /// Test hook: each watched directory with the number of Models it
    /// covers.
    #[doc(hidden)]
    pub fn watched_directories(&self) -> Vec<(PathBuf, usize)> {
        let watches = lock(&self.watches);
        let mut directories: Vec<_> = watches
            .directories
            .iter()
            .map(|(directory, entry)| (directory.clone(), entry.refcount))
            .collect();
        directories.sort();
        directories
    }

    fn linked_models(&self) -> Result<Vec<(String, String)>, CommandError> {
        self.storage
            .read(|connection| Ok(repository::linked_models(connection)))
            .and_then(|linked| linked)
            .map_err(storage_error)
    }

    /// A debounced batch: every Model whose watched directory saw an event
    /// is checked. A watched directory that was itself removed, moved, or
    /// replaced has lost its native watch, so it is watched again, or its
    /// Models move to the nearest existing ancestor.
    fn handle(&self, signal: Signal) {
        let due: Vec<String> = {
            let mut watches = lock(&self.watches);
            if watches.stopped {
                return;
            }
            match signal {
                Signal::Everything => watches.models.keys().cloned().collect(),
                Signal::Paths(paths) => {
                    let touched: HashSet<PathBuf> = watches
                        .directories
                        .keys()
                        .filter(|directory| {
                            paths.iter().any(|path| {
                                path == *directory || path.parent() == Some(directory.as_path())
                            })
                        })
                        .cloned()
                        .collect();
                    let due = watches
                        .models
                        .iter()
                        .filter(|(_, registration)| touched.contains(&registration.watched))
                        .map(|(id, _)| id.clone())
                        .collect();
                    for directory in touched.iter().filter(|directory| paths.contains(directory)) {
                        self.reestablish(&mut watches, directory);
                    }
                    due
                }
            }
        };
        for id in due {
            self.schedule(&id);
        }
    }

    /// Queues a check of `model_id`, unless one is already waiting.
    pub(crate) fn schedule(&self, model_id: &str) {
        if !lock(&self.queued).insert(model_id.to_string()) {
            return;
        }
        self.scheduled.fetch_add(1, Ordering::SeqCst);
        let this = self.this.clone();
        let id = model_id.to_string();
        tauri::async_runtime::spawn(async move {
            if let Some(supervisor) = this.upgrade() {
                if let Err(error) = supervisor.run_check(&id).await {
                    eprintln!(
                        "farm3d: checking linked Model {id} failed: {}",
                        error.message
                    );
                }
                supervisor.scheduled.fetch_sub(1, Ordering::SeqCst);
            }
        });
    }

    /// Test hook: whether a check queued by a watch event, a registration,
    /// or a moved watch has yet to finish.
    #[doc(hidden)]
    pub fn has_scheduled_checks(&self) -> bool {
        self.scheduled.load(Ordering::SeqCst) > 0
    }

    /// Checks `model_id` under its gate, publishes what changed, then moves
    /// its watch if its parent directory came or went and starts retries if
    /// it is `changing`.
    async fn run_check(&self, model_id: &str) -> Result<Applied, CommandError> {
        let gate = self.gate(model_id);
        let held = gate.lock().await;
        lock(&self.queued).remove(model_id);
        if lock(&self.watches).stopped {
            return Ok(Applied::default());
        }
        let services = self.services.upgrade().ok_or_else(CommandError::internal)?;
        let storage = Arc::clone(&self.storage);
        let id = model_id.to_string();
        let library = Arc::clone(&services);
        let applied =
            tauri::async_runtime::spawn_blocking(move || check_model(&library, &storage, &id))
                .await
                .map_err(|_| CommandError::internal())??;
        services.stream.publish(&self.app, applied.events());
        drop(held);
        if self.follow_parent(model_id) {
            self.schedule(model_id);
        }
        if applied.state == Some(SourceState::Changing) {
            self.retry(model_id);
        }
        Ok(applied)
    }

    /// D15 trigger 4: checks a `changing` Model again after 2, 4, and 8 s.
    fn retry(&self, model_id: &str) {
        if !lock(&self.retrying).insert(model_id.to_string()) {
            return;
        }
        let this = self.this.clone();
        let id = model_id.to_string();
        tauri::async_runtime::spawn(async move {
            retry_while_changing(|| {
                let (this, id) = (this.clone(), id.clone());
                async move {
                    let supervisor = this.upgrade()?;
                    supervisor.run_check(&id).await.ok()?.state
                }
            })
            .await;
            if let Some(supervisor) = this.upgrade() {
                lock(&supervisor.retrying).remove(&id);
            }
        });
    }

    fn gate(&self, model_id: &str) -> Arc<tokio::sync::Mutex<()>> {
        Arc::clone(lock(&self.gates).entry(model_id.to_string()).or_default())
    }

    /// Moves `model_id`'s watch to its parent directory, or the nearest
    /// existing ancestor, if that is no longer the directory watched.
    /// Returns whether it moved: a Model whose watch moved is checked again,
    /// since its directory may have changed before the new watch existed.
    fn follow_parent(&self, model_id: &str) -> bool {
        let mut watches = lock(&self.watches);
        !watches.stopped && self.move_watch(&mut watches, model_id)
    }

    fn move_watch(&self, watches: &mut Watches, model_id: &str) -> bool {
        let Some(registration) = watches.models.get(model_id) else {
            return false;
        };
        let target = nearest_existing_directory(&registration.parent);
        if target == registration.watched {
            return false;
        }
        let old = registration.watched.clone();
        self.acquire(watches, &target);
        self.release(watches, &old);
        if let Some(registration) = watches.models.get_mut(model_id) {
            registration.watched = target;
        }
        true
    }

    /// Re-creates the watch on `directory` after an event on the directory
    /// itself, or moves its Models to an ancestor if it is gone. A polled
    /// directory that still exists needs nothing: polling re-reads it.
    fn reestablish(&self, watches: &mut Watches, directory: &Path) {
        if !directory.is_dir() {
            let moving: Vec<String> = watches
                .models
                .iter()
                .filter(|(_, registration)| registration.watched == directory)
                .map(|(id, _)| id.clone())
                .collect();
            for id in moving {
                self.move_watch(watches, &id);
            }
            return;
        }
        let Some(backend) = watches
            .directories
            .get(directory)
            .map(|entry| entry.backend)
        else {
            return;
        };
        if backend != Backend::Native {
            return;
        }
        remove_watch(watches, directory, backend);
        let (backend, mode) = self.add_watch(watches, directory);
        if let Some(entry) = watches.directories.get_mut(directory) {
            entry.backend = backend;
            entry.mode = mode;
        }
    }

    /// Takes a reference on `directory`'s watch, adding it if it's new.
    fn acquire(&self, watches: &mut Watches, directory: &Path) -> WatchMode {
        if let Some(entry) = watches.directories.get_mut(directory) {
            entry.refcount += 1;
            return entry.mode;
        }
        let (backend, mode) = self.add_watch(watches, directory);
        watches.directories.insert(
            directory.to_path_buf(),
            WatchEntry {
                refcount: 1,
                mode,
                backend,
            },
        );
        mode
    }

    /// Drops a reference on `directory`'s watch, removing it at zero.
    fn release(&self, watches: &mut Watches, directory: &Path) {
        let Some(entry) = watches.directories.get_mut(directory) else {
            return;
        };
        entry.refcount -= 1;
        if entry.refcount == 0 {
            let backend = entry.backend;
            watches.directories.remove(directory);
            remove_watch(watches, directory, backend);
        }
    }

    /// Watches `directory` non-recursively: natively when the policy allows
    /// and the watch registers, otherwise by polling.
    fn add_watch(&self, watches: &mut Watches, directory: &Path) -> (Backend, WatchMode) {
        let (debounce, interval) = match self.policy {
            WatchPolicy::Native { debounce } => {
                let fail = self.fail_next_native_watch.swap(false, Ordering::SeqCst);
                if !fail && self.add_native_watch(watches, directory, debounce) {
                    return (Backend::Native, WatchMode::Watching);
                }
                (debounce, POLL_FALLBACK_INTERVAL)
            }
            WatchPolicy::PollOnly { interval } => (NATIVE_DEBOUNCE, interval),
        };
        if watches.poll.is_none() {
            watches.poll = new_debouncer_opt::<_, PollWatcher, NoCache>(
                debounce,
                None,
                self.forwarder(),
                NoCache,
                notify::Config::default().with_poll_interval(interval),
            )
            .ok();
        }
        let polled = watches
            .poll
            .as_mut()
            .is_some_and(|poll| poll.watch(directory, RecursiveMode::NonRecursive).is_ok());
        if polled {
            (Backend::Poll, WatchMode::Polling)
        } else {
            (Backend::None, WatchMode::NotWatched)
        }
    }

    fn add_native_watch(
        &self,
        watches: &mut Watches,
        directory: &Path,
        debounce: Duration,
    ) -> bool {
        if watches.native.is_none() {
            watches.native = new_debouncer(debounce, None, self.forwarder()).ok();
        }
        watches
            .native
            .as_mut()
            .is_some_and(|native| native.watch(directory, RecursiveMode::NonRecursive).is_ok())
    }

    /// The debouncer's handler, on its own thread: forwards each batch to
    /// the supervisor's task.
    fn forwarder(&self) -> impl FnMut(DebounceEventResult) + Send + 'static {
        let signals = self.signals.clone();
        move |result: DebounceEventResult| {
            if let Some(signal) = signal_for(result) {
                let _ = signals.send(signal);
            }
        }
    }
}

/// What a debounced batch asks of the supervisor, if anything. Reads (an
/// open, or a close after reading) are not changes: a check's own copy of
/// the source would otherwise set off the next check. A close after
/// writing is kept. A rescan (the OS dropped events) or a watcher error
/// means changes may have been missed, so everything is checked.
fn signal_for(result: DebounceEventResult) -> Option<Signal> {
    let batch = match result {
        Ok(batch) => batch,
        Err(_) => return Some(Signal::Everything),
    };
    if batch.iter().any(|event| event.need_rescan()) {
        return Some(Signal::Everything);
    }
    let mut paths: Vec<PathBuf> = Vec::new();
    for event in batch {
        let read = matches!(
            event.kind,
            EventKind::Access(access) if access != AccessKind::Close(AccessMode::Write)
        );
        if read {
            continue;
        }
        for path in event.event.paths {
            if !paths.contains(&path) {
                paths.push(path);
            }
        }
    }
    (!paths.is_empty()).then_some(Signal::Paths(paths))
}

impl<R: Runtime> Drop for LinkSupervisor<R> {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn remove_watch(watches: &mut Watches, directory: &Path, backend: Backend) {
    // A watch the OS already dropped (its directory was removed) is gone
    // either way.
    let _ = match backend {
        Backend::Native => watches
            .native
            .as_mut()
            .map(|native| native.unwatch(directory)),
        Backend::Poll => watches.poll.as_mut().map(|poll| poll.unwatch(directory)),
        Backend::None => None,
    };
}

/// `path` if it is a directory, else its nearest ancestor that is.
fn nearest_existing_directory(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|ancestor| ancestor.is_dir())
        .unwrap_or(path)
        .to_path_buf()
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    // No map here holds an invariant a panicking holder could break.
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::repository::{self, LinkObservation, NewModel, RevisionSource};
    use crate::library::selection::CancelledModelFileIo;
    use crate::library::{new_id, RevisionOrigin, StorageMode};
    use std::sync::atomic::AtomicUsize;
    use std::time::{SystemTime, UNIX_EPOCH};
    use tauri::test::MockRuntime;

    const CUBE_BINARY: &[u8] = include_bytes!("../../tests/fixtures/library/cube-binary.stl");
    const CUBE_ASCII: &[u8] = include_bytes!("../../tests/fixtures/library/cube-ascii.stl");
    const TRUNCATED: &[u8] = include_bytes!("../../tests/fixtures/library/truncated-binary.stl");
    const CORE_3MF: &[u8] = include_bytes!("../../tests/fixtures/library/core-two-objects.3mf");

    /// One in-memory source.
    #[derive(Clone)]
    enum FakeEntry {
        File {
            bytes: Vec<u8>,
            stat: SourceStat,
        },
        Directory,
        Error(io::ErrorKind),
        /// A file that is always being written: staging it fails.
        Changing {
            stat: SourceStat,
        },
    }

    /// An in-memory [`LinkFs`] that counts how often it is read.
    struct FakeFs {
        entries: Mutex<HashMap<PathBuf, FakeEntry>>,
        stages: AtomicUsize,
    }

    impl FakeFs {
        fn new() -> Self {
            Self {
                entries: Mutex::new(HashMap::new()),
                stages: AtomicUsize::new(0),
            }
        }

        fn set(&self, path: &Path, entry: FakeEntry) {
            lock(&self.entries).insert(path.to_path_buf(), entry);
        }

        fn stages(&self) -> usize {
            self.stages.load(Ordering::SeqCst)
        }
    }

    impl LinkFs for FakeFs {
        fn metadata(&self, path: &Path) -> io::Result<LinkStat> {
            match lock(&self.entries).get(path) {
                None => Err(io::ErrorKind::NotFound.into()),
                Some(FakeEntry::File { stat, .. }) | Some(FakeEntry::Changing { stat }) => {
                    Ok(LinkStat {
                        kind: FileKind::File,
                        stat: stat.clone(),
                    })
                }
                Some(FakeEntry::Directory) => Ok(LinkStat {
                    kind: FileKind::Directory,
                    stat: stat(0, 1),
                }),
                Some(FakeEntry::Error(kind)) => Err((*kind).into()),
            }
        }

        fn stage(
            &self,
            content: &ContentStore,
            path: &Path,
            staging_key: &str,
        ) -> Result<StagedFile, ContentError> {
            self.stages.fetch_add(1, Ordering::SeqCst);
            match lock(&self.entries).get(path).cloned() {
                Some(FakeEntry::File { bytes, stat }) => {
                    let mut staged = content.stage_bytes(&bytes, staging_key, "0.src")?;
                    staged.source = Some(stat);
                    Ok(staged)
                }
                Some(FakeEntry::Changing { .. }) => Err(ContentError::ChangedDuringRead),
                Some(FakeEntry::Directory) => Err(ContentError::NotAFile),
                Some(FakeEntry::Error(kind)) => Err(ContentError::Unreadable(kind)),
                None => Err(ContentError::Unreadable(io::ErrorKind::NotFound)),
            }
        }
    }

    fn stat(size: u64, mtime_secs: u64) -> SourceStat {
        SourceStat {
            size,
            modified: Some(UNIX_EPOCH + Duration::from_secs(mtime_secs)),
            file_id: Some("7:42".to_string()),
        }
    }

    struct Fixture {
        _temp: tempfile::TempDir,
        _lease: crate::persistence::MetadataRootLease,
        storage: Arc<Storage>,
        library: LibraryServices<MockRuntime>,
        fs: FakeFs,
        path: PathBuf,
        model_id: String,
    }

    impl Fixture {
        /// A linked STL Model whose one revision holds `cube-binary.stl`,
        /// observed at `stat(684, 100)`, with the same bytes at its path.
        fn new() -> Self {
            let (temp, lease, storage) = crate::test_storage();
            let content =
                Arc::new(ContentStore::open(storage.paths().content_root()).expect("content"));
            let library = LibraryServices::new(content, Arc::new(CancelledModelFileIo));
            let path = PathBuf::from("/sources/cube.stl");
            let observed = stat(CUBE_BINARY.len() as u64, 100);
            let model_id = seed_linked(&storage, &library.content, &path, &observed);
            let fs = FakeFs::new();
            fs.set(
                &path,
                FakeEntry::File {
                    bytes: CUBE_BINARY.to_vec(),
                    stat: observed,
                },
            );
            Self {
                _temp: temp,
                _lease: lease,
                storage,
                library,
                fs,
                path,
                model_id,
            }
        }

        fn snapshot(&self) -> LinkSnapshot {
            self.storage
                .read(|connection| Ok(LinkSnapshot::load(connection, &self.model_id)))
                .expect("read")
                .expect("load")
                .expect("a linked Model")
        }

        fn check(&self) -> CheckOutcome {
            check_linked_source(
                &self.fs,
                &self.library.content,
                &self.snapshot(),
                "lnk-test",
            )
            .expect("check")
        }

        fn check_and_apply(&self) -> Applied {
            let link = self.snapshot();
            let outcome = check_linked_source(&self.fs, &self.library.content, &link, "lnk-test")
                .expect("check");
            let applied =
                apply_outcome(&self.library, &self.storage, &link, outcome).expect("apply");
            self.library.content.discard_staging("lnk-test");
            applied
        }

        fn revisions(&self) -> Vec<crate::library::ModelSourceRevisionRecord> {
            self.storage
                .read(|connection| {
                    Ok(repository::list_revision_records(
                        connection,
                        &self.model_id,
                    ))
                })
                .expect("read")
                .expect("revisions")
        }

        fn stored(&self) -> crate::library::StoredModel {
            self.storage
                .read(|connection| Ok(repository::load_model(connection, &self.model_id)))
                .expect("read")
                .expect("load")
                .expect("model")
        }

        fn blob_files(&self) -> usize {
            count_files(&self.storage.paths().content_root().join("blobs"))
        }

        fn staging_files(&self) -> usize {
            count_files(&self.storage.paths().content_root().join("staging"))
        }
    }

    fn count_files(root: &Path) -> usize {
        let Ok(entries) = fs::read_dir(root) else {
            return 0;
        };
        entries
            .flatten()
            .map(|entry| {
                let path = entry.path();
                if path.is_dir() {
                    count_files(&path)
                } else {
                    1
                }
            })
            .sum()
    }

    fn seed_linked(
        storage: &Storage,
        content: &ContentStore,
        path: &Path,
        observed: &SourceStat,
    ) -> String {
        let mut staged = content
            .stage_bytes(CUBE_BINARY, "seed", "0.src")
            .expect("stage");
        staged.source = Some(observed.clone());
        let detected = formats::detect_named(&staged.path, "cube.stl").expect("detect");
        let outcome =
            formats::inspect(&staged.path, &detected, &CancelFlag::never()).expect("inspect");
        let id = new_id("mdl");
        let path_text = path.to_str().expect("utf-8");
        content
            .place_and_commit(storage, &[&staged], |tx| {
                repository::insert_model(
                    tx,
                    &NewModel {
                        id: &id,
                        name: "Cube",
                        format: ModelFormat::Stl,
                        link: Some(LinkObservation {
                            path: path_text,
                            stat: Some(observed),
                        }),
                    },
                )?;
                repository::insert_revision(
                    tx,
                    &id,
                    &staged,
                    &outcome,
                    RevisionOrigin::Import,
                    &RevisionSource {
                        path: path_text,
                        file_name: "cube.stl",
                        mtime: None,
                    },
                )?;
                Ok(())
            })
            .expect("seed");
        id
    }

    // 1. An unchanged stat with state `ok` does no hashing.
    #[test]
    fn an_unchanged_stat_with_state_ok_reads_nothing() {
        let fixture = Fixture::new();
        assert!(matches!(fixture.check(), CheckOutcome::Unchanged));
        assert_eq!(fixture.fs.stages(), 0, "the unchanged case costs one stat");

        let applied = fixture.check_and_apply();
        assert_eq!(applied.state, Some(SourceState::Ok));
        assert!(applied.record.is_none(), "nothing was written");
        assert_eq!(fixture.stored().revision, 1);
    }

    // 2. A changed mtime with the same hash updates only the observed stat.
    #[test]
    fn a_changed_mtime_with_the_same_hash_updates_the_stat_and_bumps_the_model() {
        let fixture = Fixture::new();
        let touched = stat(CUBE_BINARY.len() as u64, 200);
        fixture.fs.set(
            &fixture.path,
            FakeEntry::File {
                bytes: CUBE_BINARY.to_vec(),
                stat: touched.clone(),
            },
        );

        let applied = fixture.check_and_apply();

        assert_eq!(
            fixture.fs.stages(),
            1,
            "a stat change is settled by the hash"
        );
        assert_eq!(applied.state, Some(SourceState::Ok));
        assert!(applied.revision.is_none(), "no revision for the same bytes");
        let record = applied.record.expect("the observed stat was written");
        assert_eq!(record.revision, 2, "the Model's revision is bumped");
        assert_eq!(record.revision_count, 1);
        let stored = fixture.stored();
        assert_eq!(stored.link_observed_mtime_ns, touched.modified_ns());
        assert_eq!(stored.link_state, Some(SourceState::Ok));
        assert_eq!(fixture.revisions().len(), 1);
    }

    // 3. Changed content creates revision 2 with origin `linkedChange`.
    #[test]
    fn changed_content_adds_a_linked_change_revision_and_keeps_revision_1() {
        let fixture = Fixture::new();
        let before = fixture.revisions();
        let edited = stat(CUBE_ASCII.len() as u64, 300);
        fixture.fs.set(
            &fixture.path,
            FakeEntry::File {
                bytes: CUBE_ASCII.to_vec(),
                stat: edited.clone(),
            },
        );

        let applied = fixture.check_and_apply();

        let revision = applied.revision.expect("a new revision");
        assert_eq!(revision.sequence, 2);
        assert_eq!(revision.origin, RevisionOrigin::LinkedChange);
        assert_eq!(revision.source_file_name, "cube.stl");
        let record = applied.record.expect("record");
        assert_eq!(record.current_revision.id, revision.id);
        assert_eq!(record.revision_count, 2);
        assert_eq!(record.link.as_ref().unwrap().state, SourceState::Ok);
        let after = fixture.revisions();
        assert_eq!(after.len(), 2);
        assert_eq!(after[1], before[0], "revision 1 is untouched");
        assert_eq!(
            fixture.stored().link_observed_mtime_ns,
            edited.modified_ns()
        );
        assert_eq!(fixture.staging_files(), 0);
    }

    // 4. NotFound, PermissionDenied, and a directory.
    #[test]
    fn missing_unreadable_and_directory_sources_map_to_their_states() {
        let fixture = Fixture::new();
        for (entry, expected) in [
            (
                FakeEntry::Error(io::ErrorKind::NotFound),
                SourceState::Missing,
            ),
            (
                FakeEntry::Error(io::ErrorKind::PermissionDenied),
                SourceState::Unreadable,
            ),
            (FakeEntry::Directory, SourceState::NotAFile),
        ] {
            fixture.fs.set(&fixture.path, entry);
            match fixture.check() {
                CheckOutcome::State { state, .. } => assert_eq!(state, expected),
                other => panic!("expected {expected:?}, got {other:?}"),
            }
        }
        assert_eq!(fixture.fs.stages(), 0);

        fixture
            .fs
            .set(&fixture.path, FakeEntry::Error(io::ErrorKind::NotFound));
        let applied = fixture.check_and_apply();
        assert_eq!(applied.state, Some(SourceState::Missing));
        let record = applied.record.expect("the state change was written");
        assert_eq!(record.link.unwrap().state, SourceState::Missing);
        assert_eq!(record.revision, 2);

        let again = fixture.check_and_apply();
        assert_eq!(again.state, Some(SourceState::Missing));
        assert!(again.record.is_none(), "an unchanged state writes nothing");
    }

    // 5. Changed content that fails inspection is `invalidContent`.
    #[test]
    fn content_that_fails_inspection_is_invalid_and_leaves_nothing_behind() {
        let fixture = Fixture::new();
        let blobs_before = fixture.blob_files();
        for bytes in [TRUNCATED, CORE_3MF] {
            fixture.fs.set(
                &fixture.path,
                FakeEntry::File {
                    bytes: bytes.to_vec(),
                    stat: stat(bytes.len() as u64, 400),
                },
            );
            let applied = fixture.check_and_apply();
            assert_eq!(applied.state, Some(SourceState::InvalidContent));
            assert!(applied.revision.is_none());
            assert_eq!(fixture.staging_files(), 0, "the staged copy is gone");
        }
        assert_eq!(fixture.blob_files(), blobs_before, "no blob was placed");
        let revisions = fixture.revisions();
        assert_eq!(revisions.len(), 1, "the current revision stays current");
        assert_eq!(
            fixture.stored().link_state,
            Some(SourceState::InvalidContent)
        );
    }

    // 6. Content that changes during the copy is `changing`, retried at
    // 2, 4, and 8 s, then left alone.
    #[test]
    fn a_source_changing_during_the_copy_is_changing() {
        let fixture = Fixture::new();
        fixture.fs.set(
            &fixture.path,
            FakeEntry::Changing {
                stat: stat(999, 500),
            },
        );
        match fixture.check() {
            CheckOutcome::State { state, .. } => assert_eq!(state, SourceState::Changing),
            other => panic!("expected changing, got {other:?}"),
        }
        assert_eq!(fixture.staging_files(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn a_changing_source_is_retried_at_2_4_and_8_seconds_then_left() {
        let fixture = Fixture::new();
        fixture.fs.set(
            &fixture.path,
            FakeEntry::Changing {
                stat: stat(999, 500),
            },
        );
        let start = tokio::time::Instant::now();
        let at = Arc::new(Mutex::new(Vec::new()));
        let fixture = Arc::new(fixture);
        let checks = {
            let (at, fixture) = (Arc::clone(&at), Arc::clone(&fixture));
            move || {
                let (at, fixture) = (Arc::clone(&at), Arc::clone(&fixture));
                async move {
                    lock(&at).push(start.elapsed());
                    Some(fixture.check_and_apply().state.expect("linked"))
                }
            }
        };
        retry_while_changing(checks).await;
        tokio::time::sleep(Duration::from_secs(60)).await;

        assert_eq!(
            *lock(&at),
            [2, 6, 14].map(Duration::from_secs).to_vec(),
            "one retry after each of 2, 4, and 8 s, then none"
        );
        assert_eq!(fixture.fs.stages(), 3);
        assert_eq!(fixture.stored().link_state, Some(SourceState::Changing));
    }

    #[tokio::test(start_paused = true)]
    async fn retries_stop_once_the_source_settles() {
        let calls = Arc::new(Mutex::new(0));
        let counted = Arc::clone(&calls);
        retry_while_changing(move || {
            let counted = Arc::clone(&counted);
            async move {
                *lock(&counted) += 1;
                Some(SourceState::Ok)
            }
        })
        .await;
        assert_eq!(*lock(&calls), 1);
    }

    /// Reading a source (a check's own copy, or anyone listing its folder)
    /// is not a change: without this filter a check would trigger the next
    /// one, and a source in a state that re-reads it (`invalidContent`,
    /// `unreadable`) would be re-hashed forever.
    #[test]
    fn reads_are_not_changes_but_writes_and_rescans_are() {
        use notify::event::{AccessKind, AccessMode, CreateKind, Flag};
        use notify::{Event, EventKind};
        use notify_debouncer_full::DebouncedEvent;

        let event = |kind: EventKind| {
            DebouncedEvent::new(
                Event::new(kind).add_path(PathBuf::from("/sources/cube.stl")),
                std::time::Instant::now(),
            )
        };
        let reads = vec![
            event(EventKind::Access(AccessKind::Open(AccessMode::Any))),
            event(EventKind::Access(AccessKind::Close(AccessMode::Read))),
        ];
        assert_eq!(signal_for(Ok(reads.clone())), None);

        let mut written = reads;
        written.push(event(EventKind::Access(AccessKind::Close(
            AccessMode::Write,
        ))));
        assert_eq!(
            signal_for(Ok(written)),
            Some(Signal::Paths(vec![PathBuf::from("/sources/cube.stl")]))
        );
        assert_eq!(
            signal_for(Ok(vec![event(EventKind::Create(CreateKind::File))])),
            Some(Signal::Paths(vec![PathBuf::from("/sources/cube.stl")]))
        );

        let rescan = DebouncedEvent::new(
            Event::new(EventKind::Other).set_flag(Flag::Rescan),
            std::time::Instant::now(),
        );
        assert_eq!(signal_for(Ok(vec![rescan])), Some(Signal::Everything));
        assert_eq!(signal_for(Err(Vec::new())), Some(Signal::Everything));
    }

    #[test]
    fn a_managed_model_has_no_link_snapshot() {
        let fixture = Fixture::new();
        fixture
            .storage
            .write(|tx| {
                tx.execute(
                    "UPDATE library_models SET storage_mode = 'managed', linked_path = NULL,
                         link_state = NULL WHERE id = ?1",
                    [&fixture.model_id],
                )?;
                Ok(())
            })
            .unwrap();
        let snapshot = fixture
            .storage
            .read(|connection| Ok(LinkSnapshot::load(connection, &fixture.model_id)))
            .unwrap()
            .unwrap();
        assert_eq!(snapshot, None);
        assert_eq!(fixture.stored().storage_mode, StorageMode::Managed);
        let _ = SystemTime::now();
    }
}
