import { describe, expect, it } from "vitest";
import { decodeMeshBuffer, encodeMeshBuffer, MESH_HEADER_BYTES, MeshBufferError } from "./mesh-buffer";

/** Writes a buffer byte by byte, exactly as D6 lays it out, so the decoder
 *  is checked against the layout rather than against its own encoder. */
function handWritten(positions: number[], indices: number[], overrides: { magic?: string; version?: number; vertexCount?: number; indexCount?: number } = {}): ArrayBuffer {
  const buffer = new ArrayBuffer(16 + positions.length * 4 + indices.length * 4);
  const view = new DataView(buffer);
  const magic = overrides.magic ?? "F3DM";
  for (let i = 0; i < 4; i += 1) view.setUint8(i, magic.charCodeAt(i));
  view.setUint32(4, overrides.version ?? 1, true);
  view.setUint32(8, overrides.vertexCount ?? positions.length / 3, true);
  view.setUint32(12, overrides.indexCount ?? indices.length, true);
  positions.forEach((value, i) => view.setFloat32(16 + i * 4, value, true));
  indices.forEach((value, i) => view.setUint32(16 + positions.length * 4 + i * 4, value, true));
  return buffer;
}

const TRIANGLE = [0, 0, 0, 10, 0, 0, 0, 5.5, 2.25];

describe("decodeMeshBuffer", () => {
  it("reads D6's little-endian header, positions and indices", () => {
    const mesh = decodeMeshBuffer(handWritten(TRIANGLE, [0, 1, 2]));
    expect(mesh.vertexCount).toBe(3);
    expect(mesh.indexCount).toBe(3);
    expect(mesh.triangleCount).toBe(1);
    expect([...mesh.positions]).toEqual(TRIANGLE);
    expect([...mesh.indices]).toEqual([0, 1, 2]);
    expect(mesh.positions).toBeInstanceOf(Float32Array);
    expect(mesh.indices).toBeInstanceOf(Uint32Array);
  });

  it("decodes an empty mesh", () => {
    const mesh = decodeMeshBuffer(handWritten([], []));
    expect(mesh).toMatchObject({ vertexCount: 0, indexCount: 0, triangleCount: 0 });
    expect(mesh.positions).toHaveLength(0);
  });

  it("round-trips through encodeMeshBuffer byte for byte", () => {
    const positions = new Float32Array([...TRIANGLE, 1, 1, 1]);
    const indices = new Uint32Array([0, 1, 2, 1, 3, 2]);
    const encoded = encodeMeshBuffer(positions, indices);
    expect(new Uint8Array(encoded)).toEqual(new Uint8Array(handWritten([...positions], [...indices])));
    const decoded = decodeMeshBuffer(encoded);
    expect(decoded.positions).toEqual(positions);
    expect(decoded.indices).toEqual(indices);
  });

  it("refuses a buffer that is not an F3DM version 1 mesh", () => {
    expect(() => decodeMeshBuffer(new ArrayBuffer(MESH_HEADER_BYTES - 1))).toThrow(MeshBufferError);
    expect(() => decodeMeshBuffer(handWritten(TRIANGLE, [0, 1, 2], { magic: "F3DX" }))).toThrow(/magic/);
    expect(() => decodeMeshBuffer(handWritten(TRIANGLE, [0, 1, 2], { version: 2 }))).toThrow(/version 2/);
  });

  it("refuses counts that disagree with the buffer's length", () => {
    expect(() => decodeMeshBuffer(handWritten(TRIANGLE, [0, 1, 2], { vertexCount: 4 }))).toThrow(MeshBufferError);
    expect(() => decodeMeshBuffer(handWritten(TRIANGLE, [0, 1, 2], { indexCount: 2 }))).toThrow(MeshBufferError);
  });

  it("refuses an index count that is not whole triangles, or an index past the last vertex", () => {
    expect(() => decodeMeshBuffer(handWritten(TRIANGLE, [0, 1]))).toThrow(/triangles/);
    expect(() => decodeMeshBuffer(handWritten(TRIANGLE, [0, 1, 3]))).toThrow(/index 3/);
  });
});
