import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { navigation } from "../navigation/navigation-store";
import {
  loadWebQueueFixture,
  queueStoreMock,
  resetQueueStoreMock,
  setQueueStoreState,
} from "../queue/queue-store-mock";
import { eligibilitySummary, queueEntry } from "../queue/test-records";
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

  it("moves an entry down one position with Alt+ArrowDown on its handle", async () => {
    loadWebQueueFixture();
    render(() => <QueueScreen />);
    const first = rowFor(/Copy 1 of 3/);
    const handle = within(first).getByRole("button", { name: /^Reorder / });
    fireEvent.keyDown(handle, { key: "ArrowDown", altKey: true });
    await waitFor(() => expect(queueStoreMock.moveQueueEntry).toHaveBeenCalledOnce());
    expect(queueStoreMock.moveQueueEntry).toHaveBeenCalledWith(WEB_QUEUE_ENTRY_BRACKET_IDS[0], 1, 2);
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
