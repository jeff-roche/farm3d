import { Show } from "solid-js";
import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMonitorStore } from "../monitor/monitor-store";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterDashboard } from "./PrinterDashboard";

vi.mock("../printers/printer-catalog", () => ({
  listCatalogModels: vi.fn().mockResolvedValue([]),
  listCatalogVariants: vi.fn().mockResolvedValue([]),
  previewProfile: vi.fn().mockResolvedValue(null),
}));

// Stands in for the batch dialog's Results-step Equip, which is covered by
// PrinterBatchDialog.test.tsx: close, then report the created Printer.
vi.mock("./PrinterBatchDialog", () => ({
  PrinterBatchDialog: (props: { open: boolean; onOpenChange: (open: boolean) => void; onEquip?: (id: string) => void }) => (
    <Show when={props.open}>
      <button
        onClick={() => {
          props.onOpenChange(false);
          props.onEquip?.("prn-1");
        }}
      >
        Equip stub
      </button>
    </Show>
  ),
}));

const PRINTER = {
  id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
  catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
  catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
  profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", materialSlots: [{ id: "slt-main", position: 0, name: "Main" }], setupGaps: [], createdAt: "", updatedAt: "",
  profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
} as ResolvedPrinter;

describe("PrinterDashboard — batch Equip", () => {
  afterEach(() => cleanup());

  it("selects the equipped Printer and opens its Setup tab at Material Slots", async () => {
    const monitor = createMonitorStore({
      printers: () => [PRINTER], initialSection: "printerModel", initialDensity: "comfortable",
      persistPreferences: vi.fn().mockResolvedValue(undefined),
    });
    const onSelectionChange = vi.fn();
    render(() => <PrinterDashboard store={monitor} onSelectionChange={onSelectionChange} />);

    await fireEvent.click(screen.getByRole("button", { name: "Add Printers…" }));
    await fireEvent.click(screen.getByRole("button", { name: "Equip stub" }));

    expect(onSelectionChange).toHaveBeenCalledWith("prn-1");
    await waitFor(() => expect(screen.getByRole("tab", { name: "Setup" })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("heading", { name: "Material Slots", level: 3 })).toBeInTheDocument();
  });

  it("handles an Equip request once: reopening the Printer later starts on Status", async () => {
    const monitor = createMonitorStore({
      printers: () => [PRINTER], initialSection: "printerModel", initialDensity: "comfortable",
      persistPreferences: vi.fn().mockResolvedValue(undefined),
    });
    render(() => <PrinterDashboard store={monitor} />);
    await fireEvent.click(screen.getByRole("button", { name: "Add Printers…" }));
    await fireEvent.click(screen.getByRole("button", { name: "Equip stub" }));
    await waitFor(() => expect(screen.getByRole("tab", { name: "Setup" })).toHaveAttribute("aria-selected", "true"));

    await fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("tab", { name: "Setup" })).not.toBeInTheDocument());
    monitor.setSelectedPrinterId("prn-1");

    await waitFor(() => expect(screen.getByRole("tab", { name: "Status" })).toHaveAttribute("aria-selected", "true"));
    expect(screen.getByRole("tab", { name: "Setup" })).toHaveAttribute("aria-selected", "false");
  });
});
