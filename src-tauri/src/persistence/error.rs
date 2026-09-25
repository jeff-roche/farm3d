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
