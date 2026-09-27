import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  loadWebQueueFixture,
  queueStoreMock,
  resetQueueStoreMock,
  setQueueStoreState,
} from "../queue/queue-store-mock";
import { eligibilitySummary, job, jobHistory, queueEntry, queueEntryEligibility } from "../queue/test-records";
import {
  WEB_QUEUE_ENTRY_BRACKET_IDS,
  WEB_QUEUE_ENTRY_HISTORY_DEFERRED,
  WEB_QUEUE_JOB_DEFERRED,
} from "../queue/web-fixtures";
import { loadWebSlicingFixture, resetSlicingStoreMock } from "../slicing/slicing-store-mock";
import { QueueEntryDetail } from "./QueueEntryDetail";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
vi.mock("../slicing/slicing-store", async () => (await import("../slicing/slicing-store-mock")).slicingStoreMock);

beforeEach(() => {
  resetQueueStoreMock();
  resetSlicingStoreMock();
  loadWebSlicingFixture();
  window.location.hash = "";
});
afterEach(cleanup);

function renderDetail(entryId: string, extra: Partial<Parameters<typeof QueueEntryDetail>[0]> = {}) {
  const entry = queueStoreMock.queue.entry(entryId)!;
  return render(() => <QueueEntryDetail entry={entry} mode="inline" onClose={vi.fn()} {...extra} />);
}

describe("QueueEntryDetail", () => {
  it("lists the ranked candidates from explain_queue_entry with their reasons", async () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-1" })], eligibility: [eligibilitySummary()] });
    queueStoreMock.explainQueueEntry.mockResolvedValueOnce(queueEntryEligibility({
      candidates: [
        {
          printerId: "prn-2", printerName: "Voron", rank: 1,
          spool: { spoolId: "spl-4", spoolNumber: 4, loadedOnPrinter: true, availableMg: 500_000 },
          spoolOptions: [], loadedMatch: true, lastUsedAt: null, manualFactsAcknowledgementRequired: false,
        },
        {
          printerId: "prn-1", printerName: "Prusa", rank: 2,
          spool: { spoolId: "spl-2", spoolNumber: 2, loadedOnPrinter: false, availableMg: 900_000 },
          spoolOptions: [], loadedMatch: false, lastUsedAt: null, manualFactsAcknowledgementRequired: true,
        },
      ],
      printers: [{
        printerId: "prn-3", printerName: "Ender", eligible: false,
        blockers: [{ code: "PRINTER_OFFLINE", message: "This Printer is offline.", detail: null, recovery: "CHECK_CONNECTION", printerIds: ["prn-3"] }],
      }],
    }));
    renderDetail("qen-1");
    const list = await screen.findByRole("list", { name: "Ranked Printers" });
    const items = within(list).getAllByRole("listitem");
    expect(items[0]).toHaveTextContent("1Voron");
    expect(items[0]).toHaveTextContent("Spool #4 is loaded");
    expect(items[1]).toHaveTextContent("2Prusa");
    expect(items[1]).toHaveTextContent("Needs a manual-facts acknowledgement");
    expect(queueStoreMock.explainQueueEntry).toHaveBeenCalledWith("qen-1");
    // Printers that can't take it, with why.
    expect(screen.getByText("Ender")).toBeInTheDocument();
    expect(screen.getByText("This Printer is offline.")).toBeInTheDocument();
  });

  it("shows the entry's blockers with their recovery actions", async () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-1" })], eligibility: [eligibilitySummary({ verdict: "blocked" })] });
    queueStoreMock.explainQueueEntry.mockResolvedValueOnce(queueEntryEligibility({
      verdict: "blocked",
      blockers: [{ code: "SETUP_INCOMPLETE", message: "This Printer's setup is incomplete.", detail: null, recovery: "OPEN_PRINTER_SETUP", printerIds: ["prn-9"] }],
    }));
    renderDetail("qen-1");
    expect(await screen.findByText("This Printer's setup is incomplete.")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Open Printer setup" }));
    expect(window.location.hash).toBe("#nav=v1/monitor/printer/prn-9");
  });

  it("edits the policy through update_queue_entry with the entry's revision", async () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-1", revision: 4, policy: "manual" })], eligibility: [eligibilitySummary()] });
    renderDetail("qen-1");
    const policy = screen.getByRole("radiogroup", { name: "Dispatch Policy" });
    fireEvent.click(within(policy).getByLabelText("Automatic"));
    await waitFor(() => expect(queueStoreMock.updateQueueEntry).toHaveBeenCalledWith("qen-1", 4, { policy: "automatic" }));
  });

  it("asks before removing: Cancel sends nothing, Remove sends the command", async () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-1", revision: 2 })], eligibility: [eligibilitySummary()] });
    renderDetail("qen-1");
    fireEvent.click(screen.getByRole("button", { name: "Remove…" }));
    let confirm = await screen.findByRole("alertdialog", { name: "Remove this Queue Entry?" });
    fireEvent.click(within(confirm).getByRole("button", { name: "Keep it" }));
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(queueStoreMock.removeQueueEntry).not.toHaveBeenCalled();

    fireEvent.click(screen.getByRole("button", { name: "Remove…" }));
    confirm = await screen.findByRole("alertdialog", { name: "Remove this Queue Entry?" });
    fireEvent.click(within(confirm).getByRole("button", { name: "Remove" }));
    await waitFor(() => expect(queueStoreMock.removeQueueEntry).toHaveBeenCalledWith("qen-1", 2));
  });

  it("offers no Remove or Assign… when Rust doesn't allow them", () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-2", state: "assigned", allowedActions: ["move"], jobId: "job-1" })], jobs: [job({ queueEntryId: "qen-2" })] });
    renderDetail("qen-2");
    expect(screen.queryByRole("button", { name: "Remove…" })).toBeNull();
    expect(screen.queryByRole("button", { name: "Assign…" })).toBeNull();
  });

  it("shows an error inline when a write is refused", async () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-1" })], eligibility: [eligibilitySummary()] });
    queueStoreMock.removeQueueEntry.mockRejectedValueOnce({
      contractVersion: 1, code: "CONFLICT", message: "This Queue Entry changed. Reload.", recovery: ["RELOAD"], retryable: false,
    });
    renderDetail("qen-1");
    fireEvent.click(screen.getByRole("button", { name: "Remove…" }));
    const confirm = await screen.findByRole("alertdialog");
    fireEvent.click(within(confirm).getByRole("button", { name: "Remove" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This Queue Entry changed. Reload.");
  });

  it("offers Assign… from Rust's allowed actions and hands the entry to the caller", () => {
    setQueueStoreState({ entries: [queueEntry({ id: "qen-1" })], eligibility: [eligibilitySummary()] });
    const onAssign = vi.fn();
    renderDetail("qen-1", { onAssign });
    fireEvent.click(screen.getByRole("button", { name: "Assign…" }));
    expect(onAssign).toHaveBeenCalledWith("qen-1");
  });

  it("retries a finished Job and opens the new entry", async () => {
    loadWebQueueFixture();
    queueStoreMock.retryJob.mockResolvedValueOnce({ entries: [queueEntry({ id: "qen-retry" })], jobs: [], requirements: [] });
    renderDetail(WEB_QUEUE_ENTRY_HISTORY_DEFERRED);
    fireEvent.click(screen.getByRole("button", { name: "Retry" }));
    await waitFor(() => expect(queueStoreMock.retryJob).toHaveBeenCalledWith(WEB_QUEUE_JOB_DEFERRED));
    await waitFor(() => expect(window.location.hash).toBe("#nav=v1/queue/job/qen-retry"));
  });

  it("shows the Slice Revision's facts and estimate on the Artifact tab", async () => {
    loadWebQueueFixture();
    renderDetail(WEB_QUEUE_ENTRY_BRACKET_IDS[0]);
    fireEvent.click(screen.getByRole("tab", { name: "Artifact" }));
    expect(await screen.findByText("Material estimate")).toBeInTheDocument();
    expect(screen.getByText("Material estimate").nextElementSibling).toHaveTextContent("38.6 g (slice estimate)");
    expect(await screen.findByText("Nozzle diameter")).toBeInTheDocument();
  });

  it("shows the Job timeline and lineage links on the History tab", async () => {
    loadWebQueueFixture();
    const entry = queueStoreMock.queue.entry(WEB_QUEUE_ENTRY_HISTORY_DEFERRED)!;
    queueStoreMock.getJobHistory.mockResolvedValueOnce(jobHistory({
      job: job({ id: WEB_QUEUE_JOB_DEFERRED }),
      entry,
      lineage: [entry, queueEntry({ id: "qen-sibling", lineageId: entry.lineageId, copyIndex: 2, copyCount: 2 })],
      events: [{
        id: "jev-1", jobId: WEB_QUEUE_JOB_DEFERRED, sequence: 1, kind: "failed", fromState: "printing", toState: "failed",
        operationId: null, hostOperationId: null, detail: null, at: "2026-09-24T09:09:00Z",
      }],
    }));
    renderDetail(WEB_QUEUE_ENTRY_HISTORY_DEFERRED);
    fireEvent.click(screen.getByRole("tab", { name: "History" }));
    const timeline = await screen.findByRole("list", { name: /^Job timeline/ });
    expect(within(timeline).getByText("Failed")).toBeInTheDocument();
    fireEvent.click(screen.getByRole("button", { name: "Copy 2 of 2" }));
    expect(window.location.hash).toBe("#nav=v1/queue/job/qen-sibling");
  });
});
