import { invoke, isTauri } from "@tauri-apps/api/core";
import type { CatalogModelSummary, CatalogRef, CatalogVariantSummary, PrinterProfile } from "./types";

let modelsCache: CatalogModelSummary[] | null = null;

/** The ~370-model list, cached after first load — used only by the add-printer flow. */
export async function listCatalogModels(): Promise<CatalogModelSummary[]> {
  if (modelsCache) return modelsCache;
  modelsCache = isTauri()
    ? await invoke<CatalogModelSummary[]>("list_catalog_models")
    : (await fetchWebCatalog()).map((m) => ({ modelId: m.modelId, vendor: m.vendor, model: m.model }));
  return modelsCache;
}

/**
 * Keyed by the (vendor, model) name pair, not modelId — modelId is not unique
 * in the real catalog, so it can't identify a model on its own.
 */
export async function listCatalogVariants(
  vendor: string,
  model: string,
): Promise<CatalogVariantSummary[]> {
  if (!isTauri()) {
    const match = findModel(await fetchWebCatalog(), vendor, model);
    return (match?.variants ?? []).map((v) => ({ variant: v.variant, printerVariant: v.printerVariant }));
  }
  return invoke<CatalogVariantSummary[]>("list_catalog_variants", { vendor, model });
}

export async function previewProfile(catalogRef: CatalogRef): Promise<PrinterProfile> {
  if (!isTauri()) {
    const match = findModel(await fetchWebCatalog(), catalogRef.vendor, catalogRef.model);
    const variant = match?.variants.find((v) => v.variant === catalogRef.variant);
    if (!variant) throw new Error(`No catalog variant matches ${catalogRef.variant}`);
    return toPrinterProfile(variant);
  }
  return invoke<PrinterProfile>("preview_profile", { catalogRef });
}

// --- `just web` fallback: reads the real, bundled catalog directly -------
//
// Vite's dev server serves any file under the project root by path, so this
// reaches the exact same JSON Rust bundles as a Tauri resource — no
// duplicated data, no config change, and it's always in sync with whatever
// `gen-catalog` last produced. Fetch-based, not a static `import`: that
// keeps this entirely out of a production `vite build` (which `just build`
// and `npm run tauri build` both run) — the real Tauri app runs with
// `isTauri() === true` and never executes this path at all.

interface RawCatalogVariant extends PrinterProfile {
  variant: string;
  printerVariant: string;
}

interface RawCatalogModel {
  modelId: string;
  vendor: string;
  model: string;
  variants: RawCatalogVariant[];
}

let webCatalogCache: RawCatalogModel[] | null = null;

async function fetchWebCatalog(): Promise<RawCatalogModel[]> {
  if (webCatalogCache) return webCatalogCache;
  const response = await fetch("/src-tauri/resources/printer-catalog.json");
  const catalog = (await response.json()) as { models: RawCatalogModel[] };
  webCatalogCache = catalog.models;
  return webCatalogCache;
}

function findModel(
  models: RawCatalogModel[],
  vendor: string,
  model: string,
): RawCatalogModel | undefined {
  return models.find((m) => m.vendor === vendor && m.model === model);
}

/** Listed explicitly (not a rest-spread minus `variant`/`printerVariant`) so
 *  `noUnusedLocals` has nothing to complain about and every field this
 *  produces is visibly a real `PrinterProfile` field. */
function toPrinterProfile(variant: RawCatalogVariant): PrinterProfile {
  return {
    bedShape: variant.bedShape,
    printableHeightMm: variant.printableHeightMm,
    bedExcludeAreas: variant.bedExcludeAreas,
    defaultBedType: variant.defaultBedType,
    nozzleDiameterMm: variant.nozzleDiameterMm,
    nozzleType: variant.nozzleType,
    gcodeFlavor: variant.gcodeFlavor,
    hasAuxiliaryFan: variant.hasAuxiliaryFan,
    supportsAirFiltration: variant.supportsAirFiltration,
    supportsMultiFilament: variant.supportsMultiFilament,
    suggestedHostType: variant.suggestedHostType,
  };
}

export interface ResolvedCatalogVariant {
  catalogRef: CatalogRef;
  modelLabel: string;
  variantLabel: string;
  profile: PrinterProfile;
}

/** Used by the `just web` fallback Farm (`printer-store.ts`) to seed its
 *  printers against this same real, bundled catalog — so their profile
 *  data isn't a second, hand-maintained source of truth that can drift
 *  from the actual catalog. Returns `null` rather than throwing when no
 *  match exists, since the fallback Farm should skip a stale spec rather
 *  than crash the whole dev environment over it. */
export async function resolveWebCatalogVariant(
  vendor: string,
  model: string,
  printerVariant: string,
): Promise<ResolvedCatalogVariant | null> {
  const match = findModel(await fetchWebCatalog(), vendor, model);
  const variant = match?.variants.find((v) => v.printerVariant === printerVariant);
  if (!match || !variant) return null;
  return {
    catalogRef: {
      vendor: match.vendor,
      model: match.model,
      variant: variant.variant,
      modelId: match.modelId,
      printerVariant: variant.printerVariant,
    },
    modelLabel: match.model,
    variantLabel: variant.variant,
    profile: toPrinterProfile(variant),
  };
}
