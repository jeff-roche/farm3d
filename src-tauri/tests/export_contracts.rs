use std::collections::{BTreeMap, BTreeSet};
use std::fs;
use std::path::{Path, PathBuf};

use farm3d_lib::catalog::commands::{CatalogInfo, CatalogModelSummary, CatalogVariantSummary};
use farm3d_lib::catalog::resolve::{
    CatalogStatus, ProfileDrift, ProfileResolution, ResolvedPrinter,
};
use farm3d_lib::catalog::{BedShape, PointMm, PrinterProfile};
use farm3d_lib::connections::commands::{ConnectionSubmission, CredentialStoreInfo};
use farm3d_lib::connections::credentials::CredentialStoreKind;
use farm3d_lib::connections::discovery::DiscoveredPrinter;
use farm3d_lib::connections::supervisor::{
    PrinterSetupFacts, PrinterStatusBackfill, PrinterStatusEvent, PrinterStatusEventPayload,
    PrinterStatusEventType, PrinterStatusRow,
};
use farm3d_lib::connections::{
    status_repository::PrinterTelemetry, ConnectionConfig, ConnectionState, PrinterStatus,
    ProbeResult, ReportedCapabilities, StatusCacheWarning, StatusCacheWarningOperation,
};
use farm3d_lib::contracts::command::{
    CommandError, CommandSuccess, CorrelationId, ErrorCode, JsonNumber, JsonValue, RecoveryCode,
};
use farm3d_lib::contracts::domain::NoArgsRequest;
use farm3d_lib::contracts::event::{EventEnvelope, EventSubject, JsSafeInteger};
use farm3d_lib::contracts::inventory::CommandContracts;
use farm3d_lib::contracts::navigation::{
    NavigationDestination, NavigationSelection, NavigationSelectionKind, NavigationTarget,
};
use farm3d_lib::contracts::ContractVersion;
use farm3d_lib::printers::commands::{
    DeletePrinterResult, ExportResult as PrintersExportResult, OperationWarning,
    OperationWarningCode, PrinterMutationResult, PrinterRevisionPrecondition, PrintersImportResult,
};
use farm3d_lib::printers::operational::{
    HostActivity, OperationalInput, OperationalResult, OperationalState, PrinterReadiness,
    ReadinessReason, ReadinessState, TelemetryFreshness,
};
use farm3d_lib::printers::lifecycle::{
    LifecycleAction, LifecycleBlocker, LifecycleBlockerCode, LifecycleEligibility,
};
use farm3d_lib::printers::setup::SetupGap;
use farm3d_lib::printers::LastKnownGood;
use farm3d_lib::printers::{CatalogRef, PrinterPatch, StartSafety};
use farm3d_lib::settings::commands::{
    ExportResult as SettingsExportResult, MonitorDensity, MonitorSection, SettingsImportResult,
    SettingsRecord,
};
use serde::{Deserialize, Serialize};
use tempfile::TempDir;
use ts_rs::{Config, ExportError, TypeVisitor, TS};

#[derive(Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "fixtures/RenamedFixture.ts")]
struct RenamedFixture {
    #[serde(rename = "displayLabel")]
    #[ts(rename = "displayLabel")]
    rust_field_name: String,
}

#[derive(Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "fixtures/OptionalFixture.ts")]
struct OptionalFixture {
    required: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    #[ts(optional)]
    optional_note: Option<String>,
}

#[derive(Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "fixtures/FlattenedFixture.ts")]
struct FlattenedFixture {
    known: String,
    #[serde(flatten)]
    #[ts(flatten)]
    extensions: BTreeMap<String, JsonValue>,
}

#[derive(Deserialize, Serialize, TS)]
#[serde(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
#[ts(
    tag = "status",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    export_to = "fixtures/TaggedFixture.ts"
)]
enum TaggedFixture {
    Ready { item_count: u32 },
    Waiting,
}

#[derive(Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(rename_all = "camelCase", export_to = "fixtures/FixturePayload.ts")]
struct FixturePayload {
    value: String,
}

#[derive(Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "fixtures/ConcreteCommandEnvelope.ts"
)]
struct ConcreteCommandEnvelope {
    result: CommandSuccess<FixturePayload>,
}

#[derive(Deserialize, Serialize, TS)]
#[ts(export_to = "fixtures/FixtureEventType.ts")]
enum FixtureEventType {
    #[serde(rename = "fixture.changed")]
    #[ts(rename = "fixture.changed")]
    FixtureChanged,
}

#[derive(Deserialize, Serialize, TS)]
#[serde(rename_all = "camelCase")]
#[ts(
    rename_all = "camelCase",
    export_to = "fixtures/ConcreteEventEnvelope.ts"
)]
struct ConcreteEventEnvelope {
    event: EventEnvelope<FixtureEventType, FixturePayload>,
}

struct EventEnvelopeTypeAssertions;

impl TS for EventEnvelopeTypeAssertions {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;

    fn name(_: &Config) -> String {
        "EventEnvelopeTypeAssertions".to_string()
    }

    fn inline(_: &Config) -> String {
        "EventEnvelopeTypeAssertions".to_string()
    }

    fn decl(_: &Config) -> String {
        concat!(
            "type EventEnvelopeTypeAssertions = EventEnvelope<\"fixture.changed\", FixturePayload>;\n",
            "// @ts-expect-error event envelope discriminators must be strings\n",
            "export type InvalidEventEnvelopeDiscriminator = EventEnvelope<42, FixturePayload>;"
        )
        .to_string()
    }

    fn visit_dependencies(visitor: &mut impl TypeVisitor)
    where
        Self: 'static,
    {
        visitor.visit::<EventEnvelope<FixtureEventType, FixturePayload>>();
        visitor.visit::<FixturePayload>();
    }

    fn output_path() -> Option<PathBuf> {
        Some("fixtures/EventEnvelopeTypeAssertions.ts".into())
    }
}

struct Export {
    relative_path: PathBuf,
    render: fn(&Config) -> Result<String, ExportError>,
}

fn render<T: TS + 'static>(config: &Config) -> Result<String, ExportError> {
    T::export_to_string(config)
}

fn export<T: TS + 'static>() -> Export {
    Export {
        relative_path: T::output_path().expect("registered contract types must declare export_to"),
        render: render::<T>,
    }
}

fn export_registry() -> Vec<Export> {
    vec![
        export::<JsonValue>(),
        export::<JsonNumber>(),
        export::<CorrelationId>(),
        export::<ErrorCode>(),
        export::<RecoveryCode>(),
        export::<CommandSuccess<FixturePayload>>(),
        export::<CommandError>(),
        export::<CommandContracts>(),
        export::<NoArgsRequest>(),
        export::<SettingsRecord>(),
        export::<MonitorSection>(),
        export::<MonitorDensity>(),
        export::<PrinterRevisionPrecondition>(),
        export::<ConnectionConfig>(),
        export::<ConnectionSubmission>(),
        export::<ConnectionState>(),
        export::<HostActivity>(),
        export::<OperationalState>(),
        export::<ReadinessState>(),
        export::<ReadinessReason>(),
        export::<PrinterReadiness>(),
        export::<TelemetryFreshness>(),
        export::<OperationalInput>(),
        export::<OperationalResult>(),
        export::<PrinterStatus>(),
        export::<PrinterTelemetry>(),
        export::<PrinterSetupFacts>(),
        export::<PrinterStatusEventType>(),
        export::<PrinterStatusEventPayload>(),
        export::<PrinterStatusEvent>(),
        export::<StatusCacheWarningOperation>(),
        export::<StatusCacheWarning>(),
        export::<PrinterStatusRow>(),
        export::<PrinterStatusBackfill>(),
        export::<BedShape>(),
        export::<PointMm>(),
        export::<PrinterProfile>(),
        export::<CatalogRef>(),
        export::<CatalogStatus>(),
        export::<ProfileDrift>(),
        export::<ProfileResolution>(),
        export::<LastKnownGood>(),
        export::<ResolvedPrinter>(),
        export::<PrinterPatch>(),
        export::<StartSafety>(),
        export::<SetupGap>(),
        export::<LifecycleAction>(),
        export::<LifecycleBlockerCode>(),
        export::<LifecycleBlocker>(),
        export::<LifecycleEligibility>(),
        export::<CatalogModelSummary>(),
        export::<CatalogVariantSummary>(),
        export::<CatalogInfo>(),
        export::<ProbeResult>(),
        export::<ReportedCapabilities>(),
        export::<CredentialStoreKind>(),
        export::<CredentialStoreInfo>(),
        export::<DiscoveredPrinter>(),
        export::<OperationWarningCode>(),
        export::<OperationWarning>(),
        export::<PrinterMutationResult>(),
        export::<DeletePrinterResult>(),
        export::<PrintersExportResult>(),
        export::<PrintersImportResult>(),
        export::<SettingsExportResult>(),
        export::<SettingsImportResult>(),
        export::<EventSubject>(),
        export::<JsSafeInteger>(),
        export::<EventEnvelope<FixtureEventType, FixturePayload>>(),
        export::<NavigationDestination>(),
        export::<NavigationSelectionKind>(),
        export::<NavigationSelection>(),
        export::<NavigationTarget>(),
        export::<RenamedFixture>(),
        export::<OptionalFixture>(),
        export::<FlattenedFixture>(),
        export::<TaggedFixture>(),
        export::<FixturePayload>(),
        export::<FixtureEventType>(),
        export::<ConcreteCommandEnvelope>(),
        export::<ConcreteEventEnvelope>(),
        export::<EventEnvelopeTypeAssertions>(),
    ]
}

fn write_exports(destination: &Path) {
    let config = Config::new().with_out_dir(destination);
    let mut seen = BTreeSet::new();

    for export in export_registry() {
        assert!(
            seen.insert(export.relative_path.clone()),
            "duplicate export path: {}",
            export.relative_path.display()
        );
        let path = destination.join(&export.relative_path);
        fs::create_dir_all(path.parent().expect("export path must have a parent"))
            .expect("export directory should be created");
        let contents = (export.render)(&config).expect("contract should export");
        let contents = contents
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n")
            + "\n";
        fs::write(path, contents).expect("contract should be written");
    }
}

fn files_under(root: &Path) -> BTreeMap<PathBuf, Vec<u8>> {
    fn visit(root: &Path, current: &Path, files: &mut BTreeMap<PathBuf, Vec<u8>>) {
        let mut entries = fs::read_dir(current)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", current.display()))
            .collect::<Result<Vec<_>, _>>()
            .expect("directory entries should be readable");
        entries.sort_by_key(|entry| entry.file_name());

        for entry in entries {
            let path = entry.path();
            if path.is_dir() {
                visit(root, &path, files);
            } else {
                let relative = path
                    .strip_prefix(root)
                    .expect("visited file should be below root")
                    .to_path_buf();
                files.insert(
                    relative,
                    fs::read(path).expect("contract should be readable"),
                );
            }
        }
    }

    let mut files = BTreeMap::new();
    visit(root, root, &mut files);
    files
}

#[derive(Debug, PartialEq, Eq)]
struct ContractDrift {
    missing: Vec<PathBuf>,
    stale: Vec<PathBuf>,
    changed: Vec<PathBuf>,
}

impl ContractDrift {
    fn is_empty(&self) -> bool {
        self.missing.is_empty() && self.stale.is_empty() && self.changed.is_empty()
    }
}

impl std::fmt::Display for ContractDrift {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("generated contract drift:")?;
        for (heading, paths) in [
            ("missing from committed output", &self.missing),
            ("stale/extra in committed output", &self.stale),
            ("changed files", &self.changed),
        ] {
            if paths.is_empty() {
                continue;
            }
            write!(formatter, "\n{heading}:")?;
            for path in paths {
                write!(formatter, "\n  - {}", path.display())?;
            }
        }
        Ok(())
    }
}

fn compare_contract_trees(
    generated: &BTreeMap<PathBuf, Vec<u8>>,
    committed: &BTreeMap<PathBuf, Vec<u8>>,
) -> ContractDrift {
    let missing = generated
        .keys()
        .filter(|path| !committed.contains_key(*path))
        .cloned()
        .collect();
    let stale = committed
        .keys()
        .filter(|path| !generated.contains_key(*path))
        .cloned()
        .collect();
    let changed = generated
        .iter()
        .filter_map(|(path, contents)| {
            committed
                .get(path)
                .filter(|committed_contents| *committed_contents != contents)
                .map(|_| path.clone())
        })
        .collect();

    ContractDrift {
        missing,
        stale,
        changed,
    }
}

fn committed_contracts_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("..")
        .join("src/generated/contracts")
}

#[test]
fn generated_contracts_match_committed_relative_paths_and_bytes() {
    let temporary = TempDir::new().expect("temporary export directory should be created");
    write_exports(temporary.path());
    let generated = files_under(temporary.path());
    let committed = files_under(&committed_contracts_dir());
    let drift = compare_contract_trees(&generated, &committed);

    assert!(drift.is_empty(), "{drift}");
}

#[test]
#[ignore = "run through `just gen-contracts` to replace committed bindings"]
fn regenerate_contracts() {
    let destination = committed_contracts_dir();
    if destination.exists() {
        fs::remove_dir_all(&destination).expect("generated contracts should be removable");
    }
    fs::create_dir_all(&destination).expect("generated contracts directory should be created");
    write_exports(&destination);
}

#[test]
fn optional_fields_are_omitted_from_serialized_contracts() {
    let fixture = OptionalFixture {
        required: "present".to_string(),
        optional_note: None,
    };

    assert_eq!(
        serde_json::to_value(fixture).expect("fixture should serialize"),
        serde_json::json!({ "required": "present" })
    );
}

#[test]
fn command_envelopes_serialize_literal_version_and_structured_error_fields() {
    let success = CommandSuccess {
        contract_version: ContractVersion::V1,
        data: FixturePayload {
            value: "ready".to_string(),
        },
    };
    let error = CommandError {
        contract_version: ContractVersion::V1,
        code: ErrorCode::Internal,
        message: "The operation could not be completed".to_string(),
        recovery: vec![RecoveryCode::RestartApplication],
        retryable: false,
        correlation_id: Some(CorrelationId(
            "err-00000000-0000-4000-8000-000000000000".to_string(),
        )),
        field_errors: None,
        details: None,
    };

    assert_eq!(
        serde_json::json!({
            "success": success,
            "error": error,
        }),
        serde_json::json!({
            "success": { "contractVersion": 1, "data": { "value": "ready" } },
            "error": {
                "contractVersion": 1,
                "code": "INTERNAL",
                "message": "The operation could not be completed",
                "recovery": ["RESTART_APPLICATION"],
                "retryable": false,
                "correlationId": "err-00000000-0000-4000-8000-000000000000"
            }
        })
    );
}

#[test]
fn error_and_recovery_codes_serialize_with_exact_spellings() {
    let errors = [
        ErrorCode::Validation,
        ErrorCode::NotFound,
        ErrorCode::Conflict,
        ErrorCode::PersistenceUnavailable,
        ErrorCode::CorruptData,
        ErrorCode::MigrationFailed,
        ErrorCode::UnsupportedSchemaVersion,
        ErrorCode::CredentialUnavailable,
        ErrorCode::CredentialRequired,
        ErrorCode::UnsupportedAdapter,
        ErrorCode::PrinterUnreachable,
        ErrorCode::AuthenticationFailed,
        ErrorCode::ProtocolError,
        ErrorCode::Timeout,
        ErrorCode::IncompatibleContractVersion,
        ErrorCode::Internal,
    ];
    let recoveries = [
        RecoveryCode::Retry,
        RecoveryCode::EditFields,
        RecoveryCode::Reload,
        RecoveryCode::ReenterCredential,
        RecoveryCode::ChooseSupportedAdapter,
        RecoveryCode::CheckConnection,
        RecoveryCode::CheckCredentials,
        RecoveryCode::RestartApplication,
        RecoveryCode::UpgradeFarm3d,
    ];

    assert_eq!(
        serde_json::json!({ "errors": errors, "recoveries": recoveries }),
        serde_json::json!({
            "errors": [
                "VALIDATION", "NOT_FOUND", "CONFLICT", "PERSISTENCE_UNAVAILABLE",
                "CORRUPT_DATA", "MIGRATION_FAILED", "UNSUPPORTED_SCHEMA_VERSION",
                "CREDENTIAL_UNAVAILABLE", "CREDENTIAL_REQUIRED", "UNSUPPORTED_ADAPTER",
                "PRINTER_UNREACHABLE", "AUTHENTICATION_FAILED", "PROTOCOL_ERROR",
                "TIMEOUT", "INCOMPATIBLE_CONTRACT_VERSION", "INTERNAL"
            ],
            "recoveries": [
                "RETRY", "EDIT_FIELDS", "RELOAD", "REENTER_CREDENTIAL",
                "CHOOSE_SUPPORTED_ADAPTER", "CHECK_CONNECTION", "CHECK_CREDENTIALS",
                "RESTART_APPLICATION", "UPGRADE_FARM3D"
            ]
        })
    );
}

#[test]
fn event_envelope_serializes_generic_identity_and_payload() {
    let event = EventEnvelope {
        contract_version: ContractVersion::V1,
        stream_id: "stream-1".to_string(),
        sequence: JsSafeInteger::try_from(7_u64).expect("sequence should be JS-safe"),
        event_id: "event-7".to_string(),
        occurred_at: "2026-09-17T12:00:00Z".to_string(),
        event_type: FixtureEventType::FixtureChanged,
        subject: EventSubject {
            kind: "fixture".to_string(),
            id: "fixture-1".to_string(),
        },
        payload: FixturePayload {
            value: "changed".to_string(),
        },
    };

    assert_eq!(
        serde_json::to_value(event).expect("event should serialize"),
        serde_json::json!({
            "contractVersion": 1,
            "streamId": "stream-1",
            "sequence": 7,
            "eventId": "event-7",
            "occurredAt": "2026-09-17T12:00:00Z",
            "type": "fixture.changed",
            "subject": { "kind": "fixture", "id": "fixture-1" },
            "payload": { "value": "changed" }
        })
    );
}

#[test]
fn event_envelope_constrains_discriminators_to_strings_in_typescript() {
    let generated =
        EventEnvelope::<FixtureEventType, FixturePayload>::export_to_string(&Config::new())
            .expect("event envelope should export");

    assert!(
        generated.contains("EventEnvelope<T extends string, P>"),
        "generated event envelope did not constrain T: {generated}"
    );
}

#[test]
fn event_envelope_concrete_declaration_uses_base_identifier_and_concrete_fields() {
    assert_eq!(
        EventEnvelope::<FixtureEventType, FixturePayload>::decl_concrete(&Config::new()),
        "type EventEnvelope = { contractVersion: 1, streamId: string, sequence: number, eventId: string, occurredAt: string, type: FixtureEventType, subject: EventSubject, payload: FixturePayload, };"
    );
}

#[test]
fn navigation_target_serializes_literal_version_and_selection() {
    let target = NavigationTarget {
        version: ContractVersion::V1,
        destination: NavigationDestination::Monitor,
        selection: Some(NavigationSelection {
            kind: NavigationSelectionKind::Printer,
            id: "printer-1".to_string(),
        }),
    };

    assert_eq!(
        serde_json::to_value(target).expect("navigation target should serialize"),
        serde_json::json!({
            "version": 1,
            "destination": "monitor",
            "selection": { "kind": "printer", "id": "printer-1" }
        })
    );
}

#[test]
fn contract_version_serializes_as_one_and_rejects_other_versions() {
    assert_eq!(
        serde_json::to_string(&ContractVersion::V1).expect("version should serialize"),
        "1"
    );
    assert!(serde_json::from_str::<ContractVersion>("1").is_ok());
    assert!(serde_json::from_str::<ContractVersion>("2").is_err());
}

#[test]
fn operational_policy_contracts_export_and_serialize_camel_case_values() {
    let temporary = TempDir::new().expect("temporary export directory should be created");
    write_exports(temporary.path());

    for contract in [
        "domain/HostActivity.ts",
        "domain/OperationalInput.ts",
        "domain/OperationalResult.ts",
        "domain/OperationalState.ts",
        "domain/PrinterReadiness.ts",
        "domain/ReadinessReason.ts",
        "domain/ReadinessState.ts",
        "domain/TelemetryFreshness.ts",
    ] {
        assert!(
            temporary.path().join(contract).exists(),
            "missing operational policy contract {contract}"
        );
    }

    assert_eq!(
        serde_json::json!({
            "operational": [OperationalState::SetupIncomplete, OperationalState::Ready],
            "readiness": PrinterReadiness {
                state: ReadinessState::NotReady,
                reason: Some(ReadinessReason::StaleTelemetry),
            },
            "freshness": TelemetryFreshness::Stale,
        }),
        serde_json::json!({
            "operational": ["setupIncomplete", "ready"],
            "readiness": { "state": "notReady", "reason": "staleTelemetry" },
            "freshness": "stale",
        })
    );
}

#[test]
fn json_number_accepts_javascript_safe_integer_boundaries() {
    let positive = JsonNumber::try_from(9_007_199_254_740_991_i64)
        .expect("positive JS-safe boundary should be accepted");
    let negative = JsonNumber::try_from(-9_007_199_254_740_991_i64)
        .expect("negative JS-safe boundary should be accepted");

    assert_eq!(
        serde_json::json!([positive, negative]),
        serde_json::json!([9_007_199_254_740_991_i64, -9_007_199_254_740_991_i64])
    );
}

#[test]
fn json_number_rejects_integers_outside_javascript_safe_range() {
    assert!(JsonNumber::try_from(9_007_199_254_740_992_i64).is_err());
    assert!(JsonNumber::try_from(-9_007_199_254_740_992_i64).is_err());
    assert!(JsonNumber::try_from(9_007_199_254_740_992_u64).is_err());
    assert!(JsonNumber::try_from(i64::MIN).is_err());
    assert!(JsonNumber::try_from(i64::MAX).is_err());
    assert!(JsonNumber::try_from(u64::MAX).is_err());
    assert!(serde_json::from_str::<JsonValue>("9007199254740992").is_err());
    assert!(serde_json::from_str::<JsonValue>("-9007199254740992").is_err());
}

#[test]
fn json_number_round_trips_a_supported_fractional_value() {
    let value: JsonValue = serde_json::from_str("-1234.125").expect("fraction should parse");

    assert_eq!(
        serde_json::to_string(&value).expect("fraction should serialize"),
        "-1234.125"
    );
}

#[test]
fn json_number_rejects_precision_changing_decimal_tokens() {
    let error = serde_json::from_str::<JsonValue>("0.123456789012345678901")
        .expect_err("a precision-changing decimal must be rejected");
    let rendered = error.to_string();

    assert!(
        rendered
            .contains("JSON number cannot cross the JavaScript boundary without precision loss"),
        "precision error should expose only its safe classification: {rendered}"
    );
    assert!(!rendered.contains("0.123456789012345678901"));
    assert!(!rendered.contains("0.12345678901234568"));
}

#[test]
fn json_number_accepts_canonical_decimal_and_exponent_boundaries() {
    for token in ["0.12345678901234568", "9007199254740991.0", "5e-324"] {
        let value: JsonValue = serde_json::from_str(token)
            .unwrap_or_else(|error| panic!("canonical number {token} should parse: {error}"));

        assert_eq!(
            serde_json::to_string(&value).expect("canonical number should serialize"),
            token
        );
    }
}

#[test]
fn json_number_rejects_noncanonical_or_unsupported_decimal_and_exponent_forms() {
    for token in ["1.00", "1E-7", "1e-324", "9.007199254740992e15"] {
        assert!(
            serde_json::from_str::<JsonValue>(token).is_err(),
            "unsupported number {token} should be rejected"
        );
    }
}

#[test]
fn json_value_round_trips_supported_numbers_in_nested_objects_and_arrays() {
    let source = r#"{"values":[0.125,{"minimum":5e-324},9007199254740991.0]}"#;
    let value: JsonValue =
        serde_json::from_str(source).expect("supported nested numbers should parse");

    assert_eq!(
        serde_json::to_string(&value).expect("supported nested numbers should serialize"),
        source
    );
}

#[test]
fn deeply_nested_unsupported_numbers_use_the_same_redacted_error() {
    fn classification(error: &str) -> &str {
        error.split(" at line ").next().unwrap_or(error)
    }

    const REDACTED: &str =
        "JSON number cannot cross the JavaScript boundary without precision loss";
    const ORIGINAL: &str = "0.123456789012345678901";
    const CANONICAL: &str = "0.12345678901234568";

    let direct = serde_json::from_str::<JsonValue>(ORIGINAL)
        .expect_err("direct unsupported number should fail")
        .to_string();
    let nested = serde_json::from_str::<JsonValue>(&format!(
        r#"{{"outer":[{{"inner":[true,{ORIGINAL}]}}]}}"#
    ))
    .expect_err("deeply nested unsupported number should fail")
    .to_string();
    assert_eq!(classification(&direct), REDACTED);
    assert_eq!(classification(&nested), REDACTED);
    for rendered in [direct, nested] {
        assert!(!rendered.contains(ORIGINAL));
        assert!(!rendered.contains(CANONICAL));
    }
}

#[test]
fn json_number_rejects_non_finite_construction() {
    assert!(JsonNumber::try_from(f64::NAN).is_err());
    assert!(JsonNumber::try_from(f64::INFINITY).is_err());
    assert!(JsonNumber::try_from(f64::NEG_INFINITY).is_err());
}

#[test]
fn event_sequence_accepts_max_safe_integer_and_rejects_larger_values() {
    let maximum = JsSafeInteger::try_from(9_007_199_254_740_991_u64)
        .expect("maximum JS-safe sequence should be accepted");

    assert_eq!(
        serde_json::to_string(&maximum).expect("sequence should serialize"),
        "9007199254740991"
    );
    assert!(JsSafeInteger::try_from(9_007_199_254_740_992_u64).is_err());
    assert!(serde_json::from_str::<JsSafeInteger>("9007199254740992").is_err());
}

#[test]
fn generated_contracts_use_safe_precise_types() {
    let temporary = TempDir::new().expect("temporary export directory should be created");
    write_exports(temporary.path());
    let generated = files_under(temporary.path())
        .into_values()
        .flatten()
        .collect::<Vec<_>>();
    let generated = String::from_utf8(generated).expect("generated contracts should be UTF-8");
    let lower = generated.to_lowercase();

    assert!(
        !generated.contains("any"),
        "generated contracts contain `any`"
    );
    assert!(generated.contains("contractVersion: 1"));
    assert!(generated.contains("version: 1"));
    for forbidden in [
        "password",
        "authorization",
        "authheader",
        "cookie",
        "secret",
        "token",
        "apikey",
        "privatekey",
        "accesskey",
    ] {
        assert!(
            !lower.contains(forbidden),
            "generated contracts contain secret-bearing marker `{forbidden}`"
        );
    }
    assert!(generated.contains("export type ConnectionSubmission"));
    assert!(generated.contains("credential?: string"));
    let persisted = std::fs::read_to_string(temporary.path().join("domain/ConnectionConfig.ts"))
        .expect("generated persisted connection contract should exist");
    assert!(!persisted.contains("credential?:"));
}

#[test]
fn printer_record_contract_has_the_exact_authoritative_shape() {
    let temporary = TempDir::new().expect("temporary export directory should be created");
    write_exports(temporary.path());
    let contract = fs::read_to_string(temporary.path().join("domain/PrinterRecord.ts")).unwrap();

    assert!(contract.contains("export type PrinterRecord ="));
    assert!(contract.contains("overrides:"));
    assert!(contract.contains("profileResolution: ProfileResolution"));
    assert!(contract.contains("connection?: ConnectionConfig"));
    assert!(!contract.contains("group:"));
    assert!(!contract.contains("connection: ConnectionConfig | null"));
}

#[test]
fn printer_mutation_inputs_have_no_monitor_group_field() {
    let temporary = TempDir::new().expect("temporary export directory should be created");
    write_exports(temporary.path());
    for path in ["command/PrinterPatch.ts"] {
        let contract = fs::read_to_string(temporary.path().join(path)).unwrap();
        assert!(!contract.contains("group"), "{path}: {contract}");
    }
}

#[test]
fn command_contracts_use_the_approved_create_settings_and_web_fallback_shapes() {
    let temporary = TempDir::new().expect("temporary export directory should be created");
    write_exports(temporary.path());
    let commands =
        fs::read_to_string(temporary.path().join("command/CommandContracts.ts")).unwrap();
    let settings = fs::read_to_string(temporary.path().join("domain/SettingsRecord.ts")).unwrap();
    let settings_export =
        fs::read_to_string(temporary.path().join("command/SettingsExportOutcome.ts")).unwrap();
    let settings_import =
        fs::read_to_string(temporary.path().join("command/SettingsImportOutcome.ts")).unwrap();
    let printers_export =
        fs::read_to_string(temporary.path().join("command/PrintersExportOutcome.ts")).unwrap();
    let printers_import =
        fs::read_to_string(temporary.path().join("command/PrintersImportOutcome.ts")).unwrap();

    assert!(commands.contains(
        "CreatePrinterRequest = ContractRequest & { name: string; catalogRef: CatalogRef; location?: string; startSafety?: StartSafety; defaultBedType?: string; connection?: ConnectionSubmission }"
    ));
    assert!(!commands.contains("draft: PrinterDraft"));
    assert!(settings.contains("export type SettingsRecord ="));
    assert!(!settings.contains("SettingsWire"));
    for outcome in [
        settings_export,
        settings_import,
        printers_export,
        printers_import,
    ] {
        assert!(outcome.contains(r#"{ "status": "unsupported", reason: "desktopRequired", }"#));
    }
}

fn contract_files(entries: &[(&str, &str)]) -> BTreeMap<PathBuf, Vec<u8>> {
    entries
        .iter()
        .map(|(path, contents)| (PathBuf::from(path), contents.as_bytes().to_vec()))
        .collect()
}

#[test]
fn contract_tree_diagnostics_identify_missing_files() {
    let generated = contract_files(&[("command/A.ts", "a")]);
    let committed = BTreeMap::new();

    assert_eq!(
        compare_contract_trees(&generated, &committed).to_string(),
        "generated contract drift:\nmissing from committed output:\n  - command/A.ts"
    );
}

#[test]
fn contract_tree_diagnostics_identify_stale_extra_files() {
    let generated = BTreeMap::new();
    let committed = contract_files(&[("stale/Z.ts", "z")]);

    assert_eq!(
        compare_contract_trees(&generated, &committed).to_string(),
        "generated contract drift:\nstale/extra in committed output:\n  - stale/Z.ts"
    );
}

#[test]
fn contract_tree_diagnostics_identify_changed_files() {
    let generated = contract_files(&[("event/Event.ts", "new")]);
    let committed = contract_files(&[("event/Event.ts", "old")]);

    assert_eq!(
        compare_contract_trees(&generated, &committed).to_string(),
        "generated contract drift:\nchanged files:\n  - event/Event.ts"
    );
}

#[test]
fn contract_tree_diagnostics_sort_simultaneous_drift_by_class_and_path() {
    let generated = contract_files(&[
        ("missing/Z.ts", "z"),
        ("changed/B.ts", "new"),
        ("missing/A.ts", "a"),
        ("same.ts", "same"),
    ]);
    let committed = contract_files(&[
        ("stale/Z.ts", "z"),
        ("changed/B.ts", "old"),
        ("stale/A.ts", "a"),
        ("same.ts", "same"),
    ]);

    assert_eq!(
        compare_contract_trees(&generated, &committed).to_string(),
        concat!(
            "generated contract drift:\n",
            "missing from committed output:\n",
            "  - missing/A.ts\n",
            "  - missing/Z.ts\n",
            "stale/extra in committed output:\n",
            "  - stale/A.ts\n",
            "  - stale/Z.ts\n",
            "changed files:\n",
            "  - changed/B.ts"
        )
    );
}
