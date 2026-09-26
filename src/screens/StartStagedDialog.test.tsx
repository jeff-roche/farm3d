import { createSignal } from "solid-js";
import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import {
  hostOperationsStoreMock,
  resetHostOperationsStoreMock,
  setHostOperationsStoreState,
} from "../host-ops/host-operations-store-mock";
import { startOffer } from "../host-ops/start-rule";
import { hostOperation, printerStatus, resolvedPrinter } from "../host-ops/test-records";
import type { OperationalState } from "../host-ops/types";
import type { ResolvedPrinter } from "../printers/types";
import { StartStagedDialog } from "./StartStagedDialog";

vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);

const STAGED = hostOperation({
  id: "hop-staged", printerId: "prn-1", kind: "upload", state: "succeeded",
  sliceRevisionId: "slr-7", hostPath: "farm3d/slr-7.gcode", resolution: { kind: "artifactVerified", reconciled: false },
});

beforeEach(() => resetHostOperationsStoreMock());
afterEach(cleanup);

function commandError(code: string, message: string, recovery: string[] = []) {
  return { contractVersion: 1, code, message, recovery, retryable: false };
}

function renderDialog(printer: () => ResolvedPrinter) {
  const onOpenChange = vi.fn();
  render(() => <StartStagedDialog open onOpenChange={onOpenChange} printer={printer()} staged={STAGED} />);
  return { onOpenChange };
}

const confirmButton = () => screen.getByRole("button", { name: "Start print" });

describe("StartStagedDialog: the Start table", () => {
  const offered: [OperationalState, string, string][] = [
    ["ready", "ready", "The bed is clear."],
    ["finished", "finished", "The previous print finished. The bed is clear."],
    ["cancelled", "cancelled", "The previous print was cancelled. The bed is clear."],
  ];

  for (const [state, priorState, label] of offered) {
    for (const startSafety of ["confirmBedClear", "unattended"] as const) {
      it(`${state} (${startSafety}): offers Start behind "${label}" and sends priorState ${priorState}`, async () => {
        const status = printerStatus(state);
        const offer = startOffer(status, false);
        expect(offer).toMatchObject({ offered: true, confirmLabel: label });
        const { onOpenChange } = renderDialog(() => resolvedPrinter({ startSafety, runtimeStatus: status }));

        const checkbox = await screen.findByRole("checkbox", { name: label });
        expect(checkbox).not.toBeChecked();
        expect(confirmButton()).toBeDisabled();
        expect(screen.getByText("Tick the confirmation to start.")).toBeInTheDocument();

        await fireEvent.click(checkbox);
        expect(confirmButton()).toBeEnabled();
        await fireEvent.click(confirmButton());

        expect(hostOperationsStoreMock.startStagedArtifact).toHaveBeenCalledWith("prn-1", "hop-staged", priorState);
        await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
      });
    }
  }

  it("Failed: Start is disabled with \"Clear the error on the printer first.\" and no checkbox", async () => {
    renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("failed") }));
    expect(await screen.findByText("Clear the error on the printer first.")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(confirmButton()).toBeDisabled();
  });

  const refused: OperationalState[] = ["printing", "paused", "busy", "offline", "connecting", "unknown", "error", "setupIncomplete"];
  for (const state of refused) {
    it(`${state}: Start is disabled with the state as the reason`, async () => {
      const status = printerStatus(state);
      const offer = startOffer(status, false);
      if (offer.offered) throw new Error("expected a refusal");
      renderDialog(() => resolvedPrinter({ runtimeStatus: status }));
      expect(await screen.findByText(`The printer can't start a print now: ${offer.reason}.`)).toBeInTheDocument();
      expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
      expect(confirmButton()).toBeDisabled();
    });
  }

  it("stale telemetry: Start is disabled even while Ready", async () => {
    renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("ready", "stale") }));
    expect(await screen.findByText("The printer can't start a print now: its status is out of date.")).toBeInTheDocument();
    expect(confirmButton()).toBeDisabled();
  });

  it("an unresolved Host Operation disables Start with \"A printer operation is pending.\"", async () => {
    setHostOperationsStoreState([STAGED, hostOperation({ id: "hop-pending", printerId: "prn-1", state: "uncertain" })]);
    renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    expect(await screen.findByText("A printer operation is pending.")).toBeInTheDocument();
    expect(screen.queryByRole("checkbox")).not.toBeInTheDocument();
    expect(confirmButton()).toBeDisabled();
  });
});

describe("StartStagedDialog: re-rendering for the live status", () => {
  it("a Failed → Ready status event enables Start without a reload", async () => {
    const [printer, setPrinter] = createSignal(resolvedPrinter({ runtimeStatus: printerStatus("failed") }));
    renderDialog(printer);
    expect(await screen.findByText("Clear the error on the printer first.")).toBeInTheDocument();

    setPrinter(resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    const checkbox = await screen.findByRole("checkbox", { name: "The bed is clear." });
    await fireEvent.click(checkbox);
    expect(confirmButton()).toBeEnabled();
  });

  it("a tick given for one state is never reused for another", async () => {
    const [printer, setPrinter] = createSignal(resolvedPrinter({ runtimeStatus: printerStatus("finished") }));
    renderDialog(printer);
    await fireEvent.click(await screen.findByRole("checkbox", { name: "The previous print finished. The bed is clear." }));
    expect(confirmButton()).toBeEnabled();

    setPrinter(resolvedPrinter({ runtimeStatus: printerStatus("cancelled") }));
    const next = await screen.findByRole("checkbox", { name: "The previous print was cancelled. The bed is clear." });
    expect(next).not.toBeChecked();
    expect(confirmButton()).toBeDisabled();
  });

  it("START_PRECONDITION_CHANGED clears the tick and shows the message inline", async () => {
    hostOperationsStoreMock.startStagedArtifact.mockRejectedValueOnce(
      commandError("START_PRECONDITION_CHANGED", "The printer's state changed. Confirm the bed again.", ["RELOAD"]),
    );
    renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    const checkbox = await screen.findByRole("checkbox", { name: "The bed is clear." });
    await fireEvent.click(checkbox);
    await fireEvent.click(confirmButton());

    const alert = await screen.findByRole("alert");
    expect(alert).toHaveTextContent("The printer's state changed. Confirm the bed again.");
    expect(screen.getByRole("checkbox", { name: "The bed is clear." })).not.toBeChecked();
    expect(confirmButton()).toBeDisabled();
    expect(screen.getByRole("button", { name: "Reload" })).toBeInTheDocument();
  });

  it("START_NOT_ALLOWED clears the tick too", async () => {
    hostOperationsStoreMock.startStagedArtifact.mockRejectedValueOnce(
      commandError("START_NOT_ALLOWED", "The printer can't start a print now: it is printing.", ["RELOAD"]),
    );
    renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    await fireEvent.click(await screen.findByRole("checkbox", { name: "The bed is clear." }));
    await fireEvent.click(confirmButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("it is printing");
    expect(screen.getByRole("checkbox", { name: "The bed is clear." })).not.toBeChecked();
  });

  it("shows \"Checking the file on the printer…\" while the command runs", async () => {
    let resolve!: (value: unknown) => void;
    hostOperationsStoreMock.startStagedArtifact.mockImplementationOnce(() => new Promise((res) => { resolve = res; }) as never);
    renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    await fireEvent.click(await screen.findByRole("checkbox", { name: "The bed is clear." }));
    await fireEvent.click(confirmButton());
    expect(await screen.findByText("Checking the file on the printer…")).toBeInTheDocument();
    expect(confirmButton()).toBeDisabled();
    resolve(hostOperation({ id: "hop-start", kind: "start" }));
    await waitFor(() => expect(screen.queryByText("Checking the file on the printer…")).not.toBeInTheDocument());
  });

  it("STAGED_ARTIFACT_INVALID offers Stage again, a local action that stages the same Slice Revision", async () => {
    hostOperationsStoreMock.startStagedArtifact.mockRejectedValueOnce(
      commandError("STAGED_ARTIFACT_INVALID", "The staged file is no longer on the printer. Stage it again."),
    );
    const { onOpenChange } = renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    await fireEvent.click(await screen.findByRole("checkbox", { name: "The bed is clear." }));
    await fireEvent.click(confirmButton());

    expect(await screen.findByRole("alert")).toHaveTextContent("The staged file is no longer on the printer. Stage it again.");
    await fireEvent.click(screen.getByRole("button", { name: "Stage again" }));
    expect(hostOperationsStoreMock.stageSliceRevision).toHaveBeenCalledWith("prn-1", "slr-7");
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });

  it("HOST_OPERATION_PENDING renders with a link to the Printer's Job tab", async () => {
    hostOperationsStoreMock.startStagedArtifact.mockRejectedValueOnce({
      ...commandError("HOST_OPERATION_PENDING", "This printer has a pending operation. Finish or abandon it first.", ["OPEN_PRINTER_JOB"]),
      details: { printerIds: ["prn-1"], hostOperationIds: ["hop-x"] },
    });
    renderDialog(() => resolvedPrinter({ runtimeStatus: printerStatus("ready") }));
    await fireEvent.click(await screen.findByRole("checkbox", { name: "The bed is clear." }));
    await fireEvent.click(confirmButton());
    expect(await screen.findByRole("alert")).toHaveTextContent("This printer has a pending operation.");
    expect(screen.getByRole("button", { name: "Open the Job tab" })).toBeInTheDocument();
  });
});
