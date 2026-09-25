import { createStore, reconcile } from "solid-js/store";
import {
  binaryCommand,
  command,
  desktopAvailable,
  isCommandError,
  retryOnTransportFailure,
} from "../ipc/client";
import { createSequencedStream } from "../ipc/sequenced-stream";
import { buildWebLibraryFixture } from "../library/web-fixtures";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { ErrorCode } from "../generated/contracts/command/ErrorCode";
import { desktopOnlyError } from "./desktop-only";
import { decodeMeshBuffer, type MeshBuffer } from "./mesh-buffer";
import { buildWebSlicingFixture, type WebSlicingFixture } from "./web-fixtures";
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
}

const [state, setState] = createStore<SlicingState>({
  runtime: null,
  preparations: {},
  operations: [],
  revisionsByModel: {},
  progress: {},
  status: "idle",
  syncState: "current",
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

// --- Errors -------------------------------------------------------------------

function commandError(code: ErrorCode, message: string, details?: Record<string, string>): CommandError {
  return {
    contractVersion: 1,
    code,
    message,
    recovery: code === "VALIDATION" ? ["EDIT_FIELDS"] : [],
    retryable: false,
    ...(details ? { details } : {}),
  };
}

function notFound(id: string): CommandError {
  return commandError("NOT_FOUND", id, { entityId: id });
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
 *  strictly newer and can never regress an event. A record with a new id
 *  for the same Model replaces the old one (it was deleted). */
function settlePreparation(record: PreparationRecord, from: "event" | "result"): void {
  if (removedPreparations.has(record.id)) return;
  const existing = state.preparations[record.modelId];
  if (existing && existing.id === record.id) {
    const stale = from === "event" ? record.revision < existing.revision : record.revision <= existing.revision;
    if (stale) return;
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

function byNewest(a: SliceRevisionSummary, b: SliceRevisionSummary): number {
  return b.createdAt.localeCompare(a.createdAt);
}

/** Slice Revisions are immutable (D1), so one is only ever added or
 *  removed. */
function settleRevision(summary: SliceRevisionSummary): void {
  if (removedRevisions.has(summary.id)) return;
  const list = state.revisionsByModel[summary.modelId] ?? [];
  if (list.some((r) => r.id === summary.id)) return;
  setState("revisionsByModel", summary.modelId, [...list, summary].sort(byNewest));
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
  for (const list of Object.values(grouped)) list?.sort(byNewest);
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
    webFixture = buildWebSlicingFixture();
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
  if (!desktopAvailable()) return clone(requireWebFixture().sliceOptions);
  return command("list_slice_options", { target });
}

/** D6: a Model Source Revision's objects and build items. Unprintable build
 *  items are `buildItems[].printable === false`. Not cached here; the
 *  spec's `geometry-cache.ts` caches for the preparation view. */
export async function loadGeometry(revisionId: string): Promise<RevisionGeometry> {
  if (!desktopAvailable()) {
    const geometry = requireWebFixture().geometry[revisionId];
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
    const buffer = requireWebFixture().meshes[revisionId]?.[objectKey];
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

/** Not cached: a running operation's log grows. */
export async function loadOperationLog(sliceOperationId: string): Promise<SliceOperationLog> {
  if (!desktopAvailable()) {
    const log = requireWebFixture().logs[sliceOperationId];
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
    const record = requireWebFixture().revisionRecords[sliceRevisionId];
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

const MAX_PLATES = 36;

function requireWebFixture(): WebSlicingFixture {
  webFixture ??= buildWebSlicingFixture();
  return webFixture;
}

function validationError(fieldPath: string, message: string): CommandError {
  return commandError("VALIDATION", message, { fieldPath });
}

/** The backend's document rules that need no geometry: 1–36 plates and
 *  unique, non-empty plate and instance keys. */
function validateDocument(document: PreparationDocument): void {
  if (document.plates.length < 1 || document.plates.length > MAX_PLATES) {
    throw validationError("document.plates", `A Preparation has 1 to ${MAX_PLATES} plates.`);
  }
  const plateKeys = new Set<string>();
  const instanceKeys = new Set<string>();
  document.plates.forEach((plate, p) => {
    if (!plate.plateKey.trim() || plateKeys.has(plate.plateKey)) {
      throw validationError(`document.plates[${p}].plateKey`, "Each plate needs its own key.");
    }
    plateKeys.add(plate.plateKey);
    plate.instances.forEach((instance, i) => {
      if (!instance.instanceKey.trim() || instanceKeys.has(instance.instanceKey)) {
        throw validationError(`document.plates[${p}].instances[${i}].instanceKey`, "Each object on a plate needs its own key.");
      }
      instanceKeys.add(instance.instanceKey);
    });
  });
}

/** A simplified D5 seed: one plate per source plate (or one plate), each
 *  printable build item once at the bed's centre, and the default
 *  presets. */
function webCreatePreparation(modelId: string, target?: SliceTarget): PreparationRecord {
  const existing = state.preparations[modelId];
  if (existing) return existing;
  const fixture = requireWebFixture();
  const model = buildWebLibraryFixture().models.find((m) => m.id === modelId);
  if (!model) throw notFound(modelId);
  if (model.format === "gcode") {
    throw validationError("modelId", "A G-code Model is already sliced, so it has no Preparation.");
  }
  const geometry = fixture.geometry[model.currentRevision.id];
  if (!geometry) throw notFound(model.currentRevision.id);
  const options = fixture.sliceOptions;
  const bed = options.profileSnapshot.bedShape;
  const center: [number, number] = bed.kind === "rectangular"
    ? [bed.originXMm + bed.widthMm / 2, bed.originYMm + bed.depthMm / 2]
    : [0, 0];
  const plates = new Map<number, PreparationDocument["plates"][number]>();
  for (const item of geometry.buildItems.filter((i) => i.printable)) {
    const index = item.plateIndex ?? 1;
    const plate = plates.get(index) ?? { plateKey: crypto.randomUUID(), instances: [] };
    plate.instances.push({
      instanceKey: crypto.randomUUID(),
      objectKey: item.objectKey,
      transform: { translateMm: center, rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
    });
    plates.set(index, plate);
  }
  const now = new Date().toISOString();
  const preparation: PreparationRecord = {
    id: `prp-web-${crypto.randomUUID()}`,
    modelId,
    sourceRevisionId: model.currentRevision.id,
    revision: 1,
    stale: false,
    document: {
      plates: [...plates.entries()].sort(([a], [b]) => a - b).map(([, plate]) => plate),
      target: target ?? { kind: "profile", catalogRef: { ...options.profileSnapshot.catalogRef } },
      ...(options.defaults.processPreset ? { processPreset: options.defaults.processPreset } : {}),
      ...(options.defaults.filamentPreset ? { filamentPreset: options.defaults.filamentPreset } : {}),
      controls: {},
    },
    createdAt: now,
    updatedAt: now,
  };
  settlePreparation(preparation, "result");
  return preparation;
}

function webUpdatePreparation(preparationId: string, document: PreparationDocument): PreparationRecord {
  const held = heldPreparation(preparationId);
  validateDocument(document);
  const preparation: PreparationRecord = {
    ...held,
    document: clone(document),
    revision: held.revision + 1,
    updatedAt: new Date().toISOString(),
  };
  settlePreparation(preparation, "result");
  return preparation;
}
