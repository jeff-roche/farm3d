import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const eventMock = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => eventMock);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  eventMock.listen.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
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

    it("keeps live runtimeStatus when a mutation splices in a fresh ResolvedPrinter", async () => {
      // Rust never returns `runtimeStatus` — it is frontend-only live state.
      // A rename must not blank the connection badge and temperatures until
      // the supervisor's next push, which for an offline printer is up to a
      // minute of backoff away.
      // Clones, not the shared fixture: the store mutates what it is handed,
      // so reusing the literal would smuggle `runtimeStatus` into the
      // "fresh from Rust" value and make this pass without the fix.
      tauriMock.invoke.mockResolvedValue([structuredClone(A_RESOLVED_PRINTER)]);
      const { loadPrinters, applyStatus, updatePrinter, printers } = await import("./printer-store");
      await loadPrinters();
      applyStatus("prn-1", {
        connectionState: "error",
        error: "Could not reach the printer",
        updatedAt: "2026-08-20T14:02:11Z",
      });

      tauriMock.invoke.mockResolvedValue({
        ...structuredClone(A_RESOLVED_PRINTER),
        name: "Bay 1 — renamed",
      });
      await updatePrinter("prn-1", { name: "Bay 1 — renamed" });

      expect(printers()[0].name).toBe("Bay 1 — renamed");
      expect(printers()[0].runtimeStatus?.connectionState).toBe("error");
      expect(printers()[0].runtimeStatus?.error).toBe("Could not reach the printer");
    });

    it("applies a printer-status event straight off the Rust event channel", async () => {
      // Pins the seam between supervisor.rs's STATUS_EVENT/StatusEvent and
      // this module's own `listen(...)` string and payload shape. Both sides
      // otherwise mock each other away, so a rename on either would break
      // live status with a green test suite.
      const calls: string[] = [];
      let handler: ((event: { payload: { id: string; status: unknown } }) => void) | undefined;
      const unlisten = vi.fn();
      eventMock.listen.mockImplementation((name: string, cb: typeof handler) => {
        calls.push(`listen:${name}`);
        handler = cb;
        return Promise.resolve(unlisten);
      });
      tauriMock.invoke.mockImplementation((command: string) => {
        calls.push(`invoke:${command}`);
        return Promise.resolve(command === "list_printers" ? [structuredClone(A_RESOLVED_PRINTER)] : {});
      });

      const { loadPrinters, startStatusListener, printers } = await import("./printer-store");
      await loadPrinters();
      const stop = await startStatusListener();

      expect(eventMock.listen).toHaveBeenCalledWith("printer-status", expect.any(Function));
      expect(calls.slice(-2)).toEqual([
        "listen:printer-status",
        "invoke:printer_statuses",
      ]);
      handler!({
        payload: {
          id: "prn-1",
          status: {
            connectionState: "online",
            jobState: "printing",
            jobName: "benchy.gcode",
            progress: 0.42,
            nozzleTempC: 210.5,
            nozzleTargetC: 210,
            bedTempC: 60.1,
            bedTargetC: 60,
            printDurationS: 812.5,
            updatedAt: "2026-08-20T14:02:11Z",
          },
        },
      });

      expect(printers()[0].runtimeStatus).toEqual({
        connectionState: "online",
        jobState: "printing",
        jobName: "benchy.gcode",
        progress: 0.42,
        nozzleTempC: 210.5,
        nozzleTargetC: 210,
        bedTempC: 60.1,
        bedTargetC: 60,
        printDurationS: 812.5,
        updatedAt: "2026-08-20T14:02:11Z",
      });
      expect(stop).toBe(unlisten);
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
    beforeEach(() => {
      tauriMock.isTauri.mockReturnValue(false);
      // The web fallback resolves its seed printers against the real,
      // bundled catalog via `fetch` (see printer-catalog.ts) -- this stands
      // in for what Vite's dev server actually serves, with just the two
      // models WEB_FALLBACK_SPECS names.
      vi.stubGlobal(
        "fetch",
        vi.fn().mockResolvedValue({
          json: () =>
            Promise.resolve({
              models: [
                {
                  modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon",
                  variants: [
                    {
                      variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4",
                      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
                      printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
                      nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
                      hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
                      suggestedHostType: "elegoolink",
                    },
                    {
                      variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6",
                      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
                      printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
                      nozzleDiameterMm: [0.6], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
                      hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
                      suggestedHostType: "elegoolink",
                    },
                  ],
                },
                {
                  modelId: "MK4", vendor: "Prusa", model: "Prusa MK4",
                  variants: [
                    {
                      variant: "Prusa MK4 0.4 nozzle", printerVariant: "0.4",
                      bedShape: { kind: "rectangular", widthMm: 250, depthMm: 210, originXMm: 0, originYMm: 0 },
                      printableHeightMm: 220, bedExcludeAreas: [], defaultBedType: "",
                      nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "marlin2",
                      hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false,
                      suggestedHostType: "prusalink",
                    },
                  ],
                },
              ],
            }),
        }),
      );
    });

    it("seeds from the web fallback fixture without invoking any command", async () => {
      const { loadPrinters, printers } = await import("./printer-store");
      await loadPrinters();
      expect(printers().length).toBeGreaterThan(0);
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("overrideField and revertField are no-ops -- no per-field override machinery to resolve against in web mode", async () => {
      const { loadPrinters, printers, overrideField, revertField } = await import("./printer-store");
      await loadPrinters();
      const id = printers()[0].id;

      await overrideField(id, "printableHeightMm", 240);
      await revertField(id, "printableHeightMm");

      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("rebindPrinter re-resolves the printer against the real catalog rather than no-opping", async () => {
      // Unlike overrideField/revertField above, a rebind has somewhere real
      // to go now that the fallback catalog is real data: picking a
      // sibling variant in the UI must actually take effect locally.
      const { loadPrinters, printers, rebindPrinter } = await import("./printer-store");
      await loadPrinters();
      const centauriCarbon = printers().find((p) => p.catalogRef.printerVariant === "0.4")!;

      await rebindPrinter(centauriCarbon.id, {
        vendor: "Elegoo", model: "Elegoo Centauri Carbon",
        variant: "Elegoo Centauri Carbon 0.6 nozzle", modelId: "Elegoo-CC", printerVariant: "0.6",
      });

      const rebound = printers().find((p) => p.id === centauriCarbon.id);
      expect(rebound?.catalogRef.printerVariant).toBe("0.6");
      expect(rebound?.variantLabel).toBe("Elegoo Centauri Carbon 0.6 nozzle");
      expect(rebound?.profile.nozzleDiameterMm).toEqual([0.6]);
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("rebindPrinter leaves the printer unchanged when the target variant no longer exists", async () => {
      const { loadPrinters, printers, rebindPrinter } = await import("./printer-store");
      await loadPrinters();
      const before = printers().find((p) => p.catalogRef.printerVariant === "0.4")!;

      await rebindPrinter(before.id, {
        vendor: "Elegoo", model: "Elegoo Centauri Carbon",
        variant: "Elegoo Centauri Carbon 12.0 nozzle", modelId: "Elegoo-CC", printerVariant: "12.0",
      });

      expect(printers().find((p) => p.id === before.id)).toEqual(before);
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
