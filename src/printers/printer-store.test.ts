import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

const A_RESOLVED_PRINTER = {
  id: "prn-1",
  name: "Centauri Carbon — Bay 1",
  group: "Bay 1",
  notes: "",
  catalogRef: {
    vendor: "Elegoo", model: "Elegoo Centauri Carbon",
    variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
  },
  catalogStatus: "ok",
  modelLabel: "Elegoo Centauri Carbon",
  variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
  profile: {
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
    nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
    hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
    suggestedHostType: "elegoolink",
  },
  overriddenFields: [],
  inherited: {},
  profileDrift: [],
  unknownOverrideKeys: [],
  connection: null,
};

describe("printer-store", () => {
  describe("under Tauri", () => {
    beforeEach(() => tauriMock.isTauri.mockReturnValue(true));

    it("loads printers via list_printers", async () => {
      tauriMock.invoke.mockResolvedValue([A_RESOLVED_PRINTER]);
      const { loadPrinters, printers } = await import("./printer-store");
      await loadPrinters();
      expect(tauriMock.invoke).toHaveBeenCalledWith("list_printers");
      expect(printers()).toEqual([A_RESOLVED_PRINTER]);
    });

    it("adds a printer via create_printer and appends the resolved result", async () => {
      tauriMock.invoke.mockResolvedValue([]);
      const { loadPrinters, addPrinter, printers } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue(A_RESOLVED_PRINTER);

      const id = await addPrinter({ name: "Centauri Carbon — Bay 1", catalogRef: A_RESOLVED_PRINTER.catalogRef });

      expect(tauriMock.invoke).toHaveBeenCalledWith("create_printer", {
        draft: { name: "Centauri Carbon — Bay 1", catalogRef: A_RESOLVED_PRINTER.catalogRef },
      });
      expect(id).toBe("prn-1");
      expect(printers()).toEqual([A_RESOLVED_PRINTER]);
    });

    it("revertField sends value: null", async () => {
      tauriMock.invoke.mockResolvedValue([A_RESOLVED_PRINTER]);
      const { loadPrinters, revertField } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue(A_RESOLVED_PRINTER);

      await revertField("prn-1", "printableHeightMm");

      expect(tauriMock.invoke).toHaveBeenCalledWith("set_printer_override", {
        id: "prn-1",
        field: "printableHeightMm",
        value: null,
      });
    });

    it("removePrinter invokes delete_printer and drops the row", async () => {
      tauriMock.invoke.mockResolvedValue([A_RESOLVED_PRINTER]);
      const { loadPrinters, removePrinter, printers } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue(undefined);

      await removePrinter("prn-1");

      expect(tauriMock.invoke).toHaveBeenCalledWith("delete_printer", { id: "prn-1" });
      expect(printers()).toEqual([]);
    });

    it("a rejected mutation surfaces the message instead of rejecting, and can be dismissed", async () => {
      tauriMock.invoke.mockResolvedValue([A_RESOLVED_PRINTER]);
      const { loadPrinters, overrideField, printerStoreError, dismissPrinterStoreError } =
        await import("./printer-store");
      await loadPrinters();
      expect(printerStoreError()).toBeNull();

      tauriMock.invoke.mockRejectedValue(new Error("printers.json is read-only"));
      // Must not reject: every call site discards the promise with `void`.
      await expect(overrideField("prn-1", "printableHeightMm", 240)).resolves.toBeUndefined();

      expect(printerStoreError()).toContain("printers.json is read-only");

      dismissPrinterStoreError();
      expect(printerStoreError()).toBeNull();
    });
  });

  describe("under just web (no Tauri backend)", () => {
    beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

    it("seeds from the web fallback fixture without invoking any command", async () => {
      const { loadPrinters, printers } = await import("./printer-store");
      await loadPrinters();
      expect(printers().length).toBeGreaterThan(0);
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("overrideField and revertField are no-ops without a catalog to resolve against", async () => {
      const { loadPrinters, printers, overrideField, revertField } = await import("./printer-store");
      await loadPrinters();
      const id = printers()[0].id;

      await overrideField(id, "printableHeightMm", 240);
      await revertField(id, "printableHeightMm");

      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("merges a status event onto the matching printer and leaves siblings alone", async () => {
      const store = await import("./printer-store");
      await store.loadPrinters();
      const [first, second] = store.printers();

      store.applyStatus(first.id, {
        connectionState: "online",
        nozzleTempC: 201.4,
        updatedAt: "2026-08-20T14:02:11Z",
      });

      expect(store.printers()[0].runtimeStatus?.nozzleTempC).toBe(201.4);
      expect(store.printers().find((p) => p.id === second.id)?.runtimeStatus).toBeUndefined();
    });

    it("ignores a status event for a printer it does not know", async () => {
      // A stale event can arrive after a delete; it must not resurrect a row.
      const store = await import("./printer-store");
      await store.loadPrinters();
      const before = store.printers().length;
      store.applyStatus("prn-ghost", {
        connectionState: "online",
        updatedAt: "2026-08-20T14:02:11Z",
      });
      expect(store.printers()).toHaveLength(before);
    });

    it("does not invoke connection mutations in the web fallback", async () => {
      const store = await import("./printer-store");
      await store.loadPrinters();
      await store.setConnection("prn-voron-1", {
        kind: "moonraker",
        host: "voron.local",
        port: 7125,
        useTls: false,
      });
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });
  });
});
