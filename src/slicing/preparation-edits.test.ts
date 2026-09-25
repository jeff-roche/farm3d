import { describe, expect, it } from "vitest";
import {
  addPlate,
  deleteInstance,
  deletePlate,
  duplicateInstance,
  findInstance,
  MAX_PLATES,
  moveInstanceToPlate,
  movePlate,
  plateLabel,
  renamePlate,
  setTransform,
} from "./preparation-edits";
import type { InstanceDoc, PreparationDocument } from "./types";

const instance = (instanceKey: string, x = 0): InstanceDoc => ({
  instanceKey, objectKey: 1, transform: { translateMm: [x, 0], rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
});

const doc = (): PreparationDocument => ({
  plates: [
    { plateKey: "p1", name: "Lid", instances: [instance("a"), instance("b", 10)] },
    { plateKey: "p2", instances: [instance("c")] },
  ],
  target: { kind: "printer", printerId: "prn-1" },
  controls: {},
});

describe("plate edits", () => {
  it("labels a plate by its name, else its position", () => {
    const { plates } = doc();
    expect(plateLabel(plates[0], 0)).toBe("Lid");
    expect(plateLabel(plates[1], 1)).toBe("Plate 2");
  });

  it("adds an empty plate at the end, up to 36", () => {
    const next = addPlate(doc(), "p3");
    expect(next.plates.map((p) => p.plateKey)).toEqual(["p1", "p2", "p3"]);
    expect(next.plates[2].instances).toEqual([]);
    let full = doc();
    for (let i = full.plates.length; i < MAX_PLATES; i += 1) full = addPlate(full, `k${i}`);
    expect(full.plates).toHaveLength(MAX_PLATES);
    expect(addPlate(full, "one-too-many")).toBe(full);
  });

  it("renames, trims, and clears a blank name", () => {
    expect(renamePlate(doc(), "p2", "  Knobs ").plates[1].name).toBe("Knobs");
    expect(renamePlate(doc(), "p1", "   ").plates[0]).not.toHaveProperty("name");
    expect(renamePlate(doc(), "p1", "x".repeat(200)).plates[0].name).toHaveLength(128);
    const same = doc();
    expect(renamePlate(same, "p1", "Lid")).toBe(same);
  });

  it("moves a plate left and right, not past either end", () => {
    expect(movePlate(doc(), "p2", -1).plates.map((p) => p.plateKey)).toEqual(["p2", "p1"]);
    expect(movePlate(doc(), "p1", 1).plates.map((p) => p.plateKey)).toEqual(["p2", "p1"]);
    const same = doc();
    expect(movePlate(same, "p1", -1)).toBe(same);
    expect(movePlate(same, "p2", 1)).toBe(same);
  });

  it("deletes a plate with its objects, but never the last one", () => {
    const next = deletePlate(doc(), "p1");
    expect(next.plates.map((p) => p.plateKey)).toEqual(["p2"]);
    expect(deletePlate(next, "p2")).toBe(next);
  });
});

describe("instance edits", () => {
  it("finds an instance and its plate", () => {
    const found = findInstance(doc(), "c")!;
    expect(found.plate.plateKey).toBe("p2");
    expect(found.plateIndex).toBe(1);
    expect(findInstance(doc(), "nope")).toBeUndefined();
  });

  it("sets a transform, and returns the same document when nothing changes", () => {
    const start = doc();
    const moved = setTransform(start, "b", { ...start.plates[0].instances[1].transform, translateMm: [20, 5] });
    expect(moved.plates[0].instances[1].transform.translateMm).toEqual([20, 5]);
    // Untouched plates are shared.
    expect(moved.plates[1]).toBe(start.plates[1]);
    expect(setTransform(start, "b", { ...start.plates[0].instances[1].transform })).toBe(start);
  });

  it("moves an instance to another plate, keeping its transform", () => {
    const next = moveInstanceToPlate(doc(), "a", "p2");
    expect(next.plates[0].instances.map((i) => i.instanceKey)).toEqual(["b"]);
    expect(next.plates[1].instances.map((i) => i.instanceKey)).toEqual(["c", "a"]);
    const same = doc();
    expect(moveInstanceToPlate(same, "a", "p1")).toBe(same);
    expect(moveInstanceToPlate(same, "a", "missing")).toBe(same);
  });

  it("duplicates an instance next to the original, offset", () => {
    const next = duplicateInstance(doc(), "a", "a2", [25, 0]);
    expect(next.plates[0].instances.map((i) => i.instanceKey)).toEqual(["a", "a2", "b"]);
    expect(next.plates[0].instances[1].transform.translateMm).toEqual([25, 0]);
    expect(next.plates[0].instances[1].objectKey).toBe(1);
  });

  it("deletes an instance", () => {
    expect(deleteInstance(doc(), "b").plates[0].instances.map((i) => i.instanceKey)).toEqual(["a"]);
  });
});
