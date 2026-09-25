import { describe, expect, it } from "vitest";
import {
  checkPlacement,
  convexHull,
  footprintOf,
  measureBetween,
  placedHull,
  pointInPolygon,
  polygonsOverlap,
  polygonWithin,
  type Point2,
} from "./bounds";
import type { InstanceTransform } from "./types";
import type { BuildVolume } from "./viewport/renderer";

function box(width: number, depth: number, height: number): Float32Array {
  return Float32Array.from([
    0, 0, 0, width, 0, 0, width, depth, 0, 0, depth, 0,
    0, 0, height, width, 0, height, width, depth, height, 0, depth, height,
  ]);
}

const at = (x: number, y: number, rotateZ = 0): InstanceTransform => ({
  translateMm: [x, y], rotateDeg: [0, 0, rotateZ], scale: [1, 1, 1],
});

const rectangularBed: BuildVolume = {
  bed: { kind: "rectangular", widthMm: 200, depthMm: 100, originXMm: 0, originYMm: 0 },
  heightMm: 50,
  excludeAreas: [],
};

/** A round bed of radius 100 about the origin, as a 64-gon. */
const roundBed: BuildVolume = {
  bed: {
    kind: "polygon",
    points: Array.from({ length: 64 }, (_, i) => ({
      xMm: 100 * Math.cos((2 * Math.PI * i) / 64),
      yMm: 100 * Math.sin((2 * Math.PI * i) / 64),
    })),
  },
  heightMm: 200,
  excludeAreas: [],
};

/** An L: a 100 × 100 square with its top-right 50 × 50 quarter cut away. */
const lBed: BuildVolume = {
  bed: {
    kind: "polygon",
    points: [
      { xMm: 0, yMm: 0 }, { xMm: 100, yMm: 0 }, { xMm: 100, yMm: 50 },
      { xMm: 50, yMm: 50 }, { xMm: 50, yMm: 100 }, { xMm: 0, yMm: 100 },
    ],
  },
  heightMm: 100,
  excludeAreas: [],
};

describe("convex hull", () => {
  it("keeps only the corners, counter-clockwise, dropping inner and collinear points", () => {
    const points: Point2[] = [[0, 0], [10, 0], [5, 0], [10, 10], [0, 10], [5, 5], [2, 7]];
    expect(convexHull(points)).toEqual([[0, 0], [10, 0], [10, 10], [0, 10]]);
  });

  it("handles fewer than three distinct points", () => {
    expect(convexHull([[1, 1], [1, 1]])).toEqual([[1, 1]]);
    expect(convexHull([])).toEqual([]);
  });

  it("gets the same hull from many points as from their corners", () => {
    const points: Point2[] = [];
    for (let i = 0; i < 5000; i += 1) points.push([Math.sin(i) * 40, Math.cos(i * 1.3) * 20]);
    const hull = convexHull(points);
    for (const point of points) expect(pointInPolygon(point, hull)).not.toBe("outside");
  });
});

describe("footprint", () => {
  it("is the placed outline relative to the translation, with the height after resting", () => {
    const footprint = footprintOf(at(0, 0), box(20, 10, 5))!;
    expect(footprint.hull).toEqual([[0, 0], [20, 0], [20, 10], [0, 10]]);
    expect(footprint.min).toEqual([0, 0]);
    expect(footprint.max).toEqual([20, 10]);
    expect(footprint.heightMm).toBe(5);
    expect(placedHull(footprint, [100, 50])).toEqual([[100, 50], [120, 50], [120, 60], [100, 60]]);
  });

  it("turns with the rotation", () => {
    const footprint = footprintOf(at(0, 0, 90), box(20, 10, 5))!;
    expect(footprint.min[0]).toBeCloseTo(-10, 9);
    expect(footprint.max[0]).toBeCloseTo(0, 9);
    expect(footprint.max[1]).toBeCloseTo(20, 9);
  });

  it("is null for an object with no vertices", () => {
    expect(footprintOf(at(0, 0), new Float32Array())).toBeNull();
  });
});

describe("polygons", () => {
  const square: Point2[] = [[0, 0], [10, 0], [10, 10], [0, 10]];

  it("tells inside, outside and on the boundary", () => {
    expect(pointInPolygon([5, 5], square)).toBe("inside");
    expect(pointInPolygon([10, 5], square)).toBe("boundary");
    expect(pointInPolygon([0, 0], square)).toBe("boundary");
    expect(pointInPolygon([11, 5], square)).toBe("outside");
  });

  it("finds a polygon within a concave one only when no edge leaves it", () => {
    const l = (lBed.bed as { points: { xMm: number; yMm: number }[] }).points.map((p): Point2 => [p.xMm, p.yMm]);
    expect(polygonWithin([[10, 10], [40, 10], [40, 40], [10, 40]], l)).toBe(true);
    // Every corner is inside the L, but the box spans the cut-away corner.
    expect(polygonWithin([[40, 40], [90, 40], [90, 45], [45, 90], [40, 90]], l)).toBe(false);
  });

  it("counts real overlap, not touching", () => {
    expect(polygonsOverlap(square, [[5, 5], [15, 5], [15, 15], [5, 15]])).toBe(true);
    expect(polygonsOverlap(square, [[10, 0], [20, 0], [20, 10], [10, 10]])).toBe(false);
    expect(polygonsOverlap(square, [[2, 2], [4, 2], [4, 4], [2, 4]])).toBe(true);
    expect(polygonsOverlap([[2, 2], [4, 2], [4, 4], [2, 4]], square)).toBe(true);
    expect(polygonsOverlap(square, square)).toBe(true);
    expect(polygonsOverlap(square, [[-5, 4], [15, 4], [15, 6], [-5, 6]])).toBe(true);
  });
});

describe("placement", () => {
  const part = footprintOf(at(0, 0), box(20, 10, 5))!;

  it("is inside a rectangular bed, even flush with its edge", () => {
    expect(checkPlacement(part, [10, 10], rectangularBed)).toEqual({ outOfBounds: false, inExcludeArea: false, tooTall: false });
    expect(checkPlacement(part, [180, 90], rectangularBed).outOfBounds).toBe(false);
    expect(checkPlacement(part, [180.5, 90], rectangularBed).outOfBounds).toBe(true);
    expect(checkPlacement(part, [-1, 10], rectangularBed).outOfBounds).toBe(true);
  });

  it("uses the bed polygon itself, not its bounding box", () => {
    expect(checkPlacement(part, [-10, -5], roundBed).outOfBounds).toBe(false);
    // Inside the round bed's bounding box, but off the bed.
    expect(checkPlacement(part, [75, 85], roundBed).outOfBounds).toBe(true);
    expect(checkPlacement(part, [60, 60], lBed).outOfBounds).toBe(true);
    expect(checkPlacement(part, [60, 20], lBed).outOfBounds).toBe(false);
  });

  it("finds an instance in an exclude area", () => {
    const volume: BuildVolume = {
      ...rectangularBed,
      excludeAreas: [[{ xMm: 0, yMm: 0 }, { xMm: 30, yMm: 0 }, { xMm: 30, yMm: 20 }, { xMm: 0, yMm: 20 }]],
    };
    expect(checkPlacement(part, [20, 15], volume).inExcludeArea).toBe(true);
    expect(checkPlacement(part, [30, 0], volume).inExcludeArea).toBe(false);
    expect(checkPlacement(part, [100, 50], volume).inExcludeArea).toBe(false);
  });

  it("finds an instance taller than the volume", () => {
    const tall = footprintOf(at(0, 0), box(10, 10, 60))!;
    expect(checkPlacement(tall, [10, 10], rectangularBed).tooTall).toBe(true);
    expect(checkPlacement(footprintOf(at(0, 0), box(10, 10, 50))!, [10, 10], rectangularBed).tooTall).toBe(false);
  });
});

describe("measure", () => {
  it("gives the centre-to-centre distance and the gap between two objects", () => {
    const a = footprintOf(at(0, 0), box(10, 10, 10))!;
    const b = footprintOf(at(0, 0), box(10, 10, 20))!;
    const result = measureBetween({ footprint: a, translateMm: [0, 0] }, { footprint: b, translateMm: [30, 0] });
    expect(result.from).toEqual([5, 5, 5]);
    expect(result.to).toEqual([35, 5, 10]);
    expect(result.centreDistanceMm).toBeCloseTo(Math.hypot(30, 0, 5), 9);
    expect(result.gapMm).toBe(20);
  });

  it("has no gap when the objects overlap", () => {
    const a = footprintOf(at(0, 0), box(10, 10, 10))!;
    const result = measureBetween({ footprint: a, translateMm: [0, 0] }, { footprint: a, translateMm: [5, 5] });
    expect(result.gapMm).toBe(0);
  });

  it("measures a diagonal gap", () => {
    const a = footprintOf(at(0, 0), box(10, 10, 10))!;
    const result = measureBetween({ footprint: a, translateMm: [0, 0] }, { footprint: a, translateMm: [13, 14] });
    expect(result.gapMm).toBeCloseTo(5, 9);
  });
});
