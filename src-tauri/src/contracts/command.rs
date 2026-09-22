use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};
use serde_json::value::RawValue;
use ts_rs::TS;

use super::{ContractVersion, JS_MAX_SAFE_INTEGER};

#[derive(Clone, Copy, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum DesktopRequiredReason {
    DesktopRequired,
}

#[derive(Clone, Copy, Debug, Deserialize)]
#[serde(transparent)]
pub struct IncomingContractVersion(pub i64);

impl IncomingContractVersion {
    pub fn validate(self) -> Result<(), CommandError> {
        if self.0 == 1 {
            Ok(())
        } else {
            Err(CommandError::incompatible_contract(self.0))
        }
    }
}

/// Why a numeric value cannot cross the JavaScript JSON boundary losslessly.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum JsonNumberError {
    NonFinite,
    OutsideSafeRange,
    PrecisionLoss,
}

impl std::fmt::Display for JsonNumberError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NonFinite => formatter.write_str("JSON numbers must be finite"),
            Self::OutsideSafeRange => {
                formatter.write_str("JSON numbers must be within JavaScript's safe integer range")
            }
            Self::PrecisionLoss => formatter.write_str(
                "JSON number cannot cross the JavaScript boundary without precision loss",
            ),
        }
    }
}

impl std::error::Error for JsonNumberError {}

/// A JSON number validated for the Rust-to-TypeScript boundary.
///
/// Integer values must remain within `-(2^53 - 1)..=(2^53 - 1)`, the exact
/// integer range of a TypeScript `number`. Fractional and exponent forms are
/// accepted only when parsing as `f64` and serializing through `serde_json`
/// reproduces the original numeric text exactly. Deserialization captures that
/// text before converting through `serde_json::Number`. This deliberately
/// conservative canonical subset rejects precision loss and alternate numeric
/// spellings.
#[derive(Clone, Debug, PartialEq, TS)]
#[ts(type = "number", export_to = "command/JsonNumber.ts")]
pub struct JsonNumber(serde_json::Number);

impl TryFrom<i64> for JsonNumber {
    type Error = JsonNumberError;

    fn try_from(value: i64) -> Result<Self, Self::Error> {
        if value.unsigned_abs() > JS_MAX_SAFE_INTEGER {
            return Err(JsonNumberError::OutsideSafeRange);
        }
        Ok(Self(value.into()))
    }
}

impl TryFrom<u64> for JsonNumber {
    type Error = JsonNumberError;

    fn try_from(value: u64) -> Result<Self, Self::Error> {
        if value > JS_MAX_SAFE_INTEGER {
            return Err(JsonNumberError::OutsideSafeRange);
        }
        Ok(Self(value.into()))
    }
}

impl TryFrom<f64> for JsonNumber {
    type Error = JsonNumberError;

    fn try_from(value: f64) -> Result<Self, Self::Error> {
        if !value.is_finite() {
            return Err(JsonNumberError::NonFinite);
        }
        if value.abs() > JS_MAX_SAFE_INTEGER as f64 {
            return Err(JsonNumberError::OutsideSafeRange);
        }
        let number = serde_json::Number::from_f64(value).ok_or(JsonNumberError::NonFinite)?;
        Self::try_from(number)
    }
}

impl TryFrom<serde_json::Number> for JsonNumber {
    type Error = JsonNumberError;

    fn try_from(value: serde_json::Number) -> Result<Self, Self::Error> {
        let original = value.to_string();
        Self::from_number_text(&original, value)
    }
}

impl JsonNumber {
    fn from_number_text(
        original: &str,
        value: serde_json::Number,
    ) -> Result<Self, JsonNumberError> {
        let is_fractional_or_exponent = original
            .bytes()
            .any(|byte| matches!(byte, b'.' | b'e' | b'E'));

        if !is_fractional_or_exponent {
            if let Some(integer) = value.as_i64() {
                JsonNumber::try_from(integer)?;
                return Ok(Self(value));
            }
            if let Some(integer) = value.as_u64() {
                JsonNumber::try_from(integer)?;
                return Ok(Self(value));
            }
            return Err(JsonNumberError::OutsideSafeRange);
        }

        let parsed = value.as_f64().ok_or(JsonNumberError::NonFinite)?;
        if parsed.abs() > JS_MAX_SAFE_INTEGER as f64 {
            return Err(JsonNumberError::OutsideSafeRange);
        }
        let canonical = serde_json::Number::from_f64(parsed)
            .ok_or(JsonNumberError::NonFinite)?
            .to_string();
        if original != canonical {
            return Err(JsonNumberError::PrecisionLoss);
        }

        Ok(Self(value))
    }

    fn from_json_text(original: &str) -> Result<Self, JsonNumberError> {
        let value = serde_json::from_str::<serde_json::Number>(original)
            .map_err(|_| JsonNumberError::NonFinite)?;
        Self::from_number_text(original, value)
    }
}

impl Serialize for JsonNumber {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        self.0.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for JsonNumber {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let raw = Box::<RawValue>::deserialize(deserializer)
            .map_err(|_| serde::de::Error::custom("invalid JSON number"))?;
        Self::from_json_text(raw.get()).map_err(serde::de::Error::custom)
    }
}

/// A recursive JSON value that generates a precise TypeScript union.
#[derive(Clone, Debug, PartialEq, Serialize, TS)]
#[serde(untagged)]
#[ts(untagged, export_to = "command/JsonValue.ts")]
pub enum JsonValue {
    Null(()),
    Bool(bool),
    Number(JsonNumber),
    String(String),
    Array(Vec<JsonValue>),
    Object(BTreeMap<String, JsonValue>),
}

impl<'de> Deserialize<'de> for JsonValue {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        fn convert<E>(raw: &RawValue) -> Result<JsonValue, E>
        where
            E: serde::de::Error,
        {
            let text = raw.get();
            match text.as_bytes().first() {
                Some(b'n') => serde_json::from_str::<()>(text)
                    .map(JsonValue::Null)
                    .map_err(|_| E::custom("invalid JSON value")),
                Some(b't' | b'f') => serde_json::from_str::<bool>(text)
                    .map(JsonValue::Bool)
                    .map_err(|_| E::custom("invalid JSON value")),
                Some(b'"') => serde_json::from_str::<String>(text)
                    .map(JsonValue::String)
                    .map_err(|_| E::custom("invalid JSON value")),
                Some(b'[') => serde_json::from_str::<Vec<Box<RawValue>>>(text)
                    .map_err(|_| E::custom("invalid JSON array"))?
                    .into_iter()
                    .map(|value| convert(value.as_ref()))
                    .collect::<Result<_, _>>()
                    .map(JsonValue::Array),
                Some(b'{') => serde_json::from_str::<BTreeMap<String, Box<RawValue>>>(text)
                    .map_err(|_| E::custom("invalid JSON object"))?
                    .into_iter()
                    .map(|(key, value)| convert(value.as_ref()).map(|value| (key, value)))
                    .collect::<Result<_, _>>()
                    .map(JsonValue::Object),
                Some(b'-' | b'0'..=b'9') => JsonNumber::from_json_text(text)
                    .map(JsonValue::Number)
                    .map_err(E::custom),
                _ => Err(E::custom("expected a JSON value")),
            }
        }

        let raw = Box::<RawValue>::deserialize(deserializer)
            .map_err(|_| serde::de::Error::custom("invalid JSON value"))?;
        convert(raw.as_ref())
    }
}

impl JsonValue {
    pub fn from_serde_value(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(&serde_json::to_vec(&value)?)
    }

    pub fn into_serde_value(self) -> serde_json::Value {
        match self {
            Self::Null(()) => serde_json::Value::Null,
            Self::Bool(value) => serde_json::Value::Bool(value),
            Self::Number(value) => serde_json::Value::Number(value.0),
            Self::String(value) => serde_json::Value::String(value),
            Self::Array(values) => {
                serde_json::Value::Array(values.into_iter().map(Self::into_serde_value).collect())
            }
            Self::Object(values) => serde_json::Value::Object(
                values
                    .into_iter()
                    .map(|(key, value)| (key, value.into_serde_value()))
                    .collect(),
            ),
        }
    }
}

/// An opaque identifier attached to unexpected internal errors.
#[derive(Clone, Debug, PartialEq, Eq, TS)]
#[ts(type = "string", export_to = "command/CorrelationId.ts")]
pub struct CorrelationId(pub String);

impl Serialize for CorrelationId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.0)
    }
}

impl<'de> Deserialize<'de> for CorrelationId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        String::deserialize(deserializer).map(Self)
    }
}

/// Machine-readable command failure categories.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "command/ErrorCode.ts"
)]
pub enum ErrorCode {
    Validation,
    NotFound,
    Conflict,
    PersistenceUnavailable,
    CorruptData,
    MigrationFailed,
    UnsupportedSchemaVersion,
    CredentialUnavailable,
    CredentialRequired,
    UnsupportedAdapter,
    PrinterUnreachable,
    AuthenticationFailed,
    ProtocolError,
    Timeout,
    IncompatibleContractVersion,
    Internal,
    DuplicateHost,
    LifecycleBlocked,
}

/// Actions the frontend can offer in response to a command failure.
#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
#[ts(
    rename_all = "SCREAMING_SNAKE_CASE",
    export_to = "command/RecoveryCode.ts"
)]
pub enum RecoveryCode {
    Retry,
    EditFields,
    Reload,
    ReenterCredential,
    ChooseSupportedAdapter,
    CheckConnection,
    CheckCredentials,
    RestartApplication,
    UpgradeFarm3d,
}

/// The versioned success envelope returned by every command.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/CommandSuccess.ts")]
pub struct CommandSuccess<T> {
    #[ts(type = "1")]
    pub contract_version: ContractVersion,
    pub data: T,
}

impl<T> CommandSuccess<T> {
    pub fn new(data: T) -> Self {
        Self {
            contract_version: ContractVersion::V1,
            data,
        }
    }
}

/// The structured error envelope returned by fallible commands.
#[derive(Clone, Debug, Deserialize, PartialEq, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "command/CommandError.ts")]
pub struct CommandError {
    #[ts(type = "1")]
    pub contract_version: ContractVersion,
    pub code: ErrorCode,
    pub message: String,
    pub recovery: Vec<RecoveryCode>,
    pub retryable: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub correlation_id: Option<CorrelationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub field_errors: Option<BTreeMap<String, String>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    pub details: Option<BTreeMap<String, JsonValue>>,
}

impl CommandError {
    fn typed(
        code: ErrorCode,
        message: impl Into<String>,
        recovery: Vec<RecoveryCode>,
        retryable: bool,
    ) -> Self {
        Self {
            contract_version: ContractVersion::V1,
            code,
            message: message.into(),
            recovery,
            retryable,
            correlation_id: None,
            field_errors: None,
            details: None,
        }
    }

    pub fn internal() -> Self {
        Self {
            contract_version: ContractVersion::V1,
            code: ErrorCode::Internal,
            message: "The operation could not be completed.".to_string(),
            recovery: vec![RecoveryCode::RestartApplication],
            retryable: false,
            correlation_id: Some(CorrelationId(format!("err-{}", uuid::Uuid::new_v4()))),
            field_errors: None,
            details: None,
        }
    }

    pub fn validation(message: impl Into<String>) -> Self {
        Self::typed(
            ErrorCode::Validation,
            message,
            vec![RecoveryCode::EditFields],
            false,
        )
    }

    pub fn validation_at(field_path: impl Into<String>, message: impl Into<String>) -> Self {
        let mut error = Self::validation(message);
        error.details = Some(BTreeMap::from([(
            "fieldPath".to_string(),
            JsonValue::String(field_path.into()),
        )]));
        error
    }

    pub fn not_found(entity_id: impl Into<String>) -> Self {
        let mut error = Self::typed(
            ErrorCode::NotFound,
            "The requested item was not found.",
            vec![RecoveryCode::Reload],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "entityId".to_string(),
            JsonValue::String(entity_id.into()),
        )]));
        error
    }

    pub fn conflict(message: impl Into<String>) -> Self {
        Self::typed(
            ErrorCode::Conflict,
            message,
            vec![RecoveryCode::Reload],
            false,
        )
    }

    pub fn revision_conflict(
        entity_id: impl Into<String>,
        expected_revision: i64,
        current_revision: i64,
    ) -> Self {
        let mut error = Self::conflict("The item changed; reload and try again.");
        error.details = Some(BTreeMap::from([
            ("entityId".to_string(), JsonValue::String(entity_id.into())),
            (
                "expectedRevision".to_string(),
                JsonValue::Number(
                    JsonNumber::try_from(expected_revision)
                        .expect("positive revisions are JS-safe"),
                ),
            ),
            (
                "currentRevision".to_string(),
                JsonValue::Number(
                    JsonNumber::try_from(current_revision).expect("positive revisions are JS-safe"),
                ),
            ),
        ]));
        error
    }

    pub fn set_conflict(expected_count: usize, current_count: usize) -> Self {
        let mut error = Self::conflict("Printers changed; reload and try again.");
        error.details = Some(BTreeMap::from([
            (
                "expectedCount".to_string(),
                JsonValue::Number(
                    JsonNumber::try_from(expected_count as u64)
                        .expect("bounded Printer count is JS-safe"),
                ),
            ),
            (
                "currentCount".to_string(),
                JsonValue::Number(
                    JsonNumber::try_from(current_count as u64)
                        .expect("bounded Printer count is JS-safe"),
                ),
            ),
        ]));
        error
    }

    pub fn unsupported_schema(received_version: i64) -> Self {
        let mut error = Self::typed(
            ErrorCode::UnsupportedSchemaVersion,
            "This document was created by a newer version of farm3d.",
            vec![RecoveryCode::UpgradeFarm3d],
            false,
        );
        error.details =
            Some(BTreeMap::from([
                (
                    "supportedVersion".to_string(),
                    JsonValue::Number(JsonNumber::try_from(1_i64).expect("constant is safe")),
                ),
                (
                    "receivedVersion".to_string(),
                    JsonValue::Number(JsonNumber::try_from(received_version).unwrap_or_else(
                        |_| JsonNumber::try_from(-1_i64).expect("constant is safe"),
                    )),
                ),
            ]));
        error
    }

    pub fn persistence_unavailable() -> Self {
        Self::typed(
            ErrorCode::PersistenceUnavailable,
            "Storage is temporarily unavailable.",
            vec![RecoveryCode::Retry],
            true,
        )
    }

    pub fn discovery_timeout() -> Self {
        Self::typed(
            ErrorCode::Timeout,
            "Printer discovery did not finish in time.",
            vec![RecoveryCode::CheckConnection, RecoveryCode::Retry],
            true,
        )
    }

    pub fn credential_unavailable(kind: &str) -> Self {
        let mut error = Self::typed(
            ErrorCode::CredentialUnavailable,
            "The credential store is temporarily unavailable.",
            vec![RecoveryCode::Retry],
            true,
        );
        error.details = Some(BTreeMap::from([(
            "credentialStoreKind".to_string(),
            JsonValue::String(kind.to_string()),
        )]));
        error
    }

    pub fn migration_failed() -> Self {
        Self::typed(
            ErrorCode::MigrationFailed,
            "farm3d could not validate its data schema. Preserve the data directory and contact support.",
            vec![],
            false,
        )
    }

    pub fn unsupported_adapter(adapter_kind: impl Into<String>) -> Self {
        let mut error = Self::typed(
            ErrorCode::UnsupportedAdapter,
            "This Connection kind is not supported.",
            vec![RecoveryCode::ChooseSupportedAdapter],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "adapterKind".to_string(),
            JsonValue::String(adapter_kind.into()),
        )]));
        error
    }

    pub fn credential_required(entity_id: impl Into<String>) -> Self {
        let mut error = Self::typed(
            ErrorCode::CredentialRequired,
            "A credential is required for this Connection.",
            vec![RecoveryCode::ReenterCredential],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "entityId".to_string(),
            JsonValue::String(entity_id.into()),
        )]));
        error
    }

    /// D3: another active Printer already owns this host identity.
    pub fn duplicate_host(conflicting_printer_id: &str) -> Self {
        let mut error = Self::typed(
            ErrorCode::DuplicateHost,
            "Another active Printer already uses this host and port.",
            vec![RecoveryCode::EditFields],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "conflictingPrinterId".to_string(),
            JsonValue::String(conflicting_printer_id.to_string()),
        )]));
        error
    }

    /// D7: an action (e.g. delete) is blocked by other work that still
    /// depends on this Printer.
    pub fn lifecycle_blocked(blockers: serde_json::Value) -> Self {
        let mut error = Self::typed(
            ErrorCode::LifecycleBlocked,
            "This Printer cannot be changed while other work depends on it.",
            vec![],
            false,
        );
        let blockers =
            JsonValue::from_serde_value(blockers).unwrap_or_else(|_| JsonValue::Array(Vec::new()));
        error.details = Some(BTreeMap::from([("blockers".to_string(), blockers)]));
        error
    }

    pub fn safe_network(
        code: ErrorCode,
        message: impl Into<String>,
        recovery: Vec<RecoveryCode>,
        retryable: bool,
        entity_id: &str,
        adapter_kind: &str,
    ) -> Self {
        let mut error = Self::typed(code, message, recovery, retryable);
        let mut details = BTreeMap::from([(
            "entityId".to_string(),
            JsonValue::String(entity_id.to_string()),
        )]);
        if code == ErrorCode::ProtocolError {
            details.insert(
                "adapterKind".to_string(),
                JsonValue::String(adapter_kind.to_string()),
            );
        }
        error.details = Some(details);
        error
    }

    pub fn incompatible_contract(received_version: i64) -> Self {
        let mut error = Self::typed(
            ErrorCode::IncompatibleContractVersion,
            "This command contract is not compatible with this version of farm3d.",
            vec![RecoveryCode::UpgradeFarm3d],
            false,
        );
        error.details =
            Some(BTreeMap::from([
                (
                    "supportedVersion".to_string(),
                    JsonValue::Number(JsonNumber::try_from(1_i64).expect("constant is safe")),
                ),
                (
                    "receivedVersion".to_string(),
                    JsonValue::Number(JsonNumber::try_from(received_version).unwrap_or_else(
                        |_| JsonNumber::try_from(-1_i64).expect("constant is safe"),
                    )),
                ),
            ]));
        error
    }

    pub fn database_corrupt() -> Self {
        let mut error = Self::typed(
            ErrorCode::CorruptData,
            "farm3d could not validate its stored data. Preserve the data directory and contact support.",
            vec![],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "sourceName".to_string(),
            JsonValue::String("database".to_string()),
        )]));
        error
    }

    pub fn corrupt_import(source: &str) -> Self {
        let mut error = Self::typed(
            ErrorCode::CorruptData,
            "The selected document is not valid farm3d data.",
            vec![RecoveryCode::EditFields],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "sourceName".to_string(),
            JsonValue::String(source.to_string()),
        )]));
        error
    }

    pub fn legacy_corrupt(source: &str, source_sha256: Option<String>) -> Self {
        let mut error = Self::typed(
            ErrorCode::CorruptData,
            "farm3d could not import its legacy data. Preserve the data directory and contact support.",
            vec![],
            false,
        );
        let mut details = BTreeMap::from([(
            "sourceName".to_string(),
            JsonValue::String(source.to_string()),
        )]);
        if let Some(hash) = source_sha256 {
            details.insert("sourceSha256".to_string(), JsonValue::String(hash));
        }
        error.details = Some(details);
        error
    }

    pub fn from_safe_message(message: impl Into<String>) -> Self {
        let _ = message.into();
        Self::internal()
    }

    pub fn from_repository(error: crate::persistence::RepositoryError) -> Self {
        use crate::persistence::{RepositoryError, StorageError};
        match error {
            RepositoryError::Validation { field_path } => {
                Self::validation_at(field_path, "The submitted value is invalid.")
            }
            RepositoryError::NotFound { entity_id } => Self::not_found(entity_id),
            RepositoryError::Conflict {
                entity_id,
                expected_revision,
                current_revision,
            } => Self::revision_conflict(entity_id, expected_revision, current_revision),
            RepositoryError::SetConflict {
                expected_count,
                current_count,
            } => Self::set_conflict(expected_count, current_count),
            RepositoryError::DuplicateHost {
                conflicting_printer_id,
            } => Self::duplicate_host(&conflicting_printer_id),
            RepositoryError::Storage(StorageError::DuplicateHost(conflicting_printer_id)) => {
                Self::duplicate_host(&conflicting_printer_id)
            }
            RepositoryError::Storage(StorageError::CorruptData { .. }) => Self::database_corrupt(),
            RepositoryError::Storage(StorageError::PersistenceUnavailable)
            | RepositoryError::Storage(StorageError::Filesystem)
            | RepositoryError::Storage(StorageError::Database) => Self::persistence_unavailable(),
            RepositoryError::Storage(_) => Self::internal(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandError, ErrorCode, JsonValue, RecoveryCode};
    use crate::persistence::{RepositoryError, StorageError};

    #[test]
    fn repository_errors_keep_the_normative_code_recovery_and_details() {
        let validation = CommandError::from_repository(RepositoryError::Validation {
            field_path: "expectedRevision",
        });
        assert_eq!(validation.code, ErrorCode::Validation);
        assert_eq!(validation.recovery, vec![RecoveryCode::EditFields]);
        assert_eq!(
            validation.details.unwrap().get("fieldPath"),
            Some(&JsonValue::String("expectedRevision".to_string()))
        );

        let not_found = CommandError::from_repository(RepositoryError::NotFound {
            entity_id: "printer-a".to_string(),
        });
        assert_eq!(not_found.code, ErrorCode::NotFound);
        assert_eq!(not_found.recovery, vec![RecoveryCode::Reload]);
        assert_eq!(
            not_found.details.unwrap().get("entityId"),
            Some(&JsonValue::String("printer-a".to_string()))
        );

        let conflict = CommandError::from_repository(RepositoryError::Conflict {
            entity_id: "printer-a".to_string(),
            expected_revision: 2,
            current_revision: 3,
        });
        assert_eq!(conflict.code, ErrorCode::Conflict);
        let details = conflict.details.unwrap();
        assert_eq!(details.len(), 3);
        assert!(details.contains_key("entityId"));
        assert!(details.contains_key("expectedRevision"));
        assert!(details.contains_key("currentRevision"));

        let corrupt =
            CommandError::from_repository(RepositoryError::Storage(StorageError::CorruptData {
                source_name: "database",
                source_sha256: None,
            }));
        assert_eq!(corrupt.code, ErrorCode::CorruptData);
        assert!(!corrupt.retryable);
        assert!(corrupt.recovery.is_empty());

        let unavailable = CommandError::from_repository(RepositoryError::Storage(
            StorageError::PersistenceUnavailable,
        ));
        assert_eq!(unavailable.code, ErrorCode::PersistenceUnavailable);
        assert!(unavailable.retryable);
        assert_eq!(unavailable.recovery, vec![RecoveryCode::Retry]);

        let duplicate_host = CommandError::from_repository(RepositoryError::DuplicateHost {
            conflicting_printer_id: "printer-a".to_string(),
        });
        assert_eq!(duplicate_host.code, ErrorCode::DuplicateHost);
        assert!(!duplicate_host.retryable);
        assert_eq!(duplicate_host.recovery, vec![RecoveryCode::EditFields]);
        assert_eq!(
            duplicate_host.details.unwrap().get("conflictingPrinterId"),
            Some(&JsonValue::String("printer-a".to_string()))
        );

        // The partial unique index backstop (StorageError::DuplicateHost)
        // must map identically to the repository precheck's own variant.
        let backstop = CommandError::from_repository(RepositoryError::Storage(
            StorageError::DuplicateHost("printer-b".to_string()),
        ));
        assert_eq!(backstop.code, ErrorCode::DuplicateHost);
        assert_eq!(
            backstop.details.unwrap().get("conflictingPrinterId"),
            Some(&JsonValue::String("printer-b".to_string()))
        );
    }

    #[test]
    fn lifecycle_blocked_carries_its_blockers_and_is_never_retryable() {
        let error = CommandError::lifecycle_blocked(serde_json::json!(["setup-incomplete"]));
        assert_eq!(error.code, ErrorCode::LifecycleBlocked);
        assert!(!error.retryable);
        assert!(error.recovery.is_empty());
        assert_eq!(
            error.details.unwrap().get("blockers"),
            Some(&JsonValue::Array(vec![JsonValue::String(
                "setup-incomplete".to_string()
            )]))
        );
    }
}
