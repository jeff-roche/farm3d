import { createSignal, Show } from "solid-js";
import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterSetupPanel } from "./PrinterSetupPanel";

const updatePrinter = vi.fn();
// PrinterSetupWizard (imported for START_SAFETY_OPTIONS) now pulls in the
// Spool store through its Equip step.
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools: [], loaded: true };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));

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
  profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", materialSlots: [{ id: "slt-main", position: 0, name: "Main" }], setupGaps: [], createdAt: "", updatedAt: "",
  profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 0, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
};

describe("PrinterSetupPanel", () => {
  afterEach(() => {
    cleanup();
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

  it("persists a renamed Printer after the debounce", async () => {
    vi.useFakeTimers();
    render(() => <PrinterSetupPanel printer={printer} />);

    await fireEvent.input(screen.getByLabelText("Name"), { target: { value: "South Bay" } });
    vi.advanceTimersByTime(300);

    expect(updatePrinter).toHaveBeenCalledWith("prn-1", { name: "South Bay" });
  });

  it("continues to persist notes after the debounce", async () => {
    vi.useFakeTimers();
    render(() => <PrinterSetupPanel printer={printer} />);

    await fireEvent.input(screen.getByLabelText("Notes"), { target: { value: "spare nozzle" } });
    vi.advanceTimersByTime(300);

    expect(updatePrinter).toHaveBeenCalledWith("prn-1", { notes: "spare nozzle" });
  });

  it("cancels pending name and notes edits when the selected Printer changes", async () => {
    vi.useFakeTimers();
    const [selected, setSelected] = createSignal(printer);
    render(() => <Show when={selected()}>{(current) => <PrinterSetupPanel printer={current()} />}</Show>);

    await fireEvent.input(screen.getByLabelText("Name"), { target: { value: "South Bay" } });
    await fireEvent.input(screen.getByLabelText("Notes"), { target: { value: "for printer A" } });
    setSelected({ ...printer, id: "prn-2", name: "Printer B", notes: "Printer B note" });
    vi.advanceTimersByTime(300);

    expect(updatePrinter).not.toHaveBeenCalled();
  });

  it("persists a Location edit after the debounce, like name and notes", async () => {
    vi.useFakeTimers();
    render(() => <PrinterSetupPanel printer={printer} />);

    await fireEvent.input(screen.getByLabelText("Location"), { target: { value: "Bay 2" } });
    vi.advanceTimersByTime(300);

    expect(updatePrinter).toHaveBeenCalledWith("prn-1", { location: "Bay 2" });
  });

  it("clears Location to null, not an empty string, after the debounce", async () => {
    vi.useFakeTimers();
    render(() => <PrinterSetupPanel printer={{ ...printer, location: "Bay 2" }} />);

    await fireEvent.input(screen.getByLabelText("Location"), { target: { value: "" } });
    vi.advanceTimersByTime(300);

    expect(updatePrinter).toHaveBeenCalledWith("prn-1", { location: null });
  });

  it("cancels a pending Location edit when the selected Printer changes", async () => {
    vi.useFakeTimers();
    const [selected, setSelected] = createSignal(printer);
    render(() => <Show when={selected()}>{(current) => <PrinterSetupPanel printer={current()} />}</Show>);

    await fireEvent.input(screen.getByLabelText("Location"), { target: { value: "Bay 2" } });
    setSelected({ ...printer, id: "prn-2" });
    vi.advanceTimersByTime(300);

    expect(updatePrinter).not.toHaveBeenCalled();
  });

  it("saves the start-safety RadioGroup immediately, without a debounce", async () => {
    render(() => <PrinterSetupPanel printer={printer} />);

    await fireEvent.click(screen.getByRole("radio", { name: "Allow unattended starts" }));

    expect(updatePrinter).toHaveBeenCalledWith("prn-1", { startSafety: "unattended" });
  });
});
