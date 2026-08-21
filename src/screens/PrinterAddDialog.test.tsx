import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterAddDialog } from "./PrinterAddDialog";

const previewProfile = vi.hoisted(() =>
  vi.fn().mockImplementation((ref: { printerVariant: string }) => {
    const nozzle = ref.printerVariant === "0.6" ? 0.6 : 0.4;
    return Promise.resolve({
      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256,
      nozzleDiameterMm: [nozzle],
      bedExcludeAreas: [],
      defaultBedType: "4",
      nozzleType: "hardened_steel",
      gcodeFlavor: "klipper",
      hasAuxiliaryFan: true,
      supportsAirFiltration: true,
      supportsMultiFilament: true,
      suggestedHostType: "elegoolink",
    });
  }),
);

vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([
    { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    { modelId: "Prusa-MK4", vendor: "Prusa", model: "Prusa MK4" },
  ]),
  listCatalogVariants: vi.fn().mockResolvedValue([
    { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
    { variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6" },
  ]),
  previewProfile,
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
    // The list shows "Centauri Carbon", not "Elegoo Centauri Carbon" — the
    // Brand dropdown already said "Elegoo", so the shared prefix is stripped
    // from the option label (see the label-stripping test below).
    const modelTrigger = await screen.findByRole("button", { name: "Model" });
    await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("Centauri Carbon"));

    // The Name field still auto-fills from the model's full (unstripped)
    // name — only the dropdown's own label is abbreviated.
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

  it("strips a model's own vendor prefix in the Model dropdown, but not when the model doesn't start with it", async () => {
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={vi.fn()} />);

    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Prusa" } });
    await fireEvent.pointerUp(await screen.findByText("Prusa"), { pointerType: "mouse", button: 0 });

    const modelTrigger = await screen.findByRole("button", { name: "Model" });
    await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
    // "Prusa MK4" strips to "MK4" since it starts with the vendor "Prusa".
    expect(await screen.findByText("MK4")).toBeInTheDocument();
    expect(screen.queryByText("Prusa MK4")).not.toBeInTheDocument();
  });

  it("regenerates the profile preview when the nozzle selection changes, not just on model selection", async () => {
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={vi.fn()} />);

    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Elegoo" } });
    await fireEvent.pointerUp(await screen.findByText("Elegoo"), { pointerType: "mouse", button: 0 });

    const modelTrigger = await screen.findByRole("button", { name: "Model" });
    await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("Centauri Carbon"));

    // Auto-selects the 0.4mm nozzle by default.
    await screen.findByText(/0\.4 mm nozzle/);

    // Switching to 0.6mm must update the preview text, not leave it frozen
    // on the first-resolved (0.4mm) profile.
    const nozzleTrigger = await screen.findByRole("button", { name: /Nozzle/ });
    await fireEvent.pointerDown(nozzleTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("0.6 mm"));

    await screen.findByText(/0\.6 mm nozzle/);
    expect(screen.queryByText(/0\.4 mm nozzle/)).not.toBeInTheDocument();
  });
});
