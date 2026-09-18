import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { PrinterStatus } from "./types";

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

const A_PRINTER_RECORD = {
  id: "prn-1",
  revision: 1,
  name: "Centauri Carbon — Bay 1",
  notes: "",
  overrides: {},
  catalogRef: {
    vendor: "Elegoo", model: "Elegoo Centauri Carbon",
    variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4",
  },
  profileResolution: {
    catalogStatus: "ok" as const,
    modelLabel: "Elegoo Centauri Carbon",
    variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
    profile: {
      bedShape: { kind: "rectangular" as const, widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
      nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
      hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
      suggestedHostType: "elegoolink",
    },
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
  },
  createdAt: "",
  updatedAt: "",
};
function flattenRecord(record: typeof A_PRINTER_RECORD) {
  const { profileResolution, ...printer } = record;
  return { ...printer, ...profileResolution };
}
const A_RESOLVED_PRINTER = flattenRecord(A_PRINTER_RECORD);
const printerStatus = (
  connectionState: PrinterStatus["connectionState"],
  overrides: Partial<PrinterStatus> = {},
): PrinterStatus => ({
  connectionState,
  telemetry: { hostActivity: "idle" },
  operationalState: connectionState === "online" ? "ready" : connectionState,
  readiness: { state: connectionState === "online" ? "ready" : "notReady", reason: connectionState === "online" ? null : "offline" },
  freshness: "fresh",
  cacheWarnings: [],
  updatedAt: "2026-08-20T14:02:11Z",
  ...overrides,
});

describe("printer-store", () => {
  describe("under Tauri", () => {
    beforeEach(() => tauriMock.isTauri.mockReturnValue(true));

    it("loads printers via list_printers", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [A_PRINTER_RECORD] });
      const { loadPrinters, printers } = await import("./printer-store");
      await loadPrinters();
      expect(tauriMock.invoke).toHaveBeenCalledWith("list_printers", { contractVersion: 1 });
      expect(printers()).toEqual([A_RESOLVED_PRINTER]);
    });

    it("adds a printer via create_printer and appends the resolved result", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [] });
      const { loadPrinters, addPrinter, printers } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { printer: A_PRINTER_RECORD, warnings: [] } });

      const id = await addPrinter({ name: "Centauri Carbon — Bay 1", catalogRef: A_RESOLVED_PRINTER.catalogRef });

      expect(tauriMock.invoke).toHaveBeenCalledWith("create_printer", {
        contractVersion: 1,
        name: "Centauri Carbon — Bay 1",
        catalogRef: A_RESOLVED_PRINTER.catalogRef,
      });
      expect(id).toBe("prn-1");
      expect(printers()).toEqual([A_RESOLVED_PRINTER]);
    });

    it("revertField sends value: null", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [A_PRINTER_RECORD] });
      const { loadPrinters, revertField } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { printer: A_PRINTER_RECORD, warnings: [] } });

      await revertField("prn-1", "printableHeightMm");

      expect(tauriMock.invoke).toHaveBeenCalledWith("set_printer_override", {
        id: "prn-1",
        contractVersion: 1,
        expectedRevision: 1,
        field: "printableHeightMm",
        value: null,
      });
    });

    it("removePrinter invokes delete_printer and drops the row", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [A_PRINTER_RECORD] });
      const { loadPrinters, removePrinter, printers } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { deletedId: "prn-1", deletedRevision: 1, credentialCleanupPending: false, warnings: [] } });

      await removePrinter("prn-1");

      expect(tauriMock.invoke).toHaveBeenCalledWith("delete_printer", { contractVersion: 1, expectedRevision: 1, id: "prn-1" });
      expect(printers()).toEqual([]);
    });

    it("preserves runtime status only for Printers that survive an applied import", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [A_PRINTER_RECORD] });
      const { applyStatus, loadPrinters, importPrinters, printers } = await import("./printer-store");
      await loadPrinters();
      const liveStatus = printerStatus("online");
      applyStatus("prn-1", liveStatus);
      const importedRecord = { ...structuredClone(A_PRINTER_RECORD), name: "Imported Bay 1", revision: 2 };
      const createdRecord = { ...structuredClone(A_PRINTER_RECORD), id: "é-printer", revision: 1 };
      tauriMock.invoke.mockResolvedValue({
        contractVersion: 1,
        data: {
          status: "applied", printers: [importedRecord, createdRecord], createdCount: 1,
          updatedCount: 1, deletedCount: 0, warnings: [],
        },
      });

      await importPrinters();

      expect(printers()).toEqual([
        { ...flattenRecord(importedRecord), runtimeStatus: liveStatus },
        flattenRecord(createdRecord),
      ]);
    });

    it("sorts import revision preconditions by exact UTF-8 bytes", async () => {
      const astral = { ...structuredClone(A_PRINTER_RECORD), id: "\u{10000}", revision: 4 };
      const privateUse = { ...structuredClone(A_PRINTER_RECORD), id: "\u{e000}", revision: 3 };
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [astral, privateUse] });
      const { loadPrinters, importPrinters } = await import("./printer-store");
      await loadPrinters();
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { status: "cancelled" } });

      await importPrinters();

      expect(tauriMock.invoke).toHaveBeenLastCalledWith("import_printers", {
        contractVersion: 1,
        expectedRevisions: [
          { id: privateUse.id, revision: 3 },
          { id: astral.id, revision: 4 },
        ],
      });
    });

    it("does not turn a discovery command error into an empty successful scan", async () => {
      const commandError = {
        contractVersion: 1,
        code: "TIMEOUT",
        message: "Printer discovery did not finish in time.",
        recovery: ["CHECK_CONNECTION", "RETRY"],
        retryable: true,
      };
      tauriMock.invoke.mockRejectedValue(commandError);
      const { discoverPrinters, printerStoreError } = await import("./printer-store");

      await expect(discoverPrinters()).rejects.toEqual(commandError);
      expect(printerStoreError()).toBe(commandError.message);
    });

    it("keeps live runtimeStatus when a mutation splices in a fresh ResolvedPrinter", async () => {
      // Rust never returns `runtimeStatus` — it is frontend-only live state.
      // A rename must not blank the connection badge and temperatures until
      // the supervisor's next push, which for an offline printer is up to a
      // minute of backoff away.
      // Clones, not the shared fixture: the store mutates what it is handed,
      // so reusing the literal would smuggle `runtimeStatus` into the
      // "fresh from Rust" value and make this pass without the fix.
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [structuredClone(A_PRINTER_RECORD)] });
      const { loadPrinters, applyStatus, updatePrinter, printers } = await import("./printer-store");
      await loadPrinters();
      applyStatus("prn-1", printerStatus("error", { error: "Could not reach the printer" }));

      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { printer: {
        ...structuredClone(A_PRINTER_RECORD),
        name: "Bay 1 — renamed",
      }, warnings: [] } });
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
      let handler: ((event: { payload: unknown }) => void) | undefined;
      const unlisten = vi.fn();
      eventMock.listen.mockImplementation((name: string, cb: typeof handler) => {
        calls.push(`listen:${name}`);
        handler = cb;
        return Promise.resolve(unlisten);
      });
      tauriMock.invoke.mockImplementation((command: string) => {
        calls.push(`invoke:${command}`);
        return Promise.resolve({ contractVersion: 1, data: command === "list_printers" ? [structuredClone(A_PRINTER_RECORD)] : { streamId: "stream-a", snapshotSequence: 0, statuses: [] } });
      });

      const { loadPrinters, printerStatusSyncState, startStatusListener, printers } = await import("./printer-store");
      await loadPrinters();
      expect(printerStatusSyncState()).toBe("syncing");
      const stop = await startStatusListener();
      expect(printerStatusSyncState()).toBe("current");

      expect(eventMock.listen).toHaveBeenCalledWith("farm3d-event-v1", expect.any(Function));
      expect(calls.slice(-2)).toEqual([
        "listen:farm3d-event-v1",
        "invoke:printer_statuses",
      ]);
      handler!({
        payload: {
          contractVersion: 1,
          streamId: "stream-a",
          sequence: 1,
          eventId: "event-1",
          occurredAt: "2026-08-20T14:02:11Z",
          type: "printer.status.changed",
          subject: { kind: "printer", id: "prn-1" },
          payload: {
            type: "changed",
            status: printerStatus("online", {
              telemetry: {
                hostActivity: "printing",
                jobName: "benchy.gcode",
                progress: 0.42,
                nozzleTempC: 210.5,
                nozzleTargetC: 210,
                bedTempC: 60.1,
                bedTargetC: 60,
                printDurationS: 812.5,
              },
              operationalState: "printing",
              readiness: { state: "notReady", reason: "printerBusy" },
            }),
          },
        },
      });

      expect(printers()[0].runtimeStatus).toEqual(printerStatus("online", {
        telemetry: {
          hostActivity: "printing",
          jobName: "benchy.gcode",
          progress: 0.42,
          nozzleTempC: 210.5,
          nozzleTargetC: 210,
          bedTempC: 60.1,
          bedTargetC: 60,
          printDurationS: 812.5,
        },
        operationalState: "printing",
        readiness: { state: "notReady", reason: "printerBusy" },
      }));
      handler!({
        payload: {
          contractVersion: 1,
          streamId: "stream-a",
          sequence: 2,
          eventId: "event-2",
          occurredAt: "2026-08-20T14:02:12Z",
          type: "printer.status.removed",
          subject: { kind: "printer", id: "prn-1" },
          payload: { type: "removed" },
        },
      });
      expect(printers()[0].runtimeStatus).toBeUndefined();
      stop();
      expect(unlisten).toHaveBeenCalledOnce();
    });

    it("rejects listener startup errors with a user-safe message", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [A_PRINTER_RECORD] });
      eventMock.listen.mockRejectedValue(new Error("socket token leaked"));
      const { loadPrinters, startStatusListener } = await import("./printer-store");
      await loadPrinters();

      await expect(startStatusListener()).rejects.toThrow("Printer status monitoring could not start.");
    });

    it("rejects backfill startup errors and disposes the listener", async () => {
      const unlisten = vi.fn();
      eventMock.listen.mockResolvedValue(unlisten);
      tauriMock.invoke.mockImplementation((command: string) => command === "list_printers"
        ? Promise.resolve({ contractVersion: 1, data: [A_PRINTER_RECORD] })
        : Promise.reject(new Error("cache path exposed")));
      const { loadPrinters, startStatusListener } = await import("./printer-store");
      await loadPrinters();

      await expect(startStatusListener()).rejects.toThrow("Printer status monitoring could not start.");
      expect(unlisten).toHaveBeenCalledOnce();
    });

    it("a rejected mutation surfaces the message instead of rejecting, and can be dismissed", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [A_PRINTER_RECORD] });
      const { loadPrinters, overrideField, printerStoreError, dismissPrinterStoreError } =
        await import("./printer-store");
      await loadPrinters();
      expect(printerStoreError()).toBeNull();

      tauriMock.invoke.mockRejectedValue({
        contractVersion: 1,
        code: "PERSISTENCE_UNAVAILABLE",
        message: "printers.json is read-only",
        recovery: ["RETRY"],
        retryable: true,
      });
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

      store.applyStatus(first.id, printerStatus("online", {
        telemetry: { hostActivity: "idle", nozzleTempC: 201.4 },
      }));

      expect(store.printers()[0].runtimeStatus?.telemetry.nozzleTempC).toBe(201.4);
      expect(store.printers().find((p) => p.id === second.id)?.runtimeStatus).toBeUndefined();
    });

    it("ignores a status event for a printer it does not know", async () => {
      // A stale event can arrive after a delete; it must not resurrect a row.
      const store = await import("./printer-store");
      await store.loadPrinters();
      const before = store.printers().length;
      store.applyStatus("prn-ghost", printerStatus("online"));
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
