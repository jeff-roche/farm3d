import { describe, expect, it } from "vitest";
import {
  composeTransform,
  IDENTITY_TRANSFORM,
  instanceTransformFrom3mf,
  transformedBounds,
  transformPoint,
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
