import { createEffect, createRoot, onCleanup } from "solid-js";
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

/** How long after an Online transition to refetch again while the
 *  backend's host-facts refresh hasn't landed (about 15 s in all, which
 *  covers the host read's connect and query timeouts). */
const HOST_FACTS_RETRY_MS = [500, 1_000, 2_000, 4_000, 8_000];
/** `observedAt` has one-second precision, and the backend saw the Online
 *  transition before this frontend did. */
const OBSERVED_AT_SLACK_MS = 2_000;

/** Whether `result` carries host facts read since the Printer came Online
 *  at `onlineAt` (a `Date.now()` value). */
function hasFactsSince(result: PrinterCapabilities, onlineAt: number): boolean {
  if (!result.hostFacts || !result.observedAt) return false;
  const observedAt = Date.parse(result.observedAt);
  return Number.isFinite(observedAt) && observedAt >= onlineAt - OBSERVED_AT_SLACK_MS;
}

/** Keeps every listed Printer's capabilities loaded: fetches each once,
 *  refetches one whenever its status changes (spec "Events": there is no
 *  capability event), and forgets a Printer that leaves the list. A failed
 *  fetch keeps what was held; the next status change tries again.
 *
 *  When a Printer comes Online (or is first seen Online) the backend
 *  re-reads its host facts in the background (D6), so the refetch that the
 *  status change triggers can land first and see no facts, or the previous
 *  Online's. Until a fetch carries facts observed since that transition,
 *  this refetches on a short, bounded backoff; any later status change
 *  supersedes it. Returns a disposer. */
export function syncCapabilities(
  list: () => ReadonlyArray<{ id: string; runtimeStatus?: PrinterStatus }>,
): () => void {
  return createRoot((dispose) => {
    const seen = new Map<string, string>();
    /** Bumped on every refetch a status change starts, so an older
     *  Online's retries stop. */
    const generation = new Map<string, number>();
    const timers = new Set<ReturnType<typeof setTimeout>>();
    let disposed = false;
    onCleanup(() => {
      disposed = true;
      for (const timer of timers) clearTimeout(timer);
      timers.clear();
    });

    const current = (id: string, token: number) => !disposed && generation.get(id) === token;

    function untilFactsLand(id: string, token: number, onlineAt: number, attempt = 0): void {
      const retry = () => {
        const delay = HOST_FACTS_RETRY_MS[attempt];
        if (delay === undefined || !current(id, token)) return;
        const timer = setTimeout(() => {
          timers.delete(timer);
          if (current(id, token)) untilFactsLand(id, token, onlineAt, attempt + 1);
        }, delay);
        timers.add(timer);
      };
      loadCapabilities(id).then(
        (result) => {
          if (!hasFactsSince(result, onlineAt)) retry();
        },
        retry,
      );
    }

    createEffect(() => {
      const present = new Set<string>();
      for (const printer of list()) {
        present.add(printer.id);
        const key = statusKey(printer.runtimeStatus);
        const previous = seen.get(printer.id);
        if (previous === key) continue;
        seen.set(printer.id, key);
        const token = (generation.get(printer.id) ?? 0) + 1;
        generation.set(printer.id, token);
        const cameOnline =
          printer.runtimeStatus?.connectionState === "online" && !previous?.startsWith("online/");
        if (cameOnline && desktopAvailable()) {
          untilFactsLand(printer.id, token, Date.now());
        } else {
          loadCapabilities(printer.id).catch(() => {});
        }
      }
      for (const id of [...seen.keys()]) {
        if (present.has(id)) continue;
        seen.delete(id);
        // Bumped, not deleted: a Printer that comes back starts past any
        // retry still pending from before.
        generation.set(id, (generation.get(id) ?? 0) + 1);
        setState("byPrinter", produce((byPrinter) => { delete byPrinter[id]; }));
      }
    });
    return dispose;
  });
}
