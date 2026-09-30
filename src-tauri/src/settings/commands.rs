use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contracts::command::{
    CommandError, CommandSuccess, DesktopRequiredReason, IncomingContractVersion,
};
use crate::document_io::DocumentKind;
use crate::notifications::NotificationClassSettings;
use crate::persistence::{RepositoryError, SnapshotKind, Storage};

use super::repository::{
    SavedSettings, SettingsRecord as PersistedSettingsRecord, SettingsRepository, SettingsUpdate,
};

/// P8 D5: how long unpinned snapshots are kept, and the media store's
/// disk cap in MiB (`settings.snapshot_retention_days` /
/// `snapshot_disk_cap_mb`). `SettingsRecord.snapshotRetention` on the
/// wire.
#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
#[ts(rename_all = "camelCase", export_to = "domain/SnapshotRetention.ts")]
pub struct SnapshotRetention {
    #[ts(type = "number")]
    pub retention_days: i64,
    #[ts(type = "number")]
    pub disk_cap_mb: i64,
}

impl Default for SnapshotRetention {
    /// Decision 11: 30 days, 2 GiB.
    fn default() -> Self {
        Self {
            retention_days: 30,
            disk_cap_mb: 2048,
        }
    }
}

impl SnapshotRetention {
    pub const DAYS: std::ops::RangeInclusive<i64> = 1..=365;
    pub const DISK_CAP_MB: std::ops::RangeInclusive<i64> = 100..=102_400;

    /// `VALIDATION` on the out-of-range field.
    pub fn validate(&self) -> Result<(), RepositoryError> {
        if !Self::DAYS.contains(&self.retention_days) {
            return Err(RepositoryError::Validation {
                field_path: "snapshotRetention.retentionDays",
            });
        }
        if !Self::DISK_CAP_MB.contains(&self.disk_cap_mb) {
            return Err(RepositoryError::Validation {
                field_path: "snapshotRetention.diskCapMb",
            });
        }
        Ok(())
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/MonitorSection.ts")]
pub enum MonitorSection {
    Location,
    PrinterModel,
    OperationalState,
    None,
}

impl MonitorSection {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Location => "location",
            Self::PrinterModel => "printerModel",
            Self::OperationalState => "operationalState",
            Self::None => "none",
        }
    }
}

impl Default for MonitorSection {
    fn default() -> Self {
        Self::PrinterModel
    }
}

#[derive(Serialize, Deserialize, Clone, Copy, PartialEq, Eq, Debug, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/MonitorDensity.ts")]
pub enum MonitorDensity {
    Comfortable,
    Compact,
}

impl MonitorDensity {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Comfortable => "comfortable",
            Self::Compact => "compact",
        }
    }
}

impl Default for MonitorDensity {
    fn default() -> Self {
        Self::Comfortable
    }
}

#[derive(Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "domain/SettingsRecord.ts")]
pub struct SettingsRecord {
    #[ts(type = "number")]
    revision: i64,
    theme_mode: String,
    monitor_section: MonitorSection,
    monitor_density: MonitorDensity,
    notifications: NotificationClassSettings,
    snapshot_retention: SnapshotRetention,
    updated_at: String,
}

impl From<PersistedSettingsRecord> for SettingsRecord {
    fn from(value: PersistedSettingsRecord) -> Self {
        Self {
            revision: value.revision,
            theme_mode: value.theme_mode,
            monitor_section: value.monitor_section,
            monitor_density: value.monitor_density,
            notifications: value.notifications,
            snapshot_retention: value.snapshot_retention,
            updated_at: value.updated_at,
        }
    }
}

#[derive(Serialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename = "SettingsExportOutcome",
    export_to = "command/SettingsExportOutcome.ts"
)]
pub enum ExportResult {
    Cancelled,
    Exported {
        #[serde(rename = "exportedAt")]
        #[ts(rename = "exportedAt")]
        exported_at: String,
        #[serde(rename = "recordCount")]
        #[ts(rename = "recordCount")]
        record_count: usize,
    },
    Unsupported {
        #[ts(type = "\"desktopRequired\"")]
        reason: DesktopRequiredReason,
    },
}

#[derive(Serialize, TS)]
#[serde(tag = "status", rename_all = "camelCase")]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename = "SettingsImportOutcome",
    export_to = "command/SettingsImportOutcome.ts"
)]
pub enum SettingsImportResult {
    Cancelled,
    Applied {
        settings: SettingsRecord,
        warnings: Vec<crate::printers::commands::OperationWarning>,
    },
    Unsupported {
        #[ts(type = "\"desktopRequired\"")]
        reason: DesktopRequiredReason,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsDocument<'a> {
    schema_version: u8,
    exported_at: &'a str,
    settings: SettingsData<'a>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct SettingsData<'a> {
    theme_mode: &'a str,
    monitor_section: MonitorSection,
    monitor_density: MonitorDensity,
    notifications: NotificationClassSettings,
    snapshot_retention: SnapshotRetention,
}

/// The Settings export's current schema (P8: 3 adds `notifications` and
/// `snapshotRetention`).
const SETTINGS_SCHEMA_VERSION: u8 = 3;

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportedSettingsDocument {
    #[allow(dead_code)]
    schema_version: i64,
    #[allow(dead_code)]
    exported_at: String,
    settings: ImportedSettings,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportedSettings {
    theme_mode: String,
    #[serde(default)]
    monitor_section: MonitorSection,
    #[serde(default)]
    monitor_density: MonitorDensity,
    /// Schema 3; a missing field (or object) takes decision 7's default.
    #[serde(default)]
    notifications: Option<ImportedNotificationClasses>,
    /// Schema 3; missing takes decision 11's defaults.
    #[serde(default)]
    snapshot_retention: Option<ImportedSnapshotRetention>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportedNotificationClasses {
    fatal: Option<bool>,
    confirmation: Option<bool>,
    completion: Option<bool>,
    reconciliation: Option<bool>,
    connectivity: Option<bool>,
    inventory: Option<bool>,
}

impl ImportedNotificationClasses {
    fn or_defaults(&self) -> NotificationClassSettings {
        let defaults = NotificationClassSettings::default();
        NotificationClassSettings {
            fatal: self.fatal.unwrap_or(defaults.fatal),
            confirmation: self.confirmation.unwrap_or(defaults.confirmation),
            completion: self.completion.unwrap_or(defaults.completion),
            reconciliation: self.reconciliation.unwrap_or(defaults.reconciliation),
            connectivity: self.connectivity.unwrap_or(defaults.connectivity),
            inventory: self.inventory.unwrap_or(defaults.inventory),
        }
    }
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
struct ImportedSnapshotRetention {
    retention_days: Option<i64>,
    disk_cap_mb: Option<i64>,
}

impl ImportedSnapshotRetention {
    fn or_defaults(&self) -> SnapshotRetention {
        let defaults = SnapshotRetention::default();
        SnapshotRetention {
            retention_days: self.retention_days.unwrap_or(defaults.retention_days),
            disk_cap_mb: self.disk_cap_mb.unwrap_or(defaults.disk_cap_mb),
        }
    }
}

impl ImportedSettings {
    /// What an import writes: every field, a missing one taking its
    /// default (so a schema 1 or 2 document resets the P8 settings).
    fn classes(&self) -> NotificationClassSettings {
        self.notifications
            .as_ref()
            .map(ImportedNotificationClasses::or_defaults)
            .unwrap_or_default()
    }

    fn retention(&self) -> SnapshotRetention {
        self.snapshot_retention
            .as_ref()
            .map(ImportedSnapshotRetention::or_defaults)
            .unwrap_or_default()
    }
}

fn parse_settings_document(bytes: &[u8]) -> Result<ImportedSettingsDocument, CommandError> {
    if bytes.len() > 1024 * 1024 {
        return Err(CommandError::validation(
            "The Settings document is too large.",
        ));
    }
    let raw: serde_json::Value = serde_json::from_slice(bytes)
        .map_err(|_| CommandError::corrupt_import("settingsImport"))?;
    let version = raw
        .get("schemaVersion")
        .and_then(serde_json::Value::as_i64)
        .ok_or_else(|| CommandError::validation("schemaVersion must be a positive integer."))?;
    if version > i64::from(SETTINGS_SCHEMA_VERSION) {
        return Err(CommandError::unsupported_schema(version));
    }
    if !(1..=i64::from(SETTINGS_SCHEMA_VERSION)).contains(&version) {
        return Err(CommandError::validation(
            "schemaVersion must be 1, 2, or 3.",
        ));
    }
    let document: ImportedSettingsDocument = serde_json::from_value(raw).map_err(|_| {
        CommandError::validation("The selected Settings document has an invalid shape.")
    })?;
    if document.settings.theme_mode.is_empty() {
        return Err(CommandError::validation("themeMode is required."));
    }
    // The P8 fields are schema 3's; an older document naming them is not
    // a document farm3d wrote.
    if version < 3
        && (document.settings.notifications.is_some()
            || document.settings.snapshot_retention.is_some())
    {
        return Err(CommandError::validation(
            "The selected Settings document has an invalid shape.",
        ));
    }
    document
        .settings
        .retention()
        .validate()
        .map_err(CommandError::from_repository)?;
    Ok(document)
}

/// P9 D15 tier (a): every setting at the default `ensure_default` and the
/// column defaults define: `theme_mode 'system'`, `monitor_section
/// 'printerModel'`, `monitor_density 'comfortable'`, notify classes
/// `1,1,1,0,0,0`, 30 days, 2048 MiB. Written through the save path.
pub(crate) fn default_update() -> SettingsUpdate<'static> {
    SettingsUpdate {
        theme_mode: "system",
        monitor_section: MonitorSection::default(),
        monitor_density: MonitorDensity::default(),
        notifications: Some(NotificationClassSettings::default()),
        snapshot_retention: Some(SnapshotRetention::default()),
    }
}

fn repository(storage: &Arc<Storage>) -> SettingsRepository {
    SettingsRepository::new(Arc::clone(storage))
}

#[tauri::command]
pub fn load_settings<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<SettingsRecord>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    repository(&services.storage)
        .load()
        .map(|record| CommandSuccess::new(record.into()))
        .map_err(|error| CommandError::from_repository(error.into()))
}

#[tauri::command]
pub fn save_settings<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: tauri::State<crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
    theme_mode: String,
    monitor_section: MonitorSection,
    monitor_density: MonitorDensity,
    notifications: Option<NotificationClassSettings>,
    snapshot_retention: Option<SnapshotRetention>,
) -> Result<CommandSuccess<SettingsRecord>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let saved = repository(&services.storage)
        .save_update(
            expected_revision,
            &SettingsUpdate {
                theme_mode: &theme_mode,
                monitor_section,
                monitor_density,
                notifications,
                snapshot_retention,
            },
        )
        .map_err(CommandError::from_repository)?;
    Ok(CommandSuccess::new(after_save(&services, saved)))
}

/// After a committed settings write: a retention change asks the
/// `MediaJanitor` for a prune pass (P8 D5).
fn after_save<R: tauri::Runtime>(
    services: &crate::RuntimeServices<R>,
    saved: SavedSettings,
) -> SettingsRecord {
    if saved.retention_changed {
        services.cameras.janitor().poke();
    }
    saved.record.into()
}

#[tauri::command]
pub async fn export_settings<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
) -> Result<CommandSuccess<ExportResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let Some(path) = services.documents.save_json(DocumentKind::Settings)? else {
        return Ok(CommandSuccess::new(ExportResult::Cancelled));
    };
    let record = repository(&services.storage)
        .load()
        .map_err(|error| CommandError::from_repository(error.into()))?;
    let exported_at = crate::printers::now_rfc3339();
    let document = SettingsDocument {
        schema_version: SETTINGS_SCHEMA_VERSION,
        exported_at: &exported_at,
        settings: SettingsData {
            theme_mode: &record.theme_mode,
            monitor_section: record.monitor_section,
            monitor_density: record.monitor_density,
            notifications: record.notifications,
            snapshot_retention: record.snapshot_retention,
        },
    };
    let bytes = serde_json::to_vec_pretty(&document).map_err(|_| CommandError::internal())?;
    services.documents.atomic_write(&path, &bytes)?;
    Ok(CommandSuccess::new(ExportResult::Exported {
        exported_at,
        record_count: 1,
    }))
}

#[tauri::command]
pub async fn import_settings<R: tauri::Runtime>(
    _app: tauri::AppHandle<R>,
    bootstrap: tauri::State<'_, crate::bootstrap::BootstrapState<crate::RuntimeServices<R>>>,
    contract_version: IncomingContractVersion,
    expected_revision: i64,
) -> Result<CommandSuccess<SettingsImportResult>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    let Some(path) = services.documents.open_json(DocumentKind::Settings)? else {
        return Ok(CommandSuccess::new(SettingsImportResult::Cancelled));
    };
    let bytes = services.documents.read(&path)?;
    let document = parse_settings_document(&bytes)?;
    let current = repository(&services.storage)
        .load()
        .map_err(|error| CommandError::from_repository(error.into()))?;
    if current.revision != expected_revision {
        return Err(CommandError::revision_conflict(
            "settings",
            expected_revision,
            current.revision,
        ));
    }
    services
        .storage
        .create_snapshot(SnapshotKind::Settings)
        .map_err(|_| CommandError::persistence_unavailable())?;
    services.documents.after_snapshot(DocumentKind::Settings)?;
    let saved = repository(&services.storage)
        .save_update(
            expected_revision,
            &SettingsUpdate {
                theme_mode: &document.settings.theme_mode,
                monitor_section: document.settings.monitor_section,
                monitor_density: document.settings.monitor_density,
                notifications: Some(document.settings.classes()),
                snapshot_retention: Some(document.settings.retention()),
            },
        )
        .map_err(CommandError::from_repository)?;
    Ok(CommandSuccess::new(SettingsImportResult::Applied {
        settings: after_save(&services, saved),
        warnings: vec![],
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::contracts::command::ErrorCode;

    #[test]
    fn settings_import_distinguishes_corrupt_future_and_invalid_documents() {
        assert_eq!(
            parse_settings_document(b"not json").unwrap_err().code,
            ErrorCode::CorruptData
        );
        assert_eq!(
            parse_settings_document(
                br#"{"schemaVersion":4,"exportedAt":"x","settings":{"themeMode":"system"}}"#
            )
            .unwrap_err()
            .code,
            ErrorCode::UnsupportedSchemaVersion
        );
        assert_eq!(
            parse_settings_document(
                br#"{"schemaVersion":1,"exportedAt":"x","settings":{"themeMode":""}}"#
            )
            .unwrap_err()
            .code,
            ErrorCode::Validation
        );
    }

    #[test]
    fn settings_import_accepts_schema_one_defaults_and_schema_two_preferences() {
        let document = parse_settings_document(
            br#"{"schemaVersion":1,"exportedAt":"x","settings":{"themeMode":"farm3d-dark"}}"#,
        )
        .unwrap();
        assert_eq!(document.settings.theme_mode, "farm3d-dark");
        assert_eq!(
            document.settings.monitor_section,
            MonitorSection::PrinterModel
        );
        assert_eq!(
            document.settings.monitor_density,
            MonitorDensity::Comfortable
        );

        let document = parse_settings_document(
            br#"{"schemaVersion":2,"exportedAt":"x","settings":{"themeMode":"farm3d-dark","monitorSection":"operationalState","monitorDensity":"compact"}}"#,
        )
        .unwrap();
        assert_eq!(
            document.settings.monitor_section,
            MonitorSection::OperationalState
        );
        assert_eq!(document.settings.monitor_density, MonitorDensity::Compact);
        assert!(parse_settings_document(br#"{"schemaVersion":2,"exportedAt":"x","settings":{"themeMode":"system","monitorSection":"invalid","monitorDensity":"comfortable"}}"#).is_err());
        assert!(parse_settings_document(br#"{"schemaVersion":1,"exportedAt":"x","settings":{"themeMode":"system","extra":true}}"#).is_err());
    }

    #[test]
    fn settings_export_uses_schema_three_with_the_notification_and_retention_settings() {
        let document = SettingsDocument {
            schema_version: SETTINGS_SCHEMA_VERSION,
            exported_at: "2026-09-18T00:00:00Z",
            settings: SettingsData {
                theme_mode: "farm3d-dark",
                monitor_section: MonitorSection::OperationalState,
                monitor_density: MonitorDensity::Compact,
                notifications: NotificationClassSettings::default(),
                snapshot_retention: SnapshotRetention {
                    retention_days: 7,
                    disk_cap_mb: 512,
                },
            },
        };

        assert_eq!(
            serde_json::to_value(document).expect("settings document"),
            serde_json::json!({
                "schemaVersion": 3,
                "exportedAt": "2026-09-18T00:00:00Z",
                "settings": {
                    "themeMode": "farm3d-dark",
                    "monitorSection": "operationalState",
                    "monitorDensity": "compact",
                    "notifications": {
                        "fatal": true, "confirmation": true, "completion": true,
                        "reconciliation": false, "connectivity": false, "inventory": false
                    },
                    "snapshotRetention": { "retentionDays": 7, "diskCapMb": 512 }
                }
            })
        );
    }

    #[test]
    fn settings_import_accepts_schema_three_and_defaults_the_p8_fields_before_it() {
        let document = parse_settings_document(
            br#"{"schemaVersion":3,"exportedAt":"x","settings":{"themeMode":"system","notifications":{"fatal":false,"inventory":true},"snapshotRetention":{"retentionDays":9}}}"#,
        )
        .unwrap();
        assert_eq!(
            document.settings.classes(),
            NotificationClassSettings {
                fatal: false,
                inventory: true,
                ..NotificationClassSettings::default()
            }
        );
        assert_eq!(
            document.settings.retention(),
            SnapshotRetention {
                retention_days: 9,
                disk_cap_mb: 2048
            }
        );
        let document = parse_settings_document(
            br#"{"schemaVersion":2,"exportedAt":"x","settings":{"themeMode":"system"}}"#,
        )
        .unwrap();
        assert_eq!(
            document.settings.classes(),
            NotificationClassSettings::default()
        );
        assert_eq!(document.settings.retention(), SnapshotRetention::default());
        // Schema 2 can't carry the schema 3 fields; unknown fields stay out.
        for bad in [
            br#"{"schemaVersion":2,"exportedAt":"x","settings":{"themeMode":"system","notifications":{}}}"#.as_slice(),
            br#"{"schemaVersion":3,"exportedAt":"x","settings":{"themeMode":"system","notifications":{"loud":true}}}"#,
            br#"{"schemaVersion":3,"exportedAt":"x","settings":{"themeMode":"system","snapshotRetention":{"retentionDays":0}}}"#,
            br#"{"schemaVersion":3,"exportedAt":"x","settings":{"themeMode":"system","snapshotRetention":{"diskCapMb":99}}}"#,
        ] {
            assert_eq!(
                parse_settings_document(bad).unwrap_err().code,
                ErrorCode::Validation,
                "{}",
                String::from_utf8_lossy(bad)
            );
        }
    }
}
