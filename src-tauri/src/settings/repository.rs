use std::sync::Arc;

use rusqlite::params;

use crate::persistence::{RepositoryError, Storage, StorageError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsRecord {
    pub revision: i64,
    pub theme_mode: String,
    pub updated_at: String,
}

pub struct SettingsRepository {
    storage: Arc<Storage>,
}

impl SettingsRepository {
    pub fn new(storage: Arc<Storage>) -> Self {
        Self { storage }
    }

    pub fn ensure_default(&self) -> Result<SettingsRecord, StorageError> {
        self.storage.write(|transaction| {
            transaction.execute(
                "INSERT OR IGNORE INTO settings(singleton_id, revision, theme_mode, updated_at) VALUES (1, 1, 'system', ?1)",
                [crate::printers::now_rfc3339()],
            )?;
            load(transaction)
        })
    }

    pub fn load(&self) -> Result<SettingsRecord, StorageError> {
        self.storage.read(load_sql)
    }

    pub fn save(
        &self,
        expected_revision: i64,
        theme_mode: &str,
    ) -> Result<SettingsRecord, RepositoryError> {
        if expected_revision <= 0 {
            return Err(RepositoryError::Validation {
                field_path: "expectedRevision",
            });
        }
        if theme_mode.is_empty() {
            return Err(RepositoryError::Validation {
                field_path: "themeMode",
            });
        }
        self.storage.write(|transaction| {
            let current_revision = transaction.query_row(
                "SELECT revision FROM settings WHERE singleton_id = 1",
                [],
                |row| row.get::<_, i64>(0),
            )?;
            if current_revision != expected_revision {
                return Err(StorageError::OperationFailed);
            }
            let changed = transaction.execute(
                "UPDATE settings SET revision = revision + 1, theme_mode = ?1, updated_at = ?2 WHERE singleton_id = 1 AND revision = ?3",
                params![theme_mode, crate::printers::now_rfc3339(), expected_revision],
            )?;
            if changed != 1 {
                return Err(StorageError::OperationFailed);
            }
            load(transaction)
        }).map_err(|error| match error {
            StorageError::OperationFailed => RepositoryError::Conflict {
                entity_id: "settings".to_string(),
                expected_revision,
                current_revision: self.load().map(|value| value.revision).unwrap_or(expected_revision),
            },
            other => RepositoryError::Storage(other),
        })
    }
}

fn load(connection: &rusqlite::Connection) -> Result<SettingsRecord, StorageError> {
    load_sql(connection).map_err(StorageError::from)
}

fn load_sql(connection: &rusqlite::Connection) -> rusqlite::Result<SettingsRecord> {
    connection.query_row(
        "SELECT revision, theme_mode, updated_at FROM settings WHERE singleton_id = 1",
        [],
        |row| {
            Ok(SettingsRecord {
                revision: row.get(0)?,
                theme_mode: row.get(1)?,
                updated_at: row.get(2)?,
            })
        },
    )
}
