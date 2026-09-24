/** Record builders shared by the `src/library/*.test.ts` files. Test-only:
 *  nothing outside a test imports this module. */
import type { ModelRecord, ModelSourceRevisionSummary, ProjectRecord } from "./types";

export function project(overrides: Partial<ProjectRecord> = {}): ProjectRecord {
  return {
    id: "prj-1",
    revision: 1,
    name: "Brackets",
    modelCount: 0,
    createdAt: "2026-09-01T00:00:00Z",
    updatedAt: "2026-09-01T00:00:00Z",
    ...overrides,
  };
}

export function revisionSummary(overrides: Partial<ModelSourceRevisionSummary> = {}): ModelSourceRevisionSummary {
  return {
    id: "msr-1",
    modelId: "mdl-1",
    sequence: 1,
    sha256: "a".repeat(64),
    sizeBytes: 684,
    format: "stl",
    origin: "import",
    sourceFileName: "cube.stl",
    capturedAt: "2026-09-01T00:00:00Z",
    hasThumbnail: false,
    summary: {
      format: "stl",
      triangleCount: 12,
      boundsMm: { min: [0, 0, 0], max: [10, 10, 10] },
      unitsAssumed: true,
    },
    ...overrides,
  };
}

export function model(overrides: Partial<ModelRecord> = {}): ModelRecord {
  const id = overrides.id ?? "mdl-1";
  return {
    id,
    revision: 1,
    name: "Cube",
    projectIds: [],
    format: "stl",
    storageMode: "managed",
    link: null,
    currentRevision: revisionSummary({ modelId: id }),
    revisionCount: 1,
    createdAt: "2026-09-01T00:00:00Z",
    updatedAt: "2026-09-01T00:00:00Z",
    ...overrides,
  };
}
