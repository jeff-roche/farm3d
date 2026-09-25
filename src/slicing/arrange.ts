/** D19's **Arrange plate**: a deterministic skyline packer. It packs each
 *  instance's footprint (its axis-aligned extent after rotation and scale)
 *  with the given spacing between them, largest area first and then by
 *  `instanceKey`, into the largest axis-aligned rectangle of the bed that
 *  keeps clear of every exclude area, and centres the packed group in it.
 *  What doesn't fit is not placed: it stays where it was and is reported,
 *  never overlapped. Pure. */
import { EPSILON_MM, polygonsOverlap, polygonWithin, type Point2 } from "./bounds";
import { bedExtent, bedOutline } from "./viewport/build-volume";
import type { BuildVolume } from "./viewport/renderer";

/** D19's default spacing between arranged objects. */
export const DEFAULT_ARRANGE_SPACING_MM = 5;

export interface ArrangeItem {
  key: string;
  /** The footprint's extent relative to the instance's translation
   *  (`Footprint.min`/`max`). */
  min: readonly number[];
  max: readonly number[];
}

export interface Rect {
  minX: number;
  minY: number;
  maxX: number;
  maxY: number;
}

export interface ArrangeResult {
  /** The new `translateMm` of each placed item, by key. */
  placed: Map<string, [number, number]>;
  /** The keys that didn't fit, in key order. They keep their place. */
  unplaced: string[];
  /** Where the items were packed; `null` when the bed has no free area. */
  region: Rect | null;
}

/** Lines across the bed where a free rectangle may start or end: the bed's
 *  own corners, every exclude area's corners, and an even grid of this many
 *  steps. Exact for rectangular beds and exclude areas; within one step for
 *  other shapes. */
const GRID_STEPS = 48;

function gridLines(values: number[], low: number, high: number): number[] {
  const lines = [...values.filter((v) => v >= low && v <= high)];
  for (let i = 0; i <= GRID_STEPS; i += 1) lines.push(low + ((high - low) * i) / GRID_STEPS);
  lines.sort((a, b) => a - b);
  return lines.filter((v, i) => i === 0 || v - lines[i - 1] > EPSILON_MM);
}

/** The largest-area axis-aligned rectangle inside the bed outline that
 *  overlaps no exclude area, found over a grid of candidate lines. */
export function largestFreeRectangle(volume: BuildVolume): Rect | null {
  const extent = bedExtent(volume.bed);
  if (!extent) return null;
  const bed = bedOutline(volume.bed).map((p): Point2 => [p.xMm, p.yMm]);
  const excluded = volume.excludeAreas.filter((area) => area.length >= 3).map((area) => area.map((p): Point2 => [p.xMm, p.yMm]));
  const xs = gridLines([...bed, ...excluded.flat()].map(([x]) => x), extent.min[0], extent.max[0]);
  const ys = gridLines([...bed, ...excluded.flat()].map(([, y]) => y), extent.min[1], extent.max[1]);
  const columns = xs.length - 1;
  const rows = ys.length - 1;
  if (columns < 1 || rows < 1) return null;

  const free: boolean[][] = [];
  for (let row = 0; row < rows; row += 1) {
    free.push([]);
    for (let column = 0; column < columns; column += 1) {
      const cell: Point2[] = [[xs[column], ys[row]], [xs[column + 1], ys[row]], [xs[column + 1], ys[row + 1]], [xs[column], ys[row + 1]]];
      free[row].push(polygonWithin(cell, bed) && !excluded.some((area) => polygonsOverlap(cell, area)));
    }
  }

  let best: Rect | null = null;
  let bestArea = 0;
  for (let top = 0; top < rows; top += 1) {
    const open = new Array<boolean>(columns).fill(true);
    for (let bottom = top; bottom < rows; bottom += 1) {
      for (let column = 0; column < columns; column += 1) open[column] = open[column] && free[bottom][column];
      const depth = ys[bottom + 1] - ys[top];
      let start = -1;
      for (let column = 0; column <= columns; column += 1) {
        if (column < columns && open[column]) {
          if (start < 0) start = column;
          continue;
        }
        if (start >= 0) {
          const area = (xs[column] - xs[start]) * depth;
          if (area > bestArea + EPSILON_MM) {
            bestArea = area;
            best = { minX: xs[start], minY: ys[top], maxX: xs[column], maxY: ys[bottom + 1] };
          }
          start = -1;
        }
      }
    }
  }
  return best;
}

interface Segment {
  x: number;
  y: number;
  width: number;
}

/** Bottom-left skyline placement of a `width` × `depth` box in a
 *  container `containerWidth` × `containerDepth`: the lowest spot, then
 *  the leftmost. `null` when nothing fits. */
function findSpot(skyline: Segment[], width: number, depth: number, containerWidth: number, containerDepth: number) {
  let best: { x: number; y: number } | null = null;
  for (let i = 0; i < skyline.length; i += 1) {
    const x = skyline[i].x;
    if (x + width > containerWidth + EPSILON_MM) break;
    let y = 0;
    let reach = 0;
    for (let j = i; j < skyline.length && reach < width - EPSILON_MM; j += 1) {
      y = Math.max(y, skyline[j].y);
      reach = skyline[j].x + skyline[j].width - x;
    }
    if (y + depth > containerDepth + EPSILON_MM) continue;
    if (!best || y < best.y - EPSILON_MM || (Math.abs(y - best.y) <= EPSILON_MM && x < best.x)) best = { x, y };
  }
  return best;
}

function addToSkyline(skyline: Segment[], x: number, y: number, width: number): Segment[] {
  const end = x + width;
  const next: Segment[] = [];
  for (const segment of skyline) {
    const segmentEnd = segment.x + segment.width;
    if (segmentEnd <= x + EPSILON_MM || segment.x >= end - EPSILON_MM) {
      next.push(segment);
      continue;
    }
    if (segment.x < x) next.push({ x: segment.x, y: segment.y, width: x - segment.x });
    if (segmentEnd > end) next.push({ x: end, y: segment.y, width: segmentEnd - end });
  }
  next.push({ x, y, width });
  next.sort((a, b) => a.x - b.x);
  // Merge neighbours at the same height.
  const merged: Segment[] = [];
  for (const segment of next) {
    const last = merged[merged.length - 1];
    if (last && Math.abs(last.y - segment.y) <= EPSILON_MM && Math.abs(last.x + last.width - segment.x) <= EPSILON_MM) {
      last.width += segment.width;
    } else {
      merged.push({ ...segment });
    }
  }
  return merged;
}

const byKey = (a: string, b: string) => (a < b ? -1 : a > b ? 1 : 0);

/** Packs `items` onto the bed with `spacingMm` between them (D19). */
export function arrange(items: readonly ArrangeItem[], volume: BuildVolume, spacingMm = DEFAULT_ARRANGE_SPACING_MM): ArrangeResult {
  const spacing = Number.isFinite(spacingMm) && spacingMm > 0 ? spacingMm : 0;
  const region = largestFreeRectangle(volume);
  const size = (item: ArrangeItem) => [item.max[0] - item.min[0], item.max[1] - item.min[1]];
  const ordered = [...items].sort((a, b) => {
    const [aw, ad] = size(a);
    const [bw, bd] = size(b);
    return bw * bd - aw * ad || byKey(a.key, b.key);
  });
  const placed = new Map<string, [number, number]>();
  const unplaced: string[] = [];
  if (!region) return { placed, unplaced: ordered.map((item) => item.key).sort(byKey), region };

  // Each box grows by the spacing, and so does the container, so boxes end
  // up `spacing` apart and may sit flush with the region's edge.
  const containerWidth = region.maxX - region.minX + spacing;
  const containerDepth = region.maxY - region.minY + spacing;
  let skyline: Segment[] = [{ x: 0, y: 0, width: containerWidth }];
  const spots = new Map<string, { x: number; y: number; item: ArrangeItem }>();
  for (const item of ordered) {
    const [width, depth] = size(item);
    const spot = findSpot(skyline, width + spacing, depth + spacing, containerWidth, containerDepth);
    if (!spot) {
      unplaced.push(item.key);
      continue;
    }
    spots.set(item.key, { ...spot, item });
    skyline = addToSkyline(skyline, spot.x, spot.y + depth + spacing, width + spacing);
  }

  // Centre the packed group in the region.
  let usedMaxX = 0;
  let usedMaxY = 0;
  for (const { x, y, item } of spots.values()) {
    const [width, depth] = size(item);
    usedMaxX = Math.max(usedMaxX, x + width);
    usedMaxY = Math.max(usedMaxY, y + depth);
  }
  const offsetX = region.minX + (region.maxX - region.minX - usedMaxX) / 2;
  const offsetY = region.minY + (region.maxY - region.minY - usedMaxY) / 2;
  for (const [key, { x, y, item }] of spots) {
    placed.set(key, [offsetX + x - item.min[0], offsetY + y - item.min[1]]);
  }
  return { placed, unplaced: unplaced.sort(byKey), region };
}

