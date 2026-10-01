//! D15 tier (c): the installer's `reset` journal kind (spec "Tier (c),
//! entire Farm" and the reset rows of "Installer fault points").
//!
//! A reset only rolls forward. Every step records itself in `journal.step`
//! before it starts and is idempotent, so a crash resumes at the recorded
//! step and an I/O error stops startup with `RESTORE_FAILED`
//! (`installFailed`) and the retry resumes at the same step:
//!
//! 1. `markInstalling` (phase `installing`);
//! 2. `moveDatabaseAside` (as a restore; each file moved only if still
//!    present);
//! 3. `moveRootsAside`: `content_root`, `media_root`, and `log_root` are
//!    each renamed to a sibling `<parent>/.aside-<journalId>` (a sibling
//!    stays on the same filesystem), and the contents of `snapshot_root`
//!    and `legacy_root` move into `restore/<id>/previous/`. A root whose
//!    aside exists was already moved; if the root exists again it is the
//!    empty directory this step (or `StoragePaths`) recreated, and is left
//!    alone. A missing root is recreated with user-only permissions;
//! 4. `createFreshDatabase`: any database set in the metadata root can only
//!    be a partial fresh one, so it is deleted; then a new database is
//!    opened, migrated, and closed;
//! 5. `deleteCredentials`: each journaled ref is deleted from the
//!    credential store (an absent ref counts as deleted); every failure, or
//!    an unavailable store, is queued in the fresh database's
//!    `pending_credential_cleanup` with reason `reset`. The step never fails
//!    for a credential;
//! 6. `deleteSafetyBackups`, only when the operator ticked it: every file
//!    in `<backup_root>/safety/` except this reset's own safety backup;
//! 7. `removePrevious`: `restore/<id>/previous/` and every
//!    `.aside-<journalId>` directory;
//! 8. `markDone`.
//!
//! The journal holds credential refs only, never a value.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OpenFlags};

use super::{
    exists, move_database_aside, now, remove_dir_if_present, remove_file_if_present, FaultPoint,
    Faults, InstallOutcome, InstallReport, InstallerError, StepError, DATABASE_FILES,
    ROLLBACK_JOURNAL_FILE,
};
use crate::backup::dialogs::BACKUP_EXTENSION;
use crate::backup::journal::{self, sync_directory, JournalPhase, Outcome, RestoreJournal};
use crate::backup::InstallerStep;
use crate::connections::credentials::CredentialStore;
use crate::persistence::{apply_migrations, StoragePaths};

/// A root moved aside is renamed to `<parent>/.aside-<journalId>`.
pub const ASIDE_PREFIX: &str = ".aside-";
/// Where `moveRootsAside` puts the contents of `snapshot_root` and
/// `legacy_root`, below `restore/<id>/previous/`.
const PREVIOUS_SNAPSHOTS: &str = "snapshots";
const PREVIOUS_LEGACY: &str = "legacy";

/// The steps after `markInstalling`, in the order they run.
const STEPS: [InstallerStep; 6] = [
    InstallerStep::MoveDatabaseAside,
    InstallerStep::MoveRootsAside,
    InstallerStep::CreateFreshDatabase,
    InstallerStep::DeleteCredentials,
    InstallerStep::DeleteSafetyBackups,
    InstallerStep::RemovePrevious,
];

/// The three roots a reset renames aside, in the order it moves them.
pub fn moved_roots(paths: &StoragePaths) -> [&Path; 3] {
    [paths.content_root(), paths.media_root(), paths.log_root()]
}

/// `<parent of root>/.aside-<journalId>-<root name>`, or `None` for a root
/// with no parent or name (never a `StoragePaths` root). The root's own name
/// keeps two roots under one parent from sharing an aside.
pub fn aside_of(root: &Path, journal_id: &str) -> Option<PathBuf> {
    let name = root.file_name()?.to_string_lossy();
    root.parent()
        .map(|parent| parent.join(format!("{ASIDE_PREFIX}{journal_id}-{name}")))
}

/// A reset journal's roll-forward.
pub(super) struct Reset<'a, F> {
    paths: &'a StoragePaths,
    faults: &'a Faults,
    journal: RestoreJournal,
    /// Opened only by `deleteCredentials`, after the metadata-root lease.
    open_credentials: Option<F>,
}

impl<'a, F: FnOnce() -> CredentialStore> Reset<'a, F> {
    pub(super) fn new(
        paths: &'a StoragePaths,
        faults: &'a Faults,
        journal: RestoreJournal,
        open_credentials: F,
    ) -> Self {
        Self {
            paths,
            faults,
            journal,
            open_credentials: Some(open_credentials),
        }
    }

    pub(super) fn run(mut self) -> Result<InstallReport, InstallerError> {
        match self.install() {
            Ok(()) => Ok(InstallReport::of(
                Some(&self.journal),
                InstallOutcome::Installed,
            )),
            Err(StepError::Crash) => Err(InstallerError::Crashed),
            // The journal still names the step, so the retry resumes there.
            Err(_) => Err(InstallerError::InstallFailed {
                step: self.journal.step,
            }),
        }
    }

    fn fault(&self, step: InstallerStep, point: FaultPoint) -> Result<(), StepError> {
        self.faults.hit(step, point)
    }

    fn save(&self) -> Result<(), StepError> {
        journal::write(self.paths, &self.journal).map_err(StepError::from)
    }

    /// Records `step` in the journal before it starts.
    fn record(&mut self, step: InstallerStep) -> Result<(), StepError> {
        self.journal.step = Some(step);
        self.save()?;
        self.fault(step, FaultPoint::Start)
    }

    fn delete_safety_backups_ticked(&self) -> bool {
        self.journal
            .reset
            .is_some_and(|options| options.delete_safety_backups)
    }

    fn install(&mut self) -> Result<(), StepError> {
        // 1. markInstalling.
        if self.journal.phase == JournalPhase::Pending {
            self.journal
                .set_phase(JournalPhase::Installing)
                .map_err(|_| StepError::Io(io::ErrorKind::InvalidData))?;
            self.journal.attempts += 1;
            self.journal.step = Some(InstallerStep::MarkInstalling);
            self.save()?;
            self.fault(InstallerStep::MarkInstalling, FaultPoint::End)?;
        }
        if self.journal.phase != JournalPhase::Installing {
            return Err(StepError::Io(io::ErrorKind::InvalidData));
        }
        // 2–7, resuming at the recorded step.
        let resume = match self.journal.step {
            None | Some(InstallerStep::MarkInstalling) => 0,
            Some(step) => STEPS
                .iter()
                .position(|candidate| *candidate == step)
                .ok_or(StepError::Io(io::ErrorKind::InvalidData))?,
        };
        for step in STEPS[resume..].iter().copied() {
            if step == InstallerStep::DeleteSafetyBackups && !self.delete_safety_backups_ticked() {
                continue;
            }
            self.record(step)?;
            match step {
                InstallerStep::MoveDatabaseAside => {
                    move_database_aside(self.paths, &self.journal.id, self.faults)?;
                }
                InstallerStep::MoveRootsAside => self.move_roots_aside()?,
                InstallerStep::CreateFreshDatabase => self.create_fresh_database()?,
                InstallerStep::DeleteCredentials => self.delete_credentials()?,
                InstallerStep::DeleteSafetyBackups => self.delete_safety_backups()?,
                InstallerStep::RemovePrevious => self.remove_previous()?,
                _ => return Err(StepError::Io(io::ErrorKind::InvalidData)),
            }
            self.fault(step, FaultPoint::End)?;
        }
        // 8. markDone.
        self.journal
            .set_phase(JournalPhase::Done)
            .map_err(|_| StepError::Io(io::ErrorKind::InvalidData))?;
        self.journal.step = Some(InstallerStep::MarkDone);
        self.journal.outcome = Some(Outcome { finished_at: now() });
        self.save()?;
        self.fault(InstallerStep::MarkDone, FaultPoint::End)
    }

    /// Step 3. `Within(0)` fires after `content_root` is handled.
    fn move_roots_aside(&mut self) -> Result<(), StepError> {
        let id = self.journal.id.clone();
        for (index, root) in moved_roots(self.paths).into_iter().enumerate() {
            let parent = root
                .parent()
                .ok_or(StepError::Io(io::ErrorKind::InvalidInput))?;
            let name = root
                .file_name()
                .ok_or(StepError::Io(io::ErrorKind::InvalidInput))?;
            let aside = aside_of(root, &id).ok_or(StepError::Io(io::ErrorKind::InvalidInput))?;
            // An existing aside means this root was already moved; a root
            // that exists again is the empty directory recreated since.
            if !exists(&aside) && exists(root) {
                fs::rename(root, &aside)?;
                sync_directory(parent)?;
            }
            if !exists(root) {
                journal::create_synced_dir(parent, Path::new(name))?;
            }
            if index == 0 {
                self.fault(InstallerStep::MoveRootsAside, FaultPoint::Within(0))?;
            }
        }
        for (root, below) in [
            (self.paths.snapshot_root(), PREVIOUS_SNAPSHOTS),
            (self.paths.legacy_root(), PREVIOUS_LEGACY),
        ] {
            let (target, _) = journal::create_synced_dir(
                self.paths.metadata_root(),
                &Path::new("restore").join(&id).join("previous").join(below),
            )?;
            let entries = match fs::read_dir(root) {
                Ok(entries) => entries,
                Err(error) if error.kind() == io::ErrorKind::NotFound => {
                    journal::create_synced_dir(
                        root.parent()
                            .ok_or(StepError::Io(io::ErrorKind::InvalidInput))?,
                        Path::new(
                            root.file_name()
                                .ok_or(StepError::Io(io::ErrorKind::InvalidInput))?,
                        ),
                    )?;
                    continue;
                }
                Err(error) => return Err(error.into()),
            };
            for entry in entries {
                let entry = entry?;
                fs::rename(entry.path(), target.join(entry.file_name()))?;
            }
            sync_directory(root)?;
            sync_directory(&target)?;
        }
        Ok(())
    }

    /// Step 4. `Within(0)` fires with the new database open, before its
    /// first commit (the migrations).
    fn create_fresh_database(&mut self) -> Result<(), StepError> {
        let root = self.paths.metadata_root();
        for name in DATABASE_FILES.iter().chain([ROLLBACK_JOURNAL_FILE].iter()) {
            remove_file_if_present(&root.join(name))?;
        }
        sync_directory(root)?;
        create_private_file(self.paths.database())?;
        let mut connection = open_database(self.paths.database())?;
        self.fault(InstallerStep::CreateFreshDatabase, FaultPoint::Within(0))?;
        apply_migrations(&mut connection)?;
        connection
            .close()
            .map_err(|(_, error)| StepError::from(error))?;
        File::open(self.paths.database())?.sync_all()?;
        sync_directory(root)?;
        Ok(())
    }

    /// Step 5. `Within(0)` fires after half the refs are deleted.
    fn delete_credentials(&mut self) -> Result<(), StepError> {
        let refs = self.journal.orphan_credential_refs.clone();
        if refs.is_empty() {
            return Ok(());
        }
        let store = self.open_credentials.take().map(|open| open());
        let half = refs.len() / 2;
        let mut failed = Vec::new();
        for (index, reference) in refs.iter().enumerate() {
            if index == half && half > 0 {
                self.fault(InstallerStep::DeleteCredentials, FaultPoint::Within(0))?;
            }
            // An absent ref counts as deleted (the store's own rule).
            let deleted = store
                .as_ref()
                .is_some_and(|store| store.delete(reference).is_ok());
            if !deleted {
                failed.push(reference.clone());
            }
        }
        if failed.is_empty() {
            return Ok(());
        }
        // Queued with reason `reset`, which F1's startup retry treats as
        // automatically eligible.
        let mut connection = open_database(self.paths.database())?;
        let transaction = connection.transaction()?;
        for reference in &failed {
            crate::printers::repository::enqueue_credential_cleanup(
                &transaction,
                reference,
                None,
                "reset",
            )?;
        }
        transaction.commit()?;
        connection
            .close()
            .map_err(|(_, error)| StepError::from(error))?;
        Ok(())
    }

    /// Step 6. `Within(0)` fires after the first file is deleted.
    fn delete_safety_backups(&mut self) -> Result<(), StepError> {
        let root = self.paths.backup_root().join("safety");
        let keep = self
            .journal
            .safety_backup_id
            .as_ref()
            .map(|id| format!("{id}.{BACKUP_EXTENSION}"));
        let entries = match fs::read_dir(&root) {
            Ok(entries) => entries,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
            Err(error) => return Err(error.into()),
        };
        let mut deleted = 0_u32;
        for entry in entries {
            let entry = entry?;
            if entry.file_type()?.is_dir() {
                continue;
            }
            if keep.as_deref() == entry.file_name().to_str() {
                continue;
            }
            remove_file_if_present(&entry.path())?;
            deleted += 1;
            if deleted == 1 {
                self.fault(InstallerStep::DeleteSafetyBackups, FaultPoint::Within(0))?;
            }
        }
        sync_directory(&root)?;
        Ok(())
    }

    /// Step 7. `Within(0)` fires after `previous/` is deleted.
    fn remove_previous(&mut self) -> Result<(), StepError> {
        remove_dir_if_present(&journal::previous_dir(self.paths, &self.journal.id))?;
        self.fault(InstallerStep::RemovePrevious, FaultPoint::Within(0))?;
        for root in moved_roots(self.paths) {
            if let (Some(aside), Some(parent)) = (aside_of(root, &self.journal.id), root.parent()) {
                remove_dir_if_present(&aside)?;
                sync_directory(parent)?;
            }
        }
        Ok(())
    }
}

/// Creates `path` readable by this user only (as `Storage::open` does).
fn create_private_file(path: &Path) -> io::Result<()> {
    let mut options = OpenOptions::new();
    options.write(true).create(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options
            .mode(0o600)
            .custom_flags(rustix::fs::OFlags::NOFOLLOW.bits() as i32);
    }
    options.open(path).map(drop)
}

/// Opens the database as `Storage::open` would: WAL, `synchronous = FULL`,
/// foreign keys on.
fn open_database(path: &Path) -> Result<Connection, StepError> {
    let connection = Connection::open_with_flags(
        path,
        OpenFlags::SQLITE_OPEN_READ_WRITE
            | OpenFlags::SQLITE_OPEN_NO_MUTEX
            | OpenFlags::SQLITE_OPEN_NOFOLLOW,
    )?;
    connection.busy_timeout(Duration::from_secs(5))?;
    connection.pragma_update(None, "foreign_keys", "ON")?;
    let mode: String = connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
    if !mode.eq_ignore_ascii_case("wal") {
        return Err(StepError::Io(io::ErrorKind::Unsupported));
    }
    connection.pragma_update(None, "synchronous", "FULL")?;
    Ok(connection)
}
