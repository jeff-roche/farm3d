import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { queueStoreMock, resetQueueStoreMock } from "../queue/queue-store-mock";
import { queueEntry, queueEntryEligibility } from "../queue/test-records";
import type { Candidate } from "../queue/types";
import { pressEscape, tabOrder } from "./dialog-test-helpers";
import { AssignJobDialog } from "./AssignJobDialog";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);

beforeEach(() => resetQueueStoreMock());
afterEach(cleanup);

const VORON: Candidate = {
  printerId: "prn-2", printerName: "Voron", rank: 1,
  spool: { spoolId: "spl-4", spoolNumber: 4, loadedOnPrinter: true, availableMg: 500_000 },
  spoolOptions: [
    { spoolId: "spl-4", spoolNumber: 4, loadedOnPrinter: true, availableMg: 500_000 },
    { spoolId: "spl-5", spoolNumber: 5, loadedOnPrinter: false, availableMg: 250_000 },
  ],
  loadedMatch: true, lastUsedAt: null, manualFactsAcknowledgementRequired: false,
};
const PRUSA: Candidate = {
  printerId: "prn-1", printerName: "Prusa", rank: 2,
  spool: { spoolId: "spl-2", spoolNumber: 2, loadedOnPrinter: false, availableMg: 900_000 },
  spoolOptions: [
    { spoolId: "spl-2", spoolNumber: 2, loadedOnPrinter: false, availableMg: 900_000 },
    { spoolId: "spl-3", spoolNumber: 3, loadedOnPrinter: false, availableMg: 700_000 },
  ],
  loadedMatch: false, lastUsedAt: null, manualFactsAcknowledgementRequired: true,
};

function renderDialog(onOpenChange = vi.fn()) {
  queueStoreMock.explainQueueEntry.mockResolvedValueOnce(queueEntryEligibility({ candidates: [VORON, PRUSA] }));
  render(() => <AssignJobDialog entry={queueEntry({ id: "qen-1" })} open onOpenChange={onOpenChange} />);
  return onOpenChange;
}

async function chooseSpool(dialog: HTMLElement, name: RegExp) {
  fireEvent.pointerDown(within(dialog).getByRole("button", { name: /Spool/ }));
  const option = await screen.findByRole("option", { name });
  fireEvent.pointerDown(option, { button: 0, pointerType: "mouse" });
  fireEvent.pointerUp(option, { button: 0, pointerType: "mouse" });
}

describe("AssignJobDialog", () => {
  it("lists the candidates in Rust's order and assigns the rank-1 Printer with its Spool", async () => {
    renderDialog();
    const dialog = await screen.findByRole("dialog", { name: "Assign to a Printer" });
    const printers = await within(dialog).findByRole("radiogroup", { name: "Printer" });
    const radios = within(printers).getAllByRole("radio");
    expect(radios.map((radio) => (radio as HTMLInputElement).labels?.[0]?.textContent)).toEqual(["1 · Voron", "2 · Prusa"]);
    expect(radios[0]).toBeChecked();
    fireEvent.click(within(dialog).getByRole("button", { name: "Assign" }));
    await waitFor(() => expect(queueStoreMock.assignQueueEntry).toHaveBeenCalledWith("qen-1", "prn-2", "spl-4", undefined));
  });

  it("assigns with the chosen Spool, and needs the manual-facts acknowledgement when Rust asks for it", async () => {
    renderDialog();
    const dialog = await screen.findByRole("dialog", { name: "Assign to a Printer" });
    fireEvent.click(await within(dialog).findByLabelText("2 · Prusa"));
    await chooseSpool(dialog, /Spool #3/);
    const assign = within(dialog).getByRole("button", { name: "Assign" });
    expect(assign).toBeDisabled();
    expect(within(dialog).getByText("Tick the acknowledgement to assign.")).toBeInTheDocument();
    fireEvent.click(within(dialog).getByRole("checkbox"));
    await waitFor(() => expect(assign).toBeEnabled());
    fireEvent.click(assign);
    await waitFor(() => expect(queueStoreMock.assignQueueEntry).toHaveBeenCalledWith("qen-1", "prn-1", "spl-3", true));
  });

  it("keeps each candidate's own Spool choice", async () => {
    renderDialog();
    const dialog = await screen.findByRole("dialog", { name: "Assign to a Printer" });
    await within(dialog).findByLabelText("1 · Voron");
    await chooseSpool(dialog, /Spool #5/);
    fireEvent.click(within(dialog).getByLabelText("2 · Prusa"));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: /Spool/ })).toHaveTextContent("Spool #2"));
    fireEvent.click(within(dialog).getByLabelText("1 · Voron"));
    await waitFor(() => expect(within(dialog).getByRole("button", { name: /Spool/ })).toHaveTextContent("Spool #5"));
  });

  it("shows why nothing can take the entry when Rust lists no candidate", async () => {
    queueStoreMock.explainQueueEntry.mockResolvedValueOnce(queueEntryEligibility({
      verdict: "blocked",
      blockers: [{ code: "NO_COMPATIBLE_SPOOL", message: "No Spool matches this Slice.", detail: null, recovery: "LOAD_SPOOL", printerIds: [] }],
    }));
    render(() => <AssignJobDialog entry={queueEntry({ id: "qen-1" })} open onOpenChange={vi.fn()} />);
    expect(await screen.findByText("No Spool matches this Slice.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Assign" })).toBeDisabled();
  });

  it("is keyboard operable: Tab walks Printer, Spool, then the buttons; Escape closes", async () => {
    const onOpenChange = renderDialog();
    const dialog = await screen.findByRole("dialog", { name: "Assign to a Printer" });
    await within(dialog).findByLabelText("1 · Voron");
    const order = tabOrder(dialog);
    expect(order[0]).toBe("Close");
    expect(order.slice(1)).toEqual(["1 · Voron", expect.stringMatching(/Spool/), "Cancel", "Assign"]);
    pressEscape(dialog);
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
  });
});
