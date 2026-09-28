import { describe, expect, it } from "vitest";
import { CameraFrameError, decodeFrame, frameObjectUrl, MAX_FRAME_HEADER_BYTES } from "./frame";
import type { FrameHeader } from "../generated/contracts/domain/FrameHeader";

function header(overrides: Partial<FrameHeader> = {}): FrameHeader {
  return { contentType: "image/png", capturedAt: "2026-09-25T00:00:00Z", byteLen: 3, snapshotId: null, ...overrides };
}

/** Builds a buffer exactly as `src-tauri/src/cameras/fetch.rs`'s
 *  `encode_frame` does: a big-endian u32 header length, the header JSON,
 *  then the image bytes. */
function encodeFrame(headerValue: FrameHeader, image: Uint8Array): ArrayBuffer {
  const json = new TextEncoder().encode(JSON.stringify(headerValue));
  const out = new Uint8Array(4 + json.length + image.length);
  const view = new DataView(out.buffer);
  view.setUint32(0, json.length, false);
  out.set(json, 4);
  out.set(image, 4 + json.length);
  return out.buffer;
}

describe("decodeFrame", () => {
  it("decodes a well-formed frame's header and image", () => {
    const image = new Uint8Array([1, 2, 3]);
    const result = decodeFrame(encodeFrame(header(), image));
    expect(result.header).toEqual(header());
    expect(Array.from(result.image)).toEqual([1, 2, 3]);
  });

  it("decodes an empty image (never in practice, but well-formed)", () => {
    const result = decodeFrame(encodeFrame(header({ byteLen: 0 }), new Uint8Array()));
    expect(result.image.length).toBe(0);
  });

  it("carries a snapshotId when set (snapshot_image only)", () => {
    const result = decodeFrame(encodeFrame(header({ snapshotId: "snp-1" }), new Uint8Array([9])));
    expect(result.header.snapshotId).toBe("snp-1");
  });

  it("rejects a buffer shorter than the length prefix", () => {
    expect(() => decodeFrame(new Uint8Array([0, 0]).buffer)).toThrow(CameraFrameError);
  });

  it("rejects a header length over the 4096-byte cap", () => {
    const out = new Uint8Array(4);
    new DataView(out.buffer).setUint32(0, MAX_FRAME_HEADER_BYTES + 1, false);
    expect(() => decodeFrame(out.buffer)).toThrow(CameraFrameError);
  });

  it("rejects a buffer truncated before the claimed header ends", () => {
    const out = new Uint8Array(4 + 3);
    new DataView(out.buffer).setUint32(0, 10, false); // claims 10 header bytes, only 3 follow
    expect(() => decodeFrame(out.buffer)).toThrow(CameraFrameError);
  });

  it("rejects a header that isn't valid JSON", () => {
    const json = new TextEncoder().encode("not json");
    const out = new Uint8Array(4 + json.length);
    new DataView(out.buffer).setUint32(0, json.length, false);
    out.set(json, 4);
    expect(() => decodeFrame(out.buffer)).toThrow(CameraFrameError);
  });
});

describe("frameObjectUrl", () => {
  it("creates an object URL typed by the header's contentType", () => {
    const frame = decodeFrame(encodeFrame(header({ contentType: "image/jpeg" }), new Uint8Array([1, 2, 3])));
    const url = frameObjectUrl(frame);
    expect(url.startsWith("blob:")).toBe(true);
  });
});
