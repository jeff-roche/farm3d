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
  it("picks a brand, then a model + auto-selected variant, and submits the draft", async () => {
    const onAdd = vi.fn();
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={onAdd} />);

    // Brand: a Combobox over the distinct vendor list. getByRole("combobox",
    // ...) targets the <input> unambiguously (same technique as the old
    // single-field flow — see Task 11's findings on Kobalte's
    // aria-labelledby-driven accessible-name collisions).
    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Elegoo" } });

    const brandItem = await screen.findByText("Elegoo");
    // pointerType: "mouse" is required — Kobalte's selectable-item handler
    // only selects on pointerup when pointerType is "mouse" and button is 0.
    await fireEvent.pointerUp(brandItem, { pointerType: "mouse", button: 0 });

    // Model: a plain Select, filtered to the chosen brand. Follows this
    // repo's established Select test pattern (pointerdown to open, click to
    // select — Select's listbox item responds to click, unlike Combobox's).
    const modelTrigger = await screen.findByRole("button", { name: "Model" });
    await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("Elegoo Centauri Carbon"));

    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe(
      "Elegoo Centauri Carbon",
    );

    // getByText("Add printer") is ambiguous here too — the Dialog's own
    // title renders as an <h2>Add printer</h2>, colliding with the submit
    // button's identical label. getByRole("button", ...) disambiguates the
    // same way the queries above do.
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

  it("hides the Model dropdown until a brand is chosen, and disables Add printer until a model is selected", () => {
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Model" })).not.toBeInTheDocument();
    // See the analogous getByRole note in the test above — the Dialog's own
    // title also reads "Add printer", so getByText would be ambiguous.
    const addButton = screen.getByRole("button", { name: "Add printer" }) as HTMLButtonElement;
    expect(addButton.disabled).toBe(true);
  });
});
