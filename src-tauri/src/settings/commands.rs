use std::sync::Arc;

use serde::{Deserialize, Serialize};
use ts_rs::TS;

use crate::contracts::command::{
    CommandError, CommandSuccess, DesktopRequiredReason, IncomingContractVersion,
};
use crate::document_io::DocumentKind;
use crate::persistence::{SnapshotKind, Storage};

use super::repository::{SettingsRecord as PersistedSettingsRecord, SettingsRepository};

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
    updated_at: String,
}

impl From<PersistedSettingsRecord> for SettingsRecord {
    fn from(value: PersistedSettingsRecord) -> Self {
        Self {
            revision: value.revision,
            theme_mode: value.theme_mode,
            monitor_section: value.monitor_section,
            monitor_density: value.monitor_density,
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
}

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
    if version > 2 {
        return Err(CommandError::unsupported_schema(version));
    }
    if !(1..=2).contains(&version) {
        return Err(CommandError::validation("schemaVersion must be 1 or 2."));
    }
    let document: ImportedSettingsDocument = serde_json::from_value(raw).map_err(|_| {
        CommandError::validation("The selected Settings document has an invalid shape.")
    })?;
    if document.settings.theme_mode.is_empty() {
        return Err(CommandError::validation("themeMode is required."));
    }
    Ok(document)
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
) -> Result<CommandSuccess<SettingsRecord>, CommandError> {
    contract_version.validate()?;
    let services = bootstrap.ready()?;
    repository(&services.storage)
        .save(
            expected_revision,
            &theme_mode,
            monitor_section,
            monitor_density,
        )
        .map(|record| CommandSuccess::new(record.into()))
        .map_err(CommandError::from_repository)
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
        schema_version: 2,
        exported_at: &exported_at,
        settings: SettingsData {
            theme_mode: &record.theme_mode,
            monitor_section: record.monitor_section,
            monitor_density: record.monitor_density,
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
    let settings = repository(&services.storage)
        .save(
            expected_revision,
            &document.settings.theme_mode,
            document.settings.monitor_section,
            document.settings.monitor_density,
        )
        .map_err(CommandError::from_repository)?;
    Ok(CommandSuccess::new(SettingsImportResult::Applied {
        settings: settings.into(),
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
                br#"{"schemaVersion":3,"exportedAt":"x","settings":{"themeMode":"system"}}"#
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
    fn settings_export_uses_schema_two_with_explicit_monitor_preferences() {
        let document = SettingsDocument {
            schema_version: 2,
            exported_at: "2026-09-18T00:00:00Z",
            settings: SettingsData {
                theme_mode: "farm3d-dark",
                monitor_section: MonitorSection::OperationalState,
                monitor_density: MonitorDensity::Compact,
            },
        };

        assert_eq!(
            serde_json::to_value(document).expect("settings document"),
            serde_json::json!({
                "schemaVersion": 2,
                "exportedAt": "2026-09-18T00:00:00Z",
                "settings": {
                    "themeMode": "farm3d-dark",
                    "monitorSection": "operationalState",
                    "monitorDensity": "compact"
                }
            })
        );
    }
}
