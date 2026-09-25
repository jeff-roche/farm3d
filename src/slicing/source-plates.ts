/** A Model Source Revision's own layout, by plate, for the read-only
 *  inspector in Model details (D21). It groups build items the way D5
 *  seeding does: one plate per Orca/Bambu plate in index order (items with
 *  no plate go on the first), else one plate for every printable item.
 *  Each plate's items keep their relative XY layout, as D5 instance
 *  transforms, and the group is centred on the origin, so a plate's
 *  position on OrcaSlicer's plate grid never shows. Unprintable items are
 *  skipped and listed. */
import {
  composeTransform,
  instanceTransformFrom3mf,
  transformedBounds,
  type Positions,
} from "./transforms";
import type { RevisionGeometry } from "./types";
import type { ViewportInstance } from "./viewport/plate-view";

export interface SourcePlate {
  index: number;
  name: string;
  instances: ViewportInstance[];
}

export interface SourceLayout {
  plates: SourcePlate[];
  /** The names of the build items marked not printable, in file order. */
  unprintable: string[];
}

export function objectName(geometry: RevisionGeometry, objectKey: number): string {
  return geometry.objects.find((object) => object.objectKey === objectKey)?.name ?? `Object ${objectKey}`;
}

/** `plates` is the 3MF's plate list (`ThreeMfInspection.plates`), empty for
 *  an STL or a 3MF without plates. `positions` answers each object's mesh
 *  vertices, for the centring. */
export function sourceLayout(
  geometry: RevisionGeometry,
  plates: { index: number; name?: string }[],
  positions: (objectKey: number) => Positions | undefined,
): SourceLayout {
  const known = new Set(geometry.objects.map((object) => object.objectKey));
  const groups = new Map<number, SourcePlate>();
  const plate = (index: number, name?: string) => {
    let group = groups.get(index);
    if (!group) {
      group = { index, name: name ?? `${index}`, instances: [] };
      groups.set(index, group);
    }
    return group;
  };
  for (const entry of plates) plate(entry.index, entry.name);
  const usesPlates = groups.size > 0 || geometry.buildItems.some((item) => item.plateIndex !== undefined);
  const firstPlate = Math.min(...[...groups.keys(), Infinity]);

  const unprintable: string[] = [];
  geometry.buildItems.forEach((item, i) => {
    if (!known.has(item.objectKey)) return;
    if (!item.printable) {
      unprintable.push(objectName(geometry, item.objectKey));
      return;
    }
    const index = usesPlates ? item.plateIndex ?? (firstPlate === Infinity ? 1 : firstPlate) : 1;
    plate(index).instances.push({
      instanceKey: `item-${i + 1}`,
      objectKey: item.objectKey,
      name: objectName(geometry, item.objectKey),
      transform: instanceTransformFrom3mf(item.transform),
    });
  });

  const ordered = [...groups.values()].sort((a, b) => a.index - b.index);
  for (const group of ordered) centreOnOrigin(group.instances, positions);
  return { plates: ordered.length > 0 ? ordered : [{ index: 1, name: "1", instances: [] }], unprintable };
}

function centreOnOrigin(instances: ViewportInstance[], positions: (objectKey: number) => Positions | undefined) {
  let min = [Infinity, Infinity];
  let max = [-Infinity, -Infinity];
  for (const instance of instances) {
    const vertices = positions(instance.objectKey);
    if (!vertices) continue;
    const bounds = transformedBounds(composeTransform(instance.transform, vertices), vertices);
    if (!bounds) continue;
    min = [Math.min(min[0], bounds.min[0]), Math.min(min[1], bounds.min[1])];
    max = [Math.max(max[0], bounds.max[0]), Math.max(max[1], bounds.max[1])];
  }
  if (min[0] === Infinity) return;
  const shift = [-(min[0] + max[0]) / 2, -(min[1] + max[1]) / 2];
  for (const instance of instances) {
    const [x, y] = instance.transform.translateMm;
    instance.transform = { ...instance.transform, translateMm: [x + shift[0], y + shift[1]] };
  }
}
