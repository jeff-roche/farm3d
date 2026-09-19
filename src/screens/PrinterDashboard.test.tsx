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

function printer(overrides: Partial<ResolvedPrinter> = {}): ResolvedPrinter {
  return {
    id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
    catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
    catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
    profileDrift: [], unknownOverrideKeys: [], createdAt: "", updatedAt: "",
    profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
    ...overrides,
  };
}

function store(printers: ResolvedPrinter[]) {
  return createMonitorStore({ printers: () => printers, initialSection: "printerModel", initialDensity: "comfortable", persistPreferences: vi.fn().mockResolvedValue(undefined) });
}

describe("PrinterDashboard", () => {
  afterEach(() => {
    cleanup();
    vi.unstubAllGlobals();
  });

  it("keeps loading separate from first-run and an empty Farm", () => {
    const empty = store([]);
    render(() => <PrinterDashboard store={empty} loading isFirstRun={false} onAddPrinter={vi.fn()} />);
    expect(screen.getByText("Loading persisted Printers…")).toBeInTheDocument();

    cleanup();
    render(() => <PrinterDashboard store={empty} isFirstRun onAddPrinter={vi.fn()} />);
    expect(screen.getByText("Start your Farm by adding a Printer.")).toBeInTheDocument();

    cleanup();
    render(() => <PrinterDashboard store={empty} onAddPrinter={vi.fn()} />);
    expect(screen.getByText("This Farm has no Printers.")).toBeInTheDocument();
  });

  it("preserves the active filter for filtered-empty results and clears it only on request", async () => {
    const monitor = store([printer()]);
    monitor.setSearch("missing");
    render(() => <PrinterDashboard store={monitor} onAddPrinter={vi.fn()} />);

    expect(screen.getByText("No Printers match the current search and filters.")).toBeInTheDocument();
    expect(monitor.search()).toBe("missing");
    await fireEvent.click(screen.getByRole("button", { name: "Clear search and filters" }));
    expect(monitor.search()).toBe("");
    expect(monitor.filter()).toBe("all");
  });

  it("renders sections from the Monitor store, retains content during sync uncertainty, and selects a card", async () => {
    const monitor = store([printer()]);
    const onSelectionChange = vi.fn();
    render(() => <PrinterDashboard store={monitor} syncState="uncertain" onAddPrinter={vi.fn()} onSelectionChange={onSelectionChange} />);

    expect(screen.getByText("X1 Carbon")).toBeInTheDocument();
    expect(screen.getByRole("status")).toHaveTextContent("Live status is still reconciling.");
    await fireEvent.click(screen.getAllByRole("button", { name: /North Bay; Status unavailable/ })[0]);
    expect(onSelectionChange).toHaveBeenCalledWith("prn-1");
    expect(screen.getByRole("dialog", { name: "North Bay" })).toBeInTheDocument();

    await fireEvent.click(screen.getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
    expect(screen.getAllByRole("button", { name: /North Bay; Status unavailable/ })[0]).toHaveFocus();
  });

  it("returns View all to the complete section instead of leaving the roster action inert", async () => {
    const printers = Array.from({ length: 9 }, (_, index) => printer({ id: `prn-${index}`, name: `Bay ${index}` }));
    const monitor = store(printers);
    render(() => <PrinterDashboard store={monitor} onAddPrinter={vi.fn()} />);

    const heading = screen.getByRole("heading", { name: "X1 Carbon" });
    await fireEvent.focus(screen.getByRole("button", { name: "9 Printers" }));
    await fireEvent.click(screen.getByRole("button", { name: "View all" }));
    await waitFor(() => expect(heading).toHaveFocus());
  });

  it("keeps the dock inline only when the workspace leaves room for cards", async () => {
    class WideWorkspaceObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        this.callback([{ target, contentRect: { width: 1440 } } as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      disconnect() {}
      unobserve() {}
    }
    vi.stubGlobal("ResizeObserver", WideWorkspaceObserver);
    const monitor = store([printer()]);
    monitor.setSelectedPrinterId("prn-1");

    render(() => <PrinterDashboard store={monitor} onAddPrinter={vi.fn()} />);

    await waitFor(() => expect(screen.getByRole("complementary", { name: "North Bay" })).toBeInTheDocument());
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
  });

  it("uses an overlay dialog at a 1024px workspace", async () => {
    class NarrowWorkspaceObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        this.callback([{ target, contentRect: { width: 1024 } } as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      disconnect() {}
      unobserve() {}
    }
    vi.stubGlobal("ResizeObserver", NarrowWorkspaceObserver);
    const monitor = store([printer()]);
    monitor.setSelectedPrinterId("prn-1");

    render(() => <PrinterDashboard store={monitor} onAddPrinter={vi.fn()} />);

    await waitFor(() => expect(screen.getByRole("dialog", { name: "North Bay" })).toBeInTheDocument());
    expect(screen.queryByRole("complementary", { name: "North Bay" })).not.toBeInTheDocument();
  });
});
