import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { MoveSpoolDialog } from "./MoveSpoolDialog";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";

const moveSpool = vi.fn();
const spools: SpoolRecord[] = [];

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools };
  },
  moveSpool: (...args: unknown[]) => moveSpool(...args),
}));

const printersList = [
  {
    id: "prn-1", name: "Centauri Carbon — Bay 1", archivedAt: undefined,
    materialSlots: [
      { id: "slt-1", position: 0, name: "Main" },
      { id: "slt-2", position: 1, name: "AMS 1", occupantSpoolId: "spl-occupant" },
    ],
  },
];
vi.mock("../printers/printer-store", () => ({
  printers: () => printersList,
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  spools.length = 0;
});

const MOVING_SPOOL: SpoolRecord = {
  id: "spl-move", revision: 2, spoolNumber: 9,
  manufacturer: "eSun", materialFamily: "ABS", colorName: "White", diameter: "1.75",
  nominalMg: 1_000_000, lowThresholdMg: 100_000,
  lifecycle: "active",
  location: { kind: "storage", storageLabel: null },
  availability: { currentMg: 500_000, reservedMg: 0, availableMg: 500_000 },
  facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
  createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z",
};

const OCCUPANT: SpoolRecord = {
  id: "spl-occupant", revision: 1, spoolNumber: 7,
  manufacturer: "Overture", materialFamily: "PETG", colorName: "Black", diameter: "1.75",
  nominalMg: 1_000_000, lowThresholdMg: 100_000,
  lifecycle: "active",
  location: { kind: "slot", slotId: "slt-2", printerId: "prn-1" },
  availability: { currentMg: 400_000, reservedMg: 0, availableMg: 400_000 },
  facets: { loaded: true, reserved: false, low: false, confidence: "measured" },
  createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z",
};

const RACER: SpoolRecord = {
  id: "spl-racer", revision: 1, spoolNumber: 3,
  manufacturer: "Polymaker", materialFamily: "PLA", colorName: "Charcoal", diameter: "1.75",
  nominalMg: 1_000_000, lowThresholdMg: 100_000,
  lifecycle: "active",
  location: { kind: "slot", slotId: "slt-1", printerId: "prn-1" },
  availability: { currentMg: 900_000, reservedMg: 0, availableMg: 900_000 },
  facets: { loaded: true, reserved: false, low: false, confidence: "measured" },
  createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z",
};

async function choosePrinterAndSlot(slotName: RegExp) {
  const printerTrigger = screen.getByRole("button", { name: /Printer/ });
  await fireEvent.pointerDown(printerTrigger, { button: 0, pointerType: "mouse" });
  const printerOption = await screen.findByRole("option", { name: /Centauri Carbon/ });
  await fireEvent.pointerDown(printerOption, { button: 0, pointerType: "mouse" });
  await fireEvent.pointerUp(printerOption, { button: 0, pointerType: "mouse" });

  const slotTrigger = screen.getByRole("button", { name: /^Slot/ });
  await fireEvent.pointerDown(slotTrigger, { button: 0, pointerType: "mouse" });
  const slotOption = await screen.findByRole("option", { name: slotName });
  await fireEvent.pointerDown(slotOption, { button: 0, pointerType: "mouse" });
  await fireEvent.pointerUp(slotOption, { button: 0, pointerType: "mouse" });
}

describe("MoveSpoolDialog", () => {
  it("shows the swap line and an optional label once an occupied slot is chosen", async () => {
    spools.push(OCCUPANT);
    render(() => <MoveSpoolDialog open onOpenChange={vi.fn()} spool={MOVING_SPOOL} />);

    await fireEvent.click(screen.getByRole("radio", { name: "Printer" }));
    await choosePrinterAndSlot(/AMS 1/);

    expect(screen.getByText("Swap: #7 PETG Black goes to storage")).toBeInTheDocument();
    const displacedLabel = screen.getByLabelText(/Displaced Spool storage label/);
    expect(displacedLabel).not.toBeRequired();
    // D6: the label is optional; blank means storage with no label.
    expect(screen.getByRole("button", { name: "Move" })).not.toBeDisabled();
  });

  it("sends a null displacedStorageLabel when the displaced label is left blank", async () => {
    spools.push(OCCUPANT);
    moveSpool.mockResolvedValue({ spools: [], printers: [], movements: [] });
    render(() => <MoveSpoolDialog open onOpenChange={vi.fn()} spool={MOVING_SPOOL} />);

    await fireEvent.click(screen.getByRole("radio", { name: "Printer" }));
    await choosePrinterAndSlot(/AMS 1/);
    await fireEvent.click(screen.getByRole("button", { name: "Move" }));

    expect(moveSpool).toHaveBeenCalledWith({
      spoolId: "spl-move",
      expectedSpoolRevision: 2,
      destination: {
        kind: "slot", slotId: "slt-2", expectedOccupantSpoolId: "spl-occupant", displacedStorageLabel: null,
      },
    });
  });

  it("submits moveSpool with expectedOccupantSpoolId on the occupied slot", async () => {
    spools.push(OCCUPANT);
    moveSpool.mockResolvedValue({ spools: [], printers: [], movements: [] });
    render(() => <MoveSpoolDialog open onOpenChange={vi.fn()} spool={MOVING_SPOOL} />);

    await fireEvent.click(screen.getByRole("radio", { name: "Printer" }));
    await choosePrinterAndSlot(/AMS 1/);
    await fireEvent.input(screen.getByLabelText(/Displaced Spool storage label/), { target: { value: "Shelf C1" } });
    await fireEvent.click(screen.getByRole("button", { name: "Move" }));

    expect(moveSpool).toHaveBeenCalledWith({
      spoolId: "spl-move",
      expectedSpoolRevision: 2,
      destination: {
        kind: "slot", slotId: "slt-2", expectedOccupantSpoolId: "spl-occupant", displacedStorageLabel: "Shelf C1",
      },
    });
  });

  it("stays open and shows the new occupant on a CONFLICT", async () => {
    spools.push(RACER);
    const conflictError = {
      contractVersion: 1, code: "CONFLICT", message: "Another Spool already occupies that slot.",
      recovery: ["RETRY"], retryable: true, details: { slotId: "slt-1", currentOccupantSpoolId: "spl-racer" },
    };
    moveSpool.mockRejectedValue(conflictError);
    const onOpenChange = vi.fn();
    render(() => <MoveSpoolDialog open onOpenChange={onOpenChange} spool={MOVING_SPOOL} />);

    await fireEvent.click(screen.getByRole("radio", { name: "Printer" }));
    await choosePrinterAndSlot(/^Main/);
    await fireEvent.click(screen.getByRole("button", { name: "Move" }));

    expect(await screen.findByText("Swap: #3 PLA Charcoal goes to storage")).toBeInTheDocument();
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("forgets a CONFLICT's occupant once a different slot is chosen", async () => {
    spools.push(RACER, OCCUPANT);
    moveSpool.mockRejectedValueOnce({
      contractVersion: 1, code: "CONFLICT", message: "Another Spool already occupies that slot.",
      recovery: ["RETRY"], retryable: true, details: { slotId: "slt-1", currentOccupantSpoolId: "spl-racer" },
    });
    moveSpool.mockResolvedValueOnce({ spools: [], printers: [], movements: [] });
    render(() => <MoveSpoolDialog open onOpenChange={vi.fn()} spool={MOVING_SPOOL} />);

    await fireEvent.click(screen.getByRole("radio", { name: "Printer" }));
    await choosePrinterAndSlot(/^Main/);
    await fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(await screen.findByText("Swap: #3 PLA Charcoal goes to storage")).toBeInTheDocument();

    const slotTrigger = screen.getByRole("button", { name: /^Slot/ });
    await fireEvent.pointerDown(slotTrigger, { button: 0, pointerType: "mouse" });
    const amsOption = await screen.findByRole("option", { name: /AMS 1/ });
    await fireEvent.pointerDown(amsOption, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(amsOption, { button: 0, pointerType: "mouse" });

    expect(screen.queryByText("Swap: #3 PLA Charcoal goes to storage")).not.toBeInTheDocument();
    expect(screen.getByText("Swap: #7 PETG Black goes to storage")).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Move" }));
    expect(moveSpool).toHaveBeenLastCalledWith(expect.objectContaining({
      destination: expect.objectContaining({ slotId: "slt-2", expectedOccupantSpoolId: "spl-occupant" }),
    }));
  });
});
