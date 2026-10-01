//! D8 "Rollback and retry (restore)": the two-phase, resumable rollback.
//! Each phase's completion is durable in `journal.rollback` before the
//! next starts, so a crash or an I/O error inside a rollback resumes it
//! without redoing a destructive step, and phase (b) only ever deletes
//! files that can only be the candidate.

use std::fs;
use std::io;

use super::{
    exists, now, remove_file_if_present, FaultPoint, InstallOutcome, Restore, RollbackPoint,
    StepError, DATABASE_FILES, MAX_ATTEMPTS, PARTIAL_FILE, PREVIOUS_SNAPSHOTS,
    ROLLBACK_JOURNAL_FILE, SNAPSHOTS,
};
use crate::backup::journal::{sync_directory, Failure, JournalPhase, RollbackMarker};
use crate::backup::InstallerStep;
use crate::contracts::command::ErrorCode;

impl Restore<'_> {
    fn rollback_fault(&self, from: InstallerStep, point: RollbackPoint) -> Result<(), StepError> {
        self.fault(from, FaultPoint::Rollback(point))
    }

    fn save_marker(&mut self, marker: RollbackMarker) -> Result<(), StepError> {
        self.journal.rollback = Some(marker);
        self.save()
    }

    /// D8's two-phase rollback, resumable at the first phase not yet
    /// marked done. `journal.step` is never changed here.
    pub(super) fn roll_back(&mut self, from: InstallerStep) -> Result<(), StepError> {
        let mut marker = match self.journal.rollback {
            Some(marker) => marker,
            None => {
                let marker = RollbackMarker {
                    from,
                    media_restored: false,
                    candidate_cleared: false,
                };
                self.save_marker(marker)?;
                marker
            }
        };
        let from = marker.from;
        self.rollback_fault(from, RollbackPoint::MarkerWritten)?;

        // (a) Media.
        if !marker.media_restored {
            self.restore_media()?;
            marker.media_restored = true;
            self.journal.swap_media = None;
            self.save_marker(marker)?;
            self.rollback_fault(from, RollbackPoint::MediaRestored)?;
        }

        // (b) Clear the candidate: from `placeCandidate` on, every original
        // file is already in `previous/` (the step after `moveDatabaseAside`
        // is recorded only once it completed), so what is in the root is
        // the candidate. Before that, whatever is in the root is original.
        if !marker.candidate_cleared {
            self.rollback_fault(from, RollbackPoint::BeforeClear)?;
            if from >= InstallerStep::PlaceCandidate {
                for name in DATABASE_FILES
                    .iter()
                    .chain([ROLLBACK_JOURNAL_FILE, PARTIAL_FILE].iter())
                {
                    remove_file_if_present(&self.root(name))?;
                }
                sync_directory(self.paths.metadata_root())?;
            }
            marker.candidate_cleared = true;
            self.save_marker(marker)?;
            self.rollback_fault(from, RollbackPoint::CandidateCleared)?;
        }

        // (c) Move back, in order; a rename is atomic, so each file is in
        // exactly one of the two places.
        let previous = self.previous_dir();
        for (index, name) in DATABASE_FILES.iter().enumerate() {
            let aside = previous.join(name);
            if exists(&aside) {
                fs::rename(&aside, self.root(name))?;
            }
            self.rollback_fault(from, RollbackPoint::MovedBack(index as u8 + 1))?;
        }
        sync_directory(self.paths.metadata_root())?;
        if previous.is_dir() {
            sync_directory(&previous)?;
        }
        // Content: nothing. A blob the install added has no row, and P4's
        // startup sweep removes it.
        Ok(())
    }

    /// Rollback phase (a).
    fn restore_media(&self) -> Result<(), StepError> {
        let Some(swap) = self.journal.swap_media else {
            // `swapMedia` never renamed anything.
            return Ok(());
        };
        let live = self.paths.media_root().join(SNAPSHOTS);
        let staged = self.layout.media_snapshots_dir();
        let previous = self.layout.media_dir.join(PREVIOUS_SNAPSHOTS);
        if swap.live_existed {
            if exists(&previous) {
                // With `previous-snapshots` present, a `snapshots` can only
                // be the staged tree.
                if exists(&live) {
                    fs::rename(&live, &staged)?;
                }
                fs::rename(&previous, &live)?;
            }
        } else if exists(&live) {
            // It can only be the staged tree.
            fs::rename(&live, &staged)?;
        }
        sync_directory(self.paths.media_root())?;
        if self.layout.media_dir.is_dir() {
            sync_directory(&self.layout.media_dir)?;
        }
        Ok(())
    }

    /// The one journal write that ends a rollback: `pending` for a retry,
    /// or `failed`. Returns the outcome when the restore failed.
    pub(super) fn finish_rollback(
        &mut self,
        from: InstallerStep,
        cause: StepError,
    ) -> Result<Option<InstallOutcome>, StepError> {
        let code = match cause {
            StepError::Invalid(reason) => {
                self.invalid_reason = Some(reason);
                Some(ErrorCode::BackupInvalid)
            }
            _ if self.journal.attempts < MAX_ATTEMPTS => None,
            StepError::Io(io::ErrorKind::StorageFull) => Some(ErrorCode::InsufficientSpace),
            _ => Some(ErrorCode::RestoreFailed),
        };
        self.journal.rollback = None;
        match code {
            None => {
                self.set_phase(JournalPhase::Pending)?;
                self.save()?;
                Ok(None)
            }
            Some(code) => {
                self.set_phase(JournalPhase::Failed)?;
                self.journal.failure = Some(Failure {
                    code,
                    step: Some(from),
                    finished_at: now(),
                });
                self.save()?;
                self.layout.remove();
                Ok(Some(InstallOutcome::Failed {
                    code,
                    step: Some(from),
                }))
            }
        }
    }
}
