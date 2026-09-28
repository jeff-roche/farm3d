import { createEffect, createSignal, on, onCleanup, Show } from "solid-js";
import { Button, Dialog } from "../design-system";
import { isCommandError } from "../ipc/client";
import { setSnapshotPinned, snapshotImageUrl } from "../cameras/camera-store";
import { pruneReasonLabel, snapshotAltText, snapshotTriggerLabel } from "../attention/presentation";
import type { CameraSnapshot } from "../attention/types";
import { formatDateTime } from "../slicing/revision-presentation";
import styles from "./SnapshotViewerDialog.module.css";

export interface SnapshotViewerDialogProps {
  snapshot: CameraSnapshot;
  /** For the image's alt text (spec: "<trigger> snapshot of <Printer> at
   *  <time>"). */
  printerName: string;
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** One Snapshot, full size, with Pin/Unpin (spec "Incident detail":
 *  "`SnapshotViewerDialog` shows one snapshot with Pin/Unpin"). A pruned
 *  Snapshot shows "Evidence pruned (<reason>)" and no image (D5): pinning a
 *  pruned row is `EVIDENCE_PRUNED` and not offered here, but unpinning one
 *  stays allowed (D5 "Pinning a pruned row"). Pin/Unpin are plain `Button`s
 *  (native `<button>`s), so Enter/Space activate them like a click -- no
 *  extra keyboard wiring needed. */
export function SnapshotViewerDialog(props: SnapshotViewerDialogProps) {
  const [record, setRecord] = createSignal(props.snapshot);
  const [url, setUrl] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);

  createEffect(on(() => props.snapshot, setRecord));

  createEffect(on(() => [record().id, record().prunedAt] as const, ([id, prunedAt]) => {
    setUrl(null);
    if (prunedAt !== null) return;
    let cancelled = false;
    let created: string | null = null;
    snapshotImageUrl(id).then((loaded) => {
      if (cancelled) return;
      created = loaded;
      setUrl(loaded);
    }).catch((e) => { if (!cancelled) setError(e); });
    onCleanup(() => {
      cancelled = true;
      if (created?.startsWith("blob:")) URL.revokeObjectURL(created);
    });
  }));

  async function togglePin(pinned: boolean): Promise<void> {
    if (pending()) return;
    setPending(true);
    setError(null);
    try {
      setRecord(await setSnapshotPinned(record().id, pinned));
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog
      title={`Snapshot — ${formatDateTime(record().capturedAt)}`}
      open={props.open}
      onOpenChange={props.onOpenChange}
    >
      <div class={styles.body}>
        <Show
          when={record().prunedAt === null}
          fallback={<p class={styles.pruned}>{`Evidence pruned (${pruneReasonLabel(record().pruneReason!)})`}</p>}
        >
          <Show when={url()} fallback={<p class={styles.status} role="status">Loading the image…</p>}>
            {(src) => <img class={styles.image} src={src()} alt={snapshotAltText(record(), props.printerName)} />}
          </Show>
        </Show>

        <dl class={styles.fields}>
          <div class={styles.field}>
            <dt>Trigger</dt>
            <dd>{snapshotTriggerLabel(record().trigger)}</dd>
          </div>
          <div class={styles.field}>
            <dt>Captured</dt>
            <dd>{formatDateTime(record().capturedAt)}</dd>
          </div>
        </dl>

        <div class={styles.actions}>
          <Show when={record().pinnedAt !== null}>
            <Button variant="secondary" disabled={pending()} onClick={() => void togglePin(false)}>Unpin</Button>
          </Show>
          <Show when={record().pinnedAt === null && record().prunedAt === null}>
            <Button variant="secondary" disabled={pending()} onClick={() => void togglePin(true)}>Pin</Button>
          </Show>
        </div>

        <Show when={error()}>
          {(held) => {
            const err = held();
            return (
              <p class={styles.error} role="alert">
                {isCommandError(err) ? err.message : "This snapshot couldn't be updated."}
              </p>
            );
          }}
        </Show>
      </div>
    </Dialog>
  );
}
