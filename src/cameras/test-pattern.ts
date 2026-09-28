/** A deterministic, dependency-free PNG "test pattern" for web-mode
 *  fixtures (Task 12: "a generated PNG test pattern..., never a photo").
 *  Grayscale, 8-bit, a checkerboard, encoded as a valid PNG with no
 *  compression library: the IDAT stream is zlib-wrapped DEFLATE using only
 *  uncompressed ("stored") blocks, which needs no compression code, just
 *  correct framing -- small, exact, and trivially reproducible. */

const PNG_SIGNATURE = new Uint8Array([137, 80, 78, 71, 13, 10, 26, 10]);
const CRC_TABLE = buildCrcTable();

function buildCrcTable(): Uint32Array {
  const table = new Uint32Array(256);
  for (let n = 0; n < 256; n += 1) {
    let c = n;
    for (let k = 0; k < 8; k += 1) {
      c = (c & 1) ? (0xedb88320 ^ (c >>> 1)) : (c >>> 1);
    }
    table[n] = c >>> 0;
  }
  return table;
}

function crc32(bytes: Uint8Array): number {
  let crc = 0xffffffff;
  for (const byte of bytes) {
    crc = CRC_TABLE[(crc ^ byte) & 0xff] ^ (crc >>> 8);
  }
  return (crc ^ 0xffffffff) >>> 0;
}

function adler32(bytes: Uint8Array): number {
  const MOD = 65521;
  let a = 1;
  let b = 0;
  for (const byte of bytes) {
    a = (a + byte) % MOD;
    b = (b + a) % MOD;
  }
  return ((b << 16) | a) >>> 0;
}

function u32be(value: number): Uint8Array {
  return new Uint8Array([(value >>> 24) & 0xff, (value >>> 16) & 0xff, (value >>> 8) & 0xff, value & 0xff]);
}

function concat(...parts: Uint8Array[]): Uint8Array {
  const out = new Uint8Array(parts.reduce((sum, part) => sum + part.length, 0));
  let offset = 0;
  for (const part of parts) {
    out.set(part, offset);
    offset += part.length;
  }
  return out;
}

function chunk(type: string, data: Uint8Array): Uint8Array {
  const typeBytes = new TextEncoder().encode(type);
  const body = concat(typeBytes, data);
  return concat(u32be(data.length), body, u32be(crc32(body)));
}

/** A zlib stream (RFC 1950) wrapping `raw` in one or more uncompressed
 *  DEFLATE blocks (RFC 1951 §3.2.4), each at most 65535 bytes. */
function storedZlib(raw: Uint8Array): Uint8Array {
  const MAX_BLOCK = 65_535;
  const blocks: Uint8Array[] = [];
  let offset = 0;
  do {
    const end = Math.min(offset + MAX_BLOCK, raw.length);
    const slice = raw.subarray(offset, end);
    const final = end >= raw.length ? 1 : 0;
    const len = slice.length;
    const nlen = (~len) & 0xffff;
    blocks.push(concat(
      new Uint8Array([final]),
      new Uint8Array([len & 0xff, (len >>> 8) & 0xff]),
      new Uint8Array([nlen & 0xff, (nlen >>> 8) & 0xff]),
      slice,
    ));
    offset = end;
  } while (offset < raw.length);
  const header = new Uint8Array([0x78, 0x01]); // CMF/FLG: 32K window, no dictionary, default level
  return concat(header, ...blocks, u32be(adler32(raw)));
}

export interface TestPatternOptions {
  width?: number;
  height?: number;
  /** The checkerboard square size, in pixels. */
  cell?: number;
}

/** Builds a grayscale (color type 0, 8-bit) checkerboard PNG's raw bytes. */
export function generateTestPatternPng(options: TestPatternOptions = {}): Uint8Array {
  const width = options.width ?? 32;
  const height = options.height ?? 32;
  const cell = options.cell ?? 4;
  if (width <= 0 || height <= 0) throw new RangeError("A test pattern needs a positive width and height.");

  const raw = new Uint8Array(height * (1 + width));
  let offset = 0;
  for (let y = 0; y < height; y += 1) {
    raw[offset] = 0; // filter type: none
    offset += 1;
    for (let x = 0; x < width; x += 1) {
      const on = ((Math.floor(x / cell) + Math.floor(y / cell)) % 2) === 0;
      raw[offset] = on ? 235 : 20;
      offset += 1;
    }
  }

  const ihdr = new Uint8Array(13);
  const ihdrView = new DataView(ihdr.buffer);
  ihdrView.setUint32(0, width);
  ihdrView.setUint32(4, height);
  ihdr[8] = 8; // bit depth
  ihdr[9] = 0; // color type: grayscale
  ihdr[10] = 0; // compression method
  ihdr[11] = 0; // filter method
  ihdr[12] = 0; // interlace method

  return concat(
    PNG_SIGNATURE,
    chunk("IHDR", ihdr),
    chunk("IDAT", storedZlib(raw)),
    chunk("IEND", new Uint8Array(0)),
  );
}

function toBase64(bytes: Uint8Array): string {
  let binary = "";
  for (const byte of bytes) binary += String.fromCharCode(byte);
  return btoa(binary);
}

/** `generateTestPatternPng`, as a `data:image/png;base64,...` URL -- what
 *  `web-fixtures.ts` uses directly as an `<img>` `src`, no object URL or
 *  binary-frame decoding required. */
export function testPatternDataUrl(options?: TestPatternOptions): string {
  return `data:image/png;base64,${toBase64(generateTestPatternPng(options))}`;
}
