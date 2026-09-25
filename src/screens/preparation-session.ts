/** Everything the Preparation workspace (and Task 13's panel beside it)
 *  shares while a Model is being prepared: the store's record with the
 *  optimistic editor on top, the source revision's geometry and meshes,
 *  the target's slice options and build volume, each instance's footprint,
 *  the live validation, and the plate and object selection. */
import { batch, createEffect, createMemo, createSignal, on, onCleanup, untrack } from "solid-js";
import { isCommandError } from "../ipc/client";
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
  InstanceTransform,
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
  /** Why the slice options couldn't load, if they couldn't. */
  optionsError: () => string | undefined;
  /** The target's printable space; `null` until the options load. */
  volume: () => BuildVolume | null;
  /** The instance's footprint as last checked: after a turn or scale,
   *  the previous one until the new one is computed (see
   *  {@link checkingPlacement}); `undefined` before its first check. */
  footprint: (instance: InstanceDoc) => Footprint | undefined;
  /** The footprint shown is from before the latest turn or scale; the new
   *  one is being computed. */
  checkingPlacement: (instance: InstanceDoc) => boolean;
  /** The current footprint, computed now if need be. For one-off commands
   *  (arrange, duplicate) that need it exact; never on a key repeat. */
  footprintNow: (instance: InstanceDoc) => Footprint | undefined;
  validation: () => PreparationValidation;
  object: (objectKey: number) => GeometryObject | undefined;
  objectName: (objectKey: number) => string;
  /** An instance's name: its object's, numbered when the object is placed
   *  more than once ("Lid 2"). */
  instanceName: (instanceKey: string) => string;
  /** The plate tab shown; the first plate when the chosen one is gone. */
  plateKey: () => string | undefined;
  selectPlate: (plateKey: string) => void;
  selectedInstanceKey: () => string | null;
  select: (instanceKey: string | null) => void;
  /** **Continue with revision M**, as held in the store for `start_slice`. */
  continueWithSourceRevision: () => string | undefined;
  /** Asks the workspace to show the Preparation panel (folded behind
   *  **Settings panel** when narrow), e.g. for a slice that failed. */
  revealPanel: () => void;
  /** Counts {@link revealPanel} requests, for the workspace to follow. */
  panelReveals: () => number;
}

/** How many footprints stay cached (one per object, rotation and scale). */
const FOOTPRINT_CACHE_SIZE = 256;

export function createPreparationSession(model: () => ModelRecord): PreparationSession {
  // A session is for one Model; its id is held so the final save on
  // leaving still finds the record once the host's model is gone.
  const modelId = untrack(model).id;
  const record = () => slicing.preparation(modelId);
  const editor = createPreparationEditor({ preparation: record, save: updatePreparation });
  onCleanup(() => editor.dispose());

  // D18/State: dropped when leaving preparation mode.
  const cache = createGeometryCache({ loadGeometry, loadMesh });
  onCleanup(() => cache.clear());

  // Loads resolve after the session may be gone (leaving mid-load); their
  // results are then dropped.
  let disposed = false;
  onCleanup(() => { disposed = true; });
  const revisionId = createMemo(() => record()?.sourceRevisionId);
  const wanted = (id: string) => !disposed && untrack(revisionId) === id;

  const [geometry, setGeometry] = createSignal<RevisionGeometry | undefined>();
  const [meshes, setMeshes] = createSignal<Map<number, MeshBuffer>>(new Map());
  const [loadFailed, setLoadFailed] = createSignal(false);

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
      (loaded) => { if (wanted(id)) setGeometry(loaded); },
      () => { if (wanted(id)) setLoadFailed(true); },
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
        if (!wanted(id)) return;
        setMeshes((held) => new Map([...held, ...entries]));
      },
      () => { if (wanted(id)) setLoadFailed(true); },
    );
  }));

  const [options, setOptions] = createSignal<SliceOptions | undefined>();
  const [optionsError, setOptionsError] = createSignal<string | undefined>();
  const targetKey = createMemo(() => {
    const target = document()?.target;
    return target ? JSON.stringify(target) : undefined;
  });
  createEffect(on(targetKey, (key) => {
    batch(() => {
      setOptions(undefined);
      setOptionsError(undefined);
    });
    if (!key) return;
    const target = untrack(document)!.target;
    listSliceOptions(target).then(
      (loaded) => { if (!disposed && untrack(targetKey) === key) setOptions(loaded); },
      // No options (e.g. no slicer runtime): no volume to check against.
      (error: unknown) => {
        if (disposed || untrack(targetKey) !== key) return;
        setOptionsError(isCommandError(error) ? error.message : "The slice options could not be loaded.");
      },
    );
  }));
  const volume = createMemo<BuildVolume | null>(() => {
    const loaded = options();
    return loaded ? buildVolumeFromProfile(loaded.profileSnapshot) : null;
  });

  // A footprint scans every vertex (a quarter of a second on a
  // million-vertex mesh), so it never runs on the input path. Each is kept
  // per mesh, rotation and scale: a move reuses it, and instances sharing
  // all three share it. A turn or scale shows the instance's last known
  // footprint, marked as checking, and the new one is computed in a later
  // task; only the newest request per instance runs.
  const footprints = new Map<string, { mesh: MeshBuffer; footprint: Footprint | null }>();
  const lastKnown = new Map<string, Footprint>();
  const queued = new Map<string, { shape: string; mesh: MeshBuffer; transform: InstanceTransform }>();
  const [footprintVersion, setFootprintVersion] = createSignal(0);
  let timer: ReturnType<typeof setTimeout> | undefined;
  onCleanup(() => {
    clearTimeout(timer);
    queued.clear();
  });

  const shapeOf = (instance: InstanceDoc) => {
    const { rotateDeg, scale } = instance.transform;
    return `${instance.objectKey}|${rotateDeg.join(",")}|${scale.join(",")}`;
  };
  const remember = (shape: string, mesh: MeshBuffer, footprint: Footprint | null) => {
    footprints.delete(shape);
    footprints.set(shape, { mesh, footprint });
    if (footprints.size > FOOTPRINT_CACHE_SIZE) footprints.delete(footprints.keys().next().value!);
  };
  const cached = (instance: InstanceDoc, mesh: MeshBuffer) => {
    const held = footprints.get(shapeOf(instance));
    return held && held.mesh === mesh ? held : undefined;
  };

  const runQueued = () => {
    timer = undefined;
    const next = queued.entries().next();
    if (next.done) return;
    const [instanceKey, request] = next.value;
    queued.delete(instanceKey);
    const held = footprints.get(request.shape);
    if (!held || held.mesh !== request.mesh) {
      remember(request.shape, request.mesh, footprintOf(request.transform, request.mesh.positions));
    }
    setFootprintVersion((version) => version + 1);
    if (queued.size > 0) timer = setTimeout(runQueued, 0);
  };
  const request = (instanceKey: string, shape: string, mesh: MeshBuffer, transform: InstanceTransform) => {
    const waiting = queued.get(instanceKey);
    if (waiting?.shape === shape && waiting.mesh === mesh) return;
    // Re-inserted, so the newest request goes to the back of the queue.
    queued.delete(instanceKey);
    queued.set(instanceKey, { shape, mesh, transform });
    if (timer === undefined) timer = setTimeout(runQueued, 0);
  };

  const footprint = (instance: InstanceDoc): Footprint | undefined => {
    footprintVersion();
    const mesh = meshes().get(instance.objectKey);
    if (!mesh) return undefined;
    const held = cached(instance, mesh);
    if (held) {
      queued.delete(instance.instanceKey);
      if (held.footprint) lastKnown.set(instance.instanceKey, held.footprint);
      return held.footprint ?? undefined;
    }
    request(instance.instanceKey, shapeOf(instance), mesh, instance.transform);
    return lastKnown.get(instance.instanceKey);
  };
  const checkingPlacement = (instance: InstanceDoc): boolean => {
    footprintVersion();
    const mesh = meshes().get(instance.objectKey);
    return !!mesh && !cached(instance, mesh);
  };
  const footprintNow = (instance: InstanceDoc): Footprint | undefined => {
    const mesh = meshes().get(instance.objectKey);
    if (!mesh) return undefined;
    const held = cached(instance, mesh);
    if (held) return held.footprint ?? undefined;
    const computed = footprintOf(instance.transform, mesh.positions);
    remember(shapeOf(instance), mesh, computed);
    queued.delete(instance.instanceKey);
    setFootprintVersion((version) => version + 1);
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
  const objectName = (objectKey: number) => object(objectKey)?.name ?? `Object ${objectKey}`;
  const instanceNames = createMemo(() => {
    const instances = (document()?.plates ?? []).flatMap((plate) => plate.instances);
    const counts = new Map<number, number>();
    for (const instance of instances) counts.set(instance.objectKey, (counts.get(instance.objectKey) ?? 0) + 1);
    const seen = new Map<number, number>();
    const names = new Map<string, string>();
    for (const instance of instances) {
      const n = (seen.get(instance.objectKey) ?? 0) + 1;
      seen.set(instance.objectKey, n);
      const base = objectName(instance.objectKey);
      names.set(instance.instanceKey, (counts.get(instance.objectKey) ?? 0) > 1 ? `${base} ${n}` : base);
    }
    return names;
  });

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

  const [panelReveals, setPanelReveals] = createSignal(0);

  return {
    model,
    record,
    editor,
    document,
    geometry,
    loadFailed,
    meshes,
    options,
    optionsError,
    volume,
    footprint,
    checkingPlacement,
    footprintNow,
    validation,
    object,
    objectName,
    instanceName: (instanceKey) => instanceNames().get(instanceKey) ?? "",
    plateKey,
    selectPlate: (key) => batch(() => {
      setChosenPlate(key);
      setSelected(null);
    }),
    selectedInstanceKey,
    select: setSelected,
    continueWithSourceRevision,
    revealPanel: () => setPanelReveals((count) => count + 1),
    panelReveals,
  };
}
