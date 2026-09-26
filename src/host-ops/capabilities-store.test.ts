import { beforeEach, describe, expect, it, vi } from "vitest";
import { createStore } from "solid-js/store";
import { printerCapabilities, printerStatus } from "./test-records";
import type { PrinterStatus } from "./types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

type Responder = (args: Record<string, unknown>) => unknown;
let responders: Record<string, Responder>;

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  tauriMock.isTauri.mockReturnValue(true);
  responders = {};
  tauriMock.invoke.mockImplementation((name: string, args: Record<string, unknown>) => {
    const respond = responders[name];
    if (!respond) return Promise.reject(new Error(`unexpected command ${name}`));
    try {
      const data = respond(args);
      return data instanceof Promise
        ? data.then((value) => ({ contractVersion: 1, data: value }))
        : Promise.resolve({ contractVersion: 1, data });
    } catch (error) {
      return Promise.reject(error);
    }
  });
});

describe("capabilities-store (desktop)", () => {
  it("loads a Printer's capabilities and caches them", async () => {
    responders.printer_capabilities = (args) => printerCapabilities({ printerId: args.printerId as string });
    const { capabilities, loadCapabilities } = await import("./capabilities-store");
    expect(capabilities.forPrinter("prn-1")).toBeUndefined();
    const result = await loadCapabilities("prn-1");
    expect(result.printerId).toBe("prn-1");
    expect(capabilities.forPrinter("prn-1")).toEqual(result);
  });

  it("refreshCapabilities refetches and replaces the held row", async () => {
    let calls = 0;
    responders.printer_capabilities = () => {
      calls += 1;
      return printerCapabilities({ printerId: "prn-1", adapterKind: calls === 1 ? "moonraker" : "octoprint" });
    };
    const { capabilities, loadCapabilities, refreshCapabilities } = await import("./capabilities-store");
    await loadCapabilities("prn-1");
    expect(capabilities.forPrinter("prn-1")?.adapterKind).toBe("moonraker");
    await refreshCapabilities("prn-1");
    expect(capabilities.forPrinter("prn-1")?.adapterKind).toBe("octoprint");
    expect(calls).toBe(2);
  });

  it("holds each Printer's capabilities independently", async () => {
    responders.printer_capabilities = (args) => printerCapabilities({ printerId: args.printerId as string });
    const { capabilities, loadCapabilities } = await import("./capabilities-store");
    await loadCapabilities("prn-1");
    await loadCapabilities("prn-2");
    expect(capabilities.forPrinter("prn-1")?.printerId).toBe("prn-1");
    expect(capabilities.forPrinter("prn-2")?.printerId).toBe("prn-2");
  });

  it("loads the adapter capability matrix once and caches it", async () => {
    let calls = 0;
    responders.adapter_capability_matrix = () => {
      calls += 1;
      return [{ adapterKind: "moonraker", capabilities: printerCapabilities().capabilities }];
    };
    const { capabilities, loadAdapterCapabilityMatrix } = await import("./capabilities-store");
    expect(capabilities.adapterMatrix()).toEqual([]);
    await loadAdapterCapabilityMatrix();
    await loadAdapterCapabilityMatrix();
    expect(calls).toBe(1);
    expect(capabilities.adapterMatrix()).toHaveLength(1);
  });

  it("never carries a credential field", async () => {
    responders.printer_capabilities = (args) => printerCapabilities({ printerId: args.printerId as string });
    const { loadCapabilities } = await import("./capabilities-store");
    const result = await loadCapabilities("prn-1");
    expect(JSON.stringify(result)).not.toMatch(/credential/i);
  });
});

describe("capabilities-store (web)", () => {
  beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

  it("serves capabilities from the web fixture", async () => {
    const { capabilities, loadCapabilities } = await import("./capabilities-store");
    const { WEB_HOST_OPS_PRINTER_READY_SINGLE } = await import("./web-fixtures");
    const result = await loadCapabilities(WEB_HOST_OPS_PRINTER_READY_SINGLE);
    expect(result.printerId).toBe(WEB_HOST_OPS_PRINTER_READY_SINGLE);
    expect(capabilities.forPrinter(WEB_HOST_OPS_PRINTER_READY_SINGLE)).toEqual(result);
  });

  it("rejects an unknown Printer id with NOT_FOUND", async () => {
    const { loadCapabilities } = await import("./capabilities-store");
    await expect(loadCapabilities("prn-unknown")).rejects.toMatchObject({ code: "NOT_FOUND" });
  });
});

describe("syncCapabilities", () => {
  async function flush(): Promise<void> {
    for (let i = 0; i < 20; i += 1) await Promise.resolve();
  }

  it("loads each Printer's capabilities, refetches when its status changes, and not on a telemetry-only update", async () => {
    const requested: string[] = [];
    responders.printer_capabilities = (args) => {
      requested.push(args.printerId as string);
      return printerCapabilities({ printerId: args.printerId as string });
    };
    const [list, setList] = createStore<{ id: string; runtimeStatus?: PrinterStatus }[]>([
      { id: "prn-1", runtimeStatus: printerStatus("ready") },
      { id: "prn-2" },
    ]);
    const { capabilities, syncCapabilities } = await import("./capabilities-store");
    const stop = syncCapabilities(() => list);
    await flush();
    expect(requested.sort()).toEqual(["prn-1", "prn-2"]);
    expect(capabilities.forPrinter("prn-1")).toBeDefined();

    requested.length = 0;
    setList(0, "runtimeStatus", printerStatus("ready", "fresh", { telemetry: { hostActivity: "idle", nozzleTempC: 210 } }));
    await flush();
    expect(requested).toEqual([]);

    setList(0, "runtimeStatus", printerStatus("offline", "fresh", { connectionState: "offline" }));
    await flush();
    expect(requested).toEqual(["prn-1"]);

    setList(1, "runtimeStatus", printerStatus("ready"));
    await flush();
    expect(requested).toEqual(["prn-1", "prn-2"]);

    stop();
    setList(1, "runtimeStatus", printerStatus("printing"));
    await flush();
    expect(requested).toEqual(["prn-1", "prn-2"]);
  });

  it("forgets a Printer that is gone, and keeps the last capabilities when a refetch fails", async () => {
    let fail = false;
    responders.printer_capabilities = (args) => {
      if (fail) throw { contractVersion: 1, code: "PERSISTENCE_UNAVAILABLE", message: "busy", recovery: [], retryable: true };
      return printerCapabilities({ printerId: args.printerId as string });
    };
    const [list, setList] = createStore<{ id: string; runtimeStatus?: PrinterStatus }[]>([
      { id: "prn-1", runtimeStatus: printerStatus("ready") },
      { id: "prn-2", runtimeStatus: printerStatus("ready") },
    ]);
    const { capabilities, syncCapabilities } = await import("./capabilities-store");
    syncCapabilities(() => list);
    await flush();
    fail = true;
    setList(0, "runtimeStatus", printerStatus("printing"));
    await flush();
    expect(capabilities.forPrinter("prn-1")?.printerId).toBe("prn-1");

    setList((current) => current.filter((printer) => printer.id !== "prn-2"));
    await flush();
    expect(capabilities.forPrinter("prn-2")).toBeUndefined();
  });
});
