use std::sync::Arc;

use rusqlite::params;

use super::commands::{MonitorDensity, MonitorSection};
use crate::persistence::{RepositoryError, Storage, StorageError};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SettingsRecord {
    pub revision: i64,
    pub theme_mode: String,
    pub monitor_section: MonitorSection,
    pub monitor_density: MonitorDensity,
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
        monitor_section: MonitorSection,
        monitor_density: MonitorDensity,
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
                "UPDATE settings SET revision = revision + 1, theme_mode = ?1, monitor_section = ?2, monitor_density = ?3, updated_at = ?4 WHERE singleton_id = 1 AND revision = ?5",
                params![theme_mode, monitor_section.as_str(), monitor_density.as_str(), crate::printers::now_rfc3339(), expected_revision],
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
        "SELECT revision, theme_mode, monitor_section, monitor_density, updated_at FROM settings WHERE singleton_id = 1",
        [],
        |row| {
            Ok(SettingsRecord {
                revision: row.get(0)?,
                theme_mode: row.get(1)?,
                monitor_section: parse_monitor_section(row.get(2)?, 2)?,
                monitor_density: parse_monitor_density(row.get(3)?, 3)?,
                updated_at: row.get(4)?,
            })
        },
    )
}

fn parse_monitor_section(value: String, index: usize) -> rusqlite::Result<MonitorSection> {
    match value.as_str() {
        "location" => Ok(MonitorSection::Location),
        "printerModel" => Ok(MonitorSection::PrinterModel),
        "operationalState" => Ok(MonitorSection::OperationalState),
        "none" => Ok(MonitorSection::None),
        _ => Err(invalid_monitor_preference(index)),
    }
}

fn parse_monitor_density(value: String, index: usize) -> rusqlite::Result<MonitorDensity> {
    match value.as_str() {
        "comfortable" => Ok(MonitorDensity::Comfortable),
        "compact" => Ok(MonitorDensity::Compact),
        _ => Err(invalid_monitor_preference(index)),
    }
}

fn invalid_monitor_preference(index: usize) -> rusqlite::Error {
    rusqlite::Error::FromSqlConversionFailure(
        index,
        rusqlite::types::Type::Text,
        Box::new(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "invalid monitor preference",
        )),
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::persistence::{MetadataRootLease, Storage, StoragePaths};
    use crate::settings::commands::{MonitorDensity, MonitorSection};

    use super::SettingsRepository;

    fn storage() -> (tempfile::TempDir, MetadataRootLease, Arc<Storage>) {
        let temp = tempfile::tempdir().expect("temporary root");
        let paths = StoragePaths::new(temp.path().join("metadata"), temp.path().join("data"))
            .expect("storage paths");
        let lease = MetadataRootLease::acquire(&paths).expect("metadata lease");
        let storage = Arc::new(Storage::open(paths, &lease).expect("storage"));
        (temp, lease, storage)
    }

    #[test]
    fn monitor_preferences_default_round_trip_and_preserve_revisions() {
        let (_temp, _lease, storage) = storage();
        let repository = SettingsRepository::new(storage);

        let settings = repository.ensure_default().expect("default settings");
        assert_eq!(settings.monitor_section, MonitorSection::PrinterModel);
        assert_eq!(settings.monitor_density, MonitorDensity::Comfortable);

        let saved = repository
            .save(
                settings.revision,
                "farm3d-dark",
                MonitorSection::OperationalState,
                MonitorDensity::Compact,
            )
            .expect("saved settings");
        assert_eq!(saved.revision, 2);
        assert_eq!(saved.monitor_section, MonitorSection::OperationalState);
        assert_eq!(saved.monitor_density, MonitorDensity::Compact);
        assert!(repository
            .save(
                settings.revision,
                "system",
                MonitorSection::Location,
                MonitorDensity::Comfortable,
            )
            .is_err());
    }

    #[test]
    fn monitor_preference_saves_validate_existing_inputs() {
        let (_temp, _lease, storage) = storage();
        let repository = SettingsRepository::new(storage);
        repository.ensure_default().expect("default settings");

        assert!(repository
            .save(
                0,
                "system",
                MonitorSection::PrinterModel,
                MonitorDensity::Comfortable,
            )
            .is_err());
        assert!(repository
            .save(
                1,
                "",
                MonitorSection::PrinterModel,
                MonitorDensity::Comfortable,
            )
            .is_err());
    }
}
