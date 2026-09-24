import { describe, expect, it } from "vitest";
import {
  applyProjectsToAll,
  buildRequest,
  mergeResults,
  rowBlockers,
  rowsFromInspection,
  setRowName,
  decisionRequiredAtCommit,
  setRowAcknowledged,
  setRowAction,
  setRowProjects,
  setRowStorage,
  setRowTarget,
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
    const next = applyProjectsToAll(rows, ["prj-b", "prj-c"], []);
    expect(next.map((r) => r.projectIds)).toEqual([["prj-b", "prj-c"], [], ["prj-b", "prj-c"]]);
    expect(next[0].projectIds).not.toBe(next[2].projectIds);
  });
});

describe("keeping the same-name suggestion current (D14)", () => {
  const models = [
    model({ id: "mdl-cube-in-b", name: "Cube", projectIds: ["prj-b"] }),
    model({ id: "mdl-cube-in-c", name: "Cube", projectIds: ["prj-c"] }),
    model({ id: "mdl-lid-in-b", name: "Lid", projectIds: ["prj-b"] }),
  ];
  const cubeRow = () => rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: ["prj-b"], models })[0];

  it("moves the suggestion when the row's Projects change", () => {
    expect(cubeRow().targetModelId).toBe("mdl-cube-in-b");
    expect(setRowProjects(cubeRow(), ["prj-c"], models).targetModelId).toBe("mdl-cube-in-c");
  });

  it("clears the suggestion when nothing matches the new Projects", () => {
    const row = setRowProjects(cubeRow(), ["prj-z"], models);
    expect(row.targetModelId).toBeUndefined();
    expect(row).not.toHaveProperty("targetModelId");
    expect(setRowProjects(cubeRow(), [], models).targetModelId).toBeUndefined();
  });

  it("recomputes the suggestion when the row is renamed", () => {
    expect(setRowName(cubeRow(), "lid", models)).toMatchObject({ name: "lid", targetModelId: "mdl-lid-in-b" });
    expect(setRowName(cubeRow(), "Bracket", models).targetModelId).toBeUndefined();
  });

  it("applyProjectsToAll recomputes every row's suggestion", () => {
    const rows = rowsFromInspection(inspection(ready(0, "cube.stl"), ready(1, "lid.stl")), { projectIds: ["prj-b"], models });
    expect(rows.map((r) => r.targetModelId)).toEqual(["mdl-cube-in-b", "mdl-lid-in-b"]);
    expect(applyProjectsToAll(rows, ["prj-c"], models).map((r) => r.targetModelId)).toEqual(["mdl-cube-in-c", undefined]);
  });

  it("never chooses the action while recomputing", () => {
    expect(setRowProjects(cubeRow(), ["prj-c"], models).duplicateAction).toBeUndefined();
  });

  it("keeps a target the user picked themselves", () => {
    const picked = setRowTarget(cubeRow(), "mdl-lid-in-b");
    expect(setRowProjects(picked, ["prj-c"], models).targetModelId).toBe("mdl-lid-in-b");
    expect(setRowName(picked, "Other", models).targetModelId).toBe("mdl-lid-in-b");
    expect(applyProjectsToAll([picked], [], models)[0].targetModelId).toBe("mdl-lid-in-b");
  });

  it("keeps the target once the user has chosen Add as a new revision", () => {
    const chosen: ImportRow = { ...cubeRow(), duplicateAction: "addRevision" };
    expect(setRowProjects(chosen, ["prj-z"], models).targetModelId).toBe("mdl-cube-in-b");
    expect(setRowName(chosen, "Other", models).targetModelId).toBe("mdl-cube-in-b");
  });

  it("leaves a rejected row alone", () => {
    const [row] = rowsFromInspection(inspection(rejected(0, "notes.txt")), { projectIds: [], models });
    expect(setRowProjects(row, ["prj-b"], models)).toBe(row);
  });
});

describe("setRowAction", () => {
  const models = [model({ id: "mdl-same-name", name: "cube" })];

  it("sets the action without touching a pre-filled target", () => {
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl", { duplicates: [DUPLICATE] })), { projectIds: [], models });
    const chosen = setRowAction(row, "addRevision", models);
    expect(chosen).toMatchObject({ duplicateAction: "addRevision", targetModelId: "mdl-same-name" });
  });

  it("clears the action with undefined and re-derives the suggestion", () => {
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models });
    const renamed = setRowName(setRowAction(row, "addRevision", models), "lid", models);
    expect(renamed.targetModelId).toBe("mdl-same-name");
    const cleared = setRowAction(renamed, undefined, models);
    expect(cleared).not.toHaveProperty("duplicateAction");
    expect(cleared).not.toHaveProperty("targetModelId");
  });

  it("leaves a rejected row alone", () => {
    const [row] = rowsFromInspection(inspection(rejected(0, "notes.txt")), { projectIds: [], models });
    expect(setRowAction(row, "addAnother", models)).toBe(row);
  });
});

describe("setRowStorage and setRowAcknowledged", () => {
  it("set the storage mode and acknowledgment, leaving a rejected row alone", () => {
    const [row, rejectedRow] = rowsFromInspection(inspection(ready(0, "cube.stl"), rejected(1, "notes.txt")), { projectIds: [], models: [] });
    expect(setRowStorage(row, "linked").storageMode).toBe("linked");
    expect(setRowAcknowledged(row, true).acknowledgeUnsupported).toBe(true);
    expect(setRowStorage(rejectedRow, "linked")).toBe(rejectedRow);
    expect(setRowAcknowledged(rejectedRow, true)).toBe(rejectedRow);
  });
});

describe("decisionRequiredAtCommit", () => {
  it("is true only for a row the commit sent back with DUPLICATE_DECISION_REQUIRED", () => {
    const [row] = rowsFromInspection(inspection(ready(0, "cube.stl")), { projectIds: [], models: [] });
    expect(decisionRequiredAtCommit(row)).toBe(false);
    expect(decisionRequiredAtCommit({
      ...row,
      result: { fileIndex: 0, outcome: "rejected", errors: [{ code: "DUPLICATE_DECISION_REQUIRED", message: "m" }], warnings: [] },
    })).toBe(true);
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

  it("blocks a row the backend sent back with DUPLICATE_DECISION_REQUIRED until an action is chosen", () => {
    // Inspection saw no duplicate (e.g. two identical files in one
    // selection), so only the commit-time result says a decision is needed.
    const row: ImportRow = {
      ...base(ready(0, "cube.stl")),
      result: {
        fileIndex: 0,
        outcome: "rejected",
        errors: [{ code: "DUPLICATE_DECISION_REQUIRED", message: "The Library already has this file. Choose what to do with it." }],
        warnings: [],
      },
    };
    expect(rowBlockers(row)).toEqual(["duplicateDecision"]);
    expect(rowBlockers({ ...row, duplicateAction: "addAnother" })).toEqual([]);
  });

  it("blocks Use existing among several duplicates until the user picks one of them (D14: never silent)", () => {
    const second = { ...DUPLICATE, modelId: "mdl-second", modelName: "Cube copy", isCurrent: false };
    const row = setRowAction(base(ready(0, "cube.stl", { duplicates: [DUPLICATE, second] })), "useExisting", []);
    expect(rowBlockers(row)).toEqual(["duplicateDecision"]);
    expect(buildRequest([row], [])).toEqual([]);
    // A target picked for something else (not one of the duplicates) doesn't count.
    expect(rowBlockers(setRowTarget(row, "mdl-unrelated"))).toEqual(["duplicateDecision"]);

    const picked = setRowTarget(row, "mdl-second");
    expect(rowBlockers(picked)).toEqual([]);
    expect(buildRequest([picked], [])[0]).toMatchObject({ duplicateAction: "useExisting", targetModelId: "mdl-second" });
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
    const merged = mergeResults(rows, { items: results }, []);

    expect(merged.map((r) => r.result?.outcome)).toEqual(["imported", "rejected", "cancelled"]);
    expect(merged[1]).toMatchObject({ name: "Chosen name", storageMode: "linked", projectIds: ["prj-b"] });
    expect(buildRequest(merged, []).map((i) => i.fileIndex)).toEqual([1, 2]);
  });

  it("lists a commit-time duplicate's Model from the Library, so Use existing has a target", () => {
    const [row] = rowsFromInspection(inspection(ready(0, "twin.stl")), { projectIds: [], models: [] });
    const sha256 = row.candidate.status === "ready" ? row.candidate.sha256 : "";
    const holder = model({ id: "mdl-twin", name: "Twin", projectIds: ["prj-b"] });
    holder.currentRevision = { ...holder.currentRevision, id: "msr-twin", sha256, sequence: 2 };
    const [merged] = mergeResults([row], {
      items: [{
        fileIndex: 0,
        outcome: "rejected",
        errors: [{ code: "DUPLICATE_DECISION_REQUIRED", message: "The Library already has this file. Choose what to do with it." }],
        warnings: [],
      }],
    }, [model({ id: "mdl-other" }), holder]);

    expect(merged.candidate.status === "ready" && merged.candidate.duplicates).toEqual([
      { modelId: "mdl-twin", modelName: "Twin", projectIds: ["prj-b"], revisionId: "msr-twin", sequence: 2, isCurrent: true },
    ]);
    expect(buildRequest([setRowAction(merged, "useExisting", [])], [])[0]).toMatchObject({
      duplicateAction: "useExisting", targetModelId: "mdl-twin",
    });
  });

  it("leaves rows the result does not mention unchanged", () => {
    const rows = rowsFromInspection(inspection(ready(0, "a.stl"), ready(1, "b.stl")), { projectIds: [], models: [] });
    const merged = mergeResults(rows, { items: [{ fileIndex: 1, outcome: "reusedExisting", model: model(), errors: [], warnings: [] }] }, []);
    expect(merged[0]).toEqual(rows[0]);
    expect(merged[1].result?.outcome).toBe("reusedExisting");
    expect(buildRequest(merged, []).map((i) => i.fileIndex)).toEqual([0]);
  });
});
