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

  it("shows a VALIDATION rejection inline under the gross field and stays open, even though the client precheck passed", async () => {
    // Gross (900) is well above the client-known Cardboard tare (200 g), so
    // the client-side precheck passes -- this rejection can only have come
    // from an actual submit to Rust (fix round 1 ruling item 2).
    recordAmount.mockRejectedValue({
      contractVersion: 1, code: "VALIDATION", message: "The gross weight is less than the tare.",
      recovery: [], retryable: false, details: { fieldPath: "entry.grossMg" },
    });
    const onOpenChange = vi.fn();
    render(() => (
      <RecordAmountDialog open onOpenChange={onOpenChange} spool={SPOOL} />
    ));

    await fireEvent.click(screen.getByRole("radio", { name: "Scale" }));
    const gross = screen.getByLabelText("Gross weight (g)") as HTMLInputElement;
    await fireEvent.input(gross, { target: { value: "900" } });
    await fireEvent.click(screen.getByRole("button", { name: "Record" }));

    expect(await screen.findByText("The gross weight is less than the tare.")).toBeInTheDocument();
    expect(gross).toHaveAttribute("aria-invalid", "true");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("lets the user correct a rejected gross weight and resubmit (fix round 2)", async () => {
    // Same setup as the VALIDATION test above: the first submit is rejected
    // by Rust even though the client precheck passed.
    recordAmount.mockRejectedValueOnce({
      contractVersion: 1, code: "VALIDATION", message: "The gross weight is less than the tare.",
      recovery: [], retryable: false, details: { fieldPath: "entry.grossMg" },
    });
    recordAmount.mockResolvedValueOnce({ ...SPOOL, availability: { currentMg: 950_000, reservedMg: 0, availableMg: 950_000 } });
    const onOpenChange = vi.fn();
    render(() => (
      <RecordAmountDialog open onOpenChange={onOpenChange} spool={SPOOL} />
    ));

    await fireEvent.click(screen.getByRole("radio", { name: "Scale" }));
    const gross = screen.getByLabelText("Gross weight (g)") as HTMLInputElement;
    await fireEvent.input(gross, { target: { value: "900" } });
    const recordButton = screen.getByRole("button", { name: "Record" });
    await fireEvent.click(recordButton);

    expect(await screen.findByText("The gross weight is less than the tare.")).toBeInTheDocument();

    // Editing gross clears the server error and re-enables the button --
    // without fix round 2, `grossError()`/`canSubmit` would stay stuck on
    // the stale server rejection forever.
    await fireEvent.input(gross, { target: { value: "950" } });
    expect(screen.queryByText("The gross weight is less than the tare.")).not.toBeInTheDocument();
    expect(recordButton).not.toBeDisabled();

    await fireEvent.click(recordButton);

    expect(recordAmount).toHaveBeenCalledTimes(2);
    expect(recordAmount).toHaveBeenNthCalledWith(
      2,
      "spl-1",
      { kind: "scale", grossMg: 950_000 },
      undefined,
    );
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("shows a dialog-level message and stays open on a CONFLICT", async () => {
    recordAmount.mockRejectedValue({
      contractVersion: 1, code: "CONFLICT", message: "Someone else changed this Spool.",
      recovery: ["RETRY"], retryable: true,
    });
    const onOpenChange = vi.fn();
    render(() => (
      <RecordAmountDialog open onOpenChange={onOpenChange} spool={SPOOL} />
    ));

    const net = screen.getByLabelText("Net weight (g)") as HTMLInputElement;
    await fireEvent.input(net, { target: { value: "612" } });
    await fireEvent.click(screen.getByRole("button", { name: "Record" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/reloaded/i);
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});
