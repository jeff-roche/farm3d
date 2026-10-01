import { createEffect, createSignal, on } from "solid-js";
import { TypedConfirmDialog } from "../design-system";
import { removePrinter } from "../printers/printer-store";

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
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (open) setError(null);
  }));

  async function onConfirm() {
    if (pending()) return;
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
    <TypedConfirmDialog
      title="Delete Printer"
      open={props.open}
      onOpenChange={props.onOpenChange}
      consequences={[
        `This permanently deletes "${props.printerName}" and cannot be undone. Its Connection, overrides, and history are removed with it.`,
        "Spool movement history involving this Printer will be deleted.",
      ]}
      phrase={props.printerName}
      inputLabel="Type the Printer name to confirm"
      confirmLabel="Delete permanently"
      pending={pending()}
      error={error()}
      onConfirm={() => void onConfirm()}
    />
  );
}
