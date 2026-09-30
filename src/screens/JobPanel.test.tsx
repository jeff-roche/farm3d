import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetCapabilitiesStoreMock, setPrinterCapabilitiesForTest } from "../host-ops/capabilities-store-mock";
import {
  hostOperationsStoreMock,
  resetHostOperationsStoreMock,
  setHostOperationsStoreState,
} from "../host-ops/host-operations-store-mock";
import { historyStoreMock, resetHistoryStoreMock } from "../history/history-store-mock";
import { hostOperation, printerCapabilities, printerStatus, resolvedPrinter } from "../host-ops/test-records";
import { queueStoreMock, resetQueueStoreMock, setQueueStoreState } from "../queue/queue-store-mock";
import { job, jobHistory, queueEntry } from "../queue/test-records";
import type { Job, JobAction, JobState } from "../queue/types";
import type { ResolvedPrinter } from "../printers/types";
import { pressEscape, tabOrder } from "./dialog-test-helpers";
import { JobPanel } from "./JobPanel";

vi.mock("../history/history-store", async () => (await import("../history/history-store-mock")).historyStoreMock);
vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);
vi.mock("../host-ops/capabilities-store", async () =>
  (await import("../host-ops/capabilities-store-mock")).capabilitiesStoreMock);
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools: [], tares: [] };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));
vi.mock("../printers/printer-store", () => ({ printers: () => [] }));

beforeEach(() => {
  resetQueueStoreMock();
  resetHistoryStoreMock();
  resetHostOperationsStoreMock();
  resetCapabilitiesStoreMock();
  setPrinterCapabilitiesForTest(printerCapabilities());
  window.location.hash = "";
});
afterEach(cleanup);

function renderPanel(record: Job, printer: ResolvedPrinter = resolvedPrinter()) {
  setQueueStoreState({ entries: [queueEntry({ id: record.queueEntryId, state: "assigned", jobId: record.id })], jobs: [record] });
  return render(() => <JobPanel job={queueStoreMock.queue.job(record.id)!} printer={printer} />);
}

/** D3's `allowedActions` per state, exactly as Rust sends them. */
const ALLOWED: Record<JobState, JobAction[]> = {
  assigned: ["stage", "cancel", "release"],
  staging: [],
  awaitingStart: ["stage", "start", "cancel", "release"],
  starting: [],
  printing: ["pause", "cancel"],
  paused: ["resume", "cancel"],
  outcomeUnknown: ["declareOutcome"],
  completed: ["retry", "correctMaterial"],
  failed: ["retry", "settleMaterial"],
  cancelled: ["retry", "settleMaterial"],
};

describe("JobPanel: actions come from Rust's allowedActions", () => {
  it("offers Release only in assigned and awaitingStart", () => {
    for (const state of Object.keys(ALLOWED) as JobState[]) {
      const settlement = state === "failed" || state === "cancelled" ? "pending" : state === "completed" ? "settled" : "open";
      const { unmount } = renderPanel(job({ state, allowedActions: ALLOWED[state], settlement }));
      const offered = screen.queryByRole("button", { name: "Release" }) !== null;
      expect(offered, state).toBe(state === "assigned" || state === "awaitingStart");
      unmount();
    }
  });

  it("renders each allowed action and nothing else", () => {
    renderPanel(job({ state: "printing", allowedActions: ["pause", "cancel"] }));
    expect(screen.getByRole("button", { name: "Pause" })).toBeEnabled();
    expect(screen.getByRole("button", { name: "Cancel…" })).toBeEnabled();
    for (const name of ["Resume", "Start…", "Release", "Retry", "Settle material…", "Correct weight…", "Declare outcome…"]) {
      expect(screen.queryByRole("button", { name })).toBeNull();
    }
  });

  it("sends Pause through the Job", async () => {
    renderPanel(job({ state: "printing", allowedActions: ["pause", "cancel"] }));
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    await waitFor(() => expect(queueStoreMock.pauseJob).toHaveBeenCalledWith("job-1"));
  });

  it("releases an awaitingStart Job", async () => {
    renderPanel(job({ state: "awaitingStart", allowedActions: ALLOWED.awaitingStart }));
    fireEvent.click(screen.getByRole("button", { name: "Release" }));
    await waitFor(() => expect(queueStoreMock.releaseJob).toHaveBeenCalledWith("job-1"));
  });

  it("asks before cancelling, and Escape sends nothing", async () => {
    renderPanel(job({ state: "printing", allowedActions: ["pause", "cancel"], hostPath: "farm3d/lid.gcode" }));
    const trigger = screen.getByRole("button", { name: "Cancel…" });
    fireEvent.click(trigger);
    const dialog = await screen.findByRole("alertdialog", { name: "Cancel this Job?" });
    expect(tabOrder(dialog)).toEqual(["Keep the Job", "Cancel Job"]);
    pressEscape(dialog);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    expect(queueStoreMock.cancelJob).not.toHaveBeenCalled();
    fireEvent.click(trigger);
    fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel Job" }));
    await waitFor(() => expect(queueStoreMock.cancelJob).toHaveBeenCalledWith("job-1"));
  });

  it("shows a refused command with role=alert", async () => {
    queueStoreMock.pauseJob.mockRejectedValueOnce({
      contractVersion: 1, code: "JOB_NOT_ON_PRINTER", message: "The printer is printing a different file than this Job's.",
      recovery: ["RELOAD"], retryable: false,
    });
    renderPanel(job({ state: "printing", allowedActions: ["pause", "cancel"] }));
    fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("The printer is printing a different file than this Job's.");
  });
});

describe("JobPanel: Start", () => {
  it("is disabled with the blocker's reason while startBlockers is non-empty", () => {
    renderPanel(job({
      state: "awaitingStart",
      allowedActions: ALLOWED.awaitingStart,
      startBlockers: [{
        code: "SPOOL_NOT_LOADED", message: "Awaiting material: load Spool #12 on Bay 1.", detail: null,
        recovery: "LOAD_SPOOL", printerIds: ["prn-1"],
      }],
    }));
    expect(screen.getByRole("button", { name: "Start…" })).toBeDisabled();
    const reasons = screen.getByRole("list", { name: "Why it can't start" });
    // Rust's message is the whole sentence: shown once, not after a
    // separate "Awaiting material" label.
    expect(within(reasons).getAllByText(/Awaiting material/)).toHaveLength(1);
    expect(within(reasons).getByText("Awaiting material: load Spool #12 on Bay 1.")).toBeInTheDocument();
  });

  it("stays disabled until the P6 bed-clear checkbox is ticked, then starts with the confirmed prior state", async () => {
    renderPanel(
      job({ state: "awaitingStart", allowedActions: ALLOWED.awaitingStart, hostPath: "farm3d/lid.gcode" }),
      resolvedPrinter({ runtimeStatus: printerStatus("finished") }),
    );
    fireEvent.click(screen.getByRole("button", { name: "Start…" }));
    const dialog = await screen.findByRole("alertdialog", { name: "Start this Job?" });
    const confirm = within(dialog).getByRole("button", { name: "Start print" });
    expect(confirm).toBeDisabled();
    expect(within(dialog).getByText("Tick the confirmation to start.")).toBeInTheDocument();
    // P6's copy for the prior state the printer reports.
    const checkbox = within(dialog).getByLabelText("The previous print finished. The bed is clear.");
    expect(tabOrder(dialog)).toEqual(["The previous print finished. The bed is clear.", "Cancel"]);
    fireEvent.click(checkbox);
    await waitFor(() => expect(confirm).toBeEnabled());
    expect(tabOrder(dialog)).toEqual(["The previous print finished. The bed is clear.", "Cancel", "Start print"]);
    fireEvent.click(confirm);
    await waitFor(() => expect(queueStoreMock.startJob).toHaveBeenCalledWith("job-1", "finished"));
  });

  it("closes on Escape and returns focus to Start…", async () => {
    renderPanel(job({ state: "awaitingStart", allowedActions: ALLOWED.awaitingStart }));
    const trigger = screen.getByRole("button", { name: "Start…" });
    fireEvent.click(trigger);
    const dialog = await screen.findByRole("alertdialog", { name: "Start this Job?" });
    pressEscape(dialog);
    await waitFor(() => expect(screen.queryByRole("alertdialog")).toBeNull());
    await waitFor(() => expect(document.activeElement).toBe(trigger));
    expect(queueStoreMock.startJob).not.toHaveBeenCalled();
  });
});

describe("JobPanel: the linked Host Operation", () => {
  it("offers P6's Check again and Abandon check… for an uncertain linked operation", async () => {
    const op = hostOperation({ id: "hop-9", kind: "upload", state: "uncertain", attempts: 1, jobId: "job-1", hostPath: "farm3d/lid.gcode" });
    setHostOperationsStoreState([op]);
    renderPanel(job({ state: "staging", allowedActions: [], activeHostOperationId: "hop-9" }));
    fireEvent.click(screen.getByRole("button", { name: "Check again" }));
    await waitFor(() => expect(hostOperationsStoreMock.reconcileHostOperation).toHaveBeenCalledWith("hop-9"));
    fireEvent.click(screen.getByRole("button", { name: "Abandon check…" }));
    expect(await screen.findByRole("alertdialog", { name: "Stop checking this operation?" })).toBeInTheDocument();
  });
});

describe("JobPanel: state, facts, and history", () => {
  it("shows the state, progress, estimate, and the Timeline from get_job_history", async () => {
    queueStoreMock.getJobHistory.mockResolvedValueOnce(jobHistory({
      events: [{
        id: "jev-2", jobId: "job-1", sequence: 2, kind: "startSucceeded", fromState: "starting", toState: "printing",
        operationId: null, hostOperationId: null, detail: null, at: "2026-09-25T01:00:00Z",
      }],
    }));
    renderPanel(job({ state: "printing", allowedActions: ["pause", "cancel"], maxProgressPct: 42, estimateMg: 38_600 }));
    expect(screen.getAllByText("Printing").length).toBeGreaterThan(0);
    expect(screen.getByText("42%")).toBeInTheDocument();
    expect(screen.getByText("38.6 g")).toBeInTheDocument();
    const timeline = await screen.findByRole("list", { name: /^Job timeline/ });
    expect(within(timeline).getByText("Started")).toBeInTheDocument();
  });

  it("shows a settled Job's full timeline from get_job_timeline, not get_job_history", async () => {
    renderPanel(job({ id: "job-web-deferred", state: "failed", allowedActions: [] }));
    const timeline = await screen.findByRole("list", { name: /^Job timeline/ });
    expect(within(timeline).getAllByRole("listitem").length).toBeGreaterThan(0);
    expect(historyStoreMock.getJobTimeline).toHaveBeenCalledWith("job-web-deferred");
    expect(queueStoreMock.getJobHistory).not.toHaveBeenCalled();
  });

  it("keeps get_job_history for a Job that hasn't settled", async () => {
    renderPanel(job({ state: "printing", allowedActions: ["pause", "cancel"] }));
    await screen.findByRole("list", { name: /^Job timeline/ });
    expect(queueStoreMock.getJobHistory).toHaveBeenCalled();
    expect(historyStoreMock.getJobTimeline).not.toHaveBeenCalled();
  });

  it("explains that Declare outcome opens 30 minutes after the host became unreachable", () => {
    renderPanel(job({ state: "printing", allowedActions: ["pause", "cancel"], hostUnreachableSince: "2026-09-25T01:00:00Z" }));
    expect(screen.getByText(/Declare outcome becomes available 30 minutes after that/)).toBeInTheDocument();
  });
});
