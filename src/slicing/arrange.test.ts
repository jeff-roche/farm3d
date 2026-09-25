import { describe, expect, it } from "vitest";
import { arrange, largestFreeRectangle, type ArrangeItem } from "./arrange";
import { checkPlacement, footprintOf, type Footprint } from "./bounds";
import type { BuildVolume } from "./viewport/renderer";

function box(width: number, depth: number, height = 5): Footprint {
  return footprintOf({ translateMm: [0, 0], rotateDeg: [0, 0, 0], scale: [1, 1, 1] }, Float32Array.from([
    0, 0, 0, width, 0, 0, width, depth, 0, 0, depth, 0,
    0, 0, height, width, 0, height, width, depth, height, 0, depth, height,
  ]))!;
}

const bed: BuildVolume = {
  bed: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
  heightMm: 256,
  excludeAreas: [],
};

function items(sizes: [string, number, number][]): (ArrangeItem & { footprint: Footprint })[] {
  return sizes.map(([key, width, depth]) => {
    const footprint = box(width, depth);
    return { key, footprint, min: footprint.min, max: footprint.max };
  });
}

const PARTS: [string, number, number][] = [
  ["a", 60, 40], ["b", 30, 30], ["c", 80, 20], ["d", 30, 30], ["e", 10, 70], ["f", 45, 45], ["g", 20, 20],
];

function expectPacked(result: ReturnType<typeof arrange>, parts: ReturnType<typeof items>, volume: BuildVolume, spacing: number) {
  const placed = parts.filter((part) => result.placed.has(part.key));
  for (const part of placed) {
    const check = checkPlacement(part.footprint, result.placed.get(part.key)!, volume);
    expect(check.outOfBounds, `${part.key} inside the bed`).toBe(false);
    expect(check.inExcludeArea, `${part.key} clear of exclude areas`).toBe(false);
  }
  // No two placed parts are closer than the spacing.
  for (let i = 0; i < placed.length; i += 1) {
    for (let j = i + 1; j < placed.length; j += 1) {
      const [ax, ay] = result.placed.get(placed[i].key)!;
      const [bx, by] = result.placed.get(placed[j].key)!;
      const a = placed[i].footprint;
      const b = placed[j].footprint;
      const gapX = Math.max(a.min[0] + ax - (b.max[0] + bx), b.min[0] + bx - (a.max[0] + ax));
      const gapY = Math.max(a.min[1] + ay - (b.max[1] + by), b.min[1] + by - (a.max[1] + ay));
      expect(Math.max(gapX, gapY), `${placed[i].key}–${placed[j].key}`).toBeGreaterThanOrEqual(spacing - 1e-9);
    }
  }
}

describe("arrange", () => {
  it("places every part inside the bed with the spacing between them", () => {
    const parts = items(PARTS);
    const result = arrange(parts, bed, 5);
    expect(result.unplaced).toEqual([]);
    expect([...result.placed.keys()].sort()).toEqual(PARTS.map(([key]) => key).sort());
    expectPacked(result, parts, bed, 5);
  });

  it("is deterministic, whatever order the parts come in", () => {
    const first = arrange(items(PARTS), bed, 5);
    const again = arrange(items(PARTS), bed, 5);
    const shuffled = arrange(items([...PARTS].reverse()), bed, 5);
    const asObject = (result: typeof first) => Object.fromEntries([...result.placed.entries()].sort());
    expect(asObject(again)).toEqual(asObject(first));
    expect(asObject(shuffled)).toEqual(asObject(first));
  });

  it("orders by area, then key, so equal parts land in key order", () => {
    const result = arrange(items([["z", 30, 30], ["y", 30, 30]]), bed, 5);
    const [yx] = result.placed.get("y")!;
    const [zx] = result.placed.get("z")!;
    expect(yx).toBeLessThan(zx);
  });

  it("uses the translation that puts each part's own footprint in place", () => {
    // A footprint that doesn't start at its origin (e.g. a rotated part).
    const result = arrange([{ key: "p", min: [-20, -10], max: [0, 0] }], bed, 5);
    const [x, y] = result.placed.get("p")!;
    // Centred on the bed.
    expect(x - 10).toBeCloseTo(128, 9);
    expect(y - 5).toBeCloseTo(128, 9);
  });

  it("keeps clear of exclude areas", () => {
    const volume: BuildVolume = {
      ...bed,
      excludeAreas: [[{ xMm: 0, yMm: 0 }, { xMm: 100, yMm: 0 }, { xMm: 100, yMm: 256 }, { xMm: 0, yMm: 256 }]],
    };
    const parts = items(PARTS);
    const result = arrange(parts, volume, 5);
    expect(result.unplaced).toEqual([]);
    expectPacked(result, parts, volume, 5);
  });

  it("stays inside a round bed", () => {
    const round: BuildVolume = {
      bed: {
        kind: "polygon",
        points: Array.from({ length: 72 }, (_, i) => ({
          xMm: 110 * Math.cos((2 * Math.PI * i) / 72),
          yMm: 110 * Math.sin((2 * Math.PI * i) / 72),
        })),
      },
      heightMm: 200,
      excludeAreas: [],
    };
    const parts = items(PARTS);
    const result = arrange(parts, round, 5);
    expectPacked(result, parts, round, 5);
    expect(result.placed.size).toBeGreaterThan(0);
  });

  it("says what doesn't fit rather than overlapping it", () => {
    const parts = items([["big", 200, 200], ["huge", 300, 10], ["small", 20, 20], ["wide", 250, 100]]);
    const result = arrange(parts, bed, 5);
    expect(result.unplaced).toEqual(["huge", "wide"]);
    expect(result.placed.has("huge")).toBe(false);
    expectPacked(result, parts, bed, 5);
  });

  it("uses the spacing it is given", () => {
    const parts = items(PARTS);
    expectPacked(arrange(parts, bed, 12), parts, bed, 12);
  });
});

describe("largest free rectangle", () => {
  it("is the whole of a rectangular bed with nothing excluded", () => {
    expect(largestFreeRectangle(bed)).toEqual({ minX: 0, minY: 0, maxX: 256, maxY: 256 });
  });

  it("leaves out an exclude area", () => {
    const volume: BuildVolume = {
      ...bed,
      excludeAreas: [[{ xMm: 0, yMm: 0 }, { xMm: 40, yMm: 0 }, { xMm: 40, yMm: 30 }, { xMm: 0, yMm: 30 }]],
    };
    const rect = largestFreeRectangle(volume)!;
    const area = (rect.maxX - rect.minX) * (rect.maxY - rect.minY);
    expect(area).toBeCloseTo(Math.max(256 * 226, 216 * 256), 6);
  });

  it("is null for a bed with no area", () => {
    expect(largestFreeRectangle({ ...bed, bed: { kind: "polygon", points: [] } })).toBeNull();
  });
});
