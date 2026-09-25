/** D19's **Lay flat**: turn an instance so one of its object's lay-flat
 *  faces (`GeometryObject.layFlatFaces`, from the backend's convex hull,
 *  largest first) faces straight down. D5's Z drop then rests it on that
 *  face. Pure. */
import { eulerFromRotation, normalizeDegrees, rotationMatrix, type Matrix3, type Vec3 } from "./transforms";
import type { InstanceTransform, LayFlatFace } from "./types";

const DOWN: Vec3 = [0, 0, -1];

function normalize(v: readonly number[]): Vec3 | null {
  const length = Math.hypot(v[0], v[1], v[2]);
  return length > 0 && Number.isFinite(length) ? [v[0] / length, v[1] / length, v[2] / length] : null;
}

/** The smallest rotation taking unit vector `from` onto unit vector `to`
 *  (Rodrigues). Opposite vectors turn half-way about X, or about Y when
 *  `from` lies along X. */
function rotationBetween(from: Vec3, to: Vec3): Matrix3 {
  const cos = from[0] * to[0] + from[1] * to[1] + from[2] * to[2];
  let axis: Vec3 = [
    from[1] * to[2] - from[2] * to[1],
    from[2] * to[0] - from[0] * to[2],
    from[0] * to[1] - from[1] * to[0],
  ];
  let sin = Math.hypot(axis[0], axis[1], axis[2]);
  if (sin < 1e-12) {
    if (cos > 0) return [[1, 0, 0], [0, 1, 0], [0, 0, 1]];
    // Half a turn about any axis perpendicular to `from`.
    const helper: Vec3 = Math.abs(from[0]) < 0.9 ? [1, 0, 0] : [0, 1, 0];
    const dot = helper[0] * from[0] + helper[1] * from[1] + helper[2] * from[2];
    axis = normalize([helper[0] - dot * from[0], helper[1] - dot * from[1], helper[2] - dot * from[2]])!;
    sin = 0;
  } else {
    axis = [axis[0] / sin, axis[1] / sin, axis[2] / sin];
  }
  const angle = Math.atan2(sin, cos);
  const [x, y, z] = axis;
  const c = Math.cos(angle);
  const s = Math.sin(angle);
  const t = 1 - c;
  return [
    [t * x * x + c, t * x * y - s * z, t * x * z + s * y],
    [t * x * y + s * z, t * y * y + c, t * y * z - s * x],
    [t * x * z - s * y, t * y * z + s * x, t * z * z + c],
  ];
}

function multiply(a: Matrix3, b: Matrix3): Matrix3 {
  return [0, 1, 2].map((row) => [0, 1, 2].map((column) => (
    a[row][0] * b[0][column] + a[row][1] * b[1][column] + a[row][2] * b[2][column]
  ))) as Matrix3;
}

/** `transform` turned so `face` points straight down (−Z), keeping its
 *  turn about Z, its scale, and its XY translation (callers keep the
 *  object's centre with `keepCentre`). Under a non-uniform scale a face's
 *  placed normal is `S⁻¹·n`, so that is what is turned down. A face with no
 *  usable normal leaves the transform as it is. */
export function layFlat(transform: InstanceTransform, face: LayFlatFace): InstanceTransform {
  const scaled = normalize(face.normal.map((component, axis) => component / transform.scale[axis]));
  if (!scaled) return transform;
  const align = rotationBetween(scaled, DOWN);
  // Keep the heading: a turn about world Z leaves "down" where it is.
  const heading = rotationMatrix([0, 0, transform.rotateDeg[2]]);
  const rotateDeg = eulerFromRotation(multiply(heading, align)).map(normalizeDegrees) as Vec3;
  return { ...transform, rotateDeg };
}

/** **F** cycles through the faces, largest first: the index after
 *  `current`, wrapping; `undefined` when there are none. */
export function nextLayFlatFace(current: number | undefined, count: number): number | undefined {
  if (count <= 0) return undefined;
  if (current === undefined || current + 1 >= count || current < 0) return 0;
  return current + 1;
}
