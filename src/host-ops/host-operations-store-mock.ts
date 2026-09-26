/** A stand-in for `host-operations-store` in screen tests. Test-only:
 *  nothing outside a test imports this module. Use it as
 *
 *    vi.mock("../host-ops/host-operations-store", async () =>
 *      (await import("../host-ops/host-operations-store-mock")).hostOperationsStoreMock);
 *
 *  The read side is a real Solid store, so components react when a test
 *  changes it with `setHostOperationsStoreState` (or
 *  `loadWebHostOperationsFixture`); the actions are spies. */
import { createStore, reconcile } from "solid-js/store";
import { vi } from "vitest";
import { buildWebHostOpsFixture, type WebHostOpsFixture } from "./web-fixtures";
import { isTerminalHostOperationState } from "./types";
import type { HostOperation, PriorState } from "./types";

interface MockHostOperationsState {
  operations: HostOperation[];
  status: "idle" | "loading" | "ready" | "error";
  syncState: "syncing" | "current" | "uncertain";
}

const initialState = (): MockHostOperationsState => ({ operations: [], status: "ready", syncState: "current" });

const [state, setState] = createStore<MockHostOperationsState>(initialState());

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

export const hostOperationsStoreMock = {
  hostOperations: {
    operations: () => state.operations,
    operation: (id: string) => state.operations.find((o) => o.id === id),
    unresolvedFor: (printerId: string) =>
      state.operations.find((o) => o.printerId === printerId && !isTerminalHostOperationState(o.state)),
    stagedFor: (printerId: string) =>
      newestByHostPath(state.operations.filter((o) => o.printerId === printerId && o.kind === "upload" && o.state === "succeeded")),
    recentFor: (printerId: string) =>
      state.operations.filter((o) => o.printerId === printerId && isTerminalHostOperationState(o.state)),
    status: () => state.status,
    syncState: () => state.syncState,
  },
  startHostOperations: vi.fn(async (): Promise<() => void> => () => {}),
  refreshHostOperations: vi.fn(),
  stageSliceRevision: vi.fn(async (printerId: string, sliceRevisionId: string): Promise<HostOperation> =>
    mockRow({ printerId, sliceRevisionId, kind: "upload" })),
  startStagedArtifact: vi.fn(async (printerId: string, _hostOperationId: string, _priorState: PriorState): Promise<HostOperation> =>
    mockRow({ printerId, kind: "start" })),
  pauseHostPrint: vi.fn(async (printerId: string): Promise<HostOperation> => mockRow({ printerId, kind: "pause" })),
  resumeHostPrint: vi.fn(async (printerId: string): Promise<HostOperation> => mockRow({ printerId, kind: "resume" })),
  cancelHostPrint: vi.fn(async (printerId: string): Promise<HostOperation> => mockRow({ printerId, kind: "cancel" })),
  reconcileHostOperation: vi.fn(async (hostOperationId: string): Promise<HostOperation> =>
    state.operations.find((o) => o.id === hostOperationId) ?? mockRow({ id: hostOperationId })),
  abandonHostOperation: vi.fn(async (hostOperationId: string, _note?: string): Promise<HostOperation> =>
    mockRow({ ...state.operations.find((o) => o.id === hostOperationId), id: hostOperationId, state: "abandoned" })),
};

/** The row a mocked write resolves with: a `dispatching` stand-in (the
 *  real command's write-ahead row), not added to the store. */
function mockRow(overrides: Partial<HostOperation>): HostOperation {
  return {
    id: "hop-mock",
    printerId: "prn-mock",
    kind: "upload",
    state: "dispatching",
    sliceRevisionId: null,
    sourceHostOperationId: null,
    gcodeSha256: null,
    gcodeSize: null,
    hostPath: "farm3d/mock.gcode",
    endpoint: { kind: "moonraker", host: "192.0.2.10", port: 7125 },
    failure: null,
    resolution: null,
    attempts: 0,
    lastAttempt: null,
    noLongerPending: false,
    abandonedAt: null,
    abandonNote: null,
    createdAt: "2026-09-25T00:00:00Z",
    dispatchedAt: null,
    uncertainSince: null,
    resolvedAt: null,
    ...overrides,
  };
}

/** Replaces the mock's operations, as an event settling into the real
 *  store would. */
export function setHostOperationsStoreState(operations: HostOperation[]): void {
  setState("operations", reconcile(operations));
}

/** Loads `web-fixtures.ts`' Host Operations into the mock, and returns the
 *  full fixture (capabilities included) for the test to read from too. */
export function loadWebHostOperationsFixture(): WebHostOpsFixture {
  const fixture = buildWebHostOpsFixture();
  setState("operations", reconcile(fixture.hostOperations));
  return fixture;
}

export function resetHostOperationsStoreMock(): void {
  setState(initialState());
  for (const action of Object.values(hostOperationsStoreMock)) {
    if (typeof action === "function" && "mockClear" in action) action.mockClear();
  }
}
