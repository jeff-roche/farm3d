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
import type { HostOperation } from "./types";

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
};

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
}
