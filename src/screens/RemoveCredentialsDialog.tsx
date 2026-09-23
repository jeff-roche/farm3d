import { createEffect, createSignal, on, Show } from "solid-js";
import { Button, Dialog } from "../design-system";
import { isCommandError } from "../ipc/client";
import { setConnection } from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
import styles from "./RemoveCredentialsDialog.module.css";

export interface RemoveCredentialsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  printer: ResolvedPrinter;
}

/** Confirms clearing a Printer's stored API key. Resubmits the *saved*
 *  Connection (never the tab's unsaved edits) with `credential: ""`, which
 *  `set_printer_connection` treats as "clear" -- the Connection itself is
 *  kept, and with its settings unchanged no replacement probe runs. */
export function RemoveCredentialsDialog(props: RemoveCredentialsDialogProps) {
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (open) setError(null);
  }));

  async function onConfirm() {
    const connection = props.printer.connection;
    if (!connection || pending()) return;
    setPending(true);
    setError(null);
    try {
      await setConnection(props.printer.id, {
        kind: connection.kind,
        host: connection.host,
        port: connection.port,
        useTls: connection.useTls,
        credential: "",
      });
      props.onOpenChange(false);
    } catch (e) {
      setError(isCommandError(e) ? e.message : "The credentials could not be removed.");
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog title="Remove credentials" open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <p class={styles.note}>
          This deletes the stored API key for "{props.printer.name}". The Connection is kept. If
          the printer requires a key, it will stop connecting until you add one again.
        </p>
        <Show when={error()}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>
            Cancel
          </Button>
          <Button variant="danger" disabled={pending()} onClick={() => void onConfirm()}>
            Remove credentials
          </Button>
        </div>
      </div>
    </Dialog>
  );
}
