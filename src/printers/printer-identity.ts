import type { CatalogVariantSummary } from "./types";

/** If the model's own name already starts with its vendor's, drop that
 *  prefix in the Model dropdown — the Brand dropdown already said it. Falls
 *  back to the full name when the model doesn't literally start with the
 *  vendor string (true for ~80% of the catalog, e.g. not for Bambu Lab,
 *  whose vendor code is "BBL") or when stripping would leave nothing. */
export function stripBrandPrefix(model: string, vendor: string): string {
  if (!model.toLowerCase().startsWith(vendor.toLowerCase())) return model;
  const rest = model.slice(vendor.length).trimStart();
  return rest || model;
}

/** Base name is unique on its own if nothing already has it; otherwise
 *  appends " 2", " 3", … until one is. Prevents three Centauri Carbons from
 *  all defaulting to the literal same name with no warning. */
export function suggestUniqueName(base: string, existingNames: Set<string>): string {
  if (!existingNames.has(base)) return base;
  let n = 2;
  while (existingNames.has(`${base} ${n}`)) n++;
  return `${base} ${n}`;
}

/** Auto-selects the 0.4mm nozzle when it's offered, else the first variant
 *  in catalog order. Returns `undefined` for an empty list (nothing to
 *  default to yet, e.g. while variants are still loading). */
export function pickDefaultVariant<T extends Pick<CatalogVariantSummary, "printerVariant">>(
  variants: readonly T[],
): T | undefined {
  return variants.find((v) => v.printerVariant === "0.4") ?? variants[0];
}
