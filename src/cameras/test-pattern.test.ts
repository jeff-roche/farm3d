import { describe, expect, it } from "vitest";
import { generateTestPatternPng, testPatternDataUrl } from "./test-pattern";

const SIGNATURE = [137, 80, 78, 71, 13, 10, 26, 10];

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc ^= byte;
    for (let i = 0; i < 8; i += 1) {
      crc = (crc & 1) ? (0xedb88320 ^ (crc >>> 1)) : (crc >>> 1);
    }
  }
  return (crc ^ 0xffffffff) >>> 0;
}

/** A from-scratch, independent PNG chunk walker -- deliberately not reusing
 *  anything from `test-pattern.ts`, so a bug shared between "build" and
 *  "verify" wouldn't hide behind a green test. */
function readChunks(bytes: Uint8Array): { type: string; data: Uint8Array }[] {
  const chunks: { type: string; data: Uint8Array }[] = [];
  let offset = 8; // past the signature
  const view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
  while (offset < bytes.length) {
    const length = view.getUint32(offset);
    const type = new TextDecoder().decode(bytes.subarray(offset + 4, offset + 8));
    const data = bytes.subarray(offset + 8, offset + 8 + length);
    const storedCrc = view.getUint32(offset + 8 + length);
    const actualCrc = crc32(bytes.subarray(offset + 4, offset + 8 + length));
    expect(actualCrc).toBe(storedCrc);
    chunks.push({ type, data });
    offset += 12 + length;
  }
  return chunks;
}

describe("generateTestPatternPng", () => {
  it("starts with the PNG signature", () => {
    const bytes = generateTestPatternPng();
    expect(Array.from(bytes.subarray(0, 8))).toEqual(SIGNATURE);
  });

  it("has a well-formed IHDR with the requested dimensions, then IDAT, then IEND", () => {
    const bytes = generateTestPatternPng({ width: 16, height: 8 });
    const chunks = readChunks(bytes);
    expect(chunks.map((c) => c.type)).toEqual(["IHDR", "IDAT", "IEND"]);

    const ihdr = chunks[0].data;
    const view = new DataView(ihdr.buffer, ihdr.byteOffset, ihdr.byteLength);
    expect(view.getUint32(0)).toBe(16); // width
    expect(view.getUint32(4)).toBe(8); // height
    expect(ihdr[8]).toBe(8); // bit depth
    expect(ihdr[9]).toBe(0); // color type: grayscale

    expect(chunks[2].data.length).toBe(0); // IEND carries no data
  });

  it("differs for different dimensions (never a fixed, meaningless blob)", () => {
    const small = generateTestPatternPng({ width: 8, height: 8 });
    const large = generateTestPatternPng({ width: 32, height: 32 });
    expect(small).not.toEqual(large);
  });

  it("is deterministic for the same inputs", () => {
    const first = generateTestPatternPng({ width: 20, height: 12, cell: 3 });
    const second = generateTestPatternPng({ width: 20, height: 12, cell: 3 });
    expect(first).toEqual(second);
  });

  it("rejects a non-positive size", () => {
    expect(() => generateTestPatternPng({ width: 0, height: 8 })).toThrow(RangeError);
  });
});

describe("testPatternDataUrl", () => {
  it("is a base64 PNG data URL round-tripping to the same bytes", () => {
    const url = testPatternDataUrl({ width: 8, height: 8 });
    expect(url.startsWith("data:image/png;base64,")).toBe(true);
    const decoded = Uint8Array.from(atob(url.slice("data:image/png;base64,".length)), (c) => c.charCodeAt(0));
    expect(Array.from(decoded.subarray(0, 8))).toEqual(SIGNATURE);
    expect(decoded).toEqual(generateTestPatternPng({ width: 8, height: 8 }));
  });
});
