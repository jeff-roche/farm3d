/** D5 instance transforms, as the backend composes them
 *  (`src-tauri/src/slicing/geometry.rs`). Both sides are checked against
 *  `src-tauri/tests/fixtures/slicing/transform-vectors.json`.
 *
 *  A {@link Transform3mf} is the 3MF `transform` attribute's 12 numbers
 *  (`m00 m01 m02 m10 m11 m12 m20 m21 m22 m30 m31 m32`): a local point maps
 *  to `x' = x·m[0] + y·m[3] + z·m[6] + m[9]`, and likewise for y and z. */
import type { BoundsMm, InstanceTransform } from "./types";

export type Transform3mf = readonly number[];
export type Vec3 = [number, number, number];
/** x, y, z per vertex, as a mesh buffer carries them. */
export type Positions = ArrayLike<number>;

/** D5's per-axis scale limits. */
export const MIN_SCALE = 0.01;
export const MAX_SCALE = 100;

export const IDENTITY_TRANSFORM: InstanceTransform = {
  translateMm: [0, 0],
  rotateDeg: [0, 0, 0],
  scale: [1, 1, 1],
};

type Matrix3 = [Vec3, Vec3, Vec3];

function multiply(a: Matrix3, b: Matrix3): Matrix3 {
  const out: Matrix3 = [[0, 0, 0], [0, 0, 0], [0, 0, 0]];
  for (let row = 0; row < 3; row += 1) {
    for (let column = 0; column < 3; column += 1) {
      out[row][column] = a[row][0] * b[0][column] + a[row][1] * b[1][column] + a[row][2] * b[2][column];
    }
  }
  return out;
}

/** The Z translation that puts the lowest *vertex* (not a bounding-box
 *  corner) of `positions`, placed by `m`, exactly on Z = 0. 0 with no
 *  vertices. */
export function restingZ(m: Transform3mf, positions: Positions): number {
  let lowest = Infinity;
  for (let i = 0; i + 2 < positions.length; i += 3) {
    const z = positions[i] * m[2] + positions[i + 1] * m[5] + positions[i + 2] * m[8];
    if (z < lowest) lowest = z;
  }
  return lowest === Infinity ? 0 : -lowest;
}

/** D5: the 3MF transform for an instance of an object whose local vertices
 *  are `positions`. The mesh is scaled, rotated about X, then Y, then Z
 *  (extrinsic), and translated on XY; Z is derived so it rests on the bed. */
export function composeTransform(transform: InstanceTransform, positions: Positions): number[] {
  const [sx, sy, sz] = transform.scale;
  const [ax, ay, az] = transform.rotateDeg.map((degrees) => degrees * (Math.PI / 180));
  const [sinX, cosX] = [Math.sin(ax), Math.cos(ax)];
  const [sinY, cosY] = [Math.sin(ay), Math.cos(ay)];
  const [sinZ, cosZ] = [Math.sin(az), Math.cos(az)];
  const rotateX: Matrix3 = [[1, 0, 0], [0, cosX, -sinX], [0, sinX, cosX]];
  const rotateY: Matrix3 = [[cosY, 0, sinY], [0, 1, 0], [-sinY, 0, cosY]];
  const rotateZ: Matrix3 = [[cosZ, -sinZ, 0], [sinZ, cosZ, 0], [0, 0, 1]];
  // Column-vector form: world = Rz · Ry · Rx · S · local.
  const r = multiply(rotateZ, multiply(rotateY, rotateX));
  const scales = [sx, sy, sz];
  // The 3MF attribute holds the transpose (row-vector form).
  const m = new Array<number>(12).fill(0);
  for (let local = 0; local < 3; local += 1) {
    for (let world = 0; world < 3; world += 1) {
      m[local * 3 + world] = r[world][local] * scales[local];
    }
  }
  m[9] = transform.translateMm[0];
  m[10] = transform.translateMm[1];
  m[11] = restingZ(m, positions);
  return m;
}

export function transformPoint(m: Transform3mf, [x, y, z]: Vec3): Vec3 {
  return [
    x * m[0] + y * m[3] + z * m[6] + m[9],
    x * m[1] + y * m[4] + z * m[7] + m[10],
    x * m[2] + y * m[5] + z * m[8] + m[11],
  ];
}

/** The world bounds of `positions` placed by `m`, over the vertices
 *  themselves. `null` with no vertices. */
export function transformedBounds(m: Transform3mf, positions: Positions): BoundsMm | null {
  if (positions.length < 3) return null;
  const min: Vec3 = [Infinity, Infinity, Infinity];
  const max: Vec3 = [-Infinity, -Infinity, -Infinity];
  for (let i = 0; i + 2 < positions.length; i += 3) {
    const point = transformPoint(m, [positions[i], positions[i + 1], positions[i + 2]]);
    for (let axis = 0; axis < 3; axis += 1) {
      if (point[axis] < min[axis]) min[axis] = point[axis];
      if (point[axis] > max[axis]) max[axis] = point[axis];
    }
  }
  return { min, max };
}

/** Rounds away float noise (1e-12, -0.0), as the backend does. */
function tidy(value: number): number {
  const rounded = Math.round(value * 1e9) / 1e9;
  return rounded === 0 ? 0 : rounded;
}

/** D5's instance transform closest to a 3MF build transform: each local
 *  axis's length is its scale, and the rotation its X→Y→Z extrinsic
 *  angles. A mirrored (or degenerate) transform can't be expressed, so it
 *  keeps its scale with no rotation. Mirrors the backend's seeding. */
export function instanceTransformFrom3mf(m: Transform3mf): InstanceTransform {
  const columns: Vec3[] = [0, 1, 2].map((local) => [m[local * 3], m[local * 3 + 1], m[local * 3 + 2]] as Vec3);
  const lengths = columns.map((v) => Math.hypot(v[0], v[1], v[2]));
  const scale = lengths.map((factor) => (
    Number.isFinite(factor) && factor > 0 ? tidy(Math.min(MAX_SCALE, Math.max(MIN_SCALE, factor))) : 1
  )) as Vec3;
  const [c0, c1, c2] = columns;
  const determinant = c0[0] * (c1[1] * c2[2] - c2[1] * c1[2])
    - c1[0] * (c0[1] * c2[2] - c2[1] * c0[2])
    + c2[0] * (c0[1] * c1[2] - c1[1] * c0[2]);
  let rotateDeg: Vec3 = [0, 0, 0];
  if (determinant > 0 && lengths.every((factor) => factor > 0)) {
    // r[world][local], with each local axis normalised.
    const r = (world: number, local: number) => columns[local][world] / lengths[local];
    const y = Math.asin(Math.min(1, Math.max(-1, -r(2, 0))));
    const [x, z] = Math.abs(Math.cos(y)) > 1e-9
      ? [Math.atan2(r(2, 1), r(2, 2)), Math.atan2(r(1, 0), r(0, 0))]
      // Gimbal lock: fold the X rotation into Z.
      : [0, Math.atan2(-r(0, 1), r(1, 1))];
    rotateDeg = [x, y, z].map((radians) => tidy((radians * 180) / Math.PI)) as Vec3;
  }
  return { translateMm: [tidy(m[9]), tidy(m[10])], rotateDeg, scale };
}
