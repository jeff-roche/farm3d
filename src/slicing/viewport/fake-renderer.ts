/** A test-only `ViewportRenderer` that records what it is told. Every
 *  jsdom test gets it through `vitest.setup.ts`'s mock of
 *  `renderer-factory`; `lastFakeRenderer()` returns the newest one. */
import type {
  BuildVolume,
  CameraPose,
  CameraView,
  ObjectKey,
  PickResult,
  RenderedInstance,
  ViewportOverlays,
  ViewportRenderer,
  ViewportTheme,
} from "./renderer";
import { WebGLUnavailableError } from "./renderer";
import type { MeshBuffer } from "../mesh-buffer";

export class FakeViewportRenderer implements ViewportRenderer {
  canvas: HTMLCanvasElement | null = null;
  buildVolume: BuildVolume | null | undefined;
  meshes = new Map<ObjectKey, MeshBuffer>();
  instances: RenderedInstance[] = [];
  cameras: (CameraView | CameraPose)[] = [];
  overlays: ViewportOverlays = {};
  themes: ViewportTheme[] = [];
  disposed = false;
  /** What `pick` answers, whatever the point. */
  pickResult: PickResult | null = null;
  picks: [number, number][] = [];
  /** Makes `mount` throw as it does without WebGL. */
  failMount = false;

  mount(canvas: HTMLCanvasElement): void {
    if (this.failMount) throw new WebGLUnavailableError();
    this.canvas = canvas;
  }
  setBuildVolume(volume: BuildVolume | null): void {
    this.buildVolume = volume;
  }
  setMeshes(meshes: Map<ObjectKey, MeshBuffer>): void {
    this.meshes = new Map(meshes);
  }
  setInstances(instances: RenderedInstance[]): void {
    this.instances = instances.map((instance) => ({ ...instance }));
  }
  setCamera(view: CameraView | CameraPose): void {
    this.cameras.push(view);
  }
  setOverlays(overlays: ViewportOverlays): void {
    this.overlays = overlays;
  }
  pick(x: number, y: number): PickResult | null {
    this.picks.push([x, y]);
    return this.pickResult;
  }
  setTheme(theme: ViewportTheme): void {
    this.themes.push({ ...theme });
  }
  dispose(): void {
    this.disposed = true;
  }
}

const created: FakeViewportRenderer[] = [];
let failNextMount = false;

/** The stand-in for `createViewportRenderer`. */
export async function createFakeViewportRenderer(): Promise<FakeViewportRenderer> {
  const renderer = new FakeViewportRenderer();
  renderer.failMount = failNextMount;
  failNextMount = false;
  created.push(renderer);
  return renderer;
}

export function lastFakeRenderer(): FakeViewportRenderer | undefined {
  return created[created.length - 1];
}

/** The next renderer created fails to mount, as without WebGL. */
export function failNextFakeMount(): void {
  failNextMount = true;
}
