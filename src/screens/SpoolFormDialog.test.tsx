import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { SpoolFormDialog } from "./SpoolFormDialog";

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
});
