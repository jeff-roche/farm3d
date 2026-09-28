import { cleanup, fireEvent, render, screen, waitFor, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMonitorStore } from "../monitor/monitor-store";
import type { ResolvedPrinter } from "../printers/types";
import { loadWebQueueFixture, resetQueueStoreMock } from "../queue/queue-store-mock";
import { PrinterDashboard } from "./PrinterDashboard";

vi.mock("../queue/queue-store", async () => (await import("../queue/queue-store-mock")).queueStoreMock);
vi.mock("../host-ops/host-operations-store", async () =>
  (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);
vi.mock("../host-ops/capabilities-store", async () =>
  (await import("../host-ops/capabilities-store-mock")).capabilitiesStoreMock);

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
    profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", materialSlots: [{ id: "slt-main", position: 0, name: "Main" }], setupGaps: [], createdAt: "", updatedAt: "",
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
    resetQueueStoreMock();
  });

  it("keeps loading separate from first-run and an empty Farm", () => {
    const empty = store([]);
    render(() => <PrinterDashboard store={empty} loading isFirstRun={false} />);
    expect(screen.getByText("Loading persisted Printers…")).toBeInTheDocument();

    cleanup();
    render(() => <PrinterDashboard store={empty} isFirstRun />);
    expect(screen.getByText("Start your Farm by adding a Printer.")).toBeInTheDocument();

    cleanup();
    render(() => <PrinterDashboard store={empty} />);
    expect(screen.getByText("This Farm has no Printers.")).toBeInTheDocument();
  });

  it("opens the batch dialog from the toolbar's 'Add Printers…' button", async () => {
    const monitor = store([]);
    render(() => <PrinterDashboard store={monitor} />);

    expect(screen.queryByRole("dialog", { name: "Add Printers" })).not.toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Add Printers…" }));
    // The dialog loads on first use.
    expect(await screen.findByRole("dialog", { name: "Add Printers" })).toBeInTheDocument();
  });

  it("also offers 'Add Printers…' from the first-run empty state", async () => {
    const empty = store([]);
    render(() => <PrinterDashboard store={empty} isFirstRun />);

    // One in the toolbar (always present) plus one in the first-run empty state.
    const buttons = screen.getAllByRole("button", { name: "Add Printers…" });
    expect(buttons).toHaveLength(2);
    await fireEvent.click(buttons[1]);
    expect(await screen.findByRole("dialog", { name: "Add Printers" })).toBeInTheDocument();
  });

  it("preserves the active filter for filtered-empty results and clears it only on request", async () => {
    const monitor = store([printer()]);
    monitor.setSearch("missing");
    render(() => <PrinterDashboard store={monitor} />);

    expect(screen.getByText("No Printers match the current search and filters.")).toBeInTheDocument();
    expect(monitor.search()).toBe("missing");
    await fireEvent.click(screen.getByRole("button", { name: "Clear search and filters" }));
    expect(monitor.search()).toBe("");
    expect(monitor.filter()).toBe("all");
  });

  it("renders sections from the Monitor store, retains content during sync uncertainty, and selects a card", async () => {
    const monitor = store([printer()]);
    const onSelectionChange = vi.fn();
    render(() => <PrinterDashboard store={monitor} syncState="uncertain" onSelectionChange={onSelectionChange} />);

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
    render(() => <PrinterDashboard store={monitor} />);

    const heading = screen.getByRole("heading", { name: "X1 Carbon" });
    await fireEvent.click(screen.getByRole("button", { name: "9 Printers" }));
    await fireEvent.click(await screen.findByRole("button", { name: "View all" }));
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

    render(() => <PrinterDashboard store={monitor} />);

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

    render(() => <PrinterDashboard store={monitor} />);

    await waitFor(() => expect(screen.getByRole("dialog", { name: "North Bay" })).toBeInTheDocument());
    expect(screen.queryByRole("complementary", { name: "North Bay" })).not.toBeInTheDocument();
  });

  function stubWorkspaceWidth(width: number) {
    class WorkspaceObserver {
      constructor(private readonly callback: ResizeObserverCallback) {}
      observe(target: Element) {
        this.callback([{ target, contentRect: { width } } as ResizeObserverEntry], this as unknown as ResizeObserver);
      }
      disconnect() {}
      unobserve() {}
    }
    vi.stubGlobal("ResizeObserver", WorkspaceObserver);
  }

  it("docks the Queue preview when no Printer is selected, and opens a chosen Job's panel", async () => {
    stubWorkspaceWidth(1440);
    loadWebQueueFixture();
    render(() => <PrinterDashboard store={store([printer()])} />);
    const preview = await screen.findByRole("complementary", { name: "Queue preview" });
    expect(within(preview).getByRole("list", { name: "Active Jobs" })).toBeInTheDocument();
    fireEvent.click(within(preview).getByRole("button", { name: /Four-tool — Bay 5/ }));
    const dock = await screen.findByRole("complementary", { name: "Job" });
    expect(within(dock).getByRole("button", { name: "Pause" })).toBeInTheDocument();
    fireEvent.click(within(dock).getByRole("button", { name: "Back to Queue" }));
    expect(await screen.findByRole("complementary", { name: "Queue preview" })).toBeInTheDocument();
  });

  it("gives the Printer's detail the dock once a Printer is selected", async () => {
    stubWorkspaceWidth(1440);
    const monitor = store([printer()]);
    monitor.setSelectedPrinterId("prn-1");
    render(() => <PrinterDashboard store={monitor} />);
    await waitFor(() => expect(screen.getByRole("complementary", { name: "North Bay" })).toBeInTheDocument());
    expect(screen.queryByRole("complementary", { name: "Queue preview" })).not.toBeInTheDocument();
  });

  it("opens the Queue preview as an overlay from the toolbar at a narrow workspace", async () => {
    stubWorkspaceWidth(1024);
    render(() => <PrinterDashboard store={store([printer()])} />);
    expect(screen.queryByRole("dialog")).not.toBeInTheDocument();
    fireEvent.click(await screen.findByRole("button", { name: "Queue preview" }));
    expect(await screen.findByRole("dialog", { name: "Queue preview" })).toBeInTheDocument();
  });

  it("replaces the overlay's Queue preview with the chosen Job", async () => {
    stubWorkspaceWidth(1024);
    loadWebQueueFixture();
    render(() => <PrinterDashboard store={store([printer()])} />);
    fireEvent.click(await screen.findByRole("button", { name: "Queue preview" }));
    const dialog = await screen.findByRole("dialog", { name: "Queue preview" });
    fireEvent.click(within(dialog).getByRole("button", { name: /Four-tool — Bay 5/ }));
    const job = await screen.findByRole("region", { name: "Job" });
    expect(within(job).getByRole("button", { name: "Pause" })).toBeInTheDocument();
    expect(screen.queryByRole("list", { name: "Next up" })).toBeNull();
  });

  it("closes the overlay Queue preview, and an overlay Job, with a visible Close", async () => {
    stubWorkspaceWidth(1024);
    loadWebQueueFixture();
    render(() => <PrinterDashboard store={store([printer()])} />);
    fireEvent.click(await screen.findByRole("button", { name: "Queue preview" }));
    let dialog = await screen.findByRole("dialog", { name: "Queue preview" });
    fireEvent.click(within(dialog).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());

    fireEvent.click(screen.getByRole("button", { name: "Queue preview" }));
    dialog = await screen.findByRole("dialog", { name: "Queue preview" });
    fireEvent.click(within(dialog).getByRole("button", { name: /Four-tool — Bay 5/ }));
    await screen.findByRole("region", { name: "Job" });
    fireEvent.click(within(screen.getByRole("dialog")).getByRole("button", { name: "Close" }));
    await waitFor(() => expect(screen.queryByRole("dialog")).not.toBeInTheDocument());
  });
});
