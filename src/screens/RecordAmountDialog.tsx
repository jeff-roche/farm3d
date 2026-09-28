import { createEffect, createSignal, on, Show } from "solid-js";
import { Button, Dialog, Textarea } from "../design-system";
import { isCommandError } from "../ipc/client";
import { recordAmount } from "../spools/spool-store";
import type { AmountEntry } from "../generated/contracts/domain/AmountEntry";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import { AmountEntryFields, type AmountEntryFieldPath } from "./AmountEntryFields";
import styles from "./RecordAmountDialog.module.css";

export interface RecordAmountDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  spool: SpoolRecord;
  onRecorded?: (spool: SpoolRecord) => void;
}

/** D3/D7: **Net** / **Scale** amount entry (spec §Components), through the
 *  shared `AmountEntryFields`. Gross-below-tare (D3) is checked client-side
 *  before submit is even enabled -- a UX nicety that catches the common
 *  case instantly -- but `recordAmount` itself also rejects (fix round 1
 *  ruling), so a `VALIDATION` on `entry.grossMg` that only Rust could catch
 *  (e.g. a tare edited concurrently) still renders inline here rather than
 *  being lost to the store banner behind this dialog's own overlay. */
export function RecordAmountDialog(props: RecordAmountDialogProps) {
  const [entry, setEntry] = createSignal<AmountEntry | null>(null);
  const [note, setNote] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [dialogError, setDialogError] = createSignal<string | null>(null);
  const [serverFieldError, setServerFieldError] = createSignal<{ path: AmountEntryFieldPath; message: string } | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setNote("");
    setDialogError(null);
    setServerFieldError(null);
  }));

  /** Fix round 2: a server field error is display-only -- it never gates
   *  submit on its own (only client-side validity does), and it clears the
   *  moment the user edits any input that field's validation depends on. */
  function clearServerFieldError(path: AmountEntryFieldPath): void {
    if (serverFieldError()?.path === path) setServerFieldError(null);
  }

  const canSubmit = () => !submitting() && entry() !== null;

  async function onSubmit() {
    const current = entry();
    if (!canSubmit() || !current) return;
    setSubmitting(true);
    setDialogError(null);
    setServerFieldError(null);
    try {
      const result = await recordAmount(props.spool.id, current, note().trim() || undefined);
      props.onOpenChange(false);
      props.onRecorded?.(result);
    } catch (e) {
      if (isCommandError(e) && e.code === "CONFLICT") {
        setDialogError("This Spool changed since you opened it. It's been reloaded with the current values — check them and try again.");
      } else if (isCommandError(e) && (e.details?.fieldPath === "entry.grossMg" || e.details?.fieldPath === "entry.netMg")) {
        setServerFieldError({ path: e.details.fieldPath, message: e.message });
      } else {
        setDialogError(isCommandError(e) ? e.message : "This amount could not be recorded.");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog title="Record amount" open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <AmountEntryFields
          defaultTareId={props.spool.tareId}
          fieldErrors={serverFieldError() ? { [serverFieldError()!.path]: serverFieldError()!.message } : undefined}
          onEdit={clearServerFieldError}
          onChange={setEntry}
        />
        <Textarea label="Note (optional)" value={note()} onChange={setNote} rows={2} />
        <Show when={dialogError()}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
          <Button disabled={!canSubmit()} onClick={() => void onSubmit()}>Record</Button>
        </div>
      </div>
    </Dialog>
  );
}
