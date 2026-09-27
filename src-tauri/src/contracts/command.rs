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
    /// P6 D7/D9: the Printer has an unresolved Host Operation (a write
    /// command, or `import_printers`).
    HostOperationPending,
    /// P6 D7: the Connection change would orphan an unresolved Host
    /// Operation.
    ConnectionInUse,
    /// P6 D6: the Printer's Connection can't do this (or can't reconcile a
    /// row of this kind).
    CapabilityUnsupported,
    /// P6 D8: the row is not `uncertain`, or has had no attempt and can
    /// still be reconciled.
    HostOperationNotAbandonable,
    /// P6 D9: the Printer's state does not allow a start.
    StartNotAllowed,
    /// P6 D9: Start is offered, but from another state than the operator
    /// confirmed.
    StartPreconditionChanged,
    /// P6 D9: the Printer's state does not allow this pause, resume, or
    /// cancel.
    ControlNotAllowed,
    /// P6 D9: the staged file is gone from the host or no longer matches.
    StagedArtifactInvalid,
    /// P7 D8: `ReservationError::InsufficientAvailable`, raised as a
    /// backstop when a Job's own gates already should have caught it.
    InsufficientMaterial,
    /// P7 D8: `ReservationError::SpoolNotReservable` -- the Spool isn't
    /// `active`.
    SpoolNotReservable,
    /// P7 D8: `ReservationError::InvalidTransition` -- the reservation's
    /// state changed since the caller last read it.
    ReservationState,
    /// P7 D4: the Printer already has an active Job (`assign_queue_entry`;
    /// the raw P6 write commands, decision 9).
    JobActive,
    /// P7 D3: the Job's state doesn't allow this action.
    JobActionNotAllowed,
    /// P7 D2: the Queue Entry's state doesn't allow this action.
    QueueEntryActionNotAllowed,
    /// P7 D5: the assign transaction's in-transaction re-check refused.
    AssignmentBlocked,
    /// P7 D7: `start_job` while the Job has start blockers.
    JobStartBlocked,
    /// P7 D7: the host reports a file other than the Job's.
    JobNotOnPrinter,
    /// P7 settlement: the Job's material is already settled or corrected.
    JobAlreadySettled,
    /// P7 D2: the Job was already retried.
    JobAlreadyRetried,
    /// P7 D8: a Printers import while Job history exists.
    JobsExist,
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
    /// P6: open the Printer's Job tab (its pending Host Operation).
    OpenPrinterJob,
    /// P7 D5: open the Job in Queue (`JOB_ACTIVE`, `CONNECTION_IN_USE` with
    /// a linked Job).
    OpenJob,
    /// P7 D5: open the Printer's setup (`SETUP_INCOMPLETE`).
    OpenPrinterSetup,
    /// P7 D5: unarchive the Printer (`PRINTER_ARCHIVED`).
    UnarchivePrinter,
    /// P7 D5: open Spools filtered to compatible Spools
    /// (`NO_COMPATIBLE_SPOOL`, `INSUFFICIENT_MATERIAL`, `SPOOL_NOT_LOADED`).
    LoadSpool,
    /// P7 D5: switch the entry to Manual and open Assign
    /// (`NEEDS_MANUAL_PRINTER`, `ADAPTER_NOT_PROVEN`).
    AssignManually,
    /// P7: open the settle dialog for a `reconciliation` Spool's Job.
    SettleMaterial,
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

    /// A succeeded Slice Operation's log went with its deleted Slice
    /// Revision (D13 stores it only there).
    pub fn slice_log_deleted(operation_id: &str) -> Self {
        let mut error = Self::typed(
            ErrorCode::NotFound,
            "This slice's log was deleted with its Slice Revision.",
            vec![RecoveryCode::Reload],
            false,
        );
        error.details = Some(BTreeMap::from([(
            "entityId".to_string(),
            JsonValue::String(operation_id.to_string()),
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

    /// P6 D7 `CONNECTION_IN_USE`: `set_printer_connection` or
    /// `clear_printer_connection` would change the endpoint, or clear the
    /// Connection or its credential, while `host_operation_id` is
    /// unresolved.
    pub fn connection_in_use(printer_id: &str, host_operation_id: &str) -> Self {
        Self::typed(
            ErrorCode::ConnectionInUse,
            "Finish or abandon the pending printer operation before changing this Connection.",
            vec![RecoveryCode::OpenPrinterJob],
            false,
        )
        .with_string_details(&[
            ("printerId", printer_id),
            ("hostOperationId", host_operation_id),
        ])
    }

    /// P6 D7/D9 `HOST_OPERATION_PENDING`: these Printers have these
    /// unresolved Host Operations.
    pub fn host_operation_pending(printer_ids: &[String], host_operation_ids: &[String]) -> Self {
        let strings = |values: &[String]| {
            JsonValue::Array(values.iter().cloned().map(JsonValue::String).collect())
        };
        let mut error = Self::typed(
            ErrorCode::HostOperationPending,
            "This printer has a pending operation. Finish or abandon it first.",
            vec![RecoveryCode::OpenPrinterJob],
            false,
        );
        error.details = Some(BTreeMap::from([
            ("printerIds".to_string(), strings(printer_ids)),
            ("hostOperationIds".to_string(), strings(host_operation_ids)),
        ]));
        error
    }

    /// P6 `CAPABILITY_UNSUPPORTED`. The message is the capability's own
    /// `detail`. `capability` and `reason` are wire spellings.
    pub fn capability_unsupported(
        printer_id: &str,
        capability: &str,
        reason: &str,
        detail: &str,
    ) -> Self {
        Self::typed(ErrorCode::CapabilityUnsupported, detail, vec![], false).with_string_details(&[
            ("printerId", printer_id),
            ("capability", capability),
            ("reason", reason),
            ("detail", detail),
        ])
    }

    /// P6 D8 `HOST_OPERATION_NOT_ABANDONABLE`. `state` is the wire spelling.
    pub fn host_operation_not_abandonable(
        host_operation_id: &str,
        state: &str,
        attempts: i64,
    ) -> Self {
        let mut error = Self::typed(
            ErrorCode::HostOperationNotAbandonable,
            "farm3d can only stop checking an uncertain operation after it has checked at least once.",
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[("hostOperationId", host_operation_id), ("state", state)]);
        error.details.get_or_insert_with(BTreeMap::new).insert(
            "attempts".to_string(),
            JsonValue::Number(
                JsonNumber::try_from(attempts)
                    .unwrap_or_else(|_| JsonNumber::try_from(0_i64).expect("constant is safe")),
            ),
        );
        error
    }

    /// P6 D9 `START_NOT_ALLOWED`. `observed_state`/`freshness` are wire
    /// spellings; `state_label` is the human one.
    pub fn start_not_allowed(
        printer_id: &str,
        observed_state: &str,
        freshness: &str,
        state_label: &str,
    ) -> Self {
        Self::typed(
            ErrorCode::StartNotAllowed,
            format!("The printer can't start a print now: {state_label}."),
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[
            ("printerId", printer_id),
            ("observedState", observed_state),
            ("freshness", freshness),
        ])
    }

    /// P6 D9 `START_PRECONDITION_CHANGED`.
    pub fn start_precondition_changed(
        printer_id: &str,
        observed_state: &str,
        freshness: &str,
        prior_state: &str,
    ) -> Self {
        Self::typed(
            ErrorCode::StartPreconditionChanged,
            "The printer's state changed. Confirm the bed again.",
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[
            ("printerId", printer_id),
            ("observedState", observed_state),
            ("freshness", freshness),
            ("priorState", prior_state),
        ])
    }

    /// P6 D9 `CONTROL_NOT_ALLOWED`. `verb` is `pause`, `resume`, or
    /// `cancel`.
    pub fn control_not_allowed(
        printer_id: &str,
        verb: &str,
        observed_state: &str,
        freshness: &str,
        state_label: &str,
    ) -> Self {
        Self::typed(
            ErrorCode::ControlNotAllowed,
            format!("The printer isn't in a state to {verb} now: {state_label}."),
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[
            ("printerId", printer_id),
            ("verb", verb),
            ("observedState", observed_state),
            ("freshness", freshness),
        ])
    }

    /// P6 D9 `STAGED_ARTIFACT_INVALID`. `reason` is `absent` or `differs`.
    /// No recovery code: **Stage again** is the Start dialog's own action.
    pub fn staged_artifact_invalid(host_operation_id: &str, reason: &str) -> Self {
        let message = if reason == "absent" {
            "The staged file is no longer on the printer. Stage it again."
        } else {
            "The file on the printer no longer matches this Slice Revision. Stage it again."
        };
        Self::typed(ErrorCode::StagedArtifactInvalid, message, vec![], false)
            .with_string_details(&[("hostOperationId", host_operation_id), ("reason", reason)])
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

    /// P7 D4 `JOB_ACTIVE`.
    pub fn job_active(printer_id: &str, job_id: &str) -> Self {
        Self::typed(
            ErrorCode::JobActive,
            "This Printer has an active Job. Use the Job's controls.",
            vec![RecoveryCode::OpenJob],
            false,
        )
        .with_string_details(&[("printerId", printer_id), ("jobId", job_id)])
    }

    /// P7 D3 `JOB_ACTION_NOT_ALLOWED`.
    pub fn job_action_not_allowed(
        job_id: &str,
        action: crate::jobs::JobAction,
        state: crate::jobs::JobState,
    ) -> Self {
        Self::typed(
            ErrorCode::JobActionNotAllowed,
            format!(
                "This Job can't {} while it is {}.",
                job_action_label(action),
                job_state_label(state)
            ),
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[
            ("jobId", job_id),
            ("action", &crate::spools::encode_enum(action)),
            ("state", &crate::spools::encode_enum(state)),
        ])
    }

    /// P7 D2 `QUEUE_ENTRY_ACTION_NOT_ALLOWED`. Removing an `assigned`
    /// entry also says what to do instead.
    pub fn queue_entry_action_not_allowed(
        entry_id: &str,
        action: crate::queue::QueueEntryAction,
        state: crate::queue::QueueEntryState,
    ) -> Self {
        use crate::queue::{QueueEntryAction, QueueEntryState};
        let label = match state {
            QueueEntryState::Queued => "queued",
            QueueEntryState::Assigned => "assigned",
            QueueEntryState::Closed => "closed",
        };
        let mut message = format!("This Queue Entry is {label}.");
        if action == QueueEntryAction::Remove && state == QueueEntryState::Assigned {
            message.push_str(" Release or cancel its Job instead.");
        }
        Self::typed(
            ErrorCode::QueueEntryActionNotAllowed,
            message,
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[
            ("entryId", entry_id),
            ("action", &crate::spools::encode_enum(action)),
            ("state", &crate::spools::encode_enum(state)),
        ])
    }

    /// P7 D5 `ASSIGNMENT_BLOCKED`. The message is the first blocker's.
    pub fn assignment_blocked(
        entry_id: &str,
        printer_id: &str,
        spool_id: &str,
        blockers: &[crate::queue::Blocker],
    ) -> Self {
        let message = blockers
            .first()
            .map(|blocker| blocker.message.clone())
            .unwrap_or_else(|| "This assignment is blocked.".to_string());
        let mut error = Self::typed(
            ErrorCode::AssignmentBlocked,
            message,
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[
            ("entryId", entry_id),
            ("printerId", printer_id),
            ("spoolId", spool_id),
        ]);
        let blockers = serde_json::to_value(blockers)
            .ok()
            .and_then(|value| JsonValue::from_serde_value(value).ok())
            .unwrap_or_else(|| JsonValue::Array(Vec::new()));
        error
            .details
            .get_or_insert_with(BTreeMap::new)
            .insert("blockers".to_string(), blockers);
        error
    }

    /// P7 D2 `JOB_ALREADY_RETRIED`.
    pub fn job_already_retried(job_id: &str, retry_entry_id: &str) -> Self {
        Self::typed(
            ErrorCode::JobAlreadyRetried,
            "This Job was already retried.",
            vec![RecoveryCode::Reload],
            false,
        )
        .with_string_details(&[("jobId", job_id), ("retryEntryId", retry_entry_id)])
    }

    /// P7 D8: a reservation primitive's refusal, named for the Spool it
    /// concerns (spec "Error codes": `INSUFFICIENT_MATERIAL`,
    /// `SPOOL_NOT_RESERVABLE`, `RESERVATION_STATE`). Starts from the
    /// primitive's baseline mapping and replaces the message and `details`
    /// with what the caller knows.
    pub fn reservation(
        spool_id: &str,
        spool_number: Option<i64>,
        reservation_id: Option<&str>,
        required_mg: Option<i64>,
        error: crate::spools::reservations::ReservationError,
    ) -> Self {
        use crate::spools::encode_enum;
        use crate::spools::reservations::ReservationError;

        let spool_label = spool_number
            .map(|number| format!("Spool #{number}"))
            .unwrap_or_else(|| "This Spool".to_string());
        let number = |value: i64| {
            JsonValue::Number(JsonNumber::try_from(value).expect("milligrams are JS-safe"))
        };
        match &error {
            ReservationError::InsufficientAvailable { available_mg } => {
                let mut mapped = Self::from(ReservationError::InsufficientAvailable {
                    available_mg: *available_mg,
                });
                mapped.message = format!("{spool_label} no longer has enough material.");
                let mut details = BTreeMap::from([
                    ("spoolId".to_string(), JsonValue::String(spool_id.to_string())),
                    ("availableMg".to_string(), number(*available_mg)),
                ]);
                if let Some(required_mg) = required_mg {
                    details.insert("requiredMg".to_string(), number(required_mg));
                }
                mapped.details = Some(details);
                mapped
            }
            ReservationError::SpoolNotReservable { lifecycle } => {
                let lifecycle_text = encode_enum(*lifecycle);
                let mut mapped = Self::from(ReservationError::SpoolNotReservable {
                    lifecycle: *lifecycle,
                });
                mapped.message =
                    format!("{spool_label} is {lifecycle_text} and can't be reserved.");
                mapped.details = Some(BTreeMap::from([
                    ("spoolId".to_string(), JsonValue::String(spool_id.to_string())),
                    ("lifecycle".to_string(), JsonValue::String(lifecycle_text)),
                ]));
                mapped
            }
            ReservationError::InvalidTransition { from } => {
                let mut mapped = Self::from(ReservationError::InvalidTransition { from: *from });
                let mut details = BTreeMap::from([(
                    "state".to_string(),
                    JsonValue::String(encode_enum(*from)),
                )]);
                if let Some(reservation_id) = reservation_id {
                    details.insert(
                        "reservationId".to_string(),
                        JsonValue::String(reservation_id.to_string()),
                    );
                }
                mapped.details = Some(details);
                mapped
            }
            _ => Self::from(error),
        }
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
            // Likewise a caller bug until a later task's guard rejects the
            // request itself before this ever runs.
            RepositoryError::IllegalHostOperationTransition { .. } => Self::internal(),
            // Likewise: `mark_sent` runs exactly once per row; a second
            // call is an executor bug, not something a user triggers.
            RepositoryError::HostOperationAlreadySent { .. } => Self::internal(),
            // P7 D2: `Assign` and `Remove` are the user rows of D2's table
            // (`QUEUE_ENTRY_ACTION_NOT_ALLOWED`); `JobTerminal` and
            // `Release` only ever come from farm3d's own code, so an
            // illegal one is a bug (`INTERNAL`).
            RepositoryError::IllegalQueueEntryTransition {
                entry_id,
                from,
                event,
            } => {
                use crate::queue::state::EntryEvent;
                use crate::queue::QueueEntryAction;
                match event {
                    EntryEvent::Assign => Self::queue_entry_action_not_allowed(
                        &entry_id,
                        QueueEntryAction::Assign,
                        from,
                    ),
                    EntryEvent::Remove => Self::queue_entry_action_not_allowed(
                        &entry_id,
                        QueueEntryAction::Remove,
                        from,
                    ),
                    EntryEvent::JobTerminal(_) | EntryEvent::Release => Self::internal(),
                }
            }
            RepositoryError::QueueEntryActionNotAllowed {
                entry_id,
                action,
                state,
            } => Self::queue_entry_action_not_allowed(&entry_id, action, state),
            // P7 D3: an illegal event a user command raised is
            // `JOB_ACTION_NOT_ALLOWED`; one only farm3d's own code raises
            // is a bug (`INTERNAL`) — its callers drop the already-applied
            // ones before they ever get here.
            RepositoryError::IllegalJobTransition {
                job_id,
                from,
                event,
            } => match crate::jobs::JobAction::for_user_event(event) {
                Some(action) => Self::job_action_not_allowed(&job_id, action, from),
                None => Self::internal(),
            },
            RepositoryError::JobActionNotAllowed {
                job_id,
                action,
                state,
            } => Self::job_action_not_allowed(&job_id, action, state),
            RepositoryError::JobActive { printer_id, job_id } => {
                Self::job_active(&printer_id, &job_id)
            }
            RepositoryError::AssignmentBlocked {
                entry_id,
                printer_id,
                spool_id,
                blockers,
            } => Self::assignment_blocked(&entry_id, &printer_id, &spool_id, &blockers),
            RepositoryError::JobAlreadyRetried {
                job_id,
                retry_entry_id,
            } => Self::job_already_retried(&job_id, &retry_entry_id),
            RepositoryError::Reservation {
                spool_id,
                spool_number,
                reservation_id,
                required_mg,
                error,
            } => Self::reservation(
                &spool_id,
                spool_number,
                reservation_id.as_deref(),
                required_mg,
                error,
            ),
            RepositoryError::ConnectionInUse {
                printer_id,
                host_operation_id,
            } => Self::connection_in_use(&printer_id, &host_operation_id),
            RepositoryError::HostOperationsPending {
                printer_ids,
                host_operation_ids,
            } => Self::host_operation_pending(&printer_ids, &host_operation_ids),
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

/// D3's action in `JOB_ACTION_NOT_ALLOWED`'s message.
fn job_action_label(action: crate::jobs::JobAction) -> &'static str {
    use crate::jobs::JobAction;
    match action {
        JobAction::Stage => "stage",
        JobAction::Start => "start",
        JobAction::Pause => "pause",
        JobAction::Resume => "resume",
        JobAction::Cancel => "cancel",
        JobAction::Release => "release",
        JobAction::Retry => "retry",
        JobAction::DeclareOutcome => "declare its outcome",
        JobAction::SettleMaterial => "settle its material",
        JobAction::CorrectMaterial => "correct its material",
    }
}

/// A Job state as `JOB_ACTION_NOT_ALLOWED`'s message names it.
fn job_state_label(state: crate::jobs::JobState) -> &'static str {
    use crate::jobs::JobState;
    match state {
        JobState::Assigned => "assigned",
        JobState::Staging => "staging",
        JobState::AwaitingStart => "awaiting start",
        JobState::Starting => "starting",
        JobState::Printing => "printing",
        JobState::Paused => "paused",
        JobState::Completed => "completed",
        JobState::Failed => "failed",
        JobState::Cancelled => "cancelled",
        JobState::OutcomeUnknown => "outcome unknown",
    }
}

/// P7's mapping for `spools::reservations::ReservationError` (D8), for a
/// command that calls a reservation primitive directly (Task 6's
/// `assign_queue_entry`, Task 9's `settle_job_material`/
/// `correct_job_material`). `ReservationError` itself carries only what
/// P3's primitives compute in-flight -- `availableMg`, the Spool's
/// `lifecycle`, the reservation's prior `state` -- never a Spool's number,
/// a Spool id, or a reservation id (P3 Task 4's tests match its variants
/// verbatim, so this task doesn't add fields to them). A caller that also
/// knows the Spool's number/id or the reservation's id enriches `details`
/// itself once it has this `CommandError` back.
impl From<crate::spools::reservations::ReservationError> for CommandError {
    fn from(error: crate::spools::reservations::ReservationError) -> Self {
        use crate::spools::encode_enum;
        use crate::spools::reservations::ReservationError;

        match error {
            ReservationError::InsufficientAvailable { available_mg } => {
                let mut error = Self::typed(
                    ErrorCode::InsufficientMaterial,
                    "This Spool no longer has enough material available.",
                    vec![RecoveryCode::Reload],
                    false,
                );
                error.details = Some(BTreeMap::from([(
                    "availableMg".to_string(),
                    JsonValue::Number(
                        JsonNumber::try_from(available_mg)
                            .expect("availableMg is bounded by weight::CURRENT_MG_RANGE"),
                    ),
                )]));
                error
            }
            ReservationError::SpoolNotReservable { lifecycle } => {
                let lifecycle_text = encode_enum(lifecycle);
                let mut error = Self::typed(
                    ErrorCode::SpoolNotReservable,
                    format!("This Spool is {lifecycle_text} and can't be reserved."),
                    vec![RecoveryCode::Reload],
                    false,
                );
                error.details = Some(BTreeMap::from([(
                    "lifecycle".to_string(),
                    JsonValue::String(lifecycle_text),
                )]));
                error
            }
            ReservationError::InvalidTransition { from } => {
                let mut error = Self::typed(
                    ErrorCode::ReservationState,
                    "The reservation changed. Reload.",
                    vec![RecoveryCode::Reload],
                    false,
                );
                error.details = Some(BTreeMap::from([(
                    "state".to_string(),
                    JsonValue::String(encode_enum(from)),
                )]));
                error
            }
            ReservationError::InvalidAmount => {
                Self::validation_at("amountMg", "The submitted value is invalid.")
            }
            // R8: `consume_measured`'s caller-supplied `entry` failed
            // validation (an out-of-range amount, or a `Scale` entry
            // naming an unknown tare) -- the same `VALIDATION` shape
            // `RepositoryError::Validation` gets from `from_repository`,
            // so an operator-entered typo is recoverable, not `INTERNAL`.
            ReservationError::Validation { field_path } => {
                Self::validation_at(field_path, "The submitted value is invalid.")
            }
            ReservationError::NotFound => Self::typed(
                ErrorCode::NotFound,
                "This reservation no longer exists.",
                vec![RecoveryCode::Reload],
                false,
            ),
            ReservationError::Storage(storage_error) => {
                Self::from_repository(crate::persistence::RepositoryError::Storage(storage_error))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CommandError, ErrorCode, JsonNumber, JsonValue, RecoveryCode};
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
    fn p6_guard_errors_carry_their_code_message_recovery_and_details() {
        let in_use = CommandError::from_repository(RepositoryError::ConnectionInUse {
            printer_id: "prn-a".to_string(),
            host_operation_id: "hop-a".to_string(),
        });
        assert_eq!(in_use.code, ErrorCode::ConnectionInUse);
        assert_eq!(
            in_use.message,
            "Finish or abandon the pending printer operation before changing this Connection."
        );
        assert_eq!(in_use.recovery, vec![RecoveryCode::OpenPrinterJob]);
        assert!(!in_use.retryable);
        assert_eq!(
            serde_json::to_value(&in_use.details).unwrap(),
            serde_json::json!({"printerId": "prn-a", "hostOperationId": "hop-a"})
        );

        let pending = CommandError::from_repository(RepositoryError::HostOperationsPending {
            printer_ids: vec!["prn-a".to_string(), "prn-b".to_string()],
            host_operation_ids: vec!["hop-a".to_string(), "hop-b".to_string()],
        });
        assert_eq!(pending.code, ErrorCode::HostOperationPending);
        assert_eq!(
            pending.message,
            "This printer has a pending operation. Finish or abandon it first."
        );
        assert_eq!(pending.recovery, vec![RecoveryCode::OpenPrinterJob]);
        assert!(!pending.retryable);
        assert_eq!(
            serde_json::to_value(&pending.details).unwrap(),
            serde_json::json!({
                "printerIds": ["prn-a", "prn-b"],
                "hostOperationIds": ["hop-a", "hop-b"],
            })
        );
    }

    #[test]
    fn reservation_errors_map_to_the_new_p7_codes() {
        use crate::spools::reservations::{ReservationError, ReservationState};
        use crate::spools::SpoolLifecycle;

        let insufficient: CommandError =
            ReservationError::InsufficientAvailable { available_mg: -1_000 }.into();
        assert_eq!(insufficient.code, ErrorCode::InsufficientMaterial);
        assert_eq!(insufficient.recovery, vec![RecoveryCode::Reload]);
        assert!(!insufficient.retryable);
        assert_eq!(
            insufficient.details.unwrap().get("availableMg"),
            Some(&JsonValue::Number(JsonNumber::try_from(-1_000_i64).unwrap()))
        );

        let not_reservable: CommandError = ReservationError::SpoolNotReservable {
            lifecycle: SpoolLifecycle::Empty,
        }
        .into();
        assert_eq!(not_reservable.code, ErrorCode::SpoolNotReservable);
        assert_eq!(
            not_reservable.message,
            "This Spool is empty and can't be reserved."
        );
        assert_eq!(
            not_reservable.details.unwrap().get("lifecycle"),
            Some(&JsonValue::String("empty".to_string()))
        );

        let state: CommandError = ReservationError::InvalidTransition {
            from: ReservationState::Consumed,
        }
        .into();
        assert_eq!(state.code, ErrorCode::ReservationState);
        assert_eq!(state.message, "The reservation changed. Reload.");
        assert_eq!(
            state.details.unwrap().get("state"),
            Some(&JsonValue::String("consumed".to_string()))
        );

        let not_found: CommandError = ReservationError::NotFound.into();
        assert_eq!(not_found.code, ErrorCode::NotFound);
        assert_eq!(not_found.recovery, vec![RecoveryCode::Reload]);

        // R8: `consume_measured`'s caller-supplied `entry` failing
        // validation must map to `VALIDATION` with its `field_path`, not
        // collapse into `INTERNAL`.
        let validation: CommandError = ReservationError::Validation {
            field_path: "entry.netMg",
        }
        .into();
        assert_eq!(validation.code, ErrorCode::Validation);
        assert_eq!(validation.recovery, vec![RecoveryCode::EditFields]);
        assert_eq!(
            validation.details.unwrap().get("fieldPath"),
            Some(&JsonValue::String("entry.netMg".to_string()))
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

    fn mg(value: i64) -> JsonValue {
        JsonValue::Number(JsonNumber::try_from(value).unwrap())
    }

    /// C5: a reservation refusal inside a Job transaction names the Spool
    /// (`#<n>`) and carries the spec's details.
    #[test]
    fn reservation_refusals_name_the_spool_and_the_amounts() {
        use crate::spools::reservations::{ReservationError, ReservationState};
        use crate::spools::SpoolLifecycle;

        let insufficient = CommandError::from_repository(RepositoryError::Reservation {
            spool_id: "spl-a".to_string(),
            spool_number: Some(7),
            reservation_id: None,
            required_mg: Some(12_500),
            error: ReservationError::InsufficientAvailable { available_mg: 4_000 },
        });
        assert_eq!(insufficient.code, ErrorCode::InsufficientMaterial);
        assert_eq!(insufficient.message, "Spool #7 no longer has enough material.");
        assert_eq!(insufficient.recovery, vec![RecoveryCode::Reload]);
        let details = insufficient.details.unwrap();
        assert_eq!(details.get("spoolId"), Some(&JsonValue::String("spl-a".to_string())));
        assert_eq!(details.get("availableMg"), Some(&mg(4_000)));
        assert_eq!(details.get("requiredMg"), Some(&mg(12_500)));

        let archived = CommandError::from_repository(RepositoryError::Reservation {
            spool_id: "spl-a".to_string(),
            spool_number: Some(7),
            reservation_id: None,
            required_mg: Some(12_500),
            error: ReservationError::SpoolNotReservable {
                lifecycle: SpoolLifecycle::Archived,
            },
        });
        assert_eq!(archived.code, ErrorCode::SpoolNotReservable);
        assert_eq!(archived.message, "Spool #7 is archived and can't be reserved.");
        let details = archived.details.unwrap();
        assert_eq!(details.get("spoolId"), Some(&JsonValue::String("spl-a".to_string())));
        assert_eq!(
            details.get("lifecycle"),
            Some(&JsonValue::String("archived".to_string()))
        );

        let changed = CommandError::from_repository(RepositoryError::Reservation {
            spool_id: "spl-a".to_string(),
            spool_number: Some(7),
            reservation_id: Some("rsv-1".to_string()),
            required_mg: None,
            error: ReservationError::InvalidTransition {
                from: ReservationState::Consumed,
            },
        });
        assert_eq!(changed.code, ErrorCode::ReservationState);
        let details = changed.details.unwrap();
        assert_eq!(
            details.get("reservationId"),
            Some(&JsonValue::String("rsv-1".to_string()))
        );
        assert_eq!(
            details.get("state"),
            Some(&JsonValue::String("consumed".to_string()))
        );
    }

    /// C2: D2/D3's illegal user events are the spec's action codes; the
    /// ones only farm3d's own code raises stay `INTERNAL`.
    #[test]
    fn illegal_queue_and_job_transitions_map_by_who_raised_them() {
        use crate::jobs::{JobEventKind, JobState};
        use crate::queue::state::EntryEvent;
        use crate::queue::{CloseReason, QueueEntryState};

        let remove = CommandError::from_repository(RepositoryError::IllegalQueueEntryTransition {
            entry_id: "qen-1".to_string(),
            from: QueueEntryState::Assigned,
            event: EntryEvent::Remove,
        });
        assert_eq!(remove.code, ErrorCode::QueueEntryActionNotAllowed);
        assert_eq!(
            remove.message,
            "This Queue Entry is assigned. Release or cancel its Job instead."
        );
        let details = remove.details.unwrap();
        assert_eq!(details.get("action"), Some(&JsonValue::String("remove".to_string())));
        assert_eq!(details.get("state"), Some(&JsonValue::String("assigned".to_string())));

        let internal = CommandError::from_repository(RepositoryError::IllegalQueueEntryTransition {
            entry_id: "qen-1".to_string(),
            from: QueueEntryState::Queued,
            event: EntryEvent::JobTerminal(CloseReason::Completed),
        });
        assert_eq!(internal.code, ErrorCode::Internal);

        let release = CommandError::from_repository(RepositoryError::IllegalJobTransition {
            job_id: "job-1".to_string(),
            from: JobState::Printing,
            event: JobEventKind::Released,
        });
        assert_eq!(release.code, ErrorCode::JobActionNotAllowed);
        assert_eq!(release.message, "This Job can't release while it is printing.");
        let details = release.details.unwrap();
        assert_eq!(details.get("jobId"), Some(&JsonValue::String("job-1".to_string())));
        assert_eq!(details.get("action"), Some(&JsonValue::String("release".to_string())));
        assert_eq!(details.get("state"), Some(&JsonValue::String("printing".to_string())));

        let tracker = CommandError::from_repository(RepositoryError::IllegalJobTransition {
            job_id: "job-1".to_string(),
            from: JobState::Assigned,
            event: JobEventKind::Completed,
        });
        assert_eq!(tracker.code, ErrorCode::Internal);
    }
}
