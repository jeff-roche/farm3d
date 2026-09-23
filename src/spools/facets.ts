import type { MaterialFamily } from "../generated/contracts/domain/MaterialFamily";
import type { PrinterRecord } from "../generated/contracts/domain/PrinterRecord";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";

/** The facet chips in the Spool inventory toolbar. Every one names a
 *  Rust-derived field already on `SpoolRecord.facets` -- this module
 *  composes filters over them, and never recomputes any of them itself
 *  (global constraint: the frontend never derives facets). */
export type SpoolFacetKey = "loaded" | "reserved" | "low" | "measured" | "estimated";

export interface SpoolFilter {
  lifecycle: "active" | "empty" | "archived" | "all";
  facets: Set<SpoolFacetKey>;
  materials: Set<MaterialFamily>;
  printerId: string | null;
  query: string;
}

/** The inventory toolbar's initial state: Active Spools, no facet/material/
 *  Printer narrowing, no search. `isFiltered` compares against this. */
export const DEFAULT_SPOOL_FILTER: SpoolFilter = {
  lifecycle: "active",
  facets: new Set(),
  materials: new Set(),
  printerId: null,
  query: "",
};

function matchesFacets(spool: SpoolRecord, facets: Set<SpoolFacetKey>): boolean {
  for (const facet of facets) {
    switch (facet) {
      case "loaded":
        if (!spool.facets.loaded) return false;
        break;
      case "reserved":
        if (!spool.facets.reserved) return false;
        break;
      case "low":
        if (!spool.facets.low) return false;
        break;
      case "measured":
        if (spool.facets.confidence !== "measured") return false;
        break;
      case "estimated":
        if (spool.facets.confidence !== "estimated") return false;
        break;
    }
  }
  return true;
}

/** The Printer a Spool currently sits on, cross-referenced against each
 *  Printer's own `materialSlots` (D12's `occupantSpoolId`) rather than only
 *  trusting `SpoolRecord.location` -- both are Rust-derived and normally
 *  agree, but this is the more direct source for "which Printer holds this
 *  Spool right now" given the `printers` this function is handed. */
function printerIdForSpool(spool: SpoolRecord, printers: PrinterRecord[]): string | null {
  for (const printer of printers) {
    if (printer.materialSlots.some((slot) => slot.occupantSpoolId === spool.id)) return printer.id;
  }
  return spool.location.kind === "slot" ? spool.location.printerId : null;
}

function matchesQuery(spool: SpoolRecord, query: string): boolean {
  const q = query.trim().toLowerCase();
  if (q === "") return true;
  const candidates: (string | null | undefined)[] = [
    `#${spool.spoolNumber}`,
    String(spool.spoolNumber),
    spool.manufacturer,
    spool.product,
    spool.colorName,
    spool.location.kind === "storage" ? spool.location.storageLabel : null,
  ];
  return candidates.some((candidate) => candidate != null && candidate.toLowerCase().includes(q));
}

/** Composes every toolbar filter (lifecycle, facets, materials, Printer,
 *  search) over a Spool list. Every clause is AND'd together -- the result
 *  is their intersection. */
export function applySpoolFilter(spools: SpoolRecord[], printers: PrinterRecord[], f: SpoolFilter): SpoolRecord[] {
  return spools.filter((spool) => {
    if (f.lifecycle !== "all" && spool.lifecycle !== f.lifecycle) return false;
    if (!matchesFacets(spool, f.facets)) return false;
    if (f.materials.size > 0 && !f.materials.has(spool.materialFamily)) return false;
    if (f.printerId !== null && printerIdForSpool(spool, printers) !== f.printerId) return false;
    if (!matchesQuery(spool, f.query)) return false;
    return true;
  });
}

/** Whether `f` differs from `DEFAULT_SPOOL_FILTER` -- drives the toolbar's
 *  "Clear filters" affordance and the filtered-empty vs. plain-empty state. */
export function isFiltered(f: SpoolFilter): boolean {
  return (
    f.lifecycle !== DEFAULT_SPOOL_FILTER.lifecycle ||
    f.facets.size > 0 ||
    f.materials.size > 0 ||
    f.printerId !== null ||
    f.query.trim() !== ""
  );
}
