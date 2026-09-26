/** A stand-in for `capabilities-store` in screen tests. Test-only: nothing
 *  outside a test imports this module. Use it as
 *
 *    vi.mock("../host-ops/capabilities-store", async () =>
 *      (await import("../host-ops/capabilities-store-mock")).capabilitiesStoreMock);
 *
 *  The read side is a real Solid store, so components react when a test
 *  calls `setPrinterCapabilitiesForTest`; the actions are spies. */
import { createStore, produce, reconcile } from "solid-js/store";
import { vi } from "vitest";
import type { AdapterCapabilityRow, PrinterCapabilities, PrinterStatus } from "./types";

interface MockCapabilitiesState {
  byPrinter: { [printerId: string]: PrinterCapabilities | undefined };
  adapterMatrix: AdapterCapabilityRow[];
}

const [state, setState] = createStore<MockCapabilitiesState>({ byPrinter: {}, adapterMatrix: [] });

export const capabilitiesStoreMock = {
  capabilities: {
    forPrinter: (printerId: string) => state.byPrinter[printerId],
    adapterMatrix: () => state.adapterMatrix,
  },
  loadCapabilities: vi.fn(async (printerId: string): Promise<PrinterCapabilities> => {
    const held = state.byPrinter[printerId];
    if (!held) throw { contractVersion: 1, code: "NOT_FOUND", message: printerId, recovery: [], retryable: false };
    return held;
  }),
  refreshCapabilities: vi.fn(async (printerId: string): Promise<PrinterCapabilities | undefined> => state.byPrinter[printerId]),
  loadAdapterCapabilityMatrix: vi.fn(async (): Promise<AdapterCapabilityRow[]> => state.adapterMatrix),
  syncCapabilities: vi.fn((_list: () => ReadonlyArray<{ id: string; runtimeStatus?: PrinterStatus }>) => () => {}),
};

export function setPrinterCapabilitiesForTest(record: PrinterCapabilities): void {
  setState("byPrinter", record.printerId, reconcile(record));
}

export function resetCapabilitiesStoreMock(): void {
  setState("byPrinter", produce((byPrinter) => {
    for (const id of Object.keys(byPrinter)) delete byPrinter[id];
  }));
  setState("adapterMatrix", []);
  for (const action of Object.values(capabilitiesStoreMock)) {
    if (typeof action === "function" && "mockClear" in action) action.mockClear();
  }
}
