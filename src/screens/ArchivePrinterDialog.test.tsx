import { fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { ResolvedPrinter } from "../printers/types";
import { ArchivePrinterDialog } from "./ArchivePrinterDialog";

const archivePrinter = vi.hoisted(() => vi.fn());
const printersList = vi.hoisted(() => [] as unknown[]);
vi.mock("../printers/printer-store", () => ({
  printers: () => printersList,
  archivePrinter: (...args: unknown[]) => archivePrinter(...args),
}));

const spools = vi.hoisted(() => [] as SpoolRecord[]);
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools, loaded: true };
  },
}));

function spool(overrides: Partial<SpoolRecord>): SpoolRecord {
  return {
    id: "spl-1", revision: 1, spoolNumber: 1,
    manufacturer: "Prusament", materialFamily: "PLA", colorName: "Galaxy Black", diameter: "1.75",
    nominalMg: 1_000_000, lowThresholdMg: 100_000, lifecycle: "active",
    location: { kind: "slot", slotId: "slt-own-1", printerId: "prn-1" },
    availability: { currentMg: 500_000, reservedMg: 0, availableMg: 500_000 },
    facets: { loaded: true, reserved: false, low: false, confidence: "measured" },
    createdAt: "", updatedAt: "",
    ...overrides,
  };
}

const LOADED_A = spool({ id: "spl-a", revision: 3, spoolNumber: 7, materialFamily: "PETG", colorName: "Black" });
const LOADED_B = spool({
  id: "spl-b", revision: 2, spoolNumber: 8, colorName: "Clear",
  location: { kind: "slot", slotId: "slt-own-2", printerId: "prn-1" },
});
const OCCUPANT = spool({
  id: "spl-occ", spoolNumber: 5, materialFamily: "ABS", colorName: "White",
  location: { kind: "slot", slotId: "slt-b", printerId: "prn-2" },
});
const RACER = spool({ id: "spl-racer", spoolNumber: 6, materialFamily: "TPU", colorName: "Red" });

const THIS_PRINTER = {
  id: "prn-1", name: "Bay 1",
  materialSlots: [
    { id: "slt-own-1", position: 0, name: "Main", occupantSpoolId: "spl-a" },
    { id: "slt-own-2", position: 1, name: "Aux", occupantSpoolId: "spl-b" },
  ],
} as unknown as ResolvedPrinter;

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  spools.length = 0;
  printersList.length = 0;
});

function seed() {
  spools.push(LOADED_A, LOADED_B, OCCUPANT, RACER);
  printersList.push(
    THIS_PRINTER,
    {
      id: "prn-2", name: "Bay 2",
      materialSlots: [
        { id: "slt-a", position: 0, name: "A" },
        { id: "slt-b", position: 1, name: "B", feederLabel: "AMS 1", occupantSpoolId: "spl-occ" },
      ],
    },
    { id: "prn-3", name: "Bay 3 (old)", archivedAt: "2026-09-01T00:00:00Z", materialSlots: [{ id: "slt-z", position: 0, name: "Z" }] },
  );
}

function renderDialog(loaded: SpoolRecord[] = [LOADED_A, LOADED_B]) {
  const onOpenChange = vi.fn();
  const onArchived = vi.fn();
  render(() => (
    <ArchivePrinterDialog open onOpenChange={onOpenChange} printer={THIS_PRINTER} loadedSpools={loaded} onArchived={onArchived} />
  ));
  return { onOpenChange, onArchived };
}

const row = (spoolNumber: number) => screen.getByRole("group", { name: new RegExp(`#${spoolNumber} `) });

async function pick(trigger: HTMLElement, option: RegExp) {
  await fireEvent.pointerDown(trigger, { button: 0, pointerType: "mouse" });
  const item = await screen.findByRole("option", { name: option });
  await fireEvent.pointerDown(item, { button: 0, pointerType: "mouse" });
  await fireEvent.pointerUp(item, { button: 0, pointerType: "mouse" });
}

describe("ArchivePrinterDialog", () => {
  it("disables Archive until every loaded Spool has a disposition", async () => {
    seed();
    renderDialog();
    const archive = screen.getByRole("button", { name: "Archive" });
    expect(archive).toBeDisabled();

    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /^Storage$/);
    expect(archive).toBeDisabled();
    await pick(within(row(8)).getByRole("button", { name: /Where #8 goes/ }), /Mark empty \(used up\)/);
    expect(archive).not.toBeDisabled();
  });

  it("offers only other, active Printers' slots, and asks where a swapped-out occupant goes", async () => {
    seed();
    renderDialog([LOADED_A]);
    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /Another Printer's slot/);

    const slotTrigger = within(row(7)).getByRole("button", { name: /Destination slot/ });
    await fireEvent.pointerDown(slotTrigger, { button: 0, pointerType: "mouse" });
    const options = (await screen.findAllByRole("option")).map((o) => o.textContent);
    expect(options).toEqual(["Bay 2 — A — empty", "Bay 2 — B (AMS 1) — occupied by #5 ABS White"]);
    const optionB = screen.getByRole("option", { name: /Bay 2 — B/ });
    await fireEvent.pointerDown(optionB, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(optionB, { button: 0, pointerType: "mouse" });

    expect(within(row(7)).getByText("Swap: #5 ABS White goes to storage")).toBeInTheDocument();
    const archive = screen.getByRole("button", { name: "Archive" });
    expect(archive).toBeDisabled();
    fireEvent.input(within(row(7)).getByLabelText(/Displaced Spool storage label/), { target: { value: "Shelf C" } });
    expect(archive).not.toBeDisabled();
  });

  it("submits every disposition, then closes", async () => {
    seed();
    archivePrinter.mockResolvedValue(undefined);
    const { onOpenChange, onArchived } = renderDialog();
    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /^Storage$/);
    fireEvent.input(within(row(7)).getByLabelText(/Storage label/), { target: { value: " Shelf A " } });
    await pick(within(row(8)).getByRole("button", { name: /Where #8 goes/ }), /Mark empty/);

    fireEvent.click(screen.getByRole("button", { name: "Archive" }));

    await waitFor(() => expect(archivePrinter).toHaveBeenCalledWith("prn-1", [
      { spoolId: "spl-a", expectedSpoolRevision: 3, disposition: { kind: "storage", storageLabel: "Shelf A" } },
      { spoolId: "spl-b", expectedSpoolRevision: 2, disposition: { kind: "markEmpty", storageLabel: null } },
    ]));
    await waitFor(() => expect(onOpenChange).toHaveBeenCalledWith(false));
    expect(onArchived).toHaveBeenCalled();
  });

  it("keeps the dialog open and shows a CONFLICT on the affected row, retrying against the new occupant", async () => {
    seed();
    archivePrinter.mockRejectedValueOnce({
      contractVersion: 1, code: "CONFLICT", message: "Another Spool already occupies that slot.", recovery: ["RETRY"], retryable: true,
      details: { slotId: "slt-a", currentOccupantSpoolId: "spl-racer" },
    });
    const { onOpenChange } = renderDialog([LOADED_A]);
    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /Another Printer's slot/);
    await pick(within(row(7)).getByRole("button", { name: /Destination slot/ }), /Bay 2 — A/);

    fireEvent.click(screen.getByRole("button", { name: "Archive" }));

    expect(await within(row(7)).findByRole("alert")).toHaveTextContent("Another Spool already occupies that slot.");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
    expect(within(row(7)).getByText("Swap: #6 TPU Red goes to storage")).toBeInTheDocument();

    archivePrinter.mockResolvedValueOnce(undefined);
    fireEvent.input(within(row(7)).getByLabelText(/Displaced Spool storage label/), { target: { value: "Bin" } });
    fireEvent.click(screen.getByRole("button", { name: "Archive" }));
    await waitFor(() => expect(archivePrinter).toHaveBeenLastCalledWith("prn-1", [
      {
        spoolId: "spl-a", expectedSpoolRevision: 3,
        disposition: { kind: "slot", slotId: "slt-a", expectedOccupantSpoolId: "spl-racer", displacedStorageLabel: "Bin" },
      },
    ]));
  });

  it("frees a slot for another row once the first row switches away from it, clearing its slot choice", async () => {
    seed();
    renderDialog();
    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /Another Printer's slot/);
    await pick(within(row(7)).getByRole("button", { name: /Destination slot/ }), /Bay 2 — A/);
    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /^Storage$/);

    await pick(within(row(8)).getByRole("button", { name: /Where #8 goes/ }), /Another Printer's slot/);
    await pick(within(row(8)).getByRole("button", { name: /Destination slot/ }), /Bay 2 — A/);
    expect(within(row(8)).getByRole("button", { name: /Destination slot/ })).toHaveTextContent("Bay 2 — A");

    // Switching row 7 back to a slot starts from no choice, not the old one.
    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /Another Printer's slot/);
    expect(within(row(7)).getByRole("button", { name: /Destination slot/ })).toHaveTextContent("Choose a slot");
  });

  it("asks for the displaced label even when the slot's occupant isn't in the loaded inventory", async () => {
    seed();
    (printersList[1] as { materialSlots: { occupantSpoolId?: string }[] }).materialSlots[0].occupantSpoolId = "spl-ghost";
    archivePrinter.mockResolvedValue(undefined);
    renderDialog([LOADED_A]);
    await pick(within(row(7)).getByRole("button", { name: /Where #7 goes/ }), /Another Printer's slot/);
    await pick(within(row(7)).getByRole("button", { name: /Destination slot/ }), /Bay 2 — A/);

    expect(within(row(7)).getByText("Swap: the Spool in that slot goes to storage")).toBeInTheDocument();
    const archive = screen.getByRole("button", { name: "Archive" });
    expect(archive).toBeDisabled();
    fireEvent.input(within(row(7)).getByLabelText(/Displaced Spool storage label/), { target: { value: "Bin" } });
    expect(archive).not.toBeDisabled();
  });

  it("shows a failure it can't pin to a row at the dialog level", async () => {
    seed();
    archivePrinter.mockRejectedValueOnce({
      contractVersion: 1, code: "LIFECYCLE_BLOCKED", message: "Spool #8 is reserved for a Job.", recovery: [], retryable: false,
    });
    renderDialog([LOADED_B]);
    await pick(within(row(8)).getByRole("button", { name: /Where #8 goes/ }), /Mark empty/);
    fireEvent.click(screen.getByRole("button", { name: "Archive" }));

    expect(await screen.findByRole("alert")).toHaveTextContent("Spool #8 is reserved for a Job.");
  });
});
