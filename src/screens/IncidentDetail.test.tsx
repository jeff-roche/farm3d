import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { cameraSnapshot, incident, incidentDetail, incidentEntry } from "../attention/test-records";
import type { IncidentEntryDetail, IncidentEntryKind, IncidentTimelineItem } from "../attention/types";
import type { JobEvent } from "../generated/contracts/domain/JobEvent";
import { IncidentDetail } from "./IncidentDetail";

const getIncidentMock = vi.hoisted(() => vi.fn());
const addIncidentNoteMock = vi.hoisted(() => vi.fn());
const watchHandlers = vi.hoisted(() => new Set<() => void>());
vi.mock("../incidents/incident-store", () => ({
  getIncident: getIncidentMock,
  addIncidentNote: addIncidentNoteMock,
  watchIncidentDetail: vi.fn((_id: string, _onChange: unknown) => {
    const stop = vi.fn();
    watchHandlers.add(stop);
    return stop;
  }),
}));

const snapshotImageUrlMock = vi.hoisted(() => vi.fn(async (id: string) => `blob:${id}`));
const setSnapshotPinnedMock = vi.hoisted(() => vi.fn());
vi.mock("../cameras/camera-store", () => ({
  snapshotImageUrl: snapshotImageUrlMock,
  setSnapshotPinned: setSnapshotPinnedMock,
}));

afterEach(() => {
  cleanup();
  getIncidentMock.mockReset();
  addIncidentNoteMock.mockReset();
  snapshotImageUrlMock.mockClear();
  setSnapshotPinnedMock.mockReset();
  watchHandlers.clear();
  window.location.hash = "";
});

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

const KIND_DETAIL: Record<IncidentEntryKind, IncidentEntryDetail> = {
  opened: { kind: "opened", eventId: "atn-1" },
  eventLinked: { kind: "eventLinked", eventId: "atn-2" },
  reopened: { kind: "reopened", eventId: "atn-1" },
  eventAcknowledged: { kind: "eventAcknowledged", eventId: "atn-1", by: "operator" },
  eventResolved: { kind: "eventResolved", eventId: "atn-1", resolution: "operatorResolved" },
  evidenceCaptured: { kind: "evidenceCaptured", snapshotId: "snp-1", trigger: "manual" },
  evidenceSkipped: { kind: "evidenceSkipped", reason: "cameraError", errorKind: "timeout" },
  evidencePruned: { kind: "evidencePruned", snapshotId: "snp-1", reason: "age" },
  evidencePinned: { kind: "evidencePinned", snapshotId: "snp-1" },
  evidenceUnpinned: { kind: "evidenceUnpinned", snapshotId: "snp-1" },
  noteAdded: { kind: "noteAdded", text: "Checked the bed." },
  closed: { kind: "closed" },
};

function fullTimeline(): IncidentTimelineItem[] {
  const entries: IncidentTimelineItem[] = (Object.keys(KIND_DETAIL) as IncidentEntryKind[]).map((kind, index) => ({
    source: "incident",
    entry: incidentEntry({
      id: `iev-${index}`,
      sequence: index + 1,
      kind,
      detail: KIND_DETAIL[kind],
      at: `2026-09-25T00:0${index}:00Z`,
    }),
  }));
  const jobEvent: JobEvent = {
    id: "jev-1", jobId: "job-1", sequence: 1, kind: "assigned", fromState: null, toState: "assigned",
    operationId: null, hostOperationId: null, detail: null, at: "2026-09-25T00:20:00Z",
  };
  return [...entries, { source: "job", event: jobEvent }];
}

describe("IncidentDetail", () => {
  it("shows a loading state, then the header (kind, state, Printer identity, times)", async () => {
    getIncidentMock.mockResolvedValue(incidentDetail({
      incident: incident({ id: "inc-1", kind: "job.failed", state: "closed", closedAt: "2026-09-26T00:00:00Z" }),
    }));
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);

    expect(screen.getByText("Loading the Incident…")).toBeInTheDocument();
    expect(await screen.findByRole("heading", { name: "Job failed" })).toBeInTheDocument();
    expect(screen.getByText("Closed", { selector: "p" })).toBeInTheDocument();
    expect(screen.getByText("Bay 1")).toBeInTheDocument();
    expect(getIncidentMock).toHaveBeenCalledWith("inc-1");
  });

  it("shows an error when the Incident fails to load", async () => {
    getIncidentMock.mockRejectedValue({ contractVersion: 1, code: "NOT_FOUND", message: "Incident inc-1 was not found.", recovery: [], retryable: false });
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);

    expect(await screen.findByRole("alert")).toHaveTextContent("Incident inc-1 was not found.");
  });

  it("renders every timeline kind with text, merged with the Job's own events", async () => {
    getIncidentMock.mockResolvedValue(incidentDetail({ incident: incident({ id: "inc-1" }), timeline: fullTimeline() }));
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);
    await screen.findByRole("heading", { name: "Printer-reported failure" });

    for (const label of [
      "Opened", "Event linked", "Reopened", "Event acknowledged", "Event resolved",
      "Evidence captured", "Evidence skipped", "Evidence pruned", "Evidence pinned",
      "Evidence unpinned", "Note added", "Closed",
    ]) {
      expect(screen.getAllByText(label).length).toBeGreaterThan(0);
    }
    // The merged Job event, rendered with the Job's own label (`jobEventKindLabel`).
    expect(screen.getByText("Assigned")).toBeInTheDocument();
  });

  it("evidence thumbnails have alt text and a timestamp; a pruned item shows text and no image", async () => {
    const live = cameraSnapshot({ id: "snp-live", trigger: "manual", capturedAt: "2026-09-25T12:00:00Z" });
    const pruned = cameraSnapshot({ id: "snp-pruned", prunedAt: "2026-09-26T00:00:00Z", pruneReason: "age" });
    getIncidentMock.mockResolvedValue(incidentDetail({
      incident: incident({ id: "inc-1", printerSnapshot: { ...incident().printerSnapshot, name: "Bay 8" } }),
      snapshots: [live, pruned],
    }));
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);
    await screen.findByRole("heading", { name: "Printer-reported failure" });

    const image = await screen.findByAltText("Manual snapshot of Bay 8 at " + new Date("2026-09-25T12:00:00Z").toLocaleString(undefined, { dateStyle: "medium", timeStyle: "short" }));
    expect(image).toBeInTheDocument();
    expect(screen.getByText("Evidence pruned (past retention)")).toBeInTheDocument();
    // No <img> at all for the pruned row: only the live one has one.
    expect(screen.getAllByRole("img")).toHaveLength(1);
  });

  it("clicking a live evidence thumbnail opens the snapshot viewer", async () => {
    const live = cameraSnapshot({ id: "snp-live", trigger: "incident" });
    getIncidentMock.mockResolvedValue(incidentDetail({ incident: incident({ id: "inc-1" }), snapshots: [live] }));
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);
    await screen.findByRole("heading", { name: "Printer-reported failure" });

    await fireEvent.click(await screen.findByRole("button", { name: /Incident/ }));
    expect(await screen.findByRole("dialog")).toBeInTheDocument();
  });

  it("lists linked Events with their severity marker, and navigates to the Event's own detail", async () => {
    getIncidentMock.mockResolvedValue(incidentDetail({
      incident: incident({ id: "inc-1" }),
      events: [{
        id: "atn-1", revision: 1, dedupKey: "printer.hostFailed:printer:prn-1", condition: "printer.hostFailed",
        severity: "fatal", requiresAction: true, resolutionMode: "manual", notificationClass: "fatal",
        source: { kind: "printer", id: "prn-1" }, printerId: "prn-1", jobId: null, spoolId: null,
        requirementId: null, incidentId: "inc-1", subject: { printerName: "Bay 1", printerLocation: null, jobLabel: null, spoolNumber: null, spoolLabel: null },
        detail: { kind: "printerHostFailed" }, summary: "Bay 1 reported a failure.", origin: "live",
        firstObservedAt: "2026-09-25T00:00:00Z", lastObservedAt: "2026-09-25T00:00:00Z", observationCount: 1,
        recurrenceOf: null, readAt: null, acknowledgedAt: null, resolvedAt: null, resolution: null,
        notifiedAt: null, evidence: null, allowedActions: ["markRead", "acknowledge"],
      }],
    }));
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);
    await screen.findByRole("heading", { name: "Printer-reported failure" });

    expect(screen.getByRole("status", { name: "Fatal" })).toBeInTheDocument();
    await fireEvent.click(screen.getByText("Bay 1 reported a failure."));
    expect(window.location.hash).toBe("#nav=v1/monitor/attention/atn-1");
  });

  it("adding a note calls add_incident_note and clears the field", async () => {
    getIncidentMock.mockResolvedValue(incidentDetail({ incident: incident({ id: "inc-1" }) }));
    addIncidentNoteMock.mockResolvedValue(incidentDetail({ incident: incident({ id: "inc-1" }) }));
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);
    await screen.findByRole("heading", { name: "Printer-reported failure" });

    const field = screen.getByRole("textbox", { name: "Add a note" });
    await fireEvent.input(field, { target: { value: "Checked the bed." } });
    await fireEvent.click(screen.getByRole("button", { name: "Add note" }));
    await waitFor(() => expect(addIncidentNoteMock).toHaveBeenCalledWith("inc-1", "Checked the bed."));
    await flush();
    expect((field as HTMLTextAreaElement).value).toBe("");
  });

  it("calls onClose from the Close button", async () => {
    getIncidentMock.mockResolvedValue(incidentDetail({ incident: incident({ id: "inc-1" }) }));
    const onClose = vi.fn();
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={onClose} />);
    await fireEvent.click(await screen.findByRole("button", { name: "Close" }));
    expect(onClose).toHaveBeenCalled();
  });

  it("refetches on a live watchIncidentDetail change", async () => {
    getIncidentMock.mockResolvedValueOnce(incidentDetail({ incident: incident({ id: "inc-1", state: "open" }) }));
    render(() => <IncidentDetail incidentId="inc-1" mode="inline" onClose={vi.fn()} />);
    await screen.findByText("Open");

    const { watchIncidentDetail } = await import("../incidents/incident-store");
    const calls = (watchIncidentDetail as unknown as ReturnType<typeof vi.fn>).mock.calls;
    const onChange = calls[calls.length - 1][1] as (d: ReturnType<typeof incidentDetail>) => void;
    onChange(incidentDetail({ incident: incident({ id: "inc-1", state: "closed", closedAt: "2026-09-26T00:00:00Z" }) }));
    await flush();
    expect(await screen.findByText("Closed", { selector: "p" })).toBeInTheDocument();
  });

  it("uses a modal dialog in overlay mode", async () => {
    getIncidentMock.mockResolvedValue(incidentDetail({ incident: incident({ id: "inc-1" }) }));
    render(() => <IncidentDetail incidentId="inc-1" mode="overlay" onClose={vi.fn()} />);
    expect(await screen.findByRole("dialog", { name: "Printer-reported failure" })).toBeInTheDocument();
  });
});
