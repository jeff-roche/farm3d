import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { Tare } from "../generated/contracts/domain/Tare";
import { WEB_FIXTURE_EQUIPPED_PRINTER_ID, WEB_FIXTURE_EQUIPPED_SLOT_ID } from "../printers/printer-store";

/** `just web`'s Spool inventory seed data -- no Rust backend, so this
 *  stands in for the Farm's persisted Spools/tares (P3 design's Frontend
 *  architecture: "Web fallback"). Every facet below is written as a literal
 *  (never derived from other fields here), the same way a real
 *  `SpoolRecord` would arrive already computed by Rust -- this module's job
 *  is to look like a snapshot of that, not to compute one. */
export interface WebInventoryFixture {
  spools: SpoolRecord[];
  tares: Tare[];
}

const TARE_CARDBOARD = "tar-web-cardboard";
const TARE_PLASTIC = "tar-web-plastic";

function buildWebTares(): Tare[] {
  return [
    {
      id: TARE_CARDBOARD, revision: 1, name: "Cardboard spool (Polymaker-style)",
      weightMg: 205_000, createdAt: "2026-01-05T00:00:00Z", updatedAt: "2026-01-05T00:00:00Z",
    },
    {
      id: TARE_PLASTIC, revision: 1, name: "Plastic spool (eSun-style)",
      weightMg: 180_000, createdAt: "2026-01-05T00:00:00Z", updatedAt: "2026-01-05T00:00:00Z",
    },
  ];
}

function buildWebSpools(): SpoolRecord[] {
  return [
    // 1. Active, loaded onto the equipped Printer's first slot, measured.
    {
      id: "spl-web-1", revision: 1, spoolNumber: 1,
      manufacturer: "Prusament", product: "PLA", materialFamily: "PLA",
      colorName: "Galaxy Black", colorHex: "#0B0B0F", diameter: "1.75",
      nominalMg: 1_000_000, lowThresholdMg: 100_000, tareId: TARE_CARDBOARD,
      lifecycle: "active",
      location: { kind: "slot", slotId: WEB_FIXTURE_EQUIPPED_SLOT_ID, printerId: WEB_FIXTURE_EQUIPPED_PRINTER_ID },
      availability: { currentMg: 812_000, reservedMg: 0, availableMg: 812_000 },
      facets: { loaded: true, reserved: false, low: false, confidence: "measured" },
      lastMeasuredAt: "2026-09-10T09:00:00Z",
      createdAt: "2026-08-01T00:00:00Z", updatedAt: "2026-09-10T09:00:00Z",
    },
    // 2. Active, in storage, estimated and low.
    {
      id: "spl-web-2", revision: 1, spoolNumber: 2,
      manufacturer: "Overture", product: "PETG", materialFamily: "PETG",
      colorName: "Clear", diameter: "1.75",
      nominalMg: 1_000_000, lowThresholdMg: 100_000, tareId: TARE_PLASTIC,
      lifecycle: "active",
      location: { kind: "storage", storageLabel: "Shelf A2" },
      availability: { currentMg: 80_000, reservedMg: 0, availableMg: 80_000 },
      facets: { loaded: false, reserved: false, low: true, confidence: "estimated" },
      createdAt: "2026-07-15T00:00:00Z", updatedAt: "2026-09-05T00:00:00Z",
    },
    // 3. Seeded reserved (D8's debug fixture aid, done directly here rather
    // than through `debugSeedReservation`).
    {
      id: "spl-web-3", revision: 1, spoolNumber: 3,
      manufacturer: "Polymaker", product: "PolyLite PLA-CF", materialFamily: "PLA-CF",
      colorName: "Charcoal", diameter: "1.75",
      nominalMg: 750_000, lowThresholdMg: 100_000,
      lifecycle: "active",
      location: { kind: "storage", storageLabel: "Shelf A1" },
      availability: { currentMg: 500_000, reservedMg: 200_000, availableMg: 300_000 },
      facets: { loaded: false, reserved: true, low: false, confidence: "measured" },
      createdAt: "2026-07-20T00:00:00Z", updatedAt: "2026-09-12T00:00:00Z",
    },
    // 4. Empty.
    {
      id: "spl-web-4", revision: 1, spoolNumber: 4,
      manufacturer: "eSun", product: "ABS", materialFamily: "ABS",
      colorName: "White", diameter: "1.75",
      nominalMg: 1_000_000, lowThresholdMg: 100_000,
      lifecycle: "empty",
      location: { kind: "storage", storageLabel: "Shelf B1" },
      availability: { currentMg: 0, reservedMg: 0, availableMg: 0 },
      facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
      lastMeasuredAt: "2026-09-01T00:00:00Z",
      createdAt: "2026-05-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z",
    },
    // 5. Archived.
    {
      id: "spl-web-5", revision: 1, spoolNumber: 5,
      manufacturer: "Prusament", product: "ASA", materialFamily: "ASA",
      colorName: "Orange", diameter: "1.75",
      nominalMg: 1_000_000, lowThresholdMg: 100_000,
      lifecycle: "archived",
      location: { kind: "storage", storageLabel: null },
      availability: { currentMg: 0, reservedMg: 0, availableMg: 0 },
      facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
      createdAt: "2026-01-10T00:00:00Z", updatedAt: "2026-06-01T00:00:00Z",
    },
    // 6-8. Three more active Spools.
    {
      id: "spl-web-6", revision: 1, spoolNumber: 6,
      manufacturer: "Bambu Lab", product: "TPU 95A", materialFamily: "TPU",
      colorName: "Red", diameter: "1.75",
      nominalMg: 500_000, lowThresholdMg: 100_000,
      lifecycle: "active",
      location: { kind: "storage", storageLabel: "Shelf B2" },
      availability: { currentMg: 480_000, reservedMg: 0, availableMg: 480_000 },
      facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
      createdAt: "2026-08-20T00:00:00Z", updatedAt: "2026-08-20T00:00:00Z",
    },
    {
      id: "spl-web-7", revision: 1, spoolNumber: 7,
      manufacturer: "Polymaker", product: "PC-Max", materialFamily: "PC",
      colorName: "Natural", diameter: "1.75",
      nominalMg: 1_000_000, lowThresholdMg: 100_000,
      lifecycle: "active",
      location: { kind: "storage", storageLabel: "Shelf B3" },
      availability: { currentMg: 990_000, reservedMg: 0, availableMg: 990_000 },
      facets: { loaded: false, reserved: false, low: false, confidence: "estimated" },
      createdAt: "2026-09-15T00:00:00Z", updatedAt: "2026-09-15T00:00:00Z",
    },
    {
      id: "spl-web-8", revision: 1, spoolNumber: 8,
      manufacturer: "3DXTech", materialFamily: "OTHER", materialOther: "Wood-fill",
      colorName: "Birch", diameter: "2.85",
      nominalMg: 750_000, lowThresholdMg: 100_000,
      lifecycle: "active",
      location: { kind: "storage", storageLabel: null },
      availability: { currentMg: 750_000, reservedMg: 0, availableMg: 750_000 },
      facets: { loaded: false, reserved: false, low: false, confidence: "estimated" },
      createdAt: "2026-09-18T00:00:00Z", updatedAt: "2026-09-18T00:00:00Z",
    },
  ];
}

/** Builds a fresh fixture -- a new object graph every call, so a caller
 *  (`spool-store.ts`) that hands the arrays to a reactive store never
 *  shares mutable state with a previous load or with this module's own
 *  literals. */
export function buildWebInventoryFixture(): WebInventoryFixture {
  return { spools: buildWebSpools(), tares: buildWebTares() };
}
