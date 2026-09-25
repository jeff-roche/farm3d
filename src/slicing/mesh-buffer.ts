/** D6's binary mesh transfer: what `get_revision_mesh` answers with, for
 *  one object in its own frame. The layout is little-endian:
 *
 *    bytes 0..4    the magic `F3DM`
 *    bytes 4..8    u32 format version (1)
 *    bytes 8..12   u32 vertex count
 *    bytes 12..16  u32 index count
 *    then          f32 x, y, z per vertex
 *    then          u32 indices, three per triangle
 *
 *  There are no per-object ranges: each buffer is exactly one object. */

export const MESH_MAGIC = "F3DM";
export const MESH_FORMAT_VERSION = 1;
export const MESH_HEADER_BYTES = 16;

export interface MeshBuffer {
  vertexCount: number;
  indexCount: number;
  triangleCount: number;
  /** x, y, z per vertex, in millimetres. */
  positions: Float32Array;
  /** Three vertex indices per triangle. */
  indices: Uint32Array;
}

/** A buffer that is not a well-formed D6 mesh. */
export class MeshBufferError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "MeshBufferError";
  }
}

const LITTLE_ENDIAN_HOST = new Uint8Array(new Uint32Array([1]).buffer)[0] === 1;

/** Decodes and checks one D6 mesh. On a little-endian host (every platform
 *  farm3d ships on) the arrays are views over `buffer`, not copies. */
export function decodeMeshBuffer(buffer: ArrayBuffer): MeshBuffer {
  if (buffer.byteLength < MESH_HEADER_BYTES) {
    throw new MeshBufferError(`A mesh buffer is at least ${MESH_HEADER_BYTES} bytes; this one is ${buffer.byteLength}.`);
  }
  const view = new DataView(buffer);
  const magic = String.fromCharCode(view.getUint8(0), view.getUint8(1), view.getUint8(2), view.getUint8(3));
  if (magic !== MESH_MAGIC) throw new MeshBufferError("The mesh buffer's magic is not F3DM.");
  const version = view.getUint32(4, true);
  if (version !== MESH_FORMAT_VERSION) throw new MeshBufferError(`Mesh format version ${version} is not supported.`);
  const vertexCount = view.getUint32(8, true);
  const indexCount = view.getUint32(12, true);
  const positionsOffset = MESH_HEADER_BYTES;
  const indicesOffset = positionsOffset + vertexCount * 12;
  const expectedBytes = indicesOffset + indexCount * 4;
  if (buffer.byteLength !== expectedBytes) {
    throw new MeshBufferError(
      `A mesh of ${vertexCount} vertices and ${indexCount} indices is ${expectedBytes} bytes; this one is ${buffer.byteLength}.`,
    );
  }
  if (indexCount % 3 !== 0) throw new MeshBufferError(`${indexCount} indices are not whole triangles.`);

  let positions: Float32Array;
  let indices: Uint32Array;
  if (LITTLE_ENDIAN_HOST) {
    positions = new Float32Array(buffer, positionsOffset, vertexCount * 3);
    indices = new Uint32Array(buffer, indicesOffset, indexCount);
  } else {
    positions = new Float32Array(vertexCount * 3);
    for (let i = 0; i < positions.length; i += 1) positions[i] = view.getFloat32(positionsOffset + i * 4, true);
    indices = new Uint32Array(indexCount);
    for (let i = 0; i < indices.length; i += 1) indices[i] = view.getUint32(indicesOffset + i * 4, true);
  }
  for (let i = 0; i < indices.length; i += 1) {
    if (indices[i] >= vertexCount) {
      throw new MeshBufferError(`Mesh index ${indices[i]} is past the last of ${vertexCount} vertices.`);
    }
  }
  return { vertexCount, indexCount, triangleCount: indexCount / 3, positions, indices };
}

/** The inverse of `decodeMeshBuffer`, as the backend's `encode_mesh`
 *  writes it. Only the web fixtures and tests build buffers. */
export function encodeMeshBuffer(positions: Float32Array, indices: Uint32Array): ArrayBuffer {
  const vertexCount = positions.length / 3;
  const buffer = new ArrayBuffer(MESH_HEADER_BYTES + positions.length * 4 + indices.length * 4);
  const view = new DataView(buffer);
  for (let i = 0; i < 4; i += 1) view.setUint8(i, MESH_MAGIC.charCodeAt(i));
  view.setUint32(4, MESH_FORMAT_VERSION, true);
  view.setUint32(8, vertexCount, true);
  view.setUint32(12, indices.length, true);
  let offset = MESH_HEADER_BYTES;
  for (const value of positions) {
    view.setFloat32(offset, value, true);
    offset += 4;
  }
  for (const value of indices) {
    view.setUint32(offset, value, true);
    offset += 4;
  }
  return buffer;
}
