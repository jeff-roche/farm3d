import { createEffect, createSignal, on, Show } from "solid-js";
import { Button, Dialog, TextField } from "../design-system";
import { removePrinter } from "../printers/printer-store";
import styles from "./DeletePrinterDialog.module.css";

export interface DeletePrinterDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  printerId: string;
  printerName: string;
  /** Called only after a successful delete, once the dialog has already asked to
   *  close itself -- e.g. so the caller can close the detail dock too. */
  onDeleted?: () => void;
}

/** Guards `removePrinter` (spec D7) behind typing the Printer's name back
 *  exactly -- no `window.confirm`, and no trimming/case folding, per the
 *  ruling. Deletion is only ever offered once a Printer is archived
 *  (`canDelete`), so this dialog assumes that's already true. */
export function DeletePrinterDialog(props: DeletePrinterDialogProps) {
  const [typed, setTyped] = createSignal("");
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (open) {
      setTyped("");
      setError(null);
    }
  }));

  const matches = () => typed() === props.printerName;

  async function onConfirm() {
    if (!matches() || pending()) return;
    setPending(true);
    setError(null);
    try {
      const result = await removePrinter(props.printerId);
      if (!result.ok) {
        setError(result.message);
        return;
      }
      props.onOpenChange(false);
      props.onDeleted?.();
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog title="Delete Printer" open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <p class={styles.note}>
          This permanently deletes "{props.printerName}" and cannot be undone. Its Connection,
          overrides, and history are removed with it.
        </p>
        <p class={styles.note}>Spool movement history involving this Printer will be deleted.</p>
        <TextField
          label="Type the Printer name to confirm"
          value={typed()}
          onChange={setTyped}
        />
        <Show when={error()}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
            Cancel
          </Button>
          <Button
            variant="danger"
            disabled={!matches() || pending()}
            onClick={() => void onConfirm()}
          >
            Delete permanently
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
