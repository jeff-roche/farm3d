//! The closed F1 command inventory and its generated request/result names.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandContract {
    pub command: &'static str,
    pub request: &'static str,
    pub result: &'static str,
}

macro_rules! contracts {
    ($(($command:literal, $request:literal, $result:literal)),+ $(,)?) => {
        pub const COMMAND_CONTRACTS: [CommandContract; 41] = [
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
        "set_material_slot_layout",
        "SetMaterialSlotLayoutRequest",
        "SetMaterialSlotLayoutResult"
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
        "printer_lifecycle_eligibility",
        "PrinterLifecycleEligibilityRequest",
        "PrinterLifecycleEligibilityResult"
    ),
    (
        "archive_printer",
        "ArchivePrinterRequest",
        "ArchivePrinterResult"
    ),
    (
        "unarchive_printer",
        "UnarchivePrinterRequest",
        "UnarchivePrinterResult"
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
    (
        "probe_connection",
        "ProbeConnectionRequest",
        "ProbeConnectionResult"
    ),
    (
        "create_printers_batch",
        "CreatePrintersBatchRequest",
        "CreatePrintersBatchResult"
    ),
    (
        "cancel_printer_batch",
        "CancelPrinterBatchRequest",
        "CancelPrinterBatchResult"
    ),
    (
        "list_duplicate_host_archives",
        "ListDuplicateHostArchivesRequest",
        "ListDuplicateHostArchivesResult"
    ),
    ("list_spools", "ListSpoolsRequest", "ListSpoolsResult"),
    ("spool_history", "SpoolHistoryRequest", "SpoolHistoryResult"),
    ("create_spool", "CreateSpoolRequest", "CreateSpoolResult"),
    ("update_spool", "UpdateSpoolRequest", "UpdateSpoolResult"),
    (
        "record_spool_amount",
        "RecordSpoolAmountRequest",
        "RecordSpoolAmountResult"
    ),
    ("move_spool", "MoveSpoolRequest", "MoveSpoolResult"),
    (
        "set_spool_lifecycle",
        "SetSpoolLifecycleRequest",
        "SetSpoolLifecycleResult"
    ),
    ("create_tare", "CreateTareRequest", "CreateTareResult"),
    ("update_tare", "UpdateTareRequest", "UpdateTareResult"),
    ("delete_tare", "DeleteTareRequest", "DeleteTareResult"),
];

pub fn command_contract_inventory() -> &'static [CommandContract; 41] {
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
export type SaveSettingsRequest = ContractRequest & { expectedRevision: number; themeMode: string; monitorSection: MonitorSection; monitorDensity: MonitorDensity };
export type SaveSettingsResult = CommandSuccess<SettingsRecord>;
export type ExportSettingsRequest = NoArgsRequest;
export type ExportSettingsResult = CommandSuccess<SettingsExportOutcome>;
export type ImportSettingsRequest = ContractRequest & { expectedRevision: number };
export type ImportSettingsResult = CommandSuccess<SettingsImportOutcome>;
export type ListPrintersRequest = NoArgsRequest;
export type ListPrintersResult = CommandSuccess<PrinterRecord[]>;
export type CreatePrinterRequest = ContractRequest & { name: string; catalogRef: CatalogRef; location?: string; startSafety?: StartSafety; defaultBedType?: string; connection?: ConnectionSubmission; slotLayout?: SlotSpec[]; initialLoads?: { slotIndex: number; spoolId: string; expectedSpoolRevision: number }[] };
export type CreatePrinterResult = CommandSuccess<PrinterMutationResult>;
export type SetMaterialSlotLayoutRequest = ContractRequest & { printerId: string; expectedRevision: number; slots: SlotSpec[] };
export type SetMaterialSlotLayoutResult = CommandSuccess<PrinterMutationResult>;
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
export type PrinterLifecycleEligibilityRequest = ContractRequest & { id: string };
export type PrinterLifecycleEligibilityResult = CommandSuccess<LifecycleEligibility>;
export type ArchivePrinterRequest = ContractRequest & { id: string; expectedRevision: number; operationId: string; spoolDispositions: SpoolDispositionInput[] };
export type ArchivePrinterResult = CommandSuccess<PrinterMutationResult>;
export type UnarchivePrinterRequest = ContractRequest & { id: string; expectedRevision: number };
export type UnarchivePrinterResult = CommandSuccess<PrinterMutationResult>;
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
export type SetPrinterConnectionRequest = ContractRequest & { id: string; expectedRevision: number; submission: ConnectionSubmission; acceptUnverified?: boolean };
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
export type PrinterStatusesResult = CommandSuccess<PrinterStatusBackfill>;
export type ProbeConnectionRequest = ContractRequest & { submission: ConnectionSubmission };
export type ProbeConnectionResult = CommandSuccess<ProbeResult>;
export type CreatePrintersBatchRequest = ContractRequest & { input: CreatePrintersBatchInput };
export type CreatePrintersBatchResult = CommandSuccess<CreatePrintersBatchOutput>;
export type CancelPrinterBatchRequest = ContractRequest & { batchId: string };
export type CancelPrinterBatchResult = CommandSuccess<CancelPrinterBatchData>;
export type ListDuplicateHostArchivesRequest = NoArgsRequest;
export type ListDuplicateHostArchivesResult = CommandSuccess<DuplicateHostArchive[]>;
export type ListSpoolsRequest = NoArgsRequest;
export type ListSpoolsResult = CommandSuccess<InventorySnapshot>;
export type SpoolHistoryRequest = ContractRequest & { spoolId: string };
export type SpoolHistoryResult = CommandSuccess<SpoolHistory>;
export type CreateSpoolRequest = ContractRequest & { fields: SpoolFields; initialAmount: AmountEntry; storageLabel?: string };
export type CreateSpoolResult = CommandSuccess<SpoolMutationResult>;
export type UpdateSpoolRequest = ContractRequest & { id: string; expectedRevision: number; patch: SpoolFields };
export type UpdateSpoolResult = CommandSuccess<SpoolMutationResult>;
export type RecordSpoolAmountRequest = ContractRequest & { id: string; expectedRevision: number; entry: AmountEntry; note?: string };
export type RecordSpoolAmountResult = CommandSuccess<SpoolMutationResult>;
export type MoveSpoolRequest = ContractRequest & { operationId: string; spoolId: string; expectedSpoolRevision: number; destination: MoveDestination };
export type MoveSpoolResult = CommandSuccess<MoveSpoolData>;
export type SetSpoolLifecycleRequest = ContractRequest & { operationId: string; id: string; expectedRevision: number; action: SpoolLifecycleAction; storageLabel?: string };
export type SetSpoolLifecycleResult = CommandSuccess<SpoolMutationResult>;
export type CreateTareRequest = ContractRequest & { name: string; weightMg: number };
export type CreateTareResult = CommandSuccess<TareMutationResult>;
export type UpdateTareRequest = ContractRequest & { id: string; expectedRevision: number; name: string; weightMg: number };
export type UpdateTareResult = CommandSuccess<TareMutationResult>;
export type DeleteTareRequest = ContractRequest & { id: string; expectedRevision: number };
export type DeleteTareResult = CommandSuccess<TareMutationResult>;"#.to_string()
    }

    fn visit_dependencies(visitor: &mut impl ts_rs::TypeVisitor)
    where
        Self: 'static,
    {
        visitor.visit::<crate::contracts::command::CommandSuccess<crate::contracts::command::JsonValue>>();
        visitor.visit::<crate::contracts::command::JsonValue>();
        visitor.visit::<crate::contracts::domain::NoArgsRequest>();
        visitor.visit::<crate::settings::commands::SettingsRecord>();
        visitor.visit::<crate::settings::commands::MonitorSection>();
        visitor.visit::<crate::settings::commands::MonitorDensity>();
        visitor.visit::<crate::settings::commands::ExportResult>();
        visitor.visit::<crate::settings::commands::SettingsImportResult>();
        visitor.visit::<crate::printers::commands::PrinterRevisionPrecondition>();
        visitor.visit::<crate::printers::PrinterPatch>();
        visitor.visit::<crate::printers::CatalogRef>();
        visitor.visit::<crate::printers::StartSafety>();
        visitor.visit::<crate::catalog::PrinterProfile>();
        visitor.visit::<crate::catalog::resolve::ResolvedPrinter>();
        // `MaterialSlot` isn't visited here: it's never referenced BY NAME
        // in this file's own hand-written decl string (only `SlotSpec` is,
        // in `CreatePrinterRequest`/`SetMaterialSlotLayoutRequest`) —
        // `ResolvedPrinter`'s own `material_slots: Vec<MaterialSlot>` field
        // already pulls it in as PrinterRecord.ts's dependency. Visiting it
        // here too would add an unused import to CommandContracts.ts.
        visitor.visit::<crate::spools::slots::SlotSpec>();
        visitor.visit::<crate::printers::commands::PrinterMutationResult>();
        visitor.visit::<crate::printers::lifecycle::LifecycleEligibility>();
        visitor.visit::<crate::spools::dispositions::SpoolDispositionInput>();
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
        visitor.visit::<crate::printers::batch::CreatePrintersBatchInput>();
        visitor.visit::<crate::printers::batch::CreatePrintersBatchOutput>();
        visitor.visit::<crate::printers::batch::CancelPrinterBatchData>();
        visitor.visit::<crate::printers::host_identity::DuplicateHostArchive>();
        visitor.visit::<crate::spools::commands::InventorySnapshot>();
        visitor.visit::<crate::spools::commands::SpoolHistory>();
        visitor.visit::<crate::spools::SpoolFields>();
        visitor.visit::<crate::spools::ledger::AmountEntry>();
        visitor.visit::<crate::spools::commands::SpoolMutationResult>();
        visitor.visit::<crate::spools::movement::MoveDestination>();
        visitor.visit::<crate::spools::commands::MoveSpoolResult>();
        visitor.visit::<crate::spools::lifecycle::SpoolLifecycleAction>();
        visitor.visit::<crate::spools::commands::TareMutationResult>();
    }

    fn output_path() -> Option<std::path::PathBuf> {
        Some("command/CommandContracts.ts".into())
    }
}
