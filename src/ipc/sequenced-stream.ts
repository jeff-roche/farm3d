/** Listen-before-backfill reconciliation for one event stream on the shared
 *  `farm3d-event-v1` channel: events are buffered while a snapshot loads,
 *  the snapshot is applied, then buffered events past its sequence replay
 *  in order. A sequence gap or a new `streamId` triggers a fresh backfill;
 *  a failed backfill retries after 1, 2, 4, 8, 16, then 30 s.
 *
 *  Only `library-store` uses this (P4 ruling M8). It knows nothing about
 *  event types: the caller filters the channel before `receive` and
 *  interprets each event in `applyEvent`. */

export interface StreamEvent {
  streamId: string;
  sequence: number;
}

export interface StreamSnapshot {
  streamId: string;
  snapshotSequence: number;
}

/** `current` once a snapshot and every event after it are applied;
 *  `uncertain` from a gap or failure until a backfill settles it. */
export type StreamSyncState = "current" | "uncertain";

export interface SequencedStreamOptions<E extends StreamEvent, S extends StreamSnapshot> {
  backfill: () => Promise<S>;
  applySnapshot: (snapshot: S) => void;
  applyEvent: (event: E) => void;
  onSyncState: (state: StreamSyncState) => void;
  onBackfillError?: (error: unknown) => void;
  schedule?: (callback: () => void, delayMs: number) => ReturnType<typeof setTimeout>;
  cancel?: (handle: ReturnType<typeof setTimeout>) => void;
}

const MAX_BUFFERED_EVENTS = 1_024;
const MAX_RETRY_DELAY_MS = 30_000;

export function createSequencedStream<E extends StreamEvent, S extends StreamSnapshot>(
  options: SequencedStreamOptions<E, S>,
) {
  const schedule = options.schedule ?? setTimeout;
  const cancel = options.cancel ?? clearTimeout;
  let streamId: string | undefined;
  let sequence = 0;
  let buffering = true;
  let buffer: E[] = [];
  let syncing = false;
  let resyncWanted = false;
  let failures = 0;
  let retryTimer: ReturnType<typeof setTimeout> | undefined;
  /** Bumped by `dispose`, so a backfill that resolves afterwards is dropped. */
  let generation = 0;
  let disposed = false;

  function hold(event: E): void {
    // Past the cap the buffer is useless (the backfill will cover it), so
    // drop it rather than grow without bound.
    if (buffer.length >= MAX_BUFFERED_EVENTS) buffer = [];
    buffer.push(event);
  }

  function apply(event: E): void {
    if (event.streamId === streamId && event.sequence <= sequence) return;
    if (event.streamId !== streamId || event.sequence !== sequence + 1) {
      hold(event);
      requestResync();
      return;
    }
    sequence = event.sequence;
    options.applyEvent(event);
  }

  function requestResync(): void {
    options.onSyncState("uncertain");
    if (syncing) resyncWanted = true;
    else void synchronize();
  }

  async function synchronize(): Promise<boolean> {
    if (disposed) return false;
    if (retryTimer !== undefined) cancel(retryTimer);
    retryTimer = undefined;
    syncing = true;
    buffering = true;
    resyncWanted = false;
    const current = generation;
    let settled = false;
    try {
      const snapshot = await options.backfill();
      if (disposed || current !== generation) return false;
      streamId = snapshot.streamId;
      sequence = snapshot.snapshotSequence;
      options.applySnapshot(snapshot);
      const replay = buffer
        .filter((event) => event.streamId === streamId && event.sequence > sequence)
        .sort((a, b) => a.sequence - b.sequence);
      buffer = [];
      buffering = false;
      failures = 0;
      for (const event of replay) apply(event);
      settled = true;
      return true;
    } catch (error) {
      if (disposed || current !== generation) return false;
      options.onBackfillError?.(error);
      return false;
    } finally {
      syncing = false;
      if (!disposed && current === generation) {
        if (!settled) {
          options.onSyncState("uncertain");
          const delay = Math.min(1_000 * 2 ** failures, MAX_RETRY_DELAY_MS);
          failures += 1;
          retryTimer = schedule(() => {
            retryTimer = undefined;
            void synchronize();
          }, delay);
        } else if (resyncWanted) {
          void synchronize();
        } else {
          options.onSyncState("current");
        }
      }
    }
  }

  return {
    /** Feed one already-filtered event from the channel. */
    receive(event: E): void {
      if (disposed) return;
      if (buffering) hold(event);
      else apply(event);
    },
    /** Runs the first backfill; resolves `false` if it failed (retries are
     *  already scheduled). */
    start(): Promise<boolean> {
      return synchronize();
    },
    /** Reload from a fresh snapshot now, e.g. after a `CONFLICT`. */
    resync(): void {
      if (!disposed) requestResync();
    },
    dispose(): void {
      disposed = true;
      generation += 1;
      buffer = [];
      if (retryTimer !== undefined) cancel(retryTimer);
      retryTimer = undefined;
    },
  };
}
