import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable, needsDesktopError, retryOnTransportFailure } from "../ipc/client";
import { createSequencedStream } from "../ipc/sequenced-stream";
import { printers, printerStoreStatus } from "../printers/printer-store";
import { isHostOperationsEvent, isTerminalHostOperationState } from "./types";
import type { HostOperation, HostOperationsEvent, HostOperationsSnapshot, PriorState } from "./types";

/** The only owner of Host Operations (spec "Frontend architecture", State).
 *  Listen-before-backfill over `hostOperations.*`, copying
 *  `src/slicing/slicing-store.ts`'s pattern: subscribe first, then
 *  backfill through `list_host_operations`, so nothing between the two is
 *  missed. */

export type HostOperationsStatus = "idle" | "loading" | "ready" | "error";
export type HostOperationsSyncState = "syncing" | "current" | "uncertain";

interface HostOperationsState {
  operations: HostOperation[];
  status: HostOperationsStatus;
  syncState: HostOperationsSyncState;
}

const [state, setState] = createStore<HostOperationsState>({
  operations: [],
  status: "idle",
  syncState: "current",
});

/** The newest `succeeded` upload per `hostPath`, for one Printer (spec
 *  "the staged artifacts"). Comparing `createdAt` as a plain string is the
 *  backend's own ordering (ISO timestamps sort by code unit); ids break a
 *  tie the same way (`created_at DESC, id DESC`). */
function newestByHostPath(uploads: HostOperation[]): HostOperation[] {
  const newestFor = new Map<string, HostOperation>();
  for (const upload of uploads) {
    const held = newestFor.get(upload.hostPath);
    if (!held || upload.createdAt > held.createdAt || (upload.createdAt === held.createdAt && upload.id > held.id)) {
      newestFor.set(upload.hostPath, upload);
    }
  }
  return [...newestFor.values()];
}

/** Spec "State": the store drops rows whose Printer no longer exists
 *  (there is no `removed` event; a deleted Printer's rows just stop
 *  mattering). Until the Printers have loaded nothing is known to be gone,
 *  so every row is kept. */
function livingOperations(): HostOperation[] {
  if (printerStoreStatus() !== "ready") return state.operations;
  const known = new Set(printers().map((printer) => printer.id));
  return state.operations.filter((o) => known.has(o.printerId));
}

export const hostOperations = {
  operations: (): HostOperation[] => livingOperations(),
  operation: (id: string): HostOperation | undefined => livingOperations().find((o) => o.id === id),
  /** At most one unresolved row per Printer (owner decision 2). */
  unresolvedFor: (printerId: string): HostOperation | undefined =>
    livingOperations().find((o) => o.printerId === printerId && !isTerminalHostOperationState(o.state)),
  stagedFor: (printerId: string): HostOperation[] =>
    newestByHostPath(livingOperations().filter((o) => o.printerId === printerId && o.kind === "upload" && o.state === "succeeded")),
  recentFor: (printerId: string): HostOperation[] =>
    livingOperations().filter((o) => o.printerId === printerId && isTerminalHostOperationState(o.state)),
  status: (): HostOperationsStatus => state.status,
  syncState: (): HostOperationsSyncState => state.syncState,
};

function upsertOperation(record: HostOperation): void {
  const index = state.operations.findIndex((o) => o.id === record.id);
  if (index >= 0) setState("operations", index, reconcile(record));
  else setState("operations", (list) => [...list, record]);
}

function applyEvent(event: HostOperationsEvent): void {
  upsertOperation(event.payload);
}

function applySnapshot(snapshot: HostOperationsSnapshot): void {
  setState({ operations: snapshot.operations, status: "ready" });
}

type HostOperationsStream = ReturnType<typeof createSequencedStream<HostOperationsEvent, HostOperationsSnapshot>>;

let activeStream: HostOperationsStream | undefined;
let activeUnlisten: (() => void) | undefined;

function disposeListener(): void {
  activeStream?.dispose();
  activeStream = undefined;
  activeUnlisten?.();
  activeUnlisten = undefined;
}

/** Subscribes to `hostOperations.*` events, then backfills through
 *  `list_host_operations`, so nothing between the two is missed.
 *  Idempotent: calling it again disposes the previous listener first.
 *  Resolves once the first backfill has settled (successfully or not; a
 *  failure keeps retrying) with this start's own disposer. In web mode it
 *  loads `web-fixtures.ts`'s Host Operations instead (mirrors
 *  `slicing-store.ts`). */
export async function startHostOperations(): Promise<() => void> {
  disposeListener();
  if (!desktopAvailable()) {
    const { buildWebHostOpsFixture } = await import("./web-fixtures");
    setState({ operations: buildWebHostOpsFixture().hostOperations, status: "ready", syncState: "current" });
    return () => {};
  }

  setState({ syncState: "syncing", ...(state.status === "ready" ? {} : { status: "loading" }) });
  const stream: HostOperationsStream = createSequencedStream<HostOperationsEvent, HostOperationsSnapshot>({
    backfill: () => command("list_host_operations", {}),
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
      if (isHostOperationsEvent(candidate)) stream.receive(candidate);
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

/** Backfill now rather than wait for the stream's backoff, e.g. after a
 *  `CONFLICT`-style rejection from a Host Operation command. Web mode has
 *  no stream to refresh. */
export function refreshHostOperations(): void {
  activeStream?.resync();
}

/** A command's returned row, when the stream hasn't delivered that row
 *  yet. The command returns right after its write-ahead commit and the
 *  stream's event for it can arrive first, followed by later transitions,
 *  so a row the store already holds is never overwritten by this older
 *  copy. */
function adoptReturned(record: HostOperation): HostOperation {
  if (!state.operations.some((o) => o.id === record.id)) upsertOperation(record);
  return record;
}

/** Runs one user-initiated write. Each call is one user action, so it
 *  gets a fresh client `operationId`; a transport failure is retried once
 *  with that same id, so the backend replays a committed first try instead
 *  of writing twice (never a `CommandError`, which is the backend's
 *  answer). Rejects with the `CommandError` for the caller to render
 *  inline. Web mode has no printer to write to and refuses. Only ids go
 *  over the wire: a credential is never sent or held here. */
async function write(action: string, send: (operationId: string) => Promise<HostOperation>): Promise<HostOperation> {
  if (!desktopAvailable()) throw needsDesktopError(action);
  const operationId = crypto.randomUUID();
  return adoptReturned(await retryOnTransportFailure(() => send(operationId)));
}

/** `stage_slice_revision`: uploads a Slice Revision's G-code to the
 *  Printer. Never starts a print. */
export function stageSliceRevision(printerId: string, sliceRevisionId: string): Promise<HostOperation> {
  return write("Staging on a printer", (operationId) => command("stage_slice_revision", { operationId, printerId, sliceRevisionId }));
}

/** `start_staged_artifact`, with the `priorState` the operator confirmed
 *  the bed for (D9). */
export function startStagedArtifact(printerId: string, hostOperationId: string, priorState: PriorState): Promise<HostOperation> {
  return write("Starting a print", (operationId) =>
    command("start_staged_artifact", { operationId, printerId, hostOperationId, priorState }));
}

export function pauseHostPrint(printerId: string): Promise<HostOperation> {
  return write("Pausing a print", (operationId) => command("pause_host_print", { operationId, printerId }));
}

export function resumeHostPrint(printerId: string): Promise<HostOperation> {
  return write("Resuming a print", (operationId) => command("resume_host_print", { operationId, printerId }));
}

export function cancelHostPrint(printerId: string): Promise<HostOperation> {
  return write("Cancelling a print", (operationId) => command("cancel_host_print", { operationId, printerId }));
}

/** `reconcile_host_operation`: a read-only check of the host, so it has no
 *  `operationId`. Resolves with the row after the attempt; the stream
 *  carries the same change to the store. */
export async function reconcileHostOperation(hostOperationId: string): Promise<HostOperation> {
  if (!desktopAvailable()) throw needsDesktopError("Checking a printer");
  return adoptReturned(await retryOnTransportFailure(() => command("reconcile_host_operation", { hostOperationId })));
}

/** `abandon_host_operation` (D8). The acknowledgement is the one literal
 *  the backend accepts; the note is trimmed and left out when empty. */
export function abandonHostOperation(hostOperationId: string, note?: string): Promise<HostOperation> {
  const trimmed = note?.trim();
  return write("Abandoning a check", (operationId) => command("abandon_host_operation", {
    operationId,
    hostOperationId,
    acknowledgement: "hostStateUnknown",
    ...(trimmed ? { note: trimmed } : {}),
  }));
}
