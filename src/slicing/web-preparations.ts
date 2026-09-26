/** Web mode's local Preparation edits (spec D23): the backend's document
 *  checks that need no geometry, and a simplified D5 seed. Loaded only in
 *  web mode, so none of it is in the desktop bundle's main chunk. */
import type { ModelRecord } from "../library/types";
import { notFound, validationError } from "../ipc/local-errors";
import type { PreparationDocument, PreparationRecord, SliceTarget } from "./types";
import type { WebSlicingFixture } from "./web-fixtures";

const MAX_PLATES = 36;

/** The backend's document rules that need no geometry: 1–36 plates and
 *  unique, non-empty plate and instance keys. */
export function validateLocalDocument(document: PreparationDocument): void {
  if (document.plates.length < 1 || document.plates.length > MAX_PLATES) {
    throw validationError("document.plates", `A Preparation has 1 to ${MAX_PLATES} plates.`);
  }
  const plateKeys = new Set<string>();
  const instanceKeys = new Set<string>();
  document.plates.forEach((plate, p) => {
    if (!plate.plateKey.trim() || plateKeys.has(plate.plateKey)) {
      throw validationError(`document.plates[${p}].plateKey`, "Each plate needs its own key.");
    }
    plateKeys.add(plate.plateKey);
    plate.instances.forEach((instance, i) => {
      if (!instance.instanceKey.trim() || instanceKeys.has(instance.instanceKey)) {
        throw validationError(`document.plates[${p}].instances[${i}].instanceKey`, "Each object on a plate needs its own key.");
      }
      instanceKeys.add(instance.instanceKey);
    });
  });
}

/** A simplified D5 seed: one plate per source plate (or one plate), each
 *  printable build item once at the bed's centre, and the default
 *  presets. */
export function seedLocalPreparation(
  fixture: WebSlicingFixture,
  model: Pick<ModelRecord, "id" | "format" | "currentRevision">,
  target?: SliceTarget,
): PreparationRecord {
  if (model.format === "gcode") {
    throw validationError("modelId", "A G-code Model is already sliced, so it has no Preparation.");
  }
  const geometry = fixture.geometry[model.currentRevision.id];
  if (!geometry) throw notFound(model.currentRevision.id);
  const options = fixture.sliceOptions;
  const bed = options.profileSnapshot.bedShape;
  const center: [number, number] = bed.kind === "rectangular"
    ? [bed.originXMm + bed.widthMm / 2, bed.originYMm + bed.depthMm / 2]
    : [0, 0];
  const plates = new Map<number, PreparationDocument["plates"][number]>();
  for (const item of geometry.buildItems.filter((i) => i.printable)) {
    const index = item.plateIndex ?? 1;
    const plate = plates.get(index) ?? { plateKey: crypto.randomUUID(), instances: [] };
    plate.instances.push({
      instanceKey: crypto.randomUUID(),
      objectKey: item.objectKey,
      transform: { translateMm: center, rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
    });
    plates.set(index, plate);
  }
  const now = new Date().toISOString();
  return {
    id: `prp-web-${crypto.randomUUID()}`,
    modelId: model.id,
    sourceRevisionId: model.currentRevision.id,
    revision: 1,
    stale: false,
    document: {
      plates: [...plates.entries()].sort(([a], [b]) => a - b).map(([, plate]) => plate),
      target: target ?? { kind: "profile", catalogRef: { ...options.profileSnapshot.catalogRef } },
      ...(options.defaults.processPreset ? { processPreset: options.defaults.processPreset } : {}),
      ...(options.defaults.filamentPreset ? { filamentPreset: options.defaults.filamentPreset } : {}),
      controls: {},
    },
    createdAt: now,
    updatedAt: now,
  };
}
