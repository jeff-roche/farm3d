/** A stand-in for `library-store` in the Library screen tests. Test-only:
 *  nothing outside a test imports this module. Use it as
 *
 *    vi.mock("../library/library-store", async () =>
 *      (await import("../library/library-store-mock")).libraryStoreMock);
 *
 *  The read side is a real Solid store, so components react when a test
 *  changes it with `setLibraryState`; the actions are spies. */
import { createStore } from "solid-js/store";
import { vi } from "vitest";
import type { LibraryContentInfo, ModelRecord, ModelSourceRevisionRecord, ProjectRecord } from "./types";

interface MockLibraryState {
  projects: ProjectRecord[];
  models: ModelRecord[];
  status: "idle" | "loading" | "ready" | "error";
  syncState: "syncing" | "current" | "uncertain";
  error: string | null;
  contentInfo: LibraryContentInfo | null;
}

const initialState = (): MockLibraryState => ({
  projects: [],
  models: [],
  status: "ready",
  syncState: "current",
  error: null,
  contentInfo: null,
});

const [state, setState] = createStore<MockLibraryState>(initialState());

export const libraryStoreMock = {
  library: {
    projects: () => state.projects,
    models: () => state.models,
    status: () => state.status,
    syncState: () => state.syncState,
    error: () => state.error,
    contentInfo: () => state.contentInfo,
  },
  updateModel: vi.fn(async (_id: string, _patch: { name: string }) => {}),
  setModelProjects: vi.fn(async (_id: string, _change: { add: string[]; remove: string[] }) => {}),
  createProject: vi.fn(async (name: string): Promise<ProjectRecord> => ({
    id: "prj-new", revision: 1, name, modelCount: 0, createdAt: "", updatedAt: "",
  })),
  convertToManaged: vi.fn(async (_id: string) => {}),
  loadRevisions: vi.fn(async (_modelId: string): Promise<ModelSourceRevisionRecord[]> => []),
  loadThumbnail: vi.fn(async (_revisionId: string): Promise<string | null> => null),
  reportLibraryError: vi.fn(),
  dismissLibraryError: vi.fn(),
  refreshLibrary: vi.fn(),
};

export function setLibraryState(patch: Partial<MockLibraryState>): void {
  setState(patch);
}

/** Restores the empty ready state and clears every spy's calls, keeping
 *  the default implementations. */
export function resetLibraryStoreMock(): void {
  setState(initialState());
  for (const value of Object.values(libraryStoreMock)) {
    if (typeof value === "function" && "mockClear" in value) value.mockClear();
  }
}
