import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type * as Contracts from "../generated/contracts/command/CommandContracts";

type CommandMap = {
  load_settings: [Contracts.LoadSettingsRequest, Contracts.LoadSettingsResult];
  save_settings: [Contracts.SaveSettingsRequest, Contracts.SaveSettingsResult];
  export_settings: [Contracts.ExportSettingsRequest, Contracts.ExportSettingsResult];
  import_settings: [Contracts.ImportSettingsRequest, Contracts.ImportSettingsResult];
  list_printers: [Contracts.ListPrintersRequest, Contracts.ListPrintersResult];
  create_printer: [Contracts.CreatePrinterRequest, Contracts.CreatePrinterResult];
  set_material_slot_layout: [
    Contracts.SetMaterialSlotLayoutRequest,
    Contracts.SetMaterialSlotLayoutResult,
  ];
  update_printer: [Contracts.UpdatePrinterRequest, Contracts.UpdatePrinterResult];
  delete_printer: [Contracts.DeletePrinterRequest, Contracts.DeletePrinterResult];
  set_printer_override: [Contracts.SetPrinterOverrideRequest, Contracts.SetPrinterOverrideResult];
  rebind_printer: [Contracts.RebindPrinterRequest, Contracts.RebindPrinterResult];
  resolve_profile_drift: [Contracts.ResolveProfileDriftRequest, Contracts.ResolveProfileDriftResult];
  export_printers: [Contracts.ExportPrintersRequest, Contracts.ExportPrintersResult];
  import_printers: [Contracts.ImportPrintersRequest, Contracts.ImportPrintersResult];
  list_catalog_models: [Contracts.ListCatalogModelsRequest, Contracts.ListCatalogModelsResult];
  list_catalog_variants: [Contracts.ListCatalogVariantsRequest, Contracts.ListCatalogVariantsResult];
  preview_profile: [Contracts.PreviewProfileRequest, Contracts.PreviewProfileResult];
  catalog_info: [Contracts.CatalogInfoRequest, Contracts.CatalogInfoResult];
  set_printer_connection: [Contracts.SetPrinterConnectionRequest, Contracts.SetPrinterConnectionResult];
  clear_printer_connection: [Contracts.ClearPrinterConnectionRequest, Contracts.ClearPrinterConnectionResult];
  test_printer_connection: [Contracts.TestPrinterConnectionRequest, Contracts.TestPrinterConnectionResult];
  credential_store_info: [Contracts.CredentialStoreInfoRequest, Contracts.CredentialStoreInfoResult];
  discover_printers: [Contracts.DiscoverPrintersRequest, Contracts.DiscoverPrintersResult];
  printer_statuses: [Contracts.PrinterStatusesRequest, Contracts.PrinterStatusesResult];
  probe_connection: [Contracts.ProbeConnectionRequest, Contracts.ProbeConnectionResult];
  create_printers_batch: [Contracts.CreatePrintersBatchRequest, Contracts.CreatePrintersBatchResult];
  cancel_printer_batch: [Contracts.CancelPrinterBatchRequest, Contracts.CancelPrinterBatchResult];
  printer_lifecycle_eligibility: [
    Contracts.PrinterLifecycleEligibilityRequest,
    Contracts.PrinterLifecycleEligibilityResult,
  ];
  archive_printer: [Contracts.ArchivePrinterRequest, Contracts.ArchivePrinterResult];
  unarchive_printer: [Contracts.UnarchivePrinterRequest, Contracts.UnarchivePrinterResult];
  list_duplicate_host_archives: [
    Contracts.ListDuplicateHostArchivesRequest,
    Contracts.ListDuplicateHostArchivesResult,
  ];
  list_spools: [Contracts.ListSpoolsRequest, Contracts.ListSpoolsResult];
  spool_history: [Contracts.SpoolHistoryRequest, Contracts.SpoolHistoryResult];
  create_spool: [Contracts.CreateSpoolRequest, Contracts.CreateSpoolResult];
  update_spool: [Contracts.UpdateSpoolRequest, Contracts.UpdateSpoolResult];
  record_spool_amount: [Contracts.RecordSpoolAmountRequest, Contracts.RecordSpoolAmountResult];
  move_spool: [Contracts.MoveSpoolRequest, Contracts.MoveSpoolResult];
  set_spool_lifecycle: [Contracts.SetSpoolLifecycleRequest, Contracts.SetSpoolLifecycleResult];
  create_tare: [Contracts.CreateTareRequest, Contracts.CreateTareResult];
  update_tare: [Contracts.UpdateTareRequest, Contracts.UpdateTareResult];
  delete_tare: [Contracts.DeleteTareRequest, Contracts.DeleteTareResult];
  pick_model_files: [Contracts.PickModelFilesRequest, Contracts.PickModelFilesResult];
  inspect_import_selection: [
    Contracts.InspectImportSelectionRequest,
    Contracts.InspectImportSelectionResult,
  ];
  cancel_import_selection: [
    Contracts.CancelImportSelectionRequest,
    Contracts.CancelImportSelectionResult,
  ];
  import_models: [Contracts.ImportModelsRequest, Contracts.ImportModelsResult];
  list_library: [Contracts.ListLibraryRequest, Contracts.ListLibraryResult];
  create_project: [Contracts.CreateProjectRequest, Contracts.CreateProjectResult];
  rename_project: [Contracts.RenameProjectRequest, Contracts.RenameProjectResult];
  delete_project: [Contracts.DeleteProjectRequest, Contracts.DeleteProjectResult];
  update_model: [Contracts.UpdateModelRequest, Contracts.UpdateModelResult];
  set_model_projects: [Contracts.SetModelProjectsRequest, Contracts.SetModelProjectsResult];
  delete_model: [Contracts.DeleteModelRequest, Contracts.DeleteModelResult];
  list_model_revisions: [Contracts.ListModelRevisionsRequest, Contracts.ListModelRevisionsResult];
  get_revision_thumbnail: [
    Contracts.GetRevisionThumbnailRequest,
    Contracts.GetRevisionThumbnailResult,
  ];
  library_content_info: [Contracts.LibraryContentInfoRequest, Contracts.LibraryContentInfoResult];
  check_linked_sources: [Contracts.CheckLinkedSourcesRequest, Contracts.CheckLinkedSourcesResult];
  locate_linked_source: [Contracts.LocateLinkedSourceRequest, Contracts.LocateLinkedSourceResult];
  convert_model_to_managed: [
    Contracts.ConvertModelToManagedRequest,
    Contracts.ConvertModelToManagedResult,
  ];
  // P5. `get_revision_mesh` is deliberately absent: it answers with raw
  // bytes, not the JSON envelope, so `command()` can't carry it. It lives
  // in `BinaryCommandMap`, invoked through `binaryCommand()`.
  get_slicer_runtime: [Contracts.GetSlicerRuntimeRequest, Contracts.GetSlicerRuntimeResult];
  check_slicer_runtime: [Contracts.CheckSlicerRuntimeRequest, Contracts.CheckSlicerRuntimeResult];
  pick_slicer_engine: [Contracts.PickSlicerEngineRequest, Contracts.PickSlicerEngineResult];
  pick_preset_source: [Contracts.PickPresetSourceRequest, Contracts.PickPresetSourceResult];
  reset_slicer_runtime: [Contracts.ResetSlicerRuntimeRequest, Contracts.ResetSlicerRuntimeResult];
  list_slice_options: [Contracts.ListSliceOptionsRequest, Contracts.ListSliceOptionsResult];
  get_revision_geometry: [
    Contracts.GetRevisionGeometryRequest,
    Contracts.GetRevisionGeometryResult,
  ];
  list_slicing: [Contracts.ListSlicingRequest, Contracts.ListSlicingResult];
  create_preparation: [Contracts.CreatePreparationRequest, Contracts.CreatePreparationResult];
  update_preparation: [Contracts.UpdatePreparationRequest, Contracts.UpdatePreparationResult];
  reload_preparation: [Contracts.ReloadPreparationRequest, Contracts.ReloadPreparationResult];
  delete_preparation: [Contracts.DeletePreparationRequest, Contracts.DeletePreparationResult];
  start_slice: [Contracts.StartSliceRequest, Contracts.StartSliceResult];
  cancel_slice_operation: [
    Contracts.CancelSliceOperationRequest,
    Contracts.CancelSliceOperationResult,
  ];
  get_slice_operation_log: [
    Contracts.GetSliceOperationLogRequest,
    Contracts.GetSliceOperationLogResult,
  ];
  list_slice_revisions: [Contracts.ListSliceRevisionsRequest, Contracts.ListSliceRevisionsResult];
  get_slice_revision: [Contracts.GetSliceRevisionRequest, Contracts.GetSliceRevisionResult];
  get_slice_revision_log: [Contracts.GetSliceRevisionLogRequest, Contracts.GetSliceRevisionLogResult];
  create_external_slice_revision: [
    Contracts.CreateExternalSliceRevisionRequest,
    Contracts.CreateExternalSliceRevisionResult,
  ];
  delete_slice_revision: [
    Contracts.DeleteSliceRevisionRequest,
    Contracts.DeleteSliceRevisionResult,
  ];
  printer_capabilities: [Contracts.PrinterCapabilitiesRequest, Contracts.PrinterCapabilitiesResult];
  adapter_capability_matrix: [
    Contracts.AdapterCapabilityMatrixRequest,
    Contracts.AdapterCapabilityMatrixResult,
  ];
};

/** Commands that answer with raw bytes rather than the JSON envelope. */
export type BinaryCommandMap = {
  get_revision_mesh: [Contracts.GetRevisionMeshRequest, Contracts.GetRevisionMeshResult];
};

type RequestArgs<K extends keyof CommandMap> = Omit<CommandMap[K][0], "contractVersion">;
type ResultData<K extends keyof CommandMap> = CommandMap[K][1] extends { data: infer T } ? T : never;
type CommandArguments<K extends keyof CommandMap> = keyof RequestArgs<K> extends never
  ? [args?: RequestArgs<K>]
  : [args: RequestArgs<K>];

export function isCommandError(value: unknown): value is CommandError {
  if (!value || typeof value !== "object") return false;
  const candidate = value as Partial<CommandError>;
  return (
    candidate.contractVersion === 1 &&
    typeof candidate.code === "string" &&
    typeof candidate.message === "string" &&
    Array.isArray(candidate.recovery) &&
    typeof candidate.retryable === "boolean"
  );
}

function incompatible(receivedVersion: unknown): CommandError {
  return {
    contractVersion: 1,
    code: "INCOMPATIBLE_CONTRACT_VERSION",
    message: "This farm3d command contract is not compatible with the application.",
    recovery: ["UPGRADE_FARM3D"],
    retryable: false,
    details: {
      supportedVersion: 1,
      receivedVersion: typeof receivedVersion === "number" ? receivedVersion : -1,
    },
  };
}

/** A rejected invoke: a valid `CommandError` as is, an envelope of another
 *  contract version as `INCOMPATIBLE_CONTRACT_VERSION`, anything else (a
 *  transport failure) unchanged. */
function invokeFailure(error: unknown): unknown {
  if (isCommandError(error)) return error;
  const receivedVersion = error && typeof error === "object" && "contractVersion" in error
    ? (error as { contractVersion?: unknown }).contractVersion
    : undefined;
  return receivedVersion !== undefined ? incompatible(receivedVersion) : error;
}

/** The sole direct Tauri invoke boundary for JSON commands. */
export async function command<K extends keyof CommandMap>(
  name: K,
  ...[args = {} as RequestArgs<K>]: CommandArguments<K>
): Promise<ResultData<K>> {
  let response: { contractVersion: number; data: ResultData<K> };
  try {
    response = await invoke<{ contractVersion: number; data: ResultData<K> }>(name, {
      contractVersion: 1,
      ...args,
    });
  } catch (error) {
    throw invokeFailure(error);
  }
  if (response?.contractVersion !== 1) throw incompatible(response?.contractVersion);
  return response.data;
}

type BinaryRequestArgs<K extends keyof BinaryCommandMap> = Omit<BinaryCommandMap[K][0], "contractVersion">;

function isArrayBuffer(value: unknown): value is ArrayBuffer {
  return value instanceof ArrayBuffer || Object.prototype.toString.call(value) === "[object ArrayBuffer]";
}

/** The invoke boundary for commands that answer with raw bytes
 *  (`tauri::ipc::Response`) instead of the JSON envelope. Failures are the
 *  same `CommandError`s `command` rejects with. Tauri hands raw bytes over
 *  as an `ArrayBuffer`; a typed-array view or a plain byte array (the
 *  fallback IPC path) is copied into one. */
export async function binaryCommand<K extends keyof BinaryCommandMap>(
  name: K,
  args: BinaryRequestArgs<K>,
): Promise<ArrayBuffer> {
  let response: unknown;
  try {
    response = await invoke<unknown>(name, { contractVersion: 1, ...args });
  } catch (error) {
    throw invokeFailure(error);
  }
  if (isArrayBuffer(response)) return response;
  if (ArrayBuffer.isView(response)) {
    const view = response;
    return new Uint8Array(view.buffer, view.byteOffset, view.byteLength).slice().buffer;
  }
  if (Array.isArray(response) && response.every((byte) => Number.isInteger(byte) && byte >= 0 && byte <= 255)) {
    return Uint8Array.from(response as number[]).buffer;
  }
  throw incompatible(undefined);
}

/** Runs `attempt`, and runs it once more if it fails with a transport error
 *  (anything `command` rejects with that isn't a `CommandError`), since the
 *  first call may have committed before the failure. Only for commands
 *  idempotent by `operationId` (spec D6/D13): `attempt` must send the SAME
 *  `operationId` both times, so the backend replays a committed first try
 *  instead of applying it twice. A `CommandError` is the backend's answer
 *  and is never retried. */
export async function retryOnTransportFailure<T>(attempt: () => Promise<T>): Promise<T> {
  try {
    return await attempt();
  } catch (error) {
    if (isCommandError(error)) throw error;
    return attempt();
  }
}

export function desktopAvailable(): boolean {
  return isTauri();
}

/** Web mode (`just web`) has no backend, so a store refuses anything that
 *  needs real files, a slicer, or the desktop's pickers with this, rather
 *  than faking it. */
export function needsDesktopError(action: string): CommandError {
  return {
    contractVersion: 1,
    code: "PERSISTENCE_UNAVAILABLE",
    message: `${action} needs the desktop app.`,
    recovery: [],
    retryable: false,
  };
}

/**
 * D8's demo aid: seeds a reservation on a Spool so the `reserved` facet can
 * be checked by hand. The command exists only in debug desktop builds and is
 * deliberately outside `CommandMap` (P3 exposes no reservation command).
 */
export async function debugSeedReservation(spoolId: string, amountMg: number): Promise<void> {
  if (!import.meta.env.DEV) {
    throw new Error("debugSeedReservation is available only in development builds.");
  }
  await invoke("debug_seed_reservation", { contractVersion: 1, spoolId, amountMg });
}
