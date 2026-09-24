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
import type {
  ImportInspection,
  ImportItemRequest,
  ImportModelsResult,
  ImportProgress,
  ImportSelectionSummary,
  LibraryContentInfo,
  ModelRecord,
  ModelSourceRevisionRecord,
  ModelSourceRevisionSummary,
  ProjectRecord,
  SelectionPurpose,
} from "./types";

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

const progressHandlers = new Set<(selectionId: string, progress: ImportProgress) => void>();
const droppedHandlers = new Set<(summary: ImportSelectionSummary) => void>();
const revisionHandlers = new Set<(revision: ModelSourceRevisionSummary) => void>();

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
  renameProject: vi.fn(async (_id: string, _name: string) => {}),
  deleteProject: vi.fn(async (_id: string) => {}),
  deleteModel: vi.fn(async (_id: string) => {}),
  convertToManaged: vi.fn(async (_id: string) => {}),
  checkSources: vi.fn(async (_modelIds?: string[], _options?: { force?: boolean }) => {}),
  locateSource: vi.fn(async (
    modelId: string,
    _selectionId: string,
    _fileIndex: number,
    _acceptDifferentContent: boolean,
  ): Promise<ModelRecord> => state.models.find((m) => m.id === modelId)!),
  loadRevisions: vi.fn(async (_modelId: string): Promise<ModelSourceRevisionRecord[]> => []),
  loadThumbnail: vi.fn(async (_revisionId: string): Promise<string | null> => null),
  reportLibraryError: vi.fn(),
  dismissLibraryError: vi.fn(),
  refreshLibrary: vi.fn(),
  pickFiles: vi.fn(async (_purpose: SelectionPurpose): Promise<ImportSelectionSummary | null> => null),
  inspectSelection: vi.fn(async (selectionId: string): Promise<ImportInspection> => ({ selectionId, items: [] })),
  cancelSelection: vi.fn(async (_selectionId: string) => {}),
  importModels: vi.fn(async (
    _selectionId: string,
    _items: ImportItemRequest[],
    _operationId?: string,
  ): Promise<ImportModelsResult> => ({ items: [] })),
  onImportProgress: vi.fn((handler: (selectionId: string, progress: ImportProgress) => void) => {
    progressHandlers.add(handler);
    return () => progressHandlers.delete(handler);
  }),
  onSelectionDropped: vi.fn((handler: (summary: ImportSelectionSummary) => void) => {
    droppedHandlers.add(handler);
    return () => droppedHandlers.delete(handler);
  }),
  onRevisionCreated: vi.fn((handler: (revision: ModelSourceRevisionSummary) => void) => {
    revisionHandlers.add(handler);
    return () => revisionHandlers.delete(handler);
  }),
};

/** Delivers a `library.revision.created` event to the subscribed handlers. */
export function emitRevisionCreated(revision: ModelSourceRevisionSummary): void {
  for (const handler of revisionHandlers) handler(revision);
}

/** Delivers a `library.import.progress` event to the subscribed handlers. */
export function emitImportProgress(selectionId: string, progress: ImportProgress): void {
  for (const handler of progressHandlers) handler(selectionId, progress);
}

/** Delivers a `library.selection.dropped` event to the subscribed handlers. */
export function emitSelectionDropped(summary: ImportSelectionSummary): void {
  for (const handler of droppedHandlers) handler(summary);
}

export function setLibraryState(patch: Partial<MockLibraryState>): void {
  setState(patch);
}

/** Restores the empty ready state and resets every spy: calls cleared,
 *  and any implementation a test set replaced by the default above. */
export function resetLibraryStoreMock(): void {
  setState(initialState());
  progressHandlers.clear();
  droppedHandlers.clear();
  revisionHandlers.clear();
  for (const value of Object.values(libraryStoreMock)) {
    if (typeof value === "function" && "mockReset" in value) value.mockReset();
  }
}
