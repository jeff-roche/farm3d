import { IconChevronLeft, IconChevronRight } from "@tabler/icons-solidjs";
import { createEffect, createMemo, createResource, createSignal, on, onCleanup, Show } from "solid-js";
import { IconButton } from "../design-system";
import type { ModelRecord } from "../library/types";
import { createGeometryCache } from "../slicing/geometry-cache";
import type { MeshBuffer } from "../slicing/mesh-buffer";
import { loadGeometry, loadMesh } from "../slicing/slicing-store";
import { sourceLayout } from "../slicing/source-plates";
import styles from "./ModelPlateInspector.module.css";
import { PlateViewport } from "./PlateViewport";

export interface ModelPlateInspectorProps {
  /** An STL or 3MF Model; its current revision is shown. */
  model: ModelRecord;
  /** The 3MF's Orca/Bambu plates (`ThreeMfInspection.plates`); empty for
   *  an STL, a 3MF without plates, or while the inspection loads. */
  plates: { index: number; name?: string }[];
}

/** D21: Model details' small read-only `PlateViewport` of the current
 *  revision, one plate at a time, laid out as the file places it.
 *  Unprintable build items are not placed; they are listed, visibly and in
 *  the description (D5). */
export function ModelPlateInspector(props: ModelPlateInspectorProps) {
  const cache = createGeometryCache({ loadGeometry, loadMesh });
  onCleanup(() => cache.clear());

  const revisionId = () => props.model.currentRevision.id;
  const [source] = createResource(revisionId, async (id) => {
    const geometry = await cache.geometry(id);
    const keys = [...new Set(geometry.buildItems.filter((item) => item.printable).map((item) => item.objectKey))]
      .filter((key) => geometry.objects.some((object) => object.objectKey === key));
    const meshes = new Map<number, MeshBuffer>(
      await Promise.all(keys.map(async (key) => [key, await cache.mesh(id, key)] as const)),
    );
    return { geometry, meshes };
  });
  // Reading an errored resource throws, so check the error first.
  const loaded = () => (source.error ? undefined : source());
  const layout = createMemo(() => {
    const data = loaded();
    return data && sourceLayout(data.geometry, props.plates, (key) => data.meshes.get(key)?.positions);
  });

  const [plateAt, setPlateAt] = createSignal(0);
  const [selected, setSelected] = createSignal<string | null>(null);
  createEffect(on(revisionId, () => {
    setPlateAt(0);
    setSelected(null);
  }, { defer: true }));

  const plates = () => layout()?.plates ?? [];
  const plate = () => plates()[Math.min(plateAt(), plates().length - 1)];
  const step = (by: 1 | -1) => {
    const count = plates().length;
    setPlateAt((at) => (at + by + count) % count);
    setSelected(null);
  };
  const unprintableNote = () => {
    const names = layout()?.unprintable ?? [];
    return names.length > 0 ? `Not placed (marked not printable in the file): ${names.join(", ")}.` : undefined;
  };

  return (
    <section class={styles.inspector} aria-label="Plates">
      <Show
        when={plate()}
        fallback={
          <p class={styles.placeholder} role="status">
            {source.error ? "The 3D view could not load." : "Loading the 3D view…"}
          </p>
        }
      >
        {(current) => (
          <>
            <Show when={plates().length > 1}>
              <div class={styles.plateBar}>
                <IconButton aria-label="Previous plate" onClick={() => step(-1)}>
                  <IconChevronLeft size={14} aria-hidden="true" />
                </IconButton>
                <span class={styles.plateName} aria-live="polite">
                  Plate {plateAt() + 1} of {plates().length}: {current().name}
                </span>
                <IconButton aria-label="Next plate" onClick={() => step(1)}>
                  <IconChevronRight size={14} aria-hidden="true" />
                </IconButton>
              </div>
            </Show>
            <PlateViewport
              compact
              label={`3D view of ${props.model.name}`}
              plateKey={`${revisionId()}\n${current().index}`}
              plateName={current().name}
              meshes={loaded()?.meshes ?? new Map()}
              instances={current().instances}
              buildVolume={null}
              selectedInstanceKey={selected()}
              onSelect={setSelected}
              notes={unprintableNote() ? [unprintableNote()!] : []}
            />
            <Show when={unprintableNote()}>
              {(note) => <p class={styles.note}>{note()}</p>}
            </Show>
          </>
        )}
      </Show>
    </section>
  );
}
