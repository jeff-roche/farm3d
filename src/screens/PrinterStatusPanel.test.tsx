import { cleanup, fireEvent, render, screen, within } from "@solidjs/testing-library";
import { afterEach, describe, expect, it, vi } from "vitest";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { ResolvedPrinter } from "../printers/types";
import { PrinterStatusPanel } from "./PrinterStatusPanel";

const spools = vi.hoisted(() => [] as SpoolRecord[]);
vi.mock("../spools/spool-store", () => ({
  get spoolState() {
    return { spools, loaded: true };
  },
  ensureInventoryLoaded: () => Promise.resolve(),
}));

const LOADED_SPOOL: SpoolRecord = {
  id: "spl-7", revision: 1, spoolNumber: 7,
  manufacturer: "Overture", materialFamily: "PETG", colorName: "Black", colorHex: "#111111", diameter: "1.75",
  nominalMg: 1_000_000, lowThresholdMg: 100_000, lifecycle: "active",
  location: { kind: "slot", slotId: "slt-main", printerId: "prn-1" },
  availability: { currentMg: 612_000, reservedMg: 200_000, availableMg: 412_000 },
  facets: { loaded: true, reserved: true, low: true, confidence: "estimated" },
  createdAt: "", updatedAt: "",
};

const printer: ResolvedPrinter = {
  id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
  catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
  catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
  profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", materialSlots: [{ id: "slt-main", position: 0, name: "Main" }], setupGaps: [], createdAt: "", updatedAt: "",
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
    spools.length = 0;
    window.location.hash = "";
  });

  it("lists each Material Slot with its occupant (number, material, swatch, color, remaining est., Low/Reserved) or Empty", () => {
    spools.push(LOADED_SPOOL);
    render(() => <PrinterStatusPanel printer={{
      ...printer,
      materialSlots: [
        { id: "slt-main", position: 0, name: "Main", feederLabel: "AMS 1", occupantSpoolId: "spl-7" },
        { id: "slt-aux", position: 1, name: "Aux" },
      ],
    }} />);

    const list = screen.getByRole("list", { name: "Material slots" });
    const [main, aux] = within(list).getAllByRole("listitem");
    expect(main).toHaveTextContent("Main");
    expect(main).toHaveTextContent("AMS 1");
    const occupant = within(main).getByRole("button", { name: /#7/ });
    expect(occupant).toHaveTextContent("#7");
    expect(occupant).toHaveTextContent("PETG");
    expect(within(occupant).getByRole("img", { name: "Black" })).toBeInTheDocument();
    expect(occupant).toHaveTextContent("Black");
    expect(occupant).toHaveTextContent("612 g");
    expect(occupant).toHaveTextContent("est.");
    expect(within(main).getByText("Low")).toBeInTheDocument();
    expect(within(main).getByText("Reserved")).toBeInTheDocument();
    expect(aux).toHaveTextContent("Aux");
    expect(within(aux).getByText("Empty")).toBeInTheDocument();
  });

  it("activating an occupant deep-links to that Spool", () => {
    spools.push(LOADED_SPOOL);
    render(() => <PrinterStatusPanel printer={{
      ...printer,
      materialSlots: [{ id: "slt-main", position: 0, name: "Main", occupantSpoolId: "spl-7" }],
    }} />);

    fireEvent.click(screen.getByRole("button", { name: /#7/ }));

    expect(window.location.hash).toBe("#nav=v1/spools/spool/spl-7");
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

  it("lists each tool of a multi-tool printer as its own reading", () => {
    // A0.1 (#9), decision B2.
    const status = printer.runtimeStatus!;
    render(() => <PrinterStatusPanel printer={{ ...printer, runtimeStatus: { ...status, telemetry: {
      ...status.telemetry, tools: [{ index: 0, tempC: 210, targetC: 215 }, { index: 1, tempC: 30 }],
    } } }} />);
    const field = (label: string) => screen.getByText(label, { selector: "dt" }).nextElementSibling;
    expect(field("T0")).toHaveTextContent("210 °C / 215 °C");
    expect(field("T1")).toHaveTextContent("30 °C / —");
    expect(screen.queryByText("Nozzle", { selector: "dt" })).not.toBeInTheDocument();
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
