import { describe, expect, it } from "vitest";
import {
  applyProjectsToAll,
  buildRequest,
  mergeResults,
  rowBlockers,
  rowsFromInspection,
  type ImportRow,
} from "./import-flow";
import { model } from "./test-records";
import type { ImportCandidate, ImportInspection, ImportItemResult } from "./types";

function ready(fileIndex: number, fileName: string, overrides: Partial<Extract<ImportCandidate, { status: "ready" }>> = {}): ImportCandidate {
  return {
    status: "ready",
    fileIndex,
    fileName,
    format: "stl",
    sizeBytes: 684,
    sha256: String(fileIndex).repeat(64).slice(0, 64),
    summary: { format: "stl", triangleCount: 12, boundsMm: { min: [0, 0, 0], max: [10, 10, 10] }, unitsAssumed: true },
    unsupported: [],
    warnings: [],
    duplicates: [],
    ...overrides,
  };
}

function rejected(fileIndex: number, fileName: string): ImportCandidate {
  return { status: "rejected", fileIndex, fileName, code: "UNSUPPORTED_FORMAT", message: "notes.txt is not a supported format." };
}

function inspection(...items: ImportCandidate[]): ImportInspection {
  return { selectionId: "sel-1", items };
}

const DUPLICATE = {
  modelId: "mdl-existing",
  modelName: "Cube",
  projectIds: [],
  revisionId: "msr-existing",
  sequence: 1,
  isCurrent: true,
};

describe("rowsFromInspection", () => {
  it("defaults every row to Managed with the viewed Project, named after the file", () => {
    const rows = rowsFromInspection(inspection(ready(0, "cube.stl"), ready(1, "lid.3mf", { format: "3mf" })), {
      projectIds: ["prj-b"],
      models: [],
    });
    expect(rows.map((r) => [r.fileIndex, r.name, r.storageMode, r.projectIds, r.acknowledgeUnsupported])).toEqual([
      [0, "cube", "managed", ["prj-b"], false],
      [1, "lid", "managed", ["prj-b"], false],
    ]);
    expect(rows[0].duplicateAction).toBeUndefined();
  });

  it("defaults projectIds to [] (Unfiled) in a saved view", () => {
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models: [] });
    expect(row.projectIds).toEqual([]);
  });

  it("does not share the context's projectIds array between rows", () => {
    const rows = rowsFromInspection(inspection(ready(0, "a.stl"), ready(1, "b.stl")), { projectIds: ["prj-b"], models: [] });
    expect(rows[0].projectIds).not.toBe(rows[1].projectIds);
  });

  it("pre-fills a same-name Model in one of the row's Projects as the target without choosing addRevision", () => {
    const models = [
      model({ id: "mdl-other-project", name: "cube", projectIds: ["prj-c"] }),
      model({ id: "mdl-in-b", name: "CUBE", projectIds: ["prj-a", "prj-b"] }),
    ];
    const [row] = rowsFromInspection(inspection(ready(0, "Cube.stl")), { projectIds: ["prj-b"], models });
    expect(row.targetModelId).toBe("mdl-in-b");
    expect(row.duplicateAction).toBeUndefined();
  });

  it("pre-fills an Unfiled same-name Model for a row with no Projects", () => {
    const models = [
      model({ id: "mdl-filed", name: "Cube", projectIds: ["prj-b"] }),
      model({ id: "mdl-unfiled", name: "Cube" }),
    ];
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models });
    expect(row.targetModelId).toBe("mdl-unfiled");
  });

  it("pre-fills the most recently updated when several same-name Models match", () => {
    const models = [
      model({ id: "mdl-older", name: "Cube", updatedAt: "2026-09-01T00:00:00Z" }),
      model({ id: "mdl-newer", name: "Cube", updatedAt: "2026-09-20T00:00:00Z" }),
      model({ id: "mdl-middle", name: "Cube", updatedAt: "2026-09-10T00:00:00Z" }),
    ];
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models });
    expect(row.targetModelId).toBe("mdl-newer");
  });

  it("does not pre-fill a Model that could not take a new revision (linked, or another format)", () => {
    const models = [
      model({ id: "mdl-linked", name: "Cube", storageMode: "linked", link: { path: "/m/cube.stl", state: "ok", checkedAt: null, watchMode: "watching" } }),
      model({ id: "mdl-3mf", name: "Cube", format: "3mf" }),
    ];
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models });
    expect(row.targetModelId).toBeUndefined();
  });
});

describe("applyProjectsToAll", () => {
  it("copies one set of Projects to every non-rejected row", () => {
    const rows = rowsFromInspection(inspection(ready(0, "a.stl"), rejected(1, "notes.txt"), ready(2, "b.stl")), {
      projectIds: [],
      models: [],
    });
    const next = applyProjectsToAll(rows, ["prj-b", "prj-c"]);
    expect(next.map((r) => r.projectIds)).toEqual([["prj-b", "prj-c"], [], ["prj-b", "prj-c"]]);
    expect(next[0].projectIds).not.toBe(next[2].projectIds);
  });
});

describe("rowBlockers", () => {
  const base = (candidate: ImportCandidate): ImportRow =>
    rowsFromInspection(inspection(candidate), { projectIds: [], models: [] })[0];

  it("blocks a duplicate row until an action is chosen", () => {
    const row = base(ready(0, "cube.stl", { duplicates: [DUPLICATE] }));
    expect(rowBlockers(row)).toEqual(["duplicateDecision"]);
    expect(rowBlockers({ ...row, duplicateAction: "addAnother" })).toEqual([]);
    expect(rowBlockers({ ...row, duplicateAction: "useExisting" })).toEqual([]);
  });

  it("blocks addRevision until a target Model is chosen", () => {
    const row = base(ready(0, "cube.stl"));
    expect(rowBlockers({ ...row, duplicateAction: "addRevision" })).toEqual(["duplicateDecision"]);
    expect(rowBlockers({ ...row, duplicateAction: "addRevision", targetModelId: "mdl-1" })).toEqual([]);
  });

  it("blocks a rich 3MF until its unsupported contents are acknowledged", () => {
    const row = base(ready(0, "plate.3mf", {
      format: "3mf",
      unsupported: [{ part: "Metadata/Slic3r_PE.config", code: "SLICER_SETTINGS", detail: "PrusaSlicer settings" }],
    }));
    expect(rowBlockers(row)).toEqual(["acknowledgeUnsupported"]);
    expect(rowBlockers({ ...row, acknowledgeUnsupported: true })).toEqual([]);
  });

  it("blocks a blank name", () => {
    expect(rowBlockers({ ...base(ready(0, "cube.stl")), name: "   " })).toEqual(["name"]);
  });

  it("reports a rejected row as rejected only", () => {
    expect(rowBlockers(base(rejected(0, "notes.txt")))).toEqual(["rejected"]);
  });
});

describe("buildRequest", () => {
  it("sends projectIds de-duplicated and only the unblocked rows", () => {
    const rows = rowsFromInspection(
      inspection(ready(0, "a.stl"), rejected(1, "notes.txt"), ready(2, "dup.stl", { duplicates: [DUPLICATE] }), ready(3, " b.stl")),
      { projectIds: [], models: [] },
    );
    rows[0] = { ...rows[0], projectIds: ["prj-b", "prj-c", "prj-b"] };
    rows[3] = { ...rows[3], name: "  Spaced  " };
    expect(buildRequest(rows, [])).toEqual([
      { fileIndex: 0, name: "a", projectIds: ["prj-b", "prj-c"], storageMode: "managed", acknowledgeUnsupported: false },
      { fileIndex: 3, name: "Spaced", projectIds: [], storageMode: "managed", acknowledgeUnsupported: false },
    ]);
  });

  it("sends addRevision with the target's current revision as targetExpectedRevision", () => {
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models: [] });
    const target = model({ id: "mdl-target", revision: 7 });
    expect(buildRequest([{ ...row, duplicateAction: "addRevision", targetModelId: "mdl-target" }], [target])).toEqual([
      {
        fileIndex: 0, name: "cube", projectIds: [], storageMode: "managed",
        duplicateAction: "addRevision", targetModelId: "mdl-target", targetExpectedRevision: 7,
        acknowledgeUnsupported: false,
      },
    ]);
  });

  it("sends useExisting against the duplicate's Model, not a same-name pre-fill", () => {
    const models = [model({ id: "mdl-same-name", name: "cube" })];
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl", { duplicates: [DUPLICATE] })), { projectIds: [], models });
    expect(row.targetModelId).toBe("mdl-same-name");
    expect(buildRequest([{ ...row, duplicateAction: "useExisting" }], models)).toEqual([
      {
        fileIndex: 0, name: "cube", projectIds: [], storageMode: "managed",
        duplicateAction: "useExisting", targetModelId: "mdl-existing", acknowledgeUnsupported: false,
      },
    ]);
  });

  it("omits a pre-filled target when no action uses it", () => {
    const models = [model({ id: "mdl-same-name", name: "cube" })];
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models });
    const [item] = buildRequest([row], models);
    expect(item).not.toHaveProperty("targetModelId");
    expect(item).not.toHaveProperty("duplicateAction");
  });
});

describe("mergeResults", () => {
  it("keeps failed rows' choices, and a second buildRequest includes only rows without a successful outcome", () => {
    let rows = rowsFromInspection(inspection(ready(0, "a.stl"), ready(1, "b.stl"), ready(2, "c.stl")), { projectIds: [], models: [] });
    rows = rows.map((r) => (r.fileIndex === 1 ? { ...r, name: "Chosen name", storageMode: "linked", projectIds: ["prj-b"] } : r));
    const results: ImportItemResult[] = [
      { fileIndex: 0, outcome: "imported", model: model({ id: "mdl-a" }), errors: [], warnings: [] },
      { fileIndex: 1, outcome: "rejected", errors: [{ code: "SOURCE_UNREADABLE", message: "b.stl could not be read." }], warnings: [] },
      { fileIndex: 2, outcome: "cancelled", errors: [], warnings: [] },
    ];
    const merged = mergeResults(rows, { items: results });

    expect(merged.map((r) => r.result?.outcome)).toEqual(["imported", "rejected", "cancelled"]);
    expect(merged[1]).toMatchObject({ name: "Chosen name", storageMode: "linked", projectIds: ["prj-b"] });
    expect(buildRequest(merged, []).map((i) => i.fileIndex)).toEqual([1, 2]);
  });

  it("leaves rows the result does not mention unchanged", () => {
    const rows = rowsFromInspection(inspection(ready(0, "a.stl"), ready(1, "b.stl")), { projectIds: [], models: [] });
    const merged = mergeResults(rows, { items: [{ fileIndex: 1, outcome: "reusedExisting", model: model(), errors: [], warnings: [] }] });
    expect(merged[0]).toEqual(rows[0]);
    expect(merged[1].result?.outcome).toBe("reusedExisting");
    expect(buildRequest(merged, []).map((i) => i.fileIndex)).toEqual([0]);
  });
});
