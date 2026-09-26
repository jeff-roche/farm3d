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
