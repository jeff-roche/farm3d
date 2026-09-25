/** Everything the Preparation workspace (and Task 13's panel beside it)
 *  shares while a Model is being prepared: the store's record with the
 *  optimistic editor on top, the source revision's geometry and meshes,
 *  the target's slice options and build volume, each instance's footprint,
 *  the live validation, and the plate and object selection. */
import { batch, createEffect, createMemo, createSignal, on, onCleanup, untrack } from "solid-js";
import type { ModelRecord } from "../library/types";
import { footprintOf, type Footprint } from "../slicing/bounds";
import { createGeometryCache } from "../slicing/geometry-cache";
import type { MeshBuffer } from "../slicing/mesh-buffer";
import { createPreparationEditor, type PreparationEditor } from "../slicing/preparation-editor";
import {
  listSliceOptions,
  loadGeometry,
  loadMesh,
  slicing,
  updatePreparation,
} from "../slicing/slicing-store";
import type {
  GeometryObject,
  InstanceDoc,
  PreparationDocument,
  PreparationRecord,
  RevisionGeometry,
  SliceOptions,
} from "../slicing/types";
import { validatePreparation, type PreparationValidation } from "../slicing/validation";
import { buildVolumeFromProfile } from "../slicing/viewport/build-volume";
import type { BuildVolume } from "../slicing/viewport/renderer";

export interface PreparationSession {
  model: () => ModelRecord;
  /** The store's record (server-confirmed). */
  record: () => PreparationRecord | undefined;
  /** Edits go through here; see `preparation-editor.ts`. */
  editor: PreparationEditor;
  /** What is shown: the record plus any unsaved edits. */
  document: () => PreparationDocument | undefined;
  /** The pinned source revision's objects and build items. */
  geometry: () => RevisionGeometry | undefined;
  /** The geometry or a mesh failed to load. */
  loadFailed: () => boolean;
  /** The meshes of the objects on any plate, by object key. */
  meshes: () => Map<number, MeshBuffer>;
  /** `list_slice_options` for the document's target. */
  options: () => SliceOptions | undefined;
  /** The target's printable space; `null` until the options load. */
  volume: () => BuildVolume | null;
  footprint: (instance: InstanceDoc) => Footprint | undefined;
  validation: () => PreparationValidation;
  object: (objectKey: number) => GeometryObject | undefined;
  objectName: (objectKey: number) => string;
  /** The plate tab shown; the first plate when the chosen one is gone. */
  plateKey: () => string | undefined;
  selectPlate: (plateKey: string) => void;
  selectedInstanceKey: () => string | null;
  select: (instanceKey: string | null) => void;
  /** **Continue with revision M**, as held in the store for `start_slice`. */
  continueWithSourceRevision: () => string | undefined;
}

/** How many footprints stay cached (one per object, rotation and scale). */
const FOOTPRINT_CACHE_SIZE = 256;

export function createPreparationSession(model: () => ModelRecord): PreparationSession {
  const record = () => slicing.preparation(model().id);
  const editor = createPreparationEditor({ preparation: record, save: updatePreparation });
  onCleanup(() => editor.dispose());

  // D18/State: dropped when leaving preparation mode.
  const cache = createGeometryCache({ loadGeometry, loadMesh });
  onCleanup(() => cache.clear());

  const [geometry, setGeometry] = createSignal<RevisionGeometry | undefined>();
  const [meshes, setMeshes] = createSignal<Map<number, MeshBuffer>>(new Map());
  const [loadFailed, setLoadFailed] = createSignal(false);
  const revisionId = createMemo(() => record()?.sourceRevisionId);

  // Loaded without resources, so nothing here suspends the lazy boundary
  // the workspace sits in; each load checks it is still wanted.
  createEffect(on(revisionId, (id) => {
    batch(() => {
      setGeometry(undefined);
      setMeshes(new Map());
      setLoadFailed(false);
    });
    if (!id) return;
    cache.geometry(id).then(
      (loaded) => { if (untrack(revisionId) === id) setGeometry(loaded); },
      () => { if (untrack(revisionId) === id) setLoadFailed(true); },
    );
  }));

  const document = () => editor.document();
  const placedObjectKeys = createMemo(
    () => [...new Set((document()?.plates ?? []).flatMap((plate) => plate.instances.map((i) => i.objectKey)))].sort((a, b) => a - b),
    [],
    { equals: (a, b) => a.length === b.length && a.every((key, i) => key === b[i]) },
  );
  createEffect(on([revisionId, geometry, placedObjectKeys], ([id, loaded, keys]) => {
    if (!id || !loaded) return;
    const known = new Set(loaded.objects.map((object) => object.objectKey));
    const missing = keys.filter((key) => known.has(key) && !untrack(meshes).has(key));
    if (missing.length === 0) return;
    Promise.all(missing.map(async (key) => [key, await cache.mesh(id, key)] as const)).then(
      (entries) => {
        if (untrack(revisionId) !== id) return;
        setMeshes((held) => new Map([...held, ...entries]));
      },
      () => { if (untrack(revisionId) === id) setLoadFailed(true); },
    );
  }));

  const [options, setOptions] = createSignal<SliceOptions | undefined>();
  const targetKey = createMemo(() => {
    const target = document()?.target;
    return target ? JSON.stringify(target) : undefined;
  });
  createEffect(on(targetKey, (key) => {
    setOptions(undefined);
    if (!key) return;
    const target = untrack(document)!.target;
    listSliceOptions(target).then(
      (loaded) => { if (untrack(targetKey) === key) setOptions(loaded); },
      // No options (e.g. no slicer runtime): no volume to check against;
      // Task 13's panel reports why.
      () => {},
    );
  }));
  const volume = createMemo<BuildVolume | null>(() => {
    const loaded = options();
    return loaded ? buildVolumeFromProfile(loaded.profileSnapshot) : null;
  });

  // A footprint scans every vertex, so it is kept per mesh, rotation and
  // scale: a move reuses it, and instances that share all three share it.
  const footprints = new Map<string, { mesh: MeshBuffer; footprint: Footprint | null }>();
  const footprint = (instance: InstanceDoc): Footprint | undefined => {
    const mesh = meshes().get(instance.objectKey);
    if (!mesh) return undefined;
    const { rotateDeg, scale } = instance.transform;
    const key = `${instance.objectKey}|${rotateDeg.join(",")}|${scale.join(",")}`;
    const held = footprints.get(key);
    if (held && held.mesh === mesh) return held.footprint ?? undefined;
    const computed = footprintOf(instance.transform, mesh.positions);
    footprints.delete(key);
    footprints.set(key, { mesh, footprint: computed });
    if (footprints.size > FOOTPRINT_CACHE_SIZE) footprints.delete(footprints.keys().next().value!);
    return computed ?? undefined;
  };

  const continueWithSourceRevision = () => {
    const held = record();
    return held ? slicing.continueWithSourceRevision(held.id) : undefined;
  };

  const validation = createMemo((): PreparationValidation => {
    const shown = document();
    if (!shown) return { issues: [], placement: new Map() };
    return validatePreparation({
      document: shown,
      footprint,
      volume: volume(),
      options: options() ?? null,
      runtime: slicing.runtime(),
      stale: (record()?.stale ?? false) && !continueWithSourceRevision(),
    });
  });

  const object = (objectKey: number) => geometry()?.objects.find((candidate) => candidate.objectKey === objectKey);

  const [chosenPlate, setChosenPlate] = createSignal<string | undefined>();
  const plateKey = createMemo(() => {
    const plates = document()?.plates ?? [];
    const chosen = chosenPlate();
    return plates.some((plate) => plate.plateKey === chosen) ? chosen : plates[0]?.plateKey;
  });
  // Hold on to the plate shown by default, so reordering the plates
  // doesn't swap the tab under the user.
  createEffect(() => {
    const shown = plateKey();
    if (shown !== untrack(chosenPlate)) setChosenPlate(shown);
  });
  const [selected, setSelected] = createSignal<string | null>(null);
  // A selection that left the shown plate (moved, deleted) is dropped.
  const selectedInstanceKey = createMemo(() => {
    const key = selected();
    const plate = document()?.plates.find((candidate) => candidate.plateKey === plateKey());
    return key && plate?.instances.some((instance) => instance.instanceKey === key) ? key : null;
  });

  return {
    model,
    record,
    editor,
    document,
    geometry,
    loadFailed,
    meshes,
    options,
    volume,
    footprint,
    validation,
    object,
    objectName: (objectKey) => object(objectKey)?.name ?? `Object ${objectKey}`,
    plateKey,
    selectPlate: (key) => batch(() => {
      setChosenPlate(key);
      setSelected(null);
    }),
    selectedInstanceKey,
    select: setSelected,
    continueWithSourceRevision,
  };
}
