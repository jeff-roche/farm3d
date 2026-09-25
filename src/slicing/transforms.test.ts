import { describe, expect, it } from "vitest";
import {
  clampScale,
  composeTransform,
  eulerFromRotation,
  IDENTITY_TRANSFORM,
  instanceTransformFrom3mf,
  keepCentre,
  linearPart,
  MAX_SCALE,
  MIN_SCALE,
  normalizeDegrees,
  rotateZBy,
  rotationMatrix,
  scaleBy,
  transformedBounds,
  transformPoint,
  type Vec3,
} from "./transforms";
import type { BoundsMm, InstanceTransform } from "./types";
import committedVectors from "../../src-tauri/tests/fixtures/slicing/transform-vectors.json";

interface TransformCase {
  name: string;
  transform: InstanceTransform;
  matrix: number[];
  worldBoundsMm: BoundsMm;
}

interface TransformVectors {
  tolerance: number;
  positions: [number, number, number][];
  cases: TransformCase[];
}

// Shared with the Rust side (`tests/p5_geometry.rs`), which pins it.
const vectors = committedVectors as unknown as TransformVectors;
const positions = Float32Array.from(vectors.positions.flat());

function expectClose(actual: readonly number[], expected: readonly number[], what: string) {
  expect(actual.length, what).toBe(expected.length);
  actual.forEach((value, i) => {
    expect(Math.abs(value - expected[i]), `${what}[${i}]: ${value} vs ${expected[i]}`).toBeLessThanOrEqual(vectors.tolerance);
  });
}

describe("D5 transforms", () => {
  it("covers every committed vector", () => {
    expect(vectors.cases.map((c) => c.name)).toEqual([
      "identity", "rotateX90", "rotateY90", "rotateZ90", "rotateX30", "nonUniformScale", "combined", "rotateZ45Translated",
    ]);
  });

  for (const vector of vectors.cases) {
    it(`composes ${vector.name} as the backend does`, () => {
      const matrix = composeTransform(vector.transform, positions);
      expectClose(matrix, vector.matrix, "matrix");
      const bounds = transformedBounds(matrix, positions);
      expectClose(bounds!.min, vector.worldBoundsMm.min, "min");
      expectClose(bounds!.max, vector.worldBoundsMm.max, "max");
      expect(bounds!.min[2]).toBe(0);
    });

    it(`recovers ${vector.name}'s fields from its matrix`, () => {
      const recovered = instanceTransformFrom3mf(vector.matrix);
      expectClose(recovered.translateMm, vector.transform.translateMm, "translate");
      expectClose(recovered.scale, vector.transform.scale, "scale");
      // Recomposing is what matters: Euler angles are not unique at 90°.
      expectClose(composeTransform(recovered, positions), vector.matrix, "recomposed");
    });
  }

  it("drops a mesh with no vertices to Z 0 and has no bounds for it", () => {
    const matrix = composeTransform(IDENTITY_TRANSFORM, new Float32Array());
    expect(matrix[11]).toBe(0);
    expect(transformedBounds(matrix, new Float32Array())).toBeNull();
  });

  it("maps a point with the 3MF row-vector convention", () => {
    const m = [1, 0, 0, 0, 1, 0, 0, 0, 1, 10, 20, 30];
    expect(transformPoint(m, [1, 2, 3])).toEqual([11, 22, 33]);
  });

  it("keeps a mirrored transform's scale with no rotation", () => {
    const mirrored = [-2, 0, 0, 0, 1, 0, 0, 0, 1, 5, 6, 7];
    expect(instanceTransformFrom3mf(mirrored)).toEqual({ translateMm: [5, 6], rotateDeg: [0, 0, 0], scale: [2, 1, 1] });
  });
});

describe("transform edits", () => {
  // A 10 × 20 × 4 box from the origin.
  const box = Float32Array.from([
    0, 0, 0, 10, 0, 0, 10, 20, 0, 0, 20, 0,
    0, 0, 4, 10, 0, 4, 10, 20, 4, 0, 20, 4,
  ]);
  const centreXY = (transform: InstanceTransform) => {
    const bounds = transformedBounds(composeTransform(transform, box), box)!;
    return [(bounds.min[0] + bounds.max[0]) / 2, (bounds.min[1] + bounds.max[1]) / 2];
  };

  it("recovers X→Y→Z extrinsic angles from a rotation matrix", () => {
    for (const angles of [[0, 0, 0], [30, -20, 45], [-170, 60, 120], [90, 0, 0], [12.5, 33, -99]] as Vec3[]) {
      expectClose(eulerFromRotation(rotationMatrix(angles)), angles, `angles ${angles.join(",")}`);
    }
  });

  it("composes the linear part the same way composeTransform does", () => {
    const transform: InstanceTransform = { translateMm: [5, 6], rotateDeg: [10, 20, 30], scale: [1, 2, 3] };
    expectClose(linearPart(transform), composeTransform(transform, box).slice(0, 9), "linear");
  });

  it("wraps angles into (-180, 180]", () => {
    expect(normalizeDegrees(185)).toBe(-175);
    expect(normalizeDegrees(-180)).toBe(180);
    expect(normalizeDegrees(540)).toBe(180);
    expect(normalizeDegrees(-0)).toBe(0);
    expect(normalizeDegrees(45)).toBe(45);
  });

  it("rotates about world Z by adding to the Z angle", () => {
    const start: InstanceTransform = { translateMm: [0, 0], rotateDeg: [10, 20, 170], scale: [1, 1, 1] };
    const turned = rotateZBy(start, 15);
    expect(turned.rotateDeg).toEqual([10, 20, -175]);
    // The same as composing Rz(15°) after the old rotation.
    const expected = composeTransform(start, box);
    const r = rotationMatrix([0, 0, 15]);
    const actual = composeTransform(turned, box);
    for (let local = 0; local < 3; local += 1) {
      for (let world = 0; world < 3; world += 1) {
        const rotated = r[world][0] * expected[local * 3] + r[world][1] * expected[local * 3 + 1] + r[world][2] * expected[local * 3 + 2];
        expect(Math.abs(actual[local * 3 + world] - rotated)).toBeLessThan(1e-9);
      }
    }
  });

  it("scales uniformly within D5's limits", () => {
    const start: InstanceTransform = { translateMm: [0, 0], rotateDeg: [0, 0, 0], scale: [1, 2, 0.5] };
    expect(scaleBy(start, 1.05).scale).toEqual([1.05, 2.1, 0.525]);
    expect(scaleBy({ ...start, scale: [99, 1, 1] }, 1.05).scale).toEqual([MAX_SCALE, 1.05, 1.05]);
    expect(scaleBy({ ...start, scale: [0.0101, 1, 1] }, 0.95).scale[0]).toBe(MIN_SCALE);
  });

  it("clamps a scale into D5's limits and rejects non-numbers", () => {
    expect(clampScale(0)).toBe(MIN_SCALE);
    expect(clampScale(1000)).toBe(MAX_SCALE);
    expect(clampScale(1.5)).toBe(1.5);
    expect(clampScale(Number.NaN)).toBe(1);
  });

  it("keeps an object's centre in place when it turns or scales", () => {
    const start: InstanceTransform = { translateMm: [100, 100], rotateDeg: [0, 0, 0], scale: [1, 1, 1] };
    const localCentre: Vec3 = [5, 10, 2];
    const turned = keepCentre(start, rotateZBy(start, 90), localCentre);
    expectClose(centreXY(turned), centreXY(start), "turned centre");
    const grown = keepCentre(start, scaleBy(start, 2), localCentre);
    expectClose(centreXY(grown), centreXY(start), "grown centre");
  });
});
