import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { eligibilitySummary, job, queueChange, queueEntry, queueSnapshot, reconciliationRequirement } from "./test-records";
import type { Job, QueueEntry, QueueEvent } from "./types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const eventMock = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => eventMock);

const STREAM = "stream-queue";

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
  responders = { list_queue: () => queueSnapshot(0) };
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

function entryEnvelope(sequence: number, entry: QueueEntry, streamId = STREAM): QueueEvent {
  return {
    contractVersion: 1,
    streamId,
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-25T00:00:00Z",
    type: "queue.entry.changed",
    subject: { kind: "queueEntry", id: entry.id },
    payload: { type: "entryChanged", entry },
  };
}

function jobEnvelope(sequence: number, record: Job, streamId = STREAM): QueueEvent {
  return {
    contractVersion: 1,
    streamId,
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-25T00:00:00Z",
    type: "queue.job.changed",
    subject: { kind: "job", id: record.id },
    payload: { type: "jobChanged", job: record },
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
  const store = await import("./queue-store");
  await store.startQueue();
  return store;
}

const backfills = () => tauriMock.invoke.mock.calls.filter(([name]) => name === "list_queue").length;

describe("startQueue (desktop)", () => {
  it("listens before backfilling, drops buffered events the snapshot covers, and replays later ones in order", async () => {
    const backfill = deferred<unknown>();
    responders.list_queue = () => backfill.promise;
    const { startQueue, queue } = await import("./queue-store");
    const started = startQueue();
    await flush();
    expect(queue.syncState()).toBe("syncing");

    const older = queueEntry({ id: "qen-old", state: "queued", position: 1 });
    const newer = queueEntry({ id: "qen-1", state: "assigned", position: 1, revision: 2 });
    emit(entryEnvelope(2, older)); // covered by the snapshot below
    emit(entryEnvelope(3, newer));
    backfill.resolve(queueSnapshot(2, { entries: [queueEntry({ id: "qen-1", state: "queued", position: 1 })] }));
    await started;

    expect(queue.syncState()).toBe("current");
    expect(queue.entry("qen-1")?.state).toBe("assigned");
    expect(queue.entry("qen-old")).toBeUndefined();
  });

  it("ignores every event on the shared channel that is not queue.*", async () => {
    const { queue } = await startedStore();
    emit({ contractVersion: 1, streamId: "stream-library", sequence: 1, eventId: "e1", occurredAt: "x", type: "library.model.changed", subject: { kind: "model", id: "m1" }, payload: {} });
    await flush();
    expect(queue.syncState()).toBe("current");
    expect(backfills()).toBe(1);
  });

  it("a sequence gap triggers a new backfill and marks the store uncertain until it settles", async () => {
    const { queue } = await startedStore();
    const rebackfill = deferred<unknown>();
    responders.list_queue = () => rebackfill.promise;
    emit(entryEnvelope(3, queueEntry({ id: "qen-3", position: 1 })));
    await flush();

    expect(queue.syncState()).toBe("uncertain");
    expect(backfills()).toBe(2);

    rebackfill.resolve(queueSnapshot(3, { entries: [queueEntry({ id: "qen-3", position: 1 })] }));
    await flush();
    expect(queue.syncState()).toBe("current");
    expect(queue.entry("qen-3")).toBeDefined();
  });

  it("a duplicate/stale sequence after the snapshot applies once and causes no resync", async () => {
    responders.list_queue = () => queueSnapshot(0);
    const { queue } = await startedStore();
    emit(entryEnvelope(1, queueEntry({ id: "qen-1", position: 1, revision: 1 })));
    emit(entryEnvelope(1, queueEntry({ id: "qen-1", position: 1, revision: 99 })));
    await flush();
    expect(queue.entry("qen-1")?.revision).toBe(1);
    expect(queue.syncState()).toBe("current");
    expect(backfills()).toBe(1);
  });

  it("a failed first load keeps retrying with backoff, then recovers", async () => {
    vi.useFakeTimers({ toFake: ["setTimeout", "clearTimeout"] });
    responders.list_queue = () => { throw commandError("PERSISTENCE_UNAVAILABLE", "Storage is busy."); };
    const { queue } = await startedStore();
    expect(queue.syncState()).toBe("uncertain");

    await vi.advanceTimersByTimeAsync(1_000);
    expect(backfills()).toBe(2);
    responders.list_queue = () => queueSnapshot(0, { entries: [queueEntry({ id: "qen-1", position: 1 })] });
    await vi.advanceTimersByTimeAsync(2_000);
    expect(backfills()).toBe(3);
    expect(queue.syncState()).toBe("current");
    expect(queue.entry("qen-1")).toBeDefined();
  });

  it("is idempotent: starting again disposes the previous listener, and the returned disposer unlistens", async () => {
    const first = vi.fn();
    const second = vi.fn();
    const { startQueue } = await import("./queue-store");
    unlisten = first;
    await startQueue();
    unlisten = second;
    const dispose = await startQueue();
    expect(first).toHaveBeenCalledOnce();
    expect(second).not.toHaveBeenCalled();
    dispose();
    expect(second).toHaveBeenCalledOnce();
  });

  it("refreshQueue backfills again", async () => {
    const { refreshQueue, queue } = await startedStore();
    responders.list_queue = () => queueSnapshot(0, { entries: [queueEntry({ id: "qen-fresh", position: 1 })] });
    refreshQueue();
    await flush();
    expect(backfills()).toBe(2);
    expect(queue.entry("qen-fresh")).toBeDefined();
  });

  it("applies a jobChanged event by upserting the Job", async () => {
    const { queue } = await startedStore();
    emit(jobEnvelope(1, job({ id: "job-1", state: "printing" })));
    await flush();
    expect(queue.job("job-1")?.state).toBe("printing");
  });

  it("applies a requirementChanged event by upserting the Requirement", async () => {
    const { queue } = await startedStore();
    const event: QueueEvent = {
      contractVersion: 1, streamId: STREAM, sequence: 1, eventId: "evt-1", occurredAt: "2026-09-25T00:00:00Z",
      type: "queue.requirement.changed", subject: { kind: "reconciliationRequirement", id: "rqr-1" },
      payload: { type: "requirementChanged", requirement: reconciliationRequirement({ id: "rqr-1", status: "deferred" }) },
    };
    emit(event);
    await flush();
    expect(queue.requirements().map((r) => r.id)).toEqual(["rqr-1"]);
  });

  it("an eligibilityChanged event replaces the full set, not a diff", async () => {
    responders.list_queue = () => queueSnapshot(0, {
      eligibility: [eligibilitySummary({ entryId: "qen-1", verdict: "blocked" }), eligibilitySummary({ entryId: "qen-2", verdict: "ready" })],
    });
    const { queue } = await startedStore();
    const event: QueueEvent = {
      contractVersion: 1, streamId: STREAM, sequence: 1, eventId: "evt-1", occurredAt: "2026-09-25T00:00:00Z",
      type: "queue.eligibility.changed", subject: { kind: "queue", id: "eligibility" },
      payload: {
        type: "eligibilityChanged",
        summaries: [eligibilitySummary({ entryId: "qen-3", verdict: "ready" })],
        nextAutomaticAction: { kind: "noAutomaticEntries", evaluatedAt: "2026-09-25T00:01:00Z" },
      },
    };
    emit(event);
    await flush();
    expect(queue.eligibility("qen-1")).toBeUndefined();
    expect(queue.eligibility("qen-2")).toBeUndefined();
    expect(queue.eligibility("qen-3")?.verdict).toBe("ready");
    expect(queue.nextAutomaticAction()).toEqual({ kind: "noAutomaticEntries", evaluatedAt: "2026-09-25T00:01:00Z" });
  });
});

describe("startQueue (web)", () => {
  it("loads web-fixtures.ts' Queue", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { startQueue, queue } = await import("./queue-store");
    const { WEB_QUEUE_ENTRY_BLOCKED } = await import("./web-fixtures");
    await startQueue();
    expect(queue.syncState()).toBe("current");
    expect(queue.entry(WEB_QUEUE_ENTRY_BLOCKED)).toBeDefined();
  });
});

describe("derived reads", () => {
  it("entries returns open entries in position order", async () => {
    responders.list_queue = () => queueSnapshot(0, {
      entries: [
        queueEntry({ id: "qen-2", state: "queued", position: 2 }),
        queueEntry({ id: "qen-1", state: "assigned", position: 1 }),
        queueEntry({ id: "qen-closed", state: "closed", closeReason: "completed", position: null }),
      ],
    });
    const { queue } = await startedStore();
    expect(queue.entries().map((e) => e.id)).toEqual(["qen-1", "qen-2"]);
  });

  it("history returns closed entries, newest-closed first", async () => {
    responders.list_queue = () => queueSnapshot(0, {
      entries: [
        queueEntry({ id: "qen-a", state: "closed", closeReason: "completed", position: null, closedAt: "2026-09-20T00:00:00Z" }),
        queueEntry({ id: "qen-b", state: "closed", closeReason: "failed", position: null, closedAt: "2026-09-24T00:00:00Z" }),
      ],
    });
    const { queue } = await startedStore();
    expect(queue.history().map((e) => e.id)).toEqual(["qen-b", "qen-a"]);
  });

  it("jobFor and job find by queueEntryId and by id", async () => {
    responders.list_queue = () => queueSnapshot(0, { jobs: [job({ id: "job-1", queueEntryId: "qen-1" })] });
    const { queue } = await startedStore();
    expect(queue.jobFor("qen-1")?.id).toBe("job-1");
    expect(queue.job("job-1")?.queueEntryId).toBe("qen-1");
    expect(queue.jobFor("qen-none")).toBeUndefined();
  });

  it("activeJobFor finds a Printer's one non-terminal Job", async () => {
    responders.list_queue = () => queueSnapshot(0, {
      jobs: [
        job({ id: "job-done", printerId: "prn-1", state: "completed" }),
        job({ id: "job-live", printerId: "prn-1", state: "printing" }),
        job({ id: "job-other", printerId: "prn-2", state: "printing" }),
      ],
    });
    const { queue } = await startedStore();
    expect(queue.activeJobFor("prn-1")?.id).toBe("job-live");
    expect(queue.activeJobFor("prn-3")).toBeUndefined();
  });

  it("requirements excludes resolved rows", async () => {
    responders.list_queue = () => queueSnapshot(0, {
      requirements: [
        reconciliationRequirement({ id: "rqr-pending", status: "pending" }),
        reconciliationRequirement({ id: "rqr-deferred", status: "deferred" }),
        reconciliationRequirement({ id: "rqr-resolved", status: "resolved", resolution: { kind: "declared", outcome: "failed" } }),
      ],
    });
    const { queue } = await startedStore();
    expect(queue.requirements().map((r) => r.id).sort()).toEqual(["rqr-deferred", "rqr-pending"]);
  });
});

describe("write actions (desktop)", () => {
  const UUID = /^[0-9a-f]{8}-[0-9a-f]{4}-4[0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12}$/;
  const invoked = (name: string) => tauriMock.invoke.mock.calls.filter(([called]) => called === name).map(([, args]) => args as Record<string, unknown>);

  it("each wrapper sends a fresh operationId per user action and exactly the spec's arguments", async () => {
    for (const name of [
      "add_to_queue", "update_queue_entry", "move_queue_entry", "remove_queue_entry", "assign_queue_entry",
      "stage_job", "start_job", "pause_job", "resume_job", "cancel_job", "release_job", "retry_job",
      "declare_job_outcome", "settle_job_material", "correct_job_material",
    ]) {
      responders[name] = () => queueChange();
    }
    const store = await startedStore();
    await store.addToQueue("slr-1", 3, "manual", "loadedFirst", { manualPrinterId: "prn-1" });
    await store.addToQueue("slr-1", 3, "manual", "loadedFirst", { manualPrinterId: "prn-1" });
    await store.updateQueueEntry("qen-1", 1, { policy: "automatic" });
    await store.moveQueueEntry("qen-1", 1, 3);
    await store.removeQueueEntry("qen-1", 1);
    await store.assignQueueEntry("qen-1", "prn-1", "spl-1", true);
    await store.stageJob("job-1");
    await store.startJob("job-1", "ready");
    await store.pauseJob("job-1");
    await store.resumeJob("job-1");
    await store.cancelJob("job-1");
    await store.releaseJob("job-1");
    await store.retryJob("job-1");
    await store.declareJobOutcome("job-1", "failed");
    await store.settleJobMaterial("job-1", { kind: "estimated" });
    await store.correctJobMaterial("job-1", { kind: "net", netMg: 1_000, confidence: "measured" });

    const [firstAdd, secondAdd] = invoked("add_to_queue");
    expect(firstAdd).toEqual({ contractVersion: 1, operationId: expect.stringMatching(UUID), sliceRevisionId: "slr-1", quantity: 3, policy: "manual", preference: "loadedFirst", manualPrinterId: "prn-1" });
    expect(secondAdd.operationId).not.toBe(firstAdd.operationId);
    expect(invoked("update_queue_entry")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), entryId: "qen-1", expectedRevision: 1, policy: "automatic" }]);
    expect(invoked("move_queue_entry")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), entryId: "qen-1", expectedRevision: 1, toPosition: 3 }]);
    expect(invoked("remove_queue_entry")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), entryId: "qen-1", expectedRevision: 1 }]);
    expect(invoked("assign_queue_entry")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), entryId: "qen-1", printerId: "prn-1", spoolId: "spl-1", acknowledgeManualFacts: true }]);
    expect(invoked("stage_job")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1" }]);
    expect(invoked("start_job")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1", priorState: "ready", acknowledgement: "bedClear" }]);
    expect(invoked("pause_job")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1" }]);
    expect(invoked("resume_job")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1" }]);
    expect(invoked("cancel_job")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1" }]);
    expect(invoked("release_job")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1" }]);
    expect(invoked("retry_job")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1" }]);
    expect(invoked("declare_job_outcome")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1", outcome: "failed", acknowledgement: "hostStateUnknown" }]);
    expect(invoked("settle_job_material")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1", choice: { kind: "estimated" } }]);
    expect(invoked("correct_job_material")).toEqual([{ contractVersion: 1, operationId: expect.stringMatching(UUID), jobId: "job-1", entry: { kind: "net", netMg: 1_000, confidence: "measured" } }]);

    const ids = tauriMock.invoke.mock.calls.map(([, args]) => (args as { operationId?: string }).operationId).filter(Boolean);
    expect(new Set(ids).size).toBe(ids.length);
  });

  it("retries a transport failure once with the same operationId, never a CommandError", async () => {
    let calls = 0;
    responders.pause_job = () => {
      calls += 1;
      if (calls === 1) throw new Error("socket closed");
      return queueChange({ jobs: [job({ id: "job-1", state: "printing" })] });
    };
    responders.cancel_job = () => { throw commandError("JOB_ACTION_NOT_ALLOWED", "This Job can't cancel while it is completed."); };
    const store = await startedStore();
    await store.pauseJob("job-1");
    const [first, second] = invoked("pause_job");
    expect(second.operationId).toBe(first.operationId);
    await expect(store.cancelJob("job-1")).rejects.toMatchObject({ code: "JOB_ACTION_NOT_ALLOWED" });
    expect(invoked("cancel_job")).toHaveLength(1);
  });

  it("patches the store from the returned QueueChange for rows the stream hasn't delivered yet, and never regresses a newer one", async () => {
    responders.stage_job = () => queueChange({ jobs: [job({ id: "job-new", state: "staging" })] });
    responders.pause_job = () => queueChange({ jobs: [job({ id: "job-1", state: "printing" })] });
    responders.list_queue = () => queueSnapshot(0, { jobs: [job({ id: "job-1", state: "paused" })] });
    const store = await startedStore();
    await store.stageJob("job-new");
    expect(store.queue.job("job-new")?.state).toBe("staging");
    await store.pauseJob("job-1");
    expect(store.queue.job("job-1")?.state).toBe("paused");
  });
});

describe("write actions (web)", () => {
  it("refuses every write with a needs-the-desktop error and invokes nothing", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const store = await import("./queue-store");
    await expect(store.addToQueue("slr-1", 1, "manual", "loadedFirst")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.assignQueueEntry("qen-1", "prn-1", "spl-1")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.stageJob("job-1")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.settleJobMaterial("job-1", { kind: "defer" })).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });
});
