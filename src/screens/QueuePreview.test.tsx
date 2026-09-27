import { cleanup, fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { loadWebQueueFixture, queueStoreMock, resetQueueStoreMock, setQueueStoreState } from "../queue/queue-store-mock";
import { eligibilitySummary, job, queueEntry } from "../queue/test-records";
import { WEB_QUEUE_JOB_DEFERRED, WEB_QUEUE_JOB_PRINTING } from "../queue/web-fixtures";
import { QueuePreview } from "./QueuePreview";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools: [], tares: [] };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));
vi.mock("../printers/printer-store", () => ({ printers: () => [] }));

beforeEach(() => {
  resetQueueStoreMock();
  window.location.hash = "";
});
afterEach(cleanup);

describe("QueuePreview", () => {
  it("shows the next five queued entries in order", () => {
    const entries = Array.from({ length: 7 }, (_, index) =>
      queueEntry({ id: `qen-${index + 1}`, position: index + 1, display: { ...queueEntry().display, modelName: `Part ${index + 1}` } }));
    setQueueStoreState({ entries, eligibility: entries.map((entry) => eligibilitySummary({ entryId: entry.id })) });
    render(() => <QueuePreview />);
    const next = screen.getByRole("list", { name: "Next up" });
    expect(within(next).getAllByRole("listitem").map((item) => item.textContent)).toEqual(
      ["Part 1", "Part 2", "Part 3", "Part 4", "Part 5"].map((name) => expect.stringContaining(name)),
    );
  });

  it("lists active Jobs and opens one when chosen", () => {
    loadWebQueueFixture();
    const onSelectJob = vi.fn();
    render(() => <QueuePreview onSelectJob={onSelectJob} />);
    const active = screen.getByRole("list", { name: "Active Jobs" });
    fireEvent.click(within(active).getByRole("button", { name: /Four-tool — Bay 5/ }));
    expect(onSelectJob).toHaveBeenCalledWith(WEB_QUEUE_JOB_PRINTING);
  });

  it("shows a deferred material requirement with Settle…", async () => {
    loadWebQueueFixture();
    render(() => <QueuePreview />);
    const open = screen.getByRole("list", { name: "Needs reconciliation" });
    expect(open).toHaveTextContent("Material reconciliation");
    expect(open).toHaveTextContent("Deferred");
    fireEvent.click(within(open).getByRole("button", { name: "Settle…" }));
    const dialog = await screen.findByRole("dialog", { name: "Settle material" });
    expect(within(dialog).getByLabelText("Use estimate (2.0 g)")).toBeInTheDocument();
    expect(queueStoreMock.queue.job(WEB_QUEUE_JOB_DEFERRED)?.settlement).toBe("deferred");
  });

  it("says what automatic dispatch will do next", () => {
    setQueueStoreState({
      entries: [queueEntry({ id: "qen-1" })],
      jobs: [job({ state: "printing" })],
      nextAutomaticAction: {
        kind: "waiting", evaluatedAt: "2026-09-25T00:00:00Z", entryId: "qen-1",
        blocker: { code: "PRINTER_OFFLINE", message: "Every candidate Printer is offline.", detail: null, recovery: null, printerIds: [] },
      },
    });
    render(() => <QueuePreview />);
    expect(screen.getByText(/Every candidate Printer is offline\./)).toBeInTheDocument();
  });

  it("opens the Queue", () => {
    render(() => <QueuePreview />);
    fireEvent.click(screen.getByRole("button", { name: "Open the Queue" }));
    expect(window.location.hash).toBe("#nav=v1/queue");
  });
});
