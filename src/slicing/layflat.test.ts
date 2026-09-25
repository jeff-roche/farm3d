import { describe, expect, it } from "vitest";
import { layFlat, nextLayFlatFace } from "./layflat";
import { composeTransform, rotationMatrix, transformPoint, type Vec3 } from "./transforms";
import type { InstanceTransform, LayFlatFace } from "./types";

/** A box from the origin, as flat x, y, z positions. */
function box(width: number, depth: number, height: number): Vec3[] {
  return [
    [0, 0, 0], [width, 0, 0], [width, depth, 0], [0, depth, 0],
    [0, 0, height], [width, 0, height], [width, depth, height], [0, depth, height],
  ];
}

const flat = (points: Vec3[]) => Float32Array.from(points.flat());
const dot = (a: readonly number[], b: readonly number[]) => a[0] * b[0] + a[1] * b[1] + a[2] * b[2];

/** The vertices on the face with outward normal `normal`: the ones that
 *  reach furthest along it. */
function faceVertices(points: Vec3[], normal: readonly number[]): Vec3[] {
  const reach = Math.max(...points.map((p) => dot(p, normal)));
  return points.filter((p) => Math.abs(dot(p, normal) - reach) < 1e-4);
}

/** World Z of every vertex once the transform is composed (so resting). */
function worldZ(transform: InstanceTransform, points: Vec3[]): number[] {
  const positions = flat(points);
  const m = composeTransform(transform, positions);
  return points.map((p) => transformPoint(m, [Math.fround(p[0]), Math.fround(p[1]), Math.fround(p[2])])[2]);
}

function expectResting(transform: InstanceTransform, points: Vec3[], normal: readonly number[]) {
  const onFace = new Set(faceVertices(points, normal));
  const z = worldZ(transform, points);
  points.forEach((point, i) => {
    if (onFace.has(point)) expect(Math.abs(z[i]), `face vertex ${point.join(",")}`).toBeLessThan(1e-4);
    else expect(z[i], `vertex ${point.join(",")}`).toBeGreaterThan(-1e-6);
  });
  // Nothing else touches the bed: the face really is what it rests on.
  expect(z.filter((value) => Math.abs(value) < 1e-4).length).toBe(onFace.size);
}

const CUBE_FACES: LayFlatFace[] = [
  { normal: [0, 0, -1], areaMm2: 100 },
  { normal: [0, 0, 1], areaMm2: 100 },
  { normal: [1, 0, 0], areaMm2: 100 },
  { normal: [-1, 0, 0], areaMm2: 100 },
  { normal: [0, 1, 0], areaMm2: 100 },
  { normal: [0, -1, 0], areaMm2: 100 },
];

describe("lay flat", () => {
  const cube = box(10, 10, 10);
  const upright: InstanceTransform = { translateMm: [50, 60], rotateDeg: [0, 0, 0], scale: [1, 1, 1] };

  for (const face of CUBE_FACES) {
    it(`rests the cube on its ${face.normal.join(",")} face`, () => {
      const laid = layFlat(upright, face);
      expectResting(laid, cube, face.normal);
      expect(laid.translateMm).toEqual([50, 60]);
      expect(laid.scale).toEqual([1, 1, 1]);
    });
  }

  it("leaves a cube already on its bottom face as it is, keeping its turn about Z", () => {
    const turned: InstanceTransform = { ...upright, rotateDeg: [0, 0, 45] };
    const laid = layFlat(turned, CUBE_FACES[0]);
    laid.rotateDeg.forEach((angle, i) => expect(angle).toBeCloseTo([0, 0, 45][i], 9));
  });

  it("flips the cube over for its top face", () => {
    const laid = layFlat(upright, CUBE_FACES[1]);
    // Upside down: X or Y is ±180°, and the top face's normal now points down.
    const r = rotationMatrix(laid.rotateDeg);
    const world = [0, 1, 2].map((row) => dot(r[row], [0, 0, 1]));
    expect(world[2]).toBeCloseTo(-1, 9);
  });

  it("rests a tilted, non-uniformly scaled box on the chosen face", () => {
    // The box's own frame is tilted, so no face lines up with an axis.
    const tilt = rotationMatrix([25, -40, 10]);
    const rotate = (p: readonly number[]): Vec3 => [0, 1, 2].map((row) => dot(tilt[row], p)) as Vec3;
    const tilted = box(30, 20, 5).map(rotate);
    const bottom = rotate([0, 0, -1]);
    const side = rotate([1, 0, 0]);
    const start: InstanceTransform = { translateMm: [10, 20], rotateDeg: [5, 7, 30], scale: [1, 2, 1.5] };

    // The face is picked by its own (local) normal; the scale only changes
    // which way it faces once placed.
    expectResting(layFlat(start, { normal: bottom, areaMm2: 600 }), tilted, bottom);
    expectResting(layFlat(start, { normal: side, areaMm2: 100 }), tilted, side);
  });

  it("cycles through the faces, largest first, and wraps", () => {
    expect(nextLayFlatFace(undefined, 3)).toBe(0);
    expect(nextLayFlatFace(0, 3)).toBe(1);
    expect(nextLayFlatFace(2, 3)).toBe(0);
    expect(nextLayFlatFace(5, 3)).toBe(0);
    expect(nextLayFlatFace(undefined, 0)).toBeUndefined();
  });
});
