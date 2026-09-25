import { createEffect, createMemo, createSignal, createUniqueId, For, Match, on, onCleanup, Show, Switch } from "solid-js";
import { Button, Progress } from "../design-system";
import { isCommandError } from "../ipc/client";
import { failureText, logSegments, operationStateLabel } from "../slicing/slice-presentation";
import { cancelSliceOperation, loadOperationLog, slicing } from "../slicing/slicing-store";
import type { SliceOperationLog, SliceOperationRecord } from "../slicing/types";
import styles from "./SliceOperationPanel.module.css";

/** How many operations show before **Show all**. The store keeps finished
 *  operations until the next snapshot, so the list can grow. */
export const RECENT_OPERATION_LIMIT = 10;

export interface SliceOperationPanelProps {
  preparationId: string;
  /** An operation shown here failed: its host shows the panel, so the log
   *  can take focus. */
  onFailure?: () => void;
  /** Opens a published Slice Revision's review, when the host has one. */
  onOpenRevision?: (sliceRevisionId: string) => void;
}

function newestFirst(a: SliceOperationRecord, b: SliceOperationRecord): number {
  if (a.queuedAt !== b.queuedAt) return a.queuedAt < b.queuedAt ? 1 : -1;
  return a.id < b.id ? 1 : a.id > b.id ? -1 : 0;
}

/** D20's operation rows for one Preparation, newest first: state, message,
 *  progress, **Cancel**, and each one's log. */
export function SliceOperationPanel(props: SliceOperationPanelProps) {
  const [showAll, setShowAll] = createSignal(false);
  const operations = createMemo(() => [...slicing.operationsForPreparation(props.preparationId)].sort(newestFirst));
  // Rows are keyed by operation id, so a row lives on as its operation
  // moves forward (and sees it fail), even when a backfill replaces the
  // records; only a new or dropped operation adds or removes a row.
  const shown = createMemo(
    () => (showAll() ? operations() : operations().slice(0, RECENT_OPERATION_LIMIT)).map((record) => record.id),
    [],
    { equals: (a, b) => a.length === b.length && a.every((id, index) => id === b[index]) },
  );

  return (
    <section class={styles.panel} aria-labelledby="slice-operations-heading">
      <h3 id="slice-operations-heading" class={styles.heading}>Slices</h3>
      <Show when={operations().length > 0} fallback={<p class={styles.note}>Nothing sliced yet.</p>}>
        <ol class={styles.list}>
          <For each={shown()}>
            {(id) => <SliceOperationRow id={id} onOpenRevision={props.onOpenRevision} onFailure={props.onFailure} />}
          </For>
        </ol>
        <Show when={operations().length > RECENT_OPERATION_LIMIT}>
          <Button variant="ghost" size="sm" onClick={() => setShowAll((all) => !all)}>
            {showAll() ? "Show recent only" : `Show all ${operations().length}`}
          </Button>
        </Show>
      </Show>
    </section>
  );
}

type LogState =
  | { status: "idle" }
  | { status: "loading" }
  | { status: "loaded"; log: SliceOperationLog }
  | { status: "failed"; message: string };

function plateTitle(operation: SliceOperationRecord): string {
  return operation.plateName ? `Plate ${operation.plateIndex}: ${operation.plateName}` : `Plate ${operation.plateIndex}`;
}

function SliceOperationRow(props: { id: string; onOpenRevision?: (id: string) => void; onFailure?: () => void }) {
  // The last record held: a row being removed may outlive its record.
  const operation = createMemo<SliceOperationRecord>((held) => slicing.operation(props.id) ?? held!);
  const active = () => operation().state === "queued" || operation().state === "running";
  const progress = () => slicing.progress(props.id);
  const percent = () => progress()?.totalPercent ?? progress()?.platePercent;
  const message = () => progress()?.message ?? (operation().state === "queued" ? "Waiting to start…" : "Slicing…");

  const failureId = createUniqueId();
  const logId = createUniqueId();
  let logElement: HTMLPreElement | undefined;
  let disposed = false;
  onCleanup(() => { disposed = true; });

  // D11/D20: a failed operation's log is shown; one that fails while
  // shown here also takes focus, with the failure text above it.
  const [logOpen, setLogOpen] = createSignal(operation().state === "failed");
  createEffect(on(() => operation().state, (state, previous) => {
    if (state !== "failed" || previous === undefined || previous === "failed") return;
    setLogOpen(true);
    props.onFailure?.();
    queueMicrotask(() => logElement?.focus());
  }));

  // --- Log -------------------------------------------------------------------
  const [log, setLog] = createSignal<LogState>({ status: "idle" });
  const loadedLog = () => {
    const held = log();
    return held.status === "loaded" ? held.log : undefined;
  };
  const logFailure = () => {
    const held = log();
    return held.status === "failed" ? held.message : undefined;
  };
  let request = 0;
  const loadLog = async (): Promise<SliceOperationLog | undefined> => {
    const id = ++request;
    setLog({ status: "loading" });
    try {
      const loaded = await loadOperationLog(props.id);
      if (disposed || id !== request) return loaded;
      setLog({ status: "loaded", log: loaded });
      return loaded;
    } catch (error) {
      // A succeeded operation's log goes with its deleted Slice Revision;
      // the backend says so as NOT_FOUND.
      if (!disposed && id === request) {
        setLog({ status: "failed", message: isCommandError(error) ? error.message : "The log couldn't be loaded." });
      }
      return undefined;
    }
  };
  // Loaded when shown, and again when the operation moves on: the backend
  // stores the log only when the operation finishes, so until then it is
  // empty.
  createEffect(on([logOpen, () => operation().state], ([open]) => {
    if (open) void loadLog();
  }));

  const [copyStatus, setCopyStatus] = createSignal<string | undefined>();
  const copyLog = async () => {
    setCopyStatus(undefined);
    const loaded = loadedLog() ?? await loadLog();
    if (disposed) return;
    if (!loaded) {
      setCopyStatus("The log couldn't be loaded, so nothing was copied.");
      return;
    }
    if (loaded.text === "") {
      setCopyStatus(`There's nothing to copy. ${emptyLogText()}`);
      return;
    }
    try {
      await navigator.clipboard.writeText(loaded.text);
      if (!disposed) setCopyStatus("Log copied.");
    } catch {
      if (!disposed) setCopyStatus("The log couldn't be copied.");
    }
  };

  // --- Cancel ------------------------------------------------------------------
  const [cancelling, setCancelling] = createSignal(false);
  const [cancelError, setCancelError] = createSignal<string | undefined>();
  const cancel = async () => {
    setCancelling(true);
    setCancelError(undefined);
    try {
      await cancelSliceOperation(props.id);
    } catch (error) {
      if (!disposed) setCancelError(isCommandError(error) ? error.message : "The slice couldn't be cancelled.");
    } finally {
      if (!disposed) setCancelling(false);
    }
  };

  /** Why a loaded log is empty: not saved yet, or never written (the
   *  slice was cancelled or interrupted before it ran). */
  const emptyLogText = () => (active() ? "The log is saved when the slice finishes." : "This slice left no log.");

  const segments = createMemo(() => {
    const held = loadedLog();
    return held ? logSegments(held.text, held.noiseLines) : [];
  });

  return (
    <li class={styles.row} data-state={operation().state}>
      <div class={styles.rowHeader}>
        <span class={styles.plate}>{plateTitle(operation())}</span>
        <span class={styles.state}>{operationStateLabel(operation().state)}</span>
      </div>

      <Switch>
        <Match when={active()}>
          <Progress
            label={message()}
            value={percent()}
            indeterminate={percent() === undefined}
            showValue={percent() !== undefined}
            valueLabel={percent() === undefined ? undefined : `${Math.round(percent()!)}%`}
          />
          <Show when={progress()?.warning}>{(warning) => <p class={styles.warning}>{warning()}</p>}</Show>
          <div class={styles.actions}>
            <Button variant="secondary" size="sm" disabled={cancelling()} onClick={() => void cancel()}>
              {cancelling() ? "Cancelling…" : "Cancel"}
            </Button>
          </div>
        </Match>
        <Match when={operation().state === "failed"}>
          <p id={failureId} class={styles.failure}>
            {operation().failure ? failureText(operation().failure!) : "The slice failed."}
          </p>
        </Match>
        <Match when={operation().state === "succeeded" && !operation().sliceRevisionId}>
          <p class={styles.note}>Its Slice Revision was deleted.</p>
        </Match>
        <Match when={operation().state === "succeeded"}>
          <p class={styles.note}>Saved as a Slice Revision.</p>
          <Show when={props.onOpenRevision && operation().sliceRevisionId}>
            {(revisionId) => (
              <Button variant="ghost" size="sm" onClick={() => props.onOpenRevision?.(revisionId())}>
                Open the Slice Revision
              </Button>
            )}
          </Show>
        </Match>
        <Match when={operation().state === "cancelled"}>
          <p class={styles.note}>Cancelled. Nothing was saved.</p>
        </Match>
        <Match when={operation().state === "interrupted"}>
          <p class={styles.note}>farm3d closed before this finished. Slice again to retry.</p>
        </Match>
      </Switch>
      <Show when={cancelError()}>{(text) => <p class={styles.failure} role="alert">{text()}</p>}</Show>

      <div class={styles.actions}>
        <Button
          variant="ghost"
          size="sm"
          aria-expanded={logOpen()}
          aria-controls={logOpen() ? logId : undefined}
          onClick={() => setLogOpen((open) => !open)}
        >
          {logOpen() ? "Hide log" : "Show log"}
        </Button>
        <Button variant="ghost" size="sm" onClick={() => void copyLog()}>Copy log</Button>
        <span class={styles.note} role="status">{copyStatus()}</span>
      </div>
      <Show when={logOpen()}>
        <div class={styles.logArea}>
          <Show when={logFailure()}>{(message) => <p class={styles.note}>{message()}</p>}</Show>
          <Show when={loadedLog()?.text === ""}>
            <p class={styles.note}>{emptyLogText()}</p>
          </Show>
          <Show when={loadedLog()?.truncated}>
            <p class={styles.note}>The middle of this log was left out because it was too long.</p>
          </Show>
          <Show when={(loadedLog()?.noiseLines.length ?? 0) > 0}>
            <p class={styles.note}>Dimmed lines are known, harmless messages.</p>
          </Show>
          <pre
            ref={logElement}
            id={logId}
            class={styles.log}
            tabIndex={0}
            aria-label={`Log for ${plateTitle(operation())}`}
            aria-describedby={operation().state === "failed" ? failureId : undefined}
            aria-busy={log().status === "loading"}
          >
            <Show when={log().status === "loaded"} fallback={log().status === "loading" ? "Loading the log…" : ""}>
              <For each={segments()}>
                {(segment) => (segment.noise
                  ? <span class={styles.noise} data-noise="">{segment.text}</span>
                  : segment.text)}
              </For>
            </Show>
          </pre>
        </div>
      </Show>
    </li>
  );
}
