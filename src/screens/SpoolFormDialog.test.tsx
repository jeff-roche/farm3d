import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SpoolFormDialog } from "./SpoolFormDialog";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";

const createSpool = vi.fn();
const updateSpool = vi.fn();

vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { tares: [] };
  },
  createSpool: (...args: unknown[]) => createSpool(...args),
  updateSpool: (...args: unknown[]) => updateSpool(...args),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
});

async function fillRequiredFields() {
  await fireEvent.input(screen.getByLabelText("Manufacturer"), { target: { value: "Prusament" } });
  await fireEvent.input(screen.getByLabelText("Color name"), { target: { value: "Black" } });
}

describe("SpoolFormDialog", () => {
  it("reveals and requires the material text when OTHER is chosen", async () => {
    render(() => <SpoolFormDialog open onOpenChange={vi.fn()} />);
    expect(screen.queryByLabelText("Material (other)")).not.toBeInTheDocument();

    const materialTrigger = screen.getByRole("button", { name: /Material/ });
    await fireEvent.pointerDown(materialTrigger, { button: 0, pointerType: "mouse" });
    const otherOption = await screen.findByRole("option", { name: "Other" });
    await fireEvent.pointerDown(otherOption, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(otherOption, { button: 0, pointerType: "mouse" });

    expect(screen.getByLabelText("Material (other)")).toBeInTheDocument();

    await fillRequiredFields();
    expect(screen.getByRole("button", { name: "Add Spool" })).toBeDisabled();

    await fireEvent.input(screen.getByLabelText("Material (other)"), { target: { value: "Wood-fill" } });
    expect(screen.getByRole("button", { name: "Add Spool" })).not.toBeDisabled();
  });

  it("fills 1 kg from the nominal quick pick", async () => {
    render(() => <SpoolFormDialog open onOpenChange={vi.fn()} />);
    await fireEvent.click(screen.getByText("1 kg"));
    expect((screen.getByLabelText("Nominal weight (g)") as HTMLInputElement).value).toBe("1,000");
  });

  it("switches to measured or scale entry once 'I weighed it' is chosen", async () => {
    render(() => <SpoolFormDialog open onOpenChange={vi.fn()} />);
    expect(screen.queryByLabelText("Measured net weight (g)")).not.toBeInTheDocument();

    await fireEvent.click(screen.getByText("I weighed it"));
    expect(screen.getByLabelText("Measured net weight (g)")).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("radio", { name: "Scale" }));
    expect(screen.getByLabelText("Gross weight (g)")).toBeInTheDocument();
  });

  it("calls createSpool with mg values on submit", async () => {
    createSpool.mockResolvedValue({ id: "spl-new" });
    const onOpenChange = vi.fn();
    render(() => <SpoolFormDialog open onOpenChange={onOpenChange} />);

    await fillRequiredFields();
    await fireEvent.click(screen.getByRole("button", { name: "Add Spool" }));

    expect(createSpool).toHaveBeenCalledWith(
      expect.objectContaining({
        manufacturer: "Prusament",
        colorName: "Black",
        materialFamily: "PLA",
        diameter: "1.75",
        nominalMg: 1_000_000,
        lowThresholdMg: 100_000,
      }),
      { kind: "net", netMg: 1_000_000, confidence: "estimated" },
      undefined,
    );
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("shows a VALIDATION rejection inline under the Manufacturer field and stays open", async () => {
    createSpool.mockRejectedValue({
      contractVersion: 1, code: "VALIDATION", message: "That manufacturer name is too long.",
      recovery: ["EDIT_FIELDS"], retryable: false, details: { fieldPath: "manufacturer" },
    });
    const onOpenChange = vi.fn();
    render(() => <SpoolFormDialog open onOpenChange={onOpenChange} />);

    await fillRequiredFields();
    await fireEvent.click(screen.getByRole("button", { name: "Add Spool" }));

    expect(await screen.findByText("That manufacturer name is too long.")).toBeInTheDocument();
    expect(screen.getByLabelText("Manufacturer")).toHaveAttribute("aria-invalid", "true");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("shows a VALIDATION rejection inline under the Notes field and stays open, with the cap always spelled out", async () => {
    // The real desktop backend rejects an over-length `notes` with the same
    // generic message `RepositoryError::Validation` maps to for every field
    // (`contracts/command.rs`'s `validation_at`) -- not a notes-specific
    // one. The dialog must show the friendly cap text regardless.
    createSpool.mockRejectedValue({
      contractVersion: 1, code: "VALIDATION", message: "The submitted value is invalid.",
      recovery: ["EDIT_FIELDS"], retryable: false, details: { fieldPath: "notes" },
    });
    const onOpenChange = vi.fn();
    render(() => <SpoolFormDialog open onOpenChange={onOpenChange} />);

    await fillRequiredFields();
    await fireEvent.click(screen.getByRole("button", { name: "Add Spool" }));

    expect(await screen.findByText("Notes must be at most 2000 characters.")).toBeInTheDocument();
    expect(screen.queryByText("The submitted value is invalid.")).not.toBeInTheDocument();
    expect(screen.getByLabelText("Notes (optional)")).toHaveAttribute("aria-invalid", "true");
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("shows a VALIDATION rejection on an unknown default tareId under the tare field and stays open", async () => {
    // Matches the Notes field's mapping: the desktop `tareId` `VALIDATION`
    // also carries the same generic `RepositoryError::Validation` message,
    // not a tare-specific one, so the friendly text is spelled out here.
    createSpool.mockRejectedValue({
      contractVersion: 1, code: "VALIDATION", message: "The submitted value is invalid.",
      recovery: ["EDIT_FIELDS"], retryable: false, details: { fieldPath: "tareId" },
    });
    const onOpenChange = vi.fn();
    render(() => <SpoolFormDialog open onOpenChange={onOpenChange} />);

    await fillRequiredFields();
    await fireEvent.click(screen.getByRole("button", { name: "Add Spool" }));

    const message = await screen.findByText("That tare no longer exists.");
    expect(screen.queryByText("The submitted value is invalid.")).not.toBeInTheDocument();
    expect(screen.queryByRole("alert")).not.toBeInTheDocument();

    // Associated with the tare Select, not just shown somewhere in the
    // dialog -- Kobalte's Select links its trigger to its ErrorMessage via
    // `aria-describedby`, the same mechanism TextField/NumberField use.
    const tareTrigger = screen.getByRole("button", { name: /Default tare/ });
    expect(tareTrigger.getAttribute("aria-describedby")).toContain(message.id);

    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });

  it("lets the user correct a rejected gross weight and resubmit (fix round 2)", async () => {
    createSpool.mockRejectedValueOnce({
      contractVersion: 1, code: "VALIDATION", message: "The gross weight is less than the tare.",
      recovery: [], retryable: false, details: { fieldPath: "entry.grossMg" },
    });
    createSpool.mockResolvedValueOnce({ id: "spl-new" });
    const onOpenChange = vi.fn();
    render(() => <SpoolFormDialog open onOpenChange={onOpenChange} />);

    await fillRequiredFields();
    await fireEvent.click(screen.getByText("I weighed it"));
    await fireEvent.click(screen.getByRole("radio", { name: "Scale" }));
    const gross = screen.getByLabelText("Gross weight (g)") as HTMLInputElement;
    await fireEvent.input(gross, { target: { value: "900" } });

    const addButton = screen.getByRole("button", { name: "Add Spool" });
    await fireEvent.click(addButton);

    expect(await screen.findByText("The gross weight is less than the tare.")).toBeInTheDocument();

    // Editing gross clears the server error and re-enables the button --
    // without fix round 2, `grossError()`/`amountValid` would stay stuck on
    // the stale server rejection forever.
    await fireEvent.input(gross, { target: { value: "950" } });
    expect(screen.queryByText("The gross weight is less than the tare.")).not.toBeInTheDocument();
    expect(addButton).not.toBeDisabled();

    await fireEvent.click(addButton);

    expect(createSpool).toHaveBeenCalledTimes(2);
    expect(createSpool).toHaveBeenNthCalledWith(
      2,
      expect.objectContaining({ manufacturer: "Prusament", colorName: "Black" }),
      { kind: "scale", grossMg: 950_000, tareMg: 0 },
      undefined,
    );
    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("shows a dialog-level message and stays open on an Edit CONFLICT", async () => {
    const spool: SpoolRecord = {
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
    updateSpool.mockRejectedValue({
      contractVersion: 1, code: "CONFLICT", message: "Someone else changed this Spool.",
      recovery: ["RETRY"], retryable: true,
    });
    const onOpenChange = vi.fn();
    render(() => <SpoolFormDialog open onOpenChange={onOpenChange} spool={spool} />);

    await fireEvent.click(screen.getByRole("button", { name: "Save" }));

    expect(await screen.findByRole("alert")).toHaveTextContent(/reloaded/i);
    expect(onOpenChange).not.toHaveBeenCalledWith(false);
  });
});
