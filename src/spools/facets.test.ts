import { describe, expect, it } from "vitest";
import { applySpoolFilter, DEFAULT_SPOOL_FILTER, isFiltered, type SpoolFilter } from "./facets";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { PrinterRecord } from "../generated/contracts/domain/PrinterRecord";

let nextNumber = 1;

function spool(overrides: Partial<SpoolRecord> = {}): SpoolRecord {
  const number = overrides.spoolNumber ?? nextNumber++;
  return {
    id: `spl-${number}`,
    revision: 1,
    spoolNumber: number,
    manufacturer: "GenericCo",
    product: "Standard",
    materialFamily: "PLA",
    colorName: "Slate Gray",
    diameter: "1.75",
    nominalMg: 1_000_000,
    lowThresholdMg: 100_000,
    lifecycle: "active",
    location: { kind: "storage", storageLabel: null },
    availability: { currentMg: 500_000, reservedMg: 0, availableMg: 500_000 },
    facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
    createdAt: "2026-09-01T00:00:00Z",
    updatedAt: "2026-09-01T00:00:00Z",
    ...overrides,
  };
}

const A_PRINTER: PrinterRecord = {
  id: "prn-1",
  revision: 1,
  name: "Bay 1",
  notes: "",
  overrides: {},
  catalogRef: { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" },
  startSafety: "confirmBedClear",
  materialSlots: [{ id: "slt-1", position: 0, name: "Main" }],
  setupGaps: [],
  profileResolution: {
    catalogStatus: "ok",
    modelLabel: "Elegoo Centauri Carbon",
    variantLabel: "0.4 nozzle",
    profile: {
      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
      nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
      hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true, suggestedHostType: null,
    },
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
  },
  createdAt: "2026-09-01T00:00:00Z",
  updatedAt: "2026-09-01T00:00:00Z",
};

function filter(overrides: Partial<SpoolFilter> = {}): SpoolFilter {
  return {
    lifecycle: DEFAULT_SPOOL_FILTER.lifecycle,
    facets: new Set(DEFAULT_SPOOL_FILTER.facets),
    materials: new Set(DEFAULT_SPOOL_FILTER.materials),
    printerId: DEFAULT_SPOOL_FILTER.printerId,
    query: DEFAULT_SPOOL_FILTER.query,
    ...overrides,
  };
}

describe("applySpoolFilter", () => {
  it("composes Low + Estimated + PLA + Printer X into their intersection", () => {
    const printerX = { ...A_PRINTER, id: "prn-x", materialSlots: [{ id: "slt-x", position: 0, name: "Main", occupantSpoolId: "spl-match" }] };
    const printerY = { ...A_PRINTER, id: "prn-y", materialSlots: [{ id: "slt-y", position: 0, name: "Main", occupantSpoolId: "spl-wrong-printer" }] };

    // Matches every facet plus the material and Printer filter.
    const match = spool({
      id: "spl-match", materialFamily: "PLA",
      location: { kind: "slot", slotId: "slt-x", printerId: "prn-x" },
      facets: { loaded: true, reserved: false, low: true, confidence: "estimated" },
    });
    // Low + Estimated + PLA, but on the wrong Printer.
    const wrongPrinter = spool({
      id: "spl-wrong-printer", materialFamily: "PLA",
      location: { kind: "slot", slotId: "slt-y", printerId: "prn-y" },
      facets: { loaded: true, reserved: false, low: true, confidence: "estimated" },
    });
    // Low + Estimated + on Printer X, but PETG.
    const wrongMaterial = spool({
      materialFamily: "PETG",
      location: { kind: "storage", storageLabel: null },
      facets: { loaded: false, reserved: false, low: true, confidence: "estimated" },
    });
    // PLA + Printer X-ish + Low, but measured (not estimated).
    const wrongConfidence = spool({
      materialFamily: "PLA",
      location: { kind: "storage", storageLabel: null },
      facets: { loaded: false, reserved: false, low: true, confidence: "measured" },
    });

    const result = applySpoolFilter(
      [match, wrongPrinter, wrongMaterial, wrongConfidence],
      [printerX, printerY],
      filter({
        lifecycle: "all",
        facets: new Set(["low", "estimated"]),
        materials: new Set(["PLA"]),
        printerId: "prn-x",
      }),
    );

    expect(result.map((s) => s.id)).toEqual(["spl-match"]);
  });

  it("matches the search query against the spool number, color, manufacturer, and storage label", () => {
    const byNumber = spool({ spoolNumber: 12 });
    const byColor = spool({ colorName: "Galaxy Black" });
    const byManufacturer = spool({ manufacturer: "Polymaker" });
    const byStorage = spool({ location: { kind: "storage", storageLabel: "Dry box 2" } });
    const noMatch = spool({ manufacturer: "eSun", colorName: "Red", location: { kind: "storage", storageLabel: "Shelf B" } });
    // Sanity: every fixture above must be distinguishable from the others by
    // exactly one field, so a query matching more than intended is a real bug.

    const all = [byNumber, byColor, byManufacturer, byStorage, noMatch];

    expect(applySpoolFilter(all, [], filter({ lifecycle: "all", query: "#12" })).map((s) => s.id)).toEqual([byNumber.id]);
    expect(applySpoolFilter(all, [], filter({ lifecycle: "all", query: "galaxy" })).map((s) => s.id)).toEqual([byColor.id]);
    expect(applySpoolFilter(all, [], filter({ lifecycle: "all", query: "polymaker" })).map((s) => s.id)).toEqual([byManufacturer.id]);
    expect(applySpoolFilter(all, [], filter({ lifecycle: "all", query: "dry box" })).map((s) => s.id)).toEqual([byStorage.id]);
  });

  it("defaults to the active lifecycle", () => {
    const active = spool({ lifecycle: "active" });
    const empty = spool({ lifecycle: "empty" });
    const archived = spool({ lifecycle: "archived" });

    const result = applySpoolFilter([active, empty, archived], [], filter());

    expect(result.map((s) => s.id)).toEqual([active.id]);
  });

  it("shows every lifecycle when the lifecycle filter is 'all'", () => {
    const active = spool({ lifecycle: "active" });
    const archived = spool({ lifecycle: "archived" });

    const result = applySpoolFilter([active, archived], [], filter({ lifecycle: "all" }));

    expect(result.map((s) => s.id).sort()).toEqual([active.id, archived.id].sort());
  });
});

describe("isFiltered", () => {
  it("is false for the default filter", () => {
    expect(isFiltered(DEFAULT_SPOOL_FILTER)).toBe(false);
    expect(isFiltered(filter())).toBe(false);
  });

  it("is true once any facet, material, printer, query, or non-default lifecycle is set", () => {
    expect(isFiltered(filter({ lifecycle: "all" }))).toBe(true);
    expect(isFiltered(filter({ facets: new Set(["low"]) }))).toBe(true);
    expect(isFiltered(filter({ materials: new Set(["PLA"]) }))).toBe(true);
    expect(isFiltered(filter({ printerId: "prn-1" }))).toBe(true);
    expect(isFiltered(filter({ query: "black" }))).toBe(true);
  });

  it("treats a whitespace-only query as unfiltered", () => {
    expect(isFiltered(filter({ query: "   " }))).toBe(false);
  });
});
