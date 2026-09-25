/** D19's edits to a Preparation document, as pure functions: each returns
 *  a new document (sharing what didn't change) or the same one when there
 *  is nothing to do, so a no-op never schedules a save. */
import type { InstanceDoc, InstanceTransform, PlateDoc, PreparationDocument } from "./types";

/** OrcaSlicer's `MAX_PLATE_COUNT` (D5). */
export const MAX_PLATES = 36;
/** The backend's limit on a plate name. */
export const MAX_PLATE_NAME_CHARS = 128;

/** A plate's name as shown: its own, else its position. */
export function plateLabel(plate: PlateDoc, index: number): string {
  return plate.name?.trim() || `Plate ${index + 1}`;
}

export interface InstanceLocation {
  plate: PlateDoc;
  plateIndex: number;
  instance: InstanceDoc;
}

export function findInstance(document: PreparationDocument, instanceKey: string): InstanceLocation | undefined {
  for (const [plateIndex, plate] of document.plates.entries()) {
    const instance = plate.instances.find((candidate) => candidate.instanceKey === instanceKey);
    if (instance) return { plate, plateIndex, instance };
  }
  return undefined;
}

function mapPlates(document: PreparationDocument, change: (plate: PlateDoc) => PlateDoc): PreparationDocument {
  let changed = false;
  const plates = document.plates.map((plate) => {
    const next = change(plate);
    if (next !== plate) changed = true;
    return next;
  });
  return changed ? { ...document, plates } : document;
}

const sameTransform = (a: InstanceTransform, b: InstanceTransform) =>
  a.translateMm.every((v, i) => v === b.translateMm[i])
  && a.rotateDeg.every((v, i) => v === b.rotateDeg[i])
  && a.scale.every((v, i) => v === b.scale[i]);

/** Sets the transforms of the instances named in `transforms`. */
export function setTransforms(document: PreparationDocument, transforms: ReadonlyMap<string, InstanceTransform>): PreparationDocument {
  return mapPlates(document, (plate) => {
    let changed = false;
    const instances = plate.instances.map((instance) => {
      const next = transforms.get(instance.instanceKey);
      if (!next || sameTransform(next, instance.transform)) return instance;
      changed = true;
      return { ...instance, transform: next };
    });
    return changed ? { ...plate, instances } : plate;
  });
}

export function setTransform(document: PreparationDocument, instanceKey: string, transform: InstanceTransform): PreparationDocument {
  return setTransforms(document, new Map([[instanceKey, transform]]));
}

/** Adds an empty plate at the end; no-op at {@link MAX_PLATES}. */
export function addPlate(document: PreparationDocument, plateKey: string): PreparationDocument {
  if (document.plates.length >= MAX_PLATES) return document;
  return { ...document, plates: [...document.plates, { plateKey, instances: [] }] };
}

/** A blank name clears it (the plate shows its position instead). A long
 *  one is cut at {@link MAX_PLATE_NAME_CHARS} code points, as the backend
 *  counts them, so an emoji is never split in half. */
export function renamePlate(document: PreparationDocument, plateKey: string, name: string): PreparationDocument {
  const trimmed = Array.from(name.trim()).slice(0, MAX_PLATE_NAME_CHARS).join("");
  return mapPlates(document, (plate) => {
    if (plate.plateKey !== plateKey || (plate.name ?? "") === trimmed) return plate;
    const { name: _old, ...rest } = plate;
    return trimmed ? { ...rest, name: trimmed } : rest;
  });
}

/** Moves a plate one place left (-1) or right (1). */
export function movePlate(document: PreparationDocument, plateKey: string, step: -1 | 1): PreparationDocument {
  const at = document.plates.findIndex((plate) => plate.plateKey === plateKey);
  const to = at + step;
  if (at < 0 || to < 0 || to >= document.plates.length) return document;
  const plates = [...document.plates];
  [plates[at], plates[to]] = [plates[to], plates[at]];
  return { ...document, plates };
}

/** Deletes a plate and its instances; the last plate can't be deleted. */
export function deletePlate(document: PreparationDocument, plateKey: string): PreparationDocument {
  if (document.plates.length <= 1 || !document.plates.some((plate) => plate.plateKey === plateKey)) return document;
  return { ...document, plates: document.plates.filter((plate) => plate.plateKey !== plateKey) };
}

/** **Move to plate**: the instance goes to the end of the other plate,
 *  keeping its transform. */
export function moveInstanceToPlate(document: PreparationDocument, instanceKey: string, plateKey: string): PreparationDocument {
  const found = findInstance(document, instanceKey);
  if (!found || found.plate.plateKey === plateKey || !document.plates.some((plate) => plate.plateKey === plateKey)) {
    return document;
  }
  return mapPlates(document, (plate) => {
    if (plate === found.plate) return { ...plate, instances: plate.instances.filter((i) => i !== found.instance) };
    if (plate.plateKey === plateKey) return { ...plate, instances: [...plate.instances, found.instance] };
    return plate;
  });
}

/** **Duplicate**: a copy under `newInstanceKey`, right after the original
 *  on its plate, moved by `offsetMm`. */
export function duplicateInstance(
  document: PreparationDocument,
  instanceKey: string,
  newInstanceKey: string,
  offsetMm: readonly [number, number],
): PreparationDocument {
  const found = findInstance(document, instanceKey);
  if (!found) return document;
  const [x, y] = found.instance.transform.translateMm;
  const copy: InstanceDoc = {
    ...found.instance,
    instanceKey: newInstanceKey,
    transform: { ...found.instance.transform, translateMm: [x + offsetMm[0], y + offsetMm[1]] },
  };
  return mapPlates(document, (plate) => {
    if (plate !== found.plate) return plate;
    const instances = [...plate.instances];
    instances.splice(instances.indexOf(found.instance) + 1, 0, copy);
    return { ...plate, instances };
  });
}

export function deleteInstance(document: PreparationDocument, instanceKey: string): PreparationDocument {
  const found = findInstance(document, instanceKey);
  if (!found) return document;
  return mapPlates(document, (plate) => (
    plate === found.plate ? { ...plate, instances: plate.instances.filter((i) => i !== found.instance) } : plate
  ));
}
