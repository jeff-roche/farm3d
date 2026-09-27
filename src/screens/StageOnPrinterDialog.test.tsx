import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { resetCapabilitiesStoreMock, setPrinterCapabilitiesForTest } from "../host-ops/capabilities-store-mock";
import {
  hostOperationsStoreMock,
  resetHostOperationsStoreMock,
  setHostOperationsStoreState,
} from "../host-ops/host-operations-store-mock";
import { hostOperation, printerCapabilities, printerStatus, resolvedPrinter } from "../host-ops/test-records";
import type { ResolvedPrinter } from "../printers/types";
import { StageOnPrinterDialog } from "./StageOnPrinterDialog";

vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);
vi.mock("../host-ops/capabilities-store", async () =>
  (await import("../host-ops/capabilities-store-mock")).capabilitiesStoreMock);
const printerList = vi.hoisted(() => [] as ResolvedPrinter[]);
vi.mock("../printers/printer-store", () => ({ printers: () => printerList }));

function notVerifiedUpload(printerId: string) {
  const base = printerCapabilities({ printerId, adapterKind: "octoprint" });
  setPrinterCapabilitiesForTest({
    ...base,
    capabilities: { ...base.capabilities, upload: { status: "unsupported", reason: "notVerified", detail: "Not verified for this Connection type yet." } },
  });
}

beforeEach(() => {
  resetHostOperationsStoreMock();
  resetCapabilitiesStoreMock();
  printerList.length = 0;
  printerList.push(
    resolvedPrinter({ id: "prn-ok", name: "Ready bay" }),
    resolvedPrinter({ id: "prn-octo", name: "OctoPrint bay", connection: { kind: "octoprint", host: "192.0.2.11", port: 80, useTls: false } }),
    resolvedPrinter({ id: "prn-off", name: "Offline bay", runtimeStatus: printerStatus("offline", "fresh", { connectionState: "offline" }) }),
    resolvedPrinter({ id: "prn-busy", name: "Pending bay" }),
    resolvedPrinter({ id: "prn-archived", name: "Archived bay", archivedAt: "2026-09-01T00:00:00Z" }),
  );
  for (const id of ["prn-ok", "prn-off", "prn-busy", "prn-archived"]) setPrinterCapabilitiesForTest(printerCapabilities({ printerId: id }));
  notVerifiedUpload("prn-octo");
  setHostOperationsStoreState([hostOperation({ id: "hop-pending", printerId: "prn-busy", state: "uncertain" })]);
});
afterEach(cleanup);

function renderDialog() {
  const onOpenChange = vi.fn();
  render(() => <StageOnPrinterDialog open onOpenChange={onOpenChange} sliceRevisionId="slr-7" revisionTitle="Plate 1: Lid" />);
  return { onOpenChange };
}

async function openPicker() {
  await fireEvent.pointerDown(screen.getByRole("button", { name: /Choose a Printer/ }), { pointerType: "mouse", button: 0 });
  await screen.findByRole("listbox");
}

describe("StageOnPrinterDialog", () => {
  it("lists every active Printer; each ineligible one is disabled with its own, distinct reason", async () => {
    renderDialog();
    await openPicker();
    const option = (name: string) => screen.getByRole("option", { name: new RegExp(`^${name}`) });

    expect(option("Ready bay")).not.toHaveAttribute("aria-disabled", "true");
    expect(option("OctoPrint bay")).toHaveAttribute("aria-disabled", "true");
    expect(option("OctoPrint bay")).toHaveTextContent("Upload: Not verified yet");
    expect(option("Offline bay")).toHaveAttribute("aria-disabled", "true");
    expect(option("Offline bay")).toHaveTextContent("Offline");
    expect(option("Pending bay")).toHaveAttribute("aria-disabled", "true");
    expect(option("Pending bay")).toHaveTextContent("A printer operation is pending.");
    expect(screen.queryByRole("option", { name: /Archived bay/ })).not.toBeInTheDocument();

    const reasons = ["OctoPrint bay", "Offline bay", "Pending bay"].map((name) => option(name).textContent!.replace(name, ""));
    expect(new Set(reasons).size).toBe(3);
  });

  it("stages the Slice Revision on the chosen Printer, and says where to follow it", async () => {
    renderDialog();
    const stage = screen.getByRole("button", { name: "Stage" });
    expect(stage).toBeDisabled();
    await openPicker();
    await fireEvent.pointerUp(screen.getByRole("option", { name: "Ready bay" }), { pointerType: "mouse", button: 0 });
    await waitFor(() => expect(stage).toBeEnabled());
    await fireEvent.click(stage);
    expect(hostOperationsStoreMock.stageSliceRevision).toHaveBeenCalledWith("prn-ok", "slr-7");
    expect(await screen.findByText("Uploading to Ready bay. Its Job tab shows when the file is staged.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Open the Job tab" })).toBeInTheDocument();
  });

  it("a disabled Printer can't be chosen", async () => {
    renderDialog();
    await openPicker();
    await fireEvent.pointerUp(screen.getByRole("option", { name: /^Offline bay/ }), { pointerType: "mouse", button: 0 });
    expect(screen.getByRole("button", { name: "Stage" })).toBeDisabled();
  });

  it("shows a rejection inline with its recovery", async () => {
    hostOperationsStoreMock.stageSliceRevision.mockRejectedValueOnce({
      contractVersion: 1, code: "HOST_OPERATION_PENDING", recovery: ["OPEN_PRINTER_JOB"], retryable: false,
      message: "This printer has a pending operation. Finish or abandon it first.",
      details: { printerIds: ["prn-ok"], hostOperationIds: ["hop-x"] },
    });
    renderDialog();
    await openPicker();
    await fireEvent.pointerUp(screen.getByRole("option", { name: "Ready bay" }), { pointerType: "mouse", button: 0 });
    await fireEvent.click(screen.getByRole("button", { name: "Stage" }));
    expect(await screen.findByRole("alert")).toHaveTextContent("This printer has a pending operation.");
    expect(screen.getByRole("button", { name: "Open the Job tab" })).toBeInTheDocument();
  });
});
