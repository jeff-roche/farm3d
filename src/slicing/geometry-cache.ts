/** A least-recently-used cache of D6 reads: `RevisionGeometry` per Model
 *  Source Revision, and decoded `MeshBuffer`s per revision and object.
 *  Revisions are immutable, so a revision id names its content and an
 *  entry never goes stale.
 *
 *  - Concurrent reads of the same entry share one request.
 *  - A failed load is not kept; the next read retries.
 *  - Geometry is bounded by entry count, meshes by bytes. The newest mesh
 *    is always kept, even alone over the budget.
 *  - The owner creates one per view and drops it when leaving (the spec's
 *    "dropped when leaving preparation mode").
 *
 *  It holds CPU-side data only. GPU buffers built from a mesh belong to
 *  the renderer, which releases them when the mesh leaves its set
 *  (`setMeshes`) or on `dispose`, so evicting here never frees a buffer
 *  that is still drawn. */
import type { MeshBuffer } from "./mesh-buffer";
import type { RevisionGeometry } from "./types";

export interface GeometryCacheOptions {
  loadGeometry: (revisionId: string) => Promise<RevisionGeometry>;
  loadMesh: (revisionId: string, objectKey: number) => Promise<MeshBuffer>;
  /** Geometry entries kept. Default 8. */
  maxGeometries?: number;
  /** Mesh bytes (positions and indices) kept. Default 256 MiB. */
  maxMeshBytes?: number;
}

export interface GeometryCache {
  geometry(revisionId: string): Promise<RevisionGeometry>;
  mesh(revisionId: string, objectKey: number): Promise<MeshBuffer>;
  /** Forgets every entry. A load already in flight still answers its
   *  callers, but is not kept. */
  clear(): void;
}

export const DEFAULT_MAX_GEOMETRIES = 8;
export const DEFAULT_MAX_MESH_BYTES = 256 * 1024 * 1024;

function meshBytes(mesh: MeshBuffer): number {
  return mesh.positions.byteLength + mesh.indices.byteLength;
}

/** One LRU map: oldest first, in `Map` insertion order. */
class Lru<V> {
  private readonly entries = new Map<string, V>();
  private readonly pending = new Map<string, Promise<V>>();
  private generation = 0;

  constructor(private readonly overBudget: (entries: Map<string, V>) => boolean) {}

  read(key: string, load: () => Promise<V>): Promise<V> {
    const held = this.entries.get(key);
    if (held !== undefined) {
      this.entries.delete(key);
      this.entries.set(key, held);
      return Promise.resolve(held);
    }
    const inFlight = this.pending.get(key);
    if (inFlight) return inFlight;

    const generation = this.generation;
    const request = load().then(
      (value) => {
        if (generation === this.generation) {
          this.pending.delete(key);
          this.entries.set(key, value);
          this.evict();
        }
        return value;
      },
      (error: unknown) => {
        if (generation === this.generation) this.pending.delete(key);
        throw error;
      },
    );
    this.pending.set(key, request);
    return request;
  }

  clear(): void {
    this.generation += 1;
    this.entries.clear();
    this.pending.clear();
  }

  private evict(): void {
    while (this.entries.size > 1 && this.overBudget(this.entries)) {
      const oldest = this.entries.keys().next().value as string;
      this.entries.delete(oldest);
    }
  }
}

export function createGeometryCache(options: GeometryCacheOptions): GeometryCache {
  const maxGeometries = options.maxGeometries ?? DEFAULT_MAX_GEOMETRIES;
  const maxMeshBytes = options.maxMeshBytes ?? DEFAULT_MAX_MESH_BYTES;
  const geometries = new Lru<RevisionGeometry>((entries) => entries.size > maxGeometries);
  const meshes = new Lru<MeshBuffer>((entries) => {
    let total = 0;
    for (const mesh of entries.values()) total += meshBytes(mesh);
    return total > maxMeshBytes;
  });

  return {
    geometry: (revisionId) => geometries.read(revisionId, () => options.loadGeometry(revisionId)),
    mesh: (revisionId, objectKey) => meshes.read(
      `${revisionId}\n${objectKey}`,
      () => options.loadMesh(revisionId, objectKey),
    ),
    clear: () => {
      geometries.clear();
      meshes.clear();
    },
  };
}
