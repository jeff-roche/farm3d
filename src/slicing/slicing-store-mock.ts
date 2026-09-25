/** A stand-in for `slicing-store` in screen tests. Test-only: nothing
 *  outside a test imports this module. Use it as
 *
 *    vi.mock("../slicing/slicing-store", async () =>
 *      (await import("../slicing/slicing-store-mock")).slicingStoreMock);
 *
 *  The read side is a real Solid store, so components react when a test
 *  changes it with `setSlicingState` (or `loadWebSlicingFixture`); the
 *  actions are spies. `refuseDesktopOnlyActions` makes them answer the way
 *  the real store does in web mode. */
import { createStore, reconcile } from "solid-js/store";
import { vi } from "vitest";
import { desktopOnlyActions, desktopOnlyError } from "./desktop-only";
import { decodeMeshBuffer, type MeshBuffer } from "./mesh-buffer";
import type {
  CreateExternalSliceRevisionFacts,
  PreparationDocument,
  PreparationRecord,
  PresetSourceKind,
  ReloadPreparationData,
  RevisionGeometry,
  SliceOperationLog,
  SliceOperationRecord,
  SliceOptions,
  SliceProgress,
  SliceRevisionRecord,
  SliceRevisionSummary,
  SlicerRuntimeStatus,
  SliceTarget,
} from "./types";
import { buildWebSlicingFixture, WEB_SLICING_REVISION_EXTERNAL, type WebSlicingFixture } from "./web-fixtures";

interface MockSlicingState {
  runtime: SlicerRuntimeStatus | null;
  preparations: { [modelId: string]: PreparationRecord | undefined };
  operations: SliceOperationRecord[];
  revisionsByModel: { [modelId: string]: SliceRevisionSummary[] | undefined };
  progress: { [operationId: string]: SliceProgress | undefined };
  status: "idle" | "loading" | "ready" | "error";
  syncState: "syncing" | "current" | "uncertain";
  continueWith: { [preparationId: string]: string | undefined };
}

const initialState = (): MockSlicingState => ({
  runtime: null,
  preparations: {},
  operations: [],
  revisionsByModel: {},
  progress: {},
  status: "ready",
  syncState: "current",
  continueWith: {},
});

const [state, setState] = createStore<MockSlicingState>(initialState());

function findRevision(id: string): SliceRevisionSummary | undefined {
  return Object.values(state.revisionsByModel).flatMap((list) => list ?? []).find((r) => r.id === id);
}

export const slicingStoreMock = {
  slicing: {
    runtime: () => state.runtime,
    preparation: (modelId: string) => state.preparations[modelId],
    preparations: () => Object.values(state.preparations).filter((p): p is PreparationRecord => p !== undefined),
    operations: () => state.operations,
    operation: (id: string) => state.operations.find((o) => o.id === id),
    operationsForPreparation: (preparationId: string) => state.operations.filter((o) => o.preparationId === preparationId),
    revisions: (modelId: string) => state.revisionsByModel[modelId] ?? [],
    revision: (id: string) => findRevision(id),
    progress: (operationId: string) => state.progress[operationId],
    status: () => state.status,
    syncState: () => state.syncState,
    continueWithSourceRevision: (preparationId: string) => {
      const chosen = state.continueWith[preparationId];
      const held = Object.values(state.preparations).find((p) => p?.id === preparationId);
      return chosen && held?.stale && held.sourceRevisionId === chosen ? chosen : undefined;
    },
  },
  startSlicing: vi.fn(async (): Promise<() => void> => () => {}),
  refreshSlicing: vi.fn(),
  checkSlicerRuntime: vi.fn(async (): Promise<SlicerRuntimeStatus> => state.runtime!),
  pickSlicerEngine: vi.fn(async (): Promise<SlicerRuntimeStatus | null> => null),
  pickPresetSource: vi.fn(async (_kind: PresetSourceKind): Promise<SlicerRuntimeStatus | null> => null),
  resetSlicerRuntime: vi.fn(async (_reset: { engine: boolean; presetSource: boolean }): Promise<SlicerRuntimeStatus> => state.runtime!),
  listSliceOptions: vi.fn(async (_target: SliceTarget): Promise<SliceOptions> => buildWebSlicingFixture().sliceOptions),
  loadGeometry: vi.fn(async (_revisionId: string): Promise<RevisionGeometry> => ({ objects: [], buildItems: [] })),
  loadMesh: vi.fn(async (_revisionId: string, _objectKey: number): Promise<MeshBuffer> => ({
    vertexCount: 0, indexCount: 0, triangleCount: 0, positions: new Float32Array(), indices: new Uint32Array(),
  })),
  createPreparation: vi.fn(async (modelId: string, _target?: SliceTarget): Promise<PreparationRecord> => state.preparations[modelId]!),
  /** Settles the saved record into the mock's state, as the store does. */
  updatePreparation: vi.fn(async (preparationId: string, document: PreparationDocument): Promise<PreparationRecord> => {
    const held = Object.values(state.preparations).find((p) => p?.id === preparationId)!;
    const saved = { ...held, document: JSON.parse(JSON.stringify(document)) as PreparationDocument, revision: held.revision + 1 };
    setState("preparations", held.modelId, reconcile(saved));
    return saved;
  }),
  reloadPreparation: vi.fn(async (preparationId: string): Promise<ReloadPreparationData> => ({
    preparation: Object.values(state.preparations).find((p) => p?.id === preparationId)!,
    removedObjectKeys: [],
    addedObjectKeys: [],
  })),
  deletePreparation: vi.fn(async (_preparationId: string) => {}),
  chooseContinueWithSourceRevision: vi.fn((preparationId: string, sourceRevisionId: string | null) => {
    setState("continueWith", preparationId, sourceRevisionId ?? undefined);
  }),
  startSlice: vi.fn(async (
    _preparationId: string,
    _plateKeys: string[],
    _options?: { continueWithSourceRevision?: string; operationId?: string },
  ): Promise<SliceOperationRecord[]> => []),
  cancelSliceOperation: vi.fn(async (sliceOperationId: string): Promise<SliceOperationRecord> => (
    state.operations.find((o) => o.id === sliceOperationId)!
  )),
  loadOperationLog: vi.fn(async (_sliceOperationId: string): Promise<SliceOperationLog> => ({
    text: "", truncated: false, noiseLines: [],
  })),
  loadSliceRevisions: vi.fn(async (modelId: string): Promise<SliceRevisionSummary[]> => [...(state.revisionsByModel[modelId] ?? [])]),
  loadSliceRevision: vi.fn(async (sliceRevisionId: string): Promise<SliceRevisionRecord> => ({
    ...findRevision(sliceRevisionId)!, blobs: [],
  })),
  createExternalSliceRevision: vi.fn(async (
    sourceRevisionId: string,
    _facts: CreateExternalSliceRevisionFacts,
    _operationId?: string,
  ): Promise<SliceRevisionRecord> => {
    const record = buildWebSlicingFixture().revisionRecords[WEB_SLICING_REVISION_EXTERNAL];
    return { ...record, sourceRevisionId };
  }),
  deleteSliceRevision: vi.fn(async (_sliceRevisionId: string) => {}),
};

export function setSlicingState(patch: Partial<MockSlicingState>): void {
  setState(patch);
}

/** Sets one operation's progress, as a `slicing.operation.progress` event
 *  would (`undefined` clears it). */
export function setSlicingProgress(operationId: string, progress: SliceProgress | undefined): void {
  setState("progress", operationId, progress === undefined ? undefined : reconcile(progress));
}

/** Fills the read side from the web fixtures, and makes the read actions
 *  answer from them, the way the real store does in web mode. Returns the
 *  fixture for the test to refer to. */
export function loadWebSlicingFixture(now?: Date): WebSlicingFixture {
  const fixture = buildWebSlicingFixture(now);
  const preparations: MockSlicingState["preparations"] = {};
  for (const preparation of fixture.preparations) preparations[preparation.modelId] = preparation;
  const revisionsByModel: MockSlicingState["revisionsByModel"] = {};
  for (const revision of fixture.revisions) (revisionsByModel[revision.modelId] ??= []).push(revision);
  setState({ runtime: fixture.runtime, preparations, operations: fixture.operations, revisionsByModel, progress: {} });
  slicingStoreMock.loadGeometry.mockImplementation(async (revisionId) => fixture.geometry[revisionId]);
  slicingStoreMock.loadMesh.mockImplementation(async (revisionId, objectKey) => (
    decodeMeshBuffer(fixture.meshes[revisionId][objectKey].slice(0))
  ));
  slicingStoreMock.loadOperationLog.mockImplementation(async (id) => fixture.logs[id]);
  slicingStoreMock.loadSliceRevision.mockImplementation(async (id) => fixture.revisionRecords[id]);
  return fixture;
}

/** Makes the desktop-only actions reject as the real store does in web
 *  mode: slicing, cancelling, creating an external revision, and the
 *  runtime pickers and reset all reject with the `needsDesktop` error. */
export function refuseDesktopOnlyActions(): void {
  for (const action of desktopOnlyActions) slicingStoreMock[action].mockRejectedValue(desktopOnlyError(action));
}

/** Restores the empty ready state and resets every spy: calls cleared,
 *  and any implementation a test set replaced by the default above. */
export function resetSlicingStoreMock(): void {
  setState(reconcile(initialState()));
  for (const value of Object.values(slicingStoreMock)) {
    if (typeof value === "function" && "mockReset" in value) value.mockReset();
  }
}
