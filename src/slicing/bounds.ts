/** Where a placed instance sits relative to the printable area (D19,
 *  D20): its footprint on the bed, whether that footprint is inside the bed
 *  outline (a real polygon test, so round and odd-shaped beds are exact),
 *  whether it overlaps an exclude area, and whether it is taller than the
 *  volume. Also the measure tool's object-to-object distance. Pure.
 *
 *  **Tolerance.** Only float noise ({@link EPSILON_MM}) is forgiven: an
 *  object flush with the bed edge is inside, and one a fraction of a
 *  millimetre over is out. D19 and D20 give no tolerance, and the
 *  backend's 2 mm (D11 check 5) applies to the *printed* G-code bounds,
 *  which include extrusion width, not to the model's own outline. */
import { bedOutline } from "./viewport/build-volume";
import type { BuildVolume } from "./viewport/renderer";
import { linearPart, type Positions, type Vec3 } from "./transforms";
import type { InstanceTransform, PointMm } from "./types";

export type Point2 = [number, number];

/** How far a point may stray and still count as on a line. */
export const EPSILON_MM = 1e-6;

/** An instance's outline on the bed with its XY translation left out, so
 *  a move needs no vertex scan: the convex hull of its placed vertices
 *  (counter-clockwise), their XY extent, and its height once rested on the
 *  bed. Depends only on the object's mesh, rotation and scale. */
export interface Footprint {
  hull: Point2[];
  min: Point2;
  max: Point2;
  heightMm: number;
}

const cross = (o: Point2, a: Point2, b: Point2) => (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);

/** Andrew's monotone chain: the hull counter-clockwise from the lowest-left
 *  point, without collinear points. Fewer than three distinct points come
 *  back as they are (deduplicated). */
export function convexHull(points: readonly Point2[]): Point2[] {
  const sorted = [...points].sort((a, b) => a[0] - b[0] || a[1] - b[1]);
  const unique = sorted.filter((p, i) => i === 0 || p[0] !== sorted[i - 1][0] || p[1] !== sorted[i - 1][1]);
  if (unique.length < 3) return unique;
  const lower: Point2[] = [];
  for (const p of unique) {
    while (lower.length >= 2 && cross(lower[lower.length - 2], lower[lower.length - 1], p) <= 0) lower.pop();
    lower.push(p);
  }
  const upper: Point2[] = [];
  for (let i = unique.length - 1; i >= 0; i -= 1) {
    const p = unique[i];
    while (upper.length >= 2 && cross(upper[upper.length - 2], upper[upper.length - 1], p) <= 0) upper.pop();
    upper.push(p);
  }
  return [...lower.slice(0, -1), ...upper.slice(0, -1)];
}

/** Akl–Toussaint: the points that can still be on the hull once the
 *  extremes in eight directions are known. Keeps a million-vertex mesh's
 *  hull from sorting every vertex. */
function hullCandidates(xy: Float64Array): Point2[] {
  const count = xy.length / 2;
  const directions: Point2[] = [[1, 0], [1, 1], [0, 1], [-1, 1], [-1, 0], [-1, -1], [0, -1], [1, -1]];
  const best = directions.map(() => -Infinity);
  const extreme: Point2[] = directions.map(() => [0, 0]);
  for (let i = 0; i < count; i += 1) {
    const x = xy[i * 2];
    const y = xy[i * 2 + 1];
    for (let d = 0; d < directions.length; d += 1) {
      const reach = x * directions[d][0] + y * directions[d][1];
      if (reach > best[d]) {
        best[d] = reach;
        extreme[d] = [x, y];
      }
    }
  }
  const octagon = convexHull(extreme);
  if (octagon.length < 3) {
    const all: Point2[] = [];
    for (let i = 0; i < count; i += 1) all.push([xy[i * 2], xy[i * 2 + 1]]);
    return all;
  }
  const kept: Point2[] = [...octagon];
  for (let i = 0; i < count; i += 1) {
    const point: Point2 = [xy[i * 2], xy[i * 2 + 1]];
    let strictlyInside = true;
    for (let e = 0; e < octagon.length; e += 1) {
      if (cross(octagon[e], octagon[(e + 1) % octagon.length], point) <= EPSILON_MM) {
        strictlyInside = false;
        break;
      }
    }
    if (!strictlyInside) kept.push(point);
  }
  return kept;
}

/** The footprint of `positions` placed by `transform`'s scale and
 *  rotation; `null` with no vertices. */
export function footprintOf(transform: InstanceTransform, positions: Positions): Footprint | null {
  const count = Math.floor(positions.length / 3);
  if (count === 0) return null;
  const m = linearPart(transform);
  const xy = new Float64Array(count * 2);
  let minZ = Infinity;
  let maxZ = -Infinity;
  const min: Point2 = [Infinity, Infinity];
  const max: Point2 = [-Infinity, -Infinity];
  for (let i = 0; i < count; i += 1) {
    const x = positions[i * 3];
    const y = positions[i * 3 + 1];
    const z = positions[i * 3 + 2];
    const wx = x * m[0] + y * m[3] + z * m[6];
    const wy = x * m[1] + y * m[4] + z * m[7];
    const wz = x * m[2] + y * m[5] + z * m[8];
    xy[i * 2] = wx;
    xy[i * 2 + 1] = wy;
    if (wx < min[0]) min[0] = wx;
    if (wy < min[1]) min[1] = wy;
    if (wx > max[0]) max[0] = wx;
    if (wy > max[1]) max[1] = wy;
    if (wz < minZ) minZ = wz;
    if (wz > maxZ) maxZ = wz;
  }
  return { hull: convexHull(hullCandidates(xy)), min, max, heightMm: maxZ - minZ };
}

/** The footprint's hull on the bed, at `translateMm`. */
export function placedHull(footprint: Footprint, [tx, ty]: readonly number[]): Point2[] {
  return footprint.hull.map(([x, y]) => [x + tx, y + ty]);
}

function onSegment(p: Point2, a: Point2, b: Point2): boolean {
  const length = Math.hypot(b[0] - a[0], b[1] - a[1]);
  if (length === 0) return Math.hypot(p[0] - a[0], p[1] - a[1]) <= EPSILON_MM;
  if (Math.abs(cross(a, b, p)) / length > EPSILON_MM) return false;
  const t = ((p[0] - a[0]) * (b[0] - a[0]) + (p[1] - a[1]) * (b[1] - a[1])) / (length * length);
  return t >= -EPSILON_MM / length && t <= 1 + EPSILON_MM / length;
}

/** Where `point` is relative to `polygon` (any simple polygon, either
 *  winding). */
export function pointInPolygon(point: Point2, polygon: readonly Point2[]): "inside" | "outside" | "boundary" {
  let inside = false;
  for (let i = 0, j = polygon.length - 1; i < polygon.length; j = i, i += 1) {
    const a = polygon[i];
    const b = polygon[j];
    if (onSegment(point, a, b)) return "boundary";
    if ((a[1] > point[1]) !== (b[1] > point[1])) {
      const x = a[0] + ((point[1] - a[1]) * (b[0] - a[0])) / (b[1] - a[1]);
      if (point[0] < x) inside = !inside;
    }
  }
  return inside ? "inside" : "outside";
}

/** Whether segments ab and cd cross at a point inside both (touching or
 *  running along each other does not count). */
function properlyCross(a: Point2, b: Point2, c: Point2, d: Point2): boolean {
  const scale = (p: Point2, q: Point2) => Math.max(Math.hypot(q[0] - p[0], q[1] - p[1]), EPSILON_MM);
  const d1 = cross(c, d, a) / scale(c, d);
  const d2 = cross(c, d, b) / scale(c, d);
  const d3 = cross(a, b, c) / scale(a, b);
  const d4 = cross(a, b, d) / scale(a, b);
  const opposite = (u: number, v: number) => (u > EPSILON_MM && v < -EPSILON_MM) || (u < -EPSILON_MM && v > EPSILON_MM);
  return opposite(d1, d2) && opposite(d3, d4);
}

function edgesCross(p: readonly Point2[], q: readonly Point2[]): boolean {
  for (let i = 0; i < p.length; i += 1) {
    const a = p[i];
    const b = p[(i + 1) % p.length];
    for (let j = 0; j < q.length; j += 1) {
      if (properlyCross(a, b, q[j], q[(j + 1) % q.length])) return true;
    }
  }
  return false;
}

function centroid(polygon: readonly Point2[]): Point2 {
  const sum = polygon.reduce<Point2>((acc, [x, y]) => [acc[0] + x, acc[1] + y], [0, 0]);
  return [sum[0] / polygon.length, sum[1] / polygon.length];
}

/** Whether `inner` lies within `outer` (which may be concave), its
 *  boundary included: no vertex outside, and no edge leaving through a
 *  notch between two inside vertices. */
export function polygonWithin(inner: readonly Point2[], outer: readonly Point2[]): boolean {
  if (outer.length < 3) return false;
  if (inner.some((point) => pointInPolygon(point, outer) === "outside")) return false;
  if (edgesCross(inner, outer)) return false;
  // Every vertex on the boundary (a shape spanning a notch corner to
  // corner): its middle decides.
  return inner.length < 3 || pointInPolygon(centroid(inner), outer) !== "outside";
}

/** Whether two polygons share area. Touching along an edge or at a corner
 *  is not overlapping. `a` is convex (a footprint hull); `b` may not be. */
export function polygonsOverlap(a: readonly Point2[], b: readonly Point2[]): boolean {
  if (a.length === 0 || b.length === 0) return false;
  if (edgesCross(a, b)) return true;
  if (a.some((point) => pointInPolygon(point, b) === "inside")) return true;
  if (b.some((point) => pointInPolygon(point, a) === "inside")) return true;
  // Identical outlines, or every vertex on the other's boundary.
  return (a.length >= 3 && pointInPolygon(centroid(a), b) === "inside")
    || (b.length >= 3 && pointInPolygon(centroid(b), a) === "inside");
}

const toPoint = (point: PointMm): Point2 => [point.xMm, point.yMm];

export interface PlacementCheck {
  /** Some of the footprint is off the bed outline. */
  outOfBounds: boolean;
  /** The footprint overlaps one of the bed's exclude areas. */
  inExcludeArea: boolean;
  /** Taller than the build volume. */
  tooTall: boolean;
}

/** D20's placement checks for one instance. */
export function checkPlacement(footprint: Footprint, translateMm: readonly number[], volume: BuildVolume): PlacementCheck {
  const hull = placedHull(footprint, translateMm);
  const bed = bedOutline(volume.bed).map(toPoint);
  return {
    outOfBounds: !polygonWithin(hull, bed),
    inExcludeArea: volume.excludeAreas.some((area) => area.length >= 3 && polygonsOverlap(hull, area.map(toPoint))),
    tooTall: footprint.heightMm > volume.heightMm + EPSILON_MM,
  };
}

export interface PlacedFootprint {
  footprint: Footprint;
  translateMm: readonly number[];
}

export interface Measurement {
  /** The centre of each object's placed bounding box. */
  from: Vec3;
  to: Vec3;
  centreDistanceMm: number;
  /** The shortest distance between the two bounding boxes; 0 when they
   *  touch or overlap. */
  gapMm: number;
}

function placedBox({ footprint, translateMm: [tx, ty] }: PlacedFootprint): { min: Vec3; max: Vec3 } {
  return {
    min: [footprint.min[0] + tx, footprint.min[1] + ty, 0],
    max: [footprint.max[0] + tx, footprint.max[1] + ty, footprint.heightMm],
  };
}

/** D19 **Measure**'s keyboard alternative: the distance between two
 *  objects, centre to centre and as the gap between their bounding boxes
 *  (both rest on the bed). */
export function measureBetween(a: PlacedFootprint, b: PlacedFootprint): Measurement {
  const boxA = placedBox(a);
  const boxB = placedBox(b);
  const centre = (box: { min: Vec3; max: Vec3 }): Vec3 => [0, 1, 2].map((axis) => (box.min[axis] + box.max[axis]) / 2) as Vec3;
  const from = centre(boxA);
  const to = centre(boxB);
  const separation = [0, 1, 2].map((axis) => Math.max(0, boxA.min[axis] - boxB.max[axis], boxB.min[axis] - boxA.max[axis]));
  return {
    from,
    to,
    centreDistanceMm: Math.hypot(to[0] - from[0], to[1] - from[1], to[2] - from[2]),
    gapMm: Math.hypot(separation[0], separation[1], separation[2]),
  };
}
