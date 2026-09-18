import { cleanup, fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { MonitorPrinterView } from "../monitor/monitor-store";
import { PrinterCompactRow } from "./PrinterCompactRow";

const printer: MonitorPrinterView = {
  id: "prn-1", name: "North Bay", vendor: "Bambu Lab", model: "X1 Carbon", modelLabel: "X1 Carbon",
  catalogStatus: "ok", operationalState: "ready", readiness: { state: "ready", reason: null },
  freshness: "unavailable", severity: "fatal", hostActivity: "idle", readings: {},
  accessibleSummary: "North Bay; ready; telemetry unavailable",
};

describe("PrinterCompactRow", () => {
  afterEach(cleanup);

  it("uses the card view model without manufacturing missing readings and selects by keyboard", async () => {
    const onSelect = vi.fn();
    render(() => <PrinterCompactRow printer={printer} onSelect={onSelect} />);

    const row = screen.getByRole("button", { name: /North Bay; ready/ });
    expect(row).toHaveTextContent("Ready");
    expect(row).toHaveTextContent("Telemetry unavailable");
    expect(row).toHaveTextContent("Connection error");
    expect(row).toHaveTextContent("Nozzle — / —");
    expect(row).not.toHaveTextContent("0 °C");

    await fireEvent.keyDown(row, { key: " " });
    expect(onSelect).toHaveBeenCalledWith("prn-1");
  });

  it("renders progress only for fresh printing telemetry", () => {
    render(() => <PrinterCompactRow printer={{
      ...printer, operationalState: "printing", freshness: "fresh",
      readings: { progress: 57 }, hostActivity: "printing",
    }} onSelect={vi.fn()} />);
    expect(screen.getByText("Host print 57%")).toBeInTheDocument();

    cleanup();
    render(() => <PrinterCompactRow printer={{
      ...printer, operationalState: "printing", freshness: "stale",
      readings: { progress: 57 }, hostActivity: "printing",
    }} onSelect={vi.fn()} />);
    expect(screen.queryByText("Host print 57%")).not.toBeInTheDocument();
  });
});
