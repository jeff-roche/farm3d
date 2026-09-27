import { createEffect, createSignal, on, Show } from "solid-js";
import { AlertDialog, Button, Checkbox, RadioGroup } from "../design-system";
import { declareJobOutcome, refreshQueue } from "../queue/queue-store";
import type { DeclaredOutcome, Job } from "../queue/types";
import { formatGrams } from "../spools/weight";
import { HostOperationAlert } from "./HostOperationAlert";
import styles from "./JobDialogs.module.css";

export interface DeclareOutcomeDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  job: Job;
  returnFocus?: () => HTMLElement | null | undefined;
}

const OUTCOMES: { value: DeclaredOutcome; label: string }[] = [
  { value: "completed", label: "Completed" },
  { value: "failed", label: "Failed" },
  { value: "cancelled", label: "Cancelled" },
];

/** D9's `declare_job_outcome`: the operator's word for how a print ended
 *  when farm3d can't prove it (`outcomeUnknown`, or a Printer unreachable
 *  for 30 minutes, ruling R5). farm3d sends nothing to the printer, so it
 *  sits behind a required acknowledgement, and Confirm says why it's
 *  disabled until both choices are made. */
export function DeclareOutcomeDialog(props: DeclareOutcomeDialogProps) {
  const [outcome, setOutcome] = createSignal<DeclaredOutcome | undefined>();
  const [acknowledged, setAcknowledged] = createSignal(false);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setOutcome(undefined);
    setAcknowledged(false);
    setError(null);
  }));

  const canConfirm = () => outcome() !== undefined && acknowledged() && !pending();
  const reason = () => {
    if (outcome() === undefined) return "Choose how the print ended.";
    return acknowledged() ? undefined : "Tick the acknowledgement to declare.";
  };

  async function onConfirm() {
    const chosen = outcome();
    if (!chosen || !canConfirm()) return;
    setPending(true);
    setError(null);
    try {
      await declareJobOutcome(props.job.id, chosen);
      props.onOpenChange(false);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  return (
    <AlertDialog
      title="Declare how this Job ended?"
      open={props.open}
      onOpenChange={props.onOpenChange}
      returnFocus={props.returnFocus}
    >
      <div class={styles.body}>
        <p class={styles.text}>
          farm3d couldn't prove how the print on {props.job.printerSnapshot.name} ended. Check the printer, then say what
          happened. farm3d sends nothing to the printer.
        </p>
        <RadioGroup
          label="Outcome"
          options={OUTCOMES}
          value={outcome() ?? ""}
          onChange={(value) => setOutcome(value as DeclaredOutcome)}
          disabled={pending()}
        />
        <Show when={outcome() === "completed"}>
          <p class={styles.note}>farm3d deducts the full {formatGrams(props.job.estimateMg, 1)} estimate from the Spool.</p>
        </Show>
        <Show when={outcome() === "failed" || outcome() === "cancelled"}>
          <p class={styles.note}>The Job's material then needs settling: the estimate, a measured weight, or defer.</p>
        </Show>
        <Checkbox checked={acknowledged()} onChange={setAcknowledged} disabled={pending()}>
          farm3d can't confirm this with the printer. I checked the printer myself.
        </Checkbox>
        <Show when={error()}>
          {(held) => <HostOperationAlert error={held()} fallback="The outcome couldn't be declared." onReload={refreshQueue} />}
        </Show>
        <div class={styles.footer}>
          <Show when={reason()}>{(text) => <p class={styles.reason}>{text()}</p>}</Show>
          <div class={styles.actions}>
            <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Not now</Button>
            <Button variant="danger" disabled={!canConfirm()} onClick={() => void onConfirm()}>Declare</Button>
          </div>
        </div>
      </div>
    </AlertDialog>
  );
}
