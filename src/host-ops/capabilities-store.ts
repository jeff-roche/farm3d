import { createStore } from "solid-js/store";
import { command, desktopAvailable } from "../ipc/client";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { WebHostOpsFixture } from "./web-fixtures";
import type { AdapterCapabilityRow, PrinterCapabilities } from "./types";

/** `PrinterCapabilities` per Printer, from `printer_capabilities`; and the
 *  registry's `AdapterCapabilityRow[]`, loaded once (spec "Frontend
 *  architecture", State). Neither carries a credential field — the wire
 *  types don't have one. There is no event for a capability change: the
 *  caller refetches with `refreshCapabilities` when the Printer's status
 *  changes (spec "Events"). */

interface CapabilitiesState {
  byPrinter: { [printerId: string]: PrinterCapabilities | undefined };
  adapterMatrix: AdapterCapabilityRow[];
  adapterMatrixLoaded: boolean;
}

const [state, setState] = createStore<CapabilitiesState>({
  byPrinter: {},
  adapterMatrix: [],
  adapterMatrixLoaded: false,
});

export const capabilities = {
  forPrinter: (printerId: string): PrinterCapabilities | undefined => state.byPrinter[printerId],
  adapterMatrix: (): AdapterCapabilityRow[] => state.adapterMatrix,
};

function notFound(id: string): CommandError {
  return {
    contractVersion: 1,
    code: "NOT_FOUND",
    message: id,
    recovery: [],
    retryable: false,
    details: { entityId: id },
  };
}

let webFixture: WebHostOpsFixture | undefined;

/** Web mode only: the fixtures load on first use, so they stay out of the
 *  desktop bundle's main chunk (mirrors `slicing-store.ts`). */
async function requireWebFixture(): Promise<WebHostOpsFixture> {
  if (!webFixture) {
    const { buildWebHostOpsFixture } = await import("./web-fixtures");
    webFixture ??= buildWebHostOpsFixture();
  }
  return webFixture;
}

/** Loads (or reloads) one Printer's capabilities. Rejects with `NOT_FOUND`
 *  for an unknown Printer id in web mode; the desktop command rejects the
 *  same way for a Printer that doesn't exist. */
export async function loadCapabilities(printerId: string): Promise<PrinterCapabilities> {
  if (!desktopAvailable()) {
    const fixture = await requireWebFixture();
    const found = fixture.capabilities[printerId];
    if (!found) throw notFound(printerId);
    setState("byPrinter", printerId, found);
    return found;
  }
  const result = await command("printer_capabilities", { printerId });
  setState("byPrinter", printerId, result);
  return result;
}

/** Refetches one Printer's capabilities — call this whenever that
 *  Printer's status changes (spec "Events": "the frontend refetches
 *  `printer_capabilities` when the Printer's status changes"). */
export const refreshCapabilities = loadCapabilities;

/** Loads the adapter registry's capability matrix once and caches it;
 *  pass `force: true` to reload. */
export async function loadAdapterCapabilityMatrix(force = false): Promise<AdapterCapabilityRow[]> {
  if (!force && state.adapterMatrixLoaded) return state.adapterMatrix;
  if (!desktopAvailable()) {
    const fixture = await requireWebFixture();
    setState({ adapterMatrix: fixture.adapterMatrix, adapterMatrixLoaded: true });
    return fixture.adapterMatrix;
  }
  const rows = await command("adapter_capability_matrix");
  setState({ adapterMatrix: rows, adapterMatrixLoaded: true });
  return rows;
}
