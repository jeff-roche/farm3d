/** D18: the three.js `ViewportRenderer`. Imperative, and loaded only
 *  through `renderer-factory.ts`, so three.js stays out of the main bundle
 *  and out of jsdom.
 *
 *  - One indexed `BufferGeometry` per object; instances are meshes that
 *    share it, placed by their matrix. Flat shading derives face normals
 *    on the GPU, so nothing is computed per frame and welded vertices never
 *    smear a hard edge.
 *  - Frames render on demand (after a change or a camera move), not in a
 *    loop.
 *  - World space is Z-up millimetres, as the plate's. */
import {
  AmbientLight,
  Box3,
  BufferAttribute,
  BufferGeometry,
  Color,
  DirectionalLight,
  DoubleSide,
  EdgesGeometry,
  Float32BufferAttribute,
  Group,
  Line,
  LineBasicMaterial,
  LineSegments,
  Matrix4,
  Mesh,
  MeshStandardMaterial,
  Points,
  PointsMaterial,
  PerspectiveCamera,
  Raycaster,
  Scene,
  Sphere,
  Vector2,
  Vector3,
  WebGLRenderer,
  type Material,
  type Object3D,
} from "three";
import { OrbitControls } from "three/examples/jsm/controls/OrbitControls.js";
import type { MeshBuffer } from "../mesh-buffer";
import type { PointMm } from "../types";
import { bedOutline } from "./build-volume";
import { gridLines, hatchLines, type Segment2 } from "./plate-lines";
import {
  WebGLUnavailableError,
  type BuildVolume,
  type CameraPose,
  type CameraView,
  type ObjectKey,
  type PickResult,
  type RenderedInstance,
  type Vec3,
  type ViewportOverlays,
  type ViewportRenderer,
  type ViewportTheme,
} from "./renderer";

/** Edges sharper than this outline a selected object. */
const OUTLINE_ANGLE_DEG = 30;
const CAMERA_TWEEN_MS = 250;
const FIELD_OF_VIEW_DEG = 35;
/** Around the objects when there is no build volume. */
const FLOOR_MARGIN_MM = 20;

/** Unit directions from the target to the camera for each standard view. */
const VIEW_DIRECTIONS: Record<Exclude<CameraView, "reset">, Vec3> = {
  // A hair off the pole, so "up" on screen stays +Y.
  top: [0, -1e-4, 1],
  front: [0, -1, 0],
  left: [-1, 0, 0],
  right: [1, 0, 0],
  iso: [-0.9, -1.2, 1],
};

interface ObjectGeometry {
  source: MeshBuffer;
  geometry: BufferGeometry;
  /** Built on first selection. */
  edges?: EdgesGeometry;
}

function prefersReducedMotion(): boolean {
  return typeof window.matchMedia === "function"
    && window.matchMedia("(prefers-reduced-motion: reduce)").matches;
}

function toMatrix4(m: readonly number[]): Matrix4 {
  // 3MF row-vector order to three's column-vector matrix.
  return new Matrix4().set(
    m[0], m[3], m[6], m[9],
    m[1], m[4], m[7], m[10],
    m[2], m[5], m[8], m[11],
    0, 0, 0, 1,
  );
}

function segmentsGeometry(segments: Segment2[], z: number): BufferGeometry {
  const values = new Float32Array(segments.length * 6);
  segments.forEach(([[ax, ay], [bx, by]], i) => values.set([ax, ay, z, bx, by, z], i * 6));
  const geometry = new BufferGeometry();
  geometry.setAttribute("position", new BufferAttribute(values, 3));
  return geometry;
}

/** A closed polygon's edges. */
function outlineSegments(points: PointMm[]): Segment2[] {
  return points.map((point, i) => {
    const next = points[(i + 1) % points.length];
    return [[point.xMm, point.yMm], [next.xMm, next.yMm]];
  });
}

function disposeTree(root: Object3D, keepGeometry: (geometry: BufferGeometry) => boolean = () => false) {
  root.traverse((node) => {
    const drawable = node as Partial<Mesh>;
    if (drawable.geometry && !keepGeometry(drawable.geometry)) drawable.geometry.dispose();
  });
}

export class ThreeViewportRenderer implements ViewportRenderer {
  private renderer?: WebGLRenderer;
  private canvas?: HTMLCanvasElement;
  private controls?: OrbitControls;
  private resizeObserver?: ResizeObserver;
  private frame = 0;
  private tween = 0;

  private readonly scene = new Scene();
  private readonly camera = new PerspectiveCamera(FIELD_OF_VIEW_DEG, 1, 1, 100_000);
  private readonly volumeGroup = new Group();
  private readonly instanceGroup = new Group();
  private readonly overlayGroup = new Group();

  private readonly materials = {
    object: new MeshStandardMaterial({
      flatShading: true, roughness: 0.85, metalness: 0, side: DoubleSide,
      polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1,
    }),
    outOfBounds: new MeshStandardMaterial({
      flatShading: true, roughness: 0.85, metalness: 0, side: DoubleSide,
      polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1,
    }),
    selected: new MeshStandardMaterial({
      flatShading: true, roughness: 0.85, metalness: 0, side: DoubleSide,
      polygonOffset: true, polygonOffsetFactor: 1, polygonOffsetUnits: 1,
    }),
    outline: new LineBasicMaterial(),
    grid: new LineBasicMaterial({ transparent: true, opacity: 0.6 }),
    gridMajor: new LineBasicMaterial(),
    volume: new LineBasicMaterial(),
    exclude: new LineBasicMaterial(),
    measure: new LineBasicMaterial({ depthTest: false }),
    measurePoints: new PointsMaterial({ size: 6, sizeAttenuation: false, depthTest: false }),
  };

  private readonly geometries = new Map<ObjectKey, ObjectGeometry>();
  private instances: RenderedInstance[] = [];
  private volume: BuildVolume | null = null;
  private themed = false;

  constructor() {
    this.camera.up.set(0, 0, 1);
    this.camera.position.set(-150, -200, 170);
    // A headlight, so every view is lit from the viewer's side.
    const headlight = new DirectionalLight(0xffffff, 1.6);
    headlight.position.set(0.3, 0.6, 1);
    this.camera.add(headlight);
    this.scene.add(new AmbientLight(0xffffff, 1.1), this.camera, this.volumeGroup, this.instanceGroup, this.overlayGroup);
  }

  mount(canvas: HTMLCanvasElement): void {
    let renderer: WebGLRenderer;
    try {
      renderer = new WebGLRenderer({ canvas, antialias: true });
    } catch (error) {
      throw new WebGLUnavailableError(error instanceof Error ? error.message : undefined);
    }
    this.renderer = renderer;
    this.canvas = canvas;
    renderer.setPixelRatio(window.devicePixelRatio || 1);
    this.controls = new OrbitControls(this.camera, canvas);
    this.controls.addEventListener("change", () => this.requestRender());
    this.resizeObserver = new ResizeObserver(() => this.resize());
    this.resizeObserver.observe(canvas);
    this.resize();
    this.rebuildVolume();
    this.fit("reset", true);
  }

  setBuildVolume(volume: BuildVolume | null): void {
    this.volume = volume;
    this.rebuildVolume();
    this.requestRender();
  }

  setMeshes(meshes: Map<ObjectKey, MeshBuffer>): void {
    for (const [key, held] of this.geometries) {
      if (meshes.get(key) === held.source) continue;
      held.geometry.dispose();
      held.edges?.dispose();
      this.geometries.delete(key);
    }
    for (const [key, source] of meshes) {
      if (this.geometries.has(key)) continue;
      const geometry = new BufferGeometry();
      geometry.setAttribute("position", new BufferAttribute(source.positions, 3));
      geometry.setIndex(new BufferAttribute(source.indices, 1));
      geometry.computeBoundingBox();
      geometry.computeBoundingSphere();
      this.geometries.set(key, { source, geometry });
    }
    this.rebuildInstances();
  }

  setInstances(instances: RenderedInstance[]): void {
    this.instances = instances.map((instance) => ({ ...instance, matrix: [...instance.matrix] }));
    this.rebuildInstances();
  }

  setCamera(view: CameraView | CameraPose): void {
    if (typeof view === "string") {
      this.fit(view, prefersReducedMotion());
    } else {
      this.moveCamera(new Vector3(...view.positionMm), new Vector3(...view.targetMm), prefersReducedMotion());
    }
  }

  setOverlays(overlays: ViewportOverlays): void {
    disposeTree(this.overlayGroup);
    this.overlayGroup.clear();
    if (overlays.measure) {
      const [a, b] = overlays.measure;
      const geometry = new BufferGeometry().setAttribute("position", new Float32BufferAttribute([...a, ...b], 3));
      const line = new Line(geometry, this.materials.measure);
      const points = new Points(geometry, this.materials.measurePoints);
      line.renderOrder = points.renderOrder = 10;
      this.overlayGroup.add(line, points);
    }
    this.requestRender();
  }

  pick(x: number, y: number): PickResult | null {
    if (!this.canvas) return null;
    const width = this.canvas.clientWidth;
    const height = this.canvas.clientHeight;
    if (width === 0 || height === 0) return null;
    const raycaster = new Raycaster();
    raycaster.setFromCamera(new Vector2((x / width) * 2 - 1, -(y / height) * 2 + 1), this.camera);
    const meshes = this.instanceGroup.children.filter((child): child is Mesh => child instanceof Mesh);
    const [hit] = raycaster.intersectObjects(meshes, false);
    if (!hit) return null;
    return { instanceKey: hit.object.userData.instanceKey as string, pointMm: [hit.point.x, hit.point.y, hit.point.z] };
  }

  setTheme(theme: ViewportTheme): void {
    const color = (value: string) => new Color().setStyle(value);
    this.scene.background = color(theme.background);
    this.materials.object.color = color(theme.object);
    this.materials.outOfBounds.color = color(theme.outOfBounds);
    // The selection is tinted as well as outlined, so it reads against
    // any object colour; out of bounds still wins.
    this.materials.selected.color = color(theme.selected);
    this.materials.outline.color = color(theme.outline);
    this.materials.grid.color = color(theme.grid);
    this.materials.gridMajor.color = color(theme.volume);
    this.materials.volume.color = color(theme.volume);
    this.materials.exclude.color = color(theme.excludeArea);
    this.materials.measure.color = color(theme.measure);
    this.materials.measurePoints.color = color(theme.measure);
    this.themed = true;
    this.requestRender();
  }

  dispose(): void {
    cancelAnimationFrame(this.frame);
    cancelAnimationFrame(this.tween);
    this.resizeObserver?.disconnect();
    this.controls?.dispose();
    disposeTree(this.scene);
    for (const held of this.geometries.values()) {
      held.geometry.dispose();
      held.edges?.dispose();
    }
    this.geometries.clear();
    for (const material of Object.values(this.materials) as Material[]) material.dispose();
    this.renderer?.dispose();
    this.renderer = undefined;
  }

  // --- Drawing ---------------------------------------------------------------------------

  private requestRender(): void {
    if (!this.renderer || this.frame) return;
    this.frame = requestAnimationFrame(() => {
      this.frame = 0;
      // Nothing is drawn until the tokens are known, so no frame ever
      // shows a colour that isn't the theme's.
      if (this.renderer && this.themed) this.renderer.render(this.scene, this.camera);
    });
  }

  private resize(): void {
    if (!this.renderer || !this.canvas) return;
    const width = this.canvas.clientWidth;
    const height = this.canvas.clientHeight;
    if (width === 0 || height === 0) return;
    this.renderer.setSize(width, height, false);
    this.camera.aspect = width / height;
    this.camera.updateProjectionMatrix();
    this.requestRender();
  }

  private isOwnGeometry = (geometry: BufferGeometry): boolean => {
    for (const held of this.geometries.values()) {
      if (held.geometry === geometry || held.edges === geometry) return true;
    }
    return false;
  };

  private rebuildInstances(): void {
    disposeTree(this.instanceGroup, this.isOwnGeometry);
    this.instanceGroup.clear();
    for (const instance of this.instances) {
      const held = this.geometries.get(instance.objectKey);
      if (!held) continue;
      const matrix = toMatrix4(instance.matrix);
      const material = instance.outOfBounds
        ? this.materials.outOfBounds
        : instance.selected ? this.materials.selected : this.materials.object;
      const mesh = new Mesh(held.geometry, material);
      mesh.matrixAutoUpdate = false;
      mesh.matrix.copy(matrix);
      mesh.userData.instanceKey = instance.instanceKey;
      this.instanceGroup.add(mesh);
      if (instance.selected) {
        held.edges ??= new EdgesGeometry(held.geometry, OUTLINE_ANGLE_DEG);
        const outline = new LineSegments(held.edges, this.materials.outline);
        outline.matrixAutoUpdate = false;
        outline.matrix.copy(matrix);
        this.instanceGroup.add(outline);
      }
    }
    this.instanceGroup.updateMatrixWorld(true);
    if (!this.volume) this.rebuildVolume();
    this.requestRender();
  }

  /** The build volume, or with none a floor grid under the objects. */
  private rebuildVolume(): void {
    disposeTree(this.volumeGroup);
    this.volumeGroup.clear();
    if (this.volume) {
      const outline = bedOutline(this.volume.bed);
      if (outline.length < 3) return;
      const edges = outlineSegments(outline);
      const { minor, major } = gridLines(outline);
      this.volumeGroup.add(
        new LineSegments(segmentsGeometry(minor, 0), this.materials.grid),
        new LineSegments(segmentsGeometry(major, 0), this.materials.gridMajor),
        new LineSegments(segmentsGeometry(edges, 0), this.materials.volume),
        new LineSegments(segmentsGeometry(edges, this.volume.heightMm), this.materials.volume),
        new LineSegments(this.uprights(outline.map((p) => [p.xMm, p.yMm]), this.volume.heightMm), this.materials.volume),
      );
      for (const area of this.volume.excludeAreas) {
        // A hair above the bed so the hatching wins over the grid.
        const lines = [...outlineSegments(area), ...hatchLines(area)];
        this.volumeGroup.add(new LineSegments(segmentsGeometry(lines, 0.05), this.materials.exclude));
      }
      return;
    }
    const box = new Box3().setFromObject(this.instanceGroup);
    const [x0, y0, x1, y1] = box.isEmpty()
      ? [-50, -50, 50, 50]
      : [box.min.x, box.min.y, box.max.x, box.max.y].map((value, i) => {
        const outward = i < 2 ? -FLOOR_MARGIN_MM : FLOOR_MARGIN_MM;
        return (i < 2 ? Math.floor : Math.ceil)((value + outward) / 10) * 10;
      });
    const floor = [{ xMm: x0, yMm: y0 }, { xMm: x1, yMm: y0 }, { xMm: x1, yMm: y1 }, { xMm: x0, yMm: y1 }];
    const { minor, major } = gridLines(floor);
    this.volumeGroup.add(
      new LineSegments(segmentsGeometry(minor, 0), this.materials.grid),
      new LineSegments(segmentsGeometry(major, 0), this.materials.grid),
    );
  }

  private uprights(points: [number, number][], height: number): BufferGeometry {
    const values = new Float32Array(points.length * 6);
    points.forEach(([x, y], i) => values.set([x, y, 0, x, y, height], i * 6));
    return new BufferGeometry().setAttribute("position", new BufferAttribute(values, 3));
  }

  // --- Camera ----------------------------------------------------------------------------

  /** What a view frames: the build volume, else the objects, else a
   *  100 mm cube at the origin. */
  private framedBox(): Box3 {
    if (this.volume) {
      const outline = bedOutline(this.volume.bed);
      if (outline.length >= 3) {
        const box = new Box3();
        for (const point of outline) box.expandByPoint(new Vector3(point.xMm, point.yMm, 0));
        box.expandByPoint(new Vector3(box.min.x, box.min.y, this.volume.heightMm));
        return box;
      }
    }
    const box = new Box3().setFromObject(this.instanceGroup);
    return box.isEmpty() ? new Box3(new Vector3(-50, -50, 0), new Vector3(50, 50, 100)) : box;
  }

  private fit(view: CameraView, jump: boolean): void {
    const sphere = this.framedBox().getBoundingSphere(new Sphere());
    const direction = new Vector3(...VIEW_DIRECTIONS[view === "reset" ? "iso" : view]).normalize();
    const vertical = (FIELD_OF_VIEW_DEG * Math.PI) / 180;
    const horizontal = 2 * Math.atan(Math.tan(vertical / 2) * this.camera.aspect);
    const distance = (Math.max(sphere.radius, 1) / Math.sin(Math.min(vertical, horizontal) / 2)) * 1.05;
    const position = sphere.center.clone().addScaledVector(direction, distance);
    this.moveCamera(position, sphere.center, jump);
  }

  private moveCamera(position: Vector3, target: Vector3, jump: boolean): void {
    cancelAnimationFrame(this.tween);
    const controls = this.controls;
    const apply = (p: Vector3, t: Vector3) => {
      this.camera.position.copy(p);
      if (controls) {
        controls.target.copy(t);
        controls.update();
      } else {
        this.camera.lookAt(t);
      }
      this.requestRender();
    };
    if (jump || !this.renderer) {
      apply(position, target);
      return;
    }
    const fromPosition = this.camera.position.clone();
    const fromTarget = controls ? controls.target.clone() : target.clone();
    const start = performance.now();
    const step = () => {
      const t = Math.min(1, Math.max(0, (performance.now() - start) / CAMERA_TWEEN_MS));
      const eased = 1 - (1 - t) ** 3;
      apply(fromPosition.clone().lerp(position, eased), fromTarget.clone().lerp(target, eased));
      this.tween = t < 1 ? requestAnimationFrame(step) : 0;
    };
    this.tween = requestAnimationFrame(step);
  }
}
