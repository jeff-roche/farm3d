import { createSignal } from "solid-js";
import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetCapabilitiesStoreMock, setPrinterCapabilitiesForTest } from "../host-ops/capabilities-store-mock";
import {
  hostOperationsStoreMock,
  resetHostOperationsStoreMock,
  setHostOperationsStoreState,
} from "../host-ops/host-operations-store-mock";
import { hostOperation, printerCapabilities, printerStatus, resolvedPrinter } from "../host-ops/test-records";
import type { CapabilityKey, HostOperation } from "../host-ops/types";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterJobPanel } from "./PrinterJobPanel";

vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);
vi.mock("../host-ops/capabilities-store", async () =>
  (await import("../host-ops/capabilities-store-mock")).capabilitiesStoreMock);

const PENDING_CONTROL = "A printer operation is pending. You can still pause or cancel on the printer itself.";
const STAGED = hostOperation({
  id: "hop-staged", printerId: "prn-1", kind: "upload", state: "succeeded", sliceRevisionId: "slr-7",
  hostPath: "farm3d/slr-7.gcode", resolution: { kind: "artifactVerified", reconciled: false },
});

beforeEach(() => {
  resetHostOperationsStoreMock();
  resetCapabilitiesStoreMock();
  setPrinterCapabilitiesForTest(printerCapabilities());
});
afterEach(cleanup);

function unsupported(...keys: CapabilityKey[]) {
  const base = printerCapabilities();
  const capabilities = { ...base.capabilities };
  for (const key of keys) capabilities[key] = { status: "unsupported", reason: "notVerified", detail: "Not verified for this Connection type yet." };
  setPrinterCapabilitiesForTest({ ...base, capabilities });
}

function renderPanel(printer: ResolvedPrinter | (() => ResolvedPrinter) = resolvedPrinter()) {
  const current = typeof printer === "function" ? printer : () => printer;
  render(() => <PrinterJobPanel printer={current()} />);
}

const printing = (overrides = {}) => resolvedPrinter({
  runtimeStatus: printerStatus("printing", "fresh", {
    telemetry: { hostActivity: "printing", jobName: "farm3d/bracket.gcode", progress: 0.42, nozzleTempC: 215, nozzleTargetC: 215, bedTempC: 60, bedTargetC: 60 },
  }),
  ...overrides,
});

describe("PrinterJobPanel: the current print", () => {
  it("shows the file, state, progress, the nozzle and the bed", () => {
    renderPanel(printing());
    const current = screen.getByRole("region", { name: "Current print" });
    expect(current).toHaveTextContent("farm3d/bracket.gcode");
    expect(current).toHaveTextContent("Printing");
    expect(current).toHaveTextContent("42%");
    expect(within(current).getByText("Nozzle")).toBeInTheDocument();
    expect(current).toHaveTextContent("215");
    expect(within(current).getByText("Bed")).toBeInTheDocument();
  });

  it("a four-tool Printer shows four tools", () => {
    renderPanel(resolvedPrinter({
      runtimeStatus: printerStatus("printing", "fresh", {
        telemetry: {
          hostActivity: "printing",
          nozzleTempC: 220, nozzleTargetC: 220,
          tools: [0, 1, 2, 3].map((index) => ({ index, tempC: 200 + index, targetC: 210 })),
        },
      }),
    }));
    const current = screen.getByRole("region", { name: "Current print" });
    for (const tool of ["T0", "T1", "T2", "T3"]) expect(within(current).getByText(tool)).toBeInTheDocument();
    expect(within(current).queryByText("Nozzle")).not.toBeInTheDocument();
  });
});

describe("PrinterJobPanel: Pause, Resume, Cancel", () => {
  it("Pause is disabled while the Printer is idle, with the reason as visible text", () => {
    renderPanel(resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    expect(screen.getByRole("button", { name: "Pause" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Resume" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel print…" })).toBeDisabled();
    expect(screen.getByText("The printer isn't in a state to pause now: it is ready.")).toBeInTheDocument();
  });

  it("offers Pause and Cancel while printing; Pause sends pause_host_print for this Printer", async () => {
    renderPanel(printing());
    expect(screen.getByRole("button", { name: "Resume" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel print…" })).toBeEnabled();
    await fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(hostOperationsStoreMock.pauseHostPrint).toHaveBeenCalledWith("prn-1");
  });

  it("offers Resume while paused", async () => {
    renderPanel(resolvedPrinter({ runtimeStatus: printerStatus("paused") }));
    expect(screen.getByRole("button", { name: "Pause" })).toBeDisabled();
    await fireEvent.click(screen.getByRole("button", { name: "Resume" }));
    expect(hostOperationsStoreMock.resumeHostPrint).toHaveBeenCalledWith("prn-1");
  });

  it("Cancel print… confirms in an alert dialog before cancelling", async () => {
    renderPanel(printing());
    await fireEvent.click(screen.getByRole("button", { name: "Cancel print…" }));
    const dialog = await screen.findByRole("alertdialog");
    await fireEvent.click(within(dialog).getByRole("button", { name: "Keep printing" }));
    expect(hostOperationsStoreMock.cancelHostPrint).not.toHaveBeenCalled();

    await fireEvent.click(screen.getByRole("button", { name: "Cancel print…" }));
    await fireEvent.click(within(await screen.findByRole("alertdialog")).getByRole("button", { name: "Cancel print" }));
    expect(hostOperationsStoreMock.cancelHostPrint).toHaveBeenCalledWith("prn-1");
  });

  it("the Cancel confirmation can't send once the offer is withdrawn while it is open", async () => {
    const [printer, setPrinter] = createSignal(printing());
    renderPanel(printer);
    await fireEvent.click(screen.getByRole("button", { name: "Cancel print…" }));
    const dialog = await screen.findByRole("alertdialog");
    setPrinter(resolvedPrinter({ runtimeStatus: printerStatus("finished") }));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: "Cancel print" })).toBeDisabled());
    expect(dialog).toHaveTextContent("The printer isn't in a state to cancel now: the last print finished.");
    await fireEvent.click(within(dialog).getByRole("button", { name: "Cancel print" }));
    expect(hostOperationsStoreMock.cancelHostPrint).not.toHaveBeenCalled();
  });

  it("an unsupported control is never rendered as a button", () => {
    unsupported("pause", "resume");
    renderPanel(printing());
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Resume" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Cancel print…" })).toBeEnabled();
  });

  it("a control rejection renders inline as an alert with its recovery", async () => {
    hostOperationsStoreMock.pauseHostPrint.mockRejectedValueOnce({
      contractVersion: 1, code: "CONTROL_NOT_ALLOWED", recovery: ["RELOAD"], retryable: false,
      message: "The printer isn't in a state to pause now: it is paused.",
    });
    renderPanel(printing());
    await fireEvent.click(screen.getByRole("button", { name: "Pause" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("it is paused");
    expect(screen.getByRole("button", { name: "Reload" })).toBeInTheDocument();
  });
});

describe("PrinterJobPanel: while a Host Operation is unresolved", () => {
  it("every control is disabled, with the spec's copy about the printer itself", () => {
    setHostOperationsStoreState([STAGED, hostOperation({ id: "hop-p", printerId: "prn-1", kind: "pause", state: "dispatching" })]);
    renderPanel(printing({ runtimeStatus: printerStatus("finished") }));
    expect(screen.getByRole("button", { name: "Pause" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Resume" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Cancel print…" })).toBeDisabled();
    expect(screen.getByText(PENDING_CONTROL)).toBeInTheDocument();
    expect(screen.getByRole("button", { name: /^Start/ })).toBeDisabled();
    expect(screen.getByText("A printer operation is pending.")).toBeInTheDocument();
  });
});

describe("PrinterJobPanel: Staged on this Printer", () => {
  it("lists staged artifacts; Start… follows startOffer and opens the Start dialog", async () => {
    setHostOperationsStoreState([STAGED]);
    renderPanel(resolvedPrinter({ runtimeStatus: printerStatus("finished") }));
    const staged = screen.getByRole("region", { name: "Staged on this Printer" });
    expect(staged).toHaveTextContent("farm3d/slr-7.gcode");
    await fireEvent.click(within(staged).getByRole("button", { name: "Start…" }));
    expect(await screen.findByRole("checkbox", { name: "The previous print finished. The bed is clear." })).toBeInTheDocument();
  });

  it("never enables Start after a host-reported Failed", () => {
    setHostOperationsStoreState([STAGED]);
    renderPanel(resolvedPrinter({ runtimeStatus: printerStatus("failed") }));
    expect(screen.getByRole("button", { name: "Start…" })).toBeDisabled();
    expect(screen.getByText("Clear the error on the printer first.")).toBeInTheDocument();
  });

  it("a Failed → Ready status event enables Start without a reload", async () => {
    setHostOperationsStoreState([STAGED]);
    const [printer, setPrinter] = createSignal(resolvedPrinter({ runtimeStatus: printerStatus("failed") }));
    renderPanel(printer);
    expect(screen.getByRole("button", { name: "Start…" })).toBeDisabled();
    setPrinter(resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    await waitFor(() => expect(screen.getByRole("button", { name: "Start…" })).toBeEnabled());
  });

  it("Start… is never enabled when the start capability is unsupported", () => {
    unsupported("start");
    setHostOperationsStoreState([STAGED]);
    renderPanel(resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    expect(screen.getByRole("button", { name: "Start…" })).toBeDisabled();
    expect(screen.getByText("Start: Not verified yet")).toBeInTheDocument();
  });
});

describe("PrinterJobPanel: Host Operations", () => {
  const uncertain = (overrides: Partial<HostOperation> = {}) => hostOperation({
    id: "hop-u", printerId: "prn-1", kind: "upload", state: "uncertain", attempts: 1, hostPath: "farm3d/slr-9.gcode",
    lastAttempt: { at: "2026-09-25T00:00:00Z", reason: "responseLost" }, ...overrides,
  });

  it("shows an uncertain row's state and inconclusive reason, with Check again and Abandon check…", async () => {
    setHostOperationsStoreState([uncertain()]);
    renderPanel();
    const operations = screen.getByRole("region", { name: "Printer operations" });
    expect(operations).toHaveTextContent("Upload uncertain");
    expect(operations).toHaveTextContent("The printer's answer was lost.");
    await fireEvent.click(within(operations).getByRole("button", { name: "Check again" }));
    expect(hostOperationsStoreMock.reconcileHostOperation).toHaveBeenCalledWith("hop-u");
    await fireEvent.click(within(operations).getByRole("button", { name: "Abandon check…" }));
    expect(await screen.findByRole("alertdialog")).toHaveTextContent("farm3d/slr-9.gcode");
  });

  it("Abandon check… waits for one attempt while the row is reconcilable", () => {
    setHostOperationsStoreState([uncertain({ attempts: 0, lastAttempt: null })]);
    renderPanel();
    expect(screen.getByRole("button", { name: "Abandon check…" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Check again" })).toBeEnabled();
  });

  it("with the reconcile capability unsupported, Check again is disabled and Abandon check… is allowed at once", () => {
    unsupported("artifactIdentity");
    setHostOperationsStoreState([uncertain({ attempts: 0, lastAttempt: null })]);
    renderPanel();
    expect(screen.getByRole("button", { name: "Check again" })).toBeDisabled();
    expect(screen.getByRole("button", { name: "Abandon check…" })).toBeEnabled();
  });

  it("a failed row shows its failure copy, and no check actions", () => {
    setHostOperationsStoreState([hostOperation({
      id: "hop-f", printerId: "prn-1", kind: "start", state: "failed",
      failure: { code: "hostBusy", message: "The printer is busy with another print." },
    })]);
    renderPanel();
    const operations = screen.getByRole("region", { name: "Printer operations" });
    expect(operations).toHaveTextContent("Start failed");
    expect(operations).toHaveTextContent("The printer is busy with another print.");
    expect(within(operations).queryByRole("button", { name: "Check again" })).not.toBeInTheDocument();
  });

  it("announces Host Operation state changes through one polite live region", async () => {
    setHostOperationsStoreState([hostOperation({ id: "hop-p", printerId: "prn-1", kind: "pause", state: "dispatching", hostPath: "farm3d/a.gcode" })]);
    renderPanel(printing());
    const live = document.querySelectorAll('[aria-live="polite"]');
    expect(live).toHaveLength(1);
    expect(live[0]).toHaveTextContent("");
    setHostOperationsStoreState([hostOperation({ id: "hop-p", printerId: "prn-1", kind: "pause", state: "succeeded", hostPath: "farm3d/a.gcode" })]);
    await waitFor(() => expect(live[0]).toHaveTextContent("Paused: farm3d/a.gcode"));
  });
});

describe("PrinterJobPanel: no Connection", () => {
  it("says so instead of offering anything", () => {
    renderPanel(resolvedPrinter({ connection: undefined, runtimeStatus: undefined }));
    expect(screen.getByText("This Printer has no Connection. Add one in Setup to stage and control prints.")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Pause" })).not.toBeInTheDocument();
  });
});
