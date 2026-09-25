import type {
  Inspection,
  ModelRecord,
  ModelSourceRevisionRecord,
  ModelSourceRevisionSummary,
  ProjectRecord,
  RevisionThumbnail,
} from "./types";

/** `just web`'s Library seed data (spec §Frontend State): two Projects,
 *  five Models, and embedded 1x1 thumbnails. There is no Rust backend in
 *  web mode, so this stands in for a `list_library` snapshot, written as
 *  literals the way Rust would send them (only `modelCount` and
 *  `revisionCount` are counted, so they can't disagree). It is not
 *  persistence: edits in web mode live only until the page reloads, and
 *  anything that needs real files (picking, importing, linked-source
 *  checks) is refused by the store instead of faked.
 *
 *  Dates are relative to `now`, so the Recently added view always has
 *  something in it; the result is deterministic for a given `now`. */
export interface WebLibraryFixture {
  projects: ProjectRecord[];
  models: ModelRecord[];
  /** Newest first, as `list_model_revisions` returns them. */
  revisions: Record<string, ModelSourceRevisionRecord[]>;
  /** Keyed by revision id. */
  thumbnails: Record<string, RevisionThumbnail>;
}

export const WEB_FIXTURE_PROJECT_BRACKETS = "prj-web-brackets";
export const WEB_FIXTURE_PROJECT_CALIBRATION = "prj-web-calibration";

/** A 1x1 transparent PNG. */
const ONE_PIXEL_PNG =
  "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNkYAAAAAYAAjCB0C8AAAAASUVORK5CYII=";

const DAY_MS = 24 * 60 * 60 * 1000;

function sha(seed: string): string {
  return seed.repeat(64).slice(0, 64);
}

interface FixtureModel {
  model: Omit<ModelRecord, "currentRevision" | "revisionCount">;
  revision: Omit<ModelSourceRevisionSummary, "modelId" | "format">;
  inspection: Inspection;
  sourcePath: string;
}

function fixtureModels(at: (daysAgo: number) => string): FixtureModel[] {
  return [
    {
      // Managed STL in both Projects.
      model: {
        id: "mdl-web-bracket", revision: 3, name: "Corner bracket",
        projectIds: [WEB_FIXTURE_PROJECT_BRACKETS, WEB_FIXTURE_PROJECT_CALIBRATION],
        format: "stl", storageMode: "managed", link: null, createdAt: at(30), updatedAt: at(3),
      },
      revision: {
        id: "msr-web-bracket-1", sequence: 1, sha256: sha("b1"), sizeBytes: 684, origin: "import",
        sourceFileName: "corner-bracket.stl", capturedAt: at(30), hasThumbnail: false,
        summary: { format: "stl", triangleCount: 12, boundsMm: { min: [0, 0, 0], max: [40, 40, 5] }, unitsAssumed: true },
      },
      inspection: {
        format: "stl", encoding: "binary", triangleCount: 12,
        boundsMm: { min: [0, 0, 0], max: [40, 40, 5] }, unitsAssumed: true,
      },
      sourcePath: "/home/maker/Downloads/corner-bracket.stl",
    },
    {
      // Linked 3MF on two plates (the slicing fixtures prepare both), source
      // ok, with unsupported slicer contents.
      model: {
        id: "mdl-web-enclosure", revision: 2, name: "Enclosure lid",
        projectIds: [WEB_FIXTURE_PROJECT_BRACKETS],
        format: "3mf", storageMode: "linked",
        link: { path: "/home/maker/prints/enclosure-lid.3mf", state: "ok", checkedAt: at(0), watchMode: "watching" },
        createdAt: at(2), updatedAt: at(2),
      },
      revision: {
        id: "msr-web-enclosure-1", sequence: 1, sha256: sha("e1"), sizeBytes: 48_213, origin: "import",
        sourceFileName: "enclosure-lid.3mf", capturedAt: at(2), hasThumbnail: true,
        summary: {
          format: "3mf", objectCount: 2, plateCount: 2, triangleCount: 24,
          boundsMm: { min: [0, 0, 0], max: [120, 80, 4] }, unsupportedCount: 1,
        },
      },
      inspection: {
        format: "3mf", unit: "millimeter", producer: "PrusaSlicer-2.9.6", objectCount: 2, buildItemCount: 2,
        triangleCount: 24, boundsMm: { min: [0, 0, 0], max: [120, 80, 4] },
        plates: [{ index: 1, name: "Lid", objectIds: [1] }, { index: 2, name: "Latch", objectIds: [2] }],
        requiredExtensions: [],
        unsupported: [{ part: "Metadata/Slic3r_PE.config", code: "SLICER_SETTINGS", detail: "PrusaSlicer print settings" }],
        thumbnails: [{ format: "png", width: 1, height: 1, part: "Metadata/thumbnail.png" }],
      },
      sourcePath: "/home/maker/prints/enclosure-lid.3mf",
    },
    {
      // Linked STL whose source is missing.
      model: {
        id: "mdl-web-clip", revision: 4, name: "Cable clip",
        projectIds: [WEB_FIXTURE_PROJECT_CALIBRATION],
        format: "stl", storageMode: "linked",
        link: { path: "/home/maker/prints/cable-clip.stl", state: "missing", checkedAt: at(0), watchMode: "watching" },
        createdAt: at(20), updatedAt: at(1),
      },
      revision: {
        id: "msr-web-clip-2", sequence: 2, sha256: sha("c2"), sizeBytes: 1_284, origin: "linkedChange",
        sourceFileName: "cable-clip.stl", capturedAt: at(18), hasThumbnail: false,
        summary: { format: "stl", triangleCount: 24, boundsMm: { min: [0, 0, 0], max: [18, 12, 6] }, unitsAssumed: true },
      },
      inspection: {
        format: "stl", encoding: "ascii", solidName: "clip", triangleCount: 24,
        boundsMm: { min: [0, 0, 0], max: [18, 12, 6] }, unitsAssumed: true,
      },
      sourcePath: "/home/maker/prints/cable-clip.stl",
    },
    {
      // Pre-sliced G-code with verbatim claims, in both Projects.
      model: {
        id: "mdl-web-cube-gcode", revision: 1, name: "Calibration cube (sliced)",
        projectIds: [WEB_FIXTURE_PROJECT_BRACKETS, WEB_FIXTURE_PROJECT_CALIBRATION],
        format: "gcode", storageMode: "managed", link: null, createdAt: at(9), updatedAt: at(9),
      },
      revision: {
        id: "msr-web-cube-gcode-1", sequence: 1, sha256: sha("g1"), sizeBytes: 912_004, origin: "import",
        sourceFileName: "calibration-cube.gcode", capturedAt: at(9), hasThumbnail: true,
        summary: {
          format: "gcode", producer: { name: "OrcaSlicer", version: "2.3.0" }, lineCount: 41_207,
          claimedPrinterModel: "Elegoo Centauri Carbon", claimedEstimatedTime: "23m 41s",
        },
      },
      inspection: {
        format: "gcode", producer: { name: "OrcaSlicer", version: "2.3.0" }, trusted: false,
        claims: [
          { key: "printer_model", value: "Elegoo Centauri Carbon", line: 12 },
          { key: "estimated printing time (normal mode)", value: "23m 41s", line: 41_190 },
          { key: "filament_type", value: "PLA", line: 41_201 },
        ],
        lineCount: 41_207, commandCount: 39_880, toolsUsed: [0],
        relativePositioningSeen: false, relativeExtrusionSeen: true,
        observedBoundsMm: { min: [118, 118, 0.2], max: [138, 138, 20] },
        thumbnails: [{ format: "png", width: 1, height: 1, line: 3 }],
      },
      sourcePath: "/home/maker/Downloads/calibration-cube.gcode",
    },
    {
      // Unfiled.
      model: {
        id: "mdl-web-knob", revision: 1, name: "Spare knob",
        projectIds: [], format: "stl", storageMode: "managed", link: null, createdAt: at(45), updatedAt: at(45),
      },
      revision: {
        id: "msr-web-knob-1", sequence: 1, sha256: sha("k1"), sizeBytes: 5_484, origin: "import",
        sourceFileName: "knob.stl", capturedAt: at(45), hasThumbnail: false,
        summary: { format: "stl", triangleCount: 108, boundsMm: { min: [0, 0, 0], max: [25, 25, 14] }, unitsAssumed: true },
      },
      inspection: {
        format: "stl", encoding: "binary", triangleCount: 108,
        boundsMm: { min: [0, 0, 0], max: [25, 25, 14] }, unitsAssumed: true,
      },
      sourcePath: "/home/maker/Downloads/knob.stl",
    },
  ];
}

/** The earlier revision behind the missing clip's current one, so one
 *  fixture Model has a history longer than a single entry. */
function clipFirstRevision(at: (daysAgo: number) => string): ModelSourceRevisionRecord {
  return {
    id: "msr-web-clip-1", modelId: "mdl-web-clip", sequence: 1, sha256: sha("c1"), sizeBytes: 1_284,
    format: "stl", origin: "import", sourceFileName: "cable-clip.stl", capturedAt: at(20), hasThumbnail: false,
    summary: { format: "stl", triangleCount: 24, boundsMm: { min: [0, 0, 0], max: [18, 12, 5] }, unitsAssumed: true },
    sourcePath: "/home/maker/prints/cable-clip.stl", sourceMtime: at(20), inspectorVersion: 1,
    inspection: {
      format: "stl", encoding: "ascii", solidName: "clip", triangleCount: 24,
      boundsMm: { min: [0, 0, 0], max: [18, 12, 5] }, unitsAssumed: true,
    },
    warnings: [],
  };
}

export function buildWebLibraryFixture(now: Date = new Date()): WebLibraryFixture {
  const at = (daysAgo: number) => new Date(now.getTime() - daysAgo * DAY_MS).toISOString();
  const models: ModelRecord[] = [];
  const revisions: Record<string, ModelSourceRevisionRecord[]> = {};
  const thumbnails: Record<string, RevisionThumbnail> = {};

  for (const entry of fixtureModels(at)) {
    const summary: ModelSourceRevisionSummary = { ...entry.revision, modelId: entry.model.id, format: entry.model.format };
    const history: ModelSourceRevisionRecord[] = [{
      ...summary,
      sourcePath: entry.sourcePath,
      sourceMtime: summary.capturedAt,
      inspectorVersion: 1,
      inspection: entry.inspection,
      warnings: [],
    }];
    if (entry.model.id === "mdl-web-clip") history.push(clipFirstRevision(at));
    models.push({ ...entry.model, currentRevision: summary, revisionCount: history.length });
    revisions[entry.model.id] = history;
    if (summary.hasThumbnail) {
      thumbnails[summary.id] = { mediaType: "image/png", width: 1, height: 1, dataBase64: ONE_PIXEL_PNG };
    }
  }

  const memberCount = (projectId: string) => models.filter((m) => m.projectIds.includes(projectId)).length;
  const projects: ProjectRecord[] = [
    {
      id: WEB_FIXTURE_PROJECT_BRACKETS, revision: 1, name: "Brackets",
      modelCount: memberCount(WEB_FIXTURE_PROJECT_BRACKETS), createdAt: at(60), updatedAt: at(60),
    },
    {
      id: WEB_FIXTURE_PROJECT_CALIBRATION, revision: 1, name: "Calibration",
      modelCount: memberCount(WEB_FIXTURE_PROJECT_CALIBRATION), createdAt: at(60), updatedAt: at(60),
    },
  ];
  return { projects, models, revisions, thumbnails };
}
