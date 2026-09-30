import { cleanup, fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { commandError } from "../ipc/local-errors";
import {
  historyStoreMock,
  loadWebHistoryFixture,
  resetHistoryStoreMock,
  setHistoryStoreState,
} from "../history/history-store-mock";
import { historyKnownIds } from "../history/known-ids";
import { JobHistoryView } from "./JobHistoryView";

vi.mock("../history/history-store", async () => (await import("../history/history-store-mock")).historyStoreMock);
vi.mock("../printers/printer-store", () => ({
  printers: () => [
    { id: "prn-1", name: "Bay 4" },
    { id: "prn-2", name: "Old Bay", archivedAt: "2026-09-01T00:00:00Z" },
  ],
}));

beforeEach(() => {
  resetHistoryStoreMock();
  window.location.hash = "";
});
afterEach(cleanup);

function bodyRows(): HTMLElement[] {
  const table = screen.getByRole("grid", { name: "Job history" });
  return within(table).getAllByRole("row").filter((row) => row.closest("tbody"));
}

describe("JobHistoryView", () => {
  it("loads history when it opens and shows the rows in a table", () => {
    const rows = loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    expect(historyStoreMock.refreshHistory).toHaveBeenCalledOnce();
    expect(bodyRows()).toHaveLength(rows.length);
    expect(bodyRows()[0]).toHaveTextContent(rows[0].modelName);
    expect(bodyRows()[0]).toHaveTextContent(rows[0].printerSnapshotName);
  });

  it("sends the search text to the store", () => {
    loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    fireEvent.input(screen.getByRole("textbox", { name: "Search history" }), { target: { value: "lid" } });
    expect(historyStoreMock.setHistoryFilters).toHaveBeenCalledWith({ text: "lid" });
  });

  it("toggles a state chip, keeps at least one, and drops back to the default set", () => {
    loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    const unknown = screen.getByRole("button", { name: "Outcome unknown" });
    expect(unknown).toHaveAttribute("aria-pressed", "false");
    fireEvent.click(unknown);
    expect(historyStoreMock.setHistoryFilters).toHaveBeenLastCalledWith({
      states: ["completed", "failed", "cancelled", "outcomeUnknown"],
    });
    setHistoryStoreState({ filters: { states: ["completed", "failed", "cancelled", "outcomeUnknown"] } });
    fireEvent.click(screen.getByRole("button", { name: "Outcome unknown" }));
    expect(historyStoreMock.setHistoryFilters).toHaveBeenLastCalledWith({ states: undefined });
    // The last selected state can't be switched off.
    setHistoryStoreState({ filters: { states: ["failed"] } });
    historyStoreMock.setHistoryFilters.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Failed" }));
    expect(historyStoreMock.setHistoryFilters).not.toHaveBeenCalled();
  });

  it("lists archived Printers in the Printer filter, labelled", async () => {
    loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    fireEvent.pointerDown(screen.getByRole("button", { name: /Printer/ }), { button: 0, pointerType: "mouse" });
    expect(await screen.findByRole("option", { name: "Old Bay (archived)" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Bay 4" })).toBeInTheDocument();
    fireEvent.pointerUp(screen.getByRole("option", { name: "Old Bay (archived)" }), { pointerType: "mouse" });
    expect(historyStoreMock.setHistoryFilters).toHaveBeenCalledWith({ printerId: "prn-2" });
  });

  it("filters by an inclusive start day and end day", () => {
    loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    fireEvent.input(screen.getByLabelText("Ended on or after"), { target: { value: "2026-09-20" } });
    expect(historyStoreMock.setHistoryFilters).toHaveBeenLastCalledWith({
      endedAfter: new Date(2026, 8, 20).toISOString(),
    });
    fireEvent.input(screen.getByLabelText("Ended on or before"), { target: { value: "2026-09-22" } });
    // Exclusive upper bound: the start of the next day.
    expect(historyStoreMock.setHistoryFilters).toHaveBeenLastCalledWith({
      endedBefore: new Date(2026, 8, 23).toISOString(),
    });
  });

  it("selecting a row goes to queue/job/<id>", () => {
    const rows = loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    fireEvent.click(bodyRows()[1]);
    expect(window.location.hash).toBe(`#nav=v1/queue/job/${rows[1].jobId}`);
  });

  it("appends the next page with a real Load more button", () => {
    loadWebHistoryFixture();
    setHistoryStoreState({ hasMore: true });
    render(() => <JobHistoryView />);
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(historyStoreMock.loadMoreHistory).toHaveBeenCalledOnce();
  });

  it("offers no Load more on the last page, and disables it while loading", () => {
    loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    expect(screen.queryByRole("button", { name: "Load more" })).toBeNull();
    setHistoryStoreState({ hasMore: true, loadingMore: true });
    expect(screen.getByRole("button", { name: "Loading…" })).toBeDisabled();
  });

  it("shows a Load more failure on its row and keeps the rows", () => {
    const rows = loadWebHistoryFixture();
    setHistoryStoreState({ hasMore: true, status: "ready", error: commandError("INTERNAL", "The history couldn't be read.") });
    render(() => <JobHistoryView />);
    expect(bodyRows()).toHaveLength(rows.length);
    expect(screen.getByRole("alert")).toHaveTextContent("More Jobs couldn't be loaded.");
    fireEvent.click(screen.getByRole("button", { name: "Load more" }));
    expect(historyStoreMock.loadMoreHistory).toHaveBeenCalledOnce();
  });

  it("says so when there is no history yet", () => {
    setHistoryStoreState({ rows: [], status: "ready" });
    render(() => <JobHistoryView />);
    expect(screen.getByText("No finished Jobs yet")).toBeInTheDocument();
  });

  it("says so when the filters match nothing, and clears them", () => {
    setHistoryStoreState({ rows: [], status: "ready", filters: { text: "zzz" } });
    render(() => <JobHistoryView />);
    expect(screen.getByText("No Jobs match these filters")).toBeInTheDocument();
    fireEvent.click(screen.getAllByRole("button", { name: "Clear filters" })[0]);
    expect(historyStoreMock.setHistoryFilters).toHaveBeenCalledWith({
      text: undefined, states: undefined, printerId: undefined, endedAfter: undefined, endedBefore: undefined,
    });
  });

  it("shows loading, and an error with Retry", () => {
    setHistoryStoreState({ rows: [], status: "loading" });
    render(() => <JobHistoryView />);
    expect(screen.getByRole("status")).toHaveTextContent("Loading history");
    setHistoryStoreState({ status: "error", error: commandError("INTERNAL", "The history couldn't be read.") });
    expect(screen.getByRole("alert")).toHaveTextContent("The history couldn't be read.");
    historyStoreMock.refreshHistory.mockClear();
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    expect(historyStoreMock.refreshHistory).toHaveBeenCalledOnce();
  });

  it("pairs each state with text, not colour alone", () => {
    loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    const states = bodyRows().map((row) => row.textContent ?? "");
    expect(states.some((text) => /Completed|Failed|Cancelled/.test(text))).toBe(true);
  });

  it("publishes the listed Job and Incident ids so a deep link to them resolves", () => {
    const rows = loadWebHistoryFixture();
    render(() => <JobHistoryView />);
    const ids = historyKnownIds();
    for (const row of rows) expect(ids).toContain(row.jobId);
    expect(ids).toContain("inc-w-host-failed");
  });
});
