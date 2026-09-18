import { createStore, produce } from "solid-js/store";
import { command, desktopAvailable, isCommandError } from "../ipc/client";
import type { JsonValue } from "../generated/contracts/command/JsonValue";
import type { PrintersExportOutcome } from "../generated/contracts/command/PrintersExportOutcome";
import type { PrintersImportOutcome } from "../generated/contracts/command/PrintersImportOutcome";
import { createPrinterStatusStore, type StatusEvent } from "./printer-status-store";
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
import { resolvePrinterRecord } from "./types";

interface PrinterStoreState {
  printers: ResolvedPrinter[];
  status: "idle" | "loading" | "ready" | "error";
  error: string | null;
  retryable: boolean;
}
function compareUtf8(left: string, right: string): number {
  const encoder = new TextEncoder();
  const leftBytes = encoder.encode(left);
  const rightBytes = encoder.encode(right);
  const length = Math.min(leftBytes.length, rightBytes.length);
  for (let index = 0; index < length; index += 1) {
    if (leftBytes[index] !== rightBytes[index]) return leftBytes[index] - rightBytes[index];
  }
  return leftBytes.length - rightBytes.length;
}

const [state, setState] = createStore<PrinterStoreState>({
  printers: [],
  status: "idle",
  error: null,
  retryable: false,
});
let statusStore: ReturnType<typeof createPrinterStatusStore> | undefined;

/** Reactive getter — read inside JSX/createMemo for Solid to track it. */
export const printers = () => state.printers;
export const printerStoreStatus = () => state.status;
export const printerStoreError = () => state.error;
export const printerStoreRetryable = () => state.retryable;
export const printerStatusSyncState = () => statusStore?.syncState() ?? "syncing";

/**
 * Mutations report failures into `state.error` (surfaced by App's banner)
 * rather than rejecting: every call site does `void fn(...)`, so a rethrow
 * would only relocate the unhandled rejection.
 */
function reportError(e: unknown): void {
  setState({
    error: isCommandError(e) ? e.message : "The operation could not be completed.",
    retryable: isCommandError(e) && e.retryable,
  });
}

export function dismissPrinterStoreError(): void {
  setState({ error: null, retryable: false });
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
    id: "prn-web-cc-1", name: "Elegoo Centauri Carbon — Bay 1", notes: "",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.4",
  },
  {
    id: "prn-web-cc-2", name: "Elegoo Centauri Carbon — Bay 2",
    notes: "Running a 0.6mm nozzle for coarse drafts.",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.6",
  },
  {
    id: "prn-web-mk4-1", name: "Prusa MK4 — Bay 3", notes: "",
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
        revision: 1,
        name: spec.name,
        notes: spec.notes,
        overrides: {},
        catalogRef: match.catalogRef,
        catalogStatus: "ok",
        modelLabel: match.modelLabel,
        variantLabel: match.variantLabel,
        profile: match.profile,
        overriddenFields: [],
        inherited: {},
        profileDrift: [],
        unknownOverrideKeys: [],
        createdAt: "",
        updatedAt: "",
      };
    }),
  );
  return resolved.filter((p): p is ResolvedPrinter => p !== null);
}

export async function loadPrinters(): Promise<void> {
  setState("status", "loading");
  if (!desktopAvailable()) {
    setState({ printers: await buildWebFallbackPrinters(), status: "ready", error: null, retryable: false });
    return;
  }
  try {
    const loaded = (await command("list_printers")).map(resolvePrinterRecord);
    setState({ printers: loaded, status: "ready", error: null, retryable: false });
    statusStore?.prune();
  } catch (e) {
    setState({
      status: "error",
      error: isCommandError(e) ? e.message : "farm3d could not finish starting.",
      retryable: isCommandError(e) && e.retryable,
    });
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
  statusStore?.prune();
}

export async function addPrinter(draft: PrinterDraft): Promise<string | undefined> {
  if (!desktopAvailable()) {
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
        revision: 1,
        name: draft.name,
        notes: "",
        overrides: {},
        catalogRef: draft.catalogRef,
        catalogStatus: "ok",
        modelLabel: match?.modelLabel ?? draft.catalogRef.model,
        variantLabel: match?.variantLabel ?? draft.catalogRef.variant,
        profile: match?.profile ?? EMPTY_PROFILE,
        overriddenFields: [],
        inherited: {},
        profileDrift: [],
        unknownOverrideKeys: [],
        createdAt: "",
        updatedAt: "",
      },
    ]);
    return id;
  }
  try {
    const { printer } = await command("create_printer", draft);
    const resolved = resolvePrinterRecord(printer);
    setState("printers", (list) => [...list, resolved]);
    return resolved.id;
  } catch (e) {
    reportError(e);
    return undefined;
  }
}

export async function updatePrinter(id: string, patch: PrinterPatch): Promise<void> {
  if (!desktopAvailable()) {
    setState("printers", (p) => p.id === id, (p) => ({ ...p, ...patch }));
    return;
  }
  try {
    const { printer } = await command("update_printer", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1, patch });
    const resolved = resolvePrinterRecord(printer);
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function removePrinter(id: string): Promise<void> {
  if (!desktopAvailable()) {
    removeById(id);
    return;
  }
  try {
    await command("delete_printer", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1 });
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
  value: JsonValue,
): Promise<void> {
  if (!desktopAvailable()) return; // web fallback has no catalog to resolve overrides against
  try {
    const { printer } = await command("set_printer_override", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1, field, value });
    const resolved = resolvePrinterRecord(printer);
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function revertField(id: string, field: OverridableField): Promise<void> {
  if (!desktopAvailable()) return;
  try {
    const { printer } = await command("set_printer_override", {
      id,
      expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1,
      field,
      value: null,
    });
    spliceResolved(resolvePrinterRecord(printer));
  } catch (e) {
    reportError(e);
  }
}

export async function rebindPrinter(id: string, catalogRef: CatalogRef): Promise<void> {
  if (!desktopAvailable()) {
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
    const { printer } = await command("rebind_printer", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1, catalogRef });
    const resolved = resolvePrinterRecord(printer);
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function resolveDrift(id: string, action: "accept" | "pin"): Promise<void> {
  if (!desktopAvailable()) return;
  try {
    const { printer } = await command("resolve_profile_drift", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1, action });
    const resolved = resolvePrinterRecord(printer);
    spliceResolved(resolved);
  } catch (e) {
    reportError(e);
  }
}

export async function exportPrinters(): Promise<PrintersExportOutcome | undefined> {
  if (!desktopAvailable()) return { status: "unsupported", reason: "desktopRequired" };
  try {
    return await command("export_printers");
  } catch (e) {
    reportError(e);
  }
}

export async function importPrinters(): Promise<PrintersImportOutcome | undefined> {
  if (!desktopAvailable()) return { status: "unsupported", reason: "desktopRequired" };
  try {
    const result = await command("import_printers", {
      expectedRevisions: state.printers
        .map((printer) => ({ id: printer.id, revision: printer.revision ?? 1 }))
        .sort((left, right) => compareUtf8(left.id, right.id)),
    });
    if (result.status === "applied") {
      const runtimeStatuses = new Map(state.printers.map((printer) => [printer.id, printer.runtimeStatus]));
      setState("printers", result.printers.map((record) => {
        const resolved = resolvePrinterRecord(record);
        const runtimeStatus = runtimeStatuses.get(resolved.id);
        return runtimeStatus === undefined ? resolved : { ...resolved, runtimeStatus };
      }));
      statusStore?.prune();
    }
    return result;
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

export function removeStatus(id: string): void {
  setState("printers", (printer) => printer.id === id, produce((printer) => {
    delete printer.runtimeStatus;
  }));
}

/** Subscribes to the supervisor's status events, then backfills whatever it
 *  already knows — a printer that came online before this listener attached
 *  would otherwise show nothing until its next change. Returns an unlisten fn. */
export async function startStatusListener(): Promise<() => void> {
  if (!desktopAvailable()) return () => {};
  try {
    const { listen } = await import("@tauri-apps/api/event");
    const store = createPrinterStatusStore({
      printerIds: () => state.printers.map((printer) => printer.id),
      listen: async (handler) => listen<StatusEvent>("farm3d-event-v1", (event) => handler(event.payload)),
      backfill: () => command("printer_statuses"),
      onStatus: applyStatus,
      onStatusRemoved: removeStatus,
    });
    statusStore?.dispose();
    statusStore = store;
    await store.start();
    return () => {
      store.dispose();
      if (statusStore === store) statusStore = undefined;
    };
  } catch {
    statusStore?.dispose();
    statusStore = undefined;
    throw new Error("Printer status monitoring could not start.");
  }
}

export async function setConnection(id: string, submission: ConnectionSubmission): Promise<void> {
  if (!desktopAvailable()) return;
  try {
    spliceResolved(resolvePrinterRecord((await command("set_printer_connection", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1, submission })).printer));
  } catch (e) {
    reportError(e);
  }
}

export async function clearConnection(id: string): Promise<void> {
  if (!desktopAvailable()) return;
  try {
    spliceResolved(resolvePrinterRecord((await command("clear_printer_connection", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1 })).printer));
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
  if (!desktopAvailable()) throw new Error("Testing a connection needs the desktop app");
  return command("test_printer_connection", { id, submission });
}

export async function discoverPrinters(): Promise<DiscoveredPrinter[]> {
  if (!desktopAvailable()) return [];
  try {
    return await command("discover_printers");
  } catch (e) {
    reportError(e);
    throw e;
  }
}

export async function credentialStoreInfo(): Promise<CredentialStoreInfo | null> {
  if (!desktopAvailable()) return null;
  try {
    return await command("credential_store_info");
  } catch (e) {
    reportError(e);
    return null;
  }
}
