import { createStore, produce } from "solid-js/store";
import { command, desktopAvailable, isCommandError, retryOnTransportFailure } from "../ipc/client";
import {
  printers as printerRecords,
  registerWebDispositionApplier,
  registerWebPrintersLoaded,
  registerWebSpoolLookup,
  spliceResolved,
  WEB_FIXTURE_EQUIPPED_PRINTER_ID,
} from "../printers/printer-store";
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
import type { SpoolDispositionInput } from "../generated/contracts/domain/SpoolDispositionInput";
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

/** Fix round 2 (finding B): pushes a lookup into `printer-store.ts` so its
 *  web `lifecycleEligibility`/`archivePrinter` can resolve a slot's
 *  `occupantSpoolId` to a real `SpoolRecord` without importing this module
 *  (which would be a real cycle -- this module already imports
 *  printer-store). Registered once, closing over the stable `state`
 *  reference `createStore` returns, so it always sees the current spools
 *  even though this runs before any are loaded. */
registerWebSpoolLookup((spoolId) => state.spools.find((s) => s.id === spoolId));

export function spoolStoreError(): string | null {
  return state.error;
}

export function dismissSpoolStoreError(): void {
  setState("error", null);
}

/** Exported for non-dialog callers (e.g. `SpoolDetailDock`'s lifecycle menu
 *  and Unload) that don't render their own inline error UI -- they catch
 *  their own call's rejection and route it here, the same way this module
 *  used to do internally before the fix-round-1 ruling made
 *  `createSpool`/`updateSpool`/`recordAmount`/`setLifecycle`/the tare CRUD
 *  reject for inline handling (matching `moveSpool`) instead of reporting
 *  to this banner themselves. A dialog that DOES render its own inline
 *  error (RecordAmountDialog, SpoolFormDialog, TareManagerDialog) must not
 *  also call this -- that would show the same failure twice. */
export function reportSpoolError(e: unknown): void {
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

/** Controller ruling: per-Spool marker of the last authoritative apply's
 *  "freshness" -- a `spool.changed` event marks its own stream `sequence`;
 *  a command result (there's no stream `sequence` to attach to a direct
 *  RPC response) marks `+Infinity`, since it's always at least as fresh as
 *  anything the frontend has seen through the stream so far. The next real
 *  `spool.changed` for that Spool (which will eventually confirm the same
 *  write) supersedes the `Infinity` marker with its own real `sequence`.
 *  `applyAvailabilityHint` below is the one thing that reads this. */
const lastAuthoritativeSequence = new Map<string, number>();

/** Upserts a Spool by id. A known Spool is replaced only if the incoming
 *  record's revision is strictly newer -- this is what makes a
 *  duplicate/out-of-order/stale event, command result, or replay a no-op,
 *  and what stops a command's own (possibly slower-arriving) result from
 *  regressing a newer state a `spool.changed` event already applied. Every
 *  caller goes through this guard, desktop and web alike -- web mode's own
 *  locally-computed revisions are always exactly `current + 1`, so the
 *  guard never blocks them. `atSequence` is the stream sequence to record
 *  as this Spool's freshness marker; omitted (command results, every web
 *  mutation) it defaults to `+Infinity` -- see `lastAuthoritativeSequence`.
 *
 *  A reservation change (P7, `src-tauri/src/spools/reservations.rs`) never
 *  bumps the Spool's own `revision` -- so its confirming `spool.changed`
 *  arrives at exactly `existing.revision`, carrying fresh
 *  `availability`/`facets` this function would otherwise drop entirely,
 *  including the case where it's the *same* write's own confirmation (a
 *  command result just set the marker to `+Infinity`). Patch those two
 *  fields in for a same-revision *event* (`atSequence` present -- a
 *  same-revision command result carries nothing new and is still a no-op),
 *  and always accept it regardless of the current marker: the top-level
 *  sequence gate in `applyInventoryEnvelope` already guarantees this event
 *  is fresher than anything the stream has delivered so far, so nothing
 *  finite could ever be "greater than" a `+Infinity` marker -- requiring
 *  that here would leave the marker stuck at `+Infinity` forever, exactly
 *  the bug this is fixing. This is also what lets the marker come back
 *  down from `+Infinity` to a real sequence, so a later availability hint
 *  can apply again. A strictly older revision is still ignored. */
function upsertSpool(record: SpoolRecord, atSequence?: number): void {
  const existing = state.spools.find((s) => s.id === record.id);
  if (existing && record.revision < existing.revision) return;
  if (existing && record.revision === existing.revision) {
    if (atSequence === undefined) return;
    setState("spools", (list) => list.map((s) => (
      s.id === record.id ? { ...s, availability: record.availability, facets: record.facets } : s
    )));
    lastAuthoritativeSequence.set(record.id, atSequence);
    return;
  }
  setState("spools", (list) => (
    existing ? list.map((s) => (s.id === record.id ? record : s)) : [...list, record]
  ));
  lastAuthoritativeSequence.set(record.id, atSequence ?? Number.POSITIVE_INFINITY);
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

/** D11: `spool.availability.changed` can arrive stale relative to a
 *  `spool.changed`/command result under concurrency. `isPending` covers
 *  this store's own `moveSpool` while its optimistic step is in flight
 *  (before either has updated `lastAuthoritativeSequence` at all); the
 *  marker covers everything after -- a hint at or below the sequence
 *  recorded for that Spool's last authoritative apply is a stale echo and
 *  is ignored, never regressing what's already there. */
function applyAvailabilityHint(spoolId: string, availability: SpoolRecord["availability"], envelopeSequence: number): void {
  if (isPending(spoolId)) return;
  const marker = lastAuthoritativeSequence.get(spoolId) ?? -1;
  if (envelopeSequence <= marker) return;
  setState("spools", (list) => list.map((s) => (s.id === spoolId ? { ...s, availability } : s)));
}

function applyInventoryEnvelope(envelope: InventoryEnvelope): void {
  if (streamId !== undefined && envelope.streamId !== streamId) return;
  if (envelope.sequence <= sequence) return;
  sequence = envelope.sequence;
  const payload = envelope.payload;
  if (payload.type === "spoolChanged") {
    upsertSpool(payload.spool, envelope.sequence);
  } else if (payload.type === "printerSlotsChanged") {
    applyPrinterSlotsChanged(payload.printerId, payload.revision, payload.materialSlots);
  } else if (payload.type === "spoolAvailabilityChanged") {
    applyAvailabilityHint(payload.spoolId, payload.availability, envelope.sequence);
  }
}

/** CONFLICT recovery, shared by every rejecting mutation's own CONFLICT
 *  catch (`moveSpool`, and -- per the fix-round-1 ruling --
 *  `updateSpool`/`recordAmount`/`setLifecycle` too): reloads only the
 *  Spools this call's revision guard touched, and leaves everything else
 *  -- other Spools, tares, `pending`, and this store's own stream
 *  `sequence`/`streamId` bookkeeping -- untouched. A whole-inventory
 *  replace here would regress any other Spool an event settled more
 *  recently than this (`list_spools`) snapshot, and lowering `sequence`
 *  would make the live listener re-apply events it already processed.
 *  Every rejecting caller re-reads `expectedRevision` from `state.spools`
 *  at call time, so a retry after this refetch automatically uses the
 *  fresh revision without the caller doing anything extra. */
async function refetchAffectedSpools(affectedIds: string[]): Promise<void> {
  try {
    const snapshot = await command("list_spools");
    for (const id of affectedIds) {
      const fresh = snapshot.spools.find((s) => s.id === id);
      if (fresh) upsertSpool(fresh);
    }
  } catch {
    // Best-effort: the CONFLICT that triggered this is already being
    // rethrown to the caller regardless of whether this refetch lands.
  }
}

/** The tare equivalent of `refetchAffectedSpools`, for `updateTare`/
 *  `deleteTare`'s own CONFLICT catch -- there is no dedicated tare-listing
 *  command, so this reuses `list_spools`' snapshot (which carries `tares`
 *  too) and applies only the one tare this call's revision guard touched. */
async function refetchTare(id: string): Promise<void> {
  try {
    const snapshot = await command("list_spools");
    const fresh = snapshot.tares.find((t) => t.id === id);
    if (fresh) setState("tares", (list) => list.map((t) => (t.id === id ? fresh : t)));
  } catch {
    // Best-effort, same rationale as refetchAffectedSpools above.
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

/** Fix (Task 3, web fixture honesty): covers the cold-deep-link order --
 *  `loadInventory` (called eagerly by `SpoolInventory` on mount) settling
 *  before `printer-store.ts`'s own web `loadPrinters` has resolved. That
 *  call's own `syncWebPrinterOccupancy` below no-ops (the Printer doesn't
 *  exist yet); this re-runs it once `loadPrinters` finally does, but only
 *  if the inventory itself has already loaded -- otherwise there is
 *  nothing yet to sync from, and `loadInventory`'s own call handles the
 *  (more common) reverse order once it runs. */
registerWebPrintersLoaded(() => {
  if (state.loaded) syncWebPrinterOccupancy(WEB_FIXTURE_EQUIPPED_PRINTER_ID);
});

/** Subscribes to the inventory stream, then backfills through `list_spools`
 *  -- the same listen-before-backfill order `printer-status-store` uses, so
 *  no event between subscribing and the snapshot arriving is missed. Events
 *  at or below the snapshot's `snapshotSequence` are dropped (D11). */
export async function loadInventory(): Promise<void> {
  disposeListener();
  lastAuthoritativeSequence.clear();
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

let inventoryLoad: Promise<void> | undefined;

/** For screens outside the Spools destination that read `spoolState`
 *  (the Printer Status tab's slots, the Material Slots editor, the wizard's
 *  Equip step): loads the inventory once if nothing has yet, sharing an
 *  in-flight load, and never reloads (or re-subscribes) one that's already
 *  live. `SpoolInventory` keeps calling `loadInventory` itself on mount. */
export function ensureInventoryLoaded(): Promise<void> {
  if (state.loaded) return Promise.resolve();
  inventoryLoad ??= loadInventory().finally(() => {
    inventoryLoad = undefined;
  });
  return inventoryLoad;
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
  for (const record of result.spools) upsertSpool(record);
  for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
}

/** D6/web fallback: enforces one Spool per slot (matching the DB's
 *  `spools_slot_occupancy` index) and swaps with displacement, since there
 *  is no Rust transaction to do it. Unlike the desktop path, it cannot also
 *  return the touched `PrinterRecord`s in `printers` -- it updates
 *  printer-store directly via `spliceResolved` instead. Takes the pre-move
 *  `spool`/`occupant` and the already-computed placement as arguments, for
 *  the same reason `applyPlacement` does. Recomputes `facets.loaded` (via
 *  `webFacets`) for both the moved and displaced Spool, so a web session's
 *  own facet stays honest about where each Spool actually is. */
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

  const moved: SpoolRecord = {
    ...spool,
    location: nextLocation,
    revision: spool.revision + 1,
    facets: webFacets(
      spool.lifecycle, spool.availability.currentMg, spool.lowThresholdMg,
      spool.facets.confidence, nextLocation.kind === "slot", spool.availability.reservedMg,
    ),
  };
  const results: SpoolRecord[] = [moved];

  const touchedPrinterIds = new Set<string>();
  const beforeId = touchedPrinterId(spool.location);
  if (beforeId) touchedPrinterIds.add(beforeId);
  const afterId = touchedPrinterId(nextLocation);
  if (afterId) touchedPrinterIds.add(afterId);

  if (occupant && displacedLocation) {
    results.push({
      ...occupant,
      location: displacedLocation,
      revision: occupant.revision + 1,
      facets: webFacets(
        occupant.lifecycle, occupant.availability.currentMg, occupant.lowThresholdMg,
        occupant.facets.confidence, false, occupant.availability.reservedMg,
      ),
    });
  }

  for (const record of results) upsertSpool(record);
  for (const printerId of touchedPrinterIds) syncWebPrinterOccupancy(printerId);
  return { spools: results, printers: [], movements: [] };
}

/** D6: load/unload/swap/relocate, all through this one call. Optimistic --
 *  applies the expected placement (and displaces an occupant, if any)
 *  locally before the command resolves, marking every touched Spool
 *  `pending`. On success it settles from the authoritative result and hands
 *  any returned Printers to `printer-store` via `spliceResolved`. On
 *  failure it restores only the Spools this move itself touched, and only
 *  those whose revision hasn't moved on since (i.e. still exactly the
 *  optimistic placement, not something a settled move/mutation or an event
 *  updated in the meantime) -- restoring the *whole* store here would
 *  regress unrelated state that moved on while this move was in flight. A
 *  `CONFLICT` also refetches those same affected Spools (only) through
 *  `list_spools` before rethrowing, so the caller (an inline handler, not
 *  the banner -- this rejects rather than reporting) sees fresh state
 *  alongside the error. A transport failure is retried once with the same
 *  `operationId` (`retryOnTransportFailure`) before any of that; the
 *  pending entry and optimistic placement stay in place across the retry. */
export async function moveSpool(req: Omit<MoveSpoolRequest, "operationId" | "contractVersion">): Promise<MoveSpoolData> {
  const spool = state.spools.find((s) => s.id === req.spoolId);
  if (!spool) throw commandError("NOT_FOUND", "This Spool no longer exists.");
  if (req.destination.kind === "slot" && spool.lifecycle === "archived") {
    // D5: loading is blocked for an archived Spool (an empty one may still
    // be loaded, so this checks lifecycle, not facets.loaded).
    throw commandError("VALIDATION", "An archived Spool cannot be loaded.", { fieldPath: "destination.slotId" });
  }
  if (req.destination.kind === "slot" && spool.location.kind === "slot" && spool.location.slotId === req.destination.slotId) {
    throw commandError("VALIDATION", "This Spool already occupies that slot.", { fieldPath: "destination.slotId" });
  }
  const occupant = req.destination.kind === "slot" ? findSlotOccupant(req.destination.slotId, spool.id) : undefined;
  const nextLocation = locationForDestination(req.destination, spool.id);
  const displacedLocation: SpoolLocation | undefined = occupant && req.destination.kind === "slot"
    ? { kind: "storage", storageLabel: req.destination.displacedStorageLabel ?? null }
    : undefined;

  const operationId = crypto.randomUUID();
  const originals = new Map<string, SpoolRecord>();
  originals.set(spool.id, spool);
  if (occupant) originals.set(occupant.id, occupant);
  const affected = [...originals.keys()];
  setState("pending", operationId, affected);
  applyPlacement(spool, occupant, nextLocation, displacedLocation);

  try {
    const result = desktopAvailable()
      ? await retryOnTransportFailure(() => command("move_spool", { ...req, operationId }))
      : webMoveSpool(req, spool, occupant, nextLocation, displacedLocation);
    settleMove(result);
    return result;
  } catch (e) {
    setState("spools", (list) => list.map((s) => {
      const original = originals.get(s.id);
      return original && s.revision === original.revision ? original : s;
    }));
    if (isCommandError(e) && e.code === "CONFLICT") await refetchAffectedSpools(affected);
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

/** D2: `notes` is capped at 2000 Unicode scalar values (trimmed), the same
 *  limit Rust's `validate_fields` enforces (`spools/mod.rs`). Web-only: the
 *  desktop path never re-derives this -- it trusts the command's own
 *  `VALIDATION` rejection. `[...text]` counts code points, not UTF-16 code
 *  units, matching Rust's `chars().count()`. */
function validateNotesCap(notes: string | undefined): void {
  if (notes !== undefined && [...notes.trim()].length > 2000) {
    throw commandError("VALIDATION", "Notes must be at most 2000 characters.", { fieldPath: "notes" });
  }
}

/** D3 web parity: a Spool's default `tareId` must name an existing tare --
 *  the same `VALIDATION`/`tareId` rejection `validate_tare_reference`
 *  (`spools/repository.rs`) gives the desktop path (an unknown id would
 *  otherwise just silently fail to resolve a tare's weight later). Web-only:
 *  the desktop path never re-derives this -- it trusts the command's own
 *  rejection. Uses the same generic message Rust's `RepositoryError::
 *  Validation` maps to (`contracts/command.rs`'s `validation_at`), so a
 *  dialog showing it inline looks identical in both modes. */
function validateTareReference(tareId: string | undefined): void {
  if (tareId !== undefined && !state.tares.some((t) => t.id === tareId)) {
    throw commandError("VALIDATION", "The submitted value is invalid.", { fieldPath: "tareId" });
  }
}

/** Resolves a scale/net amount entry to milligrams. Web-only: the desktop
 *  path never computes this itself -- `record_spool_amount`/`create_spool`
 *  do (D3/D7). */
function resolveWebAmountEntry(entry: AmountEntry): { mg: number; confidence: AmountConfidence } {
  if (entry.kind === "net") return { mg: entry.netMg, confidence: entry.confidence };
  // Exactly one of `tareId`/`tareMg`, as Rust's `resolve_entry` requires
  // (D3); a scale entry with no tare sends `tareMg: 0`.
  const hasTareId = entry.tareId !== undefined;
  const hasTareMg = entry.tareMg !== undefined;
  if (hasTareId === hasTareMg) throw commandError("VALIDATION", "Choose a tare, or enter no tare.", { fieldPath: "entry.tareId" });
  let tareMg: number;
  if (hasTareMg) {
    tareMg = entry.tareMg!;
  } else {
    const tare = state.tares.find((t) => t.id === entry.tareId);
    if (!tare) throw commandError("VALIDATION", "That tare no longer exists.", { fieldPath: "entry.tareId" });
    tareMg = tare.weightMg;
  }
  // "entry.grossMg", not "grossMg": matches the real `record_spool_amount`/
  // `create_spool` field path (`spools/ledger.rs`'s `resolve_entry`), so a
  // dialog's field-path-to-field mapping behaves identically in web mode.
  if (entry.grossMg < tareMg) throw commandError("VALIDATION", "The gross weight is less than the tare.", { fieldPath: "entry.grossMg" });
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
  validateNotesCap(fields.notes);
  validateTareReference(fields.tareId);
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

/** Single-step create (D2/D3/D7). Rejects for inline handling (fix round
 *  1 ruling: matches `moveSpool`, not the report-to-banner shape the other
 *  P2-style mutations used before this fix) -- `SpoolFormDialog` catches
 *  the rejection and shows a field-level or dialog-level error. */
export async function createSpool(
  fields: SpoolFields,
  initialAmount: AmountEntry,
  storageLabel?: string,
): Promise<SpoolRecord> {
  if (!desktopAvailable()) return webCreateSpool(fields, initialAmount, storageLabel);
  const result = await command("create_spool", { fields, initialAmount, storageLabel });
  upsertSpool(result.spool);
  for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
  return result.spool;
}

/** Rejects for inline handling (fix round 1 ruling). A `CONFLICT` also
 *  refetches this Spool through `list_spools` before rethrowing (the same
 *  shape `moveSpool` and `recordAmount` use), so a retried submit reads
 *  the fresh revision automatically. */
export async function updateSpool(id: string, patch: SpoolFields): Promise<SpoolRecord> {
  if (!desktopAvailable()) {
    const existing = state.spools.find((s) => s.id === id);
    if (!existing) throw commandError("NOT_FOUND", "This Spool no longer exists.");
    validateNotesCap(patch.notes);
    validateTareReference(patch.tareId);
    const updated: SpoolRecord = {
      ...existing,
      ...patch,
      facets: webFacets(existing.lifecycle, existing.availability.currentMg, patch.lowThresholdMg, existing.facets.confidence, existing.facets.loaded, existing.availability.reservedMg),
      revision: existing.revision + 1,
      updatedAt: new Date().toISOString(),
    };
    upsertSpool(updated);
    return updated;
  }
  const expectedRevision = state.spools.find((s) => s.id === id)?.revision ?? 1;
  try {
    const result = await command("update_spool", { id, expectedRevision, patch });
    upsertSpool(result.spool);
    for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
    return result.spool;
  } catch (e) {
    if (isCommandError(e) && e.code === "CONFLICT") await refetchAffectedSpools([id]);
    throw e;
  }
}

/** Rejects for inline handling (fix round 1 ruling) -- `RecordAmountDialog`
 *  catches the rejection (e.g. a `VALIDATION` on `entry.grossMg` when the
 *  gross is below the tare, D3) and shows it inline rather than relying on
 *  the store banner, which would render behind the dialog's own overlay. */
export async function recordAmount(id: string, entry: AmountEntry, note?: string): Promise<SpoolRecord> {
  if (!desktopAvailable()) {
    const existing = state.spools.find((s) => s.id === id);
    if (!existing) throw commandError("NOT_FOUND", "This Spool no longer exists.");
    const { mg, confidence } = resolveWebAmountEntry(entry);
    const updated: SpoolRecord = {
      ...existing,
      availability: { currentMg: mg, reservedMg: existing.availability.reservedMg, availableMg: mg - existing.availability.reservedMg },
      facets: webFacets(existing.lifecycle, mg, existing.lowThresholdMg, confidence, existing.facets.loaded, existing.availability.reservedMg),
      lastMeasuredAt: new Date().toISOString(),
      revision: existing.revision + 1,
      updatedAt: new Date().toISOString(),
    };
    upsertSpool(updated);
    return updated;
  }
  const expectedRevision = state.spools.find((s) => s.id === id)?.revision ?? 1;
  try {
    const result = await command("record_spool_amount", { id, expectedRevision, entry, ...(note !== undefined ? { note } : {}) });
    upsertSpool(result.spool);
    for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
    return result.spool;
  } catch (e) {
    if (isCommandError(e) && e.code === "CONFLICT") await refetchAffectedSpools([id]);
    throw e;
  }
}

/** Web-only lifecycle approximation (D9). Doesn't enforce the reservation
 *  guards (`SPOOL_RESERVED`) the real command does -- web mode is a
 *  developer convenience, not a reimplementation of those rules. */
function webSetLifecycle(id: string, action: SpoolLifecycleAction, storageLabel?: string): SpoolRecord {
  const existing = state.spools.find((s) => s.id === id);
  if (!existing) throw commandError("NOT_FOUND", "This Spool no longer exists.");
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
  upsertSpool(updated);
  // After the upsert, not before: the sync reads occupancy from
  // `state.spools`, so a mark-empty that unloads must already be applied.
  if (existing.location.kind === "slot" && updated.location.kind !== "slot") {
    syncWebPrinterOccupancy(existing.location.printerId);
  }
  return updated;
}

/** D10 in web mode, registered into printer-store's `archivePrinter` (see
 *  `registerWebDispositionApplier`). Validates the whole set first -- every
 *  loaded Spool named exactly once, each `slot` destination on a different,
 *  non-archived Printer -- so a rejected set applies nothing, then moves
 *  each Spool through this store's own web `moveSpool`/`setLifecycle`
 *  paths. Unlike the desktop transaction, a failure part-way through (e.g.
 *  a `CONFLICT` on a later destination) leaves the earlier moves applied;
 *  web mode is a developer convenience, not a reimplementation. */
async function applyWebArchiveDispositions(printerId: string, dispositions: SpoolDispositionInput[]): Promise<void> {
  const loadedIds = new Set(
    state.spools.filter((s) => s.location.kind === "slot" && s.location.printerId === printerId).map((s) => s.id),
  );
  const named = new Set(dispositions.map((d) => d.spoolId));
  const coversExactly = named.size === dispositions.length && named.size === loadedIds.size && [...named].every((id) => loadedIds.has(id));
  const validDestinations = dispositions.every((d) => {
    if (d.disposition.kind !== "slot") return true;
    const slotId = d.disposition.slotId;
    const target = printerRecords().find((p) => p.materialSlots.some((slot) => slot.id === slotId));
    return target !== undefined && target.id !== printerId && !target.archivedAt;
  });
  if (!coversExactly || !validDestinations) {
    throw commandError("VALIDATION", "Choose where each loaded Spool goes before archiving.", { fieldPath: "spoolDispositions" });
  }
  for (const { spoolId, expectedSpoolRevision, disposition } of dispositions) {
    if (disposition.kind === "markEmpty") {
      await setLifecycle(spoolId, "markEmpty", disposition.storageLabel ?? undefined);
    } else if (disposition.kind === "storage") {
      await moveSpool({ spoolId, expectedSpoolRevision, destination: { kind: "storage", storageLabel: disposition.storageLabel ?? null } });
    } else {
      await moveSpool({
        spoolId,
        expectedSpoolRevision,
        destination: {
          kind: "slot",
          slotId: disposition.slotId,
          expectedOccupantSpoolId: disposition.expectedOccupantSpoolId,
          ...(disposition.displacedStorageLabel !== undefined ? { displacedStorageLabel: disposition.displacedStorageLabel } : {}),
        },
      });
    }
  }
}

registerWebDispositionApplier(applyWebArchiveDispositions);

/** Rejects for inline handling (fix round 1 ruling). `SpoolDetailDock`'s
 *  lifecycle menu and Unload are non-dialog callers -- they catch this
 *  themselves and route the failure to `reportSpoolError` (the banner),
 *  since they render no inline error UI of their own. Each call sends a
 *  fresh `operationId`, retried once on a transport failure
 *  (`retryOnTransportFailure`). */
export async function setLifecycle(
  id: string,
  action: SpoolLifecycleAction,
  storageLabel?: string,
): Promise<SpoolRecord> {
  if (!desktopAvailable()) return webSetLifecycle(id, action, storageLabel);
  const expectedRevision = state.spools.find((s) => s.id === id)?.revision ?? 1;
  try {
    const request = { operationId: crypto.randomUUID(), id, expectedRevision, action, ...(storageLabel !== undefined ? { storageLabel } : {}) };
    const result = await retryOnTransportFailure(() => command("set_spool_lifecycle", request));
    upsertSpool(result.spool);
    for (const printer of result.printers) spliceResolved(resolvePrinterRecord(printer));
    return result.spool;
  } catch (e) {
    if (isCommandError(e) && e.code === "CONFLICT") await refetchAffectedSpools([id]);
    throw e;
  }
}

// --- Tares (D3) --------------------------------------------------------------

/** Rejects for inline handling (fix round 1 ruling) -- `TareManagerDialog`
 *  catches the rejection (e.g. `VALIDATION` on `name` for a duplicate,
 *  case-insensitive per D3) and shows it inline. Create has no
 *  `expectedRevision`, so it has no CONFLICT path. */
export async function createTare(name: string, weightMg: number): Promise<Tare> {
  if (!desktopAvailable()) {
    const now = new Date().toISOString();
    const tare: Tare = { id: `tar-web-${crypto.randomUUID()}`, revision: 1, name, weightMg, createdAt: now, updatedAt: now };
    setState("tares", (list) => [...list, tare]);
    return tare;
  }
  const { tare } = await command("create_tare", { name, weightMg });
  setState("tares", (list) => [...list, tare]);
  return tare;
}

/** Rejects for inline handling (fix round 1 ruling). A `CONFLICT` refetches
 *  this tare (through `refetchTare`, `list_spools`' only source for tares)
 *  before rethrowing, mirroring the Spool mutations' own CONFLICT shape. */
export async function updateTare(id: string, name: string, weightMg: number): Promise<Tare> {
  if (!desktopAvailable()) {
    const existing = state.tares.find((t) => t.id === id);
    if (!existing) throw commandError("NOT_FOUND", "This tare no longer exists.");
    const updated: Tare = { ...existing, name, weightMg, revision: existing.revision + 1, updatedAt: new Date().toISOString() };
    setState("tares", (list) => list.map((t) => (t.id === id ? updated : t)));
    return updated;
  }
  const expectedRevision = state.tares.find((t) => t.id === id)?.revision ?? 1;
  try {
    const { tare } = await command("update_tare", { id, expectedRevision, name, weightMg });
    setState("tares", (list) => list.map((t) => (t.id === tare.id ? tare : t)));
    return tare;
  } catch (e) {
    if (isCommandError(e) && e.code === "CONFLICT") await refetchTare(id);
    throw e;
  }
}

/** Rejects for inline handling (fix round 1 ruling). */
export async function deleteTare(id: string): Promise<void> {
  if (!desktopAvailable()) {
    setState("tares", (list) => list.filter((t) => t.id !== id));
    setState("spools", (list) => list.map((s) => (s.tareId === id ? { ...s, tareId: undefined } : s)));
    return;
  }
  const expectedRevision = state.tares.find((t) => t.id === id)?.revision ?? 1;
  try {
    await command("delete_tare", { id, expectedRevision });
    setState("tares", (list) => list.filter((t) => t.id !== id));
  } catch (e) {
    if (isCommandError(e) && e.code === "CONFLICT") await refetchTare(id);
    throw e;
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
