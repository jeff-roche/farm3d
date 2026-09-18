import { createStore } from "solid-js/store";
import type { PrinterStatusBackfill } from "../generated/contracts/domain/PrinterStatusBackfill";
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { PrinterStatus } from "./types";

const MAX_BUFFERED_EVENTS = 1_024;
const MAX_SEEN_EVENT_IDS = 4_096;

export type StatusEvent = EventEnvelope<"printer.status.changed", PrinterStatus>;
export type StatusBackfill = PrinterStatusBackfill;

export interface StatusDependencies {
  printerIds: () => string[];
  listen: (handler: (event: StatusEvent) => void) => Promise<() => void>;
  backfill: () => Promise<StatusBackfill>;
  onStatus?: (printerId: string, status: PrinterStatus) => void;
  schedule?: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
  cancel?: (handle: ReturnType<typeof setTimeout>) => void;
}

export function createPrinterStatusStore(dependencies: StatusDependencies) {
  const [state, setState] = createStore<{ statuses: Record<string, PrinterStatus>; stale: boolean }>({
    statuses: {},
    stale: true,
  });
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let streamId: string | undefined;
  let sequence = 0;
  let buffering = true;
  let buffer: StatusEvent[] = [];
  const seen = new Set<string>();
  const seenOrder: string[] = [];
  let syncing = false;
  let resyncRequested = false;
  let bufferOverflowed = false;
  let observedStreamDuringSync: string | undefined;
  let retryAttempt = 0;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;
  const schedule = dependencies.schedule ?? setTimeout;
  const cancel = dependencies.cancel ?? clearTimeout;

  const validPrinter = (id: string) => dependencies.printerIds().includes(id);
  function remember(eventId: string): void {
    if (seen.has(eventId)) return;
    seen.add(eventId);
    seenOrder.push(eventId);
    if (seenOrder.length > MAX_SEEN_EVENT_IDS) seen.delete(seenOrder.shift()!);
  }

  function enqueue(event: StatusEvent): void {
    if (buffer.length >= MAX_BUFFERED_EVENTS) {
      buffer = [];
      bufferOverflowed = true;
      resyncRequested = true;
    } else {
      buffer.push(event);
    }
    setState("stale", true);
  }

  function apply(event: StatusEvent): void {
    if (!validPrinter(event.subject.id) || seen.has(event.eventId)) return;
    if (event.streamId !== streamId || event.sequence !== sequence + 1) {
      enqueue(event);
      scheduleSync();
      return;
    }
    remember(event.eventId);
    sequence = event.sequence;
    setState("statuses", event.subject.id, event.payload);
    dependencies.onStatus?.(event.subject.id, event.payload);
  }

  function scheduleSync(): void {
    if (disposed || syncing || retryTimer !== undefined) return;
    retryTimer = schedule(() => {
      retryTimer = undefined;
      void synchronize();
    }, Math.min(1_000 * 2 ** retryAttempt, 30_000));
  }

  async function synchronize(): Promise<void> {
    if (disposed || syncing) return;
    syncing = true;
    buffering = true;
    const currentGeneration = ++generation;
    const capturedStream = streamId;
    observedStreamDuringSync = undefined;
    resyncRequested = false;
    try {
      const snapshot = await dependencies.backfill();
      if (disposed || currentGeneration !== generation) return;
      if (capturedStream === undefined && observedStreamDuringSync !== undefined && snapshot.streamId !== observedStreamDuringSync) {
        streamId = observedStreamDuringSync;
        resyncRequested = true;
        setState("stale", true);
        return;
      }
      if (capturedStream !== undefined && snapshot.streamId !== capturedStream) {
        streamId = snapshot.streamId;
        resyncRequested = true;
        setState("stale", true);
        return;
      }
      streamId = snapshot.streamId;
      sequence = snapshot.snapshotSequence;
      seen.clear();
      seenOrder.length = 0;
      const current: Record<string, PrinterStatus> = {};
      for (const row of snapshot.statuses) if (validPrinter(row.printerId)) {
        current[row.printerId] = row.status;
        dependencies.onStatus?.(row.printerId, row.status);
      }
      setState("statuses", current);
      const replay = buffer.filter((candidate) => candidate.streamId === streamId && candidate.sequence > sequence)
        .sort((left, right) => left.sequence - right.sequence);
      buffer = [];
      buffering = false;
      for (const candidate of replay) apply(candidate);
      retryAttempt = 0;
      if (bufferOverflowed) {
        bufferOverflowed = false;
        resyncRequested = true;
      }
      setState("stale", resyncRequested || buffer.length > 0);
    } catch {
      buffering = false;
      retryAttempt += 1;
      setState("stale", true);
    } finally {
      syncing = false;
      if (state.stale || resyncRequested) scheduleSync();
    }
  }

  function receive(event: StatusEvent): void {
    if (disposed || event.contractVersion !== 1 || event.type !== "printer.status.changed") return;
    if (buffering) {
      if (streamId === undefined) {
        observedStreamDuringSync = event.streamId;
      } else if (event.streamId !== streamId) {
        streamId = event.streamId;
        generation += 1;
        resyncRequested = true;
      }
      enqueue(event);
    }
    else apply(event);
  }

  async function start(): Promise<void> {
    unlisten = await dependencies.listen(receive);
    if (disposed) {
      unlisten();
      unlisten = undefined;
      return;
    }
    await synchronize();
  }

  return {
    dependencies,
    statuses: () => state.statuses,
    stale: () => state.stale,
    start,
    dispose() {
      disposed = true;
      generation += 1;
      buffer = [];
      seen.clear();
      seenOrder.length = 0;
      if (retryTimer !== undefined) cancel(retryTimer);
      retryTimer = undefined;
      unlisten?.();
      unlisten = undefined;
    },
  };
}
