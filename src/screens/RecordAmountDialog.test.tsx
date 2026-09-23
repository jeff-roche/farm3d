import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { RecordAmountDialog } from "./RecordAmountDialog";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { Tare } from "../generated/contracts/domain/Tare";

const recordAmount = vi.fn();
const tares: Tare[] = [
  { id: "tar-cardboard", revision: 1, name: "Cardboard", weightMg: 200_000, createdAt: "", updatedAt: "" },
];

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { tares };
  },
  recordAmount: (...args: unknown[]) => recordAmount(...args),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

const SPOOL: SpoolRecord = {
  id: "spl-1", revision: 3, spoolNumber: 7,
  manufacturer: "Prusament", product: "PLA", materialFamily: "PLA",
  colorName: "Black", diameter: "1.75",
  nominalMg: 1_000_000, lowThresholdMg: 100_000,
  lifecycle: "active",
  location: { kind: "storage", storageLabel: null },
  availability: { currentMg: 500_000, reservedMg: 0, availableMg: 500_000 },
  facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
  createdAt: "2026-09-01T00:00:00Z", updatedAt: "2026-09-01T00:00:00Z",
};

describe("RecordAmountDialog", () => {
  it("shows a live net preview in Scale mode with the Cardboard tare", async () => {
    render(() => (
      <RecordAmountDialog open onOpenChange={vi.fn()} spool={SPOOL} />
    ));

    await fireEvent.click(screen.getByRole("radio", { name: "Scale" }));

    const tareSelect = screen.getByRole("button", { name: /Tare/ });
    await fireEvent.pointerDown(tareSelect);
    const cardboardOption = await screen.findByRole("option", { name: /Cardboard/ });
    await fireEvent.pointerDown(cardboardOption, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(cardboardOption, { button: 0, pointerType: "mouse" });

    const gross = screen.getByLabelText("Gross weight (g)") as HTMLInputElement;
    await fireEvent.input(gross, { target: { value: "812" } });

    expect(await screen.findByText("Net: 612.0 g")).toBeInTheDocument();
  });

  it("shows the inline error and disables Record when gross is below the tare", async () => {
    render(() => (
      <RecordAmountDialog open onOpenChange={vi.fn()} spool={SPOOL} />
    ));

    await fireEvent.click(screen.getByRole("radio", { name: "Scale" }));
    const tareSelect = screen.getByRole("button", { name: /Tare/ });
    await fireEvent.pointerDown(tareSelect);
    const cardboardOption = await screen.findByRole("option", { name: /Cardboard/ });
    await fireEvent.pointerDown(cardboardOption, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(cardboardOption, { button: 0, pointerType: "mouse" });

    const gross = screen.getByLabelText("Gross weight (g)") as HTMLInputElement;
    await fireEvent.input(gross, { target: { value: "50" } });

    expect(await screen.findByText("The gross weight is less than the tare.")).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Record" })).toBeDisabled();
  });

  it("calls recordAmount with mg values in Net mode and closes on success", async () => {
    recordAmount.mockResolvedValue({ ...SPOOL, availability: { currentMg: 612_000, reservedMg: 0, availableMg: 612_000 } });
    const onOpenChange = vi.fn();
    render(() => (
      <RecordAmountDialog open onOpenChange={onOpenChange} spool={SPOOL} />
    ));

    const net = screen.getByLabelText("Net weight (g)") as HTMLInputElement;
    await fireEvent.input(net, { target: { value: "612" } });
    await fireEvent.click(screen.getByRole("button", { name: "Record" }));

    expect(recordAmount).toHaveBeenCalledWith(
      "spl-1",
      { kind: "net", netMg: 612_000, confidence: "measured" },
      undefined,
    );
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });
});
