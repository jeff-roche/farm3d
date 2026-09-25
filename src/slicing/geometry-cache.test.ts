import { describe, expect, it, vi } from "vitest";
import { createGeometryCache } from "./geometry-cache";
import type { MeshBuffer } from "./mesh-buffer";
import type { RevisionGeometry } from "./types";

function geometry(tag: number): RevisionGeometry {
  return { objects: [], buildItems: [{ objectKey: tag, transform: [], printable: true }] };
}

/** A mesh of `vertices` vertices and no triangles: 12 bytes a vertex. */
function mesh(vertices: number): MeshBuffer {
  return {
    vertexCount: vertices, indexCount: 0, triangleCount: 0,
    positions: new Float32Array(vertices * 3), indices: new Uint32Array(),
  };
}

function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (error: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}

describe("geometry cache", () => {
  it("answers a repeated geometry read from the cache", async () => {
    const loadGeometry = vi.fn(async (id: string) => geometry(id.length));
    const cache = createGeometryCache({ loadGeometry, loadMesh: vi.fn() });

    const first = await cache.geometry("msr-a");
    expect(await cache.geometry("msr-a")).toBe(first);
    expect(loadGeometry).toHaveBeenCalledOnce();
  });

  it("shares one in-flight request between concurrent readers", async () => {
    const pending = deferred<MeshBuffer>();
    const loadMesh = vi.fn(() => pending.promise);
    const cache = createGeometryCache({ loadGeometry: vi.fn(), loadMesh });

    const a = cache.mesh("msr-a", 1);
    const b = cache.mesh("msr-a", 1);
    const loaded = mesh(1);
    pending.resolve(loaded);
    expect(await a).toBe(loaded);
    expect(await b).toBe(loaded);
    expect(loadMesh).toHaveBeenCalledOnce();
    expect(loadMesh).toHaveBeenCalledWith("msr-a", 1);
  });

  it("keys meshes by revision and object", async () => {
    const loadMesh = vi.fn(async () => mesh(1));
    const cache = createGeometryCache({ loadGeometry: vi.fn(), loadMesh });

    await cache.mesh("msr-a", 1);
    await cache.mesh("msr-a", 2);
    await cache.mesh("msr-b", 1);
    await cache.mesh("msr-a", 1);
    expect(loadMesh).toHaveBeenCalledTimes(3);
  });

  it("does not keep a failed load, so the next read retries", async () => {
    const failure = new Error("offline");
    const loadGeometry = vi.fn()
      .mockRejectedValueOnce(failure)
      .mockResolvedValueOnce(geometry(1));
    const cache = createGeometryCache({ loadGeometry, loadMesh: vi.fn() });

    await expect(cache.geometry("msr-a")).rejects.toBe(failure);
    expect(await cache.geometry("msr-a")).toEqual(geometry(1));
    expect(loadGeometry).toHaveBeenCalledTimes(2);
  });

  it("evicts the least recently used geometry past its capacity", async () => {
    const loadGeometry = vi.fn(async (id: string) => geometry(id.length));
    const cache = createGeometryCache({ loadGeometry, loadMesh: vi.fn(), maxGeometries: 2 });

    await cache.geometry("a");
    await cache.geometry("b");
    await cache.geometry("a"); // a is now the most recent
    await cache.geometry("c"); // evicts b
    loadGeometry.mockClear();

    await cache.geometry("a");
    await cache.geometry("c");
    expect(loadGeometry).not.toHaveBeenCalled();
    await cache.geometry("b");
    expect(loadGeometry).toHaveBeenCalledOnce();
  });

  it("evicts the least recently used meshes past its byte budget", async () => {
    // 10 vertices = 120 bytes each; the budget holds two.
    const loadMesh = vi.fn(async () => mesh(10));
    const cache = createGeometryCache({ loadGeometry: vi.fn(), loadMesh, maxMeshBytes: 250 });

    await cache.mesh("r", 1);
    await cache.mesh("r", 2);
    await cache.mesh("r", 1);
    await cache.mesh("r", 3); // evicts 2
    loadMesh.mockClear();

    await cache.mesh("r", 1);
    await cache.mesh("r", 3);
    expect(loadMesh).not.toHaveBeenCalled();
    await cache.mesh("r", 2);
    expect(loadMesh).toHaveBeenCalledOnce();
  });

  it("keeps the newest mesh even when it alone exceeds the budget", async () => {
    const big = mesh(100);
    const loadMesh = vi.fn(async () => big);
    const cache = createGeometryCache({ loadGeometry: vi.fn(), loadMesh, maxMeshBytes: 10 });

    await cache.mesh("r", 1);
    expect(await cache.mesh("r", 1)).toBe(big);
    expect(loadMesh).toHaveBeenCalledOnce();
  });

  it("drops everything on clear, including a load still in flight", async () => {
    const pending = deferred<RevisionGeometry>();
    const loadGeometry = vi.fn()
      .mockReturnValueOnce(pending.promise)
      .mockResolvedValue(geometry(2));
    const cache = createGeometryCache({ loadGeometry, loadMesh: vi.fn() });

    const inFlight = cache.geometry("a");
    cache.clear();
    pending.resolve(geometry(1));
    expect(await inFlight).toEqual(geometry(1));
    expect(await cache.geometry("a")).toEqual(geometry(2));
    expect(loadGeometry).toHaveBeenCalledTimes(2);
  });
});
