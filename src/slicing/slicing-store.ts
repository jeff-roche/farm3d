import { createStore, reconcile } from "solid-js/store";
import {
  binaryCommand,
  command,
  desktopAvailable,
  isCommandError,
  retryOnTransportFailure,
} from "../ipc/client";
import { createSequencedStream } from "../ipc/sequenced-stream";
import { desktopOnlyError } from "./desktop-only";
import { notFound } from "./local-errors";
import { decodeMeshBuffer, type MeshBuffer } from "./mesh-buffer";
import type { WebSlicingFixture } from "./web-fixtures";
import {
  isSlicingEvent,
  isTerminalOperationState,
  revisionSummaryOf,
  type CreateExternalSliceRevisionFacts,
  type PreparationDocument,
  type PreparationRecord,
  type PresetSourceKind,
  type ReloadPreparationData,
  type RevisionGeometry,
  type SliceOperationLog,
  type SliceOperationRecord,
  type SliceOperationState,
  type SliceOptions,
  type SliceProgress,
  type SliceRevisionRecord,
  type SliceRevisionSummary,
  type SlicerRuntimeStatus,
  type SliceTarget,
  type SlicingEvent,
  type SlicingSnapshot,
} from "./types";

/** The only owner of slicing data (spec §Frontend architecture, State). A
 *  module-level Solid store: read `slicing.*()` inside a tracked scope for
 *  Solid to pick up changes.
 *
 *  Data only. Every action rejects with the backend's `CommandError`
 *  unchanged (its `code` and `recovery`, such as `PREPARATION_STALE` with
 *  `RELOAD_PREPARATION`, or `OPERATION_NOT_CANCELLABLE`), for the
 *  component that called it to present. Nothing here formats text for
 *  display. */

export type SlicingStatus = "idle" | "loading" | "ready" | "error";
export type SlicingSyncState = "syncing" | "current" | "uncertain";

interface SlicingState {
  runtime: SlicerRuntimeStatus | null;
  /** Keyed by Model id: a Model has at most one Preparation. */
  preparations: { [modelId: string]: PreparationRecord | undefined };
  /** Every queued or running operation, then the recently finished ones,
   *  in the order the backend listed them; new ones are appended. */
  operations: SliceOperationRecord[];
  /** Keyed by Model id, newest first. */
  revisionsByModel: { [modelId: string]: SliceRevisionSummary[] | undefined };
  /** D17's ephemeral progress, keyed by operation id: only the latest, only
   *  while the operation is queued or running, and never kept across a
   *  backfill. */
  progress: { [operationId: string]: SliceProgress | undefined };
  status: SlicingStatus;
  syncState: SlicingSyncState;
  /** The user's **Continue with revision M** choice per stale Preparation,
   *  by Preparation id: the pinned `sourceRevisionId` to send as
   *  `continueWithSourceRevision`. Session-only (never persisted, kept
   *  across backfills); it lapses once the Preparation is reloaded. */
  continueWith: { [preparationId: string]: string | undefined };
}

const [state, setState] = createStore<SlicingState>({
  runtime: null,
  preparations: {},
  operations: [],
  revisionsByModel: {},
  progress: {},
  status: "idle",
  syncState: "current",
  continueWith: {},
});

export const slicing = {
  /** `null` until the first snapshot. */
  runtime: (): SlicerRuntimeStatus | null => state.runtime,
  preparation: (modelId: string): PreparationRecord | undefined => state.preparations[modelId],
  preparations: (): PreparationRecord[] => Object.values(state.preparations).filter(isDefined),
  operations: (): SliceOperationRecord[] => state.operations,
  operation: (id: string): SliceOperationRecord | undefined => state.operations.find((o) => o.id === id),
  operationsForPreparation: (preparationId: string): SliceOperationRecord[] =>
    state.operations.filter((o) => o.preparationId === preparationId),
  /** Newest first; empty for a Model without Slice Revisions. */
  revisions: (modelId: string): SliceRevisionSummary[] => state.revisionsByModel[modelId] ?? [],
  revision: (id: string): SliceRevisionSummary | undefined => findRevision(id),
  progress: (operationId: string): SliceProgress | undefined => state.progress[operationId],
  status: (): SlicingStatus => state.status,
  syncState: (): SlicingSyncState => state.syncState,
  /** D5's deliberate choice to slice a stale Preparation on the revision it
   *  is pinned to: the revision id to pass to `startSlice` as
   *  `continueWithSourceRevision`, or `undefined`. It applies only while
   *  the held Preparation is stale and still pinned to that revision. */
  continueWithSourceRevision: (preparationId: string): string | undefined => {
    const chosen = state.continueWith[preparationId];
    if (!chosen) return undefined;
    const held = Object.values(state.preparations).find((p) => p?.id === preparationId);
    return held?.stale && held.sourceRevisionId === chosen ? chosen : undefined;
  },
};

/** A deep copy that also reads through Solid store proxies. Only for JSON
 *  data. */
function clone<T>(value: T): T {
  return JSON.parse(JSON.stringify(value)) as T;
}

function isDefined<T>(value: T | undefined): value is T {
  return value !== undefined;
}

function findRevision(id: string): SliceRevisionSummary | undefined {
  for (const list of Object.values(state.revisionsByModel)) {
    const found = list?.find((r) => r.id === id);
    if (found) return found;
  }
  return undefined;
}

// --- Settling records -----------------------------------------------------------

/** Ids removed while this store has been running. Ids are never reused, so
 *  a command result that arrives after its record's removal event is
 *  dropped instead of bringing the record back. */
const removedPreparations = new Set<string>();
const removedRevisions = new Set<string>();

/** The runtime's `revision` is its configuration's, and a forced probe can
 *  change the status without bumping it. Events are ordered by sequence,
 *  so they always apply; a command result applies unless the held status
 *  is from a newer configuration. */
function settleRuntime(status: SlicerRuntimeStatus, from: "event" | "result"): void {
  if (from === "result" && state.runtime && status.revision < state.runtime.revision) return;
  setState("runtime", reconcile(status));
}

/** `stale` is derived on read and can change without a `revision` bump, so
 *  a stream event (ordered by sequence) applies at an equal revision. A
 *  command result has no sequence to order it, so it applies only when
 *  strictly newer and can never regress an event. Only an event may
 *  replace a different Preparation for the same Model (the old one was
 *  deleted); a result for another id is older than what is held. */
function settlePreparation(record: PreparationRecord, from: "event" | "result"): void {
  if (removedPreparations.has(record.id)) return;
  const existing = state.preparations[record.modelId];
  if (existing && existing.id === record.id) {
    const stale = from === "event" ? record.revision < existing.revision : record.revision <= existing.revision;
    if (stale) return;
  } else if (existing && from === "result") {
    return;
  }
  setState("preparations", record.modelId, reconcile(record));
}

function dropPreparation(id: string): void {
  removedPreparations.add(id);
  const held = Object.values(state.preparations).find((p) => p?.id === id);
  if (held) setState("preparations", held.modelId, undefined);
}

/** D10 has no revision on an operation, but its states only move forward:
 *  queued, then running, then one terminal state that never changes. */
function stateRank(value: SliceOperationState): number {
  if (value === "queued") return 0;
  if (value === "running") return 1;
  return 2;
}

/** A stream event applies unless it would move the operation backwards or
 *  out of a terminal state. A command result has no sequence, so it
 *  applies only when strictly further along. A terminal operation's
 *  progress is cleared. */
function settleOperation(record: SliceOperationRecord, from: "event" | "result"): void {
  const index = state.operations.findIndex((o) => o.id === record.id);
  if (index >= 0) {
    const existing = state.operations[index];
    if (isTerminalOperationState(existing.state)) return;
    const rank = stateRank(record.state);
    const held = stateRank(existing.state);
    if (from === "event" ? rank < held : rank <= held) return;
    setState("operations", index, reconcile(record));
  } else {
    setState("operations", (list) => [...list, record]);
  }
  if (isTerminalOperationState(record.state)) setState("progress", record.id, undefined);
}

/** Progress only for an operation this store holds as queued or running,
 *  so a late update never resurrects a finished one. Last wins. */
function settleProgress(operationId: string, progress: SliceProgress): void {
  const operation = state.operations.find((o) => o.id === operationId);
  if (!operation || isTerminalOperationState(operation.state)) return;
  setState("progress", operationId, reconcile(progress));
}

/** The backend's order: `created_at DESC, id DESC`, compared as plain
 *  strings (ISO timestamps and ids sort by code unit). */
function byNewestRevision(a: SliceRevisionSummary, b: SliceRevisionSummary): number {
  if (a.createdAt !== b.createdAt) return a.createdAt < b.createdAt ? 1 : -1;
  if (a.id !== b.id) return a.id < b.id ? 1 : -1;
  return 0;
}

/** Slice Revisions are immutable (D1), so one is only ever added or
 *  removed. */
function settleRevision(summary: SliceRevisionSummary): void {
  if (removedRevisions.has(summary.id)) return;
  const list = state.revisionsByModel[summary.modelId] ?? [];
  if (list.some((r) => r.id === summary.id)) return;
  setState("revisionsByModel", summary.modelId, [...list, summary].sort(byNewestRevision));
}

function dropRevision(id: string): void {
  removedRevisions.add(id);
  revisionRecords.delete(id);
  const held = findRevision(id);
  if (!held) return;
  setState("revisionsByModel", held.modelId, (list) => (list ?? []).filter((r) => r.id !== id));
}

function groupRevisions(revisions: SliceRevisionSummary[]): SlicingState["revisionsByModel"] {
  const grouped: SlicingState["revisionsByModel"] = {};
  for (const revision of revisions) (grouped[revision.modelId] ??= []).push(revision);
  for (const list of Object.values(grouped)) list?.sort(byNewestRevision);
  return grouped;
}

function keyPreparations(preparations: PreparationRecord[]): SlicingState["preparations"] {
  const keyed: SlicingState["preparations"] = {};
  for (const preparation of preparations) keyed[preparation.modelId] = preparation;
  return keyed;
}

// --- Events -------------------------------------------------------------------------

/** Every slicing event, progress included, consumes a stream sequence
 *  (D17), so all of them go through the sequenced stream: progress that
 *  arrives while a snapshot loads is held and replayed after it, in order,
 *  and progress the snapshot already covers is dropped. */
function applyEvent(event: SlicingEvent): void {
  switch (event.type) {
    case "slicing.runtime.changed":
      settleRuntime(event.payload as SlicerRuntimeStatus, "event");
      break;
    case "slicing.preparation.changed":
      settlePreparation(event.payload as PreparationRecord, "event");
      break;
    case "slicing.preparation.removed":
      dropPreparation(event.subject.id);
      break;
    case "slicing.operation.changed":
      settleOperation(event.payload as SliceOperationRecord, "event");
      break;
    case "slicing.operation.progress":
      settleProgress(event.subject.id, event.payload as SliceProgress);
      break;
    case "slicing.revision.created":
      settleRevision(event.payload as SliceRevisionSummary);
      break;
    case "slicing.revision.removed":
      dropRevision(event.subject.id);
      break;
  }
}

/** A snapshot replaces everything, and drops all progress: progress is
 *  never in a snapshot, and whatever follows it arrives as events. */
function applySnapshot(snapshot: SlicingSnapshot): void {
  setState({
    runtime: snapshot.runtime,
    preparations: keyPreparations(snapshot.preparations),
    operations: snapshot.activeAndRecentOperations,
    revisionsByModel: groupRevisions(snapshot.revisions),
    progress: {},
    status: "ready",
  });
}

// --- Startup (listen-before-backfill, D17) --------------------------------------------

type SlicingStream = ReturnType<typeof createSequencedStream<SlicingEvent, SlicingSnapshot>>;

let activeStream: SlicingStream | undefined;
let activeUnlisten: (() => void) | undefined;
let webFixture: WebSlicingFixture | undefined;

function disposeListener(): void {
  activeStream?.dispose();
  activeStream = undefined;
  activeUnlisten?.();
  activeUnlisten = undefined;
}

/** Subscribes to `slicing.*` events, then backfills through `list_slicing`,
 *  so nothing between the two is missed. Idempotent: calling it again
 *  disposes the previous listener first. Resolves once the first backfill
 *  has settled (successfully or not; a failure keeps retrying) with this
 *  start's own disposer. In web mode it loads the web fixtures instead. */
export async function startSlicing(): Promise<() => void> {
  disposeListener();
  if (!desktopAvailable()) {
    webFixture = (await import("./web-fixtures")).buildWebSlicingFixture();
    setState({
      runtime: webFixture.runtime,
      preparations: keyPreparations(webFixture.preparations),
      operations: webFixture.operations,
      revisionsByModel: groupRevisions(webFixture.revisions),
      progress: {},
      status: "ready",
      syncState: "current",
    });
    return () => {};
  }

  setState({ syncState: "syncing", ...(state.status === "ready" ? {} : { status: "loading" }) });
  const stream: SlicingStream = createSequencedStream<SlicingEvent, SlicingSnapshot>({
    backfill: () => command("list_slicing"),
    applySnapshot,
    applyEvent,
    onSyncState: (syncState) => setState("syncState", syncState),
    onBackfillError: () => {
      if (state.status !== "ready") setState("status", "error");
    },
  });
  activeStream = stream;
  const dispose = () => {
    if (activeStream !== stream) return;
    disposeListener();
  };

  try {
    const { listen } = await import("@tauri-apps/api/event");
    const unlisten = await listen<unknown>("farm3d-event-v1", (event) => {
      const candidate = event.payload;
      if (isSlicingEvent(candidate)) stream.receive(candidate);
    });
    if (activeStream !== stream) {
      unlisten();
      return () => {};
    }
    activeUnlisten = unlisten;
  } catch {
    dispose();
    setState({ status: "error", syncState: "uncertain" });
    return () => {};
  }

  await stream.start();
  return dispose;
}

/** Backfill now rather than wait for the stream's backoff, e.g. for a
 *  **Refresh** while slicing data may be out of date. Web mode has no
 *  stream to refresh. */
export function refreshSlicing(): void {
  activeStream?.resync();
}

/** A `CONFLICT` means this store holds a stale revision: reload, so a retry
 *  reads the fresh one, then rethrow for the caller to present. */
async function withConflictRefresh<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (error) {
    if (isCommandError(error) && error.code === "CONFLICT") activeStream?.resync();
    throw error;
  }
}

function heldPreparation(id: string): PreparationRecord {
  const preparation = Object.values(state.preparations).find((p) => p?.id === id);
  if (!preparation) throw notFound(id);
  return preparation;
}

function heldRuntimeRevision(): number {
  return state.runtime?.revision ?? 0;
}

// --- Runtime (D2, D22) ----------------------------------------------------------------

/** Forces a probe. Web mode answers with the fixture runtime. */
export async function checkSlicerRuntime(): Promise<SlicerRuntimeStatus> {
  if (!desktopAvailable()) {
    if (!state.runtime) throw notFound("runtime");
    return state.runtime;
  }
  const status = await command("check_slicer_runtime");
  settleRuntime(status, "result");
  return status;
}

/** `null` when the user cancels the picker. */
export async function pickSlicerEngine(): Promise<SlicerRuntimeStatus | null> {
  if (!desktopAvailable()) throw desktopOnlyError("pickSlicerEngine");
  const expectedRevision = heldRuntimeRevision();
  const status = await withConflictRefresh(() => command("pick_slicer_engine", { expectedRevision }));
  if (status) settleRuntime(status, "result");
  return status;
}

/** `null` when the user cancels the picker. */
export async function pickPresetSource(kind: PresetSourceKind): Promise<SlicerRuntimeStatus | null> {
  if (!desktopAvailable()) throw desktopOnlyError("pickPresetSource");
  const expectedRevision = heldRuntimeRevision();
  const status = await withConflictRefresh(() => command("pick_preset_source", { expectedRevision, kind }));
  if (status) settleRuntime(status, "result");
  return status;
}

export async function resetSlicerRuntime(reset: { engine: boolean; presetSource: boolean }): Promise<SlicerRuntimeStatus> {
  if (!desktopAvailable()) throw desktopOnlyError("resetSlicerRuntime");
  const expectedRevision = heldRuntimeRevision();
  const status = await withConflictRefresh(() => command("reset_slicer_runtime", { expectedRevision, ...reset }));
  settleRuntime(status, "result");
  return status;
}

// --- Reads: options, geometry, meshes -----------------------------------------------------

/** Not cached: presets change with the runtime. */
export async function listSliceOptions(target: SliceTarget): Promise<SliceOptions> {
  if (!desktopAvailable()) return clone((await requireWebFixture()).sliceOptions);
  return command("list_slice_options", { target });
}

/** D6: a Model Source Revision's objects and build items. Unprintable build
 *  items are `buildItems[].printable === false`. Not cached here; the
 *  spec's `geometry-cache.ts` caches for the preparation view. */
export async function loadGeometry(revisionId: string): Promise<RevisionGeometry> {
  if (!desktopAvailable()) {
    const geometry = (await requireWebFixture()).geometry[revisionId];
    if (!geometry) throw notFound(revisionId);
    return clone(geometry);
  }
  return command("get_revision_geometry", { revisionId });
}

/** D6: one object's mesh, decoded. Rejects with the backend's
 *  `CommandError` (`NOT_FOUND` for an unknown object), or a
 *  `MeshBufferError` for a malformed buffer. */
export async function loadMesh(revisionId: string, objectKey: number): Promise<MeshBuffer> {
  if (!desktopAvailable()) {
    const buffer = (await requireWebFixture()).meshes[revisionId]?.[objectKey];
    if (!buffer) throw notFound(`${revisionId}/${objectKey}`);
    return decodeMeshBuffer(buffer.slice(0));
  }
  return decodeMeshBuffer(await binaryCommand("get_revision_mesh", { revisionId, objectKey }));
}

// --- Preparations (D5) --------------------------------------------------------------

/** Returns the Model's Preparation, creating it if there is none. */
export async function createPreparation(modelId: string, target?: SliceTarget): Promise<PreparationRecord> {
  if (!desktopAvailable()) return webCreatePreparation(modelId, target);
  const preparation = await command("create_preparation", target ? { modelId, target } : { modelId });
  settlePreparation(preparation, "result");
  return state.preparations[preparation.modelId] ?? preparation;
}

/** Saves the whole document. Rejects with `VALIDATION` (its `fieldPath`
 *  in `details`) or `CONFLICT` (after reloading) for the caller. */
export async function updatePreparation(preparationId: string, document: PreparationDocument): Promise<PreparationRecord> {
  if (!desktopAvailable()) return webUpdatePreparation(preparationId, document);
  const expectedRevision = heldPreparation(preparationId).revision;
  const preparation = await withConflictRefresh(() => command("update_preparation", {
    preparationId, expectedRevision, document,
  }));
  settlePreparation(preparation, "result");
  return preparation;
}

/** D5 reload: moves a stale Preparation onto the Model's current revision. */
export async function reloadPreparation(preparationId: string): Promise<ReloadPreparationData> {
  if (!desktopAvailable()) {
    return { preparation: heldPreparation(preparationId), removedObjectKeys: [], addedObjectKeys: [] };
  }
  const expectedRevision = heldPreparation(preparationId).revision;
  const result = await withConflictRefresh(() => command("reload_preparation", { preparationId, expectedRevision }));
  settlePreparation(result.preparation, "result");
  return result;
}

export async function deletePreparation(preparationId: string): Promise<void> {
  const expectedRevision = heldPreparation(preparationId).revision;
  if (desktopAvailable()) {
    await withConflictRefresh(() => command("delete_preparation", { preparationId, expectedRevision }));
  }
  dropPreparation(preparationId);
}

/** Records (or with `null`, withdraws) **Continue with revision M** for a
 *  stale Preparation: `sourceRevisionId` is the revision it is pinned to.
 *  Read it back with `slicing.continueWithSourceRevision`. */
export function chooseContinueWithSourceRevision(preparationId: string, sourceRevisionId: string | null): void {
  setState("continueWith", preparationId, sourceRevisionId ?? undefined);
}

// --- Slice operations (D9, D10) ---------------------------------------------------------

/** Starts one operation per plate. Sends one `operationId` (generated
 *  unless given) and retries a transport failure once with the same id, so
 *  the backend replays rather than slicing twice (D10). Rejects with the
 *  backend's error unchanged, e.g. `PREPARATION_STALE` (recovery
 *  `RELOAD_PREPARATION`) or `SLICER_UNAVAILABLE` (`OPEN_SLICER_SETTINGS`).
 *  Web mode has no slicer. */
export async function startSlice(
  preparationId: string,
  plateKeys: string[],
  options: { continueWithSourceRevision?: string; operationId?: string } = {},
): Promise<SliceOperationRecord[]> {
  if (!desktopAvailable()) throw desktopOnlyError("startSlice");
  const expectedRevision = heldPreparation(preparationId).revision;
  const operationId = options.operationId ?? crypto.randomUUID();
  const { operations } = await withConflictRefresh(() => retryOnTransportFailure(() => command("start_slice", {
    operationId,
    preparationId,
    expectedRevision,
    plateKeys,
    ...(options.continueWithSourceRevision ? { continueWithSourceRevision: options.continueWithSourceRevision } : {}),
  })));
  for (const operation of operations) settleOperation(operation, "result");
  return operations;
}

/** Rejects with `OPERATION_NOT_CANCELLABLE` once the operation has
 *  finished. */
export async function cancelSliceOperation(sliceOperationId: string): Promise<SliceOperationRecord> {
  if (!desktopAvailable()) throw desktopOnlyError("cancelSliceOperation");
  const operation = await command("cancel_slice_operation", { sliceOperationId });
  settleOperation(operation, "result");
  return operation;
}

/** Not cached: the backend stores an operation's log only once it
 *  finishes, so a queued or running operation's log is empty (and one
 *  that never ran stays empty). */
export async function loadOperationLog(sliceOperationId: string): Promise<SliceOperationLog> {
  if (!desktopAvailable()) {
    const log = (await requireWebFixture()).logs[sliceOperationId];
    if (!log) throw notFound(sliceOperationId);
    return clone(log);
  }
  return command("get_slice_operation_log", { sliceOperationId });
}

// --- Slice Revisions (D14–D16) ----------------------------------------------------------

/** Newest first. Merges into the held list (revisions are immutable, so
 *  there is nothing to reconcile but additions). */
export async function loadSliceRevisions(modelId: string): Promise<SliceRevisionSummary[]> {
  if (!desktopAvailable()) return [...slicing.revisions(modelId)];
  const revisions = await command("list_slice_revisions", { modelId });
  for (const revision of revisions) settleRevision(revision);
  return revisions;
}

/** Slice Revisions are immutable, so a record is cached per id for the life
 *  of the app. A failed load is not cached. */
const revisionRecords = new Map<string, Promise<SliceRevisionRecord>>();

export function loadSliceRevision(sliceRevisionId: string): Promise<SliceRevisionRecord> {
  const cached = revisionRecords.get(sliceRevisionId);
  if (cached) return cached;
  const load = (async () => {
    if (desktopAvailable()) return command("get_slice_revision", { sliceRevisionId });
    const record = (await requireWebFixture()).revisionRecords[sliceRevisionId];
    if (!record || removedRevisions.has(sliceRevisionId)) throw notFound(sliceRevisionId);
    return clone(record);
  })();
  revisionRecords.set(sliceRevisionId, load);
  load.catch(() => revisionRecords.delete(sliceRevisionId));
  return load;
}

/** D16. Sends one `operationId` (generated unless given) and retries a
 *  transport failure once with the same id. Rejects with `VALIDATION`
 *  (`fieldPath` under `facts.`) for the dialog to show inline. Web mode
 *  has no G-code file to point at. */
export async function createExternalSliceRevision(
  sourceRevisionId: string,
  facts: CreateExternalSliceRevisionFacts,
  operationId: string = crypto.randomUUID(),
): Promise<SliceRevisionRecord> {
  if (!desktopAvailable()) throw desktopOnlyError("createExternalSliceRevision");
  const record = await retryOnTransportFailure(() => command("create_external_slice_revision", {
    operationId, sourceRevisionId, facts,
  }));
  revisionRecords.set(record.id, Promise.resolve(record));
  settleRevision(revisionSummaryOf(record));
  return record;
}

/** Rejects with `LIFECYCLE_BLOCKED` while something still depends on the
 *  revision. */
export async function deleteSliceRevision(sliceRevisionId: string): Promise<void> {
  if (desktopAvailable()) await command("delete_slice_revision", { sliceRevisionId });
  else if (!findRevision(sliceRevisionId)) throw notFound(sliceRevisionId);
  dropRevision(sliceRevisionId);
}

// --- Web-mode local Preparation edits ------------------------------------------------

/** Web mode only: the fixtures load on first use, so they stay out of the
 *  desktop bundle's main chunk. */
async function requireWebFixture(): Promise<WebSlicingFixture> {
  if (!webFixture) {
    const { buildWebSlicingFixture } = await import("./web-fixtures");
    webFixture ??= buildWebSlicingFixture();
  }
  return webFixture;
}

/** Web-mode creates in flight, per Model: a second call joins the first. */
const pendingWebCreates = new Map<string, Promise<PreparationRecord>>();

/** A simplified D5 seed (see `web-preparations.ts`). Single-flight per
 *  Model, like the backend's "return the existing one". */
function webCreatePreparation(modelId: string, target?: SliceTarget): Promise<PreparationRecord> {
  const pending = pendingWebCreates.get(modelId);
  if (pending) return pending;
  const create = seedWebPreparation(modelId, target).finally(() => pendingWebCreates.delete(modelId));
  pendingWebCreates.set(modelId, create);
  return create;
}

async function seedWebPreparation(modelId: string, target?: SliceTarget): Promise<PreparationRecord> {
  const fixture = await requireWebFixture();
  const [{ buildWebLibraryFixture }, { seedLocalPreparation }] = await Promise.all([
    import("../library/web-fixtures"),
    import("./web-preparations"),
  ]);
  // Checked after the awaits, so a Preparation made meanwhile (by an
  // event or an earlier create) is returned, not replaced.
  const existing = state.preparations[modelId];
  if (existing) return existing;
  const model = buildWebLibraryFixture().models.find((m) => m.id === modelId);
  if (!model) throw notFound(modelId);
  const preparation = seedLocalPreparation(fixture, model, target);
  settlePreparation(preparation, "result");
  return preparation;
}

async function webUpdatePreparation(preparationId: string, document: PreparationDocument): Promise<PreparationRecord> {
  const { validateLocalDocument } = await import("./web-preparations");
  const held = heldPreparation(preparationId);
  validateLocalDocument(document);
  const preparation: PreparationRecord = {
    ...held,
    document: clone(document),
    revision: held.revision + 1,
    updatedAt: new Date().toISOString(),
  };
  settlePreparation(preparation, "result");
  return preparation;
}
