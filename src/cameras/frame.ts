/** D7 "Binary frame": what `test_camera`, `camera_preview_frame`, and
 *  `snapshot_image` answer with (`tauri::ipc::Response`), mirrors
 *  `src-tauri/src/cameras/fetch.rs`'s `encode_frame`:
 *
 *    bytes 0..4     the header length H, u32 big-endian, at most 4096
 *    bytes 4..4+H   a UTF-8 JSON `FrameHeader`
 *    the rest       the image bytes
 *
 *  Mirrors `src/slicing/mesh-buffer.ts`'s decode/error shape for D6's own
 *  binary transfer. */
import type { FrameHeader } from "../generated/contracts/domain/FrameHeader";

export const MAX_FRAME_HEADER_BYTES = 4_096;

export interface CameraFrame {
  header: FrameHeader;
  /** The image bytes, as a view over the original buffer (no copy). */
  image: Uint8Array;
}

/** A buffer that is not a well-formed D7 frame. */
export class CameraFrameError extends Error {
  constructor(message: string) {
    super(message);
    this.name = "CameraFrameError";
  }
}

export function decodeFrame(buffer: ArrayBuffer): CameraFrame {
  if (buffer.byteLength < 4) {
    throw new CameraFrameError(`A camera frame is at least 4 bytes; this one is ${buffer.byteLength}.`);
  }
  const view = new DataView(buffer);
  const headerLength = view.getUint32(0, false);
  if (headerLength > MAX_FRAME_HEADER_BYTES) {
    throw new CameraFrameError(`A frame header is at most ${MAX_FRAME_HEADER_BYTES} bytes; this one claims ${headerLength}.`);
  }
  if (buffer.byteLength < 4 + headerLength) {
    throw new CameraFrameError(`A frame's header claims ${headerLength} bytes, but only ${buffer.byteLength - 4} follow.`);
  }
  const headerBytes = new Uint8Array(buffer, 4, headerLength);
  let header: FrameHeader;
  try {
    header = JSON.parse(new TextDecoder().decode(headerBytes)) as FrameHeader;
  } catch {
    throw new CameraFrameError("A camera frame's header is not valid JSON.");
  }
  const image = new Uint8Array(buffer, 4 + headerLength);
  return { header, image };
}

/** A fresh object URL for `frame`'s image bytes, typed by its header's
 *  `contentType`. The caller owns its lifetime: revoke it with
 *  `URL.revokeObjectURL` once it's no longer shown (spec "usePreview...
 *  revokes the previous object URL on each frame and on stop"). */
export function frameObjectUrl(frame: CameraFrame): string {
  const blob = new Blob([frame.image], { type: frame.header.contentType });
  return URL.createObjectURL(blob);
}
