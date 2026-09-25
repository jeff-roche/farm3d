import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { encodeMeshBuffer } from "./mesh-buffer";
import { operation, preparation, runtimeStatus, sliceRevision, slicingSnapshot } from "./test-records";
import type { SlicingEvent, SlicingEventType } from "./types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const eventMock = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => eventMock);

const STREAM = "stream-slicing";

type Handler = (event: { payload: unknown }) => void;
type Responder = (args: Record<string, unknown>) => unknown;

let handler: Handler;
let unlisten: ReturnType<typeof vi.fn>;
let calls: string[];
let responders: Record<string, Responder>;
/** Raw-byte commands answer without the JSON envelope. */
const RAW = new Set(["get_revision_mesh"]);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  eventMock.listen.mockReset();
  tauriMock.isTauri.mockReturnValue(true);
  calls = [];
  handler = () => {};
  unlisten = vi.fn();
  responders = { list_slicing: () => slicingSnapshot(0) };
  eventMock.listen.mockImplementation((name: string, cb: Handler) => {
    calls.push(`listen:${name}`);
    handler = cb;
    return Promise.resolve(unlisten);
  });
  tauriMock.invoke.mockImplementation((name: string, args: Record<string, unknown>) => {
    calls.push(`invoke:${name}`);
    const respond = responders[name];
    if (!respond) return Promise.reject(new Error(`unexpected command ${name}`));
    try {
      const data = respond(args);
      const wrap = (value: unknown) => (RAW.has(name) ? value : { contractVersion: 1, data: value });
      return data instanceof Promise ? data.then(wrap) : Promise.resolve(wrap(data));
    } catch (error) {
      return Promise.reject(error);
    }
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

const SUBJECT_KIND: Record<string, string> = {
  runtime: "runtime", preparation: "preparation", operation: "sliceOperation", revision: "sliceRevision",
};

function envelope(sequence: number, type: SlicingEventType | string, payload: unknown, subjectId = "x", streamId = STREAM) {
  return {
    contractVersion: 1,
    streamId,
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-24T00:00:00Z",
    type,
    subject: { kind: SUBJECT_KIND[type.split(".")[1]] ?? "x", id: subjectId },
    payload,
  } as SlicingEvent;
}

function emit(event: SlicingEvent): void {
  handler({ payload: event });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 20; i += 1) await Promise.resolve();
}

function commandError(code: string, message: string, extra: Record<string, unknown> = {}) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false, ...extra };
}

async function startedStore() {
  const store = await import("./slicing-store");
  await store.startSlicing();
  return store;
}

const backfills = () => tauriMock.invoke.mock.calls.filter(([name]) => name === "list_slicing").length;

describe("startSlicing (desktop)", () => {
  it("listens before backfilling, drops buffered events the snapshot covers, and replays later ones in order", async () => {
    const backfill = deferred<unknown>();
    responders.list_slicing = () => backfill.promise;
    const { startSlicing, slicing } = await import("./slicing-store");
    const started = startSlicing();
    await flush();
    expect(slicing.status()).toBe("loading");
    expect(slicing.syncState()).toBe("syncing");
    expect(calls.indexOf("listen:farm3d-event-v1")).toBeLessThan(calls.indexOf("invoke:list_slicing"));

    // Arrive while backfilling, out of order: 7 before 6, and 5 is covered.
    const running = operation({ id: "sop-1", state: "running" });
    emit(envelope(7, "slicing.operation.progress", { totalPercent: 40, message: "Slicing" }, "sop-1"));
    emit(envelope(5, "slicing.preparation.changed", preparation({ id: "prp-old", modelId: "mdl-old" }), "prp-old"));
    emit(envelope(6, "slicing.operation.changed", running, "sop-1"));
    backfill.resolve(slicingSnapshot(5, {
      preparations: [preparation({ id: "prp-1", modelId: "mdl-1" })],
      activeAndRecentOperations: [operation({ id: "sop-1", state: "queued" })],
      revisions: [sliceRevision({ id: "slr-1", modelId: "mdl-1" })],
    }));
    await started;

    expect(slicing.status()).toBe("ready");
    expect(slicing.syncState()).toBe("current");
    expect(slicing.runtime()?.engine.state).toBe("available");
    expect(slicing.preparation("mdl-old")).toBeUndefined();
    expect(slicing.preparation("mdl-1")?.id).toBe("prp-1");
    expect(slicing.operation("sop-1")?.state).toBe("running");
    // Progress (7) replayed after the operation became running (6).
    expect(slicing.progress("sop-1")).toEqual({ totalPercent: 40, message: "Slicing" });
    expect(slicing.revisions("mdl-1").map((r) => r.id)).toEqual(["slr-1"]);
  });

  it("ignores every event on the shared channel that is not slicing.*", async () => {
    const { slicing } = await startedStore();
    emit(envelope(1, "library.model.changed", {}, "mdl-1", "stream-library") as unknown as SlicingEvent);
    emit(envelope(9, "printer.status.changed", {}, "prn-1", "stream-status") as unknown as SlicingEvent);
    await flush();
    expect(slicing.syncState()).toBe("current");
    expect(backfills()).toBe(1);
    emit(envelope(1, "slicing.preparation.changed", preparation(), "prp-1"));
    expect(slicing.preparation("mdl-1")?.id).toBe("prp-1");
  });

  it("a sequence gap triggers a new backfill and marks the store uncertain until it settles", async () => {
    const { slicing } = await startedStore();
    const rebackfill = deferred<unknown>();
    responders.list_slicing = () => rebackfill.promise;
    emit(envelope(1, "slicing.preparation.changed", preparation({ id: "prp-1", modelId: "mdl-1" }), "prp-1"));
    emit(envelope(3, "slicing.preparation.changed", preparation({ id: "prp-3", modelId: "mdl-3" }), "prp-3"));
    await flush();

    expect(slicing.syncState()).toBe("uncertain");
    expect(backfills()).toBe(2);
    // Content is kept while uncertain.
    expect(slicing.preparation("mdl-1")?.id).toBe("prp-1");

    rebackfill.resolve(slicingSnapshot(3, {
      preparations: [
        preparation({ id: "prp-1", modelId: "mdl-1" }),
        preparation({ id: "prp-2", modelId: "mdl-2" }),
        preparation({ id: "prp-3", modelId: "mdl-3" }),
      ],
    }));
    await flush();
    expect(slicing.syncState()).toBe("current");
    expect(slicing.preparations().map((p) => p.id).sort()).toEqual(["prp-1", "prp-2", "prp-3"]);
  });

  it("a new streamId (an app restart) triggers a new backfill", async () => {
    const { slicing } = await startedStore();
    responders.list_slicing = () => ({ ...slicingSnapshot(1), streamId: "stream-restarted" });
    emit(envelope(1, "slicing.preparation.changed", preparation(), "prp-1", "stream-restarted"));
    await flush();
    expect(backfills()).toBe(2);
    expect(slicing.syncState()).toBe("current");
  });

  it("a failed first load reports error status, keeps retrying with backoff, then recovers", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    responders.list_slicing = () => { throw commandError("PERSISTENCE_UNAVAILABLE", "Storage is busy."); };
    const { slicing } = await startedStore();
    expect(slicing.status()).toBe("error");
    expect(slicing.syncState()).toBe("uncertain");

    await vi.advanceTimersByTimeAsync(1_000);
    expect(backfills()).toBe(2);
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation()] });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(backfills()).toBe(3);
    expect(slicing.status()).toBe("ready");
    expect(slicing.syncState()).toBe("current");
    expect(slicing.preparation("mdl-1")).toBeDefined();
  });

  it("is idempotent: starting again disposes the previous listener, and the returned disposer unlistens", async () => {
    const first = vi.fn();
    const second = vi.fn();
    const { startSlicing } = await import("./slicing-store");
    unlisten = first;
    await startSlicing();
    unlisten = second;
    const dispose = await startSlicing();
    expect(first).toHaveBeenCalledOnce();
    expect(second).not.toHaveBeenCalled();
    dispose();
    expect(second).toHaveBeenCalledOnce();
  });

  it("refreshSlicing backfills again", async () => {
    const { refreshSlicing, slicing } = await startedStore();
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ id: "prp-fresh" })] });
    refreshSlicing();
    await flush();
    expect(backfills()).toBe(2);
    expect(slicing.preparation("mdl-1")?.id).toBe("prp-fresh");
  });
});

describe("progress (ephemeral)", () => {
  it("keeps only the latest update per operation, as the throttled events arrive", async () => {
    responders.list_slicing = () => slicingSnapshot(0, {
      activeAndRecentOperations: [operation({ id: "sop-1", state: "running" }), operation({ id: "sop-2", state: "running", plateKey: "plt-2" })],
    });
    const { slicing } = await startedStore();
    emit(envelope(1, "slicing.operation.progress", { totalPercent: 1, message: "Loading" }, "sop-1"));
    emit(envelope(2, "slicing.operation.progress", { totalPercent: 10, message: "Slicing" }, "sop-2"));
    emit(envelope(3, "slicing.operation.progress", { totalPercent: 55, platePercent: 80, message: "Generating G-code" }, "sop-1"));
    emit(envelope(4, "slicing.operation.progress", { totalPercent: 60, message: "Generating G-code", warning: "Thin walls" }, "sop-1"));
    expect(slicing.progress("sop-1")).toEqual({ totalPercent: 60, message: "Generating G-code", warning: "Thin walls" });
    expect(slicing.progress("sop-2")).toEqual({ totalPercent: 10, message: "Slicing" });
  });

  it("is cleared when the operation reaches a terminal state, and a late update never brings it back", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { activeAndRecentOperations: [operation({ id: "sop-1", state: "running" })] });
    const { slicing } = await startedStore();
    emit(envelope(1, "slicing.operation.progress", { totalPercent: 90, message: "Exporting" }, "sop-1"));
    expect(slicing.progress("sop-1")).toBeDefined();

    emit(envelope(2, "slicing.operation.changed", operation({ id: "sop-1", state: "succeeded", sliceRevisionId: "slr-1" }), "sop-1"));
    expect(slicing.progress("sop-1")).toBeUndefined();
    emit(envelope(3, "slicing.operation.progress", { totalPercent: 100, message: "Done" }, "sop-1"));
    expect(slicing.progress("sop-1")).toBeUndefined();
  });

  it("is ignored for an operation the store does not hold", async () => {
    const { slicing } = await startedStore();
    emit(envelope(1, "slicing.operation.progress", { message: "Slicing…" }, "sop-unknown"));
    expect(slicing.progress("sop-unknown")).toBeUndefined();
    expect(slicing.syncState()).toBe("current");
  });

  it("is never kept across a backfill", async () => {
    const running = operation({ id: "sop-1", state: "running" });
    responders.list_slicing = () => slicingSnapshot(0, { activeAndRecentOperations: [running] });
    const { slicing, refreshSlicing } = await startedStore();
    emit(envelope(1, "slicing.operation.progress", { totalPercent: 30, message: "Slicing" }, "sop-1"));
    expect(slicing.progress("sop-1")).toBeDefined();

    responders.list_slicing = () => slicingSnapshot(1, { activeAndRecentOperations: [running] });
    refreshSlicing();
    await flush();
    expect(slicing.operation("sop-1")?.state).toBe("running");
    expect(slicing.progress("sop-1")).toBeUndefined();
  });

  it("a progress event still counts toward gap detection", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { activeAndRecentOperations: [operation({ id: "sop-1", state: "running" })] });
    const { slicing } = await startedStore();
    responders.list_slicing = () => slicingSnapshot(2, { activeAndRecentOperations: [operation({ id: "sop-1", state: "running" })] });
    emit(envelope(2, "slicing.operation.progress", { message: "Slicing" }, "sop-1"));
    await flush();
    expect(backfills()).toBe(2);
    expect(slicing.syncState()).toBe("current");
  });
});

describe("settling", () => {
  it("a preparation event applies at an equal revision (stale is derived), an older one never", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ revision: 3 })] });
    const { slicing } = await startedStore();
    emit(envelope(1, "slicing.preparation.changed", preparation({ revision: 3, stale: true }), "prp-1"));
    expect(slicing.preparation("mdl-1")?.stale).toBe(true);
    emit(envelope(2, "slicing.preparation.changed", preparation({ revision: 2, stale: false }), "prp-1"));
    expect(slicing.preparation("mdl-1")).toMatchObject({ revision: 3, stale: true });
  });

  it("an update result applies only when newer, so it never overwrites a newer event", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ revision: 1 })] });
    const update = deferred<unknown>();
    responders.update_preparation = () => update.promise;
    const { updatePreparation, slicing } = await startedStore();
    const document = { ...preparation().document, processPreset: "Mine" };
    const pending = updatePreparation("prp-1", document);
    await flush();
    expect(tauriMock.invoke).toHaveBeenCalledWith("update_preparation", {
      contractVersion: 1, preparationId: "prp-1", expectedRevision: 1, document,
    });
    emit(envelope(1, "slicing.preparation.changed", preparation({ revision: 3, document: { ...document, processPreset: "Theirs" } }), "prp-1"));
    update.resolve(preparation({ revision: 2, document }));
    await pending;
    expect(slicing.preparation("mdl-1")?.document.processPreset).toBe("Theirs");
  });

  it("replaces a record whole, so an optional field the new one lacks is gone", async () => {
    responders.list_slicing = () => slicingSnapshot(0, {
      preparations: [preparation({ revision: 1, document: { ...preparation().document, processPreset: "Old" } })],
    });
    const { slicing } = await startedStore();
    emit(envelope(1, "slicing.preparation.changed", preparation({ revision: 2 }), "prp-1"));
    expect(slicing.preparation("mdl-1")?.document.processPreset).toBeUndefined();
  });

  it("a result for a Preparation already removed by an event does not bring it back", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation()] });
    const update = deferred<unknown>();
    responders.update_preparation = () => update.promise;
    const { updatePreparation, slicing } = await startedStore();
    const pending = updatePreparation("prp-1", preparation().document);
    await flush();
    emit(envelope(1, "slicing.preparation.removed", {}, "prp-1"));
    update.resolve(preparation({ revision: 2 }));
    await pending;
    expect(slicing.preparation("mdl-1")).toBeUndefined();
  });

  it("a new Preparation for the same Model replaces the removed one", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ id: "prp-1", revision: 4 })] });
    const { slicing } = await startedStore();
    emit(envelope(1, "slicing.preparation.removed", {}, "prp-1"));
    emit(envelope(2, "slicing.preparation.changed", preparation({ id: "prp-2", revision: 1 }), "prp-2"));
    expect(slicing.preparation("mdl-1")).toMatchObject({ id: "prp-2", revision: 1 });
  });

  it("an operation only moves forward: a start_slice result never regresses an event, a terminal state never changes", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ revision: 2 })] });
    const start = deferred<unknown>();
    responders.start_slice = () => start.promise;
    const { startSlice, slicing } = await startedStore();
    const pending = startSlice("prp-1", ["plt-1"], { operationId: "op-1" });
    await flush();
    emit(envelope(1, "slicing.operation.changed", operation({ id: "sop-1", state: "running", startedAt: "2026-09-24T00:00:01Z" }), "sop-1"));
    start.resolve({ operations: [operation({ id: "sop-1", state: "queued" })] });
    await expect(pending).resolves.toHaveLength(1);
    expect(slicing.operation("sop-1")?.state).toBe("running");

    emit(envelope(2, "slicing.operation.changed", operation({ id: "sop-1", state: "failed", failure: { code: { kind: "timeout" }, message: "Timed out." } }), "sop-1"));
    emit(envelope(3, "slicing.operation.changed", operation({ id: "sop-1", state: "running" }), "sop-1"));
    expect(slicing.operation("sop-1")).toMatchObject({ state: "failed", failure: { code: { kind: "timeout" } } });
  });

  it("a slice revision is added once, newest first, and a removal is final", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { revisions: [sliceRevision({ id: "slr-1", createdAt: "2026-09-24T00:00:00Z" })] });
    responders.list_slice_revisions = () => [sliceRevision({ id: "slr-1" }), sliceRevision({ id: "slr-gone" })];
    const { loadSliceRevisions, slicing } = await startedStore();
    const newer = sliceRevision({ id: "slr-2", createdAt: "2026-09-24T01:00:00Z" });
    emit(envelope(1, "slicing.revision.created", newer, "slr-2"));
    emit(envelope(2, "slicing.revision.created", newer, "slr-2"));
    expect(slicing.revisions("mdl-1").map((r) => r.id)).toEqual(["slr-2", "slr-1"]);

    emit(envelope(3, "slicing.revision.removed", {}, "slr-gone"));
    await loadSliceRevisions("mdl-1");
    expect(slicing.revisions("mdl-1").map((r) => r.id)).toEqual(["slr-2", "slr-1"]);
    emit(envelope(4, "slicing.revision.removed", {}, "slr-1"));
    expect(slicing.revisions("mdl-1").map((r) => r.id)).toEqual(["slr-2"]);
    expect(slicing.revision("slr-1")).toBeUndefined();
  });

  it("the runtime follows events, and a command result from an older configuration never overwrites it", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { runtime: runtimeStatus({ revision: 1 }) });
    const check = deferred<unknown>();
    responders.check_slicer_runtime = () => check.promise;
    const { checkSlicerRuntime, slicing } = await startedStore();
    const pending = checkSlicerRuntime();
    await flush();
    emit(envelope(1, "slicing.runtime.changed", runtimeStatus({ revision: 2, engine: { state: "notFound" }, canSlice: false }), "local"));
    check.resolve(runtimeStatus({ revision: 1 }));
    await pending;
    expect(slicing.runtime()).toMatchObject({ revision: 2, engine: { state: "notFound" }, canSlice: false });
  });
});

describe("actions (desktop)", () => {
  it("startSlice sends the held revision and one operationId, retries a transport failure with the same id, and settles the operations", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ revision: 5 })] });
    let attempts = 0;
    responders.start_slice = () => {
      attempts += 1;
      if (attempts === 1) throw new Error("IPC dropped");
      return { operations: [operation({ id: "sop-a", plateKey: "plt-1" }), operation({ id: "sop-b", plateKey: "plt-2", plateIndex: 2 })] };
    };
    const { startSlice, slicing } = await startedStore();
    await startSlice("prp-1", ["plt-1", "plt-2"], { operationId: "op-1", continueWithSourceRevision: "msr-1" });
    const sent = tauriMock.invoke.mock.calls.filter(([name]) => name === "start_slice").map(([, args]) => args);
    expect(sent).toHaveLength(2);
    expect(sent[0]).toEqual({
      contractVersion: 1, operationId: "op-1", preparationId: "prp-1", expectedRevision: 5,
      plateKeys: ["plt-1", "plt-2"], continueWithSourceRevision: "msr-1",
    });
    expect(sent[1]).toEqual(sent[0]);
    expect(slicing.operationsForPreparation("prp-1").map((o) => o.id)).toEqual(["sop-a", "sop-b"]);
  });

  it("carries PREPARATION_STALE and its RELOAD_PREPARATION recovery through unchanged", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation()] });
    const stale = commandError("PREPARATION_STALE", "The source changed.", { recovery: ["RELOAD_PREPARATION"] });
    responders.start_slice = () => { throw stale; };
    const { startSlice } = await startedStore();
    await expect(startSlice("prp-1", ["plt-1"])).rejects.toBe(stale);
    expect(tauriMock.invoke.mock.calls.filter(([name]) => name === "start_slice")).toHaveLength(1);
  });

  it("carries SLICER_UNAVAILABLE with OPEN_SLICER_SETTINGS, and PREPARATION_INVALID with EDIT_PREPARATION, through unchanged", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation()] });
    const unavailable = commandError("SLICER_UNAVAILABLE", "No engine.", { recovery: ["OPEN_SLICER_SETTINGS"] });
    const invalid = commandError("PREPARATION_INVALID", "Nothing to slice.", { recovery: ["EDIT_PREPARATION"] });
    const errors = [unavailable, invalid];
    responders.start_slice = () => { throw errors.shift(); };
    const { startSlice } = await startedStore();
    await expect(startSlice("prp-1", ["plt-1"])).rejects.toBe(unavailable);
    await expect(startSlice("prp-1", ["plt-1"])).rejects.toBe(invalid);
  });

  it("cancelSliceOperation settles the cancelled record and clears its progress; OPERATION_NOT_CANCELLABLE passes through", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { activeAndRecentOperations: [operation({ id: "sop-1", state: "running" })] });
    responders.cancel_slice_operation = () => operation({ id: "sop-1", state: "cancelled" });
    const { cancelSliceOperation, slicing } = await startedStore();
    emit(envelope(1, "slicing.operation.progress", { message: "Slicing" }, "sop-1"));
    await cancelSliceOperation("sop-1");
    expect(tauriMock.invoke).toHaveBeenCalledWith("cancel_slice_operation", { contractVersion: 1, sliceOperationId: "sop-1" });
    expect(slicing.operation("sop-1")?.state).toBe("cancelled");
    expect(slicing.progress("sop-1")).toBeUndefined();

    const notCancellable = commandError("OPERATION_NOT_CANCELLABLE", "Already finished.");
    responders.cancel_slice_operation = () => { throw notCancellable; };
    await expect(cancelSliceOperation("sop-1")).rejects.toBe(notCancellable);
  });

  it("a CONFLICT rejects and backfills so a retry uses the fresh revision", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ revision: 1 })] });
    const { updatePreparation, slicing } = await startedStore();
    responders.update_preparation = () => { throw commandError("CONFLICT", "This Preparation changed."); };
    responders.list_slicing = () => slicingSnapshot(0, { preparations: [preparation({ revision: 6 })] });
    await expect(updatePreparation("prp-1", preparation().document)).rejects.toMatchObject({ code: "CONFLICT" });
    await flush();
    expect(slicing.preparation("mdl-1")?.revision).toBe(6);
  });

  it("createPreparation, reloadPreparation and deletePreparation settle their results", async () => {
    responders.create_preparation = () => preparation({ id: "prp-1", revision: 1 });
    responders.reload_preparation = () => ({ preparation: preparation({ revision: 2, sourceRevisionId: "msr-2" }), removedObjectKeys: [3], addedObjectKeys: [] });
    responders.delete_preparation = () => ({});
    const { createPreparation, reloadPreparation, deletePreparation, slicing } = await startedStore();

    await createPreparation("mdl-1");
    expect(tauriMock.invoke).toHaveBeenCalledWith("create_preparation", { contractVersion: 1, modelId: "mdl-1" });
    await expect(reloadPreparation("prp-1")).resolves.toMatchObject({ removedObjectKeys: [3] });
    expect(tauriMock.invoke).toHaveBeenCalledWith("reload_preparation", { contractVersion: 1, preparationId: "prp-1", expectedRevision: 1 });
    expect(slicing.preparation("mdl-1")?.sourceRevisionId).toBe("msr-2");
    await deletePreparation("prp-1");
    expect(tauriMock.invoke).toHaveBeenCalledWith("delete_preparation", { contractVersion: 1, preparationId: "prp-1", expectedRevision: 2 });
    expect(slicing.preparation("mdl-1")).toBeUndefined();
  });

  it("createExternalSliceRevision settles the new revision and caches its record", async () => {
    const record = { ...sliceRevision({ id: "slr-ext", kind: "external", estimates: null, plate: undefined }), blobs: [] };
    responders.create_external_slice_revision = () => record;
    const { createExternalSliceRevision, loadSliceRevision, slicing } = await startedStore();
    const facts = {
      printerProfile: { kind: "absent" as const },
      nozzleDiameterMm: { kind: "confirmed" as const, value: 0.4 },
      materialFamily: { kind: "absent" as const },
      filamentDiameterMm: { kind: "absent" as const },
    };
    await expect(createExternalSliceRevision("msr-g", facts, "op-ext")).resolves.toBe(record);
    expect(tauriMock.invoke).toHaveBeenCalledWith("create_external_slice_revision", {
      contractVersion: 1, operationId: "op-ext", sourceRevisionId: "msr-g", facts,
    });
    expect(slicing.revisions("mdl-1").map((r) => r.id)).toEqual(["slr-ext"]);
    expect(slicing.revision("slr-ext")).not.toHaveProperty("blobs");
    await expect(loadSliceRevision("slr-ext")).resolves.toBe(record);
    expect(tauriMock.invoke.mock.calls.filter(([name]) => name === "get_slice_revision")).toHaveLength(0);
  });

  it("deleteSliceRevision drops the revision; LIFECYCLE_BLOCKED passes through and keeps it", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { revisions: [sliceRevision({ id: "slr-1" }), sliceRevision({ id: "slr-2" })] });
    const blocked = commandError("LIFECYCLE_BLOCKED", "In use.");
    let block = true;
    responders.delete_slice_revision = () => {
      if (block) throw blocked;
      return {};
    };
    const { deleteSliceRevision, slicing } = await startedStore();
    await expect(deleteSliceRevision("slr-1")).rejects.toBe(blocked);
    expect(slicing.revisions("mdl-1")).toHaveLength(2);
    block = false;
    await deleteSliceRevision("slr-1");
    expect(slicing.revisions("mdl-1").map((r) => r.id)).toEqual(["slr-2"]);
  });

  it("loadMesh decodes the raw bytes of get_revision_mesh; NOT_FOUND passes through", async () => {
    const bytes = encodeMeshBuffer(new Float32Array([0, 0, 0, 1, 0, 0, 0, 1, 0]), new Uint32Array([0, 1, 2]));
    responders.get_revision_mesh = () => bytes;
    const { loadMesh } = await startedStore();
    const mesh = await loadMesh("msr-1", 1);
    expect(tauriMock.invoke).toHaveBeenCalledWith("get_revision_mesh", { contractVersion: 1, revisionId: "msr-1", objectKey: 1 });
    expect(mesh.triangleCount).toBe(1);
    expect([...mesh.indices]).toEqual([0, 1, 2]);

    const missing = commandError("NOT_FOUND", "msr-1/9");
    responders.get_revision_mesh = () => { throw missing; };
    await expect(loadMesh("msr-1", 9)).rejects.toBe(missing);
  });

  it("the runtime pickers send the held runtime revision, and a cancelled picker changes nothing", async () => {
    responders.list_slicing = () => slicingSnapshot(0, { runtime: runtimeStatus({ revision: 4 }) });
    responders.pick_slicer_engine = () => null;
    responders.pick_preset_source = () => runtimeStatus({ revision: 5, versionsDiffer: true });
    const { pickSlicerEngine, pickPresetSource, slicing } = await startedStore();
    await expect(pickSlicerEngine()).resolves.toBeNull();
    expect(tauriMock.invoke).toHaveBeenCalledWith("pick_slicer_engine", { contractVersion: 1, expectedRevision: 4 });
    expect(slicing.runtime()?.revision).toBe(4);
    await pickPresetSource("folder");
    expect(tauriMock.invoke).toHaveBeenCalledWith("pick_preset_source", { contractVersion: 1, expectedRevision: 4, kind: "folder" });
    expect(slicing.runtime()).toMatchObject({ revision: 5, versionsDiffer: true });
  });
});

describe("web mode", () => {
  beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

  it("loads the fixtures ready, without touching Tauri", async () => {
    const { slicing } = await startedStore();
    expect(slicing.status()).toBe("ready");
    expect(slicing.syncState()).toBe("current");
    expect(slicing.runtime()?.engine).toMatchObject({ state: "available", version: "2.4.2" });
    expect(slicing.preparation("mdl-web-enclosure")?.document.plates).toHaveLength(2);
    expect(slicing.operations().map((o) => o.state).sort()).toEqual(["failed", "succeeded"]);
    expect(slicing.revisions("mdl-web-cube-gcode")[0].facts.materialFamily.provenance).toBe("absent");
    expect(eventMock.listen).not.toHaveBeenCalled();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("Slice, Cancel, Create external, and the runtime pickers reject with the needsDesktop error", async () => {
    const store = await startedStore();
    const { isCommandError } = await import("../ipc/client");
    const facts = {
      printerProfile: { kind: "absent" as const },
      nozzleDiameterMm: { kind: "absent" as const },
      materialFamily: { kind: "absent" as const },
      filamentDiameterMm: { kind: "absent" as const },
    };
    for (const attempt of [
      () => store.startSlice("prp-web-enclosure", ["plt-web-enclosure-1"]),
      () => store.cancelSliceOperation("sop-web-enclosure-lid"),
      () => store.createExternalSliceRevision("msr-web-cube-gcode-1", facts),
      () => store.pickSlicerEngine(),
      () => store.pickPresetSource("file"),
      () => store.resetSlicerRuntime({ engine: true, presetSource: false }),
    ]) {
      const error = await attempt().catch((e: unknown) => e);
      expect(isCommandError(error)).toBe(true);
      expect(error).toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
      expect((error as { message: string }).message).toMatch(/needs the desktop app\.$/);
    }
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("the read commands answer from the fixtures", async () => {
    const store = await startedStore();
    const geometry = await store.loadGeometry("msr-web-enclosure-1");
    expect(geometry.buildItems.some((item) => !item.printable)).toBe(true);
    const mesh = await store.loadMesh("msr-web-enclosure-1", 1);
    expect(mesh.triangleCount).toBe(12);
    await expect(store.loadMesh("msr-web-enclosure-1", 9)).rejects.toMatchObject({ code: "NOT_FOUND" });
    await expect(store.loadOperationLog("sop-web-enclosure-latch")).resolves.toMatchObject({ truncated: false });
    await expect(store.loadSliceRevision("slr-web-cube-gcode")).resolves.toMatchObject({ kind: "external", blobs: [] });
    await expect(store.listSliceOptions({ kind: "printer", printerId: "prn-web-cc-1" })).resolves.toMatchObject({
      defaults: { processPreset: expect.any(String) },
    });
    await expect(store.checkSlicerRuntime()).resolves.toMatchObject({ canSlice: true });
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("Preparation edits are local and keep the backend's document rules", async () => {
    const { updatePreparation, createPreparation, deletePreparation, slicing } = await startedStore();
    const held = slicing.preparation("mdl-web-enclosure")!;
    const heldRevision = held.revision;
    const document = { ...held.document, plates: [held.document.plates[0]] };
    const updated = await updatePreparation(held.id, document);
    expect(updated.revision).toBe(heldRevision + 1);
    expect(slicing.preparation("mdl-web-enclosure")?.document.plates).toHaveLength(1);

    await expect(updatePreparation(held.id, { ...document, plates: [] })).rejects.toMatchObject({
      code: "VALIDATION", details: { fieldPath: "document.plates" },
    });

    const created = await createPreparation("mdl-web-bracket");
    expect(created).toMatchObject({ modelId: "mdl-web-bracket", sourceRevisionId: "msr-web-bracket-1", revision: 1 });
    expect(created.document.plates[0].instances.map((i) => i.objectKey)).toEqual([1]);
    await expect(createPreparation("mdl-web-bracket")).resolves.toMatchObject({ id: created.id });
    await expect(createPreparation("mdl-web-cube-gcode")).rejects.toMatchObject({ code: "VALIDATION" });

    await deletePreparation(created.id);
    expect(slicing.preparation("mdl-web-bracket")).toBeUndefined();
  });

  it("a local Preparation for a 3MF leaves unprintable build items out", async () => {
    const { deletePreparation, createPreparation, slicing } = await startedStore();
    await deletePreparation(slicing.preparation("mdl-web-enclosure")!.id);
    const created = await createPreparation("mdl-web-enclosure");
    expect(created.document.plates.map((p) => p.instances.map((i) => i.objectKey))).toEqual([[1], [2]]);
  });
});
