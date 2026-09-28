import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { queueStoreMock, resetQueueStoreMock } from "../queue/queue-store-mock";
import { queueEntry } from "../queue/test-records";
import {
  buildWebSlicingFixture,
  WEB_SLICING_REVISION_EXTERNAL,
  WEB_SLICING_REVISION_FARM3D,
} from "../slicing/web-fixtures";
import type { SliceRevisionRecord } from "../slicing/types";
import { AddToQueueDialog } from "./AddToQueueDialog";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
vi.mock("../printers/printer-store", () => ({
  printers: () => [
    { id: "prn-bay-1", name: "Bay 1", archivedAt: null },
    { id: "prn-old", name: "Old bay", archivedAt: "2026-09-01T00:00:00Z" },
  ],
}));

const fixture = buildWebSlicingFixture(new Date("2026-09-24T12:00:00Z"));
const farm3d = (): SliceRevisionRecord => structuredClone(fixture.revisionRecords[WEB_SLICING_REVISION_FARM3D]!);
const external = (): SliceRevisionRecord => structuredClone(fixture.revisionRecords[WEB_SLICING_REVISION_EXTERNAL]!);

beforeEach(() => resetQueueStoreMock());
afterEach(cleanup);

function renderDialog(revision: SliceRevisionRecord, onAdded = vi.fn()) {
  render(() => <AddToQueueDialog open onOpenChange={vi.fn()} revision={revision} onAdded={onAdded} />);
  return { dialog: screen.getByRole("dialog", { name: "Add to Queue" }), onAdded };
}

async function pick(trigger: HTMLElement, option: RegExp) {
  await fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
  const item = await screen.findByRole("option", { name: option });
  await fireEvent.pointerDown(item, { button: 0, pointerType: "mouse" });
  await fireEvent.pointerUp(item, { button: 0, pointerType: "mouse" });
}

describe("AddToQueueDialog", () => {
  it("adds quantity 3 in one add_to_queue call and reports the first new entry", async () => {
    queueStoreMock.addToQueue.mockResolvedValueOnce({
      entries: [
        queueEntry({ id: "qen-b", copyIndex: 2, copyCount: 3 }),
        queueEntry({ id: "qen-a", copyIndex: 1, copyCount: 3 }),
        queueEntry({ id: "qen-c", copyIndex: 3, copyCount: 3 }),
      ],
      jobs: [],
      requirements: [],
    });
    const { dialog, onAdded } = renderDialog(farm3d());
    // A farm3d estimate is shown, and left for Rust to fill.
    expect(dialog).toHaveTextContent("38.6 g from the slice");
    const quantity = within(dialog).getByRole("spinbutton", { name: "Copies" });
    fireEvent.input(quantity, { target: { value: "3" } });
    fireEvent.focusOut(quantity);
    fireEvent.click(within(dialog).getByRole("button", { name: "Add 3 copies" }));
    await waitFor(() => expect(queueStoreMock.addToQueue).toHaveBeenCalledOnce());
    expect(queueStoreMock.addToQueue).toHaveBeenCalledWith(WEB_SLICING_REVISION_FARM3D, 3, "recommended", "loadedFirst", {});
    await waitFor(() => expect(onAdded).toHaveBeenCalledWith("qen-a"));
  });

  it("disables Add for an external revision with no estimate, and says why", () => {
    const { dialog } = renderDialog(external());
    const add = within(dialog).getByRole("button", { name: "Add 1 copy" });
    expect(add).toBeDisabled();
    expect(add).toHaveAccessibleDescription("Enter the material this print uses.");
    expect(within(dialog).getByText("Enter the material this print uses.")).toBeVisible();
    // The file claims no weight, so there's nothing to accept.
    expect(within(dialog).queryByLabelText(/Use the file's claim/)).toBeNull();
  });

  it("sends an entered amount and the chosen manual Printer for a revision that needs one", async () => {
    const { dialog } = renderDialog(external());
    // Manual is the only policy it can take.
    const policy = within(dialog).getByRole("radiogroup", { name: "Dispatch Policy" });
    expect(within(policy).getByLabelText("Manual")).toBeChecked();
    expect(within(policy).getByLabelText("Automatic")).toBeDisabled();

    fireEvent.input(within(dialog).getByRole("textbox", { name: "Material (g)" }), { target: { value: "25" } });
    const add = within(dialog).getByRole("button", { name: "Add 1 copy" });
    expect(add).toHaveAccessibleDescription("Choose the Printer this runs on.");
    await pick(within(dialog).getByRole("button", { name: /Printer/ }), /^Bay 1/);
    // An archived Printer isn't offered.
    await waitFor(() => expect(add).toBeEnabled());
    fireEvent.click(add);
    await waitFor(() => expect(queueStoreMock.addToQueue).toHaveBeenCalledWith(
      WEB_SLICING_REVISION_EXTERNAL, 1, "manual", "loadedFirst",
      { materialEstimate: { amountMg: 25_000, source: "operatorEntered" }, manualPrinterId: "prn-bay-1" },
    ));
  });

  it("offers to accept the file's claimed grams, rounded up to the milligram as Rust does", async () => {
    const revision = external();
    revision.claimedEstimates!.filamentGrams = 12.3456;
    revision.requiresManualPrinterSelection = false;
    const { dialog } = renderDialog(revision);
    const claim = within(dialog).getByLabelText("Use the file's claim (12.3 g)");
    fireEvent.click(claim);
    fireEvent.click(within(dialog).getByRole("button", { name: "Add 1 copy" }));
    await waitFor(() => expect(queueStoreMock.addToQueue).toHaveBeenCalledWith(
      WEB_SLICING_REVISION_EXTERNAL, 1, "recommended", "loadedFirst",
      { materialEstimate: { amountMg: 12_346, source: "fileClaimConfirmed" } },
    ));
  });

  it("treats a zero-gram claim as no claim, so entering an amount still works", async () => {
    const revision = external();
    revision.claimedEstimates!.filamentGrams = 0;
    revision.requiresManualPrinterSelection = false;
    const { dialog } = renderDialog(revision);
    expect(within(dialog).queryByLabelText(/Use the file's claim/)).toBeNull();
    const add = within(dialog).getByRole("button", { name: "Add 1 copy" });
    expect(add).toHaveAccessibleDescription("Enter the material this print uses.");
    fireEvent.input(within(dialog).getByRole("textbox", { name: "Material (g)" }), { target: { value: "12.5" } });
    await waitFor(() => expect(add).toBeEnabled());
    fireEvent.click(add);
    await waitFor(() => expect(queueStoreMock.addToQueue).toHaveBeenCalledWith(
      WEB_SLICING_REVISION_EXTERNAL, 1, "recommended", "loadedFirst",
      { materialEstimate: { amountMg: 12_500, source: "operatorEntered" } },
    ));
  });

  it("shows a refusal inline and stays open", async () => {
    queueStoreMock.addToQueue.mockRejectedValueOnce({
      contractVersion: 1, code: "VALIDATION", message: "Choose between 1 and 50 copies.", recovery: ["EDIT_FIELDS"], retryable: false,
    });
    const { dialog, onAdded } = renderDialog(farm3d());
    fireEvent.click(within(dialog).getByRole("button", { name: "Add 1 copy" }));
    expect(await within(dialog).findByRole("alert")).toHaveTextContent("Choose between 1 and 50 copies.");
    expect(onAdded).not.toHaveBeenCalled();
  });
});
