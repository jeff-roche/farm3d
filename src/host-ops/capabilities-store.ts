import { createEffect, createRoot } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { command, desktopAvailable } from "../ipc/client";
import { notFound } from "../ipc/local-errors";
import type { WebHostOpsFixture } from "./web-fixtures";
import type { AdapterCapabilityRow, PrinterCapabilities, PrinterStatus } from "./types";

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

/** What counts as "the Printer's status changed" for capabilities: its
 *  connection (host facts are re-read on each Online transition, spec D6)
 *  or its operational state. Telemetry-only updates (temperatures,
 *  progress) don't refetch. */
function statusKey(status: PrinterStatus | undefined): string {
  return status ? `${status.connectionState}/${status.operationalState}` : "none";
}

/** Keeps every listed Printer's capabilities loaded: fetches each once,
 *  refetches one whenever its status changes (spec "Events": there is no
 *  capability event), and forgets a Printer that leaves the list. A failed
 *  fetch keeps what was held; the next status change tries again. Returns
 *  a disposer. */
export function syncCapabilities(
  list: () => ReadonlyArray<{ id: string; runtimeStatus?: PrinterStatus }>,
): () => void {
  return createRoot((dispose) => {
    const seen = new Map<string, string>();
    createEffect(() => {
      const current = new Set<string>();
      for (const printer of list()) {
        current.add(printer.id);
        const key = statusKey(printer.runtimeStatus);
        if (seen.get(printer.id) === key) continue;
        seen.set(printer.id, key);
        loadCapabilities(printer.id).catch(() => {});
      }
      for (const id of [...seen.keys()]) {
        if (current.has(id)) continue;
        seen.delete(id);
        setState("byPrinter", produce((byPrinter) => { delete byPrinter[id]; }));
      }
    });
    return dispose;
  });
}
