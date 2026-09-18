import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MonitorPrinterView } from "../monitor/monitor-store";
import { PrinterCard } from "./PrinterCard";

function view(overrides: Partial<MonitorPrinterView> = {}): MonitorPrinterView {
  return {
    id: "prn-1", name: "North Bay", vendor: "Bambu Lab", model: "X1 Carbon", modelLabel: "X1 Carbon",
    catalogStatus: "ok", operationalState: "printing", readiness: { state: "notReady", reason: "printerBusy" },
    operationalLabel: "Printing", freshness: "fresh", severity: "warning", severityLabel: "Monitor cache warning",
    hostActivity: "printing", hostActivityName: "Calibration cube", statusSummary: "Host print: Calibration cube · 42%", hasMissingReadings: false,
    readings: { progress: 42, nozzleTempC: 210, nozzleTargetC: 215, bedTempC: 55, bedTargetC: 60 },
    lastObservedAt: "2026-09-18T12:00:00Z", accessibleSummary: "North Bay; printing; Calibration cube",
    ...overrides,
  };
}

describe("PrinterCard", () => {
  afterEach(cleanup);

  it("shows the store-derived status summary and lets native keyboard activation select once", async () => {
    const onSelect = vi.fn();
    render(() => <PrinterCard printer={view()} onSelect={onSelect} />);

    const card = screen.getByRole("button", { name: /North Bay; printing/ });
    expect(card).toHaveTextContent("Printing");
    expect(card).toHaveTextContent("Host print: Calibration cube");
    expect(card).toHaveTextContent("42%");
    expect(card).toHaveTextContent("210 °C / 215 °C");
    expect(card).toHaveTextContent("Monitor cache warning");

    await fireEvent.keyDown(card, { key: "Enter" });
    await fireEvent.click(card);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenLastCalledWith("prn-1");
  });

  it("keeps stale ages and missing telemetry visibly unavailable", () => {
    render(() => <PrinterCard printer={view({
      operationalState: "offline", freshness: "stale", hostActivityName: undefined,
      operationalLabel: "Offline", statusSummary: "Host print: Printing", freshnessLabel: "Stale; last seen 3 minutes ago",
      readings: {}, lastObservedAt: "2026-09-18T11:57:00Z", severity: "info", severityLabel: undefined, hasMissingReadings: true,
    })} onSelect={vi.fn()} />);

    expect(screen.getByText(/Stale; last seen/)).toBeInTheDocument();
    expect(screen.getByText("Nozzle — / —")).toBeInTheDocument();
    expect(screen.queryByText("0 °C")).not.toBeInTheDocument();
  });
});
