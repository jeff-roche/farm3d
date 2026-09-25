import { describe, expect, it } from "vitest";
import { sourceLayout } from "./source-plates";
import type { RevisionGeometry } from "./types";
import { buildWebSlicingFixture } from "./web-fixtures";
import { decodeMeshBuffer } from "./mesh-buffer";
import { describePlate, type ViewportInstance } from "./viewport/plate-view";

const fixture = buildWebSlicingFixture(new Date("2026-09-25T12:00:00Z"));
const positionsOf = (revisionId: string) => (objectKey: number) => {
  const buffer = fixture.meshes[revisionId]?.[objectKey];
  return buffer ? decodeMeshBuffer(buffer.slice(0)).positions : undefined;
};

const translate = (instance: ViewportInstance) => instance.transform.translateMm;

describe("source layout", () => {
  it("puts the enclosure's items on its two plates, centred, and lists the unprintable one", () => {
    const layout = sourceLayout(
      fixture.geometry["msr-web-enclosure-1"],
      [{ index: 1, name: "Lid" }, { index: 2, name: "Latch" }],
      positionsOf("msr-web-enclosure-1"),
    );
    expect(layout.plates.map((plate) => plate.name)).toEqual(["Lid", "Latch"]);
    expect(layout.plates.map((plate) => plate.instances.map((instance) => instance.name))).toEqual([["Lid"], ["Latch"]]);
    // A 120 × 80 lid and a 20 × 10 latch, each centred on the origin.
    expect(translate(layout.plates[0].instances[0])).toEqual([-60, -40]);
    expect(translate(layout.plates[1].instances[0])).toEqual([-10, -5]);
    expect(layout.unprintable).toEqual(["Gasket"]);
  });

  it("keeps the relative layout of items that share a plate", () => {
    const geometry: RevisionGeometry = {
      objects: [
        { objectKey: 1, triangleCount: 12, boundsMm: { min: [0, 0, 0], max: [40, 40, 5] }, layFlatFaces: [] },
      ],
      buildItems: [
        { objectKey: 1, transform: [1, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0], printable: true },
        { objectKey: 1, transform: [1, 0, 0, 0, 1, 0, 0, 0, 1, 100, 0, 0], printable: true },
      ],
    };
    const layout = sourceLayout(geometry, [], positionsOf("msr-web-bracket-1"));
    expect(layout.plates).toHaveLength(1);
    expect(layout.plates[0].name).toBe("1");
    // The pair spans X 0..140; centred, it spans −70..70.
    expect(layout.plates[0].instances.map(translate)).toEqual([[-70, -20], [30, -20]]);
    expect(layout.plates[0].instances.map((instance) => instance.name)).toEqual(["Object 1", "Object 1"]);
  });

  it("gives an STL one plate with its one object", () => {
    const layout = sourceLayout(fixture.geometry["msr-web-bracket-1"], [], positionsOf("msr-web-bracket-1"));
    expect(layout.plates).toHaveLength(1);
    expect(layout.plates[0].instances).toHaveLength(1);
    expect(layout.unprintable).toEqual([]);
  });
});

describe("plate description", () => {
  const lid: ViewportInstance = {
    instanceKey: "a", objectKey: 1, name: "Lid",
    transform: { translateMm: [12.345, -4], rotateDeg: [0, 0, 15], scale: [1, 1, 1.5] },
  };
  const latch: ViewportInstance = {
    instanceKey: "b", objectKey: 2, name: "Latch", outOfBounds: true,
    transform: { translateMm: [0, 0], rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
  };

  it("names the plate, counts the objects, and says nothing is selected", () => {
    expect(describePlate({ plateName: "Lid", instances: [lid] })).toBe("Plate Lid: 1 object. No object selected.");
  });

  it("describes the selected object's position, rotation, and scale", () => {
    expect(describePlate({ plateName: "1", instances: [lid, latch], selectedInstanceKey: "a" })).toBe(
      "Plate 1: 2 objects. Selected: Lid, at X 12.3 mm, Y -4.0 mm; rotated 0°, 0°, 15° about X, Y, Z;"
        + " scaled 100%, 100%, 150%. Outside the printable area: Latch.",
    );
  });

  it("appends the caller's notes", () => {
    expect(describePlate({ plateName: "2", instances: [], notes: ["Not placed: Gasket."] })).toBe(
      "Plate 2: 0 objects. Not placed: Gasket.",
    );
  });
});
