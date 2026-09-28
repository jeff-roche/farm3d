import { cleanup, fireEvent, render, screen, waitFor } from "@solidjs/testing-library";
import { createSignal } from "solid-js";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { cameraHealth, cameraSnapshot } from "../attention/test-records";
import type { CameraHealth, CameraSnapshot } from "../attention/types";
import type { ResolvedPrinter } from "../printers/types";
import type { FrameHeader } from "../generated/contracts/domain/FrameHeader";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const attentionStoreMock = vi.hoisted(() => ({
  health: new Map<string, CameraHealth>(),
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

// A static, single import: `camera-store.ts`'s snapshot cache is a
// module-level singleton that a per-test `vi.resetModules()` would
// otherwise clear, but every test below gives its Printer a unique id, so
// `camera.snapshotsForPrinter(id)` never sees another test's rows even
// though the underlying cache keeps accumulating across the whole file.
import { PrinterCameraPanel } from "./PrinterCameraPanel";

function header(overrides: Partial<FrameHeader> = {}): FrameHeader {
  return { contentType: "image/png", capturedAt: "2026-09-25T12:00:03Z", byteLen: 3, snapshotId: null, ...overrides };
}

function encodeFrame(headerValue: FrameHeader, image: Uint8Array): ArrayBuffer {
  const json = new TextEncoder().encode(JSON.stringify(headerValue));
  const out = new Uint8Array(4 + json.length + image.length);
  new DataView(out.buffer).setUint32(0, json.length, false);
  out.set(json, 4);
  out.set(image, 4 + json.length);
  return out.buffer;
}

// Each test must give `printer()` a distinct `id`: `camera-store.ts`'s
// snapshot cache is a module-level singleton this file never resets (see
// the import comment above), so two tests sharing a printer id would see
// each other's cached rows.
function printer(overrides: Partial<ResolvedPrinter> = {}): ResolvedPrinter {
  return {
    id: "prn-1", revision: 1, name: "North Bay", notes: "", overrides: {},
    catalogRef: { vendor: "Bambu Lab", model: "X1 Carbon", variant: "X1 Carbon 0.4", modelId: "x1", printerVariant: "0.4" },
    catalogStatus: "ok", modelLabel: "X1 Carbon", variantLabel: "X1 Carbon 0.4", overriddenFields: [], inherited: {},
    profileDrift: [], unknownOverrideKeys: [], startSafety: "confirmBedClear", materialSlots: [], setupGaps: [], createdAt: "", updatedAt: "",
    profile: { bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 }, printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "", nozzleDiameterMm: [0.4], nozzleType: "brass", gcodeFlavor: "klipper", hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null },
    ...overrides,
  };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

beforeEach(() => {
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  tauriMock.isTauri.mockReturnValue(true);
  attentionStoreMock.health.clear();
  attentionStoreMock.snapshotListeners.clear();
  attentionStoreMock.printerRemovedListeners.clear();
});

afterEach(() => {
  cleanup();
  vi.useRealTimers();
  vi.restoreAllMocks();
});

describe("PrinterCameraPanel", () => {
  it("no camera source: an explanation plus 'Set up camera', which calls onOpenSetup", async () => {
    const onOpenSetup = vi.fn();
    render(() => <PrinterCameraPanel printer={printer({ id: "prn-no-camera" })} visible={() => true} onOpenSetup={onOpenSetup} />);

    expect(screen.getByText(/No camera/)).toBeInTheDocument();
    await fireEvent.click(screen.getByRole("button", { name: "Set up camera" }));
    expect(onOpenSetup).toHaveBeenCalled();
  });

  it("unsupported adapter: shows the reason, no preview/history", async () => {
    attentionStoreMock.health.set("prn-unsupported", cameraHealth({ printerId: "prn-unsupported", state: "unsupported" }));
    render(() => <PrinterCameraPanel printer={printer({ id: "prn-unsupported" })} visible={() => true} onOpenSetup={vi.fn()} />);

    expect(screen.getByText("Not supported by this Connection")).toBeInTheDocument();
    expect(screen.queryByRole("button", { name: "Capture" })).not.toBeInTheDocument();
  });

  describe("configured", () => {
    it("polls the preview only while visible, pauses when hidden, and shows health text with the last-frame time", async () => {
      const id = "prn-preview";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      vi.useFakeTimers();
      let frameCount = 0;
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") {
          frameCount += 1;
          return Promise.resolve(encodeFrame(header({ capturedAt: `2026-09-25T12:00:0${frameCount}Z` }), new Uint8Array([frameCount])));
        }
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [], nextCursor: null } });
        if (name === "media_usage") {
          return Promise.resolve({ contractVersion: 1, data: { usedBytes: 0, pinnedBytes: 0, capBytes: 1, retentionDays: 30, snapshotCount: 0, pinnedCount: 0, prunedCount: 0 } });
        }
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      const [visible, setVisible] = createSignal(true);
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={visible} onOpenSetup={vi.fn()} />);

      await vi.advanceTimersByTimeAsync(0);
      expect(screen.getByRole("status")).toHaveTextContent("Live");
      expect(screen.getByText(/Last frame/)).toBeInTheDocument();
      const countWhileVisible = frameCount;
      expect(countWhileVisible).toBeGreaterThan(0);

      setVisible(false);
      await vi.advanceTimersByTimeAsync(0);
      const countAtHidden = frameCount;
      await vi.advanceTimersByTimeAsync(5_000);
      expect(frameCount).toBe(countAtHidden);
      expect(screen.getByText("No frame captured yet.")).toBeInTheDocument();
    });

    it("reduced motion: swapping preview frames never adds a transition/fade class or inline style", async () => {
      const id = "prn-no-fade";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      vi.useFakeTimers();
      let frameCount = 0;
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") {
          frameCount += 1;
          return Promise.resolve(encodeFrame(header({ capturedAt: `frame-${frameCount}` }), new Uint8Array([frameCount])));
        }
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [], nextCursor: null } });
        if (name === "media_usage") return Promise.resolve({ contractVersion: 1, data: { usedBytes: 0, pinnedBytes: 0, capBytes: 1, retentionDays: 30, snapshotCount: 0, pinnedCount: 0, prunedCount: 0 } });
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={() => true} onOpenSetup={vi.fn()} />);

      await vi.advanceTimersByTimeAsync(0);
      const first = await screen.findByAltText(`Live camera preview of ${printer({ id }).name}`);
      const classBefore = first.className;
      const styleBefore = first.getAttribute("style");

      await vi.advanceTimersByTimeAsync(1_000);
      const second = await screen.findByAltText(`Live camera preview of ${printer({ id }).name}`);
      // A fresh `<img>` each frame (`src` swap, spec: "an object URL per
      // frame"), with the same static class and no inline `style` either
      // time -- nothing here could fade, whatever `prefers-reduced-motion`
      // says (spec "Accessibility and adaptation").
      expect(second.className).toBe(classBefore);
      expect(second.getAttribute("style")).toBe(styleBefore);
      expect(styleBefore).toBeNull();
    });

    it("shows the failing reason alongside the state text, when the live poll itself isn't currently erroring", async () => {
      // Rust's health row can lag or outlast a single poll cycle (it comes
      // from `camera.health.changed`, a separate async stream) -- this
      // covers that fallback path specifically, with a *successful* current
      // frame, distinct from the "preview error takes over" tests below.
      const id = "prn-failing";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "failing", lastFailureKind: "timeout" }));
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") return Promise.resolve(encodeFrame(header(), new Uint8Array([1])));
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [], nextCursor: null } });
        if (name === "media_usage") return Promise.resolve({ contractVersion: 1, data: { usedBytes: 0, pinnedBytes: 0, capBytes: 1, retentionDays: 30, snapshotCount: 0, pinnedCount: 0, prunedCount: 0 } });
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={() => true} onOpenSetup={vi.fn()} />);
      await flush();

      expect(screen.getByRole("status")).toHaveTextContent("Camera not answering (timeout)");
    });

    it("web mode: the preview shows needsDesktopError's text instead of the health label", async () => {
      // Spec D4 "Health and preview": "the frontend polls
      // camera_preview_frame ... [but in web mode] Camera preview and test
      // return needsDesktopError, and the UI says so" -- the Rust-health
      // "Live" label must never paper over that.
      const id = "prn-web-preview";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      tauriMock.isTauri.mockReturnValue(false);
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={() => true} onOpenSetup={vi.fn()} />);
      await flush();

      const status = screen.getByRole("status");
      expect(status).toHaveTextContent("Previewing the camera needs the desktop app.");
      expect(status).not.toHaveTextContent("Live");
      expect(tauriMock.invoke).not.toHaveBeenCalledWith("camera_preview_frame", expect.anything());
    });

    it("desktop mode: a camera_preview_frame failure shows its own message, not the health label", async () => {
      const id = "prn-preview-fails";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") {
          return Promise.reject({ contractVersion: 1, code: "CAMERA_FAILED", message: "The camera did not answer in time.", recovery: ["RETRY"], retryable: true });
        }
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [], nextCursor: null } });
        if (name === "media_usage") return Promise.resolve({ contractVersion: 1, data: { usedBytes: 0, pinnedBytes: 0, capBytes: 1, retentionDays: 30, snapshotCount: 0, pinnedCount: 0, prunedCount: 0 } });
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={() => true} onOpenSetup={vi.fn()} />);
      await flush();

      const status = screen.getByRole("status");
      expect(status).toHaveTextContent("The camera did not answer in time.");
      expect(status).not.toHaveTextContent("Live");
    });

    it("Capture adds a row to the snapshot history", async () => {
      const id = "prn-capture";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      const captured = cameraSnapshot({ id: "snp-new", printerId: id, trigger: "manual", capturedAt: "2026-09-25T12:05:00Z" });
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") return Promise.resolve(encodeFrame(header(), new Uint8Array([1])));
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [], nextCursor: null } });
        if (name === "media_usage") return Promise.resolve({ contractVersion: 1, data: { usedBytes: 0, pinnedBytes: 0, capBytes: 1, retentionDays: 30, snapshotCount: 0, pinnedCount: 0, prunedCount: 0 } });
        if (name === "capture_snapshot") return Promise.resolve({ contractVersion: 1, data: captured });
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={() => true} onOpenSetup={vi.fn()} />);
      await flush();

      await fireEvent.click(screen.getByRole("button", { name: "Capture" }));
      await waitFor(() => expect(tauriMock.invoke).toHaveBeenCalledWith(
        "capture_snapshot",
        expect.objectContaining({ printerId: id, operationId: expect.any(String) }),
      ));
      expect(await screen.findByText("Manual")).toBeInTheDocument();
    });

    it("the history lists snapshots with pinned and pruned states", async () => {
      const id = "prn-history";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      const pinned = cameraSnapshot({ id: "snp-pinned", printerId: id, trigger: "manual", pinnedAt: "2026-09-25T00:00:00Z" });
      const pruned = cameraSnapshot({ id: "snp-pruned", printerId: id, trigger: "incident", prunedAt: "2026-09-26T00:00:00Z", pruneReason: "age" });
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") return Promise.resolve(encodeFrame(header(), new Uint8Array([1])));
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [pinned, pruned], nextCursor: null } });
        if (name === "media_usage") return Promise.resolve({ contractVersion: 1, data: { usedBytes: 0, pinnedBytes: 0, capBytes: 1, retentionDays: 30, snapshotCount: 2, pinnedCount: 1, prunedCount: 1 } });
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={() => true} onOpenSetup={vi.fn()} />);

      expect(await screen.findByText("Pinned")).toBeInTheDocument();
      expect(await screen.findByText("Pruned (past retention)")).toBeInTheDocument();
    });

    it("the retention summary comes from media_usage", async () => {
      const id = "prn-retention";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") return Promise.resolve(encodeFrame(header(), new Uint8Array([1])));
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [], nextCursor: null } });
        if (name === "media_usage") {
          return Promise.resolve({
            contractVersion: 1,
            data: { usedBytes: 1_048_576, pinnedBytes: 0, capBytes: 2 * 1024 * 1024 * 1024, retentionDays: 45, snapshotCount: 1, pinnedCount: 0, prunedCount: 0 },
          });
        }
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      render(() => <PrinterCameraPanel printer={printer({ id })} visible={() => true} onOpenSetup={vi.fn()} />);

      expect(await screen.findByText(/45-day retention/)).toBeInTheDocument();
      expect(screen.getByText(/1\.0 MiB of 2\.0 GiB used/)).toBeInTheDocument();
    });

    it("no object URL leaks after 50 preview frames", async () => {
      const id = "prn-leak";
      attentionStoreMock.health.set(id, cameraHealth({ printerId: id, state: "ok", sourceKind: "hostWebcam" }));
      vi.useFakeTimers();
      let frameCount = 0;
      tauriMock.invoke.mockImplementation((name: string) => {
        if (name === "camera_preview_frame") {
          frameCount += 1;
          return Promise.resolve(encodeFrame(header(), new Uint8Array([frameCount % 256])));
        }
        if (name === "list_snapshots") return Promise.resolve({ contractVersion: 1, data: { snapshots: [], nextCursor: null } });
        if (name === "media_usage") return Promise.resolve({ contractVersion: 1, data: { usedBytes: 0, pinnedBytes: 0, capBytes: 1, retentionDays: 30, snapshotCount: 0, pinnedCount: 0, prunedCount: 0 } });
        return Promise.reject(new Error(`unexpected invoke ${name}`));
      });
      const createSpy = vi.spyOn(URL, "createObjectURL");
      const revokeSpy = vi.spyOn(URL, "revokeObjectURL");
      const [visible, setVisible] = createSignal(true);
      const { unmount } = render(() => <PrinterCameraPanel printer={printer({ id })} visible={visible} onOpenSetup={vi.fn()} />);

      await vi.advanceTimersByTimeAsync(0);
      for (let i = 0; i < 49; i += 1) {
        await vi.advanceTimersByTimeAsync(1_000);
      }
      expect(frameCount).toBeGreaterThanOrEqual(50);

      setVisible(false);
      await vi.advanceTimersByTimeAsync(0);
      unmount();

      // Every created preview object URL was revoked -- either by the next
      // frame or by stopping (spec: "no object URL leaks after 50 frames").
      expect(revokeSpy.mock.calls.length).toBe(createSpy.mock.calls.length);
    });
  });
});
