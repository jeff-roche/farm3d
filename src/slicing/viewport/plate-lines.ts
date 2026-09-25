/** The line work the renderer draws on the bed, as plain 2D segments so it
 *  is testable without WebGL: the grid clipped to the bed outline, and the
 *  hatching that fills an exclude area. */
import type { PointMm } from "../types";

export type Segment2 = [[number, number], [number, number]];

export const GRID_STEP_MM = 10;
export const GRID_MAJOR_EVERY = 5;
export const HATCH_STEP_MM = 5;

/** The parameters `t` along the line `origin + t·direction` where it
 *  crosses the polygon's edges, sorted, so consecutive pairs are the
 *  inside spans (even-odd rule). */
function crossings(polygon: PointMm[], origin: [number, number], direction: [number, number]): number[] {
  const ts: number[] = [];
  const [ox, oy] = origin;
  const [dx, dy] = direction;
  for (let i = 0; i < polygon.length; i += 1) {
    const a = polygon[i];
    const b = polygon[(i + 1) % polygon.length];
    // Signed distances of the edge's ends from the line.
    const da = (a.xMm - ox) * dy - (a.yMm - oy) * dx;
    const db = (b.xMm - ox) * dy - (b.yMm - oy) * dx;
    // Half-open, so a line through a vertex counts it once.
    if ((da < 0) === (db < 0)) continue;
    const s = da / (da - db);
    const x = a.xMm + (b.xMm - a.xMm) * s;
    const y = a.yMm + (b.yMm - a.yMm) * s;
    ts.push((x - ox) * dx + (y - oy) * dy);
  }
  return ts.sort((p, q) => p - q);
}

/** The parts of the line `origin + t·direction` (unit `direction`) inside
 *  `polygon`. */
export function clipLine(polygon: PointMm[], origin: [number, number], direction: [number, number]): Segment2[] {
  const ts = crossings(polygon, origin, direction);
  const segments: Segment2[] = [];
  for (let i = 0; i + 1 < ts.length; i += 2) {
    if (ts[i + 1] - ts[i] <= 1e-9) continue;
    const at = (t: number): [number, number] => [origin[0] + direction[0] * t, origin[1] + direction[1] * t];
    segments.push([at(ts[i]), at(ts[i + 1])]);
  }
  return segments;
}

function extent(polygon: PointMm[]) {
  const xs = polygon.map((point) => point.xMm);
  const ys = polygon.map((point) => point.yMm);
  return { x0: Math.min(...xs), x1: Math.max(...xs), y0: Math.min(...ys), y1: Math.max(...ys) };
}

/** Grid lines on multiples of `step` strictly inside `polygon`, split into minor
 *  and major (every `majorEvery`th line) sets. */
export function gridLines(
  polygon: PointMm[],
  step = GRID_STEP_MM,
  majorEvery = GRID_MAJOR_EVERY,
): { minor: Segment2[]; major: Segment2[] } {
  const minor: Segment2[] = [];
  const major: Segment2[] = [];
  if (polygon.length < 3) return { minor, major };
  const { x0, x1, y0, y1 } = extent(polygon);
  const place = (index: number, segments: Segment2[]) => (
    (index % majorEvery === 0 ? major : minor).push(...segments)
  );
  // Strictly inside the extent: the outline itself draws the edges.
  for (let i = Math.floor(x0 / step) + 1; i * step < x1; i += 1) {
    place(i, clipLine(polygon, [i * step, y0], [0, 1]));
  }
  for (let i = Math.floor(y0 / step) + 1; i * step < y1; i += 1) {
    place(i, clipLine(polygon, [x0, i * step], [1, 0]));
  }
  return { minor, major };
}

/** Diagonal hatching (45°) filling `polygon`, `step` apart. */
export function hatchLines(polygon: PointMm[], step = HATCH_STEP_MM): Segment2[] {
  if (polygon.length < 3) return [];
  const direction: [number, number] = [Math.SQRT1_2, Math.SQRT1_2];
  // Lines x − y = c, for c across the polygon's range.
  const cs = polygon.map((point) => point.xMm - point.yMm);
  const segments: Segment2[] = [];
  for (let i = Math.ceil(Math.min(...cs) / step); i * step <= Math.max(...cs); i += 1) {
    segments.push(...clipLine(polygon, [i * step, 0], direction));
  }
  return segments;
}
