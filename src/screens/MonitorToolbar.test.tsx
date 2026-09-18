import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import { createMonitorStore } from "../monitor/monitor-store";
import { MonitorToolbar } from "./MonitorToolbar";

function renderToolbar(persistPreferences = vi.fn().mockResolvedValue(undefined)) {
  const store = createMonitorStore({
    printers: () => [],
    initialSection: "printerModel",
    initialDensity: "comfortable",
    persistPreferences,
  });
  const onAddPrinter = vi.fn();
  render(() => <MonitorToolbar store={store} onAddPrinter={onAddPrinter} />);
  return { store, onAddPrinter };
}

describe("MonitorToolbar", () => {
  afterEach(cleanup);

  it("updates search, every Monitor filter, and starts Add Printer", async () => {
    const { store, onAddPrinter } = renderToolbar();

    await fireEvent.input(screen.getByRole("searchbox", { name: "Search Printers" }), {
      target: { value: "north bay" },
    });
    expect(store.search()).toBe("north bay");

    for (const [label, filter] of [
      ["All", "all"], ["Attention", "attention"], ["Printing", "printing"],
      ["Ready", "ready"], ["Offline", "offline"], ["Setup incomplete", "setupIncomplete"],
    ] as const) {
      await fireEvent.click(screen.getByRole("button", { name: label }));
      expect(store.filter()).toBe(filter);
    }

    await fireEvent.click(screen.getByRole("button", { name: "Add Printer" }));
    expect(onAddPrinter).toHaveBeenCalledOnce();
  });

  it("offers only P1 sections and changes selectors through Kobalte pointer events", async () => {
    const { store } = renderToolbar();

    const section = screen.getByRole("button", { name: /Monitor section/ });
    await fireEvent.pointerDown(section, { button: 0, pointerType: "mouse" });
    expect(screen.getByRole("option", { name: "Printer model" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "Operational state" })).toBeInTheDocument();
    expect(screen.getByRole("option", { name: "No section" })).toBeInTheDocument();
    expect(screen.queryByRole("option", { name: "Location" })).not.toBeInTheDocument();
    const operationalState = screen.getByRole("option", { name: "Operational state" });
    await fireEvent.pointerDown(operationalState, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(operationalState, { button: 0, pointerType: "mouse" });
    expect(store.section()).toBe("operationalState");

    const density = screen.getByRole("button", { name: /Monitor density/ });
    await fireEvent.pointerDown(density, { button: 0, pointerType: "mouse" });
    const compact = screen.getByRole("option", { name: "Compact" });
    await fireEvent.pointerDown(compact, { button: 0, pointerType: "mouse" });
    await fireEvent.pointerUp(compact, { button: 0, pointerType: "mouse" });
    expect(store.density()).toBe("compact");
  });

  it("shows a recoverable preference save error", async () => {
    const { store } = renderToolbar(vi.fn().mockRejectedValue(new Error("offline")));
    store.setDensity("compact");

    await waitFor(() => {
      expect(screen.getByRole("alert")).toHaveTextContent("Monitor preferences could not be saved.");
    });
  });
});
