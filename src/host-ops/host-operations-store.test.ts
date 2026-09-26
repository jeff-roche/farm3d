import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { hostOperation, hostOperationsSnapshot } from "./test-records";
import type { HostOperation, HostOperationsEvent } from "./types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const eventMock = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => eventMock);

const STREAM = "stream-host-ops";

type Handler = (event: { payload: unknown }) => void;
type Responder = (args: Record<string, unknown>) => unknown;

let handler: Handler;
let unlisten: ReturnType<typeof vi.fn>;
let responders: Record<string, Responder>;

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  eventMock.listen.mockReset();
  tauriMock.isTauri.mockReturnValue(true);
  handler = () => {};
  unlisten = vi.fn();
  responders = { list_host_operations: () => hostOperationsSnapshot(0) };
  eventMock.listen.mockImplementation((_name: string, cb: Handler) => {
    handler = cb;
    return Promise.resolve(unlisten);
  });
  tauriMock.invoke.mockImplementation((name: string, args: Record<string, unknown>) => {
    const respond = responders[name];
    if (!respond) return Promise.reject(new Error(`unexpected command ${name}`));
    try {
      const data = respond(args);
      return data instanceof Promise
        ? data.then((value) => ({ contractVersion: 1, data: value }))
        : Promise.resolve({ contractVersion: 1, data });
    } catch (error) {
      return Promise.reject(error);
    }
  });
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

function envelope(sequence: number, operation: HostOperation, streamId = STREAM): HostOperationsEvent {
  return {
    contractVersion: 1,
    streamId,
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-25T00:00:00Z",
    type: "hostOperations.operation.changed",
    subject: { kind: "hostOperation", id: operation.id },
    payload: operation,
  };
}

function emit(event: unknown): void {
  handler({ payload: event });
}

function commandError(code: string, message: string) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 20; i += 1) await Promise.resolve();
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

async function startedStore() {
  const store = await import("./host-operations-store");
  await store.startHostOperations();
  return store;
}

const backfills = () => tauriMock.invoke.mock.calls.filter(([name]) => name === "list_host_operations").length;

describe("startHostOperations (desktop)", () => {
  it("listens before backfilling, drops buffered events the snapshot covers, and replays later ones in order", async () => {
    const backfill = deferred<unknown>();
    responders.list_host_operations = () => backfill.promise;
    const { startHostOperations, hostOperations } = await import("./host-operations-store");
    const started = startHostOperations();
    await flush();
    expect(hostOperations.syncState()).toBe("syncing");

    const uncertain = hostOperation({ id: "hop-1", state: "uncertain" });
    emit(envelope(2, hostOperation({ id: "hop-old", state: "succeeded" }))); // covered by the snapshot below
    emit(envelope(3, uncertain));
    backfill.resolve(hostOperationsSnapshot(2, { operations: [hostOperation({ id: "hop-1", state: "dispatching" })] }));
    await started;

    expect(hostOperations.syncState()).toBe("current");
    expect(hostOperations.operation("hop-1")?.state).toBe("uncertain");
    expect(hostOperations.operation("hop-old")).toBeUndefined();
  });

  it("ignores every event on the shared channel that is not hostOperations.*", async () => {
    const { hostOperations } = await startedStore();
    emit({ contractVersion: 1, streamId: "stream-library", sequence: 1, eventId: "e1", occurredAt: "x", type: "library.model.changed", subject: { kind: "model", id: "m1" }, payload: {} });
    await flush();
    expect(hostOperations.syncState()).toBe("current");
    expect(backfills()).toBe(1);
  });

  it("a sequence gap triggers a new backfill and marks the store uncertain until it settles", async () => {
    const { hostOperations } = await startedStore();
    const rebackfill = deferred<unknown>();
    responders.list_host_operations = () => rebackfill.promise;
    emit(envelope(3, hostOperation({ id: "hop-3" })));
    await flush();

    expect(hostOperations.syncState()).toBe("uncertain");
    expect(backfills()).toBe(2);

    rebackfill.resolve(hostOperationsSnapshot(3, { operations: [hostOperation({ id: "hop-3" })] }));
    await flush();
    expect(hostOperations.syncState()).toBe("current");
    expect(hostOperations.operation("hop-3")).toBeDefined();
  });

  it("a duplicate sequence after the snapshot applies once and causes no resync", async () => {
    responders.list_host_operations = () => hostOperationsSnapshot(0);
    const { hostOperations } = await startedStore();
    emit(envelope(1, hostOperation({ id: "hop-1", state: "uncertain", attempts: 1 })));
    emit(envelope(1, hostOperation({ id: "hop-1", state: "uncertain", attempts: 99 })));
    await flush();
    expect(hostOperations.operation("hop-1")?.attempts).toBe(1);
    expect(hostOperations.syncState()).toBe("current");
    expect(backfills()).toBe(1);
  });

  it("a failed first load keeps retrying with backoff, then recovers", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    responders.list_host_operations = () => { throw commandError("PERSISTENCE_UNAVAILABLE", "Storage is busy."); };
    const { hostOperations } = await startedStore();
    expect(hostOperations.syncState()).toBe("uncertain");

    await vi.advanceTimersByTimeAsync(1_000);
    expect(backfills()).toBe(2);
    responders.list_host_operations = () => hostOperationsSnapshot(0, { operations: [hostOperation({ id: "hop-1" })] });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(backfills()).toBe(3);
    expect(hostOperations.syncState()).toBe("current");
    expect(hostOperations.operation("hop-1")).toBeDefined();
  });

  it("is idempotent: starting again disposes the previous listener, and the returned disposer unlistens", async () => {
    const first = vi.fn();
    const second = vi.fn();
    const { startHostOperations } = await import("./host-operations-store");
    unlisten = first;
    await startHostOperations();
    unlisten = second;
    const dispose = await startHostOperations();
    expect(first).toHaveBeenCalledOnce();
    expect(second).not.toHaveBeenCalled();
    dispose();
    expect(second).toHaveBeenCalledOnce();
  });

  it("refreshHostOperations backfills again", async () => {
    const { refreshHostOperations, hostOperations } = await startedStore();
    responders.list_host_operations = () => hostOperationsSnapshot(0, { operations: [hostOperation({ id: "hop-fresh" })] });
    refreshHostOperations();
    await flush();
    expect(backfills()).toBe(2);
    expect(hostOperations.operation("hop-fresh")).toBeDefined();
  });
});

describe("startHostOperations (web)", () => {
  it("loads web-fixtures.ts' Host Operations", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { startHostOperations, hostOperations } = await import("./host-operations-store");
    const { WEB_HOST_OPS_STAGED_OPERATION, WEB_HOST_OPS_PRINTER_FINISHED } = await import("./web-fixtures");
    await startHostOperations();
    expect(hostOperations.syncState()).toBe("current");
    expect(hostOperations.stagedFor(WEB_HOST_OPS_PRINTER_FINISHED).map((o) => o.id)).toEqual([WEB_HOST_OPS_STAGED_OPERATION]);
  });
});

describe("derived views", () => {
  it("unresolvedFor returns the one unresolved row for a Printer, if any", async () => {
    responders.list_host_operations = () => hostOperationsSnapshot(0, {
      operations: [
        hostOperation({ id: "hop-1", printerId: "prn-1", state: "uncertain" }),
        hostOperation({ id: "hop-2", printerId: "prn-2", state: "succeeded" }),
      ],
    });
    const { hostOperations } = await startedStore();
    expect(hostOperations.unresolvedFor("prn-1")?.id).toBe("hop-1");
    expect(hostOperations.unresolvedFor("prn-2")).toBeUndefined();
  });

  it("stagedFor returns succeeded uploads, newest per hostPath", async () => {
    responders.list_host_operations = () => hostOperationsSnapshot(0, {
      operations: [
        hostOperation({ id: "hop-1", printerId: "prn-1", kind: "upload", state: "succeeded", hostPath: "a.gcode", createdAt: "2026-09-24T00:00:00Z" }),
        hostOperation({ id: "hop-2", printerId: "prn-1", kind: "upload", state: "succeeded", hostPath: "a.gcode", createdAt: "2026-09-25T00:00:00Z" }),
        hostOperation({ id: "hop-3", printerId: "prn-1", kind: "start", state: "succeeded", hostPath: "a.gcode" }),
      ],
    });
    const { hostOperations } = await startedStore();
    const staged = hostOperations.stagedFor("prn-1");
    expect(staged.map((o) => o.id)).toEqual(["hop-2"]);
  });

  it("recentFor returns the Printer's terminal rows", async () => {
    responders.list_host_operations = () => hostOperationsSnapshot(0, {
      operations: [
        hostOperation({ id: "hop-1", printerId: "prn-1", state: "succeeded" }),
        hostOperation({ id: "hop-2", printerId: "prn-1", state: "dispatching" }),
        hostOperation({ id: "hop-3", printerId: "prn-2", state: "failed" }),
      ],
    });
    const { hostOperations } = await startedStore();
    expect(hostOperations.recentFor("prn-1").map((o) => o.id)).toEqual(["hop-1"]);
  });
});

describe("write actions (desktop)", () => {
  const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  const invoked = (name: string) => tauriMock.invoke.mock.calls.filter(([called]) => called === name).map(([, args]) => args as Record<string, unknown>);

  it("each wrapper sends a fresh operationId per user action and exactly the spec's arguments", async () => {
    const row = hostOperation({ id: "hop-w" });
    for (const name of ["stage_slice_revision", "start_staged_artifact", "pause_host_print", "resume_host_print", "cancel_host_print", "reconcile_host_operation", "abandon_host_operation"]) {
      responders[name] = () => row;
    }
    const store = await startedStore();
    await store.stageSliceRevision("prn-1", "slr-1");
    await store.stageSliceRevision("prn-1", "slr-1");
    await store.startStagedArtifact("prn-1", "hop-up", "finished");
    await store.pauseHostPrint("prn-1");
    await store.resumeHostPrint("prn-1");
    await store.cancelHostPrint("prn-1");
    await store.reconcileHostOperation("hop-u");
    await store.abandonHostOperation("hop-u", "  checked by hand  ");

    const [firstStage, secondStage] = invoked("stage_slice_revision");
    expect(firstStage).toEqual({ contractVersion: 1, operationId: expect.stringMatching(UUID), printerId: "prn-1", sliceRevisionId: "slr-1" });
    expect(secondStage.operationId).not.toBe(firstStage.operationId);
    expect(invoked("start_staged_artifact")).toEqual([
      { contractVersion: 1, operationId: expect.stringMatching(UUID), printerId: "prn-1", hostOperationId: "hop-up", priorState: "finished" },
    ]);
    for (const verb of ["pause_host_print", "resume_host_print", "cancel_host_print"]) {
      expect(invoked(verb)).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), printerId: "prn-1" }]);
    }
    expect(invoked("reconcile_host_operation")).toEqual([{ contractVersion: 1, hostOperationId: "hop-u" }]);
    expect(invoked("abandon_host_operation")).toEqual([
      { contractVersion: 1, operationId: expect.stringMatching(UUID), hostOperationId: "hop-u", acknowledgement: "hostStateUnknown", note: "checked by hand" },
    ]);
    const ids = tauriMock.invoke.mock.calls.map(([, args]) => (args as { operationId?: string }).operationId).filter(Boolean);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("an empty abandon note is left out", async () => {
    responders.abandon_host_operation = () => hostOperation({ id: "hop-u", state: "abandoned" });
    const store = await startedStore();
    await store.abandonHostOperation("hop-u", "   ");
    expect(invoked("abandon_host_operation")[0]).not.toHaveProperty("note");
  });

  it("retries a transport failure once with the same operationId, never a CommandError", async () => {
    let calls = 0;
    responders.pause_host_print = () => {
      calls += 1;
      if (calls === 1) throw new Error("socket closed");
      return hostOperation({ id: "hop-p", kind: "pause" });
    };
    responders.cancel_host_print = () => { throw commandError("CONTROL_NOT_ALLOWED", "The printer isn't in a state to cancel now: it is ready."); };
    const store = await startedStore();
    await store.pauseHostPrint("prn-1");
    const [first, second] = invoked("pause_host_print");
    expect(second.operationId).toBe(first.operationId);
    await expect(store.cancelHostPrint("prn-1")).rejects.toMatchObject({ code: "CONTROL_NOT_ALLOWED" });
    expect(invoked("cancel_host_print")).toHaveLength(1);
  });

  it("adds the returned row when the stream hasn't delivered it yet, and never regresses a newer one", async () => {
    responders.stage_slice_revision = () => hostOperation({ id: "hop-new", state: "dispatching" });
    responders.pause_host_print = () => hostOperation({ id: "hop-1", kind: "pause", state: "dispatching" });
    responders.list_host_operations = () => hostOperationsSnapshot(0, { operations: [hostOperation({ id: "hop-1", kind: "pause", state: "succeeded" })] });
    const store = await startedStore();
    await store.stageSliceRevision("prn-1", "slr-1");
    expect(store.hostOperations.operation("hop-new")?.state).toBe("dispatching");
    await store.pauseHostPrint("prn-1");
    expect(store.hostOperations.operation("hop-1")?.state).toBe("succeeded");
  });

  it("never sends a credential: a Printer's credentialRef appears in no request", async () => {
    const SECRET = "cred-ref-SEEDED-SECRET-9f2c";
    responders.list_printers = () => [{
      id: "prn-1", revision: 1, name: "Bay 1", notes: "", overrides: {},
      catalogRef: { vendor: "V", model: "M", variant: "M 0.4", modelId: "m", printerVariant: "0.4" },
      connection: { kind: "moonraker", host: "192.0.2.10", port: 7125, useTls: false, credentialRef: SECRET },
      profileResolution: { catalogStatus: "ok", modelLabel: "M", variantLabel: "M", profile: {}, overriddenFields: [], inherited: {}, profileDrift: [], unknownOverrideKeys: [] },
      startSafety: "confirmBedClear", materialSlots: [], setupGaps: [], createdAt: "", updatedAt: "",
    }];
    for (const name of ["stage_slice_revision", "start_staged_artifact", "pause_host_print", "resume_host_print", "cancel_host_print", "reconcile_host_operation", "abandon_host_operation"]) {
      responders[name] = () => hostOperation({ id: "hop-s" });
    }
    const { loadPrinters } = await import("../printers/printer-store");
    await loadPrinters();
    const store = await startedStore();
    await store.stageSliceRevision("prn-1", "slr-1");
    await store.startStagedArtifact("prn-1", "hop-up", "ready");
    await store.pauseHostPrint("prn-1");
    await store.resumeHostPrint("prn-1");
    await store.cancelHostPrint("prn-1");
    await store.reconcileHostOperation("hop-u");
    await store.abandonHostOperation("hop-u");
    const writes = tauriMock.invoke.mock.calls.filter(([name]) => name !== "list_printers" && name !== "list_host_operations");
    expect(writes).toHaveLength(7);
    expect(JSON.stringify(writes)).not.toContain(SECRET);
    expect(JSON.stringify(store.hostOperations.operations())).not.toContain(SECRET);
  });
});

describe("write actions (web)", () => {
  it("refuses every write with a needs-the-desktop error and invokes nothing", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const store = await import("./host-operations-store");
    await expect(store.stageSliceRevision("prn-1", "slr-1")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.startStagedArtifact("prn-1", "hop-1", "ready")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.abandonHostOperation("hop-1")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });
});

describe("rows of Printers that no longer exist", () => {
  it("are dropped once the Printers have loaded, including after a Printer is deleted", async () => {
    const record = (id: string) => ({
      id, revision: 1, name: id, notes: "", overrides: {},
      catalogRef: { vendor: "V", model: "M", variant: "M 0.4", modelId: "m", printerVariant: "0.4" },
      profileResolution: { catalogStatus: "ok", modelLabel: "M", variantLabel: "M", profile: {}, overriddenFields: [], inherited: {}, profileDrift: [], unknownOverrideKeys: [] },
      startSafety: "confirmBedClear", materialSlots: [], setupGaps: [], createdAt: "", updatedAt: "",
    });
    responders.list_printers = () => [record("prn-1"), record("prn-2")];
    responders.delete_printer = () => ({});
    responders.list_host_operations = () => hostOperationsSnapshot(0, {
      operations: [
        hostOperation({ id: "hop-1", printerId: "prn-1", state: "succeeded" }),
        hostOperation({ id: "hop-2", printerId: "prn-2", state: "failed" }),
        hostOperation({ id: "hop-gone", printerId: "prn-gone", state: "uncertain" }),
      ],
    });
    const { loadPrinters, removePrinter } = await import("../printers/printer-store");
    await loadPrinters();
    const { hostOperations } = await startedStore();
    expect(hostOperations.operations().map((o) => o.id).sort()).toEqual(["hop-1", "hop-2"]);
    expect(hostOperations.unresolvedFor("prn-gone")).toBeUndefined();
    expect(hostOperations.operation("hop-gone")).toBeUndefined();

    await removePrinter("prn-2");
    expect(hostOperations.operations().map((o) => o.id)).toEqual(["hop-1"]);
    expect(hostOperations.recentFor("prn-2")).toEqual([]);
  });
});
