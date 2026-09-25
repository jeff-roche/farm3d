import { describe, expect, it } from "vitest";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import { decodeMeshBuffer } from "./mesh-buffer";
import type { Fact } from "./types";
import { buildWebSlicingFixture } from "./web-fixtures";

const NOW = new Date("2026-09-24T12:00:00Z");

describe("buildWebSlicingFixture", () => {
  const fixture = buildWebSlicingFixture(NOW);
  const library = buildWebLibraryFixture(NOW);

  it("has an available OrcaSlicer 2.4.2 runtime that can slice", () => {
    expect(fixture.runtime.engine).toMatchObject({ state: "available", version: "2.4.2" });
    expect(fixture.runtime.presetSource).toMatchObject({ state: "available", version: "2.4.2" });
    expect(fixture.runtime.canSlice).toBe(true);
  });

  it("has a two-plate Preparation for mdl-web-enclosure on its current revision", () => {
    expect(fixture.preparations).toHaveLength(1);
    const [preparation] = fixture.preparations;
    const model = library.models.find((m) => m.id === "mdl-web-enclosure")!;
    expect(preparation.modelId).toBe(model.id);
    expect(preparation.sourceRevisionId).toBe(model.currentRevision.id);
    expect(preparation.stale).toBe(false);
    expect(preparation.document.plates).toHaveLength(2);
    const inspection = library.revisions[model.id][0].inspection;
    expect(inspection.format === "3mf" && inspection.plates).toHaveLength(2);
  });

  it("has one succeeded and one failed operation, each with a log", () => {
    const states = fixture.operations.map((o) => o.state).sort();
    expect(states).toEqual(["failed", "succeeded"]);
    const succeeded = fixture.operations.find((o) => o.state === "succeeded")!;
    const failed = fixture.operations.find((o) => o.state === "failed")!;
    expect(fixture.revisionRecords[succeeded.sliceRevisionId!]).toBeDefined();
    expect(failed.failure?.code.kind).toBe("objectsOutsidePlate");
    expect(failed.sliceRevisionId).toBeUndefined();
    for (const operation of fixture.operations) {
      expect(fixture.logs[operation.id].text.length, operation.id).toBeGreaterThan(0);
      expect(operation.preparationId).toBe(fixture.preparations[0].id);
    }
  });

  it("has a farm3d revision built only from farm3d input", () => {
    const farm3d = fixture.revisions.filter((r) => r.kind === "farm3d");
    expect(farm3d).toHaveLength(1);
    const facts = farm3d[0].facts;
    const all: Fact<unknown>[] = [facts.printerProfile, facts.nozzleDiameterMm, facts.materialFamily, facts.filamentDiameterMm];
    expect(all.every((f) => f.provenance === "farm3dInput")).toBe(true);
    expect(farm3d[0].requiresManualPrinterSelection).toBe(false);
    expect(farm3d[0].runtime?.engineVersion).toBe("2.4.2");
  });

  it("has an external revision on mdl-web-cube-gcode with materialFamily absent and never farm3dInput", () => {
    const external = fixture.revisions.filter((r) => r.kind === "external");
    expect(external).toHaveLength(1);
    const [revision] = external;
    expect(revision.modelId).toBe("mdl-web-cube-gcode");
    expect(revision.sourceRevisionId).toBe(library.models.find((m) => m.id === "mdl-web-cube-gcode")!.currentRevision.id);
    expect(revision.facts.materialFamily).toEqual({ provenance: "absent", value: null });
    const all: Fact<unknown>[] = [revision.facts.printerProfile, revision.facts.nozzleDiameterMm, revision.facts.materialFamily, revision.facts.filamentDiameterMm];
    expect(all.some((f) => f.provenance === "farm3dInput")).toBe(false);
    expect(revision.requiresManualPrinterSelection).toBe(true);
    expect(revision.estimates).toBeNull();
    expect(revision.plate).toBeUndefined();
    expect(revision.runtime).toBeUndefined();
    const record = fixture.revisionRecords[revision.id];
    expect(record.claimedEstimates?.trusted).toBe(false);
    expect(record.blobs).toEqual([]);
  });

  it("lists every revision newest first, each summary matching its record", () => {
    const times = fixture.revisions.map((r) => r.createdAt);
    expect(times).toEqual([...times].sort().reverse());
    for (const summary of fixture.revisions) {
      expect(fixture.revisionRecords[summary.id]).toMatchObject(summary);
    }
  });

  it("has geometry and decodable meshes for every non-G-code fixture revision, with the Library's triangle counts", () => {
    for (const model of library.models.filter((m) => m.format !== "gcode")) {
      for (const revision of library.revisions[model.id]) {
        const geometry = fixture.geometry[revision.id];
        expect(geometry, revision.id).toBeDefined();
        const total = geometry.objects.reduce((sum, o) => sum + o.triangleCount, 0);
        expect(revision.summary.format !== "gcode" && revision.summary.triangleCount, revision.id).toBe(total);
        for (const object of geometry.objects) {
          const mesh = decodeMeshBuffer(fixture.meshes[revision.id][object.objectKey]);
          expect(mesh.triangleCount).toBe(object.triangleCount);
          expect(object.layFlatFaces.length).toBeGreaterThan(0);
          expect(object.layFlatFaces.length).toBeLessThanOrEqual(8);
          const areas = object.layFlatFaces.map((f) => f.areaMm2);
          expect(areas).toEqual([...areas].sort((a, b) => b - a));
        }
        for (const item of geometry.buildItems) {
          expect(item.transform).toHaveLength(12);
          expect(geometry.objects.some((o) => o.objectKey === item.objectKey)).toBe(true);
        }
      }
    }
  });

  it("gives the enclosure one unprintable build item and a plate for each item", () => {
    const geometry = fixture.geometry["msr-web-enclosure-1"];
    expect(geometry.buildItems.filter((item) => !item.printable)).toHaveLength(1);
    expect(geometry.buildItems.map((item) => item.plateIndex)).toEqual([1, 2]);
  });

  it("puts a box's lay-flat faces at its six sides, the largest first", () => {
    const [bracket] = fixture.geometry["msr-web-bracket-1"].objects;
    expect(bracket.boundsMm).toEqual({ min: [0, 0, 0], max: [40, 40, 5] });
    expect(bracket.layFlatFaces).toHaveLength(6);
    expect(bracket.layFlatFaces[0].areaMm2).toBeCloseTo(1_600);
    expect(bracket.layFlatFaces.slice(0, 2).map((f) => Math.abs(f.normal[2]))).toEqual([1, 1]);
  });

  it("offers presets whose defaults are among them", () => {
    const { sliceOptions } = fixture;
    expect(sliceOptions.processPresets.map((p) => p.name)).toContain(sliceOptions.defaults.processPreset);
    expect(sliceOptions.filamentPresets.map((p) => p.name)).toContain(sliceOptions.defaults.filamentPreset);
  });

  it("is deterministic for a given time and returns fresh objects each call", () => {
    const again = buildWebSlicingFixture(NOW);
    expect(again).toEqual(fixture);
    expect(again.preparations).not.toBe(fixture.preparations);
    expect(again.meshes["msr-web-bracket-1"][1]).not.toBe(fixture.meshes["msr-web-bracket-1"][1]);
  });
});
