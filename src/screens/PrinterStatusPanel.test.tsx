import { createSignal, Show } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { PrinterStatusPanel } from "./PrinterStatusPanel";
import type { ResolvedPrinter } from "../printers/types";

const updatePrinter = vi.fn();
const rebindPrinter = vi.fn();
vi.mock("../printers/printer-store", () => ({
  updatePrinter: (...args: unknown[]) => updatePrinter(...args),
  rebindPrinter: (...args: unknown[]) => rebindPrinter(...args),
}));

const listCatalogVariants = vi.fn();
vi.mock("../printers/printer-catalog", () => ({
  listCatalogVariants: (...args: unknown[]) => listCatalogVariants(...args),
}));

afterEach(() => {
  document.body.innerHTML = "";
  vi.clearAllMocks();
  vi.useRealTimers();
});

const PRINTER: ResolvedPrinter = {
  id: "prn-1",
  name: "Centauri Carbon — Bay 1",
  group: "",
  notes: "",
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
  connection: null,
};

describe("PrinterStatusPanel", () => {
  it("renders the catalog model and variant", () => {
    listCatalogVariants.mockResolvedValue([]);
    render(() => <PrinterStatusPanel printer={PRINTER} />);
    expect(
      screen.getByText("Elegoo Centauri Carbon — Elegoo Centauri Carbon 0.4 nozzle"),
    ).toBeInTheDocument();
  });

  it("debounces a notes edit before calling updatePrinter", async () => {
    listCatalogVariants.mockResolvedValue([]);
    vi.useFakeTimers();
    render(() => <PrinterStatusPanel printer={PRINTER} />);

    const notes = screen.getByLabelText("Notes") as HTMLInputElement;
    await fireEvent.input(notes, { target: { value: "spare hotend on shelf" } });
    expect(updatePrinter).not.toHaveBeenCalled();

    vi.advanceTimersByTime(300);
    expect(updatePrinter).toHaveBeenCalledWith("prn-1", { notes: "spare hotend on shelf" });
  });

  it("cancels a pending notes edit when the printer switches under a non-keyed Show", async () => {
    // Mirrors PrinterDashboard.tsx's actual non-keyed
    // `<Show when={selected()}>{(printer) => (...)}</Show>` pattern, and
    // PrinterProfilePanel's identical regression test for the same bug
    // class: a truthy->truthy change of `selected()` does not remount the
    // child, so an in-flight debounced write must be cancelled on identity
    // change rather than committed against the wrong printer.
    listCatalogVariants.mockResolvedValue([]);
    vi.useFakeTimers();
    const printerB: ResolvedPrinter = { ...PRINTER, id: "prn-2", notes: "printer B's own notes" };
    const [selected, setSelected] = createSignal<ResolvedPrinter>(PRINTER);

    render(() => (
      <Show when={selected()}>{(printer) => <PrinterStatusPanel printer={printer()} />}</Show>
    ));

    const notes = screen.getByLabelText("Notes") as HTMLInputElement;
    await fireEvent.input(notes, { target: { value: "a note meant for printer A" } });
    expect(updatePrinter).not.toHaveBeenCalled();

    setSelected(printerB);
    vi.advanceTimersByTime(300);

    expect(updatePrinter).not.toHaveBeenCalledWith("prn-2", { notes: "a note meant for printer A" });
    expect(updatePrinter).not.toHaveBeenCalledWith("prn-1", { notes: "a note meant for printer A" });
  });

  it("offers no rebind control when the model has only one variant", async () => {
    listCatalogVariants.mockResolvedValue([
      { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
    ]);
    render(() => <PrinterStatusPanel printer={PRINTER} />);
    expect(await screen.findByLabelText("Notes")).toBeInTheDocument();
    expect(screen.queryByText("Nozzle / variant")).not.toBeInTheDocument();
  });

  it("rebinds to a sibling variant instead of overriding it", async () => {
    listCatalogVariants.mockResolvedValue([
      { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
      { variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6" },
    ]);
    render(() => <PrinterStatusPanel printer={PRINTER} />);

    const trigger = await screen.findByRole("button", { name: /0\.4 mm/ });
    await fireEvent.pointerDown(trigger, { pointerType: "mouse", button: 0 });
    await fireEvent.click(await screen.findByText("0.6 mm"));

    expect(rebindPrinter).toHaveBeenCalledWith("prn-1", {
      vendor: "Elegoo",
      model: "Elegoo Centauri Carbon",
      modelId: "Elegoo-CC",
      variant: "Elegoo Centauri Carbon 0.6 nozzle",
      printerVariant: "0.6",
    });
  });
});
