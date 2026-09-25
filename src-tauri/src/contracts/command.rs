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
    SlotOccupied,
    SelectionExpired,
    SourceContentDiffers,
    SourceUnavailable,
    UnsupportedFormat,
    /// P5 D2: no accepted OrcaSlicer engine is available.
    SlicerUnavailable,
    /// P5 D2: no readable preset source is available.
    PresetSourceUnavailable,
    /// P5 D3: the preset source has no preset by this name.
    PresetNotFound,
    /// P5 D3: a preset's `inherits` chain can't be resolved.
    PresetInvalid,
    /// P5 D3: the filament preset is not offered for the machine preset.
    FilamentIncompatible,
    /// P5 D4: a Printer Profile override has no row in the mapping table.
    UnmappedProfileOverride,
    /// P5 D4: a mapped key is unknown to the preset source.
    UnsupportedSettingForRuntime,
    /// P5 D7: a plate can't be sliced as it stands (for example, it is
    /// empty).
    PreparationInvalid,
    /// P5 D5: the Preparation is pinned to an older revision of its Model,
    /// and `start_slice` didn't say to continue with it.
    PreparationStale,
    /// P5 D10: the slice operation has already finished.
    OperationNotCancellable,
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
    /// P5: open the Slicer settings (engine and preset source).
    OpenSlicerSettings,
    /// P5 D5: reload the Preparation onto its Model's current revision.
    ReloadPreparation,
    /// P5: change the Preparation (presets, controls, or plates).
    EditPreparation,
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

    /// P3 D6 step 3: the destination slot's occupant changed since the
    /// client read it. `currentOccupantSpoolId` is `null` for an empty slot.
    pub fn occupancy_conflict(slot_id: &str, current_occupant_spool_id: Option<&str>) -> Self {
        let mut error = Self::conflict("The slot changed; reload and try again.");
        error.details = Some(BTreeMap::from([
            ("slotId".to_string(), JsonValue::String(slot_id.to_string())),
            (
                "currentOccupantSpoolId".to_string(),
                current_occupant_spool_id
                    .map_or(JsonValue::Null(()), |id| JsonValue::String(id.to_string())),
            ),
        ]));
        error
    }

    /// P3 Task 5: `set_material_slot_layout` tried to soft-remove a slot
    /// that still has a Spool loaded. The UI offers "Unload first".
    pub fn slot_occupied(slot_id: &str, spool_id: &str) -> Self {
        let mut error = Self::typed(
            ErrorCode::SlotOccupied,
            "This slot still has a Spool loaded. Unload it first.",
            vec![RecoveryCode::EditFields],
            false,
        );
        error.details = Some(BTreeMap::from([
            ("slotId".to_string(), JsonValue::String(slot_id.to_string())),
            (
                "spoolId".to_string(),
                JsonValue::String(spool_id.to_string()),
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

    /// P4 D7: the file selection is unknown, expired, cancelled, or already
    /// imported. The dialog offers **Choose files again**.
    pub fn selection_expired() -> Self {
        Self::typed(
            ErrorCode::SelectionExpired,
            "This file selection has expired. Choose the files again.",
            vec![RecoveryCode::Reload],
            false,
        )
    }

    /// P4 D16: the file chosen by Locate has different bytes from the
    /// Model's current revision. The UI offers **Relink and import as a new
    /// revision**.
    pub fn source_content_differs(
        current_sha256: &str,
        located_sha256: &str,
        located_file_name: &str,
    ) -> Self {
        let mut error = Self::typed(
            ErrorCode::SourceContentDiffers,
            "The located file is different from the stored copy.",
            vec![],
            false,
        );
        error.details = Some(BTreeMap::from([
            (
                "currentSha256".to_string(),
                JsonValue::String(current_sha256.to_string()),
            ),
            (
                "locatedSha256".to_string(),
                JsonValue::String(located_sha256.to_string()),
            ),
            (
                "locatedFileName".to_string(),
                JsonValue::String(located_file_name.to_string()),
            ),
        ]));
        error
    }

    /// P4 D16: the file chosen by Locate can't be read. `file_name` is a
    /// basename, never a path.
    pub fn source_unavailable(file_name: &str) -> Self {
        let mut error = Self::typed(
            ErrorCode::SourceUnavailable,
            "farm3d couldn't read the chosen file.",
            vec![RecoveryCode::Retry],
            true,
        );
        error.details = Some(BTreeMap::from([(
            "fileName".to_string(),
            JsonValue::String(file_name.to_string()),
        )]));
        error
    }

    /// P4 D8/D10: not a format farm3d reads. `extensions` names a 3MF's
    /// unimplemented required extensions, when that is the reason.
    pub fn unsupported_format(reason: &str, extensions: &[String]) -> Self {
        let mut error = Self::typed(ErrorCode::UnsupportedFormat, reason, vec![], false);
        let mut details =
            BTreeMap::from([("reason".to_string(), JsonValue::String(reason.to_string()))]);
        if !extensions.is_empty() {
            details.insert(
                "extensions".to_string(),
                JsonValue::Array(extensions.iter().cloned().map(JsonValue::String).collect()),
            );
        }
        error.details = Some(details);
        error
    }

    fn with_string_details(mut self, details: &[(&str, &str)]) -> Self {
        self.details = Some(
            details
                .iter()
                .map(|(key, value)| (key.to_string(), JsonValue::String(value.to_string())))
                .collect(),
        );
        self
    }

    /// P5 D2: no accepted engine. `reason` names no full path.
    pub fn slicer_unavailable(reason: &str) -> Self {
        Self::typed(
            ErrorCode::SlicerUnavailable,
            format!("OrcaSlicer is not available: {reason}"),
            vec![RecoveryCode::OpenSlicerSettings],
            false,
        )
        .with_string_details(&[("reason", reason)])
    }

    /// P5 D2: no readable preset source. `reason` names no full path.
    pub fn preset_source_unavailable(reason: &str) -> Self {
        Self::typed(
            ErrorCode::PresetSourceUnavailable,
            format!("No OrcaSlicer presets are available: {reason}"),
            vec![RecoveryCode::OpenSlicerSettings],
            false,
        )
        .with_string_details(&[("reason", reason)])
    }

    /// P5 D3: `kind` is `machine`, `process`, or `filament`.
    pub fn preset_not_found(kind: &str, preset: &str, preset_source_version: &str) -> Self {
        Self::typed(
            ErrorCode::PresetNotFound,
            format!("OrcaSlicer {preset_source_version} has no {kind} preset named \"{preset}\"."),
            vec![
                RecoveryCode::EditPreparation,
                RecoveryCode::OpenSlicerSettings,
            ],
            false,
        )
        .with_string_details(&[
            ("kind", kind),
            ("preset", preset),
            ("presetSourceVersion", preset_source_version),
        ])
    }

    /// P5 D3: `preset` can't be flattened (a cycle, a chain deeper than
    /// 20, a missing parent, or an unreadable file).
    pub fn preset_invalid(kind: &str, preset: &str, reason: &str) -> Self {
        Self::typed(
            ErrorCode::PresetInvalid,
            format!("The {kind} preset \"{preset}\" can't be used: {reason}"),
            vec![RecoveryCode::EditPreparation],
            false,
        )
        .with_string_details(&[("kind", kind), ("preset", preset), ("reason", reason)])
    }

    /// P5 D3: OrcaSlicer would silently slice with this filament (Gate F),
    /// so farm3d refuses it.
    pub fn filament_incompatible(filament_preset: &str, machine_preset: &str) -> Self {
        Self::typed(
            ErrorCode::FilamentIncompatible,
            format!(
                "The filament preset \"{filament_preset}\" is not made for \"{machine_preset}\"."
            ),
            vec![RecoveryCode::EditPreparation],
            false,
        )
        .with_string_details(&[
            ("filamentPreset", filament_preset),
            ("machinePreset", machine_preset),
        ])
    }

    /// P5 D4: `field` is the override's `PrinterProfile` field name.
    pub fn unmapped_profile_override(field: &str) -> Self {
        Self::typed(
            ErrorCode::UnmappedProfileOverride,
            format!("The Printer Profile override \"{field}\" can't be applied to a slice."),
            vec![RecoveryCode::EditFields],
            false,
        )
        .with_string_details(&[("field", field)])
    }

    /// P5 D4: the preset source doesn't know the OrcaSlicer key `key`.
    pub fn unsupported_setting_for_runtime(key: &str, preset_source_version: &str) -> Self {
        Self::typed(
            ErrorCode::UnsupportedSettingForRuntime,
            format!("OrcaSlicer {preset_source_version} does not support the setting \"{key}\"."),
            vec![
                RecoveryCode::EditPreparation,
                RecoveryCode::OpenSlicerSettings,
            ],
            false,
        )
        .with_string_details(&[("key", key), ("presetSourceVersion", preset_source_version)])
    }

    /// P5 D7: the plate `plate_key` can't be written for OrcaSlicer.
    /// `reason` is `empty`, `unknownObject`, `emptyObject`, or
    /// `invalidTransform`.
    pub fn preparation_invalid(plate_key: &str, reason: &str, message: &str) -> Self {
        Self::typed(
            ErrorCode::PreparationInvalid,
            message,
            vec![RecoveryCode::EditPreparation],
            false,
        )
        .with_string_details(&[("plateKey", plate_key), ("reason", reason)])
    }

    /// P5 D5: Preparation `preparation_id` is pinned to
    /// `source_revision_id`, which is no longer its Model's current
    /// revision. `start_slice` goes ahead only with
    /// `continueWithSourceRevision` equal to the pinned revision.
    pub fn preparation_stale(preparation_id: &str, source_revision_id: &str) -> Self {
        Self::typed(
            ErrorCode::PreparationStale,
            "The Model changed since this Preparation was made. Reload it onto the current revision, or continue with the revision it was made from.",
            vec![RecoveryCode::ReloadPreparation, RecoveryCode::EditPreparation],
            false,
        )
        .with_string_details(&[
            ("preparationId", preparation_id),
            ("sourceRevisionId", source_revision_id),
        ])
    }

    /// P5 D10: the operation is already `succeeded`, `failed`,
    /// `cancelled`, or `interrupted`.
    pub fn operation_not_cancellable(operation_id: &str, state: &str) -> Self {
        Self::typed(
            ErrorCode::OperationNotCancellable,
            "This slice has already finished.",
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[("sliceOperationId", operation_id), ("state", state)])
    }

    /// D7: an action (a Printer's archive/delete, or a Spool's archive/
    /// mark empty) is blocked. The message lists every blocker's own
    /// message, so it reads right whichever entity was blocked.
    pub fn lifecycle_blocked(blockers: &[crate::printers::lifecycle::LifecycleBlocker]) -> Self {
        let reasons = blockers
            .iter()
            .map(|blocker| blocker.message.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        let mut error = Self::typed(
            ErrorCode::LifecycleBlocked,
            format!("This action is blocked: {reasons}"),
            vec![],
            false,
        );
        let blockers = serde_json::to_value(blockers)
            .ok()
            .and_then(|value| JsonValue::from_serde_value(value).ok())
            .unwrap_or_else(|| JsonValue::Array(Vec::new()));
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

    /// P4 D2: a stored blob no longer matches its SHA-256 (or an existing
    /// blob's size disagrees with bytes of the same hash). The Library shows
    /// "Stored copy is damaged"; P4 offers no repair.
    pub fn content_corrupt() -> Self {
        let mut error = Self::typed(
            ErrorCode::CorruptData,
            "A stored copy is damaged.",
            vec![],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "sourceName".to_string(),
            JsonValue::String("content".to_string()),
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
            RepositoryError::SpoolsLoadedForImport => {
                Self::validation_at("printers", "Unload every Spool before importing Printers.")
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
            RepositoryError::OccupancyConflict {
                slot_id,
                current_occupant_spool_id,
            } => Self::occupancy_conflict(&slot_id, current_occupant_spool_id.as_deref()),
            RepositoryError::SlotOccupied { slot_id, spool_id } => {
                Self::slot_occupied(&slot_id, &spool_id)
            }
            RepositoryError::LifecycleBlocked(blockers) => Self::lifecycle_blocked(&blockers),
            RepositoryError::OperationIdReused => Self::validation_at(
                "operationId",
                "operationId was already used for a different request",
            ),
            // A move D10 doesn't allow is a caller bug until the slicing
            // commands give it a user-facing code (e.g. cancelling a
            // finished operation).
            RepositoryError::IllegalSliceTransition { .. } => Self::internal(),
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

        let occupancy = CommandError::from_repository(RepositoryError::OccupancyConflict {
            slot_id: "slt-a".to_string(),
            current_occupant_spool_id: Some("spl-a".to_string()),
        });
        assert_eq!(occupancy.code, ErrorCode::Conflict);
        let details = occupancy.details.unwrap();
        assert_eq!(details.len(), 2);
        assert_eq!(
            details.get("slotId"),
            Some(&JsonValue::String("slt-a".to_string()))
        );
        assert_eq!(
            details.get("currentOccupantSpoolId"),
            Some(&JsonValue::String("spl-a".to_string()))
        );
        let empty_slot = CommandError::from_repository(RepositoryError::OccupancyConflict {
            slot_id: "slt-a".to_string(),
            current_occupant_spool_id: None,
        });
        assert_eq!(
            empty_slot.details.unwrap().get("currentOccupantSpoolId"),
            Some(&JsonValue::Null(()))
        );

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

        // `SpoolsLoadedForImport` is a typed variant, not a magic-string
        // `Validation { field_path: "printers" }` match, but it still maps
        // to the same user-visible `CommandError`.
        let spools_loaded = CommandError::from_repository(RepositoryError::SpoolsLoadedForImport);
        assert_eq!(spools_loaded.code, ErrorCode::Validation);
        assert_eq!(
            spools_loaded.message,
            "Unload every Spool before importing Printers."
        );
        assert_eq!(
            spools_loaded.details.unwrap().get("fieldPath"),
            Some(&JsonValue::String("printers".to_string()))
        );

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
        use crate::printers::lifecycle::{LifecycleAction, LifecycleBlocker, LifecycleBlockerCode};
        let error = CommandError::lifecycle_blocked(&[LifecycleBlocker {
            action: LifecycleAction::MarkEmpty,
            code: LifecycleBlockerCode::SpoolReserved,
            message: "This Spool is reserved.".to_string(),
        }]);
        assert_eq!(error.code, ErrorCode::LifecycleBlocked);
        assert_eq!(
            error.message,
            "This action is blocked: This Spool is reserved."
        );
        assert!(!error.retryable);
        assert!(error.recovery.is_empty());
        let blockers = error.details.unwrap().remove("blockers").unwrap();
        let JsonValue::Array(blockers) = blockers else {
            panic!("blockers must be an array");
        };
        assert_eq!(blockers.len(), 1);
    }

    #[test]
    fn p4_library_errors_carry_their_code_recovery_and_basename_details() {
        let expired = CommandError::selection_expired();
        assert_eq!(expired.code, ErrorCode::SelectionExpired);
        assert_eq!(expired.recovery, vec![RecoveryCode::Reload]);
        assert!(!expired.retryable);

        let differs = CommandError::source_content_differs("aa", "bb", "cube.stl");
        assert_eq!(differs.code, ErrorCode::SourceContentDiffers);
        let details = differs.details.unwrap();
        assert_eq!(
            details.get("locatedFileName"),
            Some(&JsonValue::String("cube.stl".to_string()))
        );
        assert_eq!(
            details.get("currentSha256"),
            Some(&JsonValue::String("aa".to_string()))
        );
        assert_eq!(
            details.get("locatedSha256"),
            Some(&JsonValue::String("bb".to_string()))
        );

        let unavailable = CommandError::source_unavailable("cube.stl");
        assert_eq!(unavailable.code, ErrorCode::SourceUnavailable);
        assert_eq!(
            unavailable.details.unwrap().get("fileName"),
            Some(&JsonValue::String("cube.stl".to_string()))
        );

        let plain = CommandError::unsupported_format("Binary G-code isn't supported yet.", &[]);
        assert_eq!(plain.code, ErrorCode::UnsupportedFormat);
        assert!(!plain.details.as_ref().unwrap().contains_key("extensions"));
        let extension = CommandError::unsupported_format(
            "This 3MF needs an extension farm3d doesn't read.",
            &["b".to_string()],
        );
        assert_eq!(
            extension.details.unwrap().get("extensions"),
            Some(&JsonValue::Array(vec![JsonValue::String("b".to_string())]))
        );
    }
}
