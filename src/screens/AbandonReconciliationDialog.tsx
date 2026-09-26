import { createEffect, createSignal, on, Show } from "solid-js";
import { AlertDialog, Button, Checkbox, Textarea } from "../design-system";
import { abandonHostOperation } from "../host-ops/host-operations-store";
import { hostOperationLabel, NO_LONGER_PENDING_TEXT } from "../host-ops/presentation";
import type { HostOperation } from "../host-ops/types";
import { HostOperationAlert } from "./HostOperationAlert";
import styles from "./AbandonReconciliationDialog.module.css";

export interface AbandonReconciliationDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  operation: HostOperation;
  printerName: string;
  returnFocus?: () => HTMLElement | null | undefined;
}

/** D8: the note is optional, trimmed, and at most 500 characters. */
const NOTE_LIMIT = 500;

/** D8's **Abandon check…**: the operator's recorded decision to stop
 *  checking an uncertain Host Operation. It is not success — the printer's
 *  state stays unknown — so it sits behind a required acknowledgement. */
export function AbandonReconciliationDialog(props: AbandonReconciliationDialogProps) {
  const [acknowledged, setAcknowledged] = createSignal(false);
  const [note, setNote] = createSignal("");
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setAcknowledged(false);
    setNote("");
    setError(null);
  }));

  const noteTooLong = () => note().trim().length > NOTE_LIMIT;
  const canConfirm = () => acknowledged() && !noteTooLong() && !pending();

  async function onConfirm() {
    if (!canConfirm()) return;
    setPending(true);
    setError(null);
    try {
      await abandonHostOperation(props.operation.id, note());
      props.onOpenChange(false);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  return (
    <AlertDialog
      title="Stop checking this operation?"
      open={props.open}
      onOpenChange={props.onOpenChange}
      returnFocus={props.returnFocus}
    >
      <div class={styles.body}>
        <p class={styles.text}>
          {hostOperationLabel(props.operation).text}: <span class={styles.file}>{props.operation.hostPath}</span> on{" "}
          {props.printerName}.
        </p>
        <p class={styles.text}>
          farm3d will stop checking the printer for this operation and won't send anything to it. The printer's state
          stays unknown: stopping is not the same as success.
        </p>
        <Show when={props.operation.noLongerPending}>
          <p class={styles.text}>{NO_LONGER_PENDING_TEXT}</p>
        </Show>
        <Checkbox checked={acknowledged()} onChange={setAcknowledged} disabled={pending()}>
          I understand the printer may still have this file or be printing it.
        </Checkbox>
        <Textarea
          label="Note (optional)"
          value={note()}
          onChange={setNote}
          rows={2}
          description={`${note().trim().length}/${NOTE_LIMIT}`}
          errorMessage={noteTooLong() ? `The note can be at most ${NOTE_LIMIT} characters.` : undefined}
        />
        <Show when={error()}>
          {(held) => <HostOperationAlert error={held()} fallback="farm3d couldn't stop checking this operation." />}
        </Show>
        <div class={styles.footer}>
          <Show when={!acknowledged()}>
            <p class={styles.reason}>Tick the acknowledgement to stop checking.</p>
          </Show>
          <div class={styles.actions}>
            <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Keep checking</Button>
            <Button variant="danger" disabled={!canConfirm()} onClick={() => void onConfirm()}>
              Stop checking
            </Button>
          </div>
        </div>
      </div>
    </AlertDialog>
  );
}
