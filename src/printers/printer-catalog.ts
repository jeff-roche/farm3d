import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CatalogModelSummary, CatalogRef, CatalogVariantSummary, PrinterProfile } from "./types";

let modelsCache: CatalogModelSummary[] | null = null;

/** The ~370-model list, cached after first load — used only by the add-printer flow. */
export async function listCatalogModels(): Promise<CatalogModelSummary[]> {
  if (!isTauri()) return [];
  if (modelsCache) return modelsCache;
  modelsCache = await invoke<CatalogModelSummary[]>("list_catalog_models");
  return modelsCache;
}

export async function listCatalogVariants(modelId: string): Promise<CatalogVariantSummary[]> {
  if (!isTauri()) return [];
  return invoke<CatalogVariantSummary[]>("list_catalog_variants", { modelId });
}

export async function previewProfile(catalogRef: CatalogRef): Promise<PrinterProfile> {
  return invoke<PrinterProfile>("preview_profile", { catalogRef });
}
