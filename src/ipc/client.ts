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

/** The sole direct Tauri invoke boundary. */
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
    if (isCommandError(error)) throw error;
    const receivedVersion = error && typeof error === "object" && "contractVersion" in error
      ? (error as { contractVersion?: unknown }).contractVersion
      : undefined;
    if (receivedVersion !== undefined) throw incompatible(receivedVersion);
    throw error;
  }
  if (response?.contractVersion !== 1) throw incompatible(response?.contractVersion);
  return response.data;
}

export function desktopAvailable(): boolean {
  return isTauri();
}
