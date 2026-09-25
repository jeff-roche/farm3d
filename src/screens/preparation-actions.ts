/** D19's tools as actions on a {@link PreparationSession}, shared by the
 *  toolbar buttons, the keyboard commands (viewport and object list) and
 *  the numeric fields. Each edit goes through the session's editor, so it
 *  shows at once and saves after the pause. */
import { createSignal } from "solid-js";
import { arrange, DEFAULT_ARRANGE_SPACING_MM } from "../slicing/arrange";
import { layFlat, nextLayFlatFace } from "../slicing/layflat";
import {
  addPlate,
  deleteInstance,
  deletePlate,
  duplicateInstance,
  findInstance,
  moveInstanceToPlate,
  movePlate,
  plateLabel,
  renamePlate,
  setTransform,
  setTransforms,
} from "../slicing/preparation-edits";
import {
  clampScale,
  keepCentre,
  normalizeDegrees,
  rotateZBy,
  scaleBy,
  type Vec3,
} from "../slicing/transforms";
import type { InstanceDoc, InstanceTransform } from "../slicing/types";
import type { PreparationSession } from "./preparation-session";

/** D19's steps. */
export const MOVE_STEP_MM = 1;
export const MOVE_LARGE_STEP_MM = 10;
export const ROTATE_STEP_DEG = 15;
export const SCALE_STEP = 0.05;

export type Axis = 0 | 1 | 2;

export interface PreparationActions {
  /** Moves the selected instance on XY. */
  moveBy(dx: number, dy: number): void;
  setPosition(axis: 0 | 1, valueMm: number): void;
  rotateZ(degrees: number): void;
  setRotation(axis: Axis, degrees: number): void;
  /** ±5 % (D19), uniform, about the object's centre. */
  scaleStep(direction: 1 | -1): void;
  /** `factor` is a scale (1 = 100 %). With `uniform`, every axis follows
   *  in proportion. */
  setScale(axis: Axis, factor: number, uniform: boolean): void;
  /** **F**: the next lay-flat face, largest first. */
  layFlatNext(): void;
  layFlatOn(faceIndex: number): void;
  /** **A**: arranges the shown plate. */
  arrangePlate(): void;
  arrangeSpacing(): number;
  setArrangeSpacing(spacingMm: number): void;
  /** **Shift+1…9** and the plate `Select`. */
  moveToPlate(plateKey: string): void;
  moveToPlateAt(position: number): void;
  duplicate(): void;
  remove(): void;
  addPlate(): void;
  renamePlate(plateKey: string, name: string): void;
  movePlate(plateKey: string, step: -1 | 1): void;
  deletePlate(plateKey: string): void;
  /** The last thing a tool did, for the live region. */
  status(): string;
}

function newKey(): string {
  return crypto.randomUUID();
}

export function createPreparationActions(session: PreparationSession): PreparationActions {
  const [status, setStatus] = createSignal("");
  const [spacing, setSpacing] = createSignal(DEFAULT_ARRANGE_SPACING_MM);
  // Which lay-flat face F applies next, per instance. Tool state only; the
  // result lives in the document.
  const lastFace = new Map<string, number>();

  const selected = (): InstanceDoc | undefined => {
    const key = session.selectedInstanceKey();
    const document = session.document();
    return key && document ? findInstance(document, key)?.instance : undefined;
  };

  /** The object's local bounds centre: turns and scales happen about it. */
  const centreOf = (instance: InstanceDoc): Vec3 => {
    const bounds = session.object(instance.objectKey)?.boundsMm;
    return bounds ? [0, 1, 2].map((axis) => (bounds.min[axis] + bounds.max[axis]) / 2) as Vec3 : [0, 0, 0];
  };

  const editSelected = (change: (transform: InstanceTransform, instance: InstanceDoc) => InstanceTransform, aboutCentre = false) => {
    const instance = selected();
    if (!instance) return;
    const next = change(instance.transform, instance);
    const placed = aboutCentre ? keepCentre(instance.transform, next, centreOf(instance)) : next;
    session.editor.edit((document) => setTransform(document, instance.instanceKey, placed));
  };

  const name = (instance: InstanceDoc) => session.instanceName(instance.instanceKey);

  const layFlatOn = (faceIndex: number) => {
    const instance = selected();
    const face = instance && session.object(instance.objectKey)?.layFlatFaces[faceIndex];
    if (!instance || !face) return;
    lastFace.set(instance.instanceKey, faceIndex);
    editSelected((transform) => layFlat(transform, face), true);
    setStatus(`${name(instance)} lies flat on face ${faceIndex + 1}.`);
  };

  const moveToPlate = (plateKey: string) => {
    const instance = selected();
    const document = session.document();
    if (!instance || !document) return;
    const at = document.plates.findIndex((plate) => plate.plateKey === plateKey);
    if (at < 0 || findInstance(document, instance.instanceKey)?.plate.plateKey === plateKey) return;
    session.editor.edit((current) => moveInstanceToPlate(current, instance.instanceKey, plateKey));
    setStatus(`Moved ${name(instance)} to ${plateLabel(document.plates[at], at)}.`);
  };

  return {
    moveBy: (dx, dy) => editSelected((transform) => ({
      ...transform,
      translateMm: [transform.translateMm[0] + dx, transform.translateMm[1] + dy],
    })),
    setPosition: (axis, value) => editSelected((transform) => {
      const translateMm: [number, number] = [...transform.translateMm];
      translateMm[axis] = value;
      return { ...transform, translateMm };
    }),
    rotateZ: (degrees) => editSelected((transform) => rotateZBy(transform, degrees), true),
    setRotation: (axis, degrees) => editSelected((transform) => {
      const rotateDeg: [number, number, number] = [...transform.rotateDeg];
      rotateDeg[axis] = normalizeDegrees(degrees);
      return { ...transform, rotateDeg };
    }, true),
    scaleStep: (direction) => editSelected((transform) => scaleBy(transform, 1 + direction * SCALE_STEP), true),
    setScale: (axis, factor, uniform) => editSelected((transform) => {
      const target = clampScale(factor);
      if (uniform) return scaleBy(transform, target / transform.scale[axis]);
      const scale: [number, number, number] = [...transform.scale];
      scale[axis] = target;
      return { ...transform, scale };
    }, true),
    layFlatNext: () => {
      const instance = selected();
      const count = instance ? session.object(instance.objectKey)?.layFlatFaces.length ?? 0 : 0;
      const next = instance && nextLayFlatFace(lastFace.get(instance.instanceKey), count);
      if (next !== undefined) layFlatOn(next);
    },
    layFlatOn,
    arrangePlate: () => {
      const document = session.document();
      const volume = session.volume();
      const plate = document?.plates.find((candidate) => candidate.plateKey === session.plateKey());
      if (!plate || !volume) return;
      const items = plate.instances.flatMap((instance) => {
        const footprint = session.footprint(instance);
        return footprint ? [{ key: instance.instanceKey, min: footprint.min, max: footprint.max }] : [];
      });
      const result = arrange(items, volume, spacing());
      const transforms = new Map<string, InstanceTransform>();
      for (const instance of plate.instances) {
        const placed = result.placed.get(instance.instanceKey);
        if (placed) transforms.set(instance.instanceKey, { ...instance.transform, translateMm: placed });
      }
      session.editor.edit((current) => setTransforms(current, transforms));
      const left = plate.instances.filter((instance) => result.unplaced.includes(instance.instanceKey)).map(name);
      setStatus(left.length > 0
        ? `Arranged ${result.placed.size} of ${items.length} objects. Didn't fit, left where they were: ${left.join(", ")}.`
        : `Arranged ${result.placed.size} ${result.placed.size === 1 ? "object" : "objects"}.`);
    },
    arrangeSpacing: spacing,
    setArrangeSpacing: (value) => setSpacing(Number.isFinite(value) && value >= 0 ? value : DEFAULT_ARRANGE_SPACING_MM),
    moveToPlate,
    moveToPlateAt: (position) => {
      const plate = session.document()?.plates[position - 1];
      if (plate && plate.plateKey !== session.plateKey()) moveToPlate(plate.plateKey);
    },
    duplicate: () => {
      const instance = selected();
      if (!instance) return;
      const footprint = session.footprint(instance);
      const offset: [number, number] = [footprint ? footprint.max[0] - footprint.min[0] + DEFAULT_ARRANGE_SPACING_MM : 10, 0];
      const key = newKey();
      session.editor.edit((document) => duplicateInstance(document, instance.instanceKey, key, offset));
      session.select(key);
      setStatus(`Duplicated ${name(instance)}.`);
    },
    remove: () => {
      const instance = selected();
      if (!instance) return;
      session.editor.edit((document) => deleteInstance(document, instance.instanceKey));
      session.select(null);
      setStatus(`Removed ${name(instance)} from the plate.`);
    },
    addPlate: () => {
      const key = newKey();
      session.editor.edit((document) => addPlate(document, key));
      session.selectPlate(key);
    },
    renamePlate: (plateKey, text) => session.editor.edit((document) => renamePlate(document, plateKey, text)),
    movePlate: (plateKey, step) => session.editor.edit((document) => movePlate(document, plateKey, step)),
    deletePlate: (plateKey) => session.editor.edit((document) => deletePlate(document, plateKey)),
    status,
  };
}
