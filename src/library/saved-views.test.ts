import { describe, expect, it } from "vitest";
import { modelsFor, searchModels, sortModels, viewCounts, type SavedViewId } from "./saved-views";
import { model, project, revisionSummary } from "./test-records";
import type { ModelRecord, SourceState } from "./types";

const NOW = new Date("2026-09-24T12:00:00Z");
const DAY_MS = 24 * 60 * 60 * 1000;

function capturedDaysAgo(days: number, extraMs = 0): string {
  return new Date(NOW.getTime() - days * DAY_MS - extraMs).toISOString();
}

function linked(id: string, state: SourceState, path = `/home/me/models/${id}.stl`): ModelRecord {
  return model({
    id,
    name: id,
    storageMode: "linked",
    link: { path, state, checkedAt: null, watchMode: "watching" },
  });
}

const BRACKETS = project({ id: "prj-b", name: "Brackets" });
const CALIBRATION = project({ id: "prj-c", name: "Calibration" });

describe("modelsFor", () => {
  it("all returns every Model", () => {
    const models = [model({ id: "m1" }), model({ id: "m2", projectIds: ["prj-b"] })];
    expect(modelsFor({ kind: "view", id: "all" }, models, NOW)).toEqual(models);
  });

  it("unfiled is exactly the Models with empty projectIds", () => {
    const models = [
      model({ id: "m1" }),
      model({ id: "m2", projectIds: ["prj-b"] }),
      model({ id: "m3", projectIds: ["prj-b", "prj-c"] }),
      model({ id: "m4" }),
    ];
    expect(modelsFor({ kind: "view", id: "unfiled" }, models, NOW).map((m) => m.id)).toEqual(["m1", "m4"]);
  });

  it("recent includes a current revision captured exactly 14 days ago and excludes one a millisecond older", () => {
    const at = (id: string, capturedAt: string) =>
      model({ id, currentRevision: revisionSummary({ modelId: id, capturedAt }) });
    const models = [
      at("today", capturedDaysAgo(0)),
      at("edge", capturedDaysAgo(14)),
      at("past-edge", capturedDaysAgo(14, 1)),
      at("old", capturedDaysAgo(30)),
    ];
    expect(modelsFor({ kind: "view", id: "recent" }, models, NOW).map((m) => m.id)).toEqual(["today", "edge"]);
  });

  it("attention is linked Models whose source is not ok, never managed ones", () => {
    const models = [
      model({ id: "managed" }),
      linked("ok", "ok"),
      linked("missing", "missing"),
      linked("unreadable", "unreadable"),
      linked("notAFile", "notAFile"),
      linked("invalid", "invalidContent"),
      linked("changing", "changing"),
    ];
    expect(modelsFor({ kind: "view", id: "attention" }, models, NOW).map((m) => m.id)).toEqual([
      "missing", "unreadable", "notAFile", "invalid", "changing",
    ]);
  });

  it("gcode is the pre-sliced G-code Models", () => {
    const models = [model({ id: "stl" }), model({ id: "g", format: "gcode" }), model({ id: "3mf", format: "3mf" })];
    expect(modelsFor({ kind: "view", id: "gcode" }, models, NOW).map((m) => m.id)).toEqual(["g"]);
  });

  it("shows a Model in two Projects in both Project views, so Project counts can exceed All Models", () => {
    const models = [
      model({ id: "both", projectIds: ["prj-b", "prj-c"] }),
      model({ id: "only-b", projectIds: ["prj-b"] }),
    ];
    const inB = modelsFor({ kind: "project", id: "prj-b" }, models, NOW).map((m) => m.id);
    const inC = modelsFor({ kind: "project", id: "prj-c" }, models, NOW).map((m) => m.id);
    expect(inB).toEqual(["both", "only-b"]);
    expect(inC).toEqual(["both"]);
    expect(inB.length + inC.length).toBeGreaterThan(modelsFor({ kind: "view", id: "all" }, models, NOW).length);
  });
});

describe("searchModels", () => {
  const models = [
    model({ id: "by-name", name: "Corner Bracket" }),
    model({ id: "by-project", name: "Shelf", projectIds: ["prj-c"] }),
    linked("by-file", "ok", "/home/me/prints/Enclosure-Lid.3mf"),
    model({ id: "none", name: "Knob" }),
  ];

  it("matches the Model name, a Project name, and the linked file basename, case-insensitively", () => {
    expect(searchModels(models, [BRACKETS, CALIBRATION], "bracket").map((m) => m.id)).toEqual(["by-name"]);
    expect(searchModels(models, [BRACKETS, CALIBRATION], "CALIB").map((m) => m.id)).toEqual(["by-project"]);
    expect(searchModels(models, [BRACKETS, CALIBRATION], "enclosure-lid").map((m) => m.id)).toEqual(["by-file"]);
  });

  it("does not match a linked file's directory, only its basename", () => {
    expect(searchModels(models, [BRACKETS, CALIBRATION], "prints")).toEqual([]);
  });

  it("returns every Model for a blank query", () => {
    expect(searchModels(models, [BRACKETS], "   ")).toEqual(models);
  });
});

describe("sortModels", () => {
  const a = model({ id: "a", name: "beta", currentRevision: revisionSummary({ capturedAt: "2026-09-02T00:00:00Z" }) });
  const b = model({ id: "b", name: "Alpha", currentRevision: revisionSummary({ capturedAt: "2026-09-01T00:00:00Z" }) });
  const c = model({ id: "c", name: "gamma", currentRevision: revisionSummary({ capturedAt: "2026-09-03T00:00:00Z" }) });

  it("sorts by name case-insensitively without mutating the input", () => {
    const input = [a, b, c];
    expect(sortModels(input, "name").map((m) => m.id)).toEqual(["b", "a", "c"]);
    expect(input.map((m) => m.id)).toEqual(["a", "b", "c"]);
  });

  it("sorts recently added first by current revision capture time", () => {
    expect(sortModels([a, b, c], "recent").map((m) => m.id)).toEqual(["c", "a", "b"]);
  });
});

describe("viewCounts", () => {
  it("equals the length of each view's filtered list", () => {
    const models = [
      model({ id: "m1", currentRevision: revisionSummary({ capturedAt: capturedDaysAgo(1) }) }),
      model({ id: "m2", projectIds: ["prj-b"], format: "gcode", currentRevision: revisionSummary({ capturedAt: capturedDaysAgo(40) }) }),
      linked("m3", "missing"),
      linked("m4", "ok"),
    ];
    const counts = viewCounts(models, NOW);
    const ids: SavedViewId[] = ["all", "unfiled", "recent", "attention", "gcode"];
    for (const id of ids) {
      expect(counts[id], id).toBe(modelsFor({ kind: "view", id }, models, NOW).length);
    }
    expect(counts).toEqual({ all: 4, unfiled: 3, recent: 1, attention: 1, gcode: 1 });
  });
});
