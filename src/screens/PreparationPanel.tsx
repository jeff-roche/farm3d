import { IconAlertTriangle } from "@tabler/icons-solidjs";
import { createMemo, createSignal, createUniqueId, For, onCleanup, Show } from "solid-js";
import { Button } from "../design-system";
import { plateLabel } from "../slicing/preparation-edits";
import { openSlicerSettings } from "../slicing/slicer-settings-opener";
import {
  candidateLines,
  FIELD_LABELS,
  NOTHING_TRIED,
  runtimeProblems,
  sliceErrorView,
  type PanelField,
  type SliceErrorView,
} from "../slicing/slice-presentation";
import { refreshSlicing, slicing, startSlice } from "../slicing/slicing-store";
import { issuesForPlates, type PreparationIssue } from "../slicing/validation";
import { ADVANCED_FIELDS, PreparationControls } from "./PreparationControls";
import type { PreparationSession } from "./preparation-session";
import { SliceOperationPanel } from "./SliceOperationPanel";
import styles from "./PreparationPanel.module.css";

export interface PreparationPanelProps {
  session: PreparationSession;
  /** Opens a published Slice Revision's review, when the host has one. */
  onOpenRevision?: (sliceRevisionId: string) => void;
}

/** One click of **Slice plate** or **Slice all plates**: its id is kept
 *  for **Try again**, so the backend replays rather than slicing twice
 *  (D10). */
interface SliceAttempt {
  operationId: string;
  plateKeys: string[];
}

/** D20's Preparation panel, docked beside the workspace: the target,
 *  material, quality and slicing controls, the validation list, the
 *  unprintable items, the Slice actions (or why OrcaSlicer can't slice),
 *  and the slice operations. */
export function PreparationPanel(props: PreparationPanelProps) {
  const session = props.session;
  let panel: HTMLDivElement | undefined;
  let settingsButton: HTMLButtonElement | undefined;
  const reasonId = createUniqueId();
  let disposed = false;
  onCleanup(() => { disposed = true; });

  const document = () => session.document();
  const plates = () => document()?.plates ?? [];
  const plateName = (plateKey: string) => {
    const index = plates().findIndex((plate) => plate.plateKey === plateKey);
    return index >= 0 ? plateLabel(plates()[index], index) : "A plate";
  };

  // --- Moving focus to what an issue or error is about -----------------------
  const [advancedOpen, setAdvancedOpen] = createSignal(false);
  const workspace = (): ParentNode => panel?.closest("[data-preparation-workspace]") ?? window.document;
  const focusLater = (find: () => HTMLElement | null | undefined) => {
    // After Solid and Kobalte have shown the tab, the section or the row.
    setTimeout(() => find()?.focus(), 0);
  };
  const focusField = (field: PanelField) => {
    if (ADVANCED_FIELDS.has(field)) setAdvancedOpen(true);
    // A disabled control (its presets still loading) can't take focus, so
    // the field itself does, which reads out its label and note.
    focusLater(() => {
      const wrapper = panel?.querySelector<HTMLElement>(`[data-panel-field="${field}"]`);
      return wrapper?.querySelector<HTMLElement>(":is(button, input):not(:disabled)") ?? wrapper;
    });
  };
  const focusPlate = (plateKey: string) => {
    session.selectPlate(plateKey);
    focusLater(() => workspace().querySelector<HTMLElement>("[role='tab'][aria-selected='true']"));
  };
  const focusObject = (plateKey: string, instanceKey: string) => {
    if (session.plateKey() !== plateKey) session.selectPlate(plateKey);
    session.select(instanceKey);
    focusLater(() => workspace().querySelector<HTMLElement>("canvas[role='img']"));
  };
  const focusStaleBanner = () => {
    focusLater(() => workspace().querySelector<HTMLElement>("[role='region'][aria-label='Source changed'] button"));
  };

  // --- Validation (D20) --------------------------------------------------------
  const issues = () => session.validation().issues;
  const issueText = (issue: PreparationIssue): string => {
    switch (issue.code) {
      case "runtimeUnavailable":
        return "OrcaSlicer isn't ready to slice.";
      case "stale":
        return "The Model changed since this Preparation was made.";
      case "presetNotFound":
        if (issue.name === null) return issue.preset === "process" ? "Choose a quality preset." : "Choose a filament preset.";
        return `The ${issue.preset === "process" ? "quality" : "filament"} preset "${issue.name}" isn't offered for this target.`;
      case "filamentIncompatible":
        return `The filament preset "${issue.name}" isn't made for this target.`;
      case "emptyPlate":
        return `${plateName(issue.plateKey)} has no objects.`;
      case "outOfBounds":
        return `${session.instanceName(issue.instanceKey)} is outside the bed (${plateName(issue.plateKey)}).`;
      case "inExcludeArea":
        return `${session.instanceName(issue.instanceKey)} is in an exclude area (${plateName(issue.plateKey)}).`;
      case "tooTall":
        return `${session.instanceName(issue.instanceKey)} is too tall for this printer (${plateName(issue.plateKey)}).`;
    }
  };
  const goToIssue = (issue: PreparationIssue) => {
    switch (issue.code) {
      case "runtimeUnavailable":
        settingsButton?.focus();
        return;
      case "stale":
        focusStaleBanner();
        return;
      case "presetNotFound":
        focusField(issue.preset === "process" ? "quality" : "material");
        return;
      case "filamentIncompatible":
        focusField("material");
        return;
      case "emptyPlate":
        focusPlate(issue.plateKey);
        return;
      default:
        focusObject(issue.plateKey, issue.instanceKey);
    }
  };

  // --- Unprintable build items (D5: skipped and listed) --------------------------
  const unprintable = createMemo(() => {
    const items = session.geometry()?.buildItems ?? [];
    const keys = [...new Set(items.filter((item) => !item.printable).map((item) => item.objectKey))];
    return keys.map((key) => session.objectName(key));
  });

  // --- Slicing -----------------------------------------------------------------
  const runtime = () => slicing.runtime();
  const runtimeMissing = () => runtime()?.canSlice === false;
  const [starting, setStarting] = createSignal(false);
  const [attempt, setAttempt] = createSignal<SliceAttempt | undefined>();
  const [sliceError, setSliceError] = createSignal<SliceErrorView | undefined>();

  const currentPlateKeys = () => {
    const key = session.plateKey();
    return key ? [key] : [];
  };
  const allPlateKeys = () => plates().map((plate) => plate.plateKey);

  /** Why these plates can't be sliced now, or `undefined`. Shown as text
   *  (Accessibility: a disabled action shows its reason). */
  const blocker = (plateKeys: string[]): string | undefined => {
    if (starting()) return "Starting the slice…";
    const record = session.record();
    if (!record || !document()) return "The Preparation isn't loaded.";
    // One slice per plate at a time: a second would queue a duplicate.
    const busy = [...new Set(slicing.operationsForPreparation(record.id)
      .filter((operation) => (operation.state === "queued" || operation.state === "running") && plateKeys.includes(operation.plateKey))
      .map((operation) => operation.plateKey))];
    if (busy.length > 0) return `Already slicing ${busy.map(plateName).join(", ")}.`;
    if (!runtime()) return "Checking for OrcaSlicer…";
    if (plateKeys.length === 0) return "There is no plate to slice.";
    if (session.loadFailed()) return "The Model's geometry didn't load.";
    const optionsError = session.optionsError();
    if (optionsError) return `The slice options didn't load: ${optionsError}`;
    if (!session.options()) return "Waiting for the slice options.";
    // Until every placement is checked, the placement issues may be out
    // of date (a turned or scaled object is checked in a later task).
    const wanted = new Set(plateKeys);
    const instances = plates().filter((plate) => wanted.has(plate.plateKey)).flatMap((plate) => plate.instances);
    if (instances.some((instance) => !session.meshes().has(instance.objectKey))) return "Waiting for the Model's geometry.";
    if (instances.some((instance) => session.checkingPlacement(instance))) return "Checking placement…";
    const blocking = issuesForPlates(issues(), plateKeys);
    if (blocking.length > 0) {
      return blocking.length === 1 ? "Fix the issue listed above first." : `Fix the ${blocking.length} issues listed above first.`;
    }
    if (session.editor.dirty()) return "Saving your changes…";
    return undefined;
  };
  const plateBlocker = () => blocker(currentPlateKeys());
  const allBlocker = () => blocker(allPlateKeys());
  const reasons = () => {
    const plate = plateBlocker();
    const all = allBlocker();
    if (plate === all) return plate ? [plate] : [];
    return [
      ...(plate ? [`Slice plate: ${plate}`] : []),
      ...(all ? [`Slice all plates: ${all}`] : []),
    ];
  };

  const slice = async (plateKeys: string[], retry?: SliceAttempt) => {
    const record = session.record();
    if (!record) return;
    const operationId = retry?.operationId ?? crypto.randomUUID();
    setAttempt({ operationId, plateKeys });
    setStarting(true);
    setSliceError(undefined);
    try {
      // `start_slice` sends the held revision, so the edits go first. A
      // notice left from before (dismissed or not) doesn't stop the slice;
      // only a save that fails now does.
      const noticeBefore = session.editor.notice();
      await session.editor.flush();
      if (disposed) return;
      const noticeAfter = session.editor.notice();
      if (noticeAfter && noticeAfter !== noticeBefore) {
        setSliceError({
          message: "Your latest changes weren't saved, so nothing was sliced.",
          stale: false,
          openSettings: false,
          retry: false,
          // The workspace's notice already says why, as an alert.
          announced: true,
        });
        setAttempt(undefined);
        return;
      }
      const continueWith = session.record()?.stale ? session.continueWithSourceRevision() : undefined;
      await startSlice(record.id, plateKeys, {
        operationId,
        ...(continueWith ? { continueWithSourceRevision: continueWith } : {}),
      });
      if (!disposed) setAttempt(undefined);
    } catch (error) {
      if (disposed) return;
      const view = sliceErrorView(error);
      // The store may not know yet: reload it so the stale banner shows.
      if (view.stale) refreshSlicing();
      setSliceError(view);
      if (!view.retry) setAttempt(undefined);
    } finally {
      if (!disposed) setStarting(false);
    }
  };

  const runtimeView = () => {
    const held = runtime();
    if (!held) return undefined;
    const lines = held.engine.state === "available" ? [] : candidateLines(held);
    return {
      problems: runtimeProblems(held),
      candidates: lines,
      nothingTried: held.engine.state !== "available" && lines.length === 0,
    };
  };

  return (
    <div ref={panel} class={styles.panel}>
      <Show when={document()}>
        {(shown) => (
          <PreparationControls
            session={session}
            document={shown()}
            advancedOpen={advancedOpen()}
            onAdvancedOpenChange={setAdvancedOpen}
          />
        )}
      </Show>

      <Show when={unprintable().length > 0}>
        <section class={styles.section} aria-labelledby="preparation-not-placed">
          <h3 id="preparation-not-placed" class={styles.heading}>Not placed</h3>
          <p class={styles.note}>
            Marked not printable in the file, so {unprintable().length === 1 ? "it isn't" : "they aren't"} sliced:{" "}
            {unprintable().join(", ")}.
          </p>
        </section>
      </Show>

      <section class={styles.section} aria-labelledby="preparation-issues">
        <h3 id="preparation-issues" class={styles.heading}>
          Issues<Show when={issues().length > 0}> ({issues().length})</Show>
        </h3>
        <Show when={issues().length > 0} fallback={<p class={styles.note}>No issues found.</p>}>
          <ul class={styles.issues}>
            <For each={issues()}>
              {(issue) => (
                <li>
                  <button type="button" class={styles.issue} onClick={() => goToIssue(issue)}>
                    <IconAlertTriangle class={styles.issueIcon} size={14} aria-hidden="true" />
                    <span>{issueText(issue)}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </section>

      <section class={styles.section} aria-labelledby="preparation-slice">
        <h3 id="preparation-slice" class={styles.heading}>Slice</h3>
        <Show
          when={runtimeMissing() && runtimeView()}
          fallback={
            <>
              <div class={styles.sliceActions}>
                <Button
                  variant="primary"
                  size="sm"
                  disabled={plateBlocker() !== undefined}
                  aria-describedby={plateBlocker() ? reasonId : undefined}
                  onClick={() => void slice(currentPlateKeys())}
                >
                  Slice plate
                </Button>
                <Button
                  variant="secondary"
                  size="sm"
                  disabled={allBlocker() !== undefined}
                  aria-describedby={allBlocker() ? reasonId : undefined}
                  onClick={() => void slice(allPlateKeys())}
                >
                  Slice all plates
                </Button>
              </div>
              <Show when={session.plateKey()}>
                {(key) => <p class={styles.note}>Slice plate slices {plateName(key())}.</p>}
              </Show>
              <Show when={reasons().length > 0}>
                <div id={reasonId} class={styles.reasons}>
                  <For each={reasons()}>{(reason) => <p class={styles.note}>{reason}</p>}</For>
                </div>
              </Show>
            </>
          }
        >
          {(view) => (
            <div class={styles.runtime} role="region" aria-label="OrcaSlicer unavailable">
              <p class={styles.runtimeTitle}>Slicing needs OrcaSlicer, which isn't ready.</p>
              <For each={view().problems}>{(problem) => <p class={styles.note}>{problem}</p>}</For>
              <Show when={view().candidates.length > 0}>
                <p class={styles.note}>farm3d tried:</p>
                <ul class={styles.candidates}>
                  <For each={view().candidates}>{(line) => <li>{line}</li>}</For>
                </ul>
              </Show>
              <Show when={view().nothingTried}>
                <p class={styles.note}>{NOTHING_TRIED}</p>
              </Show>
              <div>
                <Button ref={settingsButton} variant="primary" size="sm" onClick={(event) => openSlicerSettings(event.currentTarget)}>
                  Open Slicer settings
                </Button>
              </div>
            </div>
          )}
        </Show>

        <Show when={sliceError()}>
          {(error) => (
            <div class={styles.sliceError} role={error().announced ? undefined : "alert"}>
              <p class={styles.errorText}>{error().message}</p>
              <Show when={error().stale}>
                <p class={styles.note}>
                  Reload the Preparation onto the Model's current revision, or choose to continue with the one it uses,
                  in the Source changed banner above.
                </p>
              </Show>
              <div class={styles.errorActions}>
                <Show when={error().stale}>
                  <Button variant="secondary" size="sm" onClick={focusStaleBanner}>Go to the Source changed banner</Button>
                </Show>
                <Show when={error().field}>
                  {(field) => (
                    <Button variant="secondary" size="sm" onClick={() => focusField(field())}>
                      Go to {FIELD_LABELS[field()]}
                    </Button>
                  )}
                </Show>
                <Show when={error().plateKey}>
                  {(plateKey) => (
                    <Button variant="secondary" size="sm" onClick={() => focusPlate(plateKey())}>
                      Show {plateName(plateKey())}
                    </Button>
                  )}
                </Show>
                <Show when={error().openSettings}>
                  <Button variant="secondary" size="sm" onClick={(event) => openSlicerSettings(event.currentTarget)}>Open Slicer settings</Button>
                </Show>
                <Show when={error().retry && attempt()}>
                  {(held) => (
                    <Button variant="primary" size="sm" disabled={starting()} onClick={() => void slice(held().plateKeys, held())}>
                      Try again
                    </Button>
                  )}
                </Show>
                <Button variant="ghost" size="sm" onClick={() => setSliceError(undefined)}>Dismiss</Button>
              </div>
            </div>
          )}
        </Show>
      </section>

      <Show when={session.record()}>
        {(record) => <SliceOperationPanel
            preparationId={record().id}
            onOpenRevision={props.onOpenRevision}
            onFailure={session.revealPanel}
          />}
      </Show>
    </div>
  );
}
