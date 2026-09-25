import { IconArrowLeft } from "@tabler/icons-solidjs";
import { createEffect, createMemo, createSignal, on, onCleanup, onMount, Show, type JSX } from "solid-js";
import { Button } from "../design-system";
import { loadRevisions } from "../library/library-store";
import { measureBetween } from "../slicing/bounds";
import { findInstance, plateLabel } from "../slicing/preparation-edits";
import { chooseContinueWithSourceRevision, reloadPreparation } from "../slicing/slicing-store";
import type { Vec3 } from "../slicing/transforms";
import type { PickResult } from "../slicing/viewport/renderer";
import { CommitNumberField } from "./CommitNumberField";
import { InstanceInspector } from "./InstanceInspector";
import { MeasurePanel } from "./MeasurePanel";
import { PlateTabs } from "./PlateTabs";
import { PlateViewport, type ViewportTool } from "./PlateViewport";
import {
  createPreparationActions,
  MOVE_LARGE_STEP_MM,
  MOVE_STEP_MM,
  ROTATE_STEP_DEG,
  type PreparationActions,
} from "./preparation-actions";
import type { PreparationSession } from "./preparation-session";
import { PreparationObjectList } from "./PreparationObjectList";
import { StalePreparationBanner } from "./StalePreparationBanner";
import styles from "./PreparationWorkspace.module.css";

export interface PreparationWorkspaceProps {
  session: PreparationSession;
  /** **Back to Library**. Pending edits are saved on the way out. */
  onBack: () => void;
  /** Task 13's `PreparationPanel`, docked on the right. */
  dock?: JSX.Element;
}

/** Below this width the object list folds under the viewport behind a
 *  toggle (Accessibility and adaptation, 1024 × 700). */
const INLINE_OBJECTS_MIN_WIDTH = 1280;

const ARROWS: Record<string, [number, number]> = {
  ArrowLeft: [-1, 0],
  ArrowRight: [1, 0],
  ArrowUp: [0, 1],
  ArrowDown: [0, -1],
};

const noModifiers = (event: KeyboardEvent) => !event.ctrlKey && !event.metaKey && !event.altKey;
const keyIs = (event: KeyboardEvent, key: string) => event.key.toLowerCase() === key;

/** D19's command set, built over the actions. The viewport shows every
 *  one as a toolbar button with its hint; the object list runs the same
 *  keys, except the arrows, which move through its rows. */
export function preparationTools(
  actions: PreparationActions,
  state: {
    hasSelection: () => boolean;
    canLayFlat: () => boolean;
    canArrange: () => boolean;
    measuring: () => boolean;
    toggleMeasure: () => void;
    focusPosition: () => void;
    focusPlate: () => void;
  },
): ViewportTool[] {
  const none = () => !state.hasSelection();
  return [
    {
      id: "move",
      label: "Move",
      shortcut: "←↑↓→",
      ariaKeyshortcuts: "ArrowLeft ArrowRight ArrowUp ArrowDown Shift+ArrowLeft Shift+ArrowRight Shift+ArrowUp Shift+ArrowDown",
      matches: (event) => noModifiers(event) && event.key in ARROWS,
      run: (event) => {
        if (!event) return state.focusPosition();
        const [dx, dy] = ARROWS[event.key];
        const step = event.shiftKey ? MOVE_LARGE_STEP_MM : MOVE_STEP_MM;
        actions.moveBy(dx * step, dy * step);
      },
      get disabled() { return none(); },
    },
    {
      id: "rotate",
      label: `Rotate ${ROTATE_STEP_DEG}°`,
      shortcut: "R",
      ariaKeyshortcuts: "R Shift+R",
      matches: (event) => noModifiers(event) && keyIs(event, "r"),
      run: (event) => actions.rotateZ(event?.shiftKey ? -ROTATE_STEP_DEG : ROTATE_STEP_DEG),
      get disabled() { return none(); },
    },
    {
      id: "scale-up",
      label: "Scale +5%",
      shortcut: "+",
      ariaKeyshortcuts: "+",
      matches: (event) => noModifiers(event) && (event.key === "+" || event.key === "="),
      run: () => actions.scaleStep(1),
      get disabled() { return none(); },
    },
    {
      id: "scale-down",
      label: "Scale −5%",
      shortcut: "−",
      ariaKeyshortcuts: "-",
      matches: (event) => noModifiers(event) && (event.key === "-" || event.key === "_"),
      run: () => actions.scaleStep(-1),
      get disabled() { return none(); },
    },
    {
      id: "lay-flat",
      label: "Lay flat",
      shortcut: "F",
      ariaKeyshortcuts: "F",
      matches: (event) => noModifiers(event) && !event.shiftKey && keyIs(event, "f"),
      run: () => actions.layFlatNext(),
      get disabled() { return !state.canLayFlat(); },
    },
    {
      id: "arrange",
      label: "Arrange plate",
      shortcut: "A",
      ariaKeyshortcuts: "A",
      matches: (event) => noModifiers(event) && !event.shiftKey && keyIs(event, "a"),
      run: () => actions.arrangePlate(),
      get disabled() { return !state.canArrange(); },
    },
    {
      id: "measure",
      get label() { return state.measuring() ? "Stop measuring" : "Measure"; },
      shortcut: "M",
      ariaKeyshortcuts: "M",
      matches: (event) => noModifiers(event) && !event.shiftKey && keyIs(event, "m"),
      run: () => state.toggleMeasure(),
    },
    {
      id: "move-to-plate",
      label: "Move to plate",
      shortcut: "⇧1–9",
      ariaKeyshortcuts: "Shift+1 Shift+2 Shift+3 Shift+4 Shift+5 Shift+6 Shift+7 Shift+8 Shift+9",
      // `code`, since Shift turns the digit key into a symbol.
      matches: (event) => noModifiers(event) && event.shiftKey && /^(Digit|Numpad)[1-9]$/.test(event.code),
      run: (event) => (event ? actions.moveToPlateAt(Number(event.code.slice(-1))) : state.focusPlate()),
      get disabled() { return none(); },
    },
    {
      id: "duplicate",
      label: "Duplicate",
      shortcut: "Ctrl+D",
      ariaKeyshortcuts: "Control+D Meta+D",
      matches: (event) => (event.ctrlKey || event.metaKey) && !event.altKey && keyIs(event, "d"),
      run: () => actions.duplicate(),
      get disabled() { return none(); },
    },
    {
      id: "delete",
      label: "Delete",
      shortcut: "Del",
      ariaKeyshortcuts: "Delete",
      matches: (event) => noModifiers(event) && event.key === "Delete",
      run: () => actions.remove(),
      get disabled() { return none(); },
    },
  ];
}

function noticeText(notice: ReturnType<PreparationSession["editor"]["notice"]>): string | undefined {
  if (!notice) return undefined;
  return notice.kind === "conflict"
    ? "This Preparation was changed elsewhere, so your latest edits were not saved. The current version is shown."
    : `Your latest edit was not saved: ${notice.message}`;
}

/** D19's Preparation workspace, in the Library's centre pane: plate tabs,
 *  the viewport with its tools, the object list, and the selected
 *  object's numeric fields. It also shows the stale-source banner (D5),
 *  and whether edits are saved. */
export function PreparationWorkspace(props: PreparationWorkspaceProps) {
  const session = props.session;
  const actions = createPreparationActions(session);
  let inspector: HTMLDivElement | undefined;

  const [narrow, setNarrow] = createSignal(window.innerWidth < INLINE_OBJECTS_MIN_WIDTH);
  // Folded by default when narrow, so the viewport keeps its room.
  const [objectsOpen, setObjectsOpen] = createSignal(!narrow());
  onMount(() => {
    const onResize = () => setNarrow(window.innerWidth < INLINE_OBJECTS_MIN_WIDTH);
    window.addEventListener("resize", onResize);
    onCleanup(() => window.removeEventListener("resize", onResize));
  });

  const document = () => session.document();
  const plates = () => document()?.plates ?? [];
  const plate = () => plates().find((candidate) => candidate.plateKey === session.plateKey());
  const selected = () => {
    const key = session.selectedInstanceKey();
    const shown = document();
    return key && shown ? findInstance(shown, key)?.instance : undefined;
  };

  // --- Measure ---------------------------------------------------------------
  const [measuring, setMeasuring] = createSignal(false);
  const [points, setPoints] = createSignal<Vec3[]>([]);
  const [otherKey, setOtherKey] = createSignal<string | undefined>();
  createEffect(on(() => session.plateKey(), () => {
    setPoints([]);
    setOtherKey(undefined);
  }, { defer: true }));
  const onPick = (pick: PickResult | null) => {
    if (!pick) return;
    setPoints((held) => (held.length >= 2 ? [pick.pointMm] : [...held, pick.pointMm]));
  };
  const others = () => (plate()?.instances ?? [])
    .filter((instance) => instance.instanceKey !== session.selectedInstanceKey())
    .map((instance) => ({ key: instance.instanceKey, name: session.instanceName(instance.instanceKey) }));
  const between = createMemo(() => {
    const from = selected();
    const to = plate()?.instances.find((instance) => instance.instanceKey === otherKey());
    if (!from || !to || from === to) return undefined;
    const a = session.footprint(from);
    const b = session.footprint(to);
    if (!a || !b) return undefined;
    return measureBetween(
      { footprint: a, translateMm: from.transform.translateMm },
      { footprint: b, translateMm: to.transform.translateMm },
    );
  });
  const overlays = () => {
    if (!measuring()) return {};
    const picked = points();
    if (picked.length === 2) return { measure: [picked[0], picked[1]] as [Vec3, Vec3] };
    const distance = between();
    return distance ? { measure: [distance.from, distance.to] as [Vec3, Vec3] } : {};
  };

  // --- Tools -----------------------------------------------------------------
  const focusIn = (selector: string) => inspector?.querySelector<HTMLElement>(selector)?.focus();
  const tools = preparationTools(actions, {
    hasSelection: () => selected() !== undefined,
    canLayFlat: () => {
      const instance = selected();
      return !!instance && (session.object(instance.objectKey)?.layFlatFaces.length ?? 0) > 0;
    },
    canArrange: () => session.volume() !== null && (plate()?.instances.length ?? 0) > 0,
    measuring,
    toggleMeasure: () => {
      setMeasuring((on) => !on);
      setPoints([]);
    },
    focusPosition: () => {
      if (narrow()) setObjectsOpen(true);
      queueMicrotask(() => focusIn("[data-field='position'] input"));
    },
    focusPlate: () => {
      if (narrow()) setObjectsOpen(true);
      queueMicrotask(() => focusIn("[data-field='plate'] button"));
    },
  });
  const listTools = tools.filter((tool) => tool.id !== "move");
  const onListKeyDown = (event: KeyboardEvent) => {
    if (event.defaultPrevented) return;
    const target = event.target as HTMLElement;
    if (target.closest("input, textarea, select, button, [role='listbox']")) return;
    const tool = listTools.find((candidate) => !candidate.disabled && candidate.matches(event));
    if (!tool) return;
    event.preventDefault();
    tool.run(event);
  };

  // --- Description and markers ---------------------------------------------------
  const placement = (key: string) => session.validation().placement.get(key);
  const viewportInstances = () => (plate()?.instances ?? []).map((instance) => {
    const check = placement(instance.instanceKey);
    return {
      instanceKey: instance.instanceKey,
      objectKey: instance.objectKey,
      name: session.instanceName(instance.instanceKey),
      transform: instance.transform,
      outOfBounds: !!check && (check.outOfBounds || check.inExcludeArea || check.tooTall),
    };
  });
  const platesWithIssues = createMemo(() => new Set(session.validation().issues.flatMap((issue) => (
    "plateKey" in issue ? [issue.plateKey] : []
  ))));
  const triangleCount = () => (plate()?.instances ?? [])
    .reduce((sum, instance) => sum + (session.object(instance.objectKey)?.triangleCount ?? 0), 0);

  // --- Stale source (D5) -------------------------------------------------------
  const [pinnedSequence, setPinnedSequence] = createSignal<number | undefined>();
  createEffect(on(() => session.record()?.stale ? session.record()?.sourceRevisionId : undefined, (revisionId) => {
    setPinnedSequence(undefined);
    if (!revisionId) return;
    loadRevisions(session.model().id).then(
      (revisions) => setPinnedSequence(revisions.find((revision) => revision.id === revisionId)?.sequence),
      () => {},
    );
  }));
  const [reloaded, setReloaded] = createSignal<string | undefined>();
  const reload = async () => {
    const record = session.record();
    if (!record) return;
    // Named from the revision being left: removed objects aren't in the
    // new one.
    const objects = session.geometry()?.objects ?? [];
    const names = (keys: number[]) => keys
      .map((key) => objects.find((object) => object.objectKey === key)?.name ?? `Object ${key}`)
      .join(", ");
    await session.editor.flush();
    const result = await reloadPreparation(record.id);
    const parts = [`Reloaded onto revision ${session.model().currentRevision.sequence}.`];
    if (result.removedObjectKeys.length > 0) parts.push(`Removed (no longer in the file): ${names(result.removedObjectKeys)}.`);
    if (result.addedObjectKeys.length > 0) parts.push(`Added to the first plate: ${result.addedObjectKeys.length} new ${result.addedObjectKeys.length === 1 ? "object" : "objects"}.`);
    setReloaded(parts.join(" "));
  };

  const saveState = () => {
    if (session.editor.saving()) return "Saving…";
    if (session.editor.dirty()) return "Unsaved changes";
    return "All changes saved";
  };

  const back = () => {
    session.editor.dispose();
    props.onBack();
  };

  return (
    <div class={styles.workspace}>
      <div class={styles.header}>
        <Button variant="ghost" size="sm" onClick={back}>
          <IconArrowLeft size={14} aria-hidden="true" /> Back to Library
        </Button>
        <h2 class={styles.title}>Preparing {session.model().name}</h2>
        <span class={styles.saveState} role="status">{saveState()}</span>
      </div>
      <Show when={noticeText(session.editor.notice())}>
        {(text) => (
          <div class={styles.notice} role="alert">
            <p class={styles.noticeText}>{text()}</p>
            <Button variant="ghost" size="sm" onClick={() => session.editor.dismissNotice()}>Dismiss</Button>
          </div>
        )}
      </Show>
      <Show when={session.record()?.stale}>
        <StalePreparationBanner
          pinnedSequence={pinnedSequence()}
          currentSequence={session.model().currentRevision.sequence}
          continuing={session.continueWithSourceRevision() !== undefined}
          onReload={reload}
          onContinue={() => {
            const record = session.record();
            if (record) chooseContinueWithSourceRevision(record.id, record.sourceRevisionId);
          }}
          onWithdrawContinue={() => {
            const record = session.record();
            if (record) chooseContinueWithSourceRevision(record.id, null);
          }}
        />
      </Show>
      <Show when={reloaded()}>
        {(text) => (
          <div class={styles.notice} role="status">
            <p class={styles.noticeText}>{text()}</p>
            <Button variant="ghost" size="sm" onClick={() => setReloaded(undefined)}>Dismiss</Button>
          </div>
        )}
      </Show>
      <div class={styles.main}>
        <div class={styles.centre}>
          <Show
            when={!session.loadFailed()}
            fallback={<p class={styles.message} role="alert">The Model's geometry could not load, so it can't be prepared.</p>}
          >
            <PlateTabs
              plates={plates()}
              value={session.plateKey()}
              onChange={session.selectPlate}
              onAdd={actions.addPlate}
              onRename={actions.renamePlate}
              onMove={actions.movePlate}
              onDelete={actions.deletePlate}
              withIssues={platesWithIssues()}
            >
              <div class={styles.body} classList={{ [styles.narrow]: narrow() }}>
                <PlateViewport
                  fill
                  label={`3D view of ${session.model().name}`}
                  plateKey={session.plateKey() ?? ""}
                  plateName={plate() ? plateLabel(plate()!, plates().indexOf(plate()!)) : ""}
                  meshes={session.meshes()}
                  instances={viewportInstances()}
                  buildVolume={session.volume()}
                  selectedInstanceKey={session.selectedInstanceKey()}
                  onSelect={session.select}
                  onPick={measuring() ? onPick : undefined}
                  tools={tools}
                  overlays={overlays()}
                />
                <div class={styles.side}>
                  <Show when={narrow()}>
                    <Button
                      variant="ghost"
                      size="sm"
                      aria-expanded={objectsOpen()}
                      onClick={() => setObjectsOpen((open) => !open)}
                    >
                      {objectsOpen() ? "Hide objects" : "Show objects"}
                    </Button>
                  </Show>
                  <Show when={!narrow() || objectsOpen()}>
                    <div ref={inspector} class={styles.inspector}>
                      <Show when={measuring()}>
                        <MeasurePanel
                          points={points()}
                          onClear={() => setPoints([])}
                          selectedName={selected() ? session.instanceName(selected()!.instanceKey) : undefined}
                          others={others()}
                          otherKey={otherKey()}
                          onOtherChange={setOtherKey}
                          between={between()}
                        />
                      </Show>
                      <PreparationObjectList
                        instances={plate()?.instances ?? []}
                        instanceName={session.instanceName}
                        placement={placement}
                        selectedInstanceKey={session.selectedInstanceKey()}
                        onSelect={session.select}
                        triangleCount={triangleCount()}
                        onKeyDown={onListKeyDown}
                      />
                      <Show when={selected()}>
                        {(instance) => (
                          <InstanceInspector
                            instance={instance()}
                            object={session.object(instance().objectKey)}
                            name={session.instanceName(instance().instanceKey)}
                            plates={plates()}
                            plateKey={session.plateKey()!}
                            actions={actions}
                          />
                        )}
                      </Show>
                      <div class={styles.arrange}>
                        <CommitNumberField
                          label="Arrange spacing (mm)"
                          value={actions.arrangeSpacing()}
                          minValue={0}
                          step={1}
                          onCommit={actions.setArrangeSpacing}
                        />
                        <Button
                          variant="secondary"
                          size="sm"
                          aria-keyshortcuts="A"
                          disabled={session.volume() === null || (plate()?.instances.length ?? 0) === 0}
                          onClick={() => actions.arrangePlate()}
                        >
                          Arrange plate
                        </Button>
                      </div>
                    </div>
                  </Show>
                </div>
              </div>
            </PlateTabs>
          </Show>
        </div>
        <Show when={props.dock}>
          <aside class={styles.dock} aria-label="Preparation settings">{props.dock}</aside>
        </Show>
      </div>
      <p class={styles.status} role="status">{actions.status()}</p>
    </div>
  );
}
