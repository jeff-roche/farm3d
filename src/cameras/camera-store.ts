/** The camera-side data layer (spec "Frontend architecture"): camera
 *  health is read from `attention-store.ts` (which owns the `attention`
 *  stream that carries `camera.health.changed`) -- this module never
 *  duplicates that reconciliation. It owns the rest: a live preview poll,
 *  test/probe frames, snapshots, pinning, and `media_usage`.
 *
 *  Global constraint 3: this store never holds a camera URL. `test_camera`
 *  and `camera_preview_frame` answer with image bytes, never the source
 *  that produced them; the one command that returns a manual URL
 *  (`get_printer_camera`) is the Setup editor's own concern, called
 *  directly from its form state, never through this module. */
import { createEffect, createSignal, onCleanup, type Accessor } from "solid-js";
import { createStore, reconcile } from "solid-js/store";
import { binaryCommand, command, desktopAvailable, needsDesktopError, retryOnTransportFailure } from "../ipc/client";
import { attention, onAttentionPrinterRemoved, onAttentionSnapshotChanged } from "../attention/attention-store";
import { decodeFrame, frameObjectUrl, type CameraFrame } from "./frame";
import type { CommandError } from "../generated/contracts/command/CommandError";
import type { ConnectionSubmission } from "../generated/contracts/domain/ConnectionSubmission";
import type {
  CameraHealth,
  CameraSnapshot,
  CameraSourceInput,
  HostWebcam,
  MediaUsage,
  SnapshotPage,
} from "../attention/types";

// --- Health (read-through to attention-store) -------------------------------

export const camera = {
  /** One Printer's camera health, from the `attention` stream's own
   *  backfill/`camera.health.changed` (this store never re-fetches or
   *  re-derives it). */
  health: (printerId: string): CameraHealth | undefined => attention.cameraHealth(printerId),
  allHealth: (): CameraHealth[] => attention.allCameraHealth(),
  snapshot: (id: string): CameraSnapshot | undefined => state.snapshots.find((record) => record.id === id),
  snapshotsForPrinter: (printerId: string): CameraSnapshot[] =>
    state.snapshots.filter((record) => record.printerId === printerId),
  snapshotsForIncident: (incidentId: string): CameraSnapshot[] =>
    state.snapshots.filter((record) => record.incidentId === incidentId),
};

// --- The local snapshot cache -----------------------------------------------
// Not a reconciled stream like `attention-store.ts`'s Events (snapshots
// aren't in the backfill; they only ever arrive from a command result or an
// `attention.snapshot.changed` live event) -- just a revision-gated cache of
// whatever `listSnapshots`/`captureSnapshot`/`setSnapshotPinned` have seen.

interface CameraState {
  snapshots: CameraSnapshot[];
}

const [state, setState] = createStore<CameraState>({ snapshots: [] });

function upsertSnapshot(record: CameraSnapshot): void {
  const index = state.snapshots.findIndex((candidate) => candidate.id === record.id);
  if (index >= 0 && record.revision < state.snapshots[index].revision) return;
  if (index >= 0) setState("snapshots", index, reconcile(record));
  else setState("snapshots", (list) => [...list, record]);
}

// A deleted Printer's `camera_snapshots` rows are removed by cascade with no
// removal event (spec Task 12 "Controller carry") -- drop this store's own
// cache of them the same way `attention-store.ts` drops camera health.
onAttentionPrinterRemoved((printerId) => {
  setState("snapshots", (list) => list.filter((record) => record.printerId !== printerId));
});
onAttentionSnapshotChanged((snapshot) => upsertSnapshot(snapshot));

function notFound(what: string): CommandError {
  return { contractVersion: 1, code: "NOT_FOUND", message: `${what} was not found.`, recovery: [], retryable: false };
}

async function write<T>(action: string, send: (operationId: string) => Promise<T>): Promise<T> {
  if (!desktopAvailable()) throw needsDesktopError(action);
  const operationId = crypto.randomUUID();
  return retryOnTransportFailure(() => send(operationId));
}

export interface ListSnapshotsOptions {
  printerId?: string;
  incidentId?: string;
  jobId?: string;
  includePruned?: boolean;
  before?: string;
  limit?: number;
}

/** `list_snapshots`: `capturedAt` descending, then id. Every returned row
 *  is also folded into the local cache, so `camera.snapshot(id)` etc. stay
 *  current for whatever was just listed. */
export async function listSnapshots(options: ListSnapshotsOptions = {}): Promise<SnapshotPage> {
  const page: SnapshotPage = desktopAvailable()
    ? await retryOnTransportFailure(() => command("list_snapshots", options))
    : await import("../attention/web-fixtures").then(({ webSnapshotPage }) => webSnapshotPage(options));
  for (const snapshot of page.snapshots) upsertSnapshot(snapshot);
  return page;
}

/** `capture_snapshot`: a manual capture, linking the Printer's active Job
 *  if any (Rust's own decision). Web mode has no backend to write to. */
export async function captureSnapshot(printerId: string): Promise<CameraSnapshot> {
  const snapshot = await write("Capturing a snapshot", (operationId) => command("capture_snapshot", { operationId, printerId }));
  upsertSnapshot(snapshot);
  return snapshot;
}

/** `set_snapshot_pinned`. `pinned: true` on an already-pruned row is
 *  Rust's own `EVIDENCE_PRUNED`; `pinned: false` is always allowed (D5). */
export async function setSnapshotPinned(snapshotId: string, pinned: boolean): Promise<CameraSnapshot> {
  const snapshot = await write("Pinning a snapshot", (operationId) => command("set_snapshot_pinned", { operationId, snapshotId, pinned }));
  upsertSnapshot(snapshot);
  return snapshot;
}

export async function mediaUsage(): Promise<MediaUsage> {
  if (!desktopAvailable()) {
    return import("../attention/web-fixtures").then(({ webMediaUsage }) => webMediaUsage());
  }
  return retryOnTransportFailure(() => command("media_usage"));
}

/** A shown-once image URL for one snapshot (`SnapshotViewerDialog`, Task
 *  14/15): an object URL on the desktop (the caller should
 *  `URL.revokeObjectURL` it once done), or web mode's fixture data URL
 *  directly. A pruned snapshot rejects with the same `EVIDENCE_PRUNED`
 *  shape Rust's own `snapshot_image` would. */
export async function snapshotImageUrl(snapshotId: string): Promise<string> {
  if (desktopAvailable()) {
    return frameObjectUrl(decodeFrame(await binaryCommand("snapshot_image", { snapshotId })));
  }
  const { webSnapshotPage, webSnapshotImageDataUrl } = await import("../attention/web-fixtures");
  const record = webSnapshotPage({}).snapshots.find((candidate) => candidate.id === snapshotId);
  if (!record) throw notFound(`Snapshot ${snapshotId}`);
  if (record.prunedAt !== null) {
    throw {
      contractVersion: 1,
      code: "EVIDENCE_PRUNED",
      message: `This snapshot's image was removed (${record.pruneReason}).`,
      recovery: [],
      retryable: false,
      details: { snapshotId, reason: record.pruneReason },
    } satisfies CommandError;
  }
  const url = webSnapshotImageDataUrl(snapshotId);
  if (!url) throw notFound(`Snapshot ${snapshotId}`);
  return url;
}

// --- Setup wizard/dock probes ------------------------------------------------
// `test_camera`/`list_host_webcams` are the Setup wizard's and the dock's
// Setup tab's own commands (never persisted or cached here); they live
// beside `usePreview` only because they share its binary-frame/needsDesktop
// handling.

export interface TestCameraArgs {
  printerId?: string;
  connection?: ConnectionSubmission;
  source: CameraSourceInput;
}

/** `test_camera`: never stored, and web mode refuses it outright (spec
 *  Task 12: "Camera preview and test in web mode return
 *  needsDesktopError"). */
export async function testCamera(args: TestCameraArgs): Promise<CameraFrame> {
  if (!desktopAvailable()) throw needsDesktopError("Testing the camera");
  return decodeFrame(await binaryCommand("test_camera", args));
}

export interface ListHostWebcamsArgs {
  printerId?: string;
  connection?: ConnectionSubmission;
}

export async function listHostWebcams(args: ListHostWebcamsArgs): Promise<HostWebcam[]> {
  if (!desktopAvailable()) throw needsDesktopError("Listing the printer's webcams");
  return retryOnTransportFailure(() => command("list_host_webcams", args));
}

// --- The live preview poll ---------------------------------------------------

const PREVIEW_INTERVAL_MS = 1_000;

export interface CameraPreview {
  url: Accessor<string | null>;
  error: Accessor<unknown>;
  capturedAt: Accessor<string | null>;
  loading: Accessor<boolean>;
}

/** `camera_preview_frame`, polled about once a second while `visible()` is
 *  true (spec "Frontend architecture": `usePreview(printerId, visible)`).
 *  Each poll is scheduled `PREVIEW_INTERVAL_MS` after the previous one
 *  settles (a chained timeout, not an interval), so a slow camera never
 *  has two fetches in flight.
 *  Revokes the previous object URL on each new frame and when it stops
 *  (`visible()` goes false, `printerId()` changes, or the owner is
 *  cleaned up) -- never leaks one. Web mode reports `needsDesktopError`
 *  once and never polls. */
export function usePreview(printerId: Accessor<string>, visible: Accessor<boolean>): CameraPreview {
  const [url, setUrl] = createSignal<string | null>(null);
  const [error, setError] = createSignal<unknown>(null);
  const [capturedAt, setCapturedAt] = createSignal<string | null>(null);
  const [loading, setLoading] = createSignal(false);

  let currentUrl: string | null = null;
  let timer: ReturnType<typeof setTimeout> | undefined;
  let generation = 0;
  let disposed = false;

  function revoke(): void {
    if (currentUrl !== null) {
      URL.revokeObjectURL(currentUrl);
      currentUrl = null;
    }
  }

  function stopTimer(): void {
    if (timer !== undefined) {
      clearTimeout(timer);
      timer = undefined;
    }
  }

  /** One poll, then the next one `PREVIEW_INTERVAL_MS` after it settles,
   *  for as long as this generation is current. */
  function pollLoop(id: string, myGeneration: number): void {
    timer = undefined;
    void poll(id, myGeneration).then(() => {
      if (disposed || myGeneration !== generation) return;
      timer = setTimeout(() => pollLoop(id, myGeneration), PREVIEW_INTERVAL_MS);
    });
  }

  async function poll(id: string, myGeneration: number): Promise<void> {
    setLoading(true);
    try {
      const bytes = await binaryCommand("camera_preview_frame", { printerId: id });
      if (disposed || myGeneration !== generation) return;
      const frame = decodeFrame(bytes);
      revoke();
      currentUrl = frameObjectUrl(frame);
      setUrl(currentUrl);
      setCapturedAt(frame.header.capturedAt);
      setError(null);
    } catch (caught) {
      if (disposed || myGeneration !== generation) return;
      setError(caught);
    } finally {
      if (!disposed && myGeneration === generation) setLoading(false);
    }
  }

  createEffect(() => {
    const id = printerId();
    const isVisible = visible();
    generation += 1;
    const myGeneration = generation;
    stopTimer();
    revoke();
    setUrl(null);
    setCapturedAt(null);
    if (!isVisible) {
      setError(null);
      setLoading(false);
      return;
    }
    if (!desktopAvailable()) {
      setError(needsDesktopError("Previewing the camera"));
      setLoading(false);
      return;
    }
    setError(null);
    pollLoop(id, myGeneration);
  });

  onCleanup(() => {
    disposed = true;
    generation += 1;
    stopTimer();
    revoke();
  });

  return { url, error, capturedAt, loading };
}
