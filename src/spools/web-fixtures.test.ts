import { describe, expect, it } from "vitest";
import { buildWebInventoryFixture } from "./web-fixtures";
import { WEB_FIXTURE_EQUIPPED_PRINTER_ID, WEB_FIXTURE_EQUIPPED_SLOT_ID } from "../printers/printer-store";

describe("buildWebInventoryFixture", () => {
  const fixture = buildWebInventoryFixture();

  it("has 8 Spools with unique ids and sequential, unique spool numbers", () => {
    expect(fixture.spools).toHaveLength(8);
    expect(new Set(fixture.spools.map((s) => s.id)).size).toBe(8);
    expect(fixture.spools.map((s) => s.spoolNumber).sort((a, b) => a - b)).toEqual([1, 2, 3, 4, 5, 6, 7, 8]);
  });

  it("has 2 tares with unique ids and case-insensitively unique names", () => {
    expect(fixture.tares).toHaveLength(2);
    expect(new Set(fixture.tares.map((t) => t.id)).size).toBe(2);
    const names = fixture.tares.map((t) => t.name.toLowerCase());
    expect(new Set(names).size).toBe(2);
  });

  it("has exactly one active, loaded, measured Spool, loaded on the equipped Printer's slot", () => {
    const loaded = fixture.spools.filter((s) => s.facets.loaded);
    expect(loaded).toHaveLength(1);
    const [spool] = loaded;
    expect(spool.lifecycle).toBe("active");
    expect(spool.facets.confidence).toBe("measured");
    expect(spool.location).toEqual({
      kind: "slot",
      slotId: WEB_FIXTURE_EQUIPPED_SLOT_ID,
      printerId: WEB_FIXTURE_EQUIPPED_PRINTER_ID,
    });
  });

  it("has exactly one active Spool in storage that is both estimated and low", () => {
    const matches = fixture.spools.filter(
      (s) => s.lifecycle === "active" && s.facets.confidence === "estimated" && s.facets.low,
    );
    expect(matches).toHaveLength(1);
    const [spool] = matches;
    expect(spool.location.kind).toBe("storage");
    expect(spool.availability.currentMg).toBeLessThanOrEqual(spool.lowThresholdMg);
  });

  it("has exactly one reserved Spool, with availability accounting for the reservation", () => {
    const reserved = fixture.spools.filter((s) => s.facets.reserved);
    expect(reserved).toHaveLength(1);
    const [spool] = reserved;
    expect(spool.availability.reservedMg).toBeGreaterThan(0);
    expect(spool.availability.availableMg).toBe(spool.availability.currentMg - spool.availability.reservedMg);
  });

  it("has exactly one empty Spool and one archived Spool, both in storage", () => {
    const empty = fixture.spools.filter((s) => s.lifecycle === "empty");
    const archived = fixture.spools.filter((s) => s.lifecycle === "archived");
    expect(empty).toHaveLength(1);
    expect(archived).toHaveLength(1);
    expect(empty[0].location.kind).toBe("storage");
    expect(archived[0].location.kind).toBe("storage");
  });

  it("has three more active Spools beyond the loaded, low, and reserved ones", () => {
    const plainActive = fixture.spools.filter(
      (s) => s.lifecycle === "active" && !s.facets.loaded && !s.facets.reserved && !s.facets.low,
    );
    expect(plainActive).toHaveLength(3);
  });

  it("gives every OTHER-family Spool a materialOther, and no other family one", () => {
    for (const spool of fixture.spools) {
      if (spool.materialFamily === "OTHER") expect(spool.materialOther).toBeTruthy();
      else expect(spool.materialOther).toBeUndefined();
    }
  });

  it("returns a fresh fixture (independent object identity) on each call", () => {
    const again = buildWebInventoryFixture();
    expect(again).toEqual(fixture);
    expect(again.spools).not.toBe(fixture.spools);
    expect(again.spools[0]).not.toBe(fixture.spools[0]);
  });
});
