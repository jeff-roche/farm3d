import { describe, expect, it, vi } from "vitest";
import type { MonitorSection } from "../generated/contracts/domain/MonitorSection";
import type { PrinterStatus, ResolvedPrinter } from "../printers/types";
import { createMonitorStore } from "./monitor-store";

function status(overrides: Partial<PrinterStatus> = {}): PrinterStatus {
  return {
    connectionState: "online",
    telemetry: { hostActivity: "idle" },
    operationalState: "ready",
    readiness: { state: "ready", reason: null },
    freshness: "fresh",
    cacheWarnings: [],
    updatedAt: "2026-09-18T12:00:00Z",
    ...overrides,
  };
}

function printer(overrides: Partial<ResolvedPrinter> = {}): ResolvedPrinter {
  return {
    id: "printer-1",
    revision: 1,
    name: "Bay One",
    notes: "",
    overrides: {},
    catalogRef: {
      vendor: "Bambu Lab",
      model: "X1 Carbon",
      variant: "X1 Carbon 0.4",
      modelId: "x1-carbon",
      printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "X1 Carbon",
    variantLabel: "X1 Carbon 0.4",
    profile: {
      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256,
      bedExcludeAreas: [],
      defaultBedType: "",
      nozzleDiameterMm: [0.4],
      nozzleType: "brass",
      gcodeFlavor: "klipper",
      hasAuxiliaryFan: false,
      supportsAirFiltration: false,
      supportsMultiFilament: false,
      suggestedHostType: null,
    },
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    createdAt: "",
    updatedAt: "",
    ...overrides,
  };
}

function monitor(printers: ResolvedPrinter[], section: MonitorSection = "printerModel") {
  return createMonitorStore({
    printers: () => printers,
    initialSection: section,
    initialDensity: "comfortable",
    persistPreferences: vi.fn().mockResolvedValue(undefined),
  });
}

describe("Monitor store", () => {
  it("searches names, vendor/model identity, and normalized host activity names case-insensitively", () => {
    const store = monitor([
      printer({ id: "name", name: "North Bay" }),
      printer({ id: "model", catalogRef: { ...printer().catalogRef, vendor: "Prusa Research", model: "MK4" } }),
      printer({ id: "activity", runtimeStatus: status({ telemetry: { hostActivity: "busy", hostActivityName: "Heat Soak" }, operationalState: "busy", readiness: { state: "notReady", reason: "printerBusy" } }) }),
    ]);

    store.setSearch("north");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["name"]);
    store.setSearch("PRUSA RESEARCH MK4");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["model"]);
    store.setSearch("heat soak");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["activity"]);
  });

  it("filters generated operational, readiness, and severity facts without treating stale printing as current printing", () => {
    const store = monitor([
      printer({ id: "fresh-print", runtimeStatus: status({ telemetry: { hostActivity: "printing" }, operationalState: "printing", readiness: { state: "notReady", reason: "printerBusy" } }) }),
      printer({ id: "stale-print", runtimeStatus: status({ telemetry: { hostActivity: "printing" }, operationalState: "printing", readiness: { state: "notReady", reason: "printerBusy" }, freshness: "stale" }) }),
      printer({ id: "ready", runtimeStatus: status() }),
      printer({ id: "offline", runtimeStatus: status({ connectionState: "offline", operationalState: "offline", readiness: { state: "notReady", reason: "offline" }, freshness: "stale" }) }),
      printer({ id: "setup", runtimeStatus: status({ operationalState: "setupIncomplete", readiness: { state: "notReady", reason: "setupIncomplete" }, freshness: "unavailable" }) }),
      printer({ id: "warning", runtimeStatus: status({ cacheWarnings: [{ printerId: "warning", operation: "save" }] }) }),
      printer({ id: "fatal", runtimeStatus: status({ connectionState: "error", error: "Disconnected", operationalState: "error", readiness: { state: "notReady", reason: "connectionError" }, freshness: "stale" }) }),
    ]);

    store.setFilter("printing");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["fresh-print"]);
    store.setFilter("ready");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["ready", "warning"]);
    store.setFilter("offline");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["offline"]);
    store.setFilter("setupIncomplete");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["setup"]);
    store.setFilter("attention");
    expect(store.visiblePrinters().map((item) => item.id)).toEqual(["warning", "fatal"]);
  });

  it("groups models by vendor/model identity and places unlinked printers last", () => {
    const store = monitor([
      printer({ id: "prusa", name: "Zulu", catalogRef: { ...printer().catalogRef, vendor: "Prusa", model: "X1 Carbon" }, modelLabel: "X1 Carbon" }),
      printer({ id: "bambu-b", name: "Beta" }),
      printer({ id: "bambu-a", name: "Alpha" }),
      printer({ id: "unlinked", name: "Aardvark", catalogStatus: "modelMissing" }),
    ]);

    expect(store.sections().map((section) => section.key)).toEqual([
      "Bambu Lab::X1 Carbon",
      "Prusa::X1 Carbon",
      "__unlinked__",
    ]);
    expect(store.sections()[0].printers.map((item) => item.id)).toEqual(["bambu-a", "bambu-b"]);
    expect(store.sections()[2].label).toBe("Unlinked");
  });

  it("groups by generated operational state, and none produces one unlabelled section", () => {
    const printers = [
      printer({ id: "ready", runtimeStatus: status() }),
      printer({ id: "offline", runtimeStatus: status({ connectionState: "offline", operationalState: "offline", readiness: { state: "notReady", reason: "offline" }, freshness: "stale" }) }),
    ];

    expect(monitor(printers, "operationalState").sections().map((section) => section.key).sort()).toEqual(["offline", "ready"]);
    const none = monitor(printers, "none").sections();
    expect(none).toHaveLength(1);
    expect(none[0].label).toBe("");
    expect(none[0].printers.map((item) => item.id)).toEqual(["ready", "offline"]);
  });

  it("falls back from unavailable location grouping without rewriting the explicit saved preference", () => {
    const persistPreferences = vi.fn().mockResolvedValue(undefined);
    const store = createMonitorStore({
      printers: () => [printer()],
      initialSection: "location",
      initialDensity: "comfortable",
      persistPreferences,
    });

    expect(store.section()).toBe("location");
    expect(store.sections()[0].key).toBe("Bambu Lab::X1 Carbon");
    expect(persistPreferences).not.toHaveBeenCalled();
  });

  it("distinguishes filtered-empty results from a Farm with zero Printers", () => {
    const populated = monitor([printer()]);
    populated.setSearch("not present");
    expect(populated.hasPrinters()).toBe(true);
    expect(populated.isFilteredEmpty()).toBe(true);

    const empty = monitor([]);
    expect(empty.hasPrinters()).toBe(false);
    expect(empty.isFilteredEmpty()).toBe(false);
  });

  it("builds exact filtered rosters sorted by Printer name with eight visible rows and a remaining count", () => {
    const printers = Array.from({ length: 10 }, (_, index) => printer({
      id: `printer-${index}`,
      name: `Bay ${10 - index}`,
      runtimeStatus: status({ telemetry: { hostActivity: "printing" }, operationalState: "printing", readiness: { state: "notReady", reason: "printerBusy" } }),
    }));
    const store = monitor(printers, "none");
    store.setFilter("printing");

    const [roster] = store.rosters();
    expect(roster.count).toBe(10);
    expect(roster.printers.map((item) => item.name)).toEqual([
      "Bay 1", "Bay 10", "Bay 2", "Bay 3", "Bay 4", "Bay 5", "Bay 6", "Bay 7",
    ]);
    expect(roster.remainingCount).toBe(2);
  });

  it("serializes combined preference writes and retains the selected choices after a recoverable save failure", async () => {
    const resolvers: Array<() => void> = [];
    const persistPreferences = vi.fn().mockImplementation(() => new Promise<void>((resolve) => {
      resolvers.push(resolve);
    }));
    const store = createMonitorStore({
      printers: () => [],
      initialSection: "printerModel",
      initialDensity: "comfortable",
      persistPreferences,
    });

    store.setSection("operationalState");
    store.setDensity("compact");
    await Promise.resolve();
    expect(persistPreferences).toHaveBeenCalledTimes(1);
    expect(persistPreferences).toHaveBeenLastCalledWith({ monitorSection: "operationalState", monitorDensity: "comfortable" });
    resolvers.shift()!();
    await Promise.resolve();
    await Promise.resolve();
    expect(persistPreferences).toHaveBeenLastCalledWith({ monitorSection: "operationalState", monitorDensity: "compact" });

    resolvers.shift()!();
    await Promise.resolve();
    persistPreferences.mockRejectedValueOnce(new Error("save failed"));
    store.setDensity("comfortable");
    expect(store.density()).toBe("comfortable");
    await vi.waitFor(() => {
      expect(store.preferenceError()).toBe("Monitor preferences could not be saved.");
    });
  });
});
