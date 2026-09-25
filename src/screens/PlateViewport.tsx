import {
  createEffect,
  createMemo,
  createSignal,
  createUniqueId,
  For,
  on,
  onCleanup,
  onMount,
  Show,
} from "solid-js";
import { Button, onThemeChange } from "../design-system";
import type { MeshBuffer } from "../slicing/mesh-buffer";
import { composeTransform } from "../slicing/transforms";
import { describePlate, type ViewportInstance } from "../slicing/viewport/plate-view";
import type {
  BuildVolume,
  CameraView,
  RenderedInstance,
  ViewportOverlays,
  ViewportRenderer,
} from "../slicing/viewport/renderer";
import { createViewportRenderer } from "../slicing/viewport/renderer-factory";
import { readViewportTheme } from "../slicing/viewport/theme";
import styles from "./PlateViewport.module.css";

interface Placement {
  mesh: MeshBuffer;
  rotateDeg: number[];
  scale: number[];
  matrix: readonly number[];
}

/** A toolbar command with a keyboard shortcut (D19). Task-specific tools
 *  (move, rotate, …) are passed in; the viewport always has the views. */
export interface ViewportTool {
  id: string;
  label: string;
  /** The hint shown on the button, e.g. "R". */
  shortcut: string;
  /** For `aria-keyshortcuts`, e.g. "R Shift+R". */
  ariaKeyshortcuts: string;
  /** Whether a keydown on the viewport runs this tool. */
  matches: (event: KeyboardEvent) => boolean;
  run: (event?: KeyboardEvent) => void;
  disabled?: boolean;
}

export interface PlateViewportProps {
  /** The canvas's accessible name, e.g. "3D view of Enclosure lid". */
  label: string;
  /** Changing it refits the camera. */
  plateKey: string;
  plateName: string;
  meshes: Map<number, MeshBuffer>;
  instances: ViewportInstance[];
  /** `null` draws a floor grid instead of a volume. */
  buildVolume: BuildVolume | null;
  selectedInstanceKey?: string | null;
  /** When set, `[`/`]`, their buttons, and clicking select objects. */
  onSelect?: (instanceKey: string | null) => void;
  tools?: ViewportTool[];
  overlays?: ViewportOverlays;
  /** Further sentences for the description. */
  notes?: string[];
  /** A shorter canvas, for the Model details inspector. */
  compact?: boolean;
}

/** The accessibility description settles for this long before it is
 *  announced. */
export const DESCRIPTION_DEBOUNCE_MS = 500;
/** A pointer that moves further than this between down and up orbited,
 *  so it doesn't pick. */
const CLICK_SLOP_PX = 4;

const VIEWS: { view: CameraView; label: string; key: string }[] = [
  { view: "top", label: "Top", key: "1" },
  { view: "front", label: "Front", key: "2" },
  { view: "left", label: "Left", key: "3" },
  { view: "right", label: "Right", key: "4" },
  { view: "iso", label: "Iso", key: "5" },
  { view: "reset", label: "Reset", key: "0" },
];

export const WEBGL_UNAVAILABLE = "3D view unavailable (WebGL is not available)";
export const VIEWER_FAILED = "3D view unavailable (the viewer failed to load)";
export const CONTEXT_LOST = "3D view unavailable (the graphics context was lost)";

const sameNumbers = (a: readonly number[], b: readonly number[]) => a.every((value, i) => value === b[i]);

const plainKey = (event: KeyboardEvent) => !event.ctrlKey && !event.metaKey && !event.altKey;

/** D18/D19: the canvas, a toolbar with shortcut hints, and a live text
 *  description. Used read-only (Model details) and with tools
 *  (Preparation). Shortcuts act only while the canvas has focus; Escape
 *  returns focus to the toolbar. */
export function PlateViewport(props: PlateViewportProps) {
  let canvas: HTMLCanvasElement | undefined;
  let root: HTMLDivElement | undefined;
  let toolbar: HTMLDivElement | undefined;
  const descriptionId = createUniqueId();
  const [renderer, setRenderer] = createSignal<ViewportRenderer | undefined>();
  // Why there is no 3D view, as shown in its place; `undefined` while it works.
  const [unavailable, setUnavailable] = createSignal<string | undefined>();

  onMount(() => {
    let disposed = false;
    let mounted: ViewportRenderer | undefined;
    void createViewportRenderer().then((created) => {
      if (disposed) {
        created.dispose();
        return;
      }
      try {
        created.mount(canvas!);
      } catch {
        created.dispose();
        setUnavailable(WEBGL_UNAVAILABLE);
        return;
      }
      created.setTheme(readViewportTheme(root!));
      mounted = created;
      setRenderer(created);
    }, (error: unknown) => {
      console.error("The 3D viewer failed to load:", error);
      if (!disposed) setUnavailable(VIEWER_FAILED);
    });
    // A lost context (a GPU reset, or the driver reclaiming it) would
    // otherwise leave a blank canvas.
    const onContextLost = () => {
      if (!disposed) setUnavailable(CONTEXT_LOST);
    };
    canvas!.addEventListener("webglcontextlost", onContextLost);
    const stopTheme = onThemeChange(() => mounted?.setTheme(readViewportTheme(root!)));
    onCleanup(() => {
      disposed = true;
      canvas!.removeEventListener("webglcontextlost", onContextLost);
      stopTheme();
      mounted?.dispose();
    });
  });

  // Composed matrices per instance. The Z drop scans every vertex, so it is
  // redone only when the mesh, rotation, or scale changes: a selection
  // change reuses the matrix, and a move only replaces its XY.
  const placed = new Map<string, Placement>();
  const matrixOf = (instance: ViewportInstance, mesh: MeshBuffer): readonly number[] => {
    const { translateMm, rotateDeg, scale } = instance.transform;
    const held = placed.get(instance.instanceKey);
    if (held && held.mesh === mesh && sameNumbers(held.rotateDeg, rotateDeg) && sameNumbers(held.scale, scale)) {
      if (held.matrix[9] !== translateMm[0] || held.matrix[10] !== translateMm[1]) {
        const matrix = [...held.matrix];
        matrix[9] = translateMm[0];
        matrix[10] = translateMm[1];
        held.matrix = matrix;
      }
      return held.matrix;
    }
    const matrix = composeTransform(instance.transform, mesh.positions);
    placed.set(instance.instanceKey, { mesh, rotateDeg: [...rotateDeg], scale: [...scale], matrix });
    return matrix;
  };
  const rendered = createMemo((): RenderedInstance[] => {
    const next = props.instances.flatMap((instance) => {
      const mesh = props.meshes.get(instance.objectKey);
      if (!mesh) return [];
      return [{
        instanceKey: instance.instanceKey,
        objectKey: instance.objectKey,
        matrix: matrixOf(instance, mesh),
        selected: instance.instanceKey === props.selectedInstanceKey,
        outOfBounds: instance.outOfBounds ?? false,
      }];
    });
    const keys = new Set(next.map((instance) => instance.instanceKey));
    for (const key of placed.keys()) if (!keys.has(key)) placed.delete(key);
    return next;
  });
  const hasObjects = createMemo(() => rendered().length > 0);

  createEffect(() => renderer()?.setBuildVolume(props.buildVolume));
  createEffect(() => renderer()?.setMeshes(props.meshes));
  createEffect(() => renderer()?.setInstances(rendered()));
  createEffect(() => renderer()?.setOverlays(props.overlays ?? {}));
  // Refit when the plate changes, or once its objects first show.
  createEffect(on(
    [renderer, () => props.plateKey, hasObjects],
    ([current]) => current?.setCamera("reset"),
  ));

  const showView = (view: CameraView) => renderer()?.setCamera(view);

  const selectStep = (step: 1 | -1) => {
    const keys = props.instances.map((instance) => instance.instanceKey);
    if (!props.onSelect || keys.length === 0) return;
    const at = keys.indexOf(props.selectedInstanceKey ?? "");
    const next = at === -1 ? (step === 1 ? 0 : keys.length - 1) : (at + step + keys.length) % keys.length;
    props.onSelect(keys[next]);
  };

  const onKeyDown = (event: KeyboardEvent) => {
    if (event.key === "Escape") {
      event.preventDefault();
      toolbar?.querySelector<HTMLButtonElement>("button:not([disabled])")?.focus();
      return;
    }
    const tool = props.tools?.find((candidate) => !candidate.disabled && candidate.matches(event));
    if (tool) {
      event.preventDefault();
      tool.run(event);
      return;
    }
    if (!plainKey(event) || event.shiftKey) return;
    const view = VIEWS.find((entry) => entry.key === event.key);
    if (view) {
      event.preventDefault();
      showView(view.view);
    } else if (event.key === "[" || event.key === "]") {
      event.preventDefault();
      selectStep(event.key === "]" ? 1 : -1);
    }
  };

  let down: { x: number; y: number } | undefined;
  const onPointerDown = (event: PointerEvent) => {
    down = event.button === 0 ? { x: event.clientX, y: event.clientY } : undefined;
  };
  const onPointerUp = (event: PointerEvent) => {
    const start = down;
    down = undefined;
    if (!start || !props.onSelect) return;
    if (Math.hypot(event.clientX - start.x, event.clientY - start.y) > CLICK_SLOP_PX) return;
    const box = canvas!.getBoundingClientRect();
    props.onSelect(renderer()?.pick(event.clientX - box.left, event.clientY - box.top)?.instanceKey ?? null);
  };

  // The live region repeats the description only once it settles.
  const description = createMemo(() => describePlate({
    plateName: props.plateName,
    instances: props.instances,
    selectedInstanceKey: props.selectedInstanceKey,
    notes: props.notes,
  }));
  const [announced, setAnnounced] = createSignal(description());
  createEffect(on(description, (text) => {
    const timer = setTimeout(() => setAnnounced(text), DESCRIPTION_DEBOUNCE_MS);
    onCleanup(() => clearTimeout(timer));
  }, { defer: true }));

  return (
    <div ref={root} class={styles.viewport} classList={{ [styles.compact]: props.compact }}>
      <div ref={toolbar} class={styles.toolbar} role="group" aria-label="Viewport">
        <For each={VIEWS}>
          {(entry) => (
            <Button
              variant="ghost"
              size="sm"
              aria-keyshortcuts={entry.key}
              disabled={unavailable() !== undefined}
              onClick={() => showView(entry.view)}
            >
              {entry.label}
              <kbd class={styles.kbd} aria-hidden="true">{entry.key}</kbd>
            </Button>
          )}
        </For>
        <Show when={props.onSelect && props.instances.length > 1}>
          <span class={styles.divider} aria-hidden="true" />
          <Button variant="ghost" size="sm" aria-keyshortcuts="[" onClick={() => selectStep(-1)}>
            Previous object<kbd class={styles.kbd} aria-hidden="true">[</kbd>
          </Button>
          <Button variant="ghost" size="sm" aria-keyshortcuts="]" onClick={() => selectStep(1)}>
            Next object<kbd class={styles.kbd} aria-hidden="true">]</kbd>
          </Button>
        </Show>
        <Show when={props.tools?.length}>
          <span class={styles.divider} aria-hidden="true" />
          <For each={props.tools}>
            {(tool) => (
              <Button
                variant="ghost"
                size="sm"
                aria-keyshortcuts={tool.ariaKeyshortcuts}
                disabled={tool.disabled}
                onClick={() => tool.run()}
              >
                {tool.label}
                <kbd class={styles.kbd} aria-hidden="true">{tool.shortcut}</kbd>
              </Button>
            )}
          </For>
        </Show>
      </div>
      <div class={styles.stage}>
        <canvas
          ref={canvas}
          class={styles.canvas}
          classList={{ [styles.hidden]: unavailable() !== undefined }}
          role="img"
          aria-label={props.label}
          aria-describedby={descriptionId}
          tabIndex={unavailable() ? -1 : 0}
          onKeyDown={onKeyDown}
          onPointerDown={onPointerDown}
          onPointerUp={onPointerUp}
        />
        <Show when={unavailable()}>
          {(reason) => <p class={styles.unavailable} role="note">{reason()}</p>}
        </Show>
      </div>
      <p id={descriptionId} class={styles.description} aria-live="polite">{announced()}</p>
    </div>
  );
}
