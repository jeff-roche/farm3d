import { invoke, isTauri } from "@tauri-apps/api/core";
import { createStore } from "solid-js/store";
import { resolveWebCatalogVariant } from "./printer-catalog";
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

interface WebFallbackSpec {
  id: string;
  name: string;
  group: string;
  notes: string;
  vendor: string;
  model: string;
  printerVariant: string;
}

/** `just web` seed data — no Rust backend, so this stands in for the Farm.
 *  Real catalog refs (not synthetic modelIds), resolved against the actual
 *  bundled catalog by `buildWebFallbackPrinters` below, rather than
 *  hand-typed profile numbers that could drift from it. Two Elegoo Centauri
 *  Carbons at different nozzle variants, grouped together, demonstrate
 *  grouping and rebinding; the Prusa MK4 is a second, unrelated model. */
const WEB_FALLBACK_SPECS: WebFallbackSpec[] = [
  {
    id: "prn-web-cc-1", name: "Elegoo Centauri Carbon — Bay 1", group: "Bay 1", notes: "",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.4",
  },
  {
    id: "prn-web-cc-2", name: "Elegoo Centauri Carbon — Bay 2", group: "Bay 2",
    notes: "Running a 0.6mm nozzle for coarse drafts.",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.6",
  },
  {
    id: "prn-web-mk4-1", name: "Prusa MK4 — Bay 3", group: "Bay 3", notes: "",
    vendor: "Prusa", model: "Prusa MK4", printerVariant: "0.4",
  },
];

/** A spec whose vendor/model/variant no longer resolves (a stale seed
 *  after the bundled catalog changes) is skipped rather than crashing the
 *  whole dev environment over it. */
async function buildWebFallbackPrinters(): Promise<ResolvedPrinter[]> {
  const resolved = await Promise.all(
    WEB_FALLBACK_SPECS.map(async (spec): Promise<ResolvedPrinter | null> => {
      const match = await resolveWebCatalogVariant(spec.vendor, spec.model, spec.printerVariant);
      if (!match) return null;
      return {
        id: spec.id,
        name: spec.name,
        group: spec.group,
        notes: spec.notes,
        catalogRef: match.catalogRef,
        catalogStatus: "ok",
        modelLabel: match.modelLabel,
        variantLabel: match.variantLabel,
        profile: match.profile,
        overriddenFields: [],
        inherited: {},
        profileDrift: [],
        unknownOverrideKeys: [],
        connection: null,
      };
    }),
  );
  return resolved.filter((p): p is ResolvedPrinter => p !== null);
}

export async function loadPrinters(): Promise<void> {
  setState("status", "loading");
  if (!isTauri()) {
    setState({ printers: await buildWebFallbackPrinters(), status: "ready", error: null });
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
    // The Add dialog's Brand/Model/Nozzle selects are themselves backed by
    // the real catalog in web mode now (see printer-catalog.ts), so this
    // resolves real profile data for whatever the user picked rather than
    // falling back to a generic placeholder. EMPTY_PROFILE only covers the
    // case where that lookup itself fails (e.g. the catalog fetch errored).
    const match = await resolveWebCatalogVariant(
      draft.catalogRef.vendor,
      draft.catalogRef.model,
      draft.catalogRef.printerVariant,
    );
    setState("printers", (list) => [
      ...list,
      {
        id,
        name: draft.name,
        group: draft.group ?? "",
        notes: "",
        catalogRef: draft.catalogRef,
        catalogStatus: "ok",
        modelLabel: match?.modelLabel ?? draft.catalogRef.model,
        variantLabel: match?.variantLabel ?? draft.catalogRef.variant,
        profile: match?.profile ?? EMPTY_PROFILE,
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
  if (!isTauri()) {
    // Unlike overrideField/revertField (still no-ops -- there's no per-field
    // override machinery to resolve against in web mode), a rebind has
    // somewhere real to go now: the same bundled catalog the Nozzle/variant
    // Select's own options came from. Without this, picking a variant there
    // would visibly do nothing, defeating the one thing `just web` reading
    // the real catalog was for.
    const match = await resolveWebCatalogVariant(
      catalogRef.vendor,
      catalogRef.model,
      catalogRef.printerVariant,
    );
    if (!match) return; // nothing in the catalog to rebind to; leave it as-is
    setState("printers", (p) => p.id === id, (p) => ({
      ...p,
      catalogRef: match.catalogRef,
      catalogStatus: "ok",
      modelLabel: match.modelLabel,
      variantLabel: match.variantLabel,
      profile: match.profile,
      overriddenFields: [],
      inherited: {},
      profileDrift: [],
      unknownOverrideKeys: [],
    }));
    return;
  }
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
