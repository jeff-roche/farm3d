import { createRoot, createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cameraHealth, cameraSnapshot } from "../attention/test-records";
import type { CameraSnapshot } from "../attention/types";
import type { FrameHeader } from "../generated/contracts/domain/FrameHeader";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const attentionStoreMock = vi.hoisted(() => ({
  health: new Map<string, ReturnType<typeof import("../attention/test-records").cameraHealth>>(),
  snapshotListeners: new Set<(snapshot: CameraSnapshot) => void>(),
  printerRemovedListeners: new Set<(printerId: string) => void>(),
}));
vi.mock("../attention/attention-store", () => ({
  attention: {
    cameraHealth: (printerId: string) => attentionStoreMock.health.get(printerId),
    allCameraHealth: () => Array.from(attentionStoreMock.health.values()),
  },
  onAttentionSnapshotChanged: (listener: (snapshot: CameraSnapshot) => void) => {
    attentionStoreMock.snapshotListeners.add(listener);
    return () => attentionStoreMock.snapshotListeners.delete(listener);
  },
  onAttentionPrinterRemoved: (listener: (printerId: string) => void) => {
    attentionStoreMock.printerRemovedListeners.add(listener);
    return () => attentionStoreMock.printerRemovedListeners.delete(listener);
  },
}));

function header(overrides: Partial<FrameHeader> = {}): FrameHeader {
  return { contentType: "image/png", capturedAt: "2026-09-25T00:00:00Z", byteLen: 3, snapshotId: null, ...overrides };
}

function encodeFrame(headerValue: FrameHeader, image: Uint8Array): ArrayBuffer {
  const json = new TextEncoder().encode(JSON.stringify(headerValue));
  const out = new Uint8Array(4 + json.length + image.length);
  new DataView(out.buffer).setUint32(0, json.length, false);
  out.set(json, 4);
  out.set(image, 4 + json.length);
  return out.buffer;
}

function commandError(code: string, message: string) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  attentionStoreMock.health.clear();
  attentionStoreMock.snapshotListeners.clear();
  attentionStoreMock.printerRemovedListeners.clear();
});

afterEach(() => {
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("camera health (read-through)", () => {
  it("reads from attention-store, never its own state", async () => {
    attentionStoreMock.health.set("prn-1", cameraHealth({ printerId: "prn-1", state: "ok" }));
    const { camera } = await import("./camera-store");
    expect(camera.health("prn-1")).toEqual(cameraHealth({ printerId: "prn-1", state: "ok" }));
    expect(camera.health("prn-2")).toBeUndefined();
    expect(camera.allHealth()).toHaveLength(1);
  });
});

describe("snapshots (desktop)", () => {
  beforeEach(() => tauriMock.isTauri.mockReturnValue(true));

  it("listSnapshots calls list_snapshots and caches the returned rows", async () => {
    const snapshot = cameraSnapshot({ id: "snp-1" });
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { snapshots: [snapshot], nextCursor: null } });
    const { listSnapshots, camera } = await import("./camera-store");

    const page = await listSnapshots({ printerId: "prn-1" });

    expect(page.snapshots).toEqual([snapshot]);
    expect(tauriMock.invoke).toHaveBeenCalledWith("list_snapshots", { contractVersion: 1, printerId: "prn-1" });
    expect(camera.snapshot("snp-1")).toEqual(snapshot);
  });

  it("captureSnapshot sends a fresh operationId and caches the result", async () => {
    const captured = cameraSnapshot({ id: "snp-new", trigger: "manual" });
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: captured });
    const { captureSnapshot, camera } = await import("./camera-store");

    await expect(captureSnapshot("prn-1")).resolves.toEqual(captured);
    expect(tauriMock.invoke).toHaveBeenCalledWith(
      "capture_snapshot",
      expect.objectContaining({ printerId: "prn-1", operationId: expect.any(String) }),
    );
    expect(camera.snapshot("snp-new")).toEqual(captured);
  });

  it("setSnapshotPinned updates the cache from the returned row", async () => {
    const pinned = cameraSnapshot({ id: "snp-1", pinnedAt: "2026-09-25T00:00:00Z", revision: 2 });
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: pinned });
    const { setSnapshotPinned, camera } = await import("./camera-store");

    await setSnapshotPinned("snp-1", true);
    expect(camera.snapshot("snp-1")?.pinnedAt).toBe("2026-09-25T00:00:00Z");
  });

  it("a lower-revision row never overwrites a newer cached one", async () => {
    const newer = cameraSnapshot({ id: "snp-1", revision: 3 });
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { snapshots: [newer], nextCursor: null } });
    const { listSnapshots, camera } = await import("./camera-store");
    await listSnapshots();
    expect(camera.snapshot("snp-1")?.revision).toBe(3);

    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { snapshots: [cameraSnapshot({ id: "snp-1", revision: 1 })], nextCursor: null } });
    await listSnapshots();
    expect(camera.snapshot("snp-1")?.revision).toBe(3);
  });

  it("mediaUsage calls media_usage", async () => {
    tauriMock.invoke.mockResolvedValue({
      contractVersion: 1,
      data: { usedBytes: 1, pinnedBytes: 0, capBytes: 10, retentionDays: 30, snapshotCount: 1, pinnedCount: 0, prunedCount: 0 },
    });
    const { mediaUsage } = await import("./camera-store");
    await expect(mediaUsage()).resolves.toMatchObject({ snapshotCount: 1 });
    expect(tauriMock.invoke).toHaveBeenCalledWith("media_usage", { contractVersion: 1 });
  });

  it("snapshotImageUrl decodes a binary frame into an object URL", async () => {
    tauriMock.invoke.mockResolvedValue(encodeFrame(header(), new Uint8Array([1, 2, 3])));
    const { snapshotImageUrl } = await import("./camera-store");
    await expect(snapshotImageUrl("snp-1")).resolves.toMatch(/^blob:/);
    expect(tauriMock.invoke).toHaveBeenCalledWith("snapshot_image", { contractVersion: 1, snapshotId: "snp-1" });
  });

  it("onAttentionSnapshotChanged updates the local cache without a command call", async () => {
    const { camera } = await import("./camera-store");
    const snapshot = cameraSnapshot({ id: "snp-live" });
    for (const listener of attentionStoreMock.snapshotListeners) listener(snapshot);
    expect(camera.snapshot("snp-live")).toEqual(snapshot);
  });

  it("drops a removed Printer's cached snapshots", async () => {
    tauriMock.invoke.mockResolvedValue({
      contractVersion: 1,
      data: { snapshots: [cameraSnapshot({ id: "snp-1", printerId: "prn-1" }), cameraSnapshot({ id: "snp-2", printerId: "prn-2" })], nextCursor: null },
    });
    const { listSnapshots, camera } = await import("./camera-store");
    await listSnapshots();
    expect(camera.snapshot("snp-1")).toBeDefined();

    for (const listener of attentionStoreMock.printerRemovedListeners) listener("prn-1");
    expect(camera.snapshot("snp-1")).toBeUndefined();
    expect(camera.snapshot("snp-2")).toBeDefined();
  });

  it("testCamera decodes the binary frame", async () => {
    tauriMock.invoke.mockResolvedValue(encodeFrame(header(), new Uint8Array([9])));
    const { testCamera } = await import("./camera-store");
    const frame = await testCamera({ printerId: "prn-1", source: { kind: "hostWebcam", webcamName: "cam0", webcamService: null, webPort: null } });
    expect(Array.from(frame.image)).toEqual([9]);
  });

  it("listHostWebcams calls list_host_webcams", async () => {
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: [{ name: "cam0", service: "webrtc" }] });
    const { listHostWebcams } = await import("./camera-store");
    await expect(listHostWebcams({ printerId: "prn-1" })).resolves.toEqual([{ name: "cam0", service: "webrtc" }]);
  });
});

describe("snapshots (web)", () => {
  beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

  it("listSnapshots and mediaUsage are served from web-fixtures.ts", async () => {
    const { listSnapshots, mediaUsage } = await import("./camera-store");
    const { webSnapshotPage, webMediaUsage } = await import("../attention/web-fixtures");
    await expect(listSnapshots()).resolves.toEqual(webSnapshotPage());
    await expect(mediaUsage()).resolves.toEqual(webMediaUsage());
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("snapshotImageUrl resolves the fixture's evidence snapshot to its data URL", async () => {
    const { snapshotImageUrl } = await import("./camera-store");
    const { WEB_INCIDENT_SNAPSHOT_ID } = await import("../attention/web-fixtures");
    await expect(snapshotImageUrl(WEB_INCIDENT_SNAPSHOT_ID)).resolves.toMatch(/^data:image\/png;base64,/);
  });

  it("snapshotImageUrl rejects EVIDENCE_PRUNED for the pruned fixture row", async () => {
    const { snapshotImageUrl } = await import("./camera-store");
    const { WEB_PRUNED_SNAPSHOT_ID } = await import("../attention/web-fixtures");
    await expect(snapshotImageUrl(WEB_PRUNED_SNAPSHOT_ID)).rejects.toMatchObject({ code: "EVIDENCE_PRUNED" });
  });

  it("snapshotImageUrl rejects NOT_FOUND for an unknown id", async () => {
    const { snapshotImageUrl } = await import("./camera-store");
    await expect(snapshotImageUrl("snp-nope")).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("captureSnapshot/setSnapshotPinned refuse with needsDesktopError", async () => {
    const { captureSnapshot, setSnapshotPinned } = await import("./camera-store");
    await expect(captureSnapshot("prn-1")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(setSnapshotPinned("snp-1", true)).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
  });

  it("testCamera/listHostWebcams refuse with needsDesktopError", async () => {
    const { testCamera, listHostWebcams } = await import("./camera-store");
    await expect(testCamera({ source: { kind: "snapshotUrl", snapshotUrl: "http://192.0.2.1/snap.jpg" } }))
      .rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(listHostWebcams({ printerId: "prn-1" })).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
  });
});

describe("usePreview", () => {
  it("polls camera_preview_frame about once a second while visible, and revokes the previous URL each frame", async () => {
    vi.useFakeTimers();
    tauriMock.isTauri.mockReturnValue(true);
    let frameCount = 0;
    tauriMock.invoke.mockImplementation(() => {
      frameCount += 1;
      return Promise.resolve(encodeFrame(header({ capturedAt: `frame-${frameCount}` }), new Uint8Array([frameCount])));
    });
    const revokeSpy = vi.spyOn(URL, "revokeObjectURL");
    const { usePreview } = await import("./camera-store");

    const [visible, setVisible] = createSignal(true);
    let preview!: ReturnType<typeof usePreview>;
    const dispose = createRoot((d) => {
      preview = usePreview(() => "prn-1", visible);
      return d;
    });

    await vi.advanceTimersByTimeAsync(0);
    expect(preview.url()).toMatch(/^blob:/);
    expect(preview.capturedAt()).toBe("frame-1");

    await vi.advanceTimersByTimeAsync(1_000);
    expect(preview.capturedAt()).toBe("frame-2");
    expect(revokeSpy).toHaveBeenCalled();

    setVisible(false);
    await vi.advanceTimersByTimeAsync(0);
    const countAtStop = frameCount;
    await vi.advanceTimersByTimeAsync(5_000);
    expect(frameCount).toBe(countAtStop);
    expect(preview.url()).toBeNull();

    dispose();
  });

  it("a slow poll never overlaps the next: each starts a second after the previous settles", async () => {
    vi.useFakeTimers();
    tauriMock.isTauri.mockReturnValue(true);
    const started: number[] = [];
    let inFlight = 0;
    let maxInFlight = 0;
    tauriMock.invoke.mockImplementation(() => {
      started.push(Date.now());
      inFlight += 1;
      maxInFlight = Math.max(maxInFlight, inFlight);
      // The camera takes three seconds per frame.
      return new Promise((resolve) => setTimeout(() => {
        inFlight -= 1;
        resolve(encodeFrame(header({ capturedAt: `frame-${started.length}` }), new Uint8Array([1])));
      }, 3_000));
    });
    const { usePreview } = await import("./camera-store");

    const t0 = Date.now();
    const dispose = createRoot((d) => {
      usePreview(() => "prn-1", () => true);
      return d;
    });
    await vi.advanceTimersByTimeAsync(10_000);

    expect(maxInFlight).toBe(1);
    // 0 → settles at 3 s → next at 4 s → settles at 7 s → next at 8 s.
    expect(started.map((at) => at - t0)).toEqual([0, 4_000, 8_000]);
    dispose();
    await vi.advanceTimersByTimeAsync(10_000);
    expect(started).toHaveLength(3);
  });

  it("reports needsDesktopError in web mode and never polls", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { usePreview } = await import("./camera-store");

    let preview!: ReturnType<typeof usePreview>;
    const dispose = createRoot((d) => {
      preview = usePreview(() => "prn-1", () => true);
      return d;
    });
    await flush();

    expect(preview.error()).toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    expect(preview.url()).toBeNull();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
    dispose();
  });

  it("surfaces a fetch failure as CommandError through error()", async () => {
    vi.useFakeTimers();
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockRejectedValue(commandError("CAMERA_FAILED", "timed out"));
    const { usePreview } = await import("./camera-store");

    let preview!: ReturnType<typeof usePreview>;
    const dispose = createRoot((d) => {
      preview = usePreview(() => "prn-1", () => true);
      return d;
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(preview.error()).toMatchObject({ code: "CAMERA_FAILED" });
    dispose();
  });

  it("switching printerId resets state and restarts the poll for the new one", async () => {
    vi.useFakeTimers();
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockImplementation((_name, args: { printerId: string }) =>
      Promise.resolve(encodeFrame(header({ capturedAt: args.printerId }), new Uint8Array([1]))));
    const { usePreview } = await import("./camera-store");

    const [printerId, setPrinterId] = createSignal("prn-1");
    let preview!: ReturnType<typeof usePreview>;
    const dispose = createRoot((d) => {
      // createEffect re-runs `usePreview`'s own effect when `printerId()` changes.
      preview = usePreview(printerId, () => true);
      return d;
    });
    await vi.advanceTimersByTimeAsync(0);
    expect(preview.capturedAt()).toBe("prn-1");

    setPrinterId("prn-2");
    await vi.advanceTimersByTimeAsync(0);
    expect(preview.capturedAt()).toBe("prn-2");
    dispose();
  });
});
