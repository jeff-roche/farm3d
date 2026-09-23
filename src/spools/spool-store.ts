import { createStore, produce } from "solid-js/store";
import { command, desktopAvailable, isCommandError } from "../ipc/client";
import { printers as printerRecords, spliceResolved, WEB_FIXTURE_EQUIPPED_PRINTER_ID } from "../printers/printer-store";
import { resolvePrinterRecord } from "../printers/types";
import { buildWebInventoryFixture } from "./web-fixtures";
import type { AmountConfidence } from "../generated/contracts/domain/AmountConfidence";
import type { AmountEntry } from "../generated/contracts/domain/AmountEntry";
import type { AmountEvent } from "../generated/contracts/domain/AmountEvent";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { ErrorCode } from "../generated/contracts/command/ErrorCode";
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { InventoryEventPayload } from "../generated/contracts/domain/InventoryEventPayload";
import type { InventoryEventType } from "../generated/contracts/domain/InventoryEventType";
import type { JsonValue } from "../generated/contracts/command/JsonValue";
import type { MaterialSlot } from "../generated/contracts/domain/MaterialSlot";
import type { MoveDestination } from "../generated/contracts/domain/MoveDestination";
import type { MoveSpoolData } from "../generated/contracts/command/MoveSpoolData";
import type { MoveSpoolRequest } from "../generated/contracts/command/CommandContracts";
import type { Reservation } from "../generated/contracts/domain/Reservation";
import type { SpoolFields } from "../generated/contracts/domain/SpoolFields";
import type { SpoolHistory } from "../generated/contracts/command/SpoolHistory";
import type { SpoolLifecycleAction } from "../generated/contracts/domain/SpoolLifecycleAction";
import type { SpoolLocation } from "../generated/contracts/domain/SpoolLocation";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { Tare } from "../generated/contracts/domain/Tare";

type InventoryEnvelope = EventEnvelope<InventoryEventType, InventoryEventPayload>;

interface SpoolStoreState {
  spools: SpoolRecord[];
  tares: Tare[];
  loaded: boolean;
  error: string | null;
  /** Keyed by `moveSpool`'s `operationId`; the Spool ids its optimistic
   *  update touched, cleared once that move settles or reverts. */
  pending: Record<string, string[]>;
}

const [state, setState] = createStore<SpoolStoreState>({
  spools: [],
  tares: [],
  loaded: false,
  error: null,
  pending: {},
});

/** A module-level Solid store, separate from `printer-store.ts` (P3
 *  design's "Frontend architecture -> State"). Read its fields inside a
 *  tracked scope (JSX/`createMemo`) for Solid to pick up changes. */
export const spoolState = state;

export function spoolStoreError(): string | null {
  return state.error;
}

export function dismissSpoolStoreError(): void {
  setState("error", null);
}

function reportSpoolError(e: unknown): void {
  setState("error", isCommandError(e) ? e.message : "The operation could not be completed.");
}

function commandError(code: ErrorCode, message: string, details?: Record<string, JsonValue>): CommandError {
  return {
    contractVersion: 1,
    code,
    message,
    recovery: code === "CONFLICT" ? ["RETRY"] : [],
    retryable: code === "CONFLICT",
    ...(details ? { details } : {}),
  };
}

// --- Inventory listener (D11: listen-before-backfill) ----------------------

const INVENTORY_EVENT_TYPES: ReadonlySet<string> = new Set<InventoryEventType>([
  "spool.changed", "printer.slots.changed", "spool.availability.changed",
]);

function isInventoryEnvelope(value: unknown): value is InventoryEnvelope {
  return (
    typeof value === "object" &&
    value !== null &&
    "type" in value &&
    INVENTORY_EVENT_TYPES.has((value as { type: unknown }).type as string)
  );
}

let unlisten: (() => void) | undefined;
let streamId: string | undefined;
let sequence = 0;
let buffering = false;
let buffered: InventoryEnvelope[] = [];

function disposeListener(): void {
  unlisten?.();
  unlisten = undefined;
}

function touchedPrinterId(location: SpoolLocation): string | null {
  return location.kind === "slot" ? location.printerId : null;
}

/** Upserts a Spool by id. A known Spool is replaced only if `force` is set
 *  or the incoming record's revision is strictly newer -- this is what
 *  makes a duplicate/out-of-order/stale event (or replay) a no-op. */
function upsertSpool(record: SpoolRecord, opts?: { force?: boolean }): void {
  const existing = state.spools.find((s) => s.id === record.id);
  if (existing && !opts?.force && record.revision <= existing.revision) return;
  setState("spools", (list) => (
    existing ? list.map((s) => (s.id === record.id ? record : s)) : [...list, record]
  ));
}

/** D12's `printer.slots.changed` carries only `{printerId, revision,
 *  materialSlots}`, not a full `PrinterRecord` -- patches just those fields
 *  onto whatever printer-store already has, rather than re-deriving one. */
function applyPrinterSlotsChanged(printerId: string, revision: number, materialSlots: MaterialSlot[]): void {
  const current = printerRecords().find((p) => p.id === printerId);
  if (!current || revision <= current.revision) return;
  spliceResolved({ ...current, revision, materialSlots });
}

function isPending(spoolId: string): boolean {
  return Object.values(state.pending).some((ids) => ids.includes(spoolId));
}

/** Controller ruling: `spool.availability.changed` can arrive stale
 *  relative to a `spool.changed`/command result under concurrency (D11).
 *  Preferring the availability embedded in those over this hint, this is
 *  applied only when the Spool isn't mid-optimistic-move -- the one case
 *  this store itself already knows is racing an authoritative settle. */
function applyAvailabilityHint(spoolId: string, availability: SpoolRecord["availability"]): void {
  if (isPending(spoolId)) return;
  setState("spools", (list) => list.map((s) => (s.id === spoolId ? { ...s, availability } : s)));
}

function applyInventoryEnvelope(envelope: InventoryEnvelope): void {
  if (streamId !== undefined && envelope.streamId !== streamId) return;
  if (envelope.sequence <= sequence) return;
  sequence = envelope.sequence;
  const payload = envelope.payload;
  if (payload.type === "spoolChanged") upsertSpool(payload.spool);
  else if (payload.type === "printerSlotsChanged") applyPrinterSlotsChanged(payload.printerId, payload.revision, payload.materialSlots);
  else if (payload.type === "spoolAvailabilityChanged") applyAvailabilityHint(payload.spoolId, payload.availability);
}

async function refetchInventory(): Promise<void> {
  try {
    const snapshot = await command("list_spools");
    streamId = snapshot.streamId;
    sequence = snapshot.snapshotSequence;
    setState({ spools: snapshot.spools, tares: snapshot.tares });
  } catch {
    // Best-effort: the CONFLICT that triggered this is already being
    // rethrown to the caller regardless of whether this refetch lands.
  }
}

/** D4/D12 web fixture: keeps the equipped Printer's `materialSlots`
 *  occupancy in sync with `state.spools`' own locations, so the Printer
 *  Status tab (Task 11) shows the right occupant in web mode too. */
function syncWebPrinterOccupancy(printerId: string): void {
  const printer = printerRecords().find((p) => p.id === printerId);
  if (!printer) return;
  const materialSlots: MaterialSlot[] = printer.materialSlots.map((slot) => {
    const occupant = state.spools.find((s) => s.location.kind === "slot" && s.location.slotId === slot.id);
    return occupant
      ? { id: slot.id, position: slot.position, name: slot.name, feederLabel: slot.feederLabel, occupantSpoolId: occupant.id }
      : { id: slot.id, position: slot.position, name: slot.name, feederLabel: slot.feederLabel };
  });
  spliceResolved({ ...printer, materialSlots });
}

/** Subscribes to the inventory stream, then backfills through `list_spools`
 *  -- the same listen-before-backfill order `printer-status-store` uses, so
 *  no event between subscribing and the snapshot arriving is missed. Events
 *  at or below the snapshot's `snapshotSequence` are dropped (D11). */
export async function loadInventory(): Promise<void> {
  disposeListener();
  if (!desktopAvailable()) {
    const fixture = buildWebInventoryFixture();
    setState({ spools: fixture.spools, tares: fixture.tares, loaded: true, error: null });
    syncWebPrinterOccupancy(WEB_FIXTURE_EQUIPPED_PRINTER_ID);
    return;
  }
  buffering = true;
  buffered = [];
  try {
    const { listen } = await import("@tauri-apps/api/event");
    unlisten = await listen<InventoryEnvelope>("farm3d-event-v1", (event) => {
      const candidate = event.payload;
      if (!isInventoryEnvelope(candidate)) return;
      if (buffering) buffered.push(candidate);
      else applyInventoryEnvelope(candidate);
    });
  } catch {
    setState({ error: "Inventory could not start updating live.", loaded: false });
    return;
  }
  try {
    const snapshot = await command("list_spools");
    streamId = snapshot.streamId;
    sequence = snapshot.snapshotSequence;
    setState({ spools: snapshot.spools, tares: snapshot.tares, loaded: true, error: null });
    buffering = false;
    const toReplay = buffered;
    buffered = [];
    for (const candidate of toReplay) applyInventoryEnvelope(candidate);
  } catch (e) {
    buffering = false;
    buffered = [];
    setState({ error: isCommandError(e) ? e.message : "Inventory could not load.", loaded: false });
  }
}

// --- Movement (D6): optimistic ---------------------------------------------

/** The Printer a slot destination resolves to. Prefers the slot's current
 *  occupant's own (already-correct) location over a fresh lookup through
 *  printer-store, since that's available even before any Printer data has
 *  loaded (e.g. a web-fixture move, or a test that only seeds Spools). */
function locationForDestination(destination: MoveDestination, excludeSpoolId: string): SpoolLocation {
  if (destination.kind === "storage") return { kind: "storage", storageLabel: destination.storageLabel ?? null };
  const occupant = state.spools.find(
    (s) => s.id !== excludeSpoolId && s.location.kind === "slot" && s.location.slotId === destination.slotId,
  );
  const printerId =
    (occupant?.location.kind === "slot" ? occupant.location.printerId : undefined) ??
    printerRecords().find((p) => p.materialSlots.some((slot) => slot.id === destination.slotId))?.id ??
    "";
  return { kind: "slot", slotId: destination.slotId, printerId };
}

function findSlotOccupant(slotId: string, excludeSpoolId: string): SpoolRecord | undefined {
  return state.spools.find((s) => s.id !== excludeSpoolId && s.location.kind === "slot" && s.location.slotId === slotId);
}

/** Applies (optimistically, or -- in web mode -- for real) the placement
 *  `moveSpool` already computed. Takes `nextLocation`/`displacedLocation`
 *  as arguments rather than recomputing them, since by the time a web-mode
 *  commit runs, `state.spools` has already been mutated by the optimistic
 *  step above it -- recomputing from that mutated state would see the
 *  Spool as already moved. */
function applyPlacement(
  spool: SpoolRecord,
  occupant: SpoolRecord | undefined,
  nextLocation: SpoolLocation,
  displacedLocation: SpoolLocation | undefined,
): void {
  setState("spools", (list) => list.map((s) => (s.id === spool.id ? { ...s, location: nextLocation } : s)));
  if (occupant && displacedLocation) {
    setState("spools", (list) => list.map((s) => (s.id === occupant.id ? { ...s, location: displacedLocation } : s)));
  }
}

function settleMove(result: MoveSpoolData): void {
  for (const record of result.spools) upsertSpool(record, { force: true });
  for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
}

/** D6/web fallback: enforces one Spool per slot (matching the DB's
 *  `spools_slot_occupancy` index) and swaps with displacement, since there
 *  is no Rust transaction to do it. Unlike the desktop path, it cannot also
 *  return the touched `PrinterRecord`s in `printers` -- it updates
 *  printer-store directly via `spliceResolved` instead. Takes the pre-move
 *  `spool`/`occupant` and the already-computed placement as arguments, for
 *  the same reason `applyPlacement` does. */
function webMoveSpool(
  req: Omit<MoveSpoolRequest, "operationId" | "contractVersion">,
  spool: SpoolRecord,
  occupant: SpoolRecord | undefined,
  nextLocation: SpoolLocation,
  displacedLocation: SpoolLocation | undefined,
): MoveSpoolData {
  if (req.destination.kind === "slot" && (occupant?.id ?? null) !== (req.destination.expectedOccupantSpoolId ?? null)) {
    throw commandError("CONFLICT", "Another Spool already occupies that slot.", {
      slotId: req.destination.slotId,
      currentOccupantSpoolId: occupant?.id ?? null,
    });
  }

  const moved: SpoolRecord = { ...spool, location: nextLocation, revision: spool.revision + 1 };
  const results: SpoolRecord[] = [moved];

  const touchedPrinterIds = new Set<string>();
  const beforeId = touchedPrinterId(spool.location);
  if (beforeId) touchedPrinterIds.add(beforeId);
  const afterId = touchedPrinterId(nextLocation);
  if (afterId) touchedPrinterIds.add(afterId);

  if (occupant && displacedLocation) {
    results.push({ ...occupant, location: displacedLocation, revision: occupant.revision + 1 });
  }

  for (const record of results) upsertSpool(record, { force: true });
  for (const printerId of touchedPrinterIds) syncWebPrinterOccupancy(printerId);
  return { spools: results, printers: [], movements: [] };
}

/** D6: load/unload/swap/relocate, all through this one call. Optimistic --
 *  applies the expected placement (and displaces an occupant, if any)
 *  locally before the command resolves, marking every touched Spool
 *  `pending`. On success it settles from the authoritative result and hands
 *  any returned Printers to `printer-store` via `spliceResolved`. On
 *  failure it restores the pre-move snapshot; a `CONFLICT` also refetches
 *  through `list_spools` before rethrowing, so the caller (an inline
 *  handler, not the banner -- this rejects rather than reporting) sees
 *  fresh state alongside the error. */
export async function moveSpool(req: Omit<MoveSpoolRequest, "operationId" | "contractVersion">): Promise<MoveSpoolData> {
  const spool = state.spools.find((s) => s.id === req.spoolId);
  if (!spool) throw commandError("NOT_FOUND", "This Spool no longer exists.");
  if (req.destination.kind === "slot" && spool.location.kind === "slot" && spool.location.slotId === req.destination.slotId) {
    throw commandError("VALIDATION", "This Spool already occupies that slot.", { fieldPath: "destination.slotId" });
  }
  const occupant = req.destination.kind === "slot" ? findSlotOccupant(req.destination.slotId, spool.id) : undefined;
  const nextLocation = locationForDestination(req.destination, spool.id);
  const displacedLocation: SpoolLocation | undefined = occupant && req.destination.kind === "slot"
    ? { kind: "storage", storageLabel: req.destination.displacedStorageLabel ?? null }
    : undefined;

  const operationId = crypto.randomUUID();
  const previousSnapshot = state.spools;
  const affected = occupant ? [spool.id, occupant.id] : [spool.id];
  setState("pending", operationId, affected);
  applyPlacement(spool, occupant, nextLocation, displacedLocation);

  try {
    const result = desktopAvailable()
      ? await command("move_spool", { ...req, operationId })
      : webMoveSpool(req, spool, occupant, nextLocation, displacedLocation);
    settleMove(result);
    return result;
  } catch (e) {
    setState("spools", previousSnapshot);
    if (isCommandError(e) && e.code === "CONFLICT") await refetchInventory();
    throw e;
  } finally {
    setState("pending", produce((pending) => {
      delete pending[operationId];
    }));
  }
}

// --- Other mutations (not optimistic, matching P2) --------------------------

function nextWebSpoolNumber(): number {
  return state.spools.reduce((max, s) => Math.max(max, s.spoolNumber), 0) + 1;
}

/** Resolves a scale/net amount entry to milligrams. Web-only: the desktop
 *  path never computes this itself -- `record_spool_amount`/`create_spool`
 *  do (D3/D7). */
function resolveWebAmountEntry(entry: AmountEntry): { mg: number; confidence: AmountConfidence } {
  if (entry.kind === "net") return { mg: entry.netMg, confidence: entry.confidence };
  const tareMg = entry.tareMg ?? state.tares.find((t) => t.id === entry.tareId)?.weightMg ?? 0;
  if (entry.grossMg < tareMg) throw commandError("VALIDATION", "The gross weight is less than the tare.", { fieldPath: "grossMg" });
  return { mg: entry.grossMg - tareMg, confidence: "measured" };
}

/** Web-only facet approximation (D9), used only by this module's own
 *  fixture mutations -- there is no Rust here to derive them for real, and
 *  `facets.ts`/the desktop path never do this. */
function webFacets(
  lifecycle: SpoolRecord["lifecycle"],
  currentMg: number,
  lowThresholdMg: number,
  confidence: AmountConfidence,
  loaded: boolean,
  reservedMg: number,
): SpoolRecord["facets"] {
  return { loaded, reserved: reservedMg > 0, low: lifecycle === "active" && currentMg <= lowThresholdMg, confidence };
}

async function webCreateSpool(fields: SpoolFields, initialAmount: AmountEntry, storageLabel?: string): Promise<SpoolRecord> {
  const { mg, confidence } = resolveWebAmountEntry(initialAmount);
  const now = new Date().toISOString();
  const spool: SpoolRecord = {
    id: `spl-web-${crypto.randomUUID()}`,
    revision: 1,
    spoolNumber: nextWebSpoolNumber(),
    ...fields,
    lifecycle: "active",
    location: { kind: "storage", storageLabel: storageLabel ?? null },
    availability: { currentMg: mg, reservedMg: 0, availableMg: mg },
    facets: webFacets("active", mg, fields.lowThresholdMg, confidence, false, 0),
    lastMeasuredAt: confidence === "measured" ? now : undefined,
    createdAt: now,
    updatedAt: now,
  };
  setState("spools", (list) => [...list, spool]);
  return spool;
}

/** Single-step create (D2/D3/D7): reports into `state.error` rather than
 *  rejecting -- every call site can `void` it, matching `printer-store`'s
 *  own mutations. */
export async function createSpool(
  fields: SpoolFields,
  initialAmount: AmountEntry,
  storageLabel?: string,
): Promise<SpoolRecord | undefined> {
  if (!desktopAvailable()) {
    try {
      return await webCreateSpool(fields, initialAmount, storageLabel);
    } catch (e) {
      reportSpoolError(e);
      return undefined;
    }
  }
  try {
    const result = await command("create_spool", { fields, initialAmount, storageLabel });
    upsertSpool(result.spool, { force: true });
    for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
    return result.spool;
  } catch (e) {
    reportSpoolError(e);
    return undefined;
  }
}

export async function updateSpool(id: string, patch: SpoolFields): Promise<SpoolRecord | undefined> {
  if (!desktopAvailable()) {
    const existing = state.spools.find((s) => s.id === id);
    if (!existing) return undefined;
    const updated: SpoolRecord = {
      ...existing,
      ...patch,
      facets: webFacets(existing.lifecycle, existing.availability.currentMg, patch.lowThresholdMg, existing.facets.confidence, existing.facets.loaded, existing.availability.reservedMg),
      revision: existing.revision + 1,
      updatedAt: new Date().toISOString(),
    };
    upsertSpool(updated, { force: true });
    return updated;
  }
  try {
    const expectedRevision = state.spools.find((s) => s.id === id)?.revision ?? 1;
    const result = await command("update_spool", { id, expectedRevision, patch });
    upsertSpool(result.spool, { force: true });
    for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
    return result.spool;
  } catch (e) {
    reportSpoolError(e);
    return undefined;
  }
}

export async function recordAmount(id: string, entry: AmountEntry, note?: string): Promise<SpoolRecord | undefined> {
  if (!desktopAvailable()) {
    try {
      const existing = state.spools.find((s) => s.id === id);
      if (!existing) return undefined;
      const { mg, confidence } = resolveWebAmountEntry(entry);
      const updated: SpoolRecord = {
        ...existing,
        availability: { currentMg: mg, reservedMg: existing.availability.reservedMg, availableMg: mg - existing.availability.reservedMg },
        facets: webFacets(existing.lifecycle, mg, existing.lowThresholdMg, confidence, existing.facets.loaded, existing.availability.reservedMg),
        lastMeasuredAt: new Date().toISOString(),
        revision: existing.revision + 1,
        updatedAt: new Date().toISOString(),
      };
      upsertSpool(updated, { force: true });
      return updated;
    } catch (e) {
      reportSpoolError(e);
      return undefined;
    }
  }
  try {
    const expectedRevision = state.spools.find((s) => s.id === id)?.revision ?? 1;
    const result = await command("record_spool_amount", { id, expectedRevision, entry, ...(note !== undefined ? { note } : {}) });
    upsertSpool(result.spool, { force: true });
    for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
    return result.spool;
  } catch (e) {
    reportSpoolError(e);
    return undefined;
  }
}

/** Web-only lifecycle approximation (D9). Doesn't enforce the reservation
 *  guards (`SPOOL_RESERVED`) the real command does -- web mode is a
 *  developer convenience, not a reimplementation of those rules. */
function webSetLifecycle(id: string, action: SpoolLifecycleAction, storageLabel?: string): SpoolRecord | undefined {
  const existing = state.spools.find((s) => s.id === id);
  if (!existing) return undefined;
  const now = new Date().toISOString();
  let updated: SpoolRecord;
  if (action === "markEmpty") {
    const wasLoaded = existing.location.kind === "slot";
    const location: SpoolLocation = wasLoaded ? { kind: "storage", storageLabel: storageLabel ?? null } : existing.location;
    updated = {
      ...existing,
      lifecycle: "empty",
      location,
      availability: { ...existing.availability, currentMg: 0, availableMg: -existing.availability.reservedMg },
      facets: webFacets("empty", 0, existing.lowThresholdMg, "measured", location.kind === "slot", existing.availability.reservedMg),
      lastMeasuredAt: now,
      revision: existing.revision + 1,
      updatedAt: now,
    };
    if (wasLoaded && existing.location.kind === "slot") syncWebPrinterOccupancy(existing.location.printerId);
  } else if (action === "reactivate") {
    updated = {
      ...existing, lifecycle: "active",
      facets: webFacets("active", existing.availability.currentMg, existing.lowThresholdMg, existing.facets.confidence, existing.facets.loaded, existing.availability.reservedMg),
      revision: existing.revision + 1, updatedAt: now,
    };
  } else if (action === "archive") {
    updated = { ...existing, lifecycle: "archived", revision: existing.revision + 1, updatedAt: now };
  } else {
    // "unarchive": the wire SpoolRecord never exposes archivedFrom, so this
    // web approximation always restores to "active" (the common case).
    updated = { ...existing, lifecycle: "active", revision: existing.revision + 1, updatedAt: now };
  }
  upsertSpool(updated, { force: true });
  return updated;
}

export async function setLifecycle(
  id: string,
  action: SpoolLifecycleAction,
  storageLabel?: string,
): Promise<SpoolRecord | undefined> {
  if (!desktopAvailable()) return webSetLifecycle(id, action, storageLabel);
  try {
    const expectedRevision = state.spools.find((s) => s.id === id)?.revision ?? 1;
    const result = await command("set_spool_lifecycle", { id, expectedRevision, action, ...(storageLabel !== undefined ? { storageLabel } : {}) });
    upsertSpool(result.spool, { force: true });
    for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
    return result.spool;
  } catch (e) {
    reportSpoolError(e);
    return undefined;
  }
}

// --- Tares (D3) --------------------------------------------------------------

export async function createTare(name: string, weightMg: number): Promise<Tare | undefined> {
  if (!desktopAvailable()) {
    const now = new Date().toISOString();
    const tare: Tare = { id: `tar-web-${crypto.randomUUID()}`, revision: 1, name, weightMg, createdAt: now, updatedAt: now };
    setState("tares", (list) => [...list, tare]);
    return tare;
  }
  try {
    const { tare } = await command("create_tare", { name, weightMg });
    setState("tares", (list) => [...list, tare]);
    return tare;
  } catch (e) {
    reportSpoolError(e);
    return undefined;
  }
}

export async function updateTare(id: string, name: string, weightMg: number): Promise<Tare | undefined> {
  if (!desktopAvailable()) {
    const existing = state.tares.find((t) => t.id === id);
    if (!existing) return undefined;
    const updated: Tare = { ...existing, name, weightMg, revision: existing.revision + 1, updatedAt: new Date().toISOString() };
    setState("tares", (list) => list.map((t) => (t.id === id ? updated : t)));
    return updated;
  }
  try {
    const expectedRevision = state.tares.find((t) => t.id === id)?.revision ?? 1;
    const { tare } = await command("update_tare", { id, expectedRevision, name, weightMg });
    setState("tares", (list) => list.map((t) => (t.id === tare.id ? tare : t)));
    return tare;
  } catch (e) {
    reportSpoolError(e);
    return undefined;
  }
}

export async function deleteTare(id: string): Promise<void> {
  if (!desktopAvailable()) {
    setState("tares", (list) => list.filter((t) => t.id !== id));
    setState("spools", (list) => list.map((s) => (s.tareId === id ? { ...s, tareId: undefined } : s)));
    return;
  }
  try {
    const expectedRevision = state.tares.find((t) => t.id === id)?.revision ?? 1;
    await command("delete_tare", { id, expectedRevision });
    setState("tares", (list) => list.filter((t) => t.id !== id));
  } catch (e) {
    reportSpoolError(e);
  }
}

// --- History -----------------------------------------------------------------

function buildWebHistory(spoolId: string): SpoolHistory {
  const spool = state.spools.find((s) => s.id === spoolId);
  if (!spool) return { movements: [], amountEvents: [], reservations: [] };
  const amountEvents: AmountEvent[] = [{
    id: `evt-web-${spoolId}-initial`,
    spoolId,
    sequence: 1,
    kind: "initial",
    afterMg: spool.availability.currentMg,
    confidenceAfter: spool.facets.confidence,
    occurredAt: spool.createdAt,
    isCorrection: false,
  }];
  const reservations: Reservation[] = spool.facets.reserved
    ? [{
        id: `res-web-${spoolId}`,
        spoolId,
        holder: { kind: "job", id: "job-web-demo" },
        amountMg: spool.availability.reservedMg,
        state: "active",
        operationId: "op-web-seed",
        createdAt: spool.updatedAt,
      }]
    : [];
  return { movements: [], amountEvents, reservations };
}

export async function loadHistory(spoolId: string): Promise<SpoolHistory> {
  if (!desktopAvailable()) return buildWebHistory(spoolId);
  return command("spool_history", { spoolId });
}
