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
    /// D7: `archive`/`unarchive`/`delete` is blocked by the Printer's
    /// current lifecycle state (or, in a later phase, other work that still
    /// depends on it). See `crate::printers::lifecycle::evaluate`.
    LifecycleBlocked(Vec<LifecycleBlocker>),
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
