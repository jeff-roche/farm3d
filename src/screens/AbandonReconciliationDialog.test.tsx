import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { hostOperationsStoreMock, resetHostOperationsStoreMock } from "../host-ops/host-operations-store-mock";
import { hostOperation } from "../host-ops/test-records";
import type { HostOperation } from "../host-ops/types";
import { AbandonReconciliationDialog } from "./AbandonReconciliationDialog";

vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);

const UNCERTAIN = hostOperation({
  id: "hop-u", printerId: "prn-1", kind: "upload", state: "uncertain", attempts: 1,
  hostPath: "farm3d/slr-7.gcode", lastAttempt: { at: "2026-09-25T00:00:00Z", reason: "responseLost" },
});
const ACKNOWLEDGEMENT = "I understand the printer may still have this file or be printing it.";

beforeEach(() => resetHostOperationsStoreMock());
afterEach(cleanup);

function renderDialog(operation: HostOperation = UNCERTAIN) {
  const onOpenChange = vi.fn();
  render(() => <AbandonReconciliationDialog open onOpenChange={onOpenChange} operation={operation} printerName="Bay 1" />);
  return { onOpenChange };
}

const confirm = () => screen.getByRole("button", { name: "Stop checking" });

describe("AbandonReconciliationDialog", () => {
  it("is an alertdialog naming the Printer, the operation, and the file, and says the state stays unknown", async () => {
    renderDialog();
    const dialog = await screen.findByRole("alertdialog");
    expect(dialog).toHaveTextContent("Upload uncertain");
    expect(dialog).toHaveTextContent("farm3d/slr-7.gcode");
    expect(dialog).toHaveTextContent("Bay 1");
    expect(dialog).toHaveTextContent("farm3d will stop checking");
    expect(dialog).toHaveTextContent("The printer's state stays unknown");
    expect(dialog).not.toHaveTextContent("Klipper restarted after farm3d sent this");
  });

  it("adds the noLongerPending sentence when it is set", async () => {
    renderDialog({ ...UNCERTAIN, kind: "start", noLongerPending: true });
    expect(await screen.findByText("Klipper restarted after farm3d sent this, so it is no longer waiting to run.")).toBeInTheDocument();
  });

  it("cannot be confirmed unticked", async () => {
    renderDialog();
    await screen.findByRole("alertdialog");
    expect(confirm()).toBeDisabled();
    await fireEvent.click(confirm());
    expect(hostOperationsStoreMock.abandonHostOperation).not.toHaveBeenCalled();
    expect(screen.getByText("Tick the acknowledgement to stop checking.")).toBeInTheDocument();
  });

  it("once ticked, abandons with the optional note and closes", async () => {
    const { onOpenChange } = renderDialog();
    await fireEvent.click(await screen.findByRole("checkbox", { name: ACKNOWLEDGEMENT }));
    await fireEvent.input(screen.getByRole("textbox", { name: "Note (optional)" }), { target: { value: "Checked the printer by hand." } });
    expect(confirm()).toBeEnabled();
    await fireEvent.click(confirm());
    expect(hostOperationsStoreMock.abandonHostOperation).toHaveBeenCalledWith("hop-u", "Checked the printer by hand.");
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("counts every character typed, spaces included", async () => {
    renderDialog();
    await fireEvent.input(await screen.findByRole("textbox", { name: "Note (optional)" }), { target: { value: "  ok  " } });
    expect(screen.getByText("6/500")).toBeInTheDocument();
  });

  it("refuses a note over 500 characters", async () => {
    renderDialog();
    await fireEvent.click(await screen.findByRole("checkbox", { name: ACKNOWLEDGEMENT }));
    await fireEvent.input(screen.getByRole("textbox", { name: "Note (optional)" }), { target: { value: "x".repeat(501) } });
    expect(screen.getByText("The note can be at most 500 characters.")).toBeInTheDocument();
    expect(confirm()).toBeDisabled();
  });

  it("shows HOST_OPERATION_NOT_ABANDONABLE inline with Reload", async () => {
    hostOperationsStoreMock.abandonHostOperation.mockRejectedValueOnce({
      contractVersion: 1, code: "HOST_OPERATION_NOT_ABANDONABLE", recovery: ["RELOAD"], retryable: false,
      message: "farm3d can only stop checking an uncertain operation after it has checked at least once.",
    });
    renderDialog();
    await fireEvent.click(await screen.findByRole("checkbox", { name: ACKNOWLEDGEMENT }));
    await fireEvent.click(confirm());
    expect(await screen.findByRole("alert")).toHaveTextContent("after it has checked at least once");
    await fireEvent.click(screen.getByRole("button", { name: "Reload" }));
    expect(hostOperationsStoreMock.refreshHostOperations).toHaveBeenCalled();
  });
});
