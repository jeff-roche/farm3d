import { createEffect, createSignal, For, Match, on, onCleanup, Show, Switch, type Accessor } from "solid-js";
import { Button } from "../design-system";
import { isCommandError } from "../ipc/client";
import { camera, captureSnapshot, listSnapshots, mediaUsage, usePreview } from "../cameras/camera-store";
import { cameraHealthStateLabel, pruneReasonLabel, snapshotTriggerLabel } from "../attention/presentation";
import type { CameraSnapshot, MediaUsage } from "../attention/types";
import type { ResolvedPrinter } from "../printers/types";
import { formatDateTime } from "../slicing/revision-presentation";
import { formatBytes } from "./library-presentation";
import { SnapshotViewerDialog } from "./SnapshotViewerDialog";
import styles from "./PrinterCameraPanel.module.css";

export interface PrinterCameraPanelProps {
  printer: ResolvedPrinter;
  /** True only while this is the active tab of an open dock (spec: "the
   *  frontend polls `camera_preview_frame` about once a second only while
   *  the Camera tab is visible and the dock is open"). Threaded straight
   *  into `usePreview`. */
  visible: Accessor<boolean>;
  /** "Set up camera" (no source configured yet): switches the dock to the
   *  Setup tab, where Task 15 adds the camera section. */
  onOpenSetup: () => void;
}

function formatFrameTimestamp(iso: string): string {
  const date = new Date(iso);
  if (Number.isNaN(date.getTime())) return `Last frame ${iso}`;
  return `Last frame ${date.toLocaleTimeString(undefined, { hour12: false })}`;
}

/** Printer detail dock's Camera tab (spec "Frontend architecture"): no
 *  source -> an explanation plus "Set up camera"; `unsupported` -> the
 *  reason; otherwise the live preview with its health text and last-frame
 *  time, Capture, the snapshot history, and the retention summary. Health
 *  is Rust's own (`camera.health`, read through from the `attention`
 *  stream) -- this never derives a state itself (global constraint 4). */
export function PrinterCameraPanel(props: PrinterCameraPanelProps) {
  const health = () => camera.health(props.printer.id);
  const preview = usePreview(() => props.printer.id, props.visible);

  const [historyError, setHistoryError] = createSignal<unknown>(null);
  const [usage, setUsage] = createSignal<MediaUsage | undefined>(undefined);
  const [viewerSnapshotId, setViewerSnapshotId] = createSignal<string | null>(null);
  const [capturePending, setCapturePending] = createSignal(false);
  const [captureError, setCaptureError] = createSignal<unknown>(null);

  /** `notConfigured`/no row, `unsupported`, or configured (`unknown`,
   *  `ok`, `failing`) -- three mutually exclusive branches, straight off
   *  Rust's own `state` (global constraint 4). */
  const state = () => health()?.state ?? "notConfigured";
  const configured = () => state() !== "notConfigured" && state() !== "unsupported";

  // Loads once each time the tab becomes visible (never while hidden, per
  // the same "only while visible" spirit as the preview poll) -- results
  // land in `camera-store`'s own cache, which `snapshots()` below reads
  // reactively, so a live `attention.snapshot.changed` keeps it current
  // without a second fetch.
  createEffect(on(() => [props.printer.id, props.visible(), configured()] as const, ([id, isVisible, isConfigured]) => {
    if (!isVisible || !isConfigured) return;
    let cancelled = false;
    setHistoryError(null);
    listSnapshots({ printerId: id }).catch((e) => { if (!cancelled) setHistoryError(e); });
    mediaUsage().then((loaded) => { if (!cancelled) setUsage(loaded); }).catch(() => {});
    onCleanup(() => { cancelled = true; });
  }));

  const snapshots = (): CameraSnapshot[] => camera.snapshotsForPrinter(props.printer.id)
    .slice()
    .sort((a, b) => b.capturedAt.localeCompare(a.capturedAt) || b.id.localeCompare(a.id));

  const viewerSnapshot = () => snapshots().find((snapshot) => snapshot.id === viewerSnapshotId());

  const statusLabel = () => {
    const current = health();
    if (!current) return cameraHealthStateLabel("notConfigured");
    if (current.state === "failing" && current.lastFailureKind) {
      return `${cameraHealthStateLabel("failing")} (${current.lastFailureKind})`;
    }
    return cameraHealthStateLabel(current.state);
  };

  async function onCapture(): Promise<void> {
    if (capturePending()) return;
    setCapturePending(true);
    setCaptureError(null);
    try {
      await captureSnapshot(props.printer.id);
      setUsage(await mediaUsage());
    } catch (e) {
      setCaptureError(e);
    } finally {
      setCapturePending(false);
    }
  }

  return (
    <div class={styles.panel}>
      <Switch>
        <Match when={state() === "notConfigured"}>
          <div class={styles.emptyState}>
            <p>{cameraHealthStateLabel("notConfigured")}. Add a camera to preview and capture evidence for this Printer.</p>
            <Button variant="secondary" onClick={props.onOpenSetup}>Set up camera</Button>
          </div>
        </Match>
        <Match when={state() === "unsupported"}>
          <div class={styles.emptyState}>
            <p>{cameraHealthStateLabel("unsupported")}</p>
          </div>
        </Match>
        <Match when={configured()}>
          <div class={styles.preview}>
            <div class={styles.previewFrame}>
              <Show when={preview.url()}>
                {(src) => (
                  <img
                    class={styles.previewImage}
                    src={src()}
                    alt={`Live camera preview of ${props.printer.name}`}
                  />
                )}
              </Show>
            </div>
            <p class={styles.previewStatus} role="status">{statusLabel()}</p>
            <p class={styles.previewTimestamp}>
              {preview.capturedAt() ? formatFrameTimestamp(preview.capturedAt()!) : "No frame captured yet."}
            </p>
            <Button variant="secondary" size="sm" disabled={capturePending()} onClick={() => void onCapture()}>
              Capture
            </Button>
            <Show when={captureError()}>
              {(held) => {
                const err = held();
                return (
                  <p class={styles.error} role="alert">
                    {isCommandError(err) ? err.message : "That capture failed."}
                  </p>
                );
              }}
            </Show>
          </div>

          <section class={styles.history} aria-labelledby="printer-camera-history-title">
            <h3 id="printer-camera-history-title" class={styles.historyTitle}>Snapshot history</h3>
            <Show when={historyError()}>
              {(held) => {
                const err = held();
                return (
                  <p class={styles.error} role="alert">
                    {isCommandError(err) ? err.message : "The snapshot history couldn't be loaded."}
                  </p>
                );
              }}
            </Show>
            <Show when={snapshots().length > 0} fallback={<p class={styles.empty}>No snapshots yet.</p>}>
              <ul class={styles.historyList}>
                <For each={snapshots()}>
                  {(snapshot) => (
                    <li>
                      <button type="button" class={styles.historyRow} onClick={() => setViewerSnapshotId(snapshot.id)}>
                        <span>{snapshotTriggerLabel(snapshot.trigger)}</span>
                        <time datetime={snapshot.capturedAt}>{formatDateTime(snapshot.capturedAt)}</time>
                        <Show when={snapshot.pinnedAt !== null}>
                          <span class={styles.badge}>Pinned</span>
                        </Show>
                        <Show when={snapshot.prunedAt !== null}>
                          <span class={styles.badge}>{`Pruned (${pruneReasonLabel(snapshot.pruneReason!)})`}</span>
                        </Show>
                      </button>
                    </li>
                  )}
                </For>
              </ul>
            </Show>
          </section>

          <p class={styles.retention}>
            <Show when={usage()}>
              {(loaded) => (
                <>
                  {formatBytes(loaded().usedBytes)} of {formatBytes(loaded().capBytes)} used
                  {" · "}{loaded().retentionDays}-day retention{" · "}{loaded().pinnedCount} pinned
                </>
              )}
            </Show>
          </p>
        </Match>
      </Switch>

      <Show when={viewerSnapshot()}>
        {(snapshot) => (
          <SnapshotViewerDialog
            snapshot={snapshot()}
            printerName={props.printer.name}
            open
            onOpenChange={(open) => !open && setViewerSnapshotId(null)}
          />
        )}
      </Show>
    </div>
  );
}
