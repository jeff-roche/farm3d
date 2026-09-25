import { encodeMeshBuffer } from "./mesh-buffer";
import { revisionSummaryOf } from "./types";
import type {
  CatalogRef,
  GeometryObject,
  LayFlatFace,
  PreparationRecord,
  ProfileSnapshot,
  RevisionGeometry,
  SliceOperationLog,
  SliceOperationRecord,
  SliceOptions,
  SliceRevisionRecord,
  SliceRevisionSummary,
  SlicerRuntimeStatus,
} from "./types";

/** `just web`'s slicing seed data (spec D23), on top of the Library's web
 *  fixture: an OrcaSlicer 2.4.2 runtime, a two-plate Preparation for
 *  `mdl-web-enclosure` with one succeeded and one failed operation (both
 *  with logs), the farm3d revision the succeeded one published, an
 *  external revision on `mdl-web-cube-gcode` whose `materialFamily` was
 *  left absent, and generated geometry and meshes for every non-G-code
 *  fixture revision. There is no Rust backend in web mode, so this stands
 *  in for `list_slicing` and the read commands, written as literals the way
 *  Rust would send them. It is not persistence: web-mode edits last until
 *  the page reloads, and anything that needs OrcaSlicer (slicing, the
 *  pickers) is refused by the store instead of faked.
 *
 *  Dates are relative to `now`; the result is deterministic for a given
 *  `now`. */
export interface WebSlicingFixture {
  runtime: SlicerRuntimeStatus;
  preparations: PreparationRecord[];
  operations: SliceOperationRecord[];
  /** Newest first. */
  revisions: SliceRevisionSummary[];
  /** Keyed by slice revision id. */
  revisionRecords: Record<string, SliceRevisionRecord>;
  /** Keyed by slice operation id. */
  logs: Record<string, SliceOperationLog>;
  /** Keyed by Model Source Revision id. */
  geometry: Record<string, RevisionGeometry>;
  /** D6 `F3DM` buffers, keyed by Model Source Revision id, then object key. */
  meshes: Record<string, Record<number, ArrayBuffer>>;
  /** What `list_slice_options` answers with, for any target. */
  sliceOptions: SliceOptions;
}

export const WEB_SLICING_PREPARATION = "prp-web-enclosure";
export const WEB_SLICING_OPERATION_SUCCEEDED = "sop-web-enclosure-lid";
export const WEB_SLICING_OPERATION_FAILED = "sop-web-enclosure-latch";
export const WEB_SLICING_REVISION_FARM3D = "slr-web-enclosure-lid";
export const WEB_SLICING_REVISION_EXTERNAL = "slr-web-cube-gcode";

const MINUTE_MS = 60 * 1000;

const ORCA_PATH = "/usr/bin/orca-slicer";

const CENTAURI_CARBON: CatalogRef = {
  vendor: "Elegoo",
  model: "Elegoo Centauri Carbon",
  variant: "Elegoo Centauri Carbon 0.4 nozzle",
  modelId: "Elegoo-CC",
  printerVariant: "0.4",
};

function profileSnapshot(): ProfileSnapshot {
  return {
    catalogRef: { ...CENTAURI_CARBON },
    bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
    printableHeightMm: 256,
    bedExcludeAreas: [],
    nozzleType: "hardened_steel",
    gcodeFlavor: "klipper",
  };
}

const PROCESS_PRESET = "0.20mm Standard @Elegoo CC 0.4 nozzle";
const FILAMENT_PRESET = "Elegoo PLA @ECC";

function runtime(): SlicerRuntimeStatus {
  return {
    engine: {
      state: "available", version: "2.4.2", channel: "release", source: "path",
      executableName: "orca-slicer", path: ORCA_PATH, extractAndRun: false,
    },
    presetSource: {
      state: "available", version: "2.4.2", channel: "release", origin: "engine", vendorCount: 62, path: ORCA_PATH,
    },
    canSlice: true,
    versionsDiffer: false,
    revision: 1,
    engineCandidates: [
      { source: "path", executableName: "orca-slicer", path: ORCA_PATH, result: { kind: "chosen", version: "2.4.2" } },
    ],
  };
}

function sliceOptions(): SliceOptions {
  return {
    machinePreset: CENTAURI_CARBON.variant,
    processPresets: [
      { name: "0.12mm Fine @Elegoo CC 0.4 nozzle" },
      { name: PROCESS_PRESET },
      { name: "0.28mm Extra Draft @Elegoo CC 0.4 nozzle" },
    ],
    filamentPresets: [
      { name: FILAMENT_PRESET, filamentType: "PLA", materialFamily: "PLA" },
      { name: "Elegoo PETG @ECC", filamentType: "PETG", materialFamily: "PETG" },
      { name: "Generic PA-CF @ECC", filamentType: "PA-CF", materialFamily: "PA-CF" },
    ],
    defaults: { processPreset: PROCESS_PRESET, filamentPreset: FILAMENT_PRESET },
    profileSnapshot: profileSnapshot(),
    // The web Printer fixture's two Centauri Carbons.
    matchingPrinterIds: ["prn-web-cc-1", "prn-web-cc-2"],
  };
}

// --- Generated meshes ------------------------------------------------------------

interface FixtureMesh {
  positions: number[];
  indices: number[];
}

/** An axis-aligned box from the origin: 8 vertices, 12 triangles, wound
 *  counter-clockwise seen from outside. */
function box(width: number, depth: number, height: number): FixtureMesh {
  const positions = [
    0, 0, 0, width, 0, 0, width, depth, 0, 0, depth, 0,
    0, 0, height, width, 0, height, width, depth, height, 0, depth, height,
  ];
  const indices = [
    0, 2, 1, 0, 3, 2, // bottom
    4, 5, 6, 4, 6, 7, // top
    0, 1, 5, 0, 5, 4, // front
    1, 2, 6, 1, 6, 5, // right
    2, 3, 7, 2, 7, 6, // back
    3, 0, 4, 3, 4, 7, // left
  ];
  return { positions, indices };
}

/** A convex prism over a regular `sides`-gon stretched to `width` x
 *  `depth`, from Z 0 to `height`: 2·sides vertices and 4·sides − 4
 *  triangles (fanned caps). */
function prism(sides: number, width: number, depth: number, height: number): FixtureMesh {
  const positions: number[] = [];
  for (const z of [0, height]) {
    for (let i = 0; i < sides; i += 1) {
      const angle = (2 * Math.PI * i) / sides;
      positions.push(width / 2 + (width / 2) * Math.cos(angle), depth / 2 + (depth / 2) * Math.sin(angle), z);
    }
  }
  const indices: number[] = [];
  for (let i = 1; i < sides - 1; i += 1) indices.push(0, i + 1, i); // bottom
  for (let i = 1; i < sides - 1; i += 1) indices.push(sides, sides + i, sides + i + 1); // top
  for (let i = 0; i < sides; i += 1) {
    const next = (i + 1) % sides;
    indices.push(i, next, sides + next, i, sides + next, sides + i);
  }
  return { positions, indices };
}

/** D6's lay-flat faces for a convex mesh: its triangles grouped by normal,
 *  up to 8, largest first, none below 1% of the largest. */
function layFlatFaces(mesh: FixtureMesh): LayFlatFace[] {
  const faces = new Map<string, LayFlatFace>();
  const at = (index: number) => mesh.positions.slice(index * 3, index * 3 + 3);
  for (let t = 0; t < mesh.indices.length; t += 3) {
    const [a, b, c] = [at(mesh.indices[t]), at(mesh.indices[t + 1]), at(mesh.indices[t + 2])];
    const u = [b[0] - a[0], b[1] - a[1], b[2] - a[2]];
    const v = [c[0] - a[0], c[1] - a[1], c[2] - a[2]];
    const cross = [u[1] * v[2] - u[2] * v[1], u[2] * v[0] - u[0] * v[2], u[0] * v[1] - u[1] * v[0]];
    const length = Math.hypot(cross[0], cross[1], cross[2]);
    if (length === 0) continue;
    const normal: [number, number, number] = [cross[0] / length, cross[1] / length, cross[2] / length];
    const key = normal.map((n) => n.toFixed(6)).join(",");
    const face = faces.get(key) ?? { normal, areaMm2: 0 };
    face.areaMm2 += length / 2;
    faces.set(key, face);
  }
  const sorted = [...faces.values()].sort((x, y) => y.areaMm2 - x.areaMm2);
  const largest = sorted[0]?.areaMm2 ?? 0;
  return sorted.filter((face) => face.areaMm2 >= largest * 0.01).slice(0, 8);
}

function geometryObject(objectKey: number, mesh: FixtureMesh, name?: string): GeometryObject {
  // Bounds over the f32 values the mesh buffer actually carries.
  const values = Float32Array.from(mesh.positions);
  const min: [number, number, number] = [0, 0, 0];
  const max: [number, number, number] = [0, 0, 0];
  for (let axis = 0; axis < 3; axis += 1) {
    const coordinates = values.filter((_, i) => i % 3 === axis);
    min[axis] = coordinates.length ? Math.min(...coordinates) : 0;
    max[axis] = coordinates.length ? Math.max(...coordinates) : 0;
  }
  return {
    objectKey,
    ...(name ? { name } : {}),
    triangleCount: mesh.indices.length / 3,
    boundsMm: { min, max },
    layFlatFaces: layFlatFaces(mesh),
  };
}

function identity(translate: [number, number, number] = [0, 0, 0]): number[] {
  return [1, 0, 0, 0, 1, 0, 0, 0, 1, ...translate];
}

interface FixtureRevisionGeometry {
  objects: { objectKey: number; name?: string; mesh: FixtureMesh }[];
  geometry: Pick<RevisionGeometry, "buildItems">;
}

/** One shape per non-G-code Library fixture revision, with the triangle
 *  counts its `summary` states. */
function fixtureGeometry(): Record<string, FixtureRevisionGeometry> {
  const stl = (mesh: FixtureMesh): FixtureRevisionGeometry => ({
    objects: [{ objectKey: 1, mesh }],
    geometry: { buildItems: [{ objectKey: 1, transform: identity(), printable: true }] },
  });
  return {
    "msr-web-bracket-1": stl(box(40, 40, 5)),
    "msr-web-clip-1": stl(prism(7, 18, 12, 5)),
    "msr-web-clip-2": stl(prism(7, 18, 12, 6)),
    "msr-web-knob-1": stl(prism(28, 25, 25, 14)),
    "msr-web-enclosure-1": {
      objects: [
        { objectKey: 1, name: "Lid", mesh: box(120, 80, 4) },
        { objectKey: 2, name: "Latch", mesh: box(20, 10, 4) },
        { objectKey: 3, name: "Gasket", mesh: box(110, 70, 1) },
      ],
      geometry: {
        buildItems: [
          { objectKey: 1, transform: identity(), plateIndex: 1, printable: true },
          { objectKey: 2, transform: identity([50, 35, 0]), plateIndex: 2, printable: true },
          // Marked non-printable in the source 3MF and on no plate: the
          // Preparation never places it, so it is never sliced (D5).
          { objectKey: 3, transform: identity([5, 5, 0]), printable: false },
        ],
      },
    },
  };
}

// --- Preparation, operations, revisions --------------------------------------

function preparation(at: (minutesAgo: number) => string): PreparationRecord {
  return {
    id: WEB_SLICING_PREPARATION,
    modelId: "mdl-web-enclosure",
    sourceRevisionId: "msr-web-enclosure-1",
    revision: 3,
    stale: false,
    document: {
      plates: [
        {
          plateKey: "plt-web-enclosure-1", name: "Lid",
          instances: [{
            instanceKey: "ins-web-lid", objectKey: 1,
            transform: { translateMm: [128, 128], rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
          }],
        },
        {
          plateKey: "plt-web-enclosure-2", name: "Latch",
          instances: [{
            instanceKey: "ins-web-latch", objectKey: 2,
            transform: { translateMm: [128, 128], rotateDeg: [0, 0, 0], scale: [1, 1, 1] },
          }],
        },
      ],
      target: { kind: "profile", catalogRef: { ...CENTAURI_CARBON } },
      processPreset: PROCESS_PRESET,
      filamentPreset: FILAMENT_PRESET,
      controls: { layerHeightMm: 0.2, infillDensityPercent: 15, supports: "off" },
    },
    createdAt: at(90),
    updatedAt: at(30),
  };
}

function operations(at: (minutesAgo: number) => string): SliceOperationRecord[] {
  return [
    {
      id: WEB_SLICING_OPERATION_SUCCEEDED, preparationId: WEB_SLICING_PREPARATION,
      sourceRevisionId: "msr-web-enclosure-1", plateKey: "plt-web-enclosure-1", plateName: "Lid", plateIndex: 1,
      state: "succeeded", sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
      queuedAt: at(60), startedAt: at(60), finishedAt: at(59),
    },
    {
      // Sliced before the latch was moved back onto the bed.
      id: WEB_SLICING_OPERATION_FAILED, preparationId: WEB_SLICING_PREPARATION,
      sourceRevisionId: "msr-web-enclosure-1", plateKey: "plt-web-enclosure-2", plateName: "Latch", plateIndex: 2,
      state: "failed",
      failure: { code: { kind: "objectsOutsidePlate" }, message: "An object is outside the printable area." },
      queuedAt: at(60), startedAt: at(59), finishedAt: at(59),
    },
  ];
}

function logs(): Record<string, SliceOperationLog> {
  const noise = "(orca-slicer:48213): Gtk-WARNING **: cannot open display: ";
  return {
    [WEB_SLICING_OPERATION_SUCCEEDED]: {
      text: [
        noise,
        "[info] Loading plate 1 of 1",
        "[info] Slicing: 1%",
        "[info] Generating G-code: 90%",
        "[info] Exported out/plate_1.gcode",
        "return_code 0",
      ].join("\n"),
      truncated: false,
      noiseLines: [1],
    },
    [WEB_SLICING_OPERATION_FAILED]: {
      text: [
        noise,
        "[info] Loading plate 1 of 1",
        "[error] Object Latch is outside the printable area",
        "return_code -50",
      ].join("\n"),
      truncated: false,
      noiseLines: [1],
    },
  };
}

function farm3dRevision(at: (minutesAgo: number) => string): SliceRevisionRecord {
  const profile = profileSnapshot();
  return {
    id: WEB_SLICING_REVISION_FARM3D,
    kind: "farm3d",
    modelId: "mdl-web-enclosure",
    sourceRevisionId: "msr-web-enclosure-1",
    sourceRevisionSequence: 1,
    plate: { plateKey: "plt-web-enclosure-1", plateIndex: 1, plateName: "Lid" },
    targetLabel: CENTAURI_CARBON.variant,
    estimates: {
      printSeconds: 5_412, filamentGrams: 38.6, filamentMm: 12_941, layerCount: 20, maxZMm: 4, source: "farm3dSlice",
    },
    facts: {
      printerProfile: { provenance: "farm3dInput", value: profile },
      nozzleDiameterMm: { provenance: "farm3dInput", value: 0.4 },
      materialFamily: { provenance: "farm3dInput", value: "PLA" },
      filamentDiameterMm: { provenance: "farm3dInput", value: 1.75 },
    },
    requiresManualPrinterSelection: false,
    runtime: { engineVersion: "2.4.2", engineChannel: "release", presetSourceVersion: "2.4.2", presetSourceChannel: "release" },
    createdAt: at(59),
    target: {
      target: { kind: "profile", catalogRef: { ...CENTAURI_CARBON } },
      profile: profileSnapshot(),
      machinePreset: CENTAURI_CARBON.variant,
      processPreset: PROCESS_PRESET,
      filamentPreset: FILAMENT_PRESET,
      controls: { layerHeightMm: 0.2, infillDensityPercent: 15, supports: "off" },
    },
    blobs: [
      { role: "plate3mf", sizeBytes: 3_214 },
      { role: "machinePreset", sizeBytes: 18_402 },
      { role: "processPreset", sizeBytes: 22_970 },
      { role: "filamentPreset", sizeBytes: 9_118 },
      { role: "manifest", sizeBytes: 1_047 },
      { role: "log", sizeBytes: 412 },
    ],
  };
}

/** The operator confirmed the printer, nozzle and filament diameter from
 *  the file's claims but left the material absent, so this revision needs
 *  a Printer chosen by hand. */
function externalRevision(at: (minutesAgo: number) => string): SliceRevisionRecord {
  return {
    id: WEB_SLICING_REVISION_EXTERNAL,
    kind: "external",
    modelId: "mdl-web-cube-gcode",
    sourceRevisionId: "msr-web-cube-gcode-1",
    sourceRevisionSequence: 1,
    targetLabel: CENTAURI_CARBON.variant,
    estimates: null,
    facts: {
      printerProfile: { provenance: "operatorConfirmed", value: profileSnapshot() },
      nozzleDiameterMm: { provenance: "operatorConfirmed", value: 0.4 },
      materialFamily: { provenance: "absent", value: null },
      filamentDiameterMm: { provenance: "operatorConfirmed", value: 1.75 },
    },
    requiresManualPrinterSelection: true,
    // 8 days ago: a day after its G-code Model was imported.
    createdAt: at(8 * 24 * 60),
    claimedEstimates: {
      printSeconds: 1_421, filamentGrams: null, filamentMm: null, layerCount: null, maxZMm: 20,
      source: "fileClaim", trusted: false,
    },
    producer: { name: "OrcaSlicer", version: "2.3.0" },
    blobs: [],
  };
}

export function buildWebSlicingFixture(now: Date = new Date()): WebSlicingFixture {
  const at = (minutesAgo: number) => new Date(now.getTime() - minutesAgo * MINUTE_MS).toISOString();
  const records = [farm3dRevision(at), externalRevision(at)];
  const revisionRecords: Record<string, SliceRevisionRecord> = {};
  for (const record of records) revisionRecords[record.id] = record;
  const revisions = records
    .map(revisionSummaryOf)
    .sort((a, b) => b.createdAt.localeCompare(a.createdAt));

  const geometry: Record<string, RevisionGeometry> = {};
  const meshes: Record<string, Record<number, ArrayBuffer>> = {};
  for (const [revisionId, entry] of Object.entries(fixtureGeometry())) {
    geometry[revisionId] = {
      objects: entry.objects.map(({ objectKey, name, mesh }) => geometryObject(objectKey, mesh, name)),
      buildItems: entry.geometry.buildItems,
    };
    meshes[revisionId] = {};
    for (const { objectKey, mesh } of entry.objects) {
      meshes[revisionId][objectKey] = encodeMeshBuffer(Float32Array.from(mesh.positions), Uint32Array.from(mesh.indices));
    }
  }

  return {
    runtime: runtime(),
    preparations: [preparation(at)],
    operations: operations(at),
    revisions,
    revisionRecords,
    logs: logs(),
    geometry,
    meshes,
    sliceOptions: sliceOptions(),
  };
}
