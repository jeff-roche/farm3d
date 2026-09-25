import { describe, expect, it } from "vitest";
import type { PointMm } from "../types";
import { bedExtent, bedOutline, buildVolumeFromProfile } from "./build-volume";
import { clipLine, gridLines, hatchLines, type Segment2 } from "./plate-lines";

const square = (size: number, x = 0, y = 0): PointMm[] => [
  { xMm: x, yMm: y }, { xMm: x + size, yMm: y }, { xMm: x + size, yMm: y + size }, { xMm: x, yMm: y + size },
];

const length = ([[ax, ay], [bx, by]]: Segment2) => Math.hypot(bx - ax, by - ay);

describe("build volume", () => {
  it("draws a rectangular bed from its origin and size", () => {
    const bed = { kind: "rectangular" as const, widthMm: 250, depthMm: 210, originXMm: -5, originYMm: 0 };
    expect(bedOutline(bed)).toEqual([
      { xMm: -5, yMm: 0 }, { xMm: 245, yMm: 0 }, { xMm: 245, yMm: 210 }, { xMm: -5, yMm: 210 },
    ]);
    expect(bedExtent(bed)).toEqual({ min: [-5, 0], max: [245, 210] });
  });

  it("has no extent for a polygon bed of fewer than three points", () => {
    expect(bedExtent({ kind: "polygon", points: [{ xMm: 0, yMm: 0 }, { xMm: 1, yMm: 1 }] })).toBeNull();
  });

  it("takes the profile's height and its one exclude polygon", () => {
    const bedShape = { kind: "polygon" as const, points: square(200) };
    const exclude = square(20);
    expect(buildVolumeFromProfile({ bedShape, printableHeightMm: 180, bedExcludeAreas: exclude })).toEqual({
      bed: bedShape, heightMm: 180, excludeAreas: [exclude],
    });
    expect(buildVolumeFromProfile({ bedShape, printableHeightMm: 180, bedExcludeAreas: [] }).excludeAreas).toEqual([]);
  });
});

describe("plate lines", () => {
  it("clips a line to the inside of a polygon", () => {
    expect(clipLine(square(10), [5, -3], [0, 1])).toEqual([[[5, 0], [5, 10]]]);
    expect(clipLine(square(10), [20, 0], [0, 1])).toEqual([]);
  });

  it("splits a line that crosses a concave polygon twice", () => {
    // A U shape: the line y = 8 crosses both arms.
    const u: PointMm[] = [
      { xMm: 0, yMm: 0 }, { xMm: 30, yMm: 0 }, { xMm: 30, yMm: 10 }, { xMm: 20, yMm: 10 },
      { xMm: 20, yMm: 5 }, { xMm: 10, yMm: 5 }, { xMm: 10, yMm: 10 }, { xMm: 0, yMm: 10 },
    ];
    expect(clipLine(u, [-1, 8], [1, 0])).toEqual([[[0, 8], [10, 8]], [[20, 8], [30, 8]]]);
  });

  it("grids a bed every 10 mm with a major line every 50 mm", () => {
    const { minor, major } = gridLines(square(100));
    // 9 interior lines each way (the outline draws the edges); 50 is major.
    expect(major).toHaveLength(2);
    expect(minor).toHaveLength(16);
    expect([...minor, ...major].every((segment) => length(segment) === 100)).toBe(true);
  });

  it("keeps a round bed's grid inside the circle", () => {
    const circle: PointMm[] = Array.from({ length: 64 }, (_, i) => ({
      xMm: 50 * Math.cos((2 * Math.PI * i) / 64),
      yMm: 50 * Math.sin((2 * Math.PI * i) / 64),
    }));
    const { minor, major } = gridLines(circle);
    for (const [[ax, ay], [bx, by]] of [...minor, ...major]) {
      expect(Math.hypot(ax, ay)).toBeLessThanOrEqual(50.0001);
      expect(Math.hypot(bx, by)).toBeLessThanOrEqual(50.0001);
    }
  });

  it("hatches an exclude area with 45° lines inside it", () => {
    const lines = hatchLines(square(20, 100, 100));
    expect(lines.length).toBeGreaterThan(4);
    for (const [[ax, ay], [bx, by]] of lines) {
      expect(bx - ax).toBeCloseTo(by - ay);
      for (const value of [ax, ay, bx, by]) {
        expect(value).toBeGreaterThanOrEqual(100 - 1e-9);
        expect(value).toBeLessThanOrEqual(120 + 1e-9);
      }
    }
  });
});
