//! The startup installer (spec D8 "Startup order", "Installer steps
//! (restore)", "Rollback and retry (restore)"; ADR-0016).
//!
//! [`run`] runs after the metadata-root lease and before the log and
//! `Storage::open`. It reads the journal ([`super::journal`]) and, for a
//! restore, swaps the Farm, validates it, and either commits the swap or
//! rolls it back. Any crash is recovered by the same installer on the next
//! start. A crash or an error at any step ends either fully installed or
//! byte-exactly rolled back, never mixed:
//!
//! - Every step records itself in `journal.step` before it starts, so the
//!   next start knows what the files on disk are.
//! - `farm3d.sqlite3`, `-wal`, and `-shm` always move as a set: the WAL is
//!   never separated from its database. A partial move is put back before
//!   anything opens the database.
//! - Before `markInstalled`, any crash or error rolls back. The rollback is
//!   two-phase and idempotent: each phase's completion is durable in
//!   `journal.rollback` before the next starts, so a crash inside a
//!   rollback resumes it without redoing a destructive step, and phase (b)
//!   only ever deletes files that can only be the candidate.
//! - After `markInstalled`, a crash rolls forward.
//!
//! A reset journal (Task 8) is refused here until the reset steps exist.
//!
//! [`Fault`] injects a crash (the run returns at once, dropping every
//! handle, as a process death would) or an I/O error at a named point; it
//! is test-only (`#[doc(hidden)]`) and production passes none.

use std::collections::BTreeSet;
use std::fs::{self, File};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::{SecondsFormat, Utc};
use rusqlite::{params, Connection, OpenFlags, OptionalExtension};

use super::inventory::{hash_file, table_counts};
use super::journal::{
    self, sync_directory, Failure, JournalPhase, Outcome, RestoreJournal, SwapMedia,
};
use super::manifest::{is_sha256_hex, Manifest};
use super::staging::{self, StagingLayout};
use super::{BackupInvalidReason, InstallerStep, RestoreJournalKind};
use crate::connections::credentials::CredentialStore;
use crate::contracts::command::{CommandError, ErrorCode};
use crate::persistence::integrity::{self, IntegrityRoots};
use crate::persistence::{
    apply_migrations, has_foreign_key_violation, MetadataRootLease, StorageError, StoragePaths,
};

mod fault;
mod rollback;

#[doc(hidden)]
pub use fault::{Fault, FaultEffect, FaultPoint, Faults, RollbackPoint};

/// The live database set, in the order it always moves.
const DATABASE_FILES: [&str; 3] = ["farm3d.sqlite3", "farm3d.sqlite3-wal", "farm3d.sqlite3-shm"];
/// A rollback journal SQLite may leave beside the placed candidate while
/// it switches it to WAL; only ever the candidate's.
const ROLLBACK_JOURNAL_FILE: &str = "farm3d.sqlite3-journal";
/// `placeCandidate`'s same-directory copy.
const PARTIAL_FILE: &str = ".farm3d-restore-candidate.partial";
/// `restore-<stagingId>/previous-snapshots`: the live media moved aside.
const PREVIOUS_SNAPSHOTS: &str = "previous-snapshots";
const SNAPSHOTS: &str = "snapshots";
/// A restore gets two attempts.
const MAX_ATTEMPTS: u32 = 2;
/// How long a busy metadata-root lock is retried while a journal exists.
pub const LEASE_WAIT: Duration = Duration::from_secs(10);

// --- the report and the errors ---------------------------------------------------------

/// What a run did with the journal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum InstallOutcome {
    /// No journal: nothing to do.
    NoJournal,
    /// The journal was already `done` or `failed`.
    AlreadyFinished,
    /// The restore is installed (journal `done`).
    Installed,
    /// The restore failed (journal `failed`); the Farm is as it was.
    Failed {
        code: ErrorCode,
        step: Option<InstallerStep>,
    },
}

/// [`run`]'s result, logged at startup with typed fields only.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct InstallReport {
    pub journal_id: Option<String>,
    pub kind: Option<RestoreJournalKind>,
    pub outcome: InstallOutcome,
    /// The journal's `attempts` at the end of the run.
    pub attempts: u32,
    /// Whether this run rolled an install back.
    pub rolled_back: bool,
    /// Why a verification failed (`BACKUP_INVALID`).
    pub invalid_reason: Option<BackupInvalidReason>,
}

impl InstallReport {
    fn of(journal: Option<&RestoreJournal>, outcome: InstallOutcome) -> Self {
        Self {
            journal_id: journal.map(|journal| journal.id.clone()),
            kind: journal.map(|journal| journal.kind),
            outcome,
            attempts: journal.map_or(0, |journal| journal.attempts),
            rolled_back: false,
            invalid_reason: None,
        }
    }
}

/// Why startup stops (`RESTORE_FAILED`, the bootstrap `Failed` state).
#[derive(Debug)]
pub enum InstallerError {
    /// The journal can't be read or has another version; nothing touched.
    Journal(journal::JournalError),
    /// The install couldn't proceed (retryable): the journal is unchanged
    /// or rolls forward on the retry.
    InstallFailed { step: Option<InstallerStep> },
    /// A rollback failed; `journal.rollback` records how far it got, and
    /// the retry resumes it.
    RollbackFailed { step: InstallerStep },
    /// Test-only: an injected crash stopped the run.
    #[doc(hidden)]
    Crashed,
}

impl InstallerError {
    /// `RESTORE_FAILED` with D8's `details.reason` and `step`.
    pub fn to_command_error(&self) -> CommandError {
        match self {
            InstallerError::Journal(error) => CommandError::restore_failed(error.reason(), None),
            InstallerError::InstallFailed { step } => {
                CommandError::restore_failed("installFailed", *step)
            }
            InstallerError::RollbackFailed { step } => {
                CommandError::restore_failed("rollbackFailed", Some(*step))
            }
            InstallerError::Crashed => CommandError::restore_failed("installFailed", None),
        }
    }

    fn step(&self) -> Option<InstallerStep> {
        match self {
            InstallerError::InstallFailed { step } => *step,
            InstallerError::RollbackFailed { step } => Some(*step),
            _ => None,
        }
    }
}

/// Logs a finished run (typed fields only).
pub fn log_report(report: &InstallReport) {
    match &report.outcome {
        InstallOutcome::NoJournal | InstallOutcome::AlreadyFinished => {}
        InstallOutcome::Installed => crate::f3d_log!(
            info,
            "restore.installed",
            attempts = report.attempts,
            rolled_back = report.rolled_back,
        ),
        InstallOutcome::Failed { code, step } => match step {
            Some(step) => crate::f3d_log!(
                warn,
                "restore.failed",
                code = *code,
                step = *step,
                attempts = report.attempts,
                rolled_back = report.rolled_back,
            ),
            None => crate::f3d_log!(
                warn,
                "restore.failed",
                code = *code,
                attempts = report.attempts,
                rolled_back = report.rolled_back,
            ),
        },
    }
}

/// Logs a run that stopped startup (typed fields only).
pub fn log_error(error: &InstallerError) {
    let code = ErrorCode::RestoreFailed;
    match error.step() {
        Some(step) => crate::f3d_log!(error, "restore.startupFailed", code = code, step = step),
        None => crate::f3d_log!(error, "restore.startupFailed", code = code),
    }
}

// --- the lease --------------------------------------------------------------------------

/// Startup step 2: the metadata-root lease. While `restore/journal.json`
/// exists, a busy lock is retried every 100 ms for up to [`LEASE_WAIT`],
/// because the process that requested the restart may still be exiting;
/// otherwise F1's immediate failure is unchanged.
pub fn acquire_lease(paths: &StoragePaths) -> Result<MetadataRootLease, StorageError> {
    if fs::symlink_metadata(journal::journal_path(paths)).is_ok() {
        MetadataRootLease::acquire_retrying(paths, LEASE_WAIT)
    } else {
        MetadataRootLease::acquire(paths)
    }
}

// --- the run ----------------------------------------------------------------------------

/// Startup step 3 (see the module doc). `open_credentials` is called only
/// for a reset's `deleteCredentials` step, never for a restore.
pub fn run(
    paths: &StoragePaths,
    lease: &MetadataRootLease,
    open_credentials: impl FnOnce() -> CredentialStore,
) -> Result<InstallReport, InstallerError> {
    run_with_faults(paths, lease, open_credentials, &Faults::default())
}

/// [`run`] with injected faults (tests).
#[doc(hidden)]
pub fn run_with_faults(
    paths: &StoragePaths,
    lease: &MetadataRootLease,
    open_credentials: impl FnOnce() -> CredentialStore,
    faults: &Faults,
) -> Result<InstallReport, InstallerError> {
    if lease.metadata_root() != paths.metadata_root() {
        return Err(InstallerError::InstallFailed { step: None });
    }
    let current = match journal::read(paths) {
        Ok(None) => return Ok(InstallReport::of(None, InstallOutcome::NoJournal)),
        Ok(Some(current)) => current,
        Err(error) => return Err(InstallerError::Journal(error)),
    };
    if current.phase.is_finished() {
        return Ok(InstallReport::of(
            Some(&current),
            InstallOutcome::AlreadyFinished,
        ));
    }
    match current.kind {
        RestoreJournalKind::Restore => {
            // A restore never opens the credential store.
            drop(open_credentials);
            Restore::new(paths, faults, current)?.run()
        }
        // Task 8 adds the reset steps.
        RestoreJournalKind::Reset => Err(InstallerError::InstallFailed { step: current.step }),
    }
}

/// Why a step stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum StepError {
    /// An injected crash: return at once.
    Crash,
    /// An I/O error (retryable; `StorageFull` is out of space).
    Io(io::ErrorKind),
    /// A staged file or the placed database failed verification.
    Invalid(BackupInvalidReason),
}

impl From<io::Error> for StepError {
    fn from(error: io::Error) -> Self {
        StepError::Io(error.kind())
    }
}

impl From<rusqlite::Error> for StepError {
    fn from(error: rusqlite::Error) -> Self {
        match error.sqlite_error_code() {
            Some(rusqlite::ErrorCode::DiskFull) => StepError::Io(io::ErrorKind::StorageFull),
            Some(rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase) => {
                StepError::Invalid(BackupInvalidReason::DatabaseInvalid)
            }
            _ => StepError::Io(io::ErrorKind::Other),
        }
    }
}

impl From<StorageError> for StepError {
    fn from(error: StorageError) -> Self {
        match error {
            StorageError::StorageFull => StepError::Io(io::ErrorKind::StorageFull),
            _ => StepError::Io(io::ErrorKind::Other),
        }
    }
}

/// `validate`'s reading of a SQLite error: running out of space or an I/O
/// failure rolls back and retries (D8); anything else means the placed
/// database is invalid (`BACKUP_INVALID`), which a retry can't fix.
fn validate_error(error: rusqlite::Error) -> StepError {
    use rusqlite::ErrorCode as Code;
    match error.sqlite_error_code() {
        Some(Code::DiskFull) => StepError::Io(io::ErrorKind::StorageFull),
        Some(
            Code::SystemIoFailure
            | Code::CannotOpen
            | Code::DatabaseBusy
            | Code::DatabaseLocked
            | Code::OutOfMemory
            | Code::ReadOnly
            | Code::FileLockingProtocolFailed
            | Code::NoLargeFileSupport
            | Code::OperationInterrupted,
        ) => StepError::Io(io::ErrorKind::Other),
        _ => database_invalid(),
    }
}

/// [`validate_error`] for what the migrations and the integrity catalogue
/// return.
fn validate_storage_error(error: StorageError) -> StepError {
    match error {
        StorageError::StorageFull => StepError::Io(io::ErrorKind::StorageFull),
        StorageError::Filesystem | StorageError::PersistenceUnavailable => {
            StepError::Io(io::ErrorKind::Other)
        }
        _ => database_invalid(),
    }
}

fn database_invalid() -> StepError {
    StepError::Invalid(BackupInvalidReason::DatabaseInvalid)
}

fn count_mismatch() -> StepError {
    StepError::Invalid(BackupInvalidReason::CountMismatch)
}

fn checksum_mismatch() -> StepError {
    StepError::Invalid(BackupInvalidReason::ChecksumMismatch)
}

/// How an install attempt ended.
enum Stop {
    Crash,
    /// Finished before anything moved (`recheckBlockers`).
    Finished(InstallOutcome),
    /// Roll back, for this cause.
    RollBack(StepError),
    /// Stop startup, retryable.
    Startup(InstallerError),
}

fn now() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn exists(path: &Path) -> bool {
    fs::symlink_metadata(path).is_ok()
}

fn remove_file_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_file(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

fn remove_dir_if_present(path: &Path) -> io::Result<()> {
    match fs::remove_dir_all(path) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error),
    }
}

/// A restore journal's install, rollback, and roll-forward.
struct Restore<'a> {
    paths: &'a StoragePaths,
    faults: &'a Faults,
    journal: RestoreJournal,
    layout: StagingLayout,
    manifest: Option<Manifest>,
    rolled_back: bool,
    invalid_reason: Option<BackupInvalidReason>,
}

type Work<'a> = fn(&mut Restore<'a>) -> Result<(), StepError>;

impl<'a> Restore<'a> {
    fn new(
        paths: &'a StoragePaths,
        faults: &'a Faults,
        journal: RestoreJournal,
    ) -> Result<Self, InstallerError> {
        let staging_id = journal
            .staging_id
            .clone()
            .filter(|id| staging::is_staging_id(id))
            .ok_or(InstallerError::Journal(journal::JournalError::Unreadable))?;
        let layout = StagingLayout::new(paths, &staging_id);
        Ok(Self {
            paths,
            faults,
            journal,
            layout,
            manifest: None,
            rolled_back: false,
            invalid_reason: None,
        })
    }

    fn root(&self, name: &str) -> PathBuf {
        self.paths.metadata_root().join(name)
    }

    fn previous_dir(&self) -> PathBuf {
        journal::previous_dir(self.paths, &self.journal.id)
    }

    fn report(&self, outcome: InstallOutcome) -> InstallReport {
        InstallReport {
            rolled_back: self.rolled_back,
            invalid_reason: self.invalid_reason,
            ..InstallReport::of(Some(&self.journal), outcome)
        }
    }

    fn fault(&self, step: InstallerStep, point: FaultPoint) -> Result<(), StepError> {
        self.faults.hit(step, point)
    }

    fn save(&self) -> Result<(), StepError> {
        journal::write(self.paths, &self.journal).map_err(StepError::from)
    }

    fn set_phase(&mut self, to: JournalPhase) -> Result<(), StepError> {
        self.journal
            .set_phase(to)
            .map_err(|_| StepError::Io(io::ErrorKind::InvalidData))
    }

    /// Records `step` in the journal before it starts.
    fn record(&mut self, step: InstallerStep) -> Result<(), StepError> {
        self.journal.step = Some(step);
        self.save()?;
        self.fault(step, FaultPoint::Start)
    }

    fn run(mut self) -> Result<InstallReport, InstallerError> {
        // Why the attempt before this rollback stopped. A journal found
        // `installing` at startup stopped with a crash.
        let mut cause = StepError::Crash;
        loop {
            if self.journal.phase == JournalPhase::Installing || self.journal.rollback.is_some() {
                let from = self
                    .journal
                    .rollback
                    .map(|marker| marker.from)
                    .or(self.journal.step)
                    .unwrap_or(InstallerStep::MarkInstalling);
                match self.roll_back(from) {
                    Ok(()) => {}
                    Err(StepError::Crash) => return Err(InstallerError::Crashed),
                    Err(_) => return Err(InstallerError::RollbackFailed { step: from }),
                }
                self.rolled_back = true;
                match self.finish_rollback(from, cause) {
                    Ok(Some(outcome)) => return Ok(self.report(outcome)),
                    Ok(None) => {}
                    Err(StepError::Crash) => return Err(InstallerError::Crashed),
                    Err(_) => return Err(InstallerError::RollbackFailed { step: from }),
                }
            }
            match self.journal.phase {
                JournalPhase::Pending => match self.install() {
                    Ok(()) => return Ok(self.report(InstallOutcome::Installed)),
                    Err(Stop::Crash) => return Err(InstallerError::Crashed),
                    Err(Stop::Finished(outcome)) => return Ok(self.report(outcome)),
                    Err(Stop::Startup(error)) => return Err(error),
                    Err(Stop::RollBack(error)) => cause = error,
                },
                JournalPhase::Installed => {
                    return match self.roll_forward() {
                        Ok(()) => Ok(self.report(InstallOutcome::Installed)),
                        Err(StepError::Crash) => Err(InstallerError::Crashed),
                        Err(_) => Err(InstallerError::InstallFailed {
                            step: self.journal.step,
                        }),
                    }
                }
                JournalPhase::Done | JournalPhase::Failed => {
                    return Ok(self.report(InstallOutcome::AlreadyFinished))
                }
                // Rolled back at the top of the loop.
                JournalPhase::Installing => {}
            }
        }
    }

    /// Steps 1–11 from `recheckBlockers` (phase `pending`).
    fn install(&mut self) -> Result<(), Stop> {
        fn step_error(error: StepError) -> Stop {
            match error {
                StepError::Crash => Stop::Crash,
                other => Stop::RollBack(other),
            }
        }
        fn before_install(step: InstallerStep) -> impl Fn(StepError) -> Stop {
            move |error| match error {
                StepError::Crash => Stop::Crash,
                _ => Stop::Startup(InstallerError::InstallFailed { step: Some(step) }),
            }
        }

        // 1. recheckBlockers: nothing moves.
        self.record(InstallerStep::RecheckBlockers)
            .map_err(before_install(InstallerStep::RecheckBlockers))?;
        if !self.load_staging() {
            return Err(self.fail_before_install(ErrorCode::RestoreStagingExpired));
        }
        let blocked = self
            .blocked()
            .map_err(before_install(InstallerStep::RecheckBlockers))?;
        if blocked {
            return Err(self.fail_before_install(ErrorCode::RestoreBlocked));
        }

        // 2. markInstalling.
        self.set_phase(JournalPhase::Installing)
            .map_err(before_install(InstallerStep::MarkInstalling))?;
        self.journal.attempts += 1;
        self.journal.step = Some(InstallerStep::MarkInstalling);
        self.save()
            .map_err(before_install(InstallerStep::MarkInstalling))?;
        self.fault(InstallerStep::MarkInstalling, FaultPoint::End)
            .map_err(step_error)?;

        // 3–8, each recorded before it starts.
        let steps: [(InstallerStep, Work<'a>); 6] = [
            (InstallerStep::MoveDatabaseAside, Self::move_database_aside),
            (InstallerStep::PlaceCandidate, Self::place_candidate),
            (InstallerStep::CarryLocalState, Self::carry_local_state),
            (InstallerStep::ExtractContent, Self::extract_content),
            (InstallerStep::SwapMedia, Self::swap_media),
            (InstallerStep::Validate, Self::validate),
        ];
        for (step, work) in steps {
            self.record(step).map_err(step_error)?;
            work(self).map_err(step_error)?;
            self.fault(step, FaultPoint::End).map_err(step_error)?;
        }

        // 9. markInstalled.
        self.record(InstallerStep::MarkInstalled)
            .map_err(step_error)?;
        self.set_phase(JournalPhase::Installed)
            .map_err(step_error)?;
        self.save().map_err(step_error)?;
        self.fault(InstallerStep::MarkInstalled, FaultPoint::End)
            .map_err(step_error)?;

        // 10–11 roll forward from here on.
        self.roll_forward().map_err(|error| match error {
            StepError::Crash => Stop::Crash,
            _ => Stop::Startup(InstallerError::InstallFailed {
                step: self.journal.step,
            }),
        })
    }

    /// Whether the staging the journal names is complete; loads its
    /// manifest.
    fn load_staging(&mut self) -> bool {
        let complete = self.layout.candidate_path().is_file()
            && self.layout.content_dir.is_dir()
            && self.layout.media_snapshots_dir().is_dir();
        if !complete {
            return false;
        }
        match fs::read(self.layout.manifest_path())
            .ok()
            .and_then(|bytes| Manifest::parse(&bytes).ok())
        {
            Some(manifest) => {
                self.manifest = Some(manifest);
                true
            }
            None => false,
        }
    }

    /// `recheckBlockers` fails the journal before anything moved and
    /// removes the staging.
    fn fail_before_install(&mut self, code: ErrorCode) -> Stop {
        let step = Some(InstallerStep::RecheckBlockers);
        let written = self.set_phase(JournalPhase::Failed).and_then(|()| {
            self.journal.failure = Some(Failure {
                code,
                step,
                finished_at: now(),
            });
            self.save()
        });
        match written {
            Ok(()) => {
                self.layout.remove();
                Stop::Finished(InstallOutcome::Failed { code, step })
            }
            Err(StepError::Crash) => Stop::Crash,
            Err(_) => Stop::Startup(InstallerError::InstallFailed { step }),
        }
    }

    /// D7's blocker query on the live database, opened without a
    /// checkpoint on close and read-only in effect: the main file isn't
    /// written (an empty `-wal` and a `-shm` may appear).
    fn blocked(&self) -> Result<bool, StepError> {
        let database = self.paths.database();
        if !exists(database) {
            return Ok(false);
        }
        let connection = Connection::open_with_flags(
            database,
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.set_db_config(
            rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
            true,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "query_only", "ON")?;
        let transaction = connection.unchecked_transaction()?;
        let (_, total) = super::preview::blockers(&transaction)?;
        transaction.rollback()?;
        drop(connection);
        Ok(total > 0)
    }

    /// Step 3: Moves `farm3d.sqlite3`, `-wal`, and `-shm` (each only if present)
    /// into `restore/<id>/previous/`, in that order.
    fn move_database_aside(&mut self) -> Result<(), StepError> {
        // Each directory created here is made durable (its parent synced).
        let (previous, _) = journal::create_synced_dir(
            self.paths.metadata_root(),
            &Path::new("restore").join(&self.journal.id).join("previous"),
        )?;
        for (index, name) in DATABASE_FILES.iter().enumerate() {
            let live = self.root(name);
            if exists(&live) {
                fs::rename(&live, previous.join(name))?;
            }
            if index == 0 {
                self.fault(InstallerStep::MoveDatabaseAside, FaultPoint::Within(0))?;
            }
        }
        sync_directory(self.paths.metadata_root())?;
        sync_directory(&previous)?;
        Ok(())
    }

    /// Step 4: Copies the candidate through a same-directory `.partial` file
    /// and renames it to `farm3d.sqlite3`. The staged candidate stays.
    fn place_candidate(&mut self) -> Result<(), StepError> {
        let candidate = self.layout.candidate_path();
        for suffix in ["-wal", "-shm", "-journal"] {
            let mut side = candidate.clone().into_os_string();
            side.push(suffix);
            if exists(Path::new(&side)) {
                return Err(database_invalid());
            }
        }
        let partial = self.root(PARTIAL_FILE);
        remove_file_if_present(&partial)?;
        let mut source = File::open(&candidate)?;
        let length = source.metadata()?.len();
        let mut target = File::options()
            .write(true)
            .create_new(true)
            .open(&partial)?;
        io::copy(&mut (&mut source).take(length / 2), &mut target)?;
        target.flush()?;
        self.fault(InstallerStep::PlaceCandidate, FaultPoint::Within(0))?;
        io::copy(&mut source, &mut target)?;
        target.sync_all()?;
        drop(target);
        fs::rename(&partial, self.root(DATABASE_FILES[0]))?;
        sync_directory(self.paths.metadata_root())?;
        Ok(())
    }

    /// Opens the placed database as `Storage::open` would: WAL,
    /// `synchronous = FULL`, foreign keys on.
    fn open_placed(&self) -> Result<Connection, StepError> {
        let connection = Connection::open_with_flags(
            self.paths.database(),
            OpenFlags::SQLITE_OPEN_READ_WRITE
                | OpenFlags::SQLITE_OPEN_NO_MUTEX
                | OpenFlags::SQLITE_OPEN_NOFOLLOW,
        )?;
        connection.busy_timeout(Duration::from_secs(5))?;
        connection.pragma_update(None, "foreign_keys", "ON")?;
        let mode: String =
            connection.query_row("PRAGMA journal_mode = WAL", [], |row| row.get(0))?;
        if !mode.eq_ignore_ascii_case("wal") {
            return Err(StepError::Io(io::ErrorKind::Unsupported));
        }
        connection.pragma_update(None, "synchronous", "FULL")?;
        Ok(connection)
    }

    /// Step 5: One transaction on the placed database: this machine's Slicer
    /// runtime paths, the carried cleanup rows, and an `import_orphan` row
    /// for every orphan ref, through F1's precedence upsert.
    fn carry_local_state(&mut self) -> Result<(), StepError> {
        let mut connection = self.open_placed()?;
        let result = self.carry_in(&mut connection);
        if result.is_err() {
            // What a crash leaves: the uncommitted transaction and its -wal.
            let _ = connection.set_db_config(
                rusqlite::config::DbConfig::SQLITE_DBCONFIG_NO_CKPT_ON_CLOSE,
                true,
            );
            return result;
        }
        connection
            .close()
            .map_err(|(_, error)| StepError::from(error))
    }

    fn carry_in(&self, connection: &mut Connection) -> Result<(), StepError> {
        let carry = self.journal.carry.clone().unwrap_or(journal::Carry {
            slicer_runtime: None,
            pending_credential_cleanup: Vec::new(),
        });
        let transaction = connection.transaction()?;
        if let Some(slicer) = &carry.slicer_runtime {
            transaction.execute(
                "INSERT INTO slicer_runtime_config(singleton_id, revision, engine_path,
                   preset_source_path, updated_at)
                 VALUES (1, 1, ?1, ?2, ?3)
                 ON CONFLICT(singleton_id) DO UPDATE SET
                   engine_path = excluded.engine_path,
                   preset_source_path = excluded.preset_source_path,
                   revision = slicer_runtime_config.revision + 1,
                   updated_at = excluded.updated_at",
                params![slicer.engine_path, slicer.preset_source_path, now()],
            )?;
        }
        for row in &carry.pending_credential_cleanup {
            let present = transaction
                .query_row(
                    "SELECT 1 FROM pending_credential_cleanup WHERE credential_ref = ?1",
                    [&row.credential_ref],
                    |_| Ok(()),
                )
                .optional()?
                .is_some();
            if present {
                crate::printers::repository::enqueue_credential_cleanup(
                    &transaction,
                    &row.credential_ref,
                    row.printer_id.as_deref(),
                    &row.reason,
                )?;
            } else {
                transaction.execute(
                    "INSERT INTO pending_credential_cleanup(credential_ref, printer_id, reason,
                       attempt_count, last_error_code, created_at, last_attempt_at)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        row.credential_ref,
                        row.printer_id,
                        row.reason,
                        row.attempt_count,
                        row.last_error_code,
                        row.created_at,
                        row.last_attempt_at
                    ],
                )?;
            }
        }
        for reference in &self.journal.orphan_credential_refs {
            crate::printers::repository::enqueue_credential_cleanup(
                &transaction,
                reference,
                None,
                "import_orphan",
            )?;
        }
        self.fault(InstallerStep::CarryLocalState, FaultPoint::Within(0))?;
        transaction.commit()?;
        Ok(())
    }

    fn manifest(&self) -> Result<&Manifest, StepError> {
        self.manifest
            .as_ref()
            .ok_or(StepError::Io(io::ErrorKind::NotFound))
    }

    /// Step 6: Adds every manifest blob the content store lacks, re-hashing the
    /// staged file first (D3's second verification). Only adds files.
    fn extract_content(&mut self) -> Result<(), StepError> {
        let content_root = self.paths.content_root().to_path_buf();
        let mut renamed = 0_u32;
        let entries: Vec<(String, u64, String)> = self
            .manifest()?
            .entries
            .iter()
            .filter_map(|entry| {
                entry
                    .path
                    .strip_prefix("content/sha256/")
                    .map(|rest| (rest.to_string(), entry.bytes, entry.sha256.clone()))
            })
            .collect();
        for (relative, bytes, sha256) in entries {
            let (prefix, hex) = relative
                .split_once('/')
                .filter(|(prefix, hex)| {
                    prefix.len() == 2 && is_sha256_hex(hex) && hex.starts_with(prefix)
                })
                .ok_or_else(checksum_mismatch)?;
            let target = content_root.join("blobs/sha256").join(prefix).join(hex);
            if let Ok(metadata) = fs::symlink_metadata(&target) {
                if metadata.is_file() && metadata.len() == bytes {
                    continue;
                }
            }
            let staged = self.layout.content_dir.join(prefix).join(hex);
            match hash_file(&staged) {
                Some((length, hash)) if length == bytes && hash == sha256 => {}
                _ => return Err(checksum_mismatch()),
            }
            // A new prefix directory is made durable before the rename.
            let (directory, _) =
                journal::create_synced_dir(&content_root, &Path::new("blobs/sha256").join(prefix))?;
            fs::rename(&staged, &target)?;
            sync_directory(&directory)?;
            if renamed == 0 {
                self.fault(InstallerStep::ExtractContent, FaultPoint::Within(0))?;
            }
            renamed += 1;
        }
        Ok(())
    }

    /// Step 7: Re-hashes the staged media, records `liveExisted`, then swaps
    /// `<media_root>/snapshots` for the staged tree.
    fn swap_media(&mut self) -> Result<(), StepError> {
        let staged_tree = self.layout.media_snapshots_dir();
        let media: Vec<(String, u64, String)> = self
            .manifest()?
            .entries
            .iter()
            .filter_map(|entry| {
                entry
                    .path
                    .strip_prefix("media/")
                    .map(|rest| (rest.to_string(), entry.bytes, entry.sha256.clone()))
            })
            .collect();
        for (relative, bytes, sha256) in media {
            if !super::archive::is_media_rel_path(&relative) {
                return Err(checksum_mismatch());
            }
            match hash_file(&self.layout.media_dir.join(&relative)) {
                Some((length, hash)) if length == bytes && hash == sha256 => {}
                _ => return Err(checksum_mismatch()),
            }
        }
        if !staged_tree.is_dir() {
            return Err(StepError::Io(io::ErrorKind::NotFound));
        }
        let live = self.paths.media_root().join(SNAPSHOTS);
        let previous = self.layout.media_dir.join(PREVIOUS_SNAPSHOTS);
        if exists(&previous) {
            return Err(StepError::Io(io::ErrorKind::AlreadyExists));
        }
        let live_existed = exists(&live);
        self.journal.swap_media = Some(SwapMedia { live_existed });
        self.save()?;
        self.fault(InstallerStep::SwapMedia, FaultPoint::Within(0))?;
        if live_existed {
            fs::rename(&live, &previous)?;
        }
        self.fault(InstallerStep::SwapMedia, FaultPoint::Within(1))?;
        fs::rename(&staged_tree, &live)?;
        sync_directory(self.paths.media_root())?;
        sync_directory(&self.layout.media_dir)?;
        Ok(())
    }

    /// Step 8: Opens the placed database as `Storage::open` would, migrates it,
    /// and checks it: SQLite's checks, the integrity catalogue with the
    /// live roots, and the counts (with the two carried exceptions).
    fn validate(&mut self) -> Result<(), StepError> {
        let mut connection = self.open_placed()?;
        self.fault(InstallerStep::Validate, FaultPoint::Within(0))?;
        apply_migrations(&mut connection).map_err(validate_storage_error)?;
        let check: String = connection
            .query_row("PRAGMA integrity_check", [], |row| row.get(0))
            .map_err(validate_error)?;
        if check != "ok" || has_foreign_key_violation(&connection).map_err(validate_error)? {
            return Err(database_invalid());
        }
        let report = integrity::check(&connection, Some(&IntegrityRoots::from_paths(self.paths)))
            .map_err(validate_storage_error)?;
        if !report.violations().is_empty() {
            return Err(database_invalid());
        }
        let counts = table_counts(&connection).map_err(validate_error)?;
        let expected = self.journal.expected_counts.clone().unwrap_or_default();
        let tables: BTreeSet<&String> = counts.keys().chain(expected.keys()).collect();
        for table in tables {
            if table == "pending_credential_cleanup" || table == "slicer_runtime_config" {
                continue;
            }
            if counts.get(table) != expected.get(table) {
                return Err(count_mismatch());
            }
        }
        let queued: BTreeSet<String> = connection
            .prepare("SELECT credential_ref FROM pending_credential_cleanup")
            .and_then(|mut statement| {
                statement
                    .query_map([], |row| row.get(0))?
                    .collect::<rusqlite::Result<_>>()
            })
            .map_err(validate_error)?;
        let carry = self.journal.carry.as_ref();
        let carried = carry
            .into_iter()
            .flat_map(|carry| carry.pending_credential_cleanup.iter())
            .map(|row| &row.credential_ref)
            .chain(self.journal.orphan_credential_refs.iter());
        for reference in carried {
            if !queued.contains(reference) {
                return Err(count_mismatch());
            }
        }
        if let Some(slicer) = carry.and_then(|carry| carry.slicer_runtime.as_ref()) {
            let paths: Option<(Option<String>, Option<String>)> = connection
                .query_row(
                    "SELECT engine_path, preset_source_path FROM slicer_runtime_config
                      WHERE singleton_id = 1",
                    [],
                    |row| Ok((row.get(0)?, row.get(1)?)),
                )
                .optional()
                .map_err(validate_error)?;
            if paths
                != Some((
                    slicer.engine_path.clone(),
                    slicer.preset_source_path.clone(),
                ))
            {
                return Err(count_mismatch());
            }
        }
        // A normal close checkpoints, so no -wal is left.
        connection
            .close()
            .map_err(|(_, error)| StepError::from(error))?;
        File::open(self.paths.database())?.sync_all()?;
        sync_directory(self.paths.metadata_root())?;
        Ok(())
    }

    /// Steps 10–11 (phase `installed`): removes what the install left, then
    /// marks the journal `done`.
    fn roll_forward(&mut self) -> Result<(), StepError> {
        self.record(InstallerStep::RemovePrevious)?;
        remove_dir_if_present(&self.previous_dir())?;
        self.fault(InstallerStep::RemovePrevious, FaultPoint::Within(0))?;
        for directory in [
            &self.layout.media_dir,
            &self.layout.content_dir,
            &self.layout.database_dir,
        ] {
            remove_dir_if_present(directory)?;
        }
        self.fault(InstallerStep::RemovePrevious, FaultPoint::End)?;

        self.set_phase(JournalPhase::Done)?;
        self.journal.step = Some(InstallerStep::MarkDone);
        self.journal.outcome = Some(Outcome { finished_at: now() });
        self.save()?;
        self.fault(InstallerStep::MarkDone, FaultPoint::End)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sqlite(code: std::os::raw::c_int) -> rusqlite::Error {
        rusqlite::Error::SqliteFailure(rusqlite::ffi::Error::new(code), None)
    }

    /// Out of space keeps its kind through `StorageError`, so a failed
    /// directory create maps to `INSUFFICIENT_SPACE` after the last attempt.
    #[test]
    fn a_storage_error_keeps_out_of_space() {
        let full = StorageError::from(io::Error::from(io::ErrorKind::StorageFull));
        assert_eq!(
            StepError::from(full),
            StepError::Io(io::ErrorKind::StorageFull)
        );
        let full = StorageError::from(sqlite(rusqlite::ffi::SQLITE_FULL));
        assert_eq!(
            StepError::from(full),
            StepError::Io(io::ErrorKind::StorageFull)
        );
        let other = StorageError::from(io::Error::from(io::ErrorKind::PermissionDenied));
        assert_eq!(StepError::from(other), StepError::Io(io::ErrorKind::Other));
    }

    /// `validate`: an I/O error or `SQLITE_FULL` rolls back and retries; only
    /// a database that is genuinely invalid is `BACKUP_INVALID`.
    #[test]
    fn validate_tells_io_from_an_invalid_database() {
        use rusqlite::ffi;
        assert_eq!(
            validate_error(sqlite(ffi::SQLITE_FULL)),
            StepError::Io(io::ErrorKind::StorageFull)
        );
        for code in [
            ffi::SQLITE_IOERR,
            ffi::SQLITE_CANTOPEN,
            ffi::SQLITE_BUSY,
            ffi::SQLITE_NOMEM,
        ] {
            assert!(
                matches!(validate_error(sqlite(code)), StepError::Io(_)),
                "{code}"
            );
        }
        for code in [
            ffi::SQLITE_CORRUPT,
            ffi::SQLITE_NOTADB,
            ffi::SQLITE_CONSTRAINT,
            ffi::SQLITE_ERROR,
        ] {
            assert_eq!(validate_error(sqlite(code)), database_invalid(), "{code}");
        }
        assert_eq!(
            validate_storage_error(StorageError::from(sqlite(ffi::SQLITE_FULL))),
            StepError::Io(io::ErrorKind::StorageFull)
        );
        assert!(matches!(
            validate_storage_error(StorageError::Filesystem),
            StepError::Io(_)
        ));
        assert!(matches!(
            validate_storage_error(StorageError::PersistenceUnavailable),
            StepError::Io(_)
        ));
        for invalid in [
            StorageError::MigrationFailed,
            StorageError::UnsupportedSchemaVersion,
            StorageError::Database,
        ] {
            assert_eq!(validate_storage_error(invalid), database_invalid());
        }
    }
}
