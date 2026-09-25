/** D18: the viewport's renderer, behind an interface so components and
 *  jsdom tests never load WebGL. `three-renderer.ts` is the real one,
 *  loaded lazily through `renderer-factory.ts`; tests use
 *  `FakeViewportRenderer`, which records calls.
 *
 *  World space is the plate's, in millimetres: X right, Y back, Z up,
 *  with the bed origin at (0, 0, 0). */
import type { MeshBuffer } from "../mesh-buffer";
import type { Transform3mf, Vec3 } from "../transforms";
import type { BedShape, PointMm } from "../types";

export type { Vec3 };
export type ObjectKey = number;

/** The printable space: the bed outline, its height, and the areas of the
 *  bed that must stay empty. */
export interface BuildVolume {
  bed: BedShape;
  heightMm: number;
  /** Polygons, each of at least three points. */
  excludeAreas: PointMm[][];
}

export interface RenderedInstance {
  instanceKey: string;
  objectKey: ObjectKey;
  /** Where the object's mesh is placed (D5 composition, resting on Z 0). */
  matrix: Transform3mf;
  selected: boolean;
  /** Drawn with a distinct material. Callers also say so in text, so
   *  colour is never the only signal. */
  outOfBounds: boolean;
}

/** D19's standard views. `reset` is the default iso view, fitted. */
export type CameraView = "top" | "front" | "left" | "right" | "iso" | "reset";

export interface CameraPose {
  positionMm: Vec3;
  targetMm: Vec3;
}

export interface ViewportOverlays {
  /** A measured segment between two picked surface points. */
  measure?: [Vec3, Vec3];
}

export interface PickResult {
  instanceKey: string;
  /** The picked surface point, in world millimetres. */
  pointMm: Vec3;
}

/** CSS colours, read from the `--f3d-color-*` tokens (see `theme.ts`). */
export interface ViewportTheme {
  background: string;
  grid: string;
  volume: string;
  object: string;
  /** The selected object's fill. */
  selected: string;
  /** The selected object's edge outline. */
  outline: string;
  outOfBounds: string;
  excludeArea: string;
  measure: string;
}

/** `mount` throws this when no WebGL context can be created. */
export class WebGLUnavailableError extends Error {
  constructor(message = "WebGL is not available") {
    super(message);
    this.name = "WebGLUnavailableError";
  }
}

export interface ViewportRenderer {
  /** Takes over `canvas` and follows its size. Throws
   *  `WebGLUnavailableError` when WebGL can't start. */
  mount(canvas: HTMLCanvasElement): void;
  /** `null` draws no volume, only a floor grid under the objects (the
   *  read-only inspector, which has no target). */
  setBuildVolume(volume: BuildVolume | null): void;
  /** The meshes instances may use. The renderer owns the GPU buffers it
   *  builds from them and releases a mesh's when it leaves the map. */
  setMeshes(meshes: Map<ObjectKey, MeshBuffer>): void;
  setInstances(instances: RenderedInstance[]): void;
  /** A view fits the build volume, or the instances when there is none. */
  setCamera(view: CameraView | CameraPose): void;
  setOverlays(overlays: ViewportOverlays): void;
  /** The instance under canvas CSS pixel (x, y), if any. */
  pick(x: number, y: number): PickResult | null;
  setTheme(theme: ViewportTheme): void;
  dispose(): void;
}
