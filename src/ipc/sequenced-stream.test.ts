import { describe, expect, it, vi } from "vitest";
import { createSequencedStream, type StreamEvent, type StreamSnapshot } from "./sequenced-stream";

type Event = StreamEvent & { id: string };

function event(sequence: number, streamId = "s1"): Event {
  return { streamId, sequence, id: `e${sequence}` };
}

function harness(backfill: () => Promise<StreamSnapshot>) {
  const timers: { callback: () => void; delayMs: number }[] = [];
  const applied: string[] = [];
  const snapshots: number[] = [];
  const states: string[] = [];
  const stream = createSequencedStream<Event, StreamSnapshot>({
    backfill,
    applySnapshot: (snapshot) => snapshots.push(snapshot.snapshotSequence),
    applyEvent: (e) => applied.push(e.id),
    onSyncState: (state) => states.push(state),
    schedule: (callback, delayMs) => {
      timers.push({ callback, delayMs });
      return timers.length as unknown as ReturnType<typeof setTimeout>;
    },
    cancel: () => {},
  });
  return { stream, timers, applied, snapshots, states };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

describe("createSequencedStream", () => {
  it("backs off 1, 2, 4, 8, 16, then 30 s between failed backfills, and resets after a success", async () => {
    const backfill = vi.fn((): Promise<StreamSnapshot> => Promise.reject(new Error("down")));
    const { stream, timers, states } = harness(backfill);
    expect(await stream.start()).toBe(false);
    expect(states[states.length - 1]).toBe("uncertain");

    for (let i = 0; i < 6; i += 1) {
      timers[i].callback();
      await flush();
    }
    expect(timers.map((t) => t.delayMs)).toEqual([1_000, 2_000, 4_000, 8_000, 16_000, 30_000, 30_000]);

    backfill.mockImplementation(() => Promise.resolve({ streamId: "s1", snapshotSequence: 0 }));
    timers[6].callback();
    await flush();
    expect(states[states.length - 1]).toBe("current");

    backfill.mockImplementation(() => Promise.reject(new Error("down again")));
    stream.receive(event(2));
    await flush();
    expect(timers[timers.length - 1].delayMs).toBe(1_000);
  });

  it("applies in-order events, drops duplicates, and resyncs on a gap or a new stream", async () => {
    const backfill = vi.fn(() => Promise.resolve({ streamId: "s1", snapshotSequence: 1 }));
    const { stream, applied, snapshots } = harness(backfill);
    await stream.start();
    stream.receive(event(1));
    stream.receive(event(2));
    stream.receive(event(2));
    expect(applied).toEqual(["e2"]);

    backfill.mockImplementation(() => Promise.resolve({ streamId: "s1", snapshotSequence: 4 }));
    stream.receive(event(5));
    await flush();
    expect(snapshots).toEqual([1, 4]);
    expect(applied).toEqual(["e2", "e5"]);

    backfill.mockImplementation(() => Promise.resolve({ streamId: "s2", snapshotSequence: 0 }));
    stream.receive(event(1, "s2"));
    await flush();
    expect(snapshots).toEqual([1, 4, 0]);
    expect(applied).toEqual(["e2", "e5", "e1"]);
  });

  it("stops listening for work once disposed, even with a backfill in flight", async () => {
    let resolve!: (snapshot: StreamSnapshot) => void;
    const { stream, snapshots, timers } = harness(() => new Promise((r) => (resolve = r)));
    const started = stream.start();
    stream.dispose();
    resolve({ streamId: "s1", snapshotSequence: 3 });
    expect(await started).toBe(false);
    stream.receive(event(9));
    await flush();
    expect(snapshots).toEqual([]);
    expect(timers).toEqual([]);
  });
});
