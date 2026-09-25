import { describe, expect, it } from "vitest";
import {
  bedLabel,
  controlRows,
  estimateRows,
  factValueText,
  filamentSummary,
  formatFilamentLength,
  formatPrintTime,
  revisionTitle,
  runtimeLine,
  runtimeVersionsDiffer,
} from "./revision-presentation";
import { buildWebSlicingFixture, WEB_SLICING_REVISION_EXTERNAL, WEB_SLICING_REVISION_FARM3D } from "./web-fixtures";

const fixture = buildWebSlicingFixture(new Date("2026-09-24T12:00:00Z"));
const farm3d = fixture.revisionRecords[WEB_SLICING_REVISION_FARM3D];
const external = fixture.revisionRecords[WEB_SLICING_REVISION_EXTERNAL];

describe("revision presentation", () => {
  it("names a revision by its plate, or as external G-code", () => {
    expect(revisionTitle(farm3d)).toBe("Plate 1: Lid");
    expect(revisionTitle({ kind: "farm3d", plate: { plateKey: "k", plateIndex: 2 } })).toBe("Plate 2");
    expect(revisionTitle(external)).toBe("External G-code");
  });

  it("formats times and filament", () => {
    expect(formatPrintTime(5_412)).toBe("1 h 30 min");
    expect(formatPrintTime(7_200)).toBe("2 h");
    expect(formatPrintTime(1_421)).toBe("23 min 41 s");
    expect(formatPrintTime(120)).toBe("2 min");
    expect(formatPrintTime(41)).toBe("41 s");
    expect(formatFilamentLength(12_941)).toBe("12.94 m");
    expect(formatFilamentLength(412.4)).toBe("412 mm");
    expect(filamentSummary(farm3d.estimates)).toBe("38.6 g");
    expect(filamentSummary(null)).toBeUndefined();
  });

  it("lists only the estimates that are known", () => {
    expect(estimateRows(farm3d.estimates!)).toEqual([
      { label: "Print time", value: "1 h 30 min" },
      { label: "Filament length", value: "12.94 m" },
      { label: "Filament weight", value: "38.6 g" },
      { label: "Layers", value: "20" },
      { label: "Height", value: "4 mm" },
    ]);
    expect(estimateRows(external.claimedEstimates!)).toEqual([
      { label: "Print time", value: "23 min 41 s" },
      { label: "Height", value: "20 mm" },
    ]);
  });

  it("gives a fact's value, or nothing when it is absent", () => {
    expect(factValueText(farm3d.facts, "printerProfile")).toBe("Elegoo Centauri Carbon 0.4 nozzle");
    expect(factValueText(farm3d.facts, "nozzleDiameterMm")).toBe("0.4 mm");
    expect(factValueText(farm3d.facts, "materialFamily")).toBe("PLA");
    expect(factValueText(external.facts, "materialFamily")).toBeUndefined();
    expect(factValueText({ ...external.facts, materialFamily: { provenance: "operatorConfirmed", value: "OTHER" }, materialOther: "Wood PLA" }, "materialFamily"))
      .toBe("Wood PLA");
  });

  it("describes the bed, the controls and the runtime", () => {
    expect(bedLabel({ kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 })).toBe("256 × 256 mm");
    expect(bedLabel({ kind: "polygon", points: [{ xMm: 0, yMm: 0 }, { xMm: 1, yMm: 0 }, { xMm: 1, yMm: 1 }] })).toBe("Custom shape, 3 corners");
    expect(controlRows(farm3d.target!.controls)).toEqual([
      { label: "Layer height", value: "0.2 mm" },
      { label: "Infill density", value: "15%" },
      { label: "Supports", value: "Off" },
    ]);
    const runtime = { engineVersion: "2.5.0-dev", engineChannel: "prerelease", presetSourceVersion: "2.4.2", presetSourceChannel: "release" } as const;
    expect(runtimeLine(runtime)).toBe("Engine 2.5.0-dev (prerelease) · presets 2.4.2");
    expect(runtimeVersionsDiffer(runtime)).toBe(true);
    expect(runtimeVersionsDiffer(farm3d.runtime!)).toBe(false);
  });
});
