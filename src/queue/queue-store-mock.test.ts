import { beforeEach, describe, expect, it } from "vitest";
import {
  loadWebQueueFixture,
  queueStoreMock,
  resetQueueStoreMock,
  setQueueStoreState,
} from "./queue-store-mock";
import { queueEntry } from "./test-records";
import { WEB_QUEUE_ENTRY_BLOCKED } from "./web-fixtures";

describe("queueStoreMock", () => {
  beforeEach(() => resetQueueStoreMock());

  it("mirrors every export of the real store's API", async () => {
    const real = await import("./queue-store");
    expect(Object.keys(queueStoreMock).sort()).toEqual(Object.keys(real).sort());
    expect(Object.keys(queueStoreMock.queue).sort()).toEqual(Object.keys(real.queue).sort());
  });

  it("serves web-fixtures.ts' Queue", () => {
    const fixture = loadWebQueueFixture();
    expect(fixture.entries.length).toBeGreaterThan(0);
    expect(queueStoreMock.queue.entry(WEB_QUEUE_ENTRY_BLOCKED)?.id).toBe(WEB_QUEUE_ENTRY_BLOCKED);
  });

  it("reacts when a test sets state directly", () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-1", state: "queued", position: 1 })] });
    expect(queueStoreMock.queue.entries().map((entry) => entry.id)).toEqual(["qen-1"]);
    resetQueueStoreMock();
    expect(queueStoreMock.queue.entries()).toEqual([]);
  });
});
