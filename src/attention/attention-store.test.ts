import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { attentionBackfill, attentionChange, attentionEvent, cameraHealth, incident } from "./test-records";
import type { AttentionEvent, AttentionStreamEvent, Incident } from "./types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const eventMock = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => eventMock);

const STREAM = "stream-attention";

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
  responders = { list_attention: () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0 }) };
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

function eventEnvelope(sequence: number, event: AttentionEvent, streamId = STREAM): AttentionStreamEvent {
  return {
    contractVersion: 1,
    streamId,
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-25T00:00:00Z",
    type: "attention.event.changed",
    subject: { kind: "attentionEvent", id: event.id },
    payload: { type: "eventChanged", event },
  };
}

function incidentEnvelope(sequence: number, record: Incident, streamId = STREAM): AttentionStreamEvent {
  return {
    contractVersion: 1,
    streamId,
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-25T00:00:00Z",
    type: "attention.incident.changed",
    subject: { kind: "incident", id: record.id },
    payload: { type: "incidentChanged", incident: record },
  };
}

function removedEnvelope(printerId: string) {
  return {
    contractVersion: 1,
    streamId: "stream-printer-status",
    sequence: 1,
    eventId: "evt-removed",
    occurredAt: "2026-09-25T00:00:00Z",
    type: "printer.status.removed",
    subject: { kind: "printer", id: printerId },
    payload: { type: "removed" },
  };
}

function emit(event: unknown): void {
  handler({ payload: event });
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>((res) => { resolve = res; });
  return { promise, resolve };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 20; i += 1) await Promise.resolve();
}

const backfills = () => tauriMock.invoke.mock.calls.filter(([name]) => name === "list_attention").length;

describe("startAttention (desktop)", () => {
  it("listens before backfilling: a buffered event past the snapshot's sequence replays after it", async () => {
    const backfill = deferred<unknown>();
    responders.list_attention = () => backfill.promise;
    const { startAttention, attention } = await import("./attention-store");
    const started = startAttention();
    await flush();

    // The listener attaches before the backfill resolves.
    expect(eventMock.listen).toHaveBeenCalledWith("farm3d-event-v1", expect.any(Function));
    expect(backfills()).toBe(1);

    // An event that arrives during the backfill is buffered, not dropped.
    const liveEvent = attentionEvent({ id: "atn-live", revision: 1 });
    emit(eventEnvelope(1, liveEvent));
    expect(attention.event("atn-live")).toBeUndefined();

    backfill.resolve(attentionBackfill({ streamId: STREAM, snapshotSequence: 0 }));
    await started;
    await flush();

    expect(attention.event("atn-live")).toEqual(liveEvent);
    expect(attention.syncState()).toBe("current");
  });

  it("replaces an amended Event by a higher revision", async () => {
    const original = attentionEvent({ id: "atn-1", revision: 1, summary: "first" });
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, open: [original] });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();
    expect(attention.event("atn-1")?.summary).toBe("first");

    const amended = attentionEvent({ id: "atn-1", revision: 2, summary: "second" });
    emit(eventEnvelope(1, amended));
    expect(attention.event("atn-1")?.summary).toBe("second");
    expect(attention.event("atn-1")?.revision).toBe(2);
  });

  it("ignores an older revision than the one already held", async () => {
    const current = attentionEvent({ id: "atn-1", revision: 3, summary: "current" });
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, open: [current] });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();

    const stale = attentionEvent({ id: "atn-1", revision: 2, summary: "stale" });
    emit(eventEnvelope(1, stale));
    expect(attention.event("atn-1")?.summary).toBe("current");
    expect(attention.event("atn-1")?.revision).toBe(3);
  });

  it("actionableCount counts open Events with requiresAction, acknowledged or not", async () => {
    const unacknowledged = attentionEvent({ id: "atn-1", requiresAction: true, acknowledgedAt: null });
    const acknowledged = attentionEvent({ id: "atn-2", requiresAction: true, acknowledgedAt: "2026-09-25T00:00:00Z" });
    const notActionable = attentionEvent({ id: "atn-3", requiresAction: false });
    const resolvedActionable = attentionEvent({ id: "atn-4", requiresAction: true, resolvedAt: "2026-09-25T00:00:00Z", resolution: "operatorResolved" });
    responders.list_attention = () => attentionBackfill({
      streamId: STREAM,
      snapshotSequence: 0,
      open: [unacknowledged, acknowledged, notActionable],
      resolved: [resolvedActionable],
    });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();

    expect(attention.actionableCount()).toBe(2);
  });

  it("unreadCount counts open, unread Events only", async () => {
    const unread = attentionEvent({ id: "atn-1", readAt: null });
    const read = attentionEvent({ id: "atn-2", readAt: "2026-09-25T00:00:00Z" });
    const resolvedUnread = attentionEvent({ id: "atn-3", readAt: null, resolvedAt: "2026-09-25T00:00:00Z", resolution: "conditionCleared" });
    responders.list_attention = () => attentionBackfill({
      streamId: STREAM,
      snapshotSequence: 0,
      open: [unread, read],
      resolved: [resolvedUnread],
    });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();

    expect(attention.unreadCount()).toBe(1);
  });

  it("open() sorts by severity, then firstObservedAt descending, then id", async () => {
    const info = attentionEvent({ id: "atn-info", severity: "info", firstObservedAt: "2026-09-25T00:00:00Z" });
    const fatalOld = attentionEvent({ id: "atn-fatal-old", severity: "fatal", firstObservedAt: "2026-09-20T00:00:00Z" });
    const fatalNew = attentionEvent({ id: "atn-fatal-new", severity: "fatal", firstObservedAt: "2026-09-24T00:00:00Z" });
    const warning = attentionEvent({ id: "atn-warning", severity: "warning", firstObservedAt: "2026-09-25T00:00:00Z" });
    responders.list_attention = () => attentionBackfill({
      streamId: STREAM, snapshotSequence: 0, open: [info, fatalOld, fatalNew, warning],
    });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();

    expect(attention.open().map((e) => e.id)).toEqual(["atn-fatal-new", "atn-fatal-old", "atn-warning", "atn-info"]);
  });

  it("highestOpenSeverity is the most severe open Event's, or null with none open", async () => {
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, open: [] });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();
    expect(attention.highestOpenSeverity()).toBeNull();

    emit(eventEnvelope(1, attentionEvent({ id: "atn-1", severity: "warning" })));
    expect(attention.highestOpenSeverity()).toBe("warning");
    emit(eventEnvelope(2, attentionEvent({ id: "atn-2", severity: "fatal" })));
    expect(attention.highestOpenSeverity()).toBe("fatal");
  });

  it("eventsForPrinter returns open Events whose printerId matches, of any source kind", async () => {
    const printerSourced = attentionEvent({ id: "atn-1", printerId: "prn-1", source: { kind: "printer", id: "prn-1" } });
    const jobOnPrinter = attentionEvent({ id: "atn-2", printerId: "prn-1", source: { kind: "job", id: "job-1" } });
    const otherPrinter = attentionEvent({ id: "atn-3", printerId: "prn-2", source: { kind: "printer", id: "prn-2" } });
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, open: [printerSourced, jobOnPrinter, otherPrinter] });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();

    expect(attention.eventsForPrinter("prn-1").map((e) => e.id).sort()).toEqual(["atn-1", "atn-2"]);
  });

  it("upserts Incidents and camera health from the backfill and live events", async () => {
    const openIncident = incident({ id: "inc-1", revision: 1 });
    const health = cameraHealth({ printerId: "prn-1", state: "ok" });
    responders.list_attention = () => attentionBackfill({
      streamId: STREAM, snapshotSequence: 0, openIncidents: [openIncident], cameraHealth: [health],
    });
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();
    await flush();

    expect(attention.openIncidents()).toEqual([openIncident]);
    expect(attention.cameraHealth("prn-1")).toEqual(health);

    const closed = incident({ id: "inc-1", revision: 2, state: "closed", closedAt: "2026-09-25T00:00:00Z" });
    emit(incidentEnvelope(1, closed));
    expect(attention.openIncidents()).toEqual([]);
    expect(attention.incident("inc-1")).toEqual(closed);

    emit({
      contractVersion: 1, streamId: STREAM, sequence: 2, eventId: "evt-health", occurredAt: "2026-09-25T00:00:00Z",
      type: "camera.health.changed", subject: { kind: "printer", id: "prn-1" },
      payload: { type: "cameraHealthChanged", health: { ...health, state: "failing" } },
    });
    expect(attention.cameraHealth("prn-1")?.state).toBe("failing");
  });

  it("notifies incident listeners only on a live revision bump, not on the backfill", async () => {
    const openIncident = incident({ id: "inc-1", revision: 1 });
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, openIncidents: [openIncident] });
    const { startAttention, onAttentionIncidentChanged } = await import("./attention-store");
    const seen: Incident[] = [];
    const stop = onAttentionIncidentChanged((record) => seen.push(record));
    await startAttention();
    await flush();
    expect(seen).toEqual([]);

    const bumped = incident({ id: "inc-1", revision: 2 });
    emit(incidentEnvelope(1, bumped));
    expect(seen).toEqual([bumped]);

    // A stale revision (lower than the one already held) doesn't notify.
    const stale = incident({ id: "inc-1", revision: 1 });
    emit(incidentEnvelope(2, stale));
    expect(seen).toEqual([bumped]);
    stop();
  });

  it("notifies snapshot listeners for attention.snapshot.changed", async () => {
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0 });
    const { startAttention, onAttentionSnapshotChanged } = await import("./attention-store");
    const seen: string[] = [];
    const stop = onAttentionSnapshotChanged((snapshot) => seen.push(snapshot.id));
    await startAttention();
    await flush();

    emit({
      contractVersion: 1, streamId: STREAM, sequence: 1, eventId: "evt-snap", occurredAt: "2026-09-25T00:00:00Z",
      type: "attention.snapshot.changed", subject: { kind: "cameraSnapshot", id: "snp-1" },
      payload: { type: "snapshotChanged", snapshot: { id: "snp-1", revision: 1, printerId: "prn-1", incidentId: null, jobId: null, trigger: "manual", capturedAt: "2026-09-25T00:00:00Z", contentType: "image/png", byteLen: 1, sha256: "x", pinnedAt: null, prunedAt: null, pruneReason: null } },
    });
    expect(seen).toEqual(["snp-1"]);
    stop();
  });

  it("drops camera health and notifies printer-removed listeners on printer.status.removed", async () => {
    const health = cameraHealth({ printerId: "prn-1" });
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, cameraHealth: [health] });
    const { startAttention, attention, onAttentionPrinterRemoved } = await import("./attention-store");
    const removed: string[] = [];
    const stop = onAttentionPrinterRemoved((id) => removed.push(id));
    await startAttention();
    await flush();
    expect(attention.cameraHealth("prn-1")).toEqual(health);

    emit(removedEnvelope("prn-1"));
    expect(attention.cameraHealth("prn-1")).toBeUndefined();
    expect(removed).toEqual(["prn-1"]);
    stop();
  });

  it("a fresh backfill replaces camera health wholesale (a removed Printer's row is simply absent)", async () => {
    const health = cameraHealth({ printerId: "prn-1" });
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, cameraHealth: [health] });
    const { startAttention, attention, refreshAttention } = await import("./attention-store");
    await startAttention();
    await flush();
    expect(attention.cameraHealth("prn-1")).toEqual(health);

    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, cameraHealth: [] });
    refreshAttention();
    await flush();
    expect(attention.cameraHealth("prn-1")).toBeUndefined();
  });

  it("markAttentionRead/acknowledgeAttentionEvent/resolveAttentionEvent send a fresh operationId and adopt the change", async () => {
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0 });
    responders.mark_attention_read = () => attentionChange({ events: [attentionEvent({ id: "atn-1", readAt: "2026-09-25T00:00:00Z" })] });
    responders.acknowledge_attention_event = () => attentionChange({ events: [attentionEvent({ id: "atn-2", acknowledgedAt: "2026-09-25T00:00:00Z" })] });
    responders.resolve_attention_event = () => attentionChange({ events: [attentionEvent({ id: "atn-3", resolvedAt: "2026-09-25T00:00:00Z", resolution: "operatorResolved" })] });
    const { startAttention, markAttentionRead, acknowledgeAttentionEvent, resolveAttentionEvent, attention } = await import("./attention-store");
    await startAttention();
    await flush();

    await markAttentionRead(["atn-1"]);
    expect(attention.event("atn-1")?.readAt).toBe("2026-09-25T00:00:00Z");
    expect(tauriMock.invoke).toHaveBeenCalledWith("mark_attention_read", expect.objectContaining({ eventIds: ["atn-1"], operationId: expect.any(String) }));

    await acknowledgeAttentionEvent("atn-2");
    expect(attention.event("atn-2")?.acknowledgedAt).toBe("2026-09-25T00:00:00Z");

    await resolveAttentionEvent("atn-3");
    expect(attention.event("atn-3")?.resolution).toBe("operatorResolved");
  });

  it("loadMoreResolved appends the next page and advances the cursor", async () => {
    const firstPage = attentionEvent({ id: "atn-1", resolvedAt: "2026-09-24T00:00:00Z" });
    responders.list_attention = () => attentionBackfill({ streamId: STREAM, snapshotSequence: 0, resolved: [firstPage], resolvedCursor: "cursor-1" });
    const { startAttention, loadMoreResolved, attention } = await import("./attention-store");
    await startAttention();
    await flush();
    expect(attention.resolvedCursor()).toBe("cursor-1");

    const secondPage = attentionEvent({ id: "atn-2", resolvedAt: "2026-09-20T00:00:00Z" });
    responders.list_attention = (args) => {
      expect(args.resolvedBefore).toBe("cursor-1");
      return attentionBackfill({ streamId: STREAM, snapshotSequence: 0, resolved: [secondPage], resolvedCursor: null });
    };
    await loadMoreResolved();
    expect(attention.resolvedCursor()).toBeNull();
    expect(attention.resolved().map((e) => e.id).sort()).toEqual(["atn-1", "atn-2"]);
  });
});

describe("startAttention (web)", () => {
  it("loads the deterministic web fixture instead of calling any command", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { startAttention, attention } = await import("./attention-store");
    await startAttention();

    expect(tauriMock.invoke).not.toHaveBeenCalled();
    expect(attention.status()).toBe("ready");
    expect(attention.syncState()).toBe("current");
    expect(attention.open().length).toBeGreaterThan(0);
  });

  it("write commands refuse with needsDesktopError", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { markAttentionRead } = await import("./attention-store");
    await expect(markAttentionRead(["atn-1"])).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
  });
});
