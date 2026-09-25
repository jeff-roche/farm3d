/** D19's **Arrange plate**: a deterministic skyline packer. It packs each
 *  instance's footprint (its axis-aligned extent after rotation and scale)
 *  with the given spacing between them, largest area first and then by
 *  `instanceKey`, into the largest axis-aligned rectangle of the bed that
 *  keeps clear of every exclude area, and centres the packed group in it.
 *  What doesn't fit, or has an extent that isn't a finite number, is not
 *  placed: it stays where it was and is reported, never overlapped. Pure. */
import { EPSILON_MM, pointInPolygon, polygonsOverlap, type Point2 } from "./bounds";
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

/** Lines across the bed where a free rectangle may start or end: an even
 *  grid of this many steps, plus the corners of the bed and of each
 *  exclude area drawn with few enough points to be a real corner (see
 *  {@link MAX_CORNER_POINTS}). Exact for rectangular beds and exclude
 *  areas; within one step for other shapes. */
const GRID_STEPS = 48;

/** A shape with more points than this is a curve drawn in segments (a
 *  round bed), not corners: only its extent adds lines, since a line per
 *  point would make the search slow and gain less than a grid step. */
const MAX_CORNER_POINTS = 16;

const cornerPoints = (shape: Point2[]): Point2[] => {
  if (shape.length <= MAX_CORNER_POINTS) return shape;
  const xs = shape.map(([x]) => x);
  const ys = shape.map(([, y]) => y);
  return [[Math.min(...xs), Math.min(...ys)], [Math.max(...xs), Math.max(...ys)]];
};

function gridLines(values: number[], low: number, high: number): number[] {
  const lines = [...values.filter((v) => v >= low && v <= high)];
  for (let i = 0; i <= GRID_STEPS; i += 1) lines.push(low + ((high - low) * i) / GRID_STEPS);
  lines.sort((a, b) => a - b);
  return lines.filter((v, i) => i === 0 || v - lines[i - 1] > EPSILON_MM);
}

/** Whether a segment passes through the open rectangle (touching its
 *  sides doesn't count), by Liang–Barsky clipping against the rectangle
 *  shrunk by the tolerance. */
function crossesInterior(a: Point2, b: Point2, minX: number, minY: number, maxX: number, maxY: number): boolean {
  const x0 = minX + EPSILON_MM, y0 = minY + EPSILON_MM, x1 = maxX - EPSILON_MM, y1 = maxY - EPSILON_MM;
  if (x0 >= x1 || y0 >= y1) return false;
  const dx = b[0] - a[0];
  const dy = b[1] - a[1];
  let low = 0;
  let high = 1;
  for (const [p, q] of [[-dx, a[0] - x0], [dx, x1 - a[0]], [-dy, a[1] - y0], [dy, y1 - a[1]]]) {
    if (p === 0) {
      if (q < 0) return false;
      continue;
    }
    const t = q / p;
    if (p < 0) low = Math.max(low, t);
    else high = Math.min(high, t);
    if (low > high) return false;
  }
  return true;
}

/** The index of the last line at or below `value`. */
function lineBelow(lines: number[], value: number): number {
  let low = 0;
  let high = lines.length - 1;
  while (low < high) {
    const middle = (low + high + 1) >> 1;
    if (lines[middle] <= value) low = middle;
    else high = middle - 1;
  }
  return low;
}

/** Which grid cells lie wholly inside the bed and clear of every exclude
 *  area. A cell is inside when its corners and centre are, and no bed edge
 *  passes through it: each grid point is tested once, and each bed edge
 *  only against the cells its extent covers, so a finely drawn round bed
 *  stays cheap. */
function freeCells(xs: number[], ys: number[], bed: Point2[], excluded: Point2[][]): boolean[][] {
  const columns = xs.length - 1;
  const rows = ys.length - 1;
  const cornerInside = ys.map((y) => xs.map((x) => pointInPolygon([x, y], bed) !== "outside"));
  const crossed = Array.from({ length: rows }, () => new Array<boolean>(columns).fill(false));
  for (let i = 0; i < bed.length; i += 1) {
    const a = bed[i];
    const b = bed[(i + 1) % bed.length];
    const fromColumn = Math.min(lineBelow(xs, Math.min(a[0], b[0])), columns - 1);
    const toColumn = Math.min(lineBelow(xs, Math.max(a[0], b[0])), columns - 1);
    const fromRow = Math.min(lineBelow(ys, Math.min(a[1], b[1])), rows - 1);
    const toRow = Math.min(lineBelow(ys, Math.max(a[1], b[1])), rows - 1);
    for (let row = fromRow; row <= toRow; row += 1) {
      for (let column = fromColumn; column <= toColumn; column += 1) {
        if (!crossed[row][column] && crossesInterior(a, b, xs[column], ys[row], xs[column + 1], ys[row + 1])) {
          crossed[row][column] = true;
        }
      }
    }
  }
  const free: boolean[][] = [];
  for (let row = 0; row < rows; row += 1) {
    free.push([]);
    for (let column = 0; column < columns; column += 1) {
      const inside = !crossed[row][column]
        && cornerInside[row][column] && cornerInside[row][column + 1]
        && cornerInside[row + 1][column] && cornerInside[row + 1][column + 1]
        && pointInPolygon([(xs[column] + xs[column + 1]) / 2, (ys[row] + ys[row + 1]) / 2], bed) === "inside";
      if (!inside) {
        free[row].push(false);
        continue;
      }
      const cell: Point2[] = [[xs[column], ys[row]], [xs[column + 1], ys[row]], [xs[column + 1], ys[row + 1]], [xs[column], ys[row + 1]]];
      free[row].push(!excluded.some((area) => polygonsOverlap(cell, area)));
    }
  }
  return free;
}

/** The largest-area axis-aligned rectangle inside the bed outline that
 *  overlaps no exclude area, found over a grid of candidate lines. */
export function largestFreeRectangle(volume: BuildVolume): Rect | null {
  const extent = bedExtent(volume.bed);
  if (!extent) return null;
  const bed = bedOutline(volume.bed).map((p): Point2 => [p.xMm, p.yMm]);
  const excluded = volume.excludeAreas.filter((area) => area.length >= 3).map((area) => area.map((p): Point2 => [p.xMm, p.yMm]));
  const corners = [bed, ...excluded].flatMap(cornerPoints);
  const xs = gridLines(corners.map(([x]) => x), extent.min[0], extent.max[0]);
  const ys = gridLines(corners.map(([, y]) => y), extent.min[1], extent.max[1]);
  const columns = xs.length - 1;
  const rows = ys.length - 1;
  if (columns < 1 || rows < 1) return null;

  const free = freeCells(xs, ys, bed, excluded);
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
  // An extent that isn't a number can't be packed; it is reported instead.
  const finite = (item: ArrangeItem) => [item.min[0], item.min[1], item.max[0], item.max[1]].every(Number.isFinite);
  const ordered = items.filter(finite).sort((a, b) => {
    const [aw, ad] = size(a);
    const [bw, bd] = size(b);
    return bw * bd - aw * ad || byKey(a.key, b.key);
  });
  const placed = new Map<string, [number, number]>();
  const unplaced: string[] = items.filter((item) => !finite(item)).map((item) => item.key);
  if (!region) return { placed, unplaced: items.map((item) => item.key).sort(byKey), region };

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

