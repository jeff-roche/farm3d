import { describe, expect, it } from "vitest";
import { footprintOf } from "./bounds";
import { issuesForPlates, validatePreparation, type ValidationInput } from "./validation";
import { buildWebSlicingFixture } from "./web-fixtures";
import type { InstanceDoc, PreparationDocument } from "./types";
import type { BuildVolume } from "./viewport/renderer";

function box(width: number, depth: number, height: number): Float32Array {
  return Float32Array.from([
    0, 0, 0, width, 0, 0, width, depth, 0, 0, depth, 0,
    0, 0, height, width, 0, height, width, depth, height, 0, depth, height,
  ]);
}

const meshes: Record<number, Float32Array> = { 1: box(20, 20, 10), 2: box(10, 10, 300) };

const volume: BuildVolume = {
  bed: { kind: "rectangular", widthMm: 200, depthMm: 200, originXMm: 0, originYMm: 0 },
  heightMm: 250,
  excludeAreas: [[{ xMm: 0, yMm: 0 }, { xMm: 30, yMm: 0 }, { xMm: 30, yMm: 30 }, { xMm: 0, yMm: 30 }]],
};

const instance = (instanceKey: string, objectKey: number, x: number, y: number): InstanceDoc => ({
  instanceKey, objectKey, transform: { translateMm: [x, y], rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
});

function input(document: Partial<PreparationDocument>, overrides: Partial<ValidationInput> = {}): ValidationInput {
  const fixture = buildWebSlicingFixture();
  return {
    document: {
      plates: [],
      target: { kind: "profile", catalogRef: fixture.sliceOptions.profileSnapshot.catalogRef },
      processPreset: fixture.sliceOptions.defaults.processPreset!,
      filamentPreset: fixture.sliceOptions.defaults.filamentPreset!,
      controls: {},
      ...document,
    },
    footprint: (doc) => {
      const positions = meshes[doc.objectKey];
      return positions ? footprintOf(doc.transform, positions) ?? undefined : undefined;
    },
    volume,
    options: fixture.sliceOptions,
    runtime: fixture.runtime,
    stale: false,
    ...overrides,
  };
}

describe("validatePreparation", () => {
  it("finds nothing wrong with a good Preparation", () => {
    const result = validatePreparation(input({ plates: [{ plateKey: "p1", instances: [instance("i1", 1, 100, 100)] }] }));
    expect(result.issues).toEqual([]);
    expect(result.placement.get("i1")).toEqual({ outOfBounds: false, inExcludeArea: false, tooTall: false });
  });

  it("reports empty plates and each placement problem by plate and instance", () => {
    const result = validatePreparation(input({
      plates: [
        { plateKey: "p1", instances: [instance("off", 1, 190, 100), instance("excluded", 1, 10, 10)] },
        { plateKey: "p2", instances: [] },
        { plateKey: "p3", instances: [instance("tall", 2, 100, 100)] },
      ],
    }));
    expect(result.issues).toEqual([
      { code: "outOfBounds", plateKey: "p1", instanceKey: "off" },
      { code: "inExcludeArea", plateKey: "p1", instanceKey: "excluded" },
      { code: "emptyPlate", plateKey: "p2" },
      { code: "tooTall", plateKey: "p3", instanceKey: "tall" },
    ]);
    expect(result.placement.get("off")?.outOfBounds).toBe(true);
  });

  it("skips placement checks until the target volume and the meshes are known", () => {
    const plates = [{ plateKey: "p1", instances: [instance("off", 1, 190, 100), instance("unknown", 9, 0, 0)] }];
    expect(validatePreparation(input({ plates }, { volume: null })).issues).toEqual([]);
    expect(validatePreparation(input({ plates })).issues).toEqual([{ code: "outOfBounds", plateKey: "p1", instanceKey: "off" }]);
  });

  it("reports the runtime, a stale source, and presets the runtime doesn't offer, before plate issues", () => {
    const fixture = buildWebSlicingFixture();
    const result = validatePreparation(input(
      {
        plates: [{ plateKey: "p1", instances: [] }],
        processPreset: "0.10mm Missing",
        filamentPreset: "Some ABS",
      },
      { runtime: { ...fixture.runtime, canSlice: false }, stale: true },
    ));
    expect(result.issues).toEqual([
      { code: "runtimeUnavailable" },
      { code: "stale" },
      { code: "presetNotFound", preset: "process", name: "0.10mm Missing" },
      { code: "filamentIncompatible", name: "Some ABS" },
      { code: "emptyPlate", plateKey: "p1" },
    ]);
  });

  it("reports unchosen presets, and waits for the options before checking them", () => {
    const plates = [{ plateKey: "p1", instances: [instance("i1", 1, 100, 100)] }];
    const unset = input({ plates, processPreset: undefined, filamentPreset: undefined });
    expect(validatePreparation(unset).issues).toEqual([
      { code: "presetNotFound", preset: "process", name: null },
      { code: "presetNotFound", preset: "filament", name: null },
    ]);
    expect(validatePreparation({ ...unset, options: null }).issues).toEqual([]);
  });

  it("treats a missing runtime status as unknown, not unavailable", () => {
    const plates = [{ plateKey: "p1", instances: [instance("i1", 1, 100, 100)] }];
    expect(validatePreparation(input({ plates }, { runtime: null })).issues).toEqual([]);
  });
});

describe("issuesForPlates", () => {
  it("keeps the whole-Preparation issues and those on the given plates", () => {
    const issues = validatePreparation(input(
      { plates: [{ plateKey: "p1", instances: [] }, { plateKey: "p2", instances: [] }] },
      { stale: true },
    )).issues;
    expect(issuesForPlates(issues, ["p2"])).toEqual([{ code: "stale" }, { code: "emptyPlate", plateKey: "p2" }]);
  });
});
