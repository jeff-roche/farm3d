import { createRoot, createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { createPreparationEditor, SAVE_DEBOUNCE_MS, type PreparationEditor } from "./preparation-editor";
import { renamePlate } from "./preparation-edits";
import { preparation } from "./test-records";
import type { PreparationDocument, PreparationRecord } from "./types";

function commandError(code: string, message = code) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false };
}

/** A stand-in for the store: holds a record, and saves the way
 *  `updatePreparation` settles its result. */
function harness() {
  const [record, setRecord] = createSignal<PreparationRecord | undefined>(preparation({ revision: 1 }));
  const saves: { document: PreparationDocument; expectedRevision: number }[] = [];
  const save = vi.fn(async (_id: string, document: PreparationDocument) => {
    const held = record()!;
    saves.push({ document, expectedRevision: held.revision });
    const next = { ...held, document, revision: held.revision + 1 };
    setRecord(next);
    return next;
  });
  let editor!: PreparationEditor;
  let dispose!: () => void;
  createRoot((d) => {
    dispose = d;
    editor = createPreparationEditor({ preparation: record, save });
  });
  return { record, setRecord, save, saves, editor, dispose };
}

const named = (name: string) => (document: PreparationDocument) => renamePlate(document, "plt-1", name);

async function settle() {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

describe("createPreparationEditor", () => {
  beforeEach(() => vi.useFakeTimers());
  afterEach(() => vi.useRealTimers());

  it("shows an edit at once and saves the latest document once, after the pause", async () => {
    const h = harness();
    h.editor.edit(named("A"));
    h.editor.edit(named("B"));
    expect(h.editor.document()?.plates[0].name).toBe("B");
    expect(h.editor.dirty()).toBe(true);
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS - 1);
    expect(h.save).not.toHaveBeenCalled();
    vi.advanceTimersByTime(1);
    await settle();
    expect(h.saves).toEqual([{ document: expect.objectContaining({ plates: [expect.objectContaining({ name: "B" })] }), expectedRevision: 1 }]);
    // Confirmed: what shows is the store's record again.
    expect(h.editor.dirty()).toBe(false);
    expect(h.editor.document()).toBe(h.record()!.document);
    h.dispose();
  });

  it("ignores a change that changes nothing", () => {
    const h = harness();
    h.editor.edit((document) => document);
    expect(h.editor.dirty()).toBe(false);
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    expect(h.save).not.toHaveBeenCalled();
  });

  it("saves edits made during a save afterwards, on the revision that save produced", async () => {
    const h = harness();
    let release!: () => void;
    h.save.mockImplementationOnce(async (_id, document) => {
      await new Promise<void>((resolve) => { release = resolve; });
      const next = { ...h.record()!, document, revision: 2 };
      h.setRecord(next);
      return next;
    });
    h.editor.edit(named("A"));
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    await settle();
    h.editor.edit(named("B"));
    expect(h.editor.document()?.plates[0].name).toBe("B");
    release();
    await settle();
    // Still showing the newer local edit, not the confirmed "A".
    expect(h.editor.document()?.plates[0].name).toBe("B");
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    await settle();
    expect(h.saves).toEqual([{ document: expect.anything(), expectedRevision: 2 }]);
    expect(h.record()!.document.plates[0].name).toBe("B");
    expect(h.editor.dirty()).toBe(false);
  });

  it("on CONFLICT drops the local edits for the latest record, with a notice", async () => {
    const h = harness();
    h.save.mockImplementationOnce(async () => {
      // The store reloads on CONFLICT; here, the newer record arrives.
      h.setRecord(preparation({ revision: 5, document: { ...preparation().document, processPreset: "Theirs" } }));
      throw commandError("CONFLICT");
    });
    h.editor.edit(named("Mine"));
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    await settle();
    expect(h.editor.notice()).toEqual({ kind: "conflict" });
    expect(h.editor.dirty()).toBe(false);
    expect(h.editor.document()?.processPreset).toBe("Theirs");
    expect(h.editor.document()?.plates[0].name).toBeUndefined();
    // The next edit starts from the latest record, and clears the notice.
    h.editor.edit(named("Again"));
    expect(h.editor.notice()).toBeUndefined();
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    await settle();
    expect(h.saves[h.saves.length - 1]).toMatchObject({ expectedRevision: 5, document: { processPreset: "Theirs" } });
  });

  it("never writes pending edits over a record that changed meanwhile", async () => {
    const h = harness();
    h.editor.edit(named("Mine"));
    h.setRecord(preparation({ revision: 2, document: { ...preparation().document, processPreset: "Theirs" } }));
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    await settle();
    expect(h.save).not.toHaveBeenCalled();
    expect(h.editor.notice()).toEqual({ kind: "conflict" });
    expect(h.editor.document()?.processPreset).toBe("Theirs");
  });

  it("reverts to the saved record when the save is refused", async () => {
    const h = harness();
    h.save.mockRejectedValueOnce(commandError("VALIDATION", "A plate name is at most 128 characters."));
    h.editor.edit(named("Bad"));
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    await settle();
    expect(h.editor.notice()).toEqual({ kind: "saveFailed", message: "A plate name is at most 128 characters." });
    expect(h.editor.document()).toBe(h.record()!.document);
    h.editor.dismissNotice();
    expect(h.editor.notice()).toBeUndefined();
  });

  it("flush saves now, and dispose saves what is pending", async () => {
    const h = harness();
    h.editor.edit(named("Now"));
    await h.editor.flush();
    expect(h.saves).toHaveLength(1);
    h.editor.edit(named("Leaving"));
    h.editor.dispose();
    await settle();
    expect(h.saves).toHaveLength(2);
    expect(h.record()!.document.plates[0].name).toBe("Leaving");
  });

  it("on dispose during a save, saves the newer edits after it, in order, losing nothing", async () => {
    const h = harness();
    let release!: () => void;
    const gate = new Promise<void>((resolve) => { release = resolve; });
    const settleSave = h.save.getMockImplementation()!;
    h.save.mockImplementationOnce(async (id, document) => {
      await gate;
      return settleSave(id, document);
    });
    h.editor.edit(named("First"));
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS);
    await settle();
    expect(h.save).toHaveBeenCalledTimes(1);
    // A newer edit while the first save is in flight, then leaving.
    h.editor.edit(named("Second"));
    h.editor.dispose();
    await settle();
    expect(h.save).toHaveBeenCalledTimes(1);
    release();
    await settle();
    expect(h.saves.map((saved) => [saved.document.plates[0].name, saved.expectedRevision])).toEqual([
      ["First", 1],
      ["Second", 2],
    ]);
    expect(h.record()!.document.plates[0].name).toBe("Second");
    // Nothing more is scheduled once disposed.
    vi.advanceTimersByTime(SAVE_DEBOUNCE_MS * 4);
    await settle();
    expect(h.save).toHaveBeenCalledTimes(2);
  });

  it("reports a save refused after dispose with a warning, since nothing shows the notice", async () => {
    const h = harness();
    const warn = vi.spyOn(console, "warn").mockImplementation(() => {});
    h.save.mockRejectedValueOnce(commandError("VALIDATION", "A plate name is at most 128 characters."));
    h.editor.edit(named("Bad"));
    h.editor.dispose();
    await settle();
    expect(warn).toHaveBeenCalledWith(
      "Preparation edits made before leaving were not saved:",
      "A plate name is at most 128 characters.",
    );
    warn.mockRestore();
  });
});
