/** The build volume from a Printer profile (D18): the bed shape, the
 *  printable height, and the bed exclude area. */
import type { BedShape, PointMm } from "../types";
import type { BuildVolume } from "./renderer";

/** What a profile (a Printer's or a Slice Revision's snapshot) states
 *  about its printable space. */
export interface VolumeSource {
  bedShape: BedShape;
  printableHeightMm: number;
  /** OrcaSlicer's `bed_exclude_area`: one polygon, empty when none. */
  bedExcludeAreas: PointMm[];
}

export function buildVolumeFromProfile(profile: VolumeSource): BuildVolume {
  return {
    bed: profile.bedShape,
    heightMm: profile.printableHeightMm,
    excludeAreas: profile.bedExcludeAreas.length >= 3 ? [profile.bedExcludeAreas] : [],
  };
}

/** The bed's outline, counter-clockwise for a rectangle. */
export function bedOutline(bed: BedShape): PointMm[] {
  if (bed.kind === "polygon") return bed.points;
  const x0 = bed.originXMm;
  const y0 = bed.originYMm;
  const x1 = x0 + bed.widthMm;
  const y1 = y0 + bed.depthMm;
  return [{ xMm: x0, yMm: y0 }, { xMm: x1, yMm: y0 }, { xMm: x1, yMm: y1 }, { xMm: x0, yMm: y1 }];
}

/** The outline's axis-aligned extent, or `null` for a polygon of fewer
 *  than three points (which has no area). */
export function bedExtent(bed: BedShape): { min: [number, number]; max: [number, number] } | null {
  const points = bedOutline(bed);
  if (points.length < 3) return null;
  const xs = points.map((point) => point.xMm);
  const ys = points.map((point) => point.yMm);
  return { min: [Math.min(...xs), Math.min(...ys)], max: [Math.max(...xs), Math.max(...ys)] };
}
