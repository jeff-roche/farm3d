import { createSignal } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterAddDialog } from "./PrinterAddDialog";
import type { ResolvedPrinter } from "../printers/types";

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

const CENTAURI_MODEL = { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" };

function existingPrinter(name: string): ResolvedPrinter {
  return {
    id: `prn-${name}`,
    revision: 1,
    name,
    notes: "",
    overrides: {},
    catalogRef: {
      vendor: "Elegoo", model: "Elegoo Centauri Carbon",
      variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "Elegoo Centauri Carbon",
    variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
    profile: {
      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256,
      bedExcludeAreas: [],
      defaultBedType: "4",
      nozzleDiameterMm: [0.4],
      nozzleType: "hardened_steel",
      gcodeFlavor: "klipper",
      hasAuxiliaryFan: true,
      supportsAirFiltration: true,
      supportsMultiFilament: true,
      suggestedHostType: "elegoolink",
    },
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    createdAt: "",
    updatedAt: "",
  };
}

describe("PrinterAddDialog", () => {
  it("picks a brand, then a model + auto-selected variant, and submits the draft", async () => {
    const onAdd = vi.fn().mockResolvedValue(existingPrinter("Elegoo Centauri Carbon"));
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

    await fireEvent.click(screen.getByRole("button", { name: "Connect →" }));

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

  it("hides the Model dropdown until a brand is chosen, and disables Connect until a model is selected", () => {
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={vi.fn()} />);
    expect(screen.queryByRole("button", { name: "Model" })).not.toBeInTheDocument();
    const addButton = screen.getByRole("button", { name: "Connect →" }) as HTMLButtonElement;
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
    await screen.findByText(/Nozzle: 0\.4 mm/);

    // Switching to 0.6mm must update the preview text, not leave it frozen
    // on the first-resolved (0.4mm) profile.
    const nozzleTrigger = await screen.findByRole("button", { name: /Nozzle/ });
    await fireEvent.pointerDown(nozzleTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("0.6 mm"));

    await screen.findByText(/Nozzle: 0\.6 mm/);
    expect(screen.queryByText(/Nozzle: 0\.4 mm/)).not.toBeInTheDocument();
  });

  it("prefills a group's model and suggests a unique name, avoiding an existing one", async () => {
    render(() => (
      <PrinterAddDialog
        open
        onOpenChange={() => {}}
        onAdd={vi.fn()}
        prefillModel={CENTAURI_MODEL}
        existingPrinters={[existingPrinter("Elegoo Centauri Carbon")]}
      />
    ));

    // Brand/Model are pre-seeded, not hidden -- confirms the prefill landed
    // rather than leaving the form on its blank default. The trigger's
    // accessible name is "Model" + its selected value concatenated via
    // aria-labelledby, hence the partial match rather than an exact one.
    expect(await screen.findByRole("button", { name: /Centauri Carbon/ })).toBeInTheDocument();
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe(
      "Elegoo Centauri Carbon 2",
    );
  });

  it("resets every signal after closing, even with no prefill on the next open", async () => {
    const [open, setOpen] = createSignal(true);
    render(() => <PrinterAddDialog open={open()} onOpenChange={setOpen} onAdd={vi.fn()} />);

    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Elegoo" } });
    await fireEvent.pointerUp(await screen.findByText("Elegoo"), { pointerType: "mouse", button: 0 });
    expect(await screen.findByRole("button", { name: "Model" })).toBeInTheDocument();

    await fireEvent.input(screen.getByLabelText("Name"), { target: { value: "half-typed name" } });

    setOpen(false);
    setOpen(true);

    expect(screen.queryByRole("button", { name: "Model" })).not.toBeInTheDocument();
    expect((screen.getByLabelText("Name") as HTMLInputElement).value).toBe("");
  });

  it("warns on a duplicate name without disabling Add", async () => {
    render(() => (
      <PrinterAddDialog
        open
        onOpenChange={() => {}}
        onAdd={vi.fn()}
        existingPrinters={[existingPrinter("Elegoo Centauri Carbon")]}
      />
    ));

    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Elegoo" } });
    await fireEvent.pointerUp(await screen.findByText("Elegoo"), { pointerType: "mouse", button: 0 });

    const modelTrigger = await screen.findByRole("button", { name: "Model" });
    await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("Centauri Carbon"));

    // The auto-filled name already collides with the existing printer.
    expect(await screen.findByText("Another printer is already named this")).toBeInTheDocument();
    const addButton = screen.getByRole("button", { name: "Connect →" }) as HTMLButtonElement;
    expect(addButton.disabled).toBe(false);
  });

  async function addACentauriCarbon(onAdd: ReturnType<typeof vi.fn>) {
    const brandInput = await screen.findByRole("combobox", { name: "Brand" });
    await fireEvent.pointerDown(brandInput, { pointerType: "mouse", button: 0 });
    await fireEvent.input(brandInput, { target: { value: "Elegoo" } });
    await fireEvent.pointerUp(await screen.findByText("Elegoo"), { pointerType: "mouse", button: 0 });

    const modelTrigger = await screen.findByRole("button", { name: "Model" });
    await fireEvent.pointerDown(modelTrigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("Centauri Carbon"));

    await fireEvent.click(screen.getByRole("button", { name: "Connect →" }));
    await vi.waitFor(() => expect(onAdd).toHaveBeenCalled());
  }

  it("moves to a Connection step for the newly created printer after a successful add", async () => {
    const onAdd = vi.fn().mockResolvedValue(existingPrinter("Elegoo Centauri Carbon"));
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={onAdd} />);

    await addACentauriCarbon(onAdd);

    expect(
      await screen.findByText(/Elegoo Centauri Carbon was added\. Set up its connection now/),
    ).toBeInTheDocument();
    // The details form is gone -- Brand is no longer on screen.
    expect(screen.queryByRole("combobox", { name: "Brand" })).not.toBeInTheDocument();
    // PrinterConnectionPanel itself is reused unchanged; its own suite
    // covers its behavior in full -- this only confirms it's the thing
    // that rendered.
    expect(await screen.findByRole("button", { name: /Kind/ })).toBeInTheDocument();
    expect(screen.getByLabelText("Host")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Connect →" })).not.toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Done" })).toBeInTheDocument();
  });

  it("stays on the details step, form intact, when the add fails", async () => {
    const onAdd = vi.fn().mockResolvedValue(undefined);
    render(() => <PrinterAddDialog open onOpenChange={() => {}} onAdd={onAdd} />);

    await addACentauriCarbon(onAdd);

    expect(screen.getByRole("combobox", { name: "Brand" })).toBeInTheDocument();
    expect(screen.getByRole("button", { name: "Connect →" })).toBeInTheDocument();
    expect(screen.queryByText(/was added\. Set up its connection now/)).not.toBeInTheDocument();
  });

  it("Done on the Connection step closes the dialog", async () => {
    const onAdd = vi.fn().mockResolvedValue(existingPrinter("Elegoo Centauri Carbon"));
    const onOpenChange = vi.fn();
    render(() => <PrinterAddDialog open onOpenChange={onOpenChange} onAdd={onAdd} />);

    await addACentauriCarbon(onAdd);
    await fireEvent.click(await screen.findByRole("button", { name: "Done" }));

    expect(onOpenChange).toHaveBeenCalledWith(false);
  });

  it("returns to the details step on the next open after a completed Connection step", async () => {
    const onAdd = vi.fn().mockResolvedValue(existingPrinter("Elegoo Centauri Carbon"));
    const [open, setOpen] = createSignal(true);
    render(() => <PrinterAddDialog open={open()} onOpenChange={setOpen} onAdd={onAdd} />);

    await addACentauriCarbon(onAdd);
    await screen.findByRole("button", { name: "Done" });

    setOpen(false);
    setOpen(true);

    expect(await screen.findByRole("combobox", { name: "Brand" })).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Done" })).not.toBeInTheDocument();
  });
});
