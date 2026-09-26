import { createStore, reconcile } from "solid-js/store";
import { command, desktopAvailable } from "../ipc/client";
import { createSequencedStream } from "../ipc/sequenced-stream";
import { isHostOperationsEvent, isTerminalHostOperationState } from "./types";
import type { HostOperation, HostOperationsEvent, HostOperationsSnapshot } from "./types";

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

export const hostOperations = {
  operations: (): HostOperation[] => state.operations,
  operation: (id: string): HostOperation | undefined => state.operations.find((o) => o.id === id),
  /** At most one unresolved row per Printer (owner decision 2). */
  unresolvedFor: (printerId: string): HostOperation | undefined =>
    state.operations.find((o) => o.printerId === printerId && !isTerminalHostOperationState(o.state)),
  stagedFor: (printerId: string): HostOperation[] =>
    newestByHostPath(state.operations.filter((o) => o.printerId === printerId && o.kind === "upload" && o.state === "succeeded")),
  recentFor: (printerId: string): HostOperation[] =>
    state.operations.filter((o) => o.printerId === printerId && isTerminalHostOperationState(o.state)),
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
