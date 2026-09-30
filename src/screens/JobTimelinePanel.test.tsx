import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { historyStoreMock, resetHistoryStoreMock } from "../history/history-store-mock";
import { webJobTimeline } from "../history/web-fixtures";
import { WEB_QUEUE_JOB_COMPLETED, WEB_QUEUE_JOB_DEFERRED } from "../queue/web-fixtures";
import { historyKnownIds } from "../history/known-ids";
import { clearHistoryKnownIds } from "../history/known-ids-actions";
import { JobTimelinePanel } from "./JobTimelinePanel";

vi.mock("../history/history-store", async () => (await import("../history/history-store-mock")).historyStoreMock);
vi.mock("../cameras/camera-store", () => ({
  snapshotImageUrl: vi.fn(async () => "data:image/png;base64,AAAA"),
  setSnapshotPinned: vi.fn(),
}));

beforeEach(() => {
  window.location.hash = "";
  resetHistoryStoreMock();
  clearHistoryKnownIds();
});
afterEach(cleanup);

function renderPanel(jobId: string) {
  return render(() => <JobTimelinePanel jobId={jobId} label="Bay 4" />);
}

describe("JobTimelinePanel", () => {
  it("renders every timeline item with a label, a time, and a detail", async () => {
    renderPanel(WEB_QUEUE_JOB_COMPLETED);
    const list = await screen.findByRole("list", { name: "Job timeline for Bay 4" });
    const items = within(list).getAllByRole("listitem");
    expect(items).toHaveLength(webJobTimeline(WEB_QUEUE_JOB_COMPLETED)!.items.length);
    for (const item of items) {
      expect(item.querySelector("time")).not.toBeNull();
      expect(item.textContent?.trim().length).toBeGreaterThan(10);
    }
    for (const source of ["Job", "Material reservation", "Material", "Attention", "Incident", "Snapshot"]) {
      expect(within(list).getAllByText(source).length, source).toBeGreaterThan(0);
    }
  });

  it("shows the material section with the reservation, deduction, and correction", async () => {
    renderPanel(WEB_QUEUE_JOB_COMPLETED);
    const section = await screen.findByRole("region", { name: "Material" });
    expect(within(section).getByText(/^Reservation /)).toBeInTheDocument();
    expect(within(section).getByText("Consumption")).toBeInTheDocument();
    expect(within(section).getByText("Correction")).toBeInTheDocument();
    expect(within(section).getByText(/Spool 412\.0 g to 377\.4 g/)).toBeInTheDocument();
  });

  it("opens the Incident from the timeline", async () => {
    renderPanel(WEB_QUEUE_JOB_COMPLETED);
    fireEvent.click((await screen.findAllByRole("button", { name: "Open Incident" }))[0]);
    expect(window.location.hash).toBe("#nav=v1/monitor/incident/inc-w-host-failed");
  });

  it("registers the Job and its Incident as deep-link targets so the shell doesn't gate the link", async () => {
    renderPanel(WEB_QUEUE_JOB_COMPLETED);
    await screen.findByRole("list", { name: /^Job timeline/ });
    await waitFor(() => expect(historyKnownIds()).toEqual(expect.arrayContaining([WEB_QUEUE_JOB_COMPLETED, "inc-w-host-failed"])));
    expect(new Set(historyKnownIds()).size).toBe(historyKnownIds().length);
  });

  it("opens a snapshot in the viewer and shows pruned evidence as text", async () => {
    renderPanel(WEB_QUEUE_JOB_COMPLETED);
    const list = await screen.findByRole("list", { name: /^Job timeline/ });
    expect(within(list).getAllByText(/Evidence pruned \(Removed after the retention period\)/).length).toBeGreaterThan(0);
    expect(list.querySelectorAll("img")).toHaveLength(0);
    const views = within(list).getAllByRole("button", { name: "View snapshot" });
    expect(views).toHaveLength(1); // the pruned one has no viewer
    fireEvent.click(views[0]);
    expect(await screen.findByRole("dialog", { name: /^Snapshot/ })).toBeInTheDocument();
  });

  it("renders a Job without an Incident", async () => {
    renderPanel(WEB_QUEUE_JOB_DEFERRED);
    await screen.findByRole("list", { name: /^Job timeline/ });
    expect(screen.queryByRole("button", { name: "Open Incident" })).toBeNull();
  });

  it("says when the timeline can't load, and retries", async () => {
    historyStoreMock.getJobTimeline.mockRejectedValueOnce(new Error("boom"));
    renderPanel(WEB_QUEUE_JOB_COMPLETED);
    expect(await screen.findByRole("alert")).toHaveTextContent("The Job's timeline couldn't be loaded.");
    fireEvent.click(screen.getByRole("button", { name: "Try again" }));
    await waitFor(() => expect(screen.getByRole("list", { name: /^Job timeline/ })).toBeInTheDocument());
  });

  it("shows a loading status first", () => {
    renderPanel(WEB_QUEUE_JOB_COMPLETED);
    expect(screen.getByRole("status")).toHaveTextContent("Loading the Job's timeline");
  });
});
