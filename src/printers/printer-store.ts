import { createSignal } from "solid-js";
import { createStore, produce } from "solid-js/store";
import { command, desktopAvailable, isCommandError } from "../ipc/client";
import type { CreatePrintersBatchInput } from "../generated/contracts/command/CreatePrintersBatchInput";
import type { CreatePrintersBatchOutput } from "../generated/contracts/command/CreatePrintersBatchOutput";
import type { BatchRowResult } from "../generated/contracts/command/BatchRowResult";
import type { PrinterRecord } from "../generated/contracts/domain/PrinterRecord";
import type { JsonValue } from "../generated/contracts/command/JsonValue";
import type { PrintersExportOutcome } from "../generated/contracts/command/PrintersExportOutcome";
import type { PrintersImportOutcome } from "../generated/contracts/command/PrintersImportOutcome";
import { createPrinterStatusStore, type StatusEvent } from "./printer-status-store";
import { resolveWebCatalogVariant } from "./printer-catalog";
import type {
  CatalogRef,
  ConnectionSubmission,
  CreatePrinterOptions,
  CredentialStoreInfo,
  DiscoveredPrinter,
  LifecycleEligibility,
  MaterialSlot,
  OverridableField,
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
  /** Explains Printers archived automatically for sharing a host (D3). */
  archiveNotice: { message: string; dismissKeys: string[] } | null;
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
  archiveNotice: null,
});
let statusStore: ReturnType<typeof createPrinterStatusStore> | undefined;
const [statusStoreRevision, setStatusStoreRevision] = createSignal(0);

/** Reactive getter — read inside JSX/createMemo for Solid to track it. */
export const printers = () => state.printers;
export const printerStoreStatus = () => state.status;
export const printerStoreError = () => state.error;
export const printerStoreRetryable = () => state.retryable;
export const printerArchiveNotice = () => state.archiveNotice?.message ?? null;
export const printerStatusSyncState = () => {
  statusStoreRevision();
  return statusStore?.syncState() ?? "syncing";
};

/**
 * Mutations report failures into `state.error` (surfaced by App's banner)
 * rather than rejecting: every call site does `void fn(...)`, so a rethrow
 * would only relocate the unhandled rejection.
 *
 * Exported for the handful of callers (currently just
 * `PrinterConnectionPanel`'s replace-connection save) whose own mutation
 * function rejects instead -- because its caller needs the rejection to
 * offer something the banner can't (Ruling R2's "Save anyway") -- but who
 * still want a rejection they don't otherwise handle to land in the same
 * banner every other mutation uses.
 */
export function reportError(e: unknown): void {
  setState({
    error: isCommandError(e) ? e.message : "The operation could not be completed.",
    retryable: isCommandError(e) && e.retryable,
  });
}

export function dismissPrinterStoreError(): void {
  setState({ error: null, retryable: false });
}

const DISMISSED_ARCHIVES_KEY = "farm3d:dismissed-duplicate-host-archives";

function dismissedArchiveIds(): Set<string> {
  try {
    const parsed: unknown = JSON.parse(window.localStorage.getItem(DISMISSED_ARCHIVES_KEY) ?? "[]");
    return new Set(Array.isArray(parsed) ? parsed.filter((id) => typeof id === "string") : []);
  } catch {
    return new Set();
  }
}

function archiveNoticeMessage(cause: string, names: string[]): string {
  const one = names.length === 1;
  return (
    `Archived ${cause} because ${one ? "it shares" : "they share"} a host with another Printer: ${names.join(", ")}. ` +
    `Change ${one ? "its host" : "their hosts"}, then unarchive ${one ? "it" : "them"}.`
  );
}

/** Dismisses the archive notice. A notice from the upgrade stays dismissed
 *  across restarts (its ledger ids are remembered in localStorage). */
export function dismissPrinterArchiveNotice(): void {
  const keys = state.archiveNotice?.dismissKeys ?? [];
  if (keys.length > 0) {
    try {
      const dismissed = dismissedArchiveIds();
      for (const key of keys) dismissed.add(key);
      window.localStorage.setItem(DISMISSED_ARCHIVES_KEY, JSON.stringify([...dismissed]));
    } catch {
      // Storage unavailable: the notice returns next launch, which is harmless.
    }
  }
  setState({ archiveNotice: null });
}

/** Reads the Printers the v3 upgrade archived for sharing a host and, for
 *  those still archived and not yet dismissed, raises the archive notice.
 *  Advisory only: a failed read is ignored rather than raised as an error. */
export async function loadDuplicateHostArchives(): Promise<void> {
  if (!desktopAvailable()) return;
  let archives;
  try {
    archives = await command("list_duplicate_host_archives");
  } catch {
    return;
  }
  const dismissed = dismissedArchiveIds();
  const byId = new Map(state.printers.map((printer) => [printer.id, printer]));
  const pending = archives.filter(
    (archive) => !dismissed.has(archive.warningId) && byId.get(archive.archivedPrinterId)?.archivedAt,
  );
  if (pending.length === 0) return;
  const names = pending.map((archive) => {
    const name = byId.get(archive.archivedPrinterId)!.name;
    const kept = byId.get(archive.keptPrinterId)?.name;
    return kept ? `${name} (same host as ${kept})` : name;
  });
  setState({
    archiveNotice: {
      message: archiveNoticeMessage("during the upgrade", names),
      dismissKeys: pending.map((archive) => archive.warningId),
    },
  });
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

/** D4/D12: web mode has no Rust repository to insert a real layout, so
 *  every web-only Printer gets the same single-slot `[Main]` layout
 *  `create_printer`/batch create default to on the desktop. */
function defaultWebMaterialSlots(seedId: string): MaterialSlot[] {
  return [{ id: `slt-web-${seedId}`, position: 0, name: "Main" }];
}

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
        startSafety: "confirmBedClear",
        materialSlots: defaultWebMaterialSlots(spec.id),
        setupGaps: [],
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

/** Single-step create (spec D1/D5/D9) — can also carry a Connection, start
 *  safety, and a shared bed-type override up front. */
export async function createPrinter(options: CreatePrinterOptions): Promise<ResolvedPrinter | undefined> {
  if (!desktopAvailable()) {
    const id = `prn-web-${state.printers.length + 1}`;
    const match = await resolveWebCatalogVariant(
      options.catalogRef.vendor,
      options.catalogRef.model,
      options.catalogRef.printerVariant,
    );
    const resolved: ResolvedPrinter = {
      id,
      revision: 1,
      name: options.name,
      notes: "",
      overrides: {},
      catalogRef: options.catalogRef,
      catalogStatus: "ok",
      modelLabel: match?.modelLabel ?? options.catalogRef.model,
      variantLabel: match?.variantLabel ?? options.catalogRef.variant,
      profile: match?.profile ?? EMPTY_PROFILE,
      overriddenFields: [],
      inherited: {},
      profileDrift: [],
      unknownOverrideKeys: [],
      location: options.location,
      startSafety: options.startSafety ?? "confirmBedClear",
      materialSlots: defaultWebMaterialSlots(id),
      setupGaps: options.connection ? [] : ["missingConnection"],
      createdAt: "",
      updatedAt: "",
      ...(options.connection
        ? {
            connection: {
              kind: options.connection.kind,
              host: options.connection.host,
              port: options.connection.port,
              useTls: options.connection.useTls,
            },
          }
        : {}),
    };
    setState("printers", (list) => [...list, resolved]);
    return resolved;
  }
  try {
    const { printer } = await command("create_printer", options);
    const resolved = resolvePrinterRecord(printer);
    setState("printers", (list) => [...list, resolved]);
    return resolved;
  } catch (e) {
    reportError(e);
    return undefined;
  }
}

export async function updatePrinter(id: string, patch: PrinterPatch): Promise<void> {
  if (!desktopAvailable()) {
    // PrinterPatch.location is `string | null` (null clears it), but
    // ResolvedPrinter.location is `string | undefined` — map null to
    // undefined so the optimistic merge below stays assignable.
    const { location, ...rest } = patch;
    setState("printers", (p) => p.id === id, (p) => ({
      ...p,
      ...rest,
      ...(location !== undefined ? { location: location ?? undefined } : {}),
    }));
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

export type RemovePrinterResult = { ok: true } | { ok: false; message: string };

/** Resolves (never rejects) with whether the delete committed, so its one
 *  caller -- `DeletePrinterDialog` -- can keep itself open and show the
 *  failure inline instead of closing over an unchanged row. */
export async function removePrinter(id: string): Promise<RemovePrinterResult> {
  if (!desktopAvailable()) {
    removeById(id);
    return { ok: true };
  }
  try {
    await command("delete_printer", { id, expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1 });
    removeById(id);
    return { ok: true };
  } catch (e) {
    return {
      ok: false,
      message: isCommandError(e) ? e.message : "The Printer could not be deleted.",
    };
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
      catalogStatus: "ok" as const,
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
      const archivedIds = new Set(
        result.warnings.filter((warning) => warning.code === "DUPLICATE_HOST_ARCHIVED").map((warning) => warning.entityId),
      );
      const names = result.printers.filter((record) => archivedIds.has(record.id)).map((record) => record.name);
      if (names.length > 0) {
        setState({ archiveNotice: { message: archiveNoticeMessage("on import", names), dismissKeys: [] } });
      }
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
    setStatusStoreRevision((revision) => revision + 1);
    return () => {
      store.dispose();
      if (statusStore === store) statusStore = undefined;
      setStatusStoreRevision((revision) => revision + 1);
    };
  } catch {
    statusStore?.dispose();
    statusStore = undefined;
    setStatusStoreRevision((revision) => revision + 1);
    throw new Error("Printer status monitoring could not start.");
  }
}

/** Rejects rather than reporting into the banner (Ruling R2): when a
 *  Connection is being replaced, the backend probes the new submission
 *  first, and a probe failure must let the caller offer "Save anyway"
 *  (`acceptUnverified: true`) instead of the failure silently only showing
 *  up in the banner. `PrinterConnectionPanel` currently catches this itself
 *  and routes it to `reportError` so its own behaviour is unchanged until
 *  Task 11 adds the inline "Save anyway" affordance. */
export async function setConnection(
  id: string,
  submission: ConnectionSubmission,
  acceptUnverified?: boolean,
): Promise<void> {
  if (!desktopAvailable()) return;
  const { printer } = await command("set_printer_connection", {
    id,
    expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1,
    submission,
    ...(acceptUnverified !== undefined ? { acceptUnverified } : {}),
  });
  spliceResolved(resolvePrinterRecord(printer));
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

/** Rejects rather than reporting into the banner: the wizard/batch dialog's
 *  Connect step renders a probe failure inline. Unlike `testConnection`,
 *  there is no Printer id yet -- this probes a candidate submission before
 *  any Printer exists (spec D8). */
export async function probeCandidate(submission: ConnectionSubmission): Promise<ProbeResult> {
  if (!desktopAvailable()) throw new Error("Probing a printer needs the desktop app");
  return command("probe_connection", { submission });
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

/** Upserts by id: every printer a batch result carries is newly created, so
 *  this is normally an append, but it stays id-keyed rather than a plain
 *  push so a retried/duplicated response can't double a row. */
function mergeCreatedPrinters(records: ResolvedPrinter[]): void {
  if (records.length === 0) return;
  setState("printers", (list) => {
    const byId = new Map(list.map((printer) => [printer.id, printer]));
    for (const record of records) byId.set(record.id, record);
    return [...byId.values()];
  });
}

/** The `PrinterRecord` shape of a web-mode `ResolvedPrinter`, for results
 *  that carry a `printer` (e.g. the batch web shim). */
function toWebRecord(printer: ResolvedPrinter): PrinterRecord {
  const {
    catalogStatus,
    modelLabel,
    variantLabel,
    profile,
    overriddenFields,
    inherited,
    profileDrift,
    unknownOverrideKeys,
    runtimeStatus: _runtimeStatus, // live-only; not part of the durable record
    ...record
  } = printer;
  return {
    ...record,
    profileResolution: {
      catalogStatus,
      modelLabel,
      variantLabel,
      profile,
      overriddenFields,
      inherited,
      profileDrift,
      unknownOverrideKeys,
    },
  };
}

/** Rejects rather than reporting into the banner: the batch dialog's
 *  Review & results step renders per-row outcomes and errors inline, and a
 *  batch-level failure (e.g. a duplicate `batchId`) needs to reach that same
 *  step rather than the global banner. Successfully created Printers are
 *  merged into the store either way. */
export async function createPrintersBatch(
  input: CreatePrintersBatchInput,
): Promise<CreatePrintersBatchOutput> {
  if (!desktopAvailable()) {
    const created: ResolvedPrinter[] = [];
    const rows: BatchRowResult[] = [];
    for (const row of input.rows) {
      const match = await resolveWebCatalogVariant(
        input.shared.catalogRef.vendor,
        input.shared.catalogRef.model,
        input.shared.catalogRef.printerVariant,
      );
      const id = `prn-web-${state.printers.length + created.length + 1}`;
      const resolved: ResolvedPrinter = {
        id,
        revision: 1,
        name: row.name,
        notes: "",
        overrides: {},
        catalogRef: input.shared.catalogRef,
        catalogStatus: "ok",
        modelLabel: match?.modelLabel ?? input.shared.catalogRef.model,
        variantLabel: match?.variantLabel ?? input.shared.catalogRef.variant,
        profile: match?.profile ?? EMPTY_PROFILE,
        overriddenFields: [],
        inherited: {},
        profileDrift: [],
        unknownOverrideKeys: [],
        location: row.location,
        startSafety: input.shared.startSafety,
        materialSlots: defaultWebMaterialSlots(id),
        setupGaps: ["missingConnection"],
        createdAt: "",
        updatedAt: "",
      };
      created.push(resolved);
      rows.push({
        rowId: row.rowId,
        outcome: "createdSetupIncomplete",
        // The same local record merged below, so callers can correlate
        // row -> Printer exactly as on desktop.
        printer: toWebRecord(resolved),
        credentialStored: false,
        errors: [],
        warnings: [],
      });
    }
    mergeCreatedPrinters(created);
    return { batchId: input.batchId, rows };
  }
  const output = await command("create_printers_batch", { input });
  mergeCreatedPrinters(
    output.rows.flatMap((row) => (row.printer ? [resolvePrinterRecord(row.printer)] : [])),
  );
  return output;
}

export async function cancelBatch(batchId: string): Promise<void> {
  if (!desktopAvailable()) return;
  try {
    await command("cancel_printer_batch", { batchId });
  } catch (e) {
    reportError(e);
  }
}

export async function archivePrinter(id: string): Promise<void> {
  if (!desktopAvailable()) {
    setState("printers", (p) => p.id === id, "archivedAt", new Date().toISOString());
    return;
  }
  try {
    const { printer } = await command("archive_printer", {
      id,
      expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1,
    });
    spliceResolved(resolvePrinterRecord(printer));
  } catch (e) {
    reportError(e);
  }
}

/** Rejects rather than reporting into the banner: unarchiving re-checks the
 *  host identity (spec D3/D6), and a `DUPLICATE_HOST` failure needs to reach
 *  the Setup tab inline so it can name the conflicting Printer — the banner
 *  has no room for that. Mirrors `setConnection`'s rejection for the same
 *  reason. */
export async function unarchivePrinter(id: string): Promise<void> {
  if (!desktopAvailable()) {
    setState("printers", (p) => p.id === id, produce((printer) => {
      delete printer.archivedAt;
    }));
    return;
  }
  const { printer } = await command("unarchive_printer", {
    id,
    expectedRevision: state.printers.find((printer) => printer.id === id)?.revision ?? 1,
  });
  spliceResolved(resolvePrinterRecord(printer));
}

/** Rejects rather than reporting into the banner: the Setup tab renders
 *  blockers inline next to the Archive/Unarchive/Delete actions they
 *  explain. In web mode this derives the same shape locally from
 *  `archivedAt` (spec D7's P2 blocker source: `NOT_ARCHIVED`/
 *  `ALREADY_ARCHIVED`), since there is no backend to ask. */
export async function lifecycleEligibility(id: string): Promise<LifecycleEligibility> {
  if (!desktopAvailable()) {
    const archived = Boolean(state.printers.find((printer) => printer.id === id)?.archivedAt);
    return archived
      ? {
          canArchive: false,
          canUnarchive: true,
          canDelete: true,
          blockers: [
            { action: "archive", code: "ALREADY_ARCHIVED", message: "This Printer is already archived." },
          ],
        }
      : {
          canArchive: true,
          canUnarchive: false,
          canDelete: false,
          blockers: [
            { action: "delete", code: "NOT_ARCHIVED", message: "Archive this Printer before deleting it." },
            { action: "unarchive", code: "NOT_ARCHIVED", message: "This Printer is not archived." },
          ],
        };
  }
  return command("printer_lifecycle_eligibility", { id });
}
