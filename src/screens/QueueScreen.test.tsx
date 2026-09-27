import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { navigation } from "../navigation/navigation-store";
import {
  loadWebQueueFixture,
  queueStoreMock,
  resetQueueStoreMock,
  setQueueStoreState,
  setQueueStoreStatus,
} from "../queue/queue-store-mock";
import { eligibilitySummary, job, queueEntry } from "../queue/test-records";
import {
  WEB_QUEUE_ENTRY_BLOCKED,
  WEB_QUEUE_ENTRY_BRACKET_IDS,
  WEB_QUEUE_ENTRY_HISTORY_DEFERRED,
  WEB_QUEUE_ENTRY_PRINTING,
} from "../queue/web-fixtures";
import { QueueScreen } from "./QueueScreen";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
vi.mock("../library/library-store", async () => (await import("../library/library-store-mock")).libraryStoreMock);
vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);
vi.mock("../printers/printer-store", () => ({ printers: () => [] }));

function openQueue() {
  navigation.navigate({ version: 1, destination: "queue" }, { availableDestinations: ["queue"], availableIds: [] });
}

/** The data rows of the Queue table, in DOM order (the header row is not one). */
function bodyRows(): HTMLElement[] {
  const table = screen.getByRole("grid", { name: "Queue" });
  return within(table).getAllByRole("row").filter((row) => row.closest("tbody"));
}

function rowFor(name: RegExp | string): HTMLElement {
  const row = bodyRows().find((candidate) =>
    typeof name === "string" ? candidate.textContent?.includes(name) : name.test(candidate.textContent ?? ""));
  if (!row) throw new Error(`No Queue row matching ${String(name)}`);
  return row;
}

beforeEach(() => {
  resetQueueStoreMock();
  openQueue();
  window.location.hash = "";
});
afterEach(cleanup);

describe("QueueScreen", () => {
  it("renders open entries in position order", () => {
    setQueueStoreState({
      entries: [
        queueEntry({ id: "qen-c", position: 3, display: { ...queueEntry().display, modelName: "Charlie" } }),
        queueEntry({ id: "qen-a", position: 1, display: { ...queueEntry().display, modelName: "Alpha" } }),
        queueEntry({ id: "qen-b", position: 2, display: { ...queueEntry().display, modelName: "Bravo" } }),
      ],
      eligibility: ["qen-a", "qen-b", "qen-c"].map((entryId) => eligibilitySummary({ entryId })),
    });
    render(() => <QueueScreen />);
    const names = bodyRows().map((row) => row.textContent ?? "");
    expect(names[0]).toContain("Alpha");
    expect(names[1]).toContain("Bravo");
    expect(names[2]).toContain("Charlie");
  });

  it("filters rows by each view tab", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    // The default tab is the whole open Queue, in order.
    expect(bodyRows()).toHaveLength(5);

    fireEvent.click(screen.getByRole("tab", { name: /^Awaiting operator/ }));
    await waitFor(() => expect(bodyRows()).toHaveLength(3));
    expect(bodyRows().every((row) => row.textContent?.includes("Copy"))).toBe(true);

    fireEvent.click(screen.getByRole("tab", { name: /^Blocked/ }));
    await waitFor(() => expect(bodyRows()).toHaveLength(1));
    expect(bodyRows()[0]).toHaveAttribute("data-entry-id", WEB_QUEUE_ENTRY_BLOCKED);

    fireEvent.click(screen.getByRole("tab", { name: /^Printing now/ }));
    await waitFor(() => expect(bodyRows()[0]).toHaveAttribute("data-entry-id", WEB_QUEUE_ENTRY_PRINTING));
    expect(bodyRows()).toHaveLength(1);

    fireEvent.click(screen.getByRole("tab", { name: /^History/ }));
    await waitFor(() => expect(bodyRows()[0]).toHaveAttribute("data-entry-id", WEB_QUEUE_ENTRY_HISTORY_DEFERRED));
    expect(bodyRows()).toHaveLength(1);
    // A deferred settlement shows in the History view.
    expect(bodyRows()[0]).toHaveTextContent("Deferred");

    fireEvent.click(screen.getByRole("tab", { name: /^Ready/ }));
    await waitFor(() => expect(screen.getByText("No entries in this view")).toBeInTheDocument());
  });

  it("lists an assigned entry whose Job isn't printing under Assigned", async () => {
    setQueueStoreState({
      entries: [
        queueEntry({ id: "qen-queued", position: 1 }),
        queueEntry({ id: "qen-assigned", position: 2, state: "assigned", jobId: "job-a", allowedActions: ["move"] }),
      ],
      jobs: [job({ id: "job-a", queueEntryId: "qen-assigned", state: "awaitingStart" })],
      eligibility: [eligibilitySummary({ entryId: "qen-queued" })],
    });
    render(() => <QueueScreen />);
    fireEvent.click(screen.getByRole("tab", { name: /^Assigned/ }));
    await waitFor(() => expect(bodyRows()).toHaveLength(1));
    expect(bodyRows()[0]).toHaveAttribute("data-entry-id", "qen-assigned");
    expect(bodyRows()[0]).toHaveTextContent("Awaiting start");
  });

  it("moves an entry down one position with Alt+ArrowDown on its handle", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    const first = rowFor(/Copy 1 of 3/);
    const handle = within(first).getByRole("button", { name: /^Reorder / });
    fireEvent.keyDown(handle, { key: "ArrowDown", altKey: true });
    await waitFor(() => expect(queueStoreMock.moveQueueEntry).toHaveBeenCalledOnce());
    expect(queueStoreMock.moveQueueEntry).toHaveBeenCalledWith(WEB_QUEUE_ENTRY_BRACKET_IDS[0], 1, 2);
  });

  it("sends a move inside a filtered view as the target row's absolute position, and announces it", async () => {
    const blocked = eligibilitySummary({ verdict: "blocked", eligibleCount: 0, candidatePrinterIds: [] });
    setQueueStoreState({
      entries: [1, 2, 3, 4, 5].map((position) => queueEntry({
        id: `qen-${position}`, position, revision: 10 + position,
        display: { ...queueEntry().display, modelName: `Part ${position}` },
      })),
      eligibility: [1, 2, 3, 4, 5].map((position) => (position === 2 || position === 5
        ? { ...blocked, entryId: `qen-${position}` }
        : eligibilitySummary({ entryId: `qen-${position}` }))),
    });
    render(() => <QueueScreen />);
    fireEvent.click(screen.getByRole("tab", { name: /^Blocked/ }));
    await waitFor(() => expect(bodyRows()).toHaveLength(2));
    fireEvent.keyDown(within(rowFor("Part 2")).getByRole("button", { name: /^Reorder / }), { key: "ArrowDown", altKey: true });
    await waitFor(() => expect(queueStoreMock.moveQueueEntry).toHaveBeenCalledWith("qen-2", 12, 5));
    const live = rowFor("Part 2").querySelector('[aria-live="polite"]');
    expect(live).toHaveTextContent("Moved Part 2 — Plate 1 to position 5 of 5");
  });

  it("disables every handle while a move is pending, until the moved entry's revision changes", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    const first = () => within(rowFor(/Copy 1 of 3/)).getByRole("button", { name: /^Reorder / });
    fireEvent.keyDown(first(), { key: "ArrowDown", altKey: true });
    await waitFor(() => expect(queueStoreMock.moveQueueEntry).toHaveBeenCalledOnce());
    // A second move during the first sends nothing.
    await waitFor(() => expect(first()).toBeDisabled());
    fireEvent.keyDown(within(rowFor(/Copy 2 of 3/)).getByRole("button", { name: /^Reorder / }), { key: "ArrowUp", altKey: true });
    expect(queueStoreMock.moveQueueEntry).toHaveBeenCalledOnce();
    // The moved entry's new revision arrives: the handles come back.
    const entries = queueStoreMock.queue.entries().map((entry) =>
      entry.id === WEB_QUEUE_ENTRY_BRACKET_IDS[0] ? { ...entry, revision: entry.revision + 1 } : entry);
    setQueueStoreState({ entries: [...entries, ...queueStoreMock.queue.history()] });
    await waitFor(() => expect(first()).toBeEnabled());
  });

  it("re-enables the handles and refreshes the Queue when a move is refused as stale", async () => {
    loadWebQueueFixture();
    queueStoreMock.moveQueueEntry.mockRejectedValueOnce({
      contractVersion: 1, code: "CONFLICT", message: "This Queue Entry changed. Reload.", recovery: ["RELOAD"], retryable: false,
    });
    render(() => <QueueScreen />);
    const first = () => within(rowFor(/Copy 1 of 3/)).getByRole("button", { name: /^Reorder / });
    fireEvent.keyDown(first(), { key: "ArrowDown", altKey: true });
    expect(await screen.findByRole("alert")).toHaveTextContent("This Queue Entry changed. Reload.");
    expect(queueStoreMock.refreshQueue).toHaveBeenCalledOnce();
    expect(first()).toBeEnabled();
  });

  it("shows a disabled handle for an open entry Rust doesn't let move", () => {
    setQueueStoreState({
      entries: [queueEntry({ id: "qen-fixed", allowedActions: ["assign"] })],
      eligibility: [eligibilitySummary({ entryId: "qen-fixed" })],
    });
    render(() => <QueueScreen />);
    expect(within(rowFor("Bracket")).getByRole("button", { name: /^Reorder / })).toBeDisabled();
  });

  it("maps a move inside a filtered view to the target row's own position", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    fireEvent.click(screen.getByRole("tab", { name: /^Blocked/ }));
    await waitFor(() => expect(bodyRows()).toHaveLength(1));
    // The only row in its view: nowhere to move.
    const handle = within(bodyRows()[0]).getByRole("button", { name: /^Reorder / });
    fireEvent.keyDown(handle, { key: "ArrowUp", altKey: true });
    expect(queueStoreMock.moveQueueEntry).not.toHaveBeenCalled();
  });

  it("marks every open row as a reorder row, as ReorderHandle's drag geometry needs", () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    expect(bodyRows().every((row) => row.hasAttribute("data-reorder-row"))).toBe(true);
  });

  it("shows a blocked row's first blocker and its recovery action", () => {
    setQueueStoreState({
      entries: [queueEntry({ id: "qen-blocked" })],
      eligibility: [eligibilitySummary({
        entryId: "qen-blocked",
        verdict: "blocked",
        eligibleCount: 0,
        candidatePrinterIds: [],
        topBlocker: {
          code: "NO_COMPATIBLE_SPOOL", message: "No Spool of PLA 1.75 mm is available.", detail: null,
          recovery: "LOAD_SPOOL", printerIds: [],
        },
      })],
    });
    render(() => <QueueScreen />);
    const row = rowFor("Bracket");
    expect(row).toHaveTextContent("Blocked");
    expect(row).toHaveTextContent("No Spool of PLA 1.75 mm is available.");
    fireEvent.click(within(row).getByRole("button", { name: "Open Spools" }));
    expect(window.location.hash).toBe("#nav=v1/spools");
  });

  it("shows three linked copies as Copy n of 3, and highlights the lineage on hover", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    expect(screen.getByText("Copy 1 of 3")).toBeInTheDocument();
    expect(screen.getByText("Copy 2 of 3")).toBeInTheDocument();
    expect(screen.getByText("Copy 3 of 3")).toBeInTheDocument();

    fireEvent.pointerOver(rowFor(/Copy 2 of 3/));
    await waitFor(() => expect(rowFor(/Copy 1 of 3/)).toHaveAttribute("data-lineage-highlight"));
    expect(rowFor(/Copy 3 of 3/)).toHaveAttribute("data-lineage-highlight");
    expect(bodyRows().find((row) => row.dataset.entryId === WEB_QUEUE_ENTRY_BLOCKED)).not.toHaveAttribute("data-lineage-highlight");
  });

  it("highlights the lineage siblings when a row's control takes focus", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    fireEvent.focusIn(within(rowFor(/Copy 3 of 3/)).getByRole("button", { name: /^Reorder / }));
    await waitFor(() => expect(rowFor(/Copy 1 of 3/)).toHaveAttribute("data-lineage-highlight"));
    expect(rowFor(/Copy 2 of 3/)).toHaveAttribute("data-lineage-highlight");
    fireEvent.focusOut(within(rowFor(/Copy 3 of 3/)).getByRole("button", { name: /^Reorder / }), { relatedTarget: document.body });
    await waitFor(() => expect(rowFor(/Copy 1 of 3/)).not.toHaveAttribute("data-lineage-highlight"));
  });

  it("shows a loading placeholder, not the empty Queue, while the Queue loads", () => {
    setQueueStoreStatus("loading", "syncing");
    render(() => <QueueScreen />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading the Queue…");
    expect(screen.queryByText("The Queue is empty")).toBeNull();
  });

  it("shows a load failure with Retry", () => {
    setQueueStoreStatus("error", "uncertain");
    render(() => <QueueScreen />);
    expect(screen.getByRole("alert")).toHaveTextContent("The Queue couldn't be loaded.");
    expect(screen.queryByText("The Queue is empty")).toBeNull();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(queueStoreMock.refreshQueue).toHaveBeenCalledOnce();
  });

  it("labels a Queue that may be stale, and offers a refresh", () => {
    loadWebQueueFixture();
    setQueueStoreStatus("ready", "uncertain");
    render(() => <QueueScreen />);
    expect(screen.getByText("The Queue may be out of date")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Refresh" }));
    expect(queueStoreMock.refreshQueue).toHaveBeenCalledOnce();
    expect(bodyRows()).toHaveLength(5);
  });

  it("keeps the policy filter on a filtered-empty view and offers one way out", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    fireEvent.click(screen.getByRole("tab", { name: /^Awaiting operator/ }));
    const automatic = screen.getByRole("button", { name: "Automatic" });
    fireEvent.click(automatic);
    expect(await screen.findByText("No entries match these filters")).toBeInTheDocument();
    expect(automatic).toHaveAttribute("aria-pressed", "true");
    fireEvent.click(screen.getByRole("button", { name: "Clear policy filters" }));
    await waitFor(() => expect(bodyRows()).toHaveLength(3));
    expect(automatic).toHaveAttribute("aria-pressed", "false");
  });

  it("offers the Library when the Queue is empty", () => {
    render(() => <QueueScreen />);
    expect(screen.getByText("The Queue is empty")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open the Library" }));
    expect(window.location.hash).toBe("#nav=v1/library");
  });

  it("opens an entry's detail from its row, and resolves a Job id to its entry", async () => {
    const fixture = loadWebQueueFixture();
    render(() => <QueueScreen />);
    fireEvent.click(rowFor(/Copy 2 of 3/));
    expect(window.location.hash).toBe(`#nav=v1/queue/job/${WEB_QUEUE_ENTRY_BRACKET_IDS[1]}`);
    expect(await screen.findByRole("tab", { name: "Dispatch" })).toBeInTheDocument();

    const printingJob = fixture.jobs.find((j) => j.queueEntryId === WEB_QUEUE_ENTRY_PRINTING)!;
    navigation.navigate(
      { version: 1, destination: "queue", selection: { kind: "job", id: printingJob.id } },
      { availableDestinations: ["queue"], availableIds: [printingJob.id] },
    );
    await waitFor(() => expect(rowFor(/Four-tool/)).toHaveAttribute("aria-selected", "true"));
  });
});
