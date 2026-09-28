import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import {
  attentionStoreMock,
  resetAttentionStoreMock,
} from "../attention/attention-store-mock";
import { attentionEvent } from "../attention/test-records";
import { AttentionEventDetail } from "./AttentionEventDetail";

const printersState = vi.hoisted(() => [] as { id: string }[]);
vi.mock("../printers/printer-store", () => ({ printers: () => printersState }));
vi.mock("../spools/spool-store", () => ({ spoolState: { spools: [] } }));
vi.mock("../queue/queue-store", () => ({ queue: { entries: () => [], history: () => [] } }));
vi.mock("../attention/attention-store", async () => (await import("../attention/attention-store-mock")).attentionStoreMock);

afterEach(() => {
  cleanup();
  resetAttentionStoreMock();
  printersState.length = 0;
  window.location.hash = "";
});

describe("AttentionEventDetail", () => {
  it("shows the severity marker (shape/label/color), source, timestamps, and observation count", () => {
    printersState.push({ id: "prn-1" });
    const event = attentionEvent({
      id: "atn-1", severity: "fatal", firstObservedAt: "2026-09-25T00:00:00Z",
      lastObservedAt: "2026-09-26T00:00:00Z", observationCount: 3,
    });
    render(() => <AttentionEventDetail event={event} mode="inline" onClose={vi.fn()} />);

    expect(screen.getByRole("status", { name: "Fatal" })).toBeInTheDocument();
    expect(screen.getByText("Printer")).toBeInTheDocument();
    expect(screen.getByText("3")).toBeInTheDocument();
    expect(screen.getAllByText(/2026/).length).toBeGreaterThan(0);
  });

  it("shows the recurrence link and navigates to the earlier occurrence", async () => {
    const event = attentionEvent({ id: "atn-2", recurrenceOf: "atn-1" });
    render(() => <AttentionEventDetail event={event} mode="inline" onClose={vi.fn()} />);

    await fireEvent.click(screen.getByRole("button", { name: "View earlier occurrence" }));
    expect(window.location.hash).toBe("#nav=v1/monitor/attention/atn-1");
  });

  it("shows the Incident link and navigates to it", async () => {
    const event = attentionEvent({ id: "atn-3", incidentId: "inc-1" });
    render(() => <AttentionEventDetail event={event} mode="inline" onClose={vi.fn()} />);

    await fireEvent.click(screen.getByRole("button", { name: "View Incident" }));
    expect(window.location.hash).toBe("#nav=v1/monitor/incident/inc-1");
  });

  it("shows Acknowledge only when allowed, and Resolve only for manual Conditions", () => {
    const notYetAllowed = attentionEvent({ id: "atn-4", allowedActions: ["markRead"] });
    render(() => <AttentionEventDetail event={notYetAllowed} mode="inline" onClose={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Acknowledge" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Resolve" })).not.toBeInTheDocument();

    cleanup();
    const both = attentionEvent({ id: "atn-5", allowedActions: ["acknowledge", "resolve"], resolutionMode: "manual" });
    render(() => <AttentionEventDetail event={both} mode="inline" onClose={vi.fn()} />);
    expect(screen.getByRole("button", { name: "Acknowledge" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Resolve" })).toBeInTheDocument();
  });

  it("acknowledging calls the store action; the Event stays actionable (a store concern, not re-derived here)", async () => {
    const event = attentionEvent({ id: "atn-6", allowedActions: ["acknowledge"], requiresAction: true, resolvedAt: null });
    render(() => <AttentionEventDetail event={event} mode="inline" onClose={vi.fn()} />);

    await fireEvent.click(screen.getByRole("button", { name: "Acknowledge" }));
    expect(attentionStoreMock.acknowledgeAttentionEvent).toHaveBeenCalledWith("atn-6");
  });

  it("marks the Event read on open, only when it's unread", () => {
    const unread = attentionEvent({ id: "atn-7", readAt: null });
    render(() => <AttentionEventDetail event={unread} mode="inline" onClose={vi.fn()} />);
    expect(attentionStoreMock.markAttentionRead).toHaveBeenCalledWith(["atn-7"]);

    cleanup();
    attentionStoreMock.markAttentionRead.mockClear();
    const alreadyRead = attentionEvent({ id: "atn-8", readAt: "2026-09-25T00:00:00Z" });
    render(() => <AttentionEventDetail event={alreadyRead} mode="inline" onClose={vi.fn()} />);
    expect(attentionStoreMock.markAttentionRead).not.toHaveBeenCalled();
  });

  it("'Open source' navigates through deep-link.ts to the Printer when it still exists", async () => {
    printersState.push({ id: "prn-1" });
    const event = attentionEvent({ id: "atn-9", source: { kind: "printer", id: "prn-1" }, printerId: "prn-1" });
    render(() => <AttentionEventDetail event={event} mode="inline" onClose={vi.fn()} />);

    await fireEvent.click(screen.getByRole("button", { name: "Open source" }));
    expect(window.location.hash).toBe("#nav=v1/monitor/printer/prn-1");
  });

  it("a deleted source shows the snapshot identity with 'Printer deleted', and 'Open source' falls back to the Event itself", async () => {
    // No Printer in `printersState`: the source no longer exists.
    const event = attentionEvent({
      id: "atn-10", source: { kind: "printer", id: "prn-gone" }, printerId: "prn-gone",
      subject: { printerName: "Bay 9", printerLocation: null, jobLabel: null, spoolNumber: null, spoolLabel: null },
    });
    render(() => <AttentionEventDetail event={event} mode="inline" onClose={vi.fn()} />);

    expect(screen.getByText("Bay 9")).toBeInTheDocument();
    expect(screen.getByText("Printer deleted")).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Open source" }));
    expect(window.location.hash).toBe("#nav=v1/monitor/attention/atn-10");
  });
});
