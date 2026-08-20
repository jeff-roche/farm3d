import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterAddDialog } from "./PrinterAddDialog";

vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    { modelId: "Prusa-MK4", vendor: "Prusa", model: "Prusa MK4" },
  ]),
  listCatalogVariants: vi.fn().mockResolvedValue([
    { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
  ]),
  previewProfile: vi.fn().mockResolvedValue({
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256,
    nozzleDiameterMm: [0.4],
    bedExcludeAreas: [],
    defaultBedType: "4",
    nozzleType: "hardened_steel",
    gcodeFlavor: "klipper",
    hasAuxiliaryFan: true,
    supportsAirFiltration: true,
    supportsMultiFilament: true,
    suggestedHostType: "elegoolink",
  }),
}));

afterEach(() => {
  document.body.innerHTML = "";
});

describe("PrinterAddDialog", () => {
  it("searches, selects a model + auto-selected variant, and submits the draft", async () => {
    const onAdd = vi.fn();
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={onAdd} />);

    // findByLabelText is ambiguous here — Kobalte's Combobox trigger button
    // also carries "Printer model" in its computed accessible name via
    // aria-labelledby, so it collides with the input under
    // @testing-library/dom's (non-recursive) label-matching heuristic.
    // getByRole("combobox", ...) is unambiguous: that role belongs only to
    // the <input> (verified against Kobalte's combobox source during Task 11).
    const input = await screen.findByRole("combobox", { name: "Printer model" });
    await fireEvent.pointerDown(input, { pointerType: "mouse", button: 0 });
    await fireEvent.input(input, { target: { value: "Centauri" } });

    const item = await screen.findByText("Elegoo · Elegoo Centauri Carbon");
    // pointerType: "mouse" is required — Kobalte's selectable-item handler
    // only selects on pointerup when pointerType is "mouse" and button is 0
    // (verified against createSelectableItem during Task 11; omitting this
    // makes the event a no-op and onChange never fires).
    await fireEvent.pointerUp(item, { pointerType: "mouse", button: 0 });

    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe(
      "Elegoo Centauri Carbon",
    );

    // getByText("Add printer") is ambiguous here too — the Dialog's own
    // title renders as an <h2>Add printer</h2>, colliding with the submit
    // button's identical label. getByRole("button", ...) disambiguates the
    // same way the combobox query above does.
    await fireEvent.click(screen.getByRole("button", { name: "Add printer" }));

    expect(onAdd).toHaveBeenCalledWith({
      name: "Elegoo Centauri Carbon",
      catalogRef: {
        vendor: "Elegoo",
        model: "Elegoo Centauri Carbon",
        variant: "Elegoo Centauri Carbon 0.4 nozzle",
        modelId: "Elegoo-CC",
        printerVariant: "0.4",
      },
    });
  });

  it("disables Add printer until a model is selected", () => {
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={vi.fn()} />);
    // See the analogous getByRole note in the test above — the Dialog's own
    // title also reads "Add printer", so getByText would be ambiguous.
    const addButton = screen.getByRole("button", { name: "Add printer" }) as HTMLButtonElement;
    expect(addButton.disabled).toBe(true);
  });
});
