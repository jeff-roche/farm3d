/** Optimistic editing of one Preparation (D19 **Saving**). Edits show at
 *  once and save through `update_preparation`, debounced
 *  {@link SAVE_DEBOUNCE_MS}. The store stays the only owner of saved data:
 *  the editor holds just the one unsaved document on top of the store's
 *  record, and drops it as soon as the store has what it sent. So what is
 *  shown is always either the store's record or that record plus the
 *  user's own pending edits, and a server-confirmed state is never
 *  replaced by anything but a newer one.
 *
 *  **Conflicts (D5).** A save sends the record's `revision` as
 *  `expectedRevision`. If the store's record has moved on since the pending
 *  edits began (another window, a reload), or the backend answers
 *  `CONFLICT`, the pending edits are dropped rather than written over the
 *  newer document: the store reloads, the latest record shows, and a
 *  notice says the edits were not saved. */
import { batch, createSignal, untrack } from "solid-js";
import { isCommandError } from "../ipc/client";
import type { PreparationDocument, PreparationRecord } from "./types";

/** D19: edits save after this long without another. */
export const SAVE_DEBOUNCE_MS = 500;

export type EditorNotice =
  /** The Preparation changed elsewhere; the local edits were dropped. */
  | { kind: "conflict" }
  /** The backend refused the save (e.g. `VALIDATION`); the edits were
   *  dropped. */
  | { kind: "saveFailed"; message: string };

export interface PreparationEditor {
  /** The document to show: the pending edits, else the store's record. */
  document(): PreparationDocument | undefined;
  /** Applies `change` to the shown document. A change that returns the
   *  same document is a no-op. */
  edit(change: (document: PreparationDocument) => PreparationDocument): void;
  /** Saves pending edits now (e.g. before a reload), and resolves once
   *  nothing is pending or in flight. */
  flush(): Promise<void>;
  /** Unsaved or in-flight edits exist. */
  dirty(): boolean;
  saving(): boolean;
  notice(): EditorNotice | undefined;
  dismissNotice(): void;
  /** Saves what is pending without waiting (leaving the workspace). */
  dispose(): void;
}

export interface PreparationEditorOptions {
  /** The store's record (`slicing.preparation(modelId)`). */
  preparation: () => PreparationRecord | undefined;
  /** `updatePreparation` from the store: it sends the held record's
   *  `revision` as `expectedRevision` and settles the result. */
  save: (preparationId: string, document: PreparationDocument) => Promise<PreparationRecord>;
  debounceMs?: number;
}

interface Pending {
  document: PreparationDocument;
  /** The record revision the edits were made on. */
  baseRevision: number;
}

export function createPreparationEditor(options: PreparationEditorOptions): PreparationEditor {
  const debounceMs = options.debounceMs ?? SAVE_DEBOUNCE_MS;
  const [pending, setPending] = createSignal<Pending | null>(null);
  const [saving, setSaving] = createSignal(false);
  const [notice, setNotice] = createSignal<EditorNotice | undefined>();
  let timer: ReturnType<typeof setTimeout> | undefined;
  let inFlight: Promise<void> | undefined;

  const clearTimer = () => {
    if (timer !== undefined) clearTimeout(timer);
    timer = undefined;
  };
  const schedule = () => {
    clearTimer();
    timer = setTimeout(() => void flush(), debounceMs);
  };
  const drop = (reason: EditorNotice) => {
    clearTimer();
    batch(() => {
      setPending(null);
      setNotice(reason);
    });
  };

  async function saveOnce(): Promise<void> {
    const edits = untrack(pending);
    if (!edits) return;
    const record = untrack(options.preparation);
    if (!record) {
      setPending(null);
      return;
    }
    if (record.revision !== edits.baseRevision) {
      drop({ kind: "conflict" });
      return;
    }
    const sent = edits.document;
    setSaving(true);
    try {
      const saved = await options.save(record.id, sent);
      const now = untrack(pending);
      if (now?.document === sent) setPending(null);
      else if (now) setPending({ document: now.document, baseRevision: saved.revision });
    } catch (error) {
      if (isCommandError(error) && error.code === "CONFLICT") drop({ kind: "conflict" });
      else drop({ kind: "saveFailed", message: isCommandError(error) ? error.message : "The change could not be saved." });
    } finally {
      setSaving(false);
    }
  }

  async function flush(): Promise<void> {
    clearTimer();
    while (inFlight) await inFlight;
    if (!untrack(pending)) return;
    inFlight = saveOnce().finally(() => {
      inFlight = undefined;
    });
    await inFlight;
    // Edits made while that save was in flight go next, after the usual
    // pause.
    if (untrack(pending) && timer === undefined) schedule();
  }

  return {
    document: () => pending()?.document ?? options.preparation()?.document,
    edit(change) {
      const record = untrack(options.preparation);
      if (!record) return;
      const held = untrack(pending);
      const current = held?.document ?? record.document;
      const next = change(current);
      if (next === current) return;
      batch(() => {
        setPending({ document: next, baseRevision: held?.baseRevision ?? record.revision });
        setNotice(undefined);
      });
      if (!inFlight) schedule();
    },
    flush,
    dirty: () => pending() !== null || saving(),
    saving,
    notice,
    dismissNotice: () => setNotice(undefined),
    dispose() {
      if (untrack(pending)) void flush();
      else clearTimer();
    },
  };
}
