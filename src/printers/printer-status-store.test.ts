import { afterEach, describe, expect, it, vi } from "vitest";
import { createPrinterStatusStore, type StatusEvent } from "./printer-status-store";
import type { PrinterStatus } from "./types";

const status = (state: PrinterStatus["connectionState"]): PrinterStatus => ({
  connectionState: state,
  updatedAt: "2026-09-17T00:00:00Z",
});

const event = (sequence: number, state: PrinterStatus["connectionState"]): StatusEvent => ({
  contractVersion: 1,
  streamId: "stream-a",
  sequence,
  eventId: `event-${sequence}`,
  occurredAt: "2026-09-17T00:00:00Z",
  type: "printer.status.changed",
  subject: { kind: "printer", id: "prn-1" },
  payload: status(state),
});

describe("printer status reconciliation", () => {
  afterEach(() => vi.useRealTimers());
  it("replays an event received while backfill is in flight exactly once", async () => {
    let receive!: (event: StatusEvent) => void;
    let resolveBackfill!: (value: unknown) => void;
    const backfill = new Promise((resolve) => (resolveBackfill = resolve));
    const store = createPrinterStatusStore({
      printerIds: () => ["prn-1"],
      listen: async (handler) => {
        receive = handler;
        return () => undefined;
      },
      backfill: () => backfill as never,
    });

    const started = store.start();
    await Promise.resolve();
    receive(event(2, "online"));
    resolveBackfill({
      streamId: "stream-a",
      snapshotSequence: 1,
      statuses: [{ printerId: "prn-1", status: status("connecting") }],
    });
    await started;
    receive(event(2, "offline"));

    expect(store.statuses()["prn-1"].connectionState).toBe("online");
  });

  it("backfills again after a sequence gap and accepts a restarted stream", async () => {
    vi.useFakeTimers();
    let receive!: (event: StatusEvent) => void;
    const backfill = vi.fn()
      .mockResolvedValueOnce({ streamId: "stream-a", snapshotSequence: 1, statuses: [] })
      .mockResolvedValueOnce({ streamId: "stream-b", snapshotSequence: 4, statuses: [{ printerId: "prn-1", status: status("offline") }] })
      .mockResolvedValueOnce({ streamId: "stream-b", snapshotSequence: 4, statuses: [{ printerId: "prn-1", status: status("online") }] });
    const store = createPrinterStatusStore({
      printerIds: () => ["prn-1"],
      listen: async (handler) => { receive = handler; return () => undefined; },
      backfill,
    });
    await store.start();
    receive({ ...event(3, "offline"), streamId: "stream-b" });
    expect(store.stale()).toBe(true);
    await vi.advanceTimersByTimeAsync(1_000);
    expect(store.statuses()["prn-1"]).toBeUndefined();
    await vi.advanceTimersByTimeAsync(1_000);
    expect(backfill).toHaveBeenCalledTimes(3);
    expect(store.statuses()["prn-1"].connectionState).toBe("online");
    expect(store.stale()).toBe(false);
    store.dispose();
  });

  it("retries a failed backfill and discards statuses for removed printers", async () => {
    vi.useFakeTimers();
    const backfill = vi.fn().mockRejectedValueOnce(new Error("offline")).mockResolvedValueOnce({
      streamId: "stream-a", snapshotSequence: 1,
      statuses: [{ printerId: "removed", status: status("online") }],
    });
    const store = createPrinterStatusStore({ printerIds: () => ["prn-1"], listen: async () => () => undefined, backfill });
    await store.start();
    expect(store.stale()).toBe(true);
    await vi.advanceTimersByTimeAsync(2_000);
    expect(store.statuses()).toEqual({});
    expect(store.stale()).toBe(false);
    store.dispose();
  });

  it("unlistens when disposed before listener registration resolves", async () => {
    let resolveListen!: (value: () => void) => void;
    const unlisten = vi.fn();
    const store = createPrinterStatusStore({
      printerIds: () => [],
      listen: () => new Promise((resolve) => (resolveListen = resolve)),
      backfill: vi.fn(),
    });
    const started = store.start();
    store.dispose();
    resolveListen(unlisten);
    await started;
    expect(unlisten).toHaveBeenCalledOnce();
    expect(store.dependencies.backfill).not.toHaveBeenCalled();
  });

  it("ignores a slow old-stream backfill after a newer stream is observed", async () => {
    vi.useFakeTimers();
    let receive!: (event: StatusEvent) => void;
    let resolveFirst!: (value: unknown) => void;
    const first = new Promise((resolve) => (resolveFirst = resolve));
    const backfill = vi.fn()
      .mockReturnValueOnce(first)
      .mockResolvedValueOnce({
        streamId: "stream-b",
        snapshotSequence: 2,
        statuses: [{ printerId: "prn-1", status: status("online") }],
      });
    const store = createPrinterStatusStore({
      printerIds: () => ["prn-1"],
      listen: async (handler) => { receive = handler; return () => undefined; },
      backfill,
    });

    const started = store.start();
    await Promise.resolve();
    receive({ ...event(1, "connecting"), streamId: "stream-b" });
    resolveFirst({
      streamId: "stream-a",
      snapshotSequence: 9,
      statuses: [{ printerId: "prn-1", status: status("offline") }],
    });
    await started;
    await vi.advanceTimersByTimeAsync(1_000);

    expect(backfill).toHaveBeenCalledTimes(2);
    expect(store.statuses()["prn-1"].connectionState).toBe("online");
    store.dispose();
  });

  it("caps buffered events and forces a clean backfill after overflow", async () => {
    let receive!: (event: StatusEvent) => void;
    let resolveBackfill!: (value: unknown) => void;
    const pending = new Promise((resolve) => (resolveBackfill = resolve));
    const backfill = vi.fn().mockReturnValue(pending);
    const store = createPrinterStatusStore({
      printerIds: () => ["prn-1"],
      listen: async (handler) => { receive = handler; return () => undefined; },
      backfill,
    });
    const started = store.start();
    await Promise.resolve();
    for (let sequence = 1; sequence <= 1_025; sequence += 1) receive(event(sequence, "online"));
    resolveBackfill({ streamId: "stream-a", snapshotSequence: 1_025, statuses: [] });
    await started;

    expect(store.stale()).toBe(true);
    expect(store.statuses()).toEqual({});
    store.dispose();
  });
});
