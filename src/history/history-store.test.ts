import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { buildWebQueueFixture } from "../queue/web-fixtures";
import type { JobHistoryPage, JobHistoryRow } from "./types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

function row(jobId: string): JobHistoryRow {
  return { ...(webRows()[0]), jobId };
}
function webRows(): JobHistoryRow[] {
  return [{
    jobId: "job-x", state: "completed", cancelReason: null, historyAt: "2026-09-20T09:29:00Z", startedAt: null,
    endedAt: "2026-09-20T09:29:00Z", printerId: "prt-1", printerSnapshotName: "Bay 4", printerArchived: false,
    spoolId: "spl-1", spoolNumber: 1, modelId: "mdl-1", modelName: "Lid", sliceRevisionId: "slr-1", plateName: null,
    settlement: "settled", incidentId: null, snapshotCount: 0,
  }];
}
const ok = (data: JobHistoryPage) => ({ contractVersion: 1, data });
function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((r) => { resolve = r; });
  return { promise, resolve };
}

async function load() {
  return import("./history-store");
}

beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  tauriMock.isTauri.mockReset().mockReturnValue(true);
  tauriMock.invoke.mockReset();
});
afterEach(() => vi.useRealTimers());

describe("history store", () => {
  it("debounces filter changes by 250 ms into one query", async () => {
    tauriMock.invoke.mockResolvedValue(ok({ rows: [row("a")], nextCursor: null }));
    const store = await load();
    store.setHistoryFilters({ text: "l" });
    store.setHistoryFilters({ text: "li" });
    store.setHistoryFilters({ text: " lid " });
    await vi.advanceTimersByTimeAsync(249);
    expect(tauriMock.invoke).not.toHaveBeenCalled();
    await vi.advanceTimersByTimeAsync(1);
    expect(tauriMock.invoke).toHaveBeenCalledTimes(1);
    expect(tauriMock.invoke).toHaveBeenCalledWith("list_job_history", { contractVersion: 1, query: { text: "lid" } });
    expect(store.history.rows().map((r) => r.jobId)).toEqual(["a"]);
    expect(store.history.status()).toBe("ready");
  });

  it("drops an out-of-order response", async () => {
    const slow = deferred<unknown>();
    tauriMock.invoke
      .mockReturnValueOnce(slow.promise)
      .mockResolvedValueOnce(ok({ rows: [row("new")], nextCursor: null }));
    const store = await load();
    store.setHistoryFilters({ printerId: "p1" });
    await vi.advanceTimersByTimeAsync(250);
    store.setHistoryFilters({ printerId: "p2" });
    await vi.advanceTimersByTimeAsync(250);
    expect(store.history.rows().map((r) => r.jobId)).toEqual(["new"]);
    slow.resolve(ok({ rows: [row("stale")], nextCursor: "zzz" }));
    await vi.advanceTimersByTimeAsync(0);
    expect(store.history.rows().map((r) => r.jobId)).toEqual(["new"]);
    expect(store.history.hasMore()).toBe(false);
  });

  it("appends on load more and a filter change resets the cursor", async () => {
    tauriMock.invoke
      .mockResolvedValueOnce(ok({ rows: [row("a")], nextCursor: "c1" }))
      .mockResolvedValueOnce(ok({ rows: [row("b")], nextCursor: null }))
      .mockResolvedValueOnce(ok({ rows: [row("c")], nextCursor: null }));
    const store = await load();
    await store.refreshHistory();
    expect(store.history.hasMore()).toBe(true);
    await store.loadMoreHistory();
    expect(tauriMock.invoke).toHaveBeenLastCalledWith("list_job_history", { contractVersion: 1, query: { after: "c1" } });
    expect(store.history.rows().map((r) => r.jobId)).toEqual(["a", "b"]);
    expect(store.history.hasMore()).toBe(false);
    store.setHistoryFilters({ states: ["failed"] });
    expect(store.history.hasMore()).toBe(false);
    await vi.advanceTimersByTimeAsync(250);
    expect(tauriMock.invoke).toHaveBeenLastCalledWith("list_job_history", { contractVersion: 1, query: { states: ["failed"] } });
    expect(store.history.rows().map((r) => r.jobId)).toEqual(["c"]);
  });

  it("drops a load-more response when a filter changed meanwhile", async () => {
    const slow = deferred<unknown>();
    tauriMock.invoke
      .mockResolvedValueOnce(ok({ rows: [row("a")], nextCursor: "c1" }))
      .mockReturnValueOnce(slow.promise);
    const store = await load();
    await store.refreshHistory();
    const more = store.loadMoreHistory();
    store.setHistoryFilters({ text: "x" });
    slow.resolve(ok({ rows: [row("late")], nextCursor: null }));
    await more;
    expect(store.history.rows().map((r) => r.jobId)).toEqual(["a"]);
  });

  it("surfaces a query failure", async () => {
    tauriMock.invoke.mockRejectedValue({ contractVersion: 1, code: "VALIDATION", message: "bad", recovery: [], retryable: false });
    const store = await load();
    await store.refreshHistory();
    expect(store.history.status()).toBe("error");
    expect(store.history.error()?.code).toBe("VALIDATION");
  });

  it("reads web fixtures without IPC", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const store = await load();
    await store.refreshHistory();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
    expect(store.history.rows().length).toBeGreaterThan(0);
    const first = store.history.rows()[0].jobId;
    const timeline = await store.getJobTimeline(first);
    expect(timeline.job.id).toBe(first);
    await expect(store.getJobTimeline("nope")).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("web fixture defaults exclude outcomeUnknown and honour filters", async () => {
    const { webJobHistoryPage } = await import("./web-fixtures");
    const states = new Set(webJobHistoryPage().rows.map((r) => r.state));
    expect(states.has("outcomeUnknown")).toBe(false);
    expect(webJobHistoryPage({ states: ["outcomeUnknown"] }).rows.every((r) => r.state === "outcomeUnknown")).toBe(true);
    const paged = webJobHistoryPage({ limit: 1 });
    expect(paged.rows).toHaveLength(1);
    expect(paged.nextCursor).not.toBeNull();
    expect(buildWebQueueFixture().jobs.length).toBeGreaterThan(0);
  });
});
