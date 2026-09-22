import { cleanup, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterStatusPanel } from "./PrinterStatusPanel";

const printer: ResolvedPrinter = {
  id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
  catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
  catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
  profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", setupGaps: [], createdAt: "", updatedAt: "",
  profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 0, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
  runtimeStatus: {
    connectionState: "online", telemetry: { hostActivity: "printing", hostActivityName: "calibration cube", progress: 0.42, nozzleTempC: 210, nozzleTargetC: 215, bedTempC: 60, bedTargetC: 60 },
    operationalState: "printing", readiness: { state: "notReady", reason: "printerBusy" }, freshness: "fresh", cacheWarnings: [], lastObservedAt: "2026-09-18T12:00:00Z", updatedAt: "2026-09-18T12:00:00Z",
  },
};

describe("PrinterStatusPanel", () => {
  afterEach(() => {
    cleanup();
    vi.useRealTimers();
  });

  it("renders generated operational state, telemetry, and reconciliation uncertainty", () => {
    render(() => <PrinterStatusPanel printer={printer} syncState="uncertain" />);

    expect(screen.getByText("printing", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("Printer busy")).toBeInTheDocument();
    expect(screen.getByText("online", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("calibration cube")).toBeInTheDocument();
    expect(screen.getByText("42%")).toBeInTheDocument();
    expect(screen.getByText("210 °C / 215 °C")).toBeInTheDocument();
    expect(screen.getByText("fresh", { exact: true })).toBeInTheDocument();
    expect(screen.getByText("Live status is still reconciling.")).toBeInTheDocument();
  });

  it("uses unavailable values rather than inventing telemetry", () => {
    render(() => <PrinterStatusPanel printer={{ ...printer, runtimeStatus: undefined }} />);
    expect(screen.getAllByText("Unavailable").length).toBeGreaterThan(0);
    expect(screen.getAllByText("—").length).toBeGreaterThan(0);
  });

  it("formats stale observations as a relative age", () => {
    vi.useFakeTimers();
    vi.setSystemTime(new Date("2026-09-18T12:02:00Z"));
    render(() => <PrinterStatusPanel printer={{
      ...printer,
      runtimeStatus: { ...printer.runtimeStatus!, freshness: "stale" },
    }} />);

    expect(screen.getByText("2 minutes ago")).toBeInTheDocument();
  });

  it("shows Last observed as unavailable without an observation", () => {
    render(() => <PrinterStatusPanel printer={{
      ...printer,
      runtimeStatus: { ...printer.runtimeStatus!, lastObservedAt: undefined },
    }} />);

    expect(screen.getByText("Last observed").nextElementSibling).toHaveTextContent("Unavailable");
  });
});
