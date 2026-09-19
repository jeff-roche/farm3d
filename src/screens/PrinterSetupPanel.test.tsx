import { createSignal, Show } from "solid-js";
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterSetupPanel } from "./PrinterSetupPanel";

const updatePrinter = vi.fn();
vi.mock("../printers/printer-store", () => ({
  updatePrinter: (...args: unknown[]) => updatePrinter(...args),
  rebindPrinter: vi.fn(),
}));
vi.mock("../printers/printer-catalog", () => ({
  listCatalogVariants: vi.fn().mockResolvedValue([]),
}));

const printer: ResolvedPrinter = {
  id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
  catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
  catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
  profileDrift: [], unknownOverrideKeys: [], createdAt: "", updatedAt: "",
  profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 0, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
};

describe("PrinterSetupPanel", () => {
  afterEach(() => {
    vi.clearAllMocks();
    vi.useRealTimers();
  });

  it("cancels a pending notes edit when the selected Printer changes", async () => {
    vi.useFakeTimers();
    const [selected, setSelected] = createSignal(printer);
    render(() => <Show when={selected()}>{(current) => <PrinterSetupPanel printer={current()} />}</Show>);

    await fireEvent.input(screen.getByLabelText("Notes"), { target: { value: "for printer A" } });
    setSelected({ ...printer, id: "prn-2", notes: "Printer B note" });
    vi.advanceTimersByTime(300);

    expect(updatePrinter).not.toHaveBeenCalled();
  });
});
