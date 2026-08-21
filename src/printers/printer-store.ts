import { invoke, isTauri } from "@tauri-apps/api/core";
import { createStore } from "solid-js/store";
import type {
  CatalogRef,
  ConnectionSubmission,
  CredentialStoreInfo,
  DiscoveredPrinter,
  OverridableField,
  PrinterDraft,
  PrinterPatch,
  PrinterProfile,
  PrinterStatus,
  ProbeResult,
  ResolvedPrinter,
} from "./types";

interface PrinterStoreState {
  printers: ResolvedPrinter[];
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
}

const [state, setState] = createStore<PrinterStoreState>({
  printers: [],
  status: "idle",
  error: null,
});

/** Reactive getter — read inside JSX/createMemo for Solid to track it. */
export const printers = () => state.printers;
export const printerStoreStatus = () => state.status;
export const printerStoreError = () => state.error;

/**
 * Mutations report failures into `state.error` (surfaced by App's banner)
 * rather than rejecting: every call site does `void fn(...)`, so a rethrow
 * would only relocate the unhandled rejection.
 */
function reportError(e: unknown): void {
  setState("error", String(e));
}

export function dismissPrinterStoreError(): void {
  setState("error", null);
}

const EMPTY_PROFILE: PrinterProfile = {
  bedShape: { kind: "rectangular", widthMm: 220, depthMm: 220, originXMm: 0, originYMm: 0 },
  printableHeightMm: 250,
  bedExcludeAreas: [],
  defaultBedType: "1",
  nozzleDiameterMm: [0.4],
  nozzleType: "brass",
  gcodeFlavor: "marlin",
  hasAuxiliaryFan: false,
  supportsAirFiltration: false,
  supportsMultiFilament: false,
  suggestedHostType: null,
};

/** `just web` seed data — no Rust backend, so this stands in for both the
 *  Farm and the catalog. Shaped after the pre-catalog mock in App.tsx. */
const WEB_FALLBACK_PRINTERS: ResolvedPrinter[] = [
  {
    id: "prn-voron-1",
    name: "Voron 2.4 — Bay 1",
    group: "Bay 1",
    notes: "",
    catalogRef: {
      vendor: "Voron", model: "Voron 2.4", variant: "Voron 2.4 0.4 nozzle",
      modelId: "web-voron-24", printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "Voron 2.4",
    variantLabel: "Voron 2.4 0.4 nozzle",
    profile: EMPTY_PROFILE,
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    connection: null,
  },
  {
    id: "prn-prusa-1",
    name: "Prusa MK4 — Bay 2",
    group: "Bay 2",
    notes: "",
    catalogRef: {
      vendor: "Prusa", model: "Prusa MK4", variant: "Prusa MK4 0.4 nozzle",
      modelId: "web-prusa-mk4", printerVariant: "0.4",
    },
    catalogStatus: "ok",
    modelLabel: "Prusa MK4",
    variantLabel: "Prusa MK4 0.4 nozzle",
    profile: EMPTY_PROFILE,
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    connection: null,
  },
];

export async function loadPrinters(): Promise<void> {
  setState("status", "loading");
  if (!isTauri()) {
    setState({ printers: WEB_FALLBACK_PRINTERS, status: "ready", error: null });
    return;
  }
  try {
    const loaded = await invoke<ResolvedPrinter[]>("list_printers");
    setState({ printers: loaded, status: "ready", error: null });
  } catch (e) {
    setState({ status: "error", error: String(e) });
  }
}

/** `runtimeStatus` is frontend-only live state that Rust never sends back, so
 *  it must survive every mutation that splices in a fresh `ResolvedPrinter` —
 *  otherwise a rename would blank the connection badge and temperatures until
 *  the supervisor's next push, up to a minute of backoff away for an offline
 *  printer. Solid's store merges an object at a path (absent keys are left
 *  alone) so this already held, but it held by accident of that merge; carried
 *  forward explicitly here, and pinned by a test, so it stays true. */
function spliceResolved(resolved: ResolvedPrinter): void {
  setState(
    "printers",
    (p) => p.id === resolved.id,
    (previous) => ({ ...resolved, runtimeStatus: previous.runtimeStatus }),
  );
}

function removeById(id: string): void {
  setState("printers", (list) => list.filter((p) => p.id !== id));
}

export async function addPrinter(draft: PrinterDraft): Promise<string | undefined> {
  if (!isTauri()) {
    const id = `prn-web-${state.printers.length + 1}`;
    setState("printers", (list) => [
      ...list,
      {
        id,
        name: draft.name,
        group: draft.group ?? "",
        notes: "",
        catalogRef: draft.catalogRef,
        catalogStatus: "ok",
        modelLabel: draft.catalogRef.model,
        variantLabel: draft.catalogRef.variant,
        profile: EMPTY_PROFILE,
        overriddenFields: [],
        inherited: {},
        profileDrift: [],
        unknownOverrideKeys: [],
        connection: null,
      },
    ]);
    return id;
  }
  try {
    const resolved = await invoke<ResolvedPrinter>("create_printer", { draft });
    setState("printers", (list) => [...list, resolved]);
    return resolved.id;
  } catch (e) {
    reportError(e);
    return undefined;
  }
}

export async function updatePrinter(id: string, patch: PrinterPatch): Promise<void> {
  if (!isTauri()) {
    setState("printers", (p) => p.id === id, (p) => ({ ...p, ...patch }));
    return;
  }
  try {
    const resolved = await invoke<ResolvedPrinter>("update_printer", { id, patch });
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function removePrinter(id: string): Promise<void> {
  if (!isTauri()) {
    removeById(id);
    return;
  }
  try {
    await invoke("delete_printer", { id });
    removeById(id);
  } catch (e) {
    reportError(e);
  }
}

/** `value: undefined` is not valid here — pass a concrete value to override,
 *  or use `revertField()` to clear one. */
export async function overrideField(
  id: string,
  field: OverridableField,
  value: unknown,
): Promise<void> {
  if (!isTauri()) return; // web fallback has no catalog to resolve overrides against
  try {
    const resolved = await invoke<ResolvedPrinter>("set_printer_override", { id, field, value });
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function revertField(id: string, field: OverridableField): Promise<void> {
  if (!isTauri()) return;
  try {
    const resolved = await invoke<ResolvedPrinter>("set_printer_override", {
      id,
      field,
      value: null,
    });
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function rebindPrinter(id: string, catalogRef: CatalogRef): Promise<void> {
  if (!isTauri()) return;
  try {
    const resolved = await invoke<ResolvedPrinter>("rebind_printer", { id, catalogRef });
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function resolveDrift(id: string, action: "accept" | "pin"): Promise<void> {
  if (!isTauri()) return;
  try {
    const resolved = await invoke<ResolvedPrinter>("resolve_profile_drift", { id, action });
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function openPrintersFile(): Promise<void> {
  if (!isTauri()) return;
  try {
    await invoke("open_printers_file");
  } catch (e) {
    reportError(e);
  }
}

/** Merges live status onto one printer row. Unknown ids are ignored — a
 *  status event can arrive after a delete, and must not resurrect the row. */
export function applyStatus(id: string, status: PrinterStatus): void {
  if (!state.printers.some((p) => p.id === id)) return;
  setState("printers", (p) => p.id === id, "runtimeStatus", status);
}

/** Subscribes to the supervisor's status events, then backfills whatever it
 *  already knows — a printer that came online before this listener attached
 *  would otherwise show nothing until its next change. Returns an unlisten fn. */
export async function startStatusListener(): Promise<() => void> {
  if (!isTauri()) return () => {};
  const { listen } = await import("@tauri-apps/api/event");
  const unlisten = await listen<{ id: string; status: PrinterStatus }>(
    "printer-status",
    (event) => applyStatus(event.payload.id, event.payload.status),
  );
  try {
    const known = await invoke<Record<string, PrinterStatus>>("printer_statuses");
    for (const [id, status] of Object.entries(known)) applyStatus(id, status);
  } catch (e) {
    reportError(e);
  }
  return unlisten;
}

export async function setConnection(id: string, submission: ConnectionSubmission): Promise<void> {
  if (!isTauri()) return;
  try {
    spliceResolved(await invoke<ResolvedPrinter>("set_printer_connection", { id, submission }));
  } catch (e) {
    reportError(e);
  }
}

export async function clearConnection(id: string): Promise<void> {
  if (!isTauri()) return;
  try {
    spliceResolved(await invoke<ResolvedPrinter>("clear_printer_connection", { id }));
  } catch (e) {
    reportError(e);
  }
}

/** Rejects rather than reporting into the banner: the Connection tab renders
 *  a probe failure inline, next to the fields the user needs to correct. */
export async function testConnection(
  id: string,
  submission: ConnectionSubmission,
): Promise<ProbeResult> {
  if (!isTauri()) throw new Error("Testing a connection needs the desktop app");
  return invoke<ProbeResult>("test_printer_connection", { id, submission });
}

export async function discoverPrinters(): Promise<DiscoveredPrinter[]> {
  if (!isTauri()) return [];
  try {
    return await invoke<DiscoveredPrinter[]>("discover_printers");
  } catch (e) {
    reportError(e);
    return [];
  }
}

export async function credentialStoreInfo(): Promise<CredentialStoreInfo | null> {
  if (!isTauri()) return null;
  try {
    return await invoke<CredentialStoreInfo>("credential_store_info");
  } catch (e) {
    reportError(e);
    return null;
  }
}
