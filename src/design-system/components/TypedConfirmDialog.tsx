import { createEffect, createSignal, For, on, Show } from "solid-js";
import { Button } from "./Button";
import { Dialog } from "./Dialog";
import { TextField } from "./TextField";
import styles from "./TypedConfirmDialog.module.css";

export interface TypedConfirmDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  title: string;
  /** What will happen; rendered as a list inside the dialog's description. */
  consequences: string[];
  /** The text the user must type back. Matching is exact: no trim, no case folding. */
  phrase: string;
  confirmLabel: string;
  pending?: boolean;
  onConfirm: () => void;
  /** Label for the input. Defaults to naming the phrase. */
  inputLabel?: string;
  /** A failure from the last attempt, shown as an alert. */
  error?: string | null;
}

/** A confirmation for an irreversible action: the confirm button stays
 *  disabled until the user types `phrase` back exactly. The input clears
 *  every time the dialog opens. */
export function TypedConfirmDialog(props: TypedConfirmDialogProps) {
  const [typed, setTyped] = createSignal("");

  createEffect(on(() => props.open, (open) => {
    if (open) setTyped("");
  }));

  const matches = () => typed() === props.phrase;

  function confirm() {
    if (!matches() || props.pending) return;
    props.onConfirm();
  }

  return (
    <Dialog
      title={props.title}
      open={props.open}
      onOpenChange={props.onOpenChange}
      description={
        <ul class={styles.consequences}>
          <For each={props.consequences}>{(line) => <li>{line}</li>}</For>
        </ul>
      }
    >
      <div class={styles.body}>
        <TextField
          label={props.inputLabel ?? `Type "${props.phrase}" to confirm`}
          value={typed()}
          onChange={setTyped}
        />
        <Show when={props.error}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="danger" disabled={!matches() || props.pending} onClick={confirm}>
            {props.confirmLabel}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
