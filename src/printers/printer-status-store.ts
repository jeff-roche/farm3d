import { createStore, produce } from "solid-js/store";
import type { PrinterStatusBackfill } from "../generated/contracts/domain/PrinterStatusBackfill";
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { PrinterStatus, PrinterStatusEventPayload, PrinterStatusEventType } from "./types";

const MAX_BUFFERED_EVENTS = 1_024;
const MAX_SEEN_EVENT_IDS = 4_096;

export type StatusEvent = EventEnvelope<PrinterStatusEventType, PrinterStatusEventPayload>;
export type StatusBackfill = PrinterStatusBackfill;
export type StatusSyncState = "syncing" | "current" | "uncertain";

export interface StatusDependencies {
  printerIds: () => string[];
  listen: (handler: (event: StatusEvent) => void) => Promise<() => void>;
  backfill: () => Promise<StatusBackfill>;
  onStatus?: (printerId: string, status: PrinterStatus) => void;
  onStatusRemoved?: (printerId: string) => void;
  schedule?: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
  cancel?: (handle: ReturnType<typeof setTimeout>) => void;
}

export function createPrinterStatusStore(dependencies: StatusDependencies) {
  const [state, setState] = createStore<{ statuses: Record<string, PrinterStatus>; stale: boolean; syncing: boolean }>({
    statuses: {},
    stale: true,
    syncing: false,
  });
  let disposed = false;
  let unlisten: (() => void) | undefined;
  let streamId: string | undefined;
  let sequence = 0;
  let buffering = true;
  let buffer: StatusEvent[] = [];
  const seen = new Set<string>();
  const seenOrder: string[] = [];
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

  function deleteStatus(printerId: string): void {
    setState("statuses", produce((statuses) => {
      delete statuses[printerId];
    }));
  }

  function apply(event: StatusEvent): void {
    if (seen.has(event.eventId)) return;
    if (event.streamId !== streamId || event.sequence !== sequence + 1) {
      enqueue(event);
      scheduleSync();
      return;
    }
    remember(event.eventId);
    sequence = event.sequence;
    if (event.type === "printer.status.changed" && event.payload.type === "changed" && validPrinter(event.subject.id)) {
      setState("statuses", event.subject.id, event.payload.status);
      dependencies.onStatus?.(event.subject.id, event.payload.status);
    } else if (event.type === "printer.status.removed" && event.payload.type === "removed") {
      deleteStatus(event.subject.id);
      dependencies.onStatusRemoved?.(event.subject.id);
    }
  }

  function scheduleSync(): void {
    if (disposed || state.syncing || retryTimer !== undefined) return;
    retryTimer = schedule(() => {
      retryTimer = undefined;
      void synchronize();
    }, Math.min(1_000 * 2 ** retryAttempt, 30_000));
  }

  async function synchronize(): Promise<boolean> {
    if (disposed || state.syncing) return false;
    setState("syncing", true);
    buffering = true;
    const currentGeneration = ++generation;
    const capturedStream = streamId;
    observedStreamDuringSync = undefined;
    resyncRequested = false;
    try {
      const snapshot = await dependencies.backfill();
      if (disposed || currentGeneration !== generation) return false;
      if (capturedStream === undefined && observedStreamDuringSync !== undefined && snapshot.streamId !== observedStreamDuringSync) {
        streamId = observedStreamDuringSync;
        resyncRequested = true;
        setState("stale", true);
        return true;
      }
      if (capturedStream !== undefined && snapshot.streamId !== capturedStream) {
        streamId = snapshot.streamId;
        resyncRequested = true;
        setState("stale", true);
        return true;
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
      for (const printerId of Object.keys(state.statuses)) {
        if (!Object.prototype.hasOwnProperty.call(current, printerId)) {
          deleteStatus(printerId);
          dependencies.onStatusRemoved?.(printerId);
        }
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
      return true;
    } catch {
      buffering = false;
      retryAttempt += 1;
      setState("stale", true);
      return false;
    } finally {
      setState("syncing", false);
      if (state.stale || resyncRequested) scheduleSync();
    }
  }

  function receive(event: StatusEvent): void {
    if (
      disposed
      || event.contractVersion !== 1
      || (event.type !== "printer.status.changed" && event.type !== "printer.status.removed")
    ) return;
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
    try {
      unlisten = await dependencies.listen(receive);
    } catch {
      throw new Error("Printer status monitoring could not start.");
    }
    if (disposed) {
      unlisten();
      unlisten = undefined;
      return;
    }
    if (!await synchronize()) {
      dispose();
      throw new Error("Printer status monitoring could not start.");
    }
  }

  function prune(): void {
    for (const printerId of Object.keys(state.statuses)) {
      if (!validPrinter(printerId)) {
        deleteStatus(printerId);
        dependencies.onStatusRemoved?.(printerId);
      }
    }
  }

  function dispose(): void {
    disposed = true;
    generation += 1;
    buffer = [];
    seen.clear();
    seenOrder.length = 0;
    if (retryTimer !== undefined) cancel(retryTimer);
    retryTimer = undefined;
    unlisten?.();
    unlisten = undefined;
  }

  return {
    dependencies,
    statuses: () => state.statuses,
    stale: () => state.stale,
    syncState: (): StatusSyncState => state.syncing ? "syncing" : state.stale ? "uncertain" : "current",
    start,
    prune,
    dispose,
  };
}
