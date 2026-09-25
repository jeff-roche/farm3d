/** Record builders shared by the slicing tests. Test-only: nothing outside
 *  a test imports this module. */
import type {
  PreparationRecord,
  SliceOperationRecord,
  SliceRevisionSummary,
  SlicerRuntimeStatus,
  SlicingSnapshot,
} from "./types";

export function runtimeStatus(overrides: Partial<SlicerRuntimeStatus> = {}): SlicerRuntimeStatus {
  return {
    engine: {
      state: "available", version: "2.4.2", channel: "release", source: "path",
      executableName: "orca-slicer", path: "/usr/bin/orca-slicer", extractAndRun: false,
    },
    presetSource: {
      state: "available", version: "2.4.2", channel: "release", origin: "engine", vendorCount: 62, path: "/usr/bin/orca-slicer",
    },
    canSlice: true,
    versionsDiffer: false,
    revision: 1,
    engineCandidates: [],
    ...overrides,
  };
}

export function preparation(overrides: Partial<PreparationRecord> = {}): PreparationRecord {
  return {
    id: "prp-1",
    modelId: "mdl-1",
    sourceRevisionId: "msr-1",
    revision: 1,
    stale: false,
    document: {
      plates: [{
        plateKey: "plt-1",
        instances: [{
          instanceKey: "ins-1", objectKey: 1,
          transform: { translateMm: [128, 128], rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
        }],
      }],
      target: { kind: "printer", printerId: "prn-1" },
      controls: {},
    },
    createdAt: "2026-09-24T00:00:00Z",
    updatedAt: "2026-09-24T00:00:00Z",
    ...overrides,
  };
}

export function operation(overrides: Partial<SliceOperationRecord> = {}): SliceOperationRecord {
  return {
    id: "sop-1",
    preparationId: "prp-1",
    sourceRevisionId: "msr-1",
    plateKey: "plt-1",
    plateIndex: 1,
    state: "queued",
    queuedAt: "2026-09-24T00:00:00Z",
    ...overrides,
  };
}

export function sliceRevision(overrides: Partial<SliceRevisionSummary> = {}): SliceRevisionSummary {
  return {
    id: "slr-1",
    kind: "farm3d",
    modelId: "mdl-1",
    sourceRevisionId: "msr-1",
    sourceRevisionSequence: 1,
    plate: { plateKey: "plt-1", plateIndex: 1 },
    targetLabel: "Elegoo Centauri Carbon 0.4 nozzle",
    estimates: { printSeconds: 60, filamentGrams: 1, filamentMm: 300, layerCount: 10, maxZMm: 2, source: "farm3dSlice" },
    facts: {
      printerProfile: { provenance: "absent", value: null },
      nozzleDiameterMm: { provenance: "farm3dInput", value: 0.4 },
      materialFamily: { provenance: "farm3dInput", value: "PLA" },
      filamentDiameterMm: { provenance: "farm3dInput", value: 1.75 },
    },
    requiresManualPrinterSelection: true,
    createdAt: "2026-09-24T00:00:00Z",
    ...overrides,
  };
}

export function slicingSnapshot(sequence: number, overrides: Partial<Omit<SlicingSnapshot, "snapshotSequence">> = {}): SlicingSnapshot {
  return {
    streamId: "stream-slicing",
    snapshotSequence: sequence,
    runtime: runtimeStatus(),
    preparations: [],
    activeAndRecentOperations: [],
    revisions: [],
    ...overrides,
  };
}
