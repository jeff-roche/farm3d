//! The closed F1 command inventory and its generated request/result names.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CommandContract {
    pub command: &'static str,
    pub request: &'static str,
    pub result: &'static str,
}

macro_rules! contracts {
    ($(($command:literal, $request:literal, $result:literal)),+ $(,)?) => {
        pub const COMMAND_CONTRACTS: [CommandContract; 89] = [
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
    (
        "pick_model_files",
        "PickModelFilesRequest",
        "PickModelFilesResult"
    ),
    (
        "inspect_import_selection",
        "InspectImportSelectionRequest",
        "InspectImportSelectionResult"
    ),
    (
        "cancel_import_selection",
        "CancelImportSelectionRequest",
        "CancelImportSelectionResult"
    ),
    ("import_models", "ImportModelsRequest", "ImportModelsResult"),
    ("list_library", "ListLibraryRequest", "ListLibraryResult"),
    (
        "create_project",
        "CreateProjectRequest",
        "CreateProjectResult"
    ),
    (
        "rename_project",
        "RenameProjectRequest",
        "RenameProjectResult"
    ),
    (
        "delete_project",
        "DeleteProjectRequest",
        "DeleteProjectResult"
    ),
    ("update_model", "UpdateModelRequest", "UpdateModelResult"),
    (
        "set_model_projects",
        "SetModelProjectsRequest",
        "SetModelProjectsResult"
    ),
    ("delete_model", "DeleteModelRequest", "DeleteModelResult"),
    (
        "list_model_revisions",
        "ListModelRevisionsRequest",
        "ListModelRevisionsResult"
    ),
    (
        "get_revision_thumbnail",
        "GetRevisionThumbnailRequest",
        "GetRevisionThumbnailResult"
    ),
    (
        "library_content_info",
        "LibraryContentInfoRequest",
        "LibraryContentInfoResult"
    ),
    (
        "check_linked_sources",
        "CheckLinkedSourcesRequest",
        "CheckLinkedSourcesResult"
    ),
    (
        "locate_linked_source",
        "LocateLinkedSourceRequest",
        "LocateLinkedSourceResult"
    ),
    (
        "convert_model_to_managed",
        "ConvertModelToManagedRequest",
        "ConvertModelToManagedResult"
    ),
    (
        "get_slicer_runtime",
        "GetSlicerRuntimeRequest",
        "GetSlicerRuntimeResult"
    ),
    (
        "check_slicer_runtime",
        "CheckSlicerRuntimeRequest",
        "CheckSlicerRuntimeResult"
    ),
    (
        "pick_slicer_engine",
        "PickSlicerEngineRequest",
        "PickSlicerEngineResult"
    ),
    (
        "pick_preset_source",
        "PickPresetSourceRequest",
        "PickPresetSourceResult"
    ),
    (
        "reset_slicer_runtime",
        "ResetSlicerRuntimeRequest",
        "ResetSlicerRuntimeResult"
    ),
    (
        "list_slice_options",
        "ListSliceOptionsRequest",
        "ListSliceOptionsResult"
    ),
    (
        "get_revision_geometry",
        "GetRevisionGeometryRequest",
        "GetRevisionGeometryResult"
    ),
    (
        "get_revision_mesh",
        "GetRevisionMeshRequest",
        "GetRevisionMeshResult"
    ),
    ("list_slicing", "ListSlicingRequest", "ListSlicingResult"),
    (
        "create_preparation",
        "CreatePreparationRequest",
        "CreatePreparationResult"
    ),
    (
        "update_preparation",
        "UpdatePreparationRequest",
        "UpdatePreparationResult"
    ),
    (
        "reload_preparation",
        "ReloadPreparationRequest",
        "ReloadPreparationResult"
    ),
    (
        "delete_preparation",
        "DeletePreparationRequest",
        "DeletePreparationResult"
    ),
    ("start_slice", "StartSliceRequest", "StartSliceResult"),
    (
        "cancel_slice_operation",
        "CancelSliceOperationRequest",
        "CancelSliceOperationResult"
    ),
    (
        "get_slice_operation_log",
        "GetSliceOperationLogRequest",
        "GetSliceOperationLogResult"
    ),
    (
        "list_slice_revisions",
        "ListSliceRevisionsRequest",
        "ListSliceRevisionsResult"
    ),
    (
        "get_slice_revision",
        "GetSliceRevisionRequest",
        "GetSliceRevisionResult"
    ),
    (
        "get_slice_revision_log",
        "GetSliceRevisionLogRequest",
        "GetSliceRevisionLogResult"
    ),
    (
        "create_external_slice_revision",
        "CreateExternalSliceRevisionRequest",
        "CreateExternalSliceRevisionResult"
    ),
    (
        "delete_slice_revision",
        "DeleteSliceRevisionRequest",
        "DeleteSliceRevisionResult"
    ),
    (
        "printer_capabilities",
        "PrinterCapabilitiesRequest",
        "PrinterCapabilitiesResult"
    ),
    (
        "adapter_capability_matrix",
        "AdapterCapabilityMatrixRequest",
        "AdapterCapabilityMatrixResult"
    ),
    (
        "list_host_operations",
        "ListHostOperationsRequest",
        "ListHostOperationsResult"
    ),
    (
        "stage_slice_revision",
        "StageSliceRevisionRequest",
        "StageSliceRevisionResult"
    ),
    (
        "start_staged_artifact",
        "StartStagedArtifactRequest",
        "StartStagedArtifactResult"
    ),
    (
        "pause_host_print",
        "PauseHostPrintRequest",
        "PauseHostPrintResult"
    ),
    (
        "resume_host_print",
        "ResumeHostPrintRequest",
        "ResumeHostPrintResult"
    ),
    (
        "cancel_host_print",
        "CancelHostPrintRequest",
        "CancelHostPrintResult"
    ),
    (
        "reconcile_host_operation",
        "ReconcileHostOperationRequest",
        "ReconcileHostOperationResult"
    ),
    (
        "abandon_host_operation",
        "AbandonHostOperationRequest",
        "AbandonHostOperationResult"
    ),
];

pub fn command_contract_inventory() -> &'static [CommandContract; 89] {
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
export type DeleteTareResult = CommandSuccess<TareMutationResult>;
export type PickModelFilesRequest = ContractRequest & { purpose: SelectionPurpose };
export type PickModelFilesResult = CommandSuccess<ImportSelectionSummary | null>;
export type InspectImportSelectionRequest = ContractRequest & { selectionId: string };
export type InspectImportSelectionResult = CommandSuccess<ImportInspection>;
export type CancelImportSelectionRequest = ContractRequest & { selectionId: string };
export type CancelImportSelectionResult = CommandSuccess<CancelImportSelectionData>;
export type ImportModelsRequest = ContractRequest & { selectionId: string; operationId: string; items: ImportItemRequest[] };
export type ImportModelsResult = CommandSuccess<ImportModelsData>;
export type ListLibraryRequest = NoArgsRequest;
export type ListLibraryResult = CommandSuccess<LibrarySnapshot>;
export type CreateProjectRequest = ContractRequest & { name: string };
export type CreateProjectResult = CommandSuccess<ProjectMutationResult>;
export type RenameProjectRequest = ContractRequest & { id: string; expectedRevision: number; name: string };
export type RenameProjectResult = CommandSuccess<ProjectMutationResult>;
export type DeleteProjectRequest = ContractRequest & { id: string; expectedRevision: number };
export type DeleteProjectResult = CommandSuccess<DeleteProjectData>;
export type UpdateModelRequest = ContractRequest & { id: string; expectedRevision: number; patch: ModelPatch };
export type UpdateModelResult = CommandSuccess<ModelMutationResult>;
export type SetModelProjectsRequest = ContractRequest & { modelId: string; expectedRevision: number; add: string[]; remove: string[] };
export type SetModelProjectsResult = CommandSuccess<ModelMutationResult>;
export type DeleteModelRequest = ContractRequest & { id: string; expectedRevision: number };
export type DeleteModelResult = CommandSuccess<DeleteModelData>;
export type ListModelRevisionsRequest = ContractRequest & { modelId: string };
export type ListModelRevisionsResult = CommandSuccess<ModelSourceRevisionRecord[]>;
export type GetRevisionThumbnailRequest = ContractRequest & { revisionId: string };
export type GetRevisionThumbnailResult = CommandSuccess<RevisionThumbnail | null>;
export type LibraryContentInfoRequest = NoArgsRequest;
export type LibraryContentInfoResult = CommandSuccess<LibraryContentInfo>;
export type CheckLinkedSourcesRequest = ContractRequest & { modelIds?: string[] };
export type CheckLinkedSourcesResult = CommandSuccess<ModelRecord[]>;
export type LocateLinkedSourceRequest = ContractRequest & { modelId: string; expectedRevision: number; selectionId: string; fileIndex: number; acceptDifferentContent: boolean };
export type LocateLinkedSourceResult = CommandSuccess<ModelMutationResult>;
export type ConvertModelToManagedRequest = ContractRequest & { modelId: string; expectedRevision: number };
export type ConvertModelToManagedResult = CommandSuccess<ModelMutationResult>;
export type GetSlicerRuntimeRequest = NoArgsRequest;
export type GetSlicerRuntimeResult = CommandSuccess<SlicerRuntimeStatus>;
export type CheckSlicerRuntimeRequest = NoArgsRequest;
export type CheckSlicerRuntimeResult = CommandSuccess<SlicerRuntimeStatus>;
export type PickSlicerEngineRequest = ContractRequest & { expectedRevision: number };
export type PickSlicerEngineResult = CommandSuccess<SlicerRuntimeStatus | null>;
export type PickPresetSourceRequest = ContractRequest & { expectedRevision: number; kind: PresetSourceKind };
export type PickPresetSourceResult = CommandSuccess<SlicerRuntimeStatus | null>;
export type ResetSlicerRuntimeRequest = ContractRequest & { expectedRevision: number; engine: boolean; presetSource: boolean };
export type ResetSlicerRuntimeResult = CommandSuccess<SlicerRuntimeStatus>;
export type ListSliceOptionsRequest = ContractRequest & { target: SliceTarget };
export type ListSliceOptionsResult = CommandSuccess<SliceOptions>;
export type GetRevisionGeometryRequest = ContractRequest & { revisionId: string };
export type GetRevisionGeometryResult = CommandSuccess<RevisionGeometry>;
export type GetRevisionMeshRequest = ContractRequest & { revisionId: string; objectKey: number };
export type GetRevisionMeshResult = ArrayBuffer;
export type ListSlicingRequest = NoArgsRequest;
export type ListSlicingResult = CommandSuccess<SlicingSnapshot>;
export type CreatePreparationRequest = ContractRequest & { modelId: string; target?: SliceTarget };
export type CreatePreparationResult = CommandSuccess<PreparationRecord>;
export type UpdatePreparationRequest = ContractRequest & { preparationId: string; expectedRevision: number; document: PreparationDocument };
export type UpdatePreparationResult = CommandSuccess<PreparationRecord>;
export type ReloadPreparationRequest = ContractRequest & { preparationId: string; expectedRevision: number };
export type ReloadPreparationResult = CommandSuccess<ReloadPreparationData>;
export type DeletePreparationRequest = ContractRequest & { preparationId: string; expectedRevision: number };
export type DeletePreparationResult = CommandSuccess<SlicingDeleted>;
export type StartSliceRequest = ContractRequest & { operationId: string; preparationId: string; expectedRevision: number; plateKeys: string[]; continueWithSourceRevision?: string };
export type StartSliceResult = CommandSuccess<StartSliceData>;
export type CancelSliceOperationRequest = ContractRequest & { sliceOperationId: string };
export type CancelSliceOperationResult = CommandSuccess<SliceOperationRecord>;
export type GetSliceOperationLogRequest = ContractRequest & { sliceOperationId: string };
export type GetSliceOperationLogResult = CommandSuccess<SliceOperationLog>;
export type ListSliceRevisionsRequest = ContractRequest & { modelId: string };
export type ListSliceRevisionsResult = CommandSuccess<SliceRevisionSummary[]>;
export type GetSliceRevisionRequest = ContractRequest & { sliceRevisionId: string };
export type GetSliceRevisionResult = CommandSuccess<SliceRevisionRecord>;
export type GetSliceRevisionLogRequest = ContractRequest & { sliceRevisionId: string };
export type GetSliceRevisionLogResult = CommandSuccess<SliceRevisionLog>;
export type CreateExternalSliceRevisionRequest = ContractRequest & { operationId: string; sourceRevisionId: string; facts: CreateExternalSliceRevisionFacts };
export type CreateExternalSliceRevisionResult = CommandSuccess<SliceRevisionRecord>;
export type DeleteSliceRevisionRequest = ContractRequest & { sliceRevisionId: string };
export type DeleteSliceRevisionResult = CommandSuccess<SlicingDeleted>;
export type PrinterCapabilitiesRequest = ContractRequest & { printerId: string };
export type PrinterCapabilitiesResult = CommandSuccess<PrinterCapabilities>;
export type AdapterCapabilityMatrixRequest = NoArgsRequest;
export type AdapterCapabilityMatrixResult = CommandSuccess<AdapterCapabilityRow[]>;
export type ListHostOperationsRequest = ContractRequest & { printerId?: string };
export type ListHostOperationsResult = CommandSuccess<HostOperationsSnapshot>;
export type StageSliceRevisionRequest = ContractRequest & { operationId: string; printerId: string; sliceRevisionId: string };
export type StageSliceRevisionResult = CommandSuccess<HostOperation>;
export type StartStagedArtifactRequest = ContractRequest & { operationId: string; printerId: string; hostOperationId: string; priorState: PriorState };
export type StartStagedArtifactResult = CommandSuccess<HostOperation>;
export type PauseHostPrintRequest = ContractRequest & { operationId: string; printerId: string };
export type PauseHostPrintResult = CommandSuccess<HostOperation>;
export type ResumeHostPrintRequest = ContractRequest & { operationId: string; printerId: string };
export type ResumeHostPrintResult = CommandSuccess<HostOperation>;
export type CancelHostPrintRequest = ContractRequest & { operationId: string; printerId: string };
export type CancelHostPrintResult = CommandSuccess<HostOperation>;
export type ReconcileHostOperationRequest = ContractRequest & { hostOperationId: string };
export type ReconcileHostOperationResult = CommandSuccess<HostOperation>;
export type AbandonHostOperationRequest = ContractRequest & { operationId: string; hostOperationId: string; acknowledgement: "hostStateUnknown"; note?: string };
export type AbandonHostOperationResult = CommandSuccess<HostOperation>;"#.to_string()
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
        visitor.visit::<crate::library::selection::SelectionPurpose>();
        visitor.visit::<crate::library::selection::ImportSelectionSummary>();
        visitor.visit::<crate::library::inspection::ImportInspection>();
        visitor.visit::<crate::library::selection::CancelImportSelectionData>();
        visitor.visit::<crate::library::import::ImportItemRequest>();
        visitor.visit::<crate::library::import::ImportModelsResult>();
        visitor.visit::<crate::library::commands::LibrarySnapshot>();
        visitor.visit::<crate::library::commands::ProjectMutationResult>();
        visitor.visit::<crate::library::commands::DeleteProjectResult>();
        visitor.visit::<crate::library::commands::ModelPatch>();
        visitor.visit::<crate::library::commands::ModelMutationResult>();
        visitor.visit::<crate::library::commands::DeleteModelResult>();
        visitor.visit::<crate::library::ModelSourceRevisionRecord>();
        visitor.visit::<crate::library::commands::RevisionThumbnail>();
        visitor.visit::<crate::library::commands::LibraryContentInfo>();
        visitor.visit::<crate::library::ModelRecord>();
        visitor.visit::<crate::slicing::SlicerRuntimeStatus>();
        visitor.visit::<crate::slicing::commands::PresetSourceKind>();
        visitor.visit::<crate::slicing::SliceTarget>();
        visitor.visit::<crate::slicing::SliceOptions>();
        visitor.visit::<crate::slicing::RevisionGeometry>();
        visitor.visit::<crate::slicing::commands::SlicingSnapshot>();
        visitor.visit::<crate::slicing::PreparationRecord>();
        visitor.visit::<crate::slicing::PreparationDocument>();
        visitor.visit::<crate::slicing::preparation::ReloadPreparationData>();
        visitor.visit::<crate::slicing::commands::SlicingDeleted>();
        visitor.visit::<crate::slicing::commands::StartSliceData>();
        visitor.visit::<crate::slicing::SliceOperationRecord>();
        visitor.visit::<crate::slicing::commands::SliceOperationLog>();
        visitor.visit::<crate::slicing::commands::SliceRevisionLog>();
        visitor.visit::<crate::slicing::SliceRevisionSummary>();
        visitor.visit::<crate::slicing::SliceRevisionRecord>();
        visitor.visit::<crate::slicing::external::CreateExternalSliceRevisionFacts>();
        visitor.visit::<crate::connections::capabilities::PrinterCapabilities>();
        visitor.visit::<crate::connections::capabilities::AdapterCapabilityRow>();
        visitor.visit::<crate::host_ops::HostOperationsSnapshot>();
        visitor.visit::<crate::host_ops::HostOperation>();
        visitor.visit::<crate::host_ops::PriorState>();
    }

    fn output_path() -> Option<std::path::PathBuf> {
        Some("command/CommandContracts.ts".into())
    }
}
