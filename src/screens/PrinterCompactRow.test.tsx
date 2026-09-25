import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MonitorPrinterView } from "../monitor/monitor-store";
import { PrinterCompactRow } from "./PrinterCompactRow";

const printer: MonitorPrinterView = {
  id: "prn-1", name: "North Bay", vendor: "Bambu Lab", model: "X1 Carbon", modelLabel: "X1 Carbon",
  archived: false,
  catalogStatus: "ok", operationalState: "ready", readiness: { state: "ready", reason: null },
  operationalLabel: "Ready", freshness: "unavailable", freshnessLabel: "Telemetry unavailable", severity: "fatal", severityLabel: "Connection error",
  hostActivity: "idle", statusSummary: "Telemetry unavailable", hasMissingReadings: true, readings: {},
  accessibleSummary: "North Bay; Ready; Telemetry unavailable",
};

describe("PrinterCompactRow", () => {
  afterEach(cleanup);

  it("uses the card view model without manufacturing missing readings and lets native keyboard activation select once", async () => {
    const onSelect = vi.fn();
    render(() => <PrinterCompactRow printer={printer} onSelect={onSelect} />);

    const row = screen.getByRole("button", { name: /North Bay; Ready/ });
    expect(row).toHaveTextContent("Ready");
    expect(row).toHaveTextContent("Telemetry unavailable");
    expect(row).toHaveTextContent("Connection error");
    expect(row).toHaveTextContent("Nozzle — / —");
    expect(row).toHaveTextContent("Readings unavailable");
    expect(row).not.toHaveTextContent("0 °C");

    await fireEvent.keyDown(row, { key: " " });
    await fireEvent.click(row);
    expect(onSelect).toHaveBeenCalledTimes(1);
    expect(onSelect).toHaveBeenCalledWith("prn-1");
  });

  it("shows each tool of a multi-tool printer in place of the single Nozzle reading", () => {
    // A0.1 (#9), decision B2.
    render(() => <PrinterCompactRow printer={{
      ...printer, hasMissingReadings: true,
      readings: { nozzleTempC: 24, nozzleTargetC: 0, bedTempC: 22, bedTargetC: 0, tools: [{ index: 0, tempC: 24, targetC: 0 }, { index: 1 }] },
    }} onSelect={vi.fn()} />);
    const row = screen.getByRole("button", { name: /North Bay/ });
    expect(row).toHaveTextContent("T0 24 °C / 0 °C · T1 — / — · Bed 22 °C / 0 °C");
    expect(row).not.toHaveTextContent("Nozzle");
  });

  it("renders progress only for fresh printing telemetry", () => {
    render(() => <PrinterCompactRow printer={{
      ...printer, operationalState: "printing", freshness: "fresh",
      operationalLabel: "Printing", statusSummary: "Host print: Printing · 57%", freshnessLabel: undefined,
      readings: { progress: 0.57 }, hostActivity: "printing",
    }} onSelect={vi.fn()} />);
    expect(screen.getByText("Host print: Printing · 57%")).toBeInTheDocument();

    cleanup();
    render(() => <PrinterCompactRow printer={{
      ...printer, operationalState: "printing", freshness: "stale",
      operationalLabel: "Printing", statusSummary: "Host print: Printing", freshnessLabel: "Stale; last seen 3 minutes ago",
      readings: { progress: 0.57 }, hostActivity: "printing",
    }} onSelect={vi.fn()} />);
    expect(screen.queryByText("Host print: Printing · 57%")).not.toBeInTheDocument();
  });
});
