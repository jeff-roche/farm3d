//! The closed F1 command inventory and its generated request/result names.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandContract {
    pub command: &'static str,
    pub request: &'static str,
    pub result: &'static str,
}

macro_rules! contracts {
    ($(($command:literal, $request:literal, $result:literal)),+ $(,)?) => {
        pub const COMMAND_CONTRACTS: [CommandContract; 23] = [
            $(CommandContract { command: $command, request: $request, result: $result }),+
        ];
    };
}

contracts![
    ("load_settings", "LoadSettingsRequest", "LoadSettingsResult"),
    ("save_settings", "SaveSettingsRequest", "SaveSettingsResult"),
    (
        "export_settings",
        "ExportSettingsRequest",
        "ExportSettingsResult"
    ),
    (
        "import_settings",
        "ImportSettingsRequest",
        "ImportSettingsResult"
    ),
    ("list_printers", "ListPrintersRequest", "ListPrintersResult"),
    (
        "create_printer",
        "CreatePrinterRequest",
        "CreatePrinterResult"
    ),
    (
        "update_printer",
        "UpdatePrinterRequest",
        "UpdatePrinterResult"
    ),
    (
        "delete_printer",
        "DeletePrinterRequest",
        "DeletePrinterResult"
    ),
    (
        "set_printer_override",
        "SetPrinterOverrideRequest",
        "SetPrinterOverrideResult"
    ),
    (
        "rebind_printer",
        "RebindPrinterRequest",
        "RebindPrinterResult"
    ),
    (
        "resolve_profile_drift",
        "ResolveProfileDriftRequest",
        "ResolveProfileDriftResult"
    ),
    (
        "export_printers",
        "ExportPrintersRequest",
        "ExportPrintersResult"
    ),
    (
        "import_printers",
        "ImportPrintersRequest",
        "ImportPrintersResult"
    ),
    (
        "list_catalog_models",
        "ListCatalogModelsRequest",
        "ListCatalogModelsResult"
    ),
    (
        "list_catalog_variants",
        "ListCatalogVariantsRequest",
        "ListCatalogVariantsResult"
    ),
    (
        "preview_profile",
        "PreviewProfileRequest",
        "PreviewProfileResult"
    ),
    ("catalog_info", "CatalogInfoRequest", "CatalogInfoResult"),
    (
        "set_printer_connection",
        "SetPrinterConnectionRequest",
        "SetPrinterConnectionResult"
    ),
    (
        "clear_printer_connection",
        "ClearPrinterConnectionRequest",
        "ClearPrinterConnectionResult"
    ),
    (
        "test_printer_connection",
        "TestPrinterConnectionRequest",
        "TestPrinterConnectionResult"
    ),
    (
        "credential_store_info",
        "CredentialStoreInfoRequest",
        "CredentialStoreInfoResult"
    ),
    (
        "discover_printers",
        "DiscoverPrintersRequest",
        "DiscoverPrintersResult"
    ),
    (
        "printer_statuses",
        "PrinterStatusesRequest",
        "PrinterStatusesResult"
    ),
];

pub fn command_contract_inventory() -> &'static [CommandContract; 23] {
    &COMMAND_CONTRACTS
}

/// The closed command map emitted beside the derived Rust DTO contracts.
/// Request wrappers describe Tauri's flat argument objects; every nested
/// payload and result refers to its corresponding `TS`-derived Rust wire type.
pub struct CommandContracts;

impl ts_rs::TS for CommandContracts {
    type WithoutGenerics = Self;
    type OptionInnerType = Self;

    fn name(_: &ts_rs::Config) -> String {
        "CommandContracts".to_string()
    }

    fn inline(_: &ts_rs::Config) -> String {
        "CommandContracts".to_string()
    }

    fn decl(_: &ts_rs::Config) -> String {
        r#"type ContractRequest = { contractVersion: 1 };
export type LoadSettingsRequest = NoArgsRequest;
export type LoadSettingsResult = CommandSuccess<SettingsRecord>;
export type SaveSettingsRequest = ContractRequest & { expectedRevision: number; themeMode: string };
export type SaveSettingsResult = CommandSuccess<SettingsRecord>;
export type ExportSettingsRequest = NoArgsRequest;
export type ExportSettingsResult = CommandSuccess<SettingsExportOutcome>;
export type ImportSettingsRequest = ContractRequest & { expectedRevision: number };
export type ImportSettingsResult = CommandSuccess<SettingsImportOutcome>;
export type ListPrintersRequest = NoArgsRequest;
export type ListPrintersResult = CommandSuccess<PrinterRecord[]>;
export type CreatePrinterRequest = ContractRequest & { name: string; catalogRef: CatalogRef };
export type CreatePrinterResult = CommandSuccess<PrinterMutationResult>;
export type UpdatePrinterRequest = ContractRequest & { id: string; expectedRevision: number; patch: PrinterPatch };
export type UpdatePrinterResult = CommandSuccess<PrinterMutationResult>;
export type DeletePrinterRequest = ContractRequest & { id: string; expectedRevision: number };
export type DeletePrinterResult = CommandSuccess<DeletePrinterData>;
export type SetPrinterOverrideRequest = ContractRequest & { id: string; expectedRevision: number; field: string; value: JsonValue | null };
export type SetPrinterOverrideResult = CommandSuccess<PrinterMutationResult>;
export type RebindPrinterRequest = ContractRequest & { id: string; expectedRevision: number; catalogRef: CatalogRef };
export type RebindPrinterResult = CommandSuccess<PrinterMutationResult>;
export type ResolveProfileDriftRequest = ContractRequest & { id: string; expectedRevision: number; action: "accept" | "pin" };
export type ResolveProfileDriftResult = CommandSuccess<PrinterMutationResult>;
export type ExportPrintersRequest = NoArgsRequest;
export type ExportPrintersResult = CommandSuccess<PrintersExportOutcome>;
export type ImportPrintersRequest = ContractRequest & { expectedRevisions: PrinterRevisionPrecondition[] };
export type ImportPrintersResult = CommandSuccess<PrintersImportOutcome>;
export type ListCatalogModelsRequest = NoArgsRequest;
export type ListCatalogModelsResult = CommandSuccess<CatalogModelSummary[]>;
export type ListCatalogVariantsRequest = ContractRequest & { vendor: string; model: string };
export type ListCatalogVariantsResult = CommandSuccess<CatalogVariantSummary[]>;
export type PreviewProfileRequest = ContractRequest & { catalogRef: CatalogRef };
export type PreviewProfileResult = CommandSuccess<PrinterProfile>;
export type CatalogInfoRequest = NoArgsRequest;
export type CatalogInfoResult = CommandSuccess<CatalogInfo>;
export type SetPrinterConnectionRequest = ContractRequest & { id: string; expectedRevision: number; submission: ConnectionSubmission };
export type SetPrinterConnectionResult = CommandSuccess<PrinterMutationResult>;
export type ClearPrinterConnectionRequest = ContractRequest & { id: string; expectedRevision: number };
export type ClearPrinterConnectionResult = CommandSuccess<PrinterMutationResult>;
export type TestPrinterConnectionRequest = ContractRequest & { id: string; submission: ConnectionSubmission };
export type TestPrinterConnectionResult = CommandSuccess<ProbeResult>;
export type CredentialStoreInfoRequest = NoArgsRequest;
export type CredentialStoreInfoResult = CommandSuccess<CredentialStoreInfo>;
export type DiscoverPrintersRequest = NoArgsRequest;
export type DiscoverPrintersResult = CommandSuccess<DiscoveredPrinter[]>;
export type PrinterStatusesRequest = NoArgsRequest;
export type PrinterStatusesResult = CommandSuccess<PrinterStatusBackfill>;"#.to_string()
    }

    fn visit_dependencies(visitor: &mut impl ts_rs::TypeVisitor)
    where
        Self: 'static,
    {
        visitor.visit::<crate::contracts::command::CommandSuccess<crate::contracts::command::JsonValue>>();
        visitor.visit::<crate::contracts::command::JsonValue>();
        visitor.visit::<crate::contracts::domain::NoArgsRequest>();
        visitor.visit::<crate::settings::commands::SettingsRecord>();
        visitor.visit::<crate::settings::commands::ExportResult>();
        visitor.visit::<crate::settings::commands::SettingsImportResult>();
        visitor.visit::<crate::printers::commands::PrinterRevisionPrecondition>();
        visitor.visit::<crate::printers::PrinterPatch>();
        visitor.visit::<crate::printers::CatalogRef>();
        visitor.visit::<crate::catalog::PrinterProfile>();
        visitor.visit::<crate::catalog::resolve::ResolvedPrinter>();
        visitor.visit::<crate::printers::commands::PrinterMutationResult>();
        visitor.visit::<crate::printers::commands::DeletePrinterResult>();
        visitor.visit::<crate::printers::commands::ExportResult>();
        visitor.visit::<crate::printers::commands::PrintersImportResult>();
        visitor.visit::<crate::catalog::commands::CatalogModelSummary>();
        visitor.visit::<crate::catalog::commands::CatalogVariantSummary>();
        visitor.visit::<crate::catalog::commands::CatalogInfo>();
        visitor.visit::<crate::connections::commands::ConnectionSubmission>();
        visitor.visit::<crate::connections::ProbeResult>();
        visitor.visit::<crate::connections::commands::CredentialStoreInfo>();
        visitor.visit::<crate::connections::discovery::DiscoveredPrinter>();
        visitor.visit::<crate::connections::supervisor::PrinterStatusBackfill>();
    }

    fn output_path() -> Option<std::path::PathBuf> {
        Some("command/CommandContracts.ts".into())
    }
}
