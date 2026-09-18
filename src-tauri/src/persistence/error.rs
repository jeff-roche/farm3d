use std::fmt;

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
