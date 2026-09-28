use std::fmt;

use crate::printers::lifecycle::LifecycleBlocker;

#[derive(Debug)]
pub enum StorageError {
    PathCollision,
    PersistenceUnavailable,
    UnsupportedLocking,
    UnsupportedSchemaVersion,
    MigrationFailed,
    CorruptData {
        source_name: &'static str,
        source_sha256: Option<String>,
    },
    InvalidSnapshot,
    Database,
    Filesystem,
    OperationFailed,
    /// Another active Printer already owns this host identity (D3). Carries
    /// the conflicting Printer's id, or an empty string when it's raised by
    /// the partial unique index backstop and the follow-up lookup for that
    /// id itself fails.
    DuplicateHost(String),
}

impl fmt::Display for StorageError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::PathCollision => "storage paths overlap",
            Self::PersistenceUnavailable => "storage is temporarily unavailable",
            Self::UnsupportedLocking => "metadata locking is not supported",
            Self::UnsupportedSchemaVersion => "database schema is newer than this application",
            Self::MigrationFailed => "database migration validation failed",
            Self::CorruptData { .. } => "stored data is corrupt",
            Self::InvalidSnapshot => "database snapshot validation failed",
            Self::Database => "database operation failed",
            Self::Filesystem => "storage filesystem operation failed",
            Self::OperationFailed => "storage operation was cancelled",
            Self::DuplicateHost(_) => "another active printer already uses this host and port",
        })
    }
}

impl std::error::Error for StorageError {}

impl From<rusqlite::Error> for StorageError {
    fn from(error: rusqlite::Error) -> Self {
        match &error {
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseBusy | rusqlite::ErrorCode::DatabaseLocked
                ) =>
            {
                Self::PersistenceUnavailable
            }
            rusqlite::Error::SqliteFailure(code, _)
                if matches!(
                    code.code,
                    rusqlite::ErrorCode::DatabaseCorrupt | rusqlite::ErrorCode::NotADatabase
                ) =>
            {
                Self::CorruptData {
                    source_name: "database",
                    source_sha256: None,
                }
            }
            rusqlite::Error::FromSqlConversionFailure(..)
            | rusqlite::Error::IntegralValueOutOfRange(..)
            | rusqlite::Error::InvalidColumnType(..) => Self::CorruptData {
                source_name: "database",
                source_sha256: None,
            },
            _ => Self::Database,
        }
    }
}

#[derive(Debug)]
pub enum RepositoryError {
    Validation {
        field_path: &'static str,
    },
    NotFound {
        entity_id: String,
    },
    Conflict {
        entity_id: String,
        expected_revision: i64,
        current_revision: i64,
    },
    SetConflict {
        expected_count: usize,
        current_count: usize,
    },
    DuplicateHost {
        conflicting_printer_id: String,
    },
    /// P3 D6 step 3: a slot's current occupant isn't the one the move
    /// expected (`expectedOccupantSpoolId`). `None` means the slot is empty.
    OccupancyConflict {
        slot_id: String,
        current_occupant_spool_id: Option<String>,
    },
    /// P3 Task 5: `set_material_slot_layout` tried to soft-remove a slot
    /// that still has a Spool loaded. The UI offers "Unload first".
    SlotOccupied {
        slot_id: String,
        spool_id: String,
    },
    /// D7: `archive`/`unarchive`/`delete` is blocked by the Printer's
    /// current lifecycle state (or, in a later phase, other work that still
    /// depends on it). See `crate::printers::lifecycle::evaluate`.
    LifecycleBlocked(Vec<LifecycleBlocker>),
    /// P3 D6: the `operationId` is already in the operations ledger for a
    /// different request (another kind of operation, or different fields).
    /// Nothing was written. `VALIDATION` on `operationId`.
    OperationIdReused,
    /// P3: `PrinterRepository::replace_all` (the Printers-import path)
    /// refuses to run while any Spool is loaded into a slot — see
    /// `spools::repository::any_loaded`'s doc comment for why. `VALIDATION`
    /// on `printers`, with a message telling the user to unload first.
    SpoolsLoadedForImport,
    /// P5 D10: a slice operation can't move from `from` to `to`. Nothing
    /// was written.
    IllegalSliceTransition {
        operation_id: String,
        from: crate::slicing::SliceOperationState,
        to: crate::slicing::SliceOperationState,
    },
    /// P6 D3: a Host Operation can't move from `from` to `to` (`state.rs`'s
    /// table says so, or a repository function's own narrower rule, e.g.
    /// `record_attempt` outside `reconciling`). Nothing was written.
    IllegalHostOperationTransition {
        host_operation_id: String,
        from: crate::host_ops::HostOperationState,
        to: crate::host_ops::HostOperationState,
    },
    /// P6 D3: `mark_sent` was called on a row that isn't `dispatching`, or
    /// that already has `dispatched_at` set — an executor bug either way
    /// (`mark_sent` runs exactly once per row, immediately before the
    /// send). Distinct from `NotFound` so the two can't be confused: this
    /// means the row exists but is past the point `mark_sent` may touch
    /// it. Nothing was written.
    HostOperationAlreadySent {
        host_operation_id: String,
    },
    /// P6 D7: the Connection change would change the endpoint, or clear the
    /// Connection or its credential, while `printer_id` has the unresolved
    /// Host Operation `host_operation_id`. Nothing was written.
    /// `CONNECTION_IN_USE`. P7 ruling R5(b): `job_id` is the unresolved
    /// row's own `job_id`, when it has one — an active Job with no
    /// unresolved Host Operation never reaches this variant.
    ConnectionInUse {
        printer_id: String,
        host_operation_id: String,
        job_id: Option<String>,
    },
    /// P6 D7: a Printers import while these Printers have these unresolved
    /// Host Operations. Nothing was written. `HOST_OPERATION_PENDING`. The
    /// two lists are aligned: `printer_ids[i]` owns `host_operation_ids[i]`.
    HostOperationsPending {
        printer_ids: Vec<String>,
        host_operation_ids: Vec<String>,
    },
    /// P7 D2: `queue::repository::apply` rejected an event the pure
    /// `queue::state::transition` doesn't allow from the entry's current
    /// state. Nothing was written. A later task's caller maps this to
    /// `QUEUE_ENTRY_ACTION_NOT_ALLOWED` (a user command) or `INTERNAL`
    /// (farm3d's own code), per D2's table.
    IllegalQueueEntryTransition {
        entry_id: String,
        from: crate::queue::QueueEntryState,
        event: crate::queue::state::EntryEvent,
    },
    /// P7 D2: `move_entry`/`update_entry` refused because the entry isn't
    /// in a state that action allows (`move` needs an open entry;
    /// `update` needs `queued`). Nothing was written.
    /// `QUEUE_ENTRY_ACTION_NOT_ALLOWED`.
    QueueEntryActionNotAllowed {
        entry_id: String,
        action: crate::queue::QueueEntryAction,
        state: crate::queue::QueueEntryState,
    },
    /// P7 D3: `jobs::repository::transition` rejected an event the pure
    /// `jobs::state::transition` doesn't allow from the Job's current
    /// state. Nothing was written. A later task's caller maps this to
    /// `JOB_ACTION_NOT_ALLOWED` (a user command) or drops it as an
    /// already-applied idempotent no-op (the driver, tracker, or
    /// `apply_host_outcome`), per D3's table.
    IllegalJobTransition {
        job_id: String,
        from: crate::jobs::JobState,
        event: crate::jobs::JobEventKind,
    },
    /// P7 D3: a Job command whose action the Job's state doesn't allow.
    /// Nothing was written. `JOB_ACTION_NOT_ALLOWED`.
    JobActionNotAllowed {
        job_id: String,
        action: crate::jobs::JobAction,
        state: crate::jobs::JobState,
    },
    /// P7 D4: assignment to a Printer that already has the active Job
    /// `job_id`. Nothing was written. `JOB_ACTIVE`.
    JobActive { printer_id: String, job_id: String },
    /// P7 D7: a pause, resume, or cancel handoff found the host printing a
    /// file other than the Job's own (`host_path`). The write-ahead rolled
    /// back; nothing was sent. `JOB_NOT_ON_PRINTER`.
    JobNotOnPrinter { job_id: String, printer_id: String },
    /// P7 D7, ruling R13(a): the start link found a start blocker the
    /// pre-checks didn't (the Spool left the Printer in between). The
    /// write-ahead rolled back; nothing was sent. `JOB_START_BLOCKED`.
    JobStartBlocked {
        job_id: String,
        blockers: Vec<crate::queue::Blocker>,
    },
    /// P7 D7, ruling R13(a): an unattended start's link found the Printer
    /// no longer `unattended`. The write-ahead rolled back; nothing was
    /// sent. `START_PRECONDITION_CHANGED`.
    StartSafetyChanged { job_id: String, printer_id: String },
    /// P7 D5: the assign transaction's in-transaction `check_assignment`
    /// refused the pair. Nothing was written. `ASSIGNMENT_BLOCKED`.
    AssignmentBlocked {
        entry_id: String,
        printer_id: String,
        spool_id: String,
        blockers: Vec<crate::queue::Blocker>,
    },
    /// P7 D2: `retry_job` on a Job whose entry already has its successor
    /// `retry_entry_id`. Nothing was written. `JOB_ALREADY_RETRIED`.
    JobAlreadyRetried {
        job_id: String,
        retry_entry_id: String,
    },
    /// P7 settlement: a second `settle_job_material` on an already-`settled`
    /// Job (`reason: settled`), or a second `correct_job_material`
    /// (`reason: corrected`). Nothing was written. `JOB_ALREADY_SETTLED`.
    JobAlreadySettled {
        job_id: String,
        reason: crate::jobs::SettleFailureReason,
    },
    /// P7 D8: `import_printers` (`replace_all`) while any Job exists, or
    /// any open Queue Entry is pinned to a Printer (`manual_printer_id`).
    /// `replace_all` deletes every Printer, and `jobs.printer_id` is `ON
    /// DELETE RESTRICT`, so any Job -- even a finished one -- would make
    /// the delete fail mid-import; this is checked up front instead.
    /// Nothing was written. `JOBS_EXIST`. Each list is capped at 20.
    JobsExist {
        printer_ids: Vec<String>,
        job_ids: Vec<String>,
        queue_entry_ids: Vec<String>,
    },
    /// P7 D8: a `spools::reservations` primitive refused inside a Job
    /// transaction. Carries what the caller knows beyond the primitive's
    /// own error, so the command's message and `details` can name the
    /// Spool (`#<n>`), the amount it needed, and the reservation.
    Reservation {
        spool_id: String,
        spool_number: Option<i64>,
        reservation_id: Option<String>,
        required_mg: Option<i64>,
        error: crate::spools::reservations::ReservationError,
    },
    /// P8 D2 "Lifecycle rules": `resolve_attention_event` on an Event
    /// whose `resolution_mode` isn't `manual`
    /// (`attention::lifecycle::LifecycleError::NotManual`). Nothing was
    /// written. `ATTENTION_NOT_MANUAL`.
    AttentionNotManual {
        event_id: String,
        condition: crate::attention::ConditionKind,
        resolution_mode: crate::attention::ResolutionMode,
    },
    /// P8 D5: `snapshot_image`, or `set_snapshot_pinned(true)`, on a
    /// pruned snapshot. `EVIDENCE_PRUNED`.
    EvidencePruned {
        snapshot_id: String,
        reason: crate::cameras::PruneReason,
    },
    /// P8 D5: `capture_snapshot` when only pinned snapshots would be left
    /// to prune. `SNAPSHOT_DISK_CAP`.
    SnapshotDiskCap {
        used_bytes: i64,
        cap_bytes: i64,
        pinned_bytes: i64,
    },
    Storage(StorageError),
}

impl From<StorageError> for RepositoryError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}

impl From<rusqlite::Error> for RepositoryError {
    fn from(error: rusqlite::Error) -> Self {
        Self::Storage(error.into())
    }
}

impl From<std::io::Error> for StorageError {
    fn from(_: std::io::Error) -> Self {
        Self::Filesystem
    }
}

#[cfg(test)]
mod tests {
    use super::StorageError;

    #[test]
    fn sqlite_corruption_and_not_a_database_are_corrupt_data() {
        for code in [rusqlite::ffi::SQLITE_CORRUPT, rusqlite::ffi::SQLITE_NOTADB] {
            let sqlite = rusqlite::ffi::Error::new(code);
            let mapped = StorageError::from(rusqlite::Error::SqliteFailure(sqlite, None));

            assert!(matches!(
                mapped,
                StorageError::CorruptData {
                    source_name: "database",
                    source_sha256: None
                }
            ));
        }
    }
}
