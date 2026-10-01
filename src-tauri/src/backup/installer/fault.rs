//! Fault injection for the installer (test-only, `#[doc(hidden)]`):
//! `installer::Fault { step, point }` makes the run crash (return at once,
//! dropping every handle, as a process death would) or fail with an I/O
//! error at a named point. Production passes none.

use std::io;
use std::sync::Mutex;

use super::StepError;
use crate::backup::InstallerStep;

/// A point inside a rollback.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RollbackPoint {
    /// `journal.rollback` is written; phase (a) hasn't started.
    MarkerWritten,
    /// Phase (a) is durable (`mediaRestored`).
    MediaRestored,
    /// Phase (b) is about to delete the candidate set.
    BeforeClear,
    /// Phase (b) is durable (`candidateCleared`).
    CandidateCleared,
    /// Phase (c) handled the first `n` of `farm3d.sqlite3`, `-wal`, `-shm`.
    MovedBack(u8),
}

/// Where in a step a fault fires.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultPoint {
    /// The step is recorded in the journal; nothing of it is done.
    Start,
    /// A named point inside the step: `moveDatabaseAside` 0 (the main file
    /// moved), `placeCandidate` 0 (the `.partial` half-written),
    /// `carryLocalState` 0 (before the commit), `extractContent` 0 (after
    /// an attempt's first blob rename), `swapMedia` 0 (`liveExisted`
    /// recorded) and 1 (between the renames), `validate` 0 (the database
    /// open), `removePrevious` 0 (`previous/` deleted).
    Within(u8),
    /// The step is done; the next isn't recorded.
    End,
    /// Inside a rollback whose `from` is the fault's step.
    Rollback(RollbackPoint),
}

#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FaultEffect {
    /// The process dies: the run returns [`InstallerError::Crashed`] at
    /// once, with nothing cleaned up.
    Crash,
    /// An I/O error of this kind, handled in-run.
    Error(io::ErrorKind),
}

/// `installer::Fault { step, point }`: fires `times` times.
#[doc(hidden)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Fault {
    pub step: InstallerStep,
    pub point: FaultPoint,
    pub effect: FaultEffect,
    pub times: u32,
}

#[doc(hidden)]
#[derive(Default)]
pub struct Faults(Mutex<Vec<Fault>>);

impl Faults {
    pub fn new(faults: impl IntoIterator<Item = Fault>) -> Self {
        Self(Mutex::new(faults.into_iter().collect()))
    }

    pub(super) fn hit(&self, step: InstallerStep, point: FaultPoint) -> Result<(), StepError> {
        let mut faults = self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let Some(fault) = faults
            .iter_mut()
            .find(|fault| fault.step == step && fault.point == point && fault.times > 0)
        else {
            return Ok(());
        };
        fault.times -= 1;
        Err(match fault.effect {
            FaultEffect::Crash => StepError::Crash,
            FaultEffect::Error(kind) => StepError::Io(kind),
        })
    }
}
