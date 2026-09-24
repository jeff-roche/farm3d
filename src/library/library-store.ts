import { createStore } from "solid-js/store";
import { command, desktopAvailable, isCommandError, retryOnTransportFailure } from "../ipc/client";
import { createSequencedStream } from "../ipc/sequenced-stream";
import { buildWebLibraryFixture, type WebLibraryFixture } from "./web-fixtures";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { ErrorCode } from "../generated/contracts/command/ErrorCode";
import {
  isLibraryEvent,
  type ImportInspection,
  type ImportItemRequest,
  type ImportModelsResult,
  type ImportProgress,
  type ImportSelectionSummary,
  type LibraryContentInfo,
  type LibraryEvent,
  type LibrarySnapshot,
  type ModelRecord,
  type ModelSourceRevisionRecord,
  type ModelSourceRevisionSummary,
  type ProjectRecord,
  type SelectionPurpose,
} from "./types";

/** The only owner of Library data (spec §Frontend State). A module-level
 *  Solid store: read `library.*()` inside a tracked scope for Solid to pick
 *  up changes.
 *
 *  Error handling follows P3's inline-versus-banner split: every mutation
 *  rejects, so a dialog can show the failure inline next to what the user
 *  must fix. A caller with no inline UI of its own (a menu item, a chip, a
 *  focus-triggered source check) catches the rejection and passes it to
 *  `reportLibraryError`, which raises the Library banner. Never both, or
 *  the same failure shows twice. */

type LibraryStatus = "idle" | "loading" | "ready" | "error";
type LibrarySyncState = "syncing" | "current" | "uncertain";

interface LibraryState {
  projects: ProjectRecord[];
  models: ModelRecord[];
  status: LibraryStatus;
  syncState: LibrarySyncState;
  error: string | null;
  contentInfo: LibraryContentInfo | null;
}

const [state, setState] = createStore<LibraryState>({
  projects: [],
  models: [],
  status: "idle",
  syncState: "current",
  error: null,
  contentInfo: null,
});

export const library = {
  projects: (): ProjectRecord[] => state.projects,
  models: (): ModelRecord[] => state.models,
  status: (): LibraryStatus => state.status,
  syncState: (): LibrarySyncState => state.syncState,
  error: (): string | null => state.error,
  /** The content store's totals. Always `null` in web mode, which has no
   *  content store. */
  contentInfo: (): LibraryContentInfo | null => state.contentInfo,
};

const LOAD_ERROR = "The Library could not load.";
const LISTEN_ERROR = "The Library could not start updating live.";
const SOURCE_CHECK_INTERVAL_MS = 30_000;

// --- Errors -----------------------------------------------------------------

export function reportLibraryError(error: unknown): void {
  setState("error", isCommandError(error) ? error.message : "The operation could not be completed.");
}

export function dismissLibraryError(): void {
  setState("error", null);
}

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

/** Web mode has no files and no content store, so anything that reads real
 *  files is refused rather than faked. */
function needsDesktop(action: string): CommandError {
  return commandError("PERSISTENCE_UNAVAILABLE", `${action} needs the desktop app.`);
}

function notFound(kind: "Project" | "Model", id: string): CommandError {
  return commandError("NOT_FOUND", `This ${kind} no longer exists.`, { entityId: id });
}

function heldProject(id: string): ProjectRecord {
  const project = state.projects.find((p) => p.id === id);
  if (!project) throw notFound("Project", id);
  return project;
}

function heldModel(id: string): ModelRecord {
  const model = state.models.find((m) => m.id === id);
  if (!model) throw notFound("Model", id);
  return model;
}

// --- Settling records ---------------------------------------------------------

/** Ids removed while this store has been running. Ids are never reused, so
 *  a command result that arrives after its record's removal event is
 *  dropped instead of bringing the record back. */
const removedProjects = new Set<string>();
const removedModels = new Set<string>();

/** Membership changes never bump a Project's `revision`, only its
 *  `modelCount`, so a stream event (already ordered by sequence) applies
 *  at an equal revision. A command result has no sequence to order it, so
 *  it applies only when strictly newer and can never regress an event. */
function settleProject(record: ProjectRecord, from: "event" | "result"): void {
  if (removedProjects.has(record.id)) return;
  const existing = state.projects.find((p) => p.id === record.id);
  if (existing && (from === "event" ? record.revision < existing.revision : record.revision <= existing.revision)) return;
  setState("projects", (list) => (existing ? list.map((p) => (p.id === record.id ? record : p)) : [...list, record]));
}

/** Every Model change bumps its `revision` (D1, D15), so both events and
 *  command results apply only when strictly newer (D17). */
function settleModel(record: ModelRecord): void {
  if (removedModels.has(record.id)) return;
  const existing = state.models.find((m) => m.id === record.id);
  if (existing && record.revision <= existing.revision) return;
  setState("models", (list) => (existing ? list.map((m) => (m.id === record.id ? record : m)) : [...list, record]));
}

function dropProject(id: string): void {
  removedProjects.add(id);
  setState("projects", (list) => list.filter((p) => p.id !== id));
}

function dropModel(id: string): void {
  removedModels.add(id);
  setState("models", (list) => list.filter((m) => m.id !== id));
}

// --- Ephemeral event handlers ----------------------------------------------------

const droppedHandlers = new Set<(summary: ImportSelectionSummary) => void>();
const progressHandlers = new Set<(selectionId: string, progress: ImportProgress) => void>();

export function onSelectionDropped(handler: (summary: ImportSelectionSummary) => void): () => void {
  droppedHandlers.add(handler);
  return () => droppedHandlers.delete(handler);
}

export function onImportProgress(handler: (selectionId: string, progress: ImportProgress) => void): () => void {
  progressHandlers.add(handler);
  return () => progressHandlers.delete(handler);
}

const revisionHandlers = new Set<(revision: ModelSourceRevisionSummary) => void>();

/** `library.revision.created`, in stream order (after the Model's own
 *  `library.model.changed`), so the details panel can say a linked source
 *  was captured. The snapshot has no such events, so one a backfill covers
 *  is not replayed. */
export function onRevisionCreated(handler: (revision: ModelSourceRevisionSummary) => void): () => void {
  revisionHandlers.add(handler);
  return () => revisionHandlers.delete(handler);
}

/** Drops and progress are not in any snapshot, so they are delivered the
 *  moment they arrive -- even mid-backfill, when an ordinary event would
 *  still be buffered. The stream still sees them, for gap detection. */
function deliverEphemeral(event: LibraryEvent): void {
  if (event.type === "library.selection.dropped") {
    for (const handler of droppedHandlers) handler(event.payload as ImportSelectionSummary);
  } else if (event.type === "library.import.progress") {
    for (const handler of progressHandlers) handler(event.subject.id, event.payload as ImportProgress);
  }
}

function applyEvent(event: LibraryEvent): void {
  switch (event.type) {
    case "library.project.changed":
      settleProject(event.payload as ProjectRecord, "event");
      break;
    case "library.project.removed":
      dropProject(event.subject.id);
      break;
    case "library.model.changed":
      settleModel(event.payload as ModelRecord);
      break;
    case "library.model.removed":
      dropModel(event.subject.id);
      break;
    case "library.revision.created":
      // The record itself comes with a `library.model.changed` carrying
      // the new `currentRevision`; this only tells the listeners.
      for (const handler of revisionHandlers) handler(event.payload as ModelSourceRevisionSummary);
      break;
    default:
      // Drops and progress were delivered on arrival.
      break;
  }
}

// --- Startup (listen-before-backfill, D17) --------------------------------------

type LibraryStream = ReturnType<typeof createSequencedStream<LibraryEvent, LibrarySnapshot>>;

let activeStream: LibraryStream | undefined;
let activeUnlisten: (() => void) | undefined;
let webFixture: WebLibraryFixture | undefined;

function disposeListener(): void {
  activeStream?.dispose();
  activeStream = undefined;
  activeUnlisten?.();
  activeUnlisten = undefined;
}

async function refreshContentInfo(): Promise<void> {
  try {
    setState("contentInfo", await command("library_content_info"));
  } catch {
    // Only the status line reads this; it keeps its last value.
  }
}

function applySnapshot(snapshot: LibrarySnapshot): void {
  const wasReady = state.status === "ready";
  setState({ projects: snapshot.projects, models: snapshot.models, status: "ready" });
  if (!wasReady && (state.error === LOAD_ERROR || state.error === LISTEN_ERROR)) setState("error", null);
  void refreshContentInfo();
}

/** Subscribes to `library.*` events, then backfills through `list_library`,
 *  so nothing between the two is missed. Idempotent: calling it again
 *  disposes the previous listener first. Resolves once the first backfill
 *  has settled (successfully or not; a failure keeps retrying) with this
 *  start's own disposer. */
export async function startLibrary(): Promise<() => void> {
  disposeListener();
  if (!desktopAvailable()) {
    webFixture = buildWebLibraryFixture();
    setState({
      projects: webFixture.projects,
      models: webFixture.models,
      status: "ready",
      syncState: "current",
      error: null,
      contentInfo: null,
    });
    return () => {};
  }

  setState({ syncState: "syncing", ...(state.status === "ready" ? {} : { status: "loading" }) });
  const stream: LibraryStream = createSequencedStream<LibraryEvent, LibrarySnapshot>({
    backfill: () => command("list_library"),
    applySnapshot,
    applyEvent,
    onSyncState: (syncState) => setState("syncState", syncState),
    onBackfillError: () => {
      if (state.status !== "ready") setState({ status: "error", error: LOAD_ERROR });
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
      if (!isLibraryEvent(candidate)) return;
      deliverEphemeral(candidate);
      stream.receive(candidate);
    });
    if (activeStream !== stream) {
      unlisten();
      return () => {};
    }
    activeUnlisten = unlisten;
  } catch {
    dispose();
    setState({ status: "error", syncState: "uncertain", error: LISTEN_ERROR });
    return () => {};
  }

  await stream.start();
  return dispose;
}

/** The workspace's **Refresh** while the Library may be out of date:
 *  backfill now rather than wait for the stream's backoff. Web mode has no
 *  stream to refresh. */
export function refreshLibrary(): void {
  activeStream?.resync();
}

/** A `CONFLICT` means this store holds a stale revision: reload, so a retry
 *  reads the fresh one, then rethrow for the caller to report. */
async function withConflictRefresh<T>(run: () => Promise<T>): Promise<T> {
  try {
    return await run();
  } catch (error) {
    if (isCommandError(error) && error.code === "CONFLICT") activeStream?.resync();
    throw error;
  }
}

// --- Web-mode local edits --------------------------------------------------------

const MAX_PROJECT_NAME_CHARS = 128;
const MAX_MODEL_NAME_CHARS = 255;
const MAX_MEMBERSHIP_CHANGES = 64;

function validName(name: string, max: number, message: string): string {
  const trimmed = name.trim();
  const length = [...trimmed].length;
  if (length < 1 || length > max) throw commandError("VALIDATION", message, { fieldPath: "name" });
  return trimmed;
}

function validProjectName(name: string, excludingId?: string): string {
  const trimmed = validName(name, MAX_PROJECT_NAME_CHARS, "The Project name must be 1-128 characters.");
  const folded = trimmed.toLowerCase();
  if (state.projects.some((p) => p.id !== excludingId && p.name.toLowerCase() === folded)) {
    throw commandError("VALIDATION", "A Project with this name already exists.", { fieldPath: "name" });
  }
  return trimmed;
}

function byProjectName(ids: Iterable<string>): string[] {
  const nameOf = (id: string) => state.projects.find((p) => p.id === id)?.name ?? "";
  return [...ids].sort((a, b) => nameOf(a).localeCompare(nameOf(b)));
}

function adjustModelCounts(projectIds: string[], delta: number): void {
  setState("projects", (list) => list.map((p) => (
    projectIds.includes(p.id) ? { ...p, modelCount: p.modelCount + delta } : p
  )));
}

function webCreateProject(name: string): ProjectRecord {
  const now = new Date().toISOString();
  const project: ProjectRecord = {
    id: `prj-web-${crypto.randomUUID()}`,
    revision: 1,
    name: validProjectName(name),
    modelCount: 0,
    createdAt: now,
    updatedAt: now,
  };
  settleProject(project, "result");
  return project;
}

function webRenameProject(id: string, name: string): void {
  const project = heldProject(id);
  const renamed = validProjectName(name, id);
  settleProject({ ...project, name: renamed, revision: project.revision + 1, updatedAt: new Date().toISOString() }, "result");
}

function webDeleteProject(id: string): void {
  heldProject(id);
  const now = new Date().toISOString();
  for (const model of state.models.filter((m) => m.projectIds.includes(id))) {
    settleModel({ ...model, projectIds: model.projectIds.filter((p) => p !== id), revision: model.revision + 1, updatedAt: now });
  }
  dropProject(id);
}

function webUpdateModel(id: string, patch: { name: string }): void {
  const model = heldModel(id);
  const name = validName(patch.name, MAX_MODEL_NAME_CHARS, "The Model name must be 1-255 characters.");
  settleModel({ ...model, name, revision: model.revision + 1, updatedAt: new Date().toISOString() });
}

function webSetModelProjects(id: string, change: { add: string[]; remove: string[] }): void {
  const model = heldModel(id);
  if (change.add.length > MAX_MEMBERSHIP_CHANGES || change.remove.length > MAX_MEMBERSHIP_CHANGES) {
    throw commandError("VALIDATION", "Change at most 64 Projects at once.", { fieldPath: "add" });
  }
  if (change.add.some((p) => change.remove.includes(p))) {
    throw commandError("VALIDATION", "A Project can't be both added and removed.", { fieldPath: "remove" });
  }
  for (const projectId of [...change.add, ...change.remove]) heldProject(projectId);
  const next = new Set(model.projectIds);
  for (const projectId of change.add) next.add(projectId);
  for (const projectId of change.remove) next.delete(projectId);
  const added = [...next].filter((p) => !model.projectIds.includes(p));
  const removed = model.projectIds.filter((p) => !next.has(p));
  if (added.length === 0 && removed.length === 0) return;
  settleModel({ ...model, projectIds: byProjectName(next), revision: model.revision + 1, updatedAt: new Date().toISOString() });
  adjustModelCounts(added, 1);
  adjustModelCounts(removed, -1);
}

function webDeleteModel(id: string): void {
  const model = heldModel(id);
  dropModel(id);
  adjustModelCounts(model.projectIds, -1);
}

// --- Project and Model edits -------------------------------------------------------

/** Rejects (a `VALIDATION` on `name`, for example) for the dialog to show
 *  inline. */
export async function createProject(name: string): Promise<ProjectRecord> {
  if (!desktopAvailable()) return webCreateProject(name);
  const { project } = await command("create_project", { name });
  settleProject(project, "result");
  return project;
}

export async function renameProject(id: string, name: string): Promise<void> {
  if (!desktopAvailable()) return webRenameProject(id, name);
  const expectedRevision = heldProject(id).revision;
  const { project } = await withConflictRefresh(() => command("rename_project", { id, expectedRevision, name }));
  settleProject(project, "result");
}

/** D18: never deletes a Model. The affected Models lose the membership
 *  here; their own `library.model.changed` events bring the bumped
 *  revisions. */
export async function deleteProject(id: string): Promise<void> {
  if (!desktopAvailable()) return webDeleteProject(id);
  const expectedRevision = heldProject(id).revision;
  const result = await withConflictRefresh(() => command("delete_project", { id, expectedRevision }));
  dropProject(result.deletedId);
  setState("models", (list) => list.map((m) => (
    result.affectedModelIds.includes(m.id) ? { ...m, projectIds: m.projectIds.filter((p) => p !== result.deletedId) } : m
  )));
}

export async function updateModel(id: string, patch: { name: string }): Promise<void> {
  if (!desktopAvailable()) return webUpdateModel(id, patch);
  const expectedRevision = heldModel(id).revision;
  const { model } = await withConflictRefresh(() => command("update_model", { id, expectedRevision, patch }));
  settleModel(model);
}

export async function setModelProjects(id: string, change: { add: string[]; remove: string[] }): Promise<void> {
  if (!desktopAvailable()) return webSetModelProjects(id, change);
  const expectedRevision = heldModel(id).revision;
  const { model } = await withConflictRefresh(() => command("set_model_projects", {
    modelId: id, expectedRevision, add: change.add, remove: change.remove,
  }));
  settleModel(model);
}

export async function deleteModel(id: string): Promise<void> {
  if (!desktopAvailable()) return webDeleteModel(id);
  const expectedRevision = heldModel(id).revision;
  const { deletedId } = await withConflictRefresh(() => command("delete_model", { id, expectedRevision }));
  dropModel(deletedId);
  void refreshContentInfo();
}

// --- Revisions and thumbnails ---------------------------------------------------------

/** Newest first. Not cached: the history grows with linked changes. */
export async function loadRevisions(modelId: string): Promise<ModelSourceRevisionRecord[]> {
  if (!desktopAvailable()) return [...(webFixture?.revisions[modelId] ?? [])];
  return command("list_model_revisions", { modelId });
}

/** Revisions are immutable, so a thumbnail (or its absence) is cached per
 *  revision id for the life of the app. A failed load is not cached. */
const thumbnails = new Map<string, Promise<string | null>>();

export function loadThumbnail(revisionId: string): Promise<string | null> {
  const cached = thumbnails.get(revisionId);
  if (cached) return cached;
  const load = (async () => {
    const thumbnail = desktopAvailable()
      ? await command("get_revision_thumbnail", { revisionId })
      : webFixture?.thumbnails[revisionId] ?? null;
    return thumbnail ? `data:${thumbnail.mediaType};base64,${thumbnail.dataBase64}` : null;
  })();
  thumbnails.set(revisionId, load);
  load.catch(() => thumbnails.delete(revisionId));
  return load;
}

// --- Import (the dialog renders every failure inline) ---------------------------

/** `null` when the user cancels the picker. */
export async function pickFiles(purpose: SelectionPurpose): Promise<ImportSelectionSummary | null> {
  if (!desktopAvailable()) throw needsDesktop(purpose === "import" ? "Importing Models" : "Locating a source file");
  return command("pick_model_files", { purpose });
}

export async function inspectSelection(selectionId: string): Promise<ImportInspection> {
  if (!desktopAvailable()) throw needsDesktop("Importing Models");
  return command("inspect_import_selection", { selectionId });
}

/** Sends one `operationId` (generated unless given) and retries a transport
 *  failure once with the same id, so the backend replays rather than
 *  importing twice (D13). */
export async function importModels(
  selectionId: string,
  items: ImportItemRequest[],
  operationId: string = crypto.randomUUID(),
): Promise<ImportModelsResult> {
  if (!desktopAvailable()) throw needsDesktop("Importing Models");
  const result = await retryOnTransportFailure(() => command("import_models", { selectionId, operationId, items }));
  for (const item of result.items) if (item.model) settleModel(item.model);
  void refreshContentInfo();
  return result;
}

/** Cancelling an unknown selection is a no-op, and web mode has none. */
export async function cancelSelection(selectionId: string): Promise<void> {
  if (!desktopAvailable()) return;
  await command("cancel_import_selection", { selectionId });
}

// --- Linked sources ---------------------------------------------------------------

let lastSourceCheckAt: number | undefined;

/** D15 trigger 3. Throttled to one call per 30 s -- the Library calls it
 *  when it becomes visible and when the window regains focus -- unless
 *  `force` (the user's **Check sources**). A failed call still counts
 *  toward the throttle, so a focus storm can't retry it in a loop. Settles
 *  the Models that changed. */
export async function checkSources(modelIds?: string[], options: { force?: boolean } = {}): Promise<void> {
  if (!desktopAvailable()) throw needsDesktop("Checking linked sources");
  const now = Date.now();
  if (!options.force && lastSourceCheckAt !== undefined && now - lastSourceCheckAt < SOURCE_CHECK_INTERVAL_MS) return;
  lastSourceCheckAt = now;
  const changed = await command("check_linked_sources", modelIds ? { modelIds } : {});
  for (const model of changed) settleModel(model);
}

/** D16. Rejects with `SOURCE_CONTENT_DIFFERS` when the located file's bytes
 *  differ and `acceptDifferentContent` is false, for the dialog to offer
 *  **Relink and import as a new revision**. */
export async function locateSource(
  modelId: string,
  selectionId: string,
  fileIndex: number,
  acceptDifferentContent: boolean,
): Promise<ModelRecord> {
  if (!desktopAvailable()) throw needsDesktop("Locating a source file");
  const expectedRevision = heldModel(modelId).revision;
  const { model } = await withConflictRefresh(() => command("locate_linked_source", {
    modelId, expectedRevision, selectionId, fileIndex, acceptDifferentContent,
  }));
  settleModel(model);
  return model;
}

export async function convertToManaged(modelId: string): Promise<void> {
  if (!desktopAvailable()) throw needsDesktop("Converting to managed storage");
  const expectedRevision = heldModel(modelId).revision;
  const { model } = await withConflictRefresh(() => command("convert_model_to_managed", { modelId, expectedRevision }));
  settleModel(model);
  void refreshContentInfo();
}
