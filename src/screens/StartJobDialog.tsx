import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { AlertDialog, Button, Checkbox } from "../design-system";
import { isCommandError } from "../ipc/client";
import { hostOperations } from "../host-ops/host-operations-store";
import { startOffer, startRefusalText, statusOrUnknown, type StartOffer } from "../host-ops/start-rule";
import type { PriorState } from "../host-ops/types";
import type { ResolvedPrinter } from "../printers/types";
import { refreshQueue, startJob } from "../queue/queue-store";
import type { Job } from "../queue/types";
import { HostOperationAlert } from "./HostOperationAlert";
import styles from "./JobDialogs.module.css";

export interface StartJobDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  job: Job;
  /** The Job's Printer, for its live status. Without one, its state is
   *  unknown and nothing is offered. */
  printer: ResolvedPrinter | undefined;
  returnFocus?: () => HTMLElement | null | undefined;
}

/** The two errors after which the operator must look at the bed again for
 *  the state the printer is in now (P6 D9, "Start dialog"). */
const RECONFIRM_CODES = new Set(["START_PRECONDITION_CHANGED", "START_NOT_ALLOWED"]);

/** **Start…** for a Job (D7 `start_job`): a Kobalte alert dialog with P6's
 *  bed-clear checkbox, its label `startOffer`'s `confirmLabel` for the
 *  Printer's live state ("The previous print finished. The bed is clear."
 *  and so on). Confirm stays disabled until it is ticked, and while Rust's
 *  `startBlockers` list anything. A tick belongs to the prior state it was
 *  given for: any change in what is offered clears it (as P6's own Start
 *  dialog does). */
export function StartJobDialog(props: StartJobDialogProps) {
  const [ticked, setTicked] = createSignal(false);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);

  const offer = (): StartOffer => startOffer(
    statusOrUnknown(props.printer?.runtimeStatus),
    hostOperations.unresolvedFor(props.job.printerId) !== undefined,
  );
  const offered = () => {
    const current = offer();
    return current.offered ? current : undefined;
  };
  const refusal = () => {
    const current = offer();
    return current.offered ? "" : current.reason;
  };
  const blocked = () => props.job.startBlockers.length > 0;

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setTicked(false);
    setError(null);
  }));

  const offerKey = createMemo(() => {
    const current = offer();
    return current.offered ? `offered:${current.priorState}` : `refused:${current.reason}`;
  });
  createEffect(on(offerKey, () => setTicked(false), { defer: true }));

  const canConfirm = () => Boolean(offered()) && ticked() && !blocked() && !pending();

  async function onConfirm() {
    const current = offered();
    if (!current || !canConfirm()) return;
    const priorState: PriorState = current.priorState;
    setPending(true);
    setError(null);
    try {
      await startJob(props.job.id, priorState);
      props.onOpenChange(false);
    } catch (e) {
      if (isCommandError(e) && RECONFIRM_CODES.has(e.code)) setTicked(false);
      setError(e);
    } finally {
      setPending(false);
    }
  }

  return (
    <AlertDialog title="Start this Job?" open={props.open} onOpenChange={props.onOpenChange} returnFocus={props.returnFocus}>
      <div class={styles.body}>
        <p class={styles.text}>
          Start <span class={styles.file}>{props.job.hostPath ?? "the staged file"}</span> on{" "}
          {props.printer?.name ?? props.job.printerSnapshot.name}.
        </p>
        <Show when={blocked()}>
          <ul class={styles.list} aria-label="Why it can't start">
            <For each={props.job.startBlockers}>{(blocker) => <li class={styles.reason}>{blocker.message}</li>}</For>
          </ul>
        </Show>
        <Show when={!blocked()}>
          <Show when={offered()} fallback={<p class={styles.reason}>{startRefusalText(refusal())}</p>}>
            {(current) => (
              <Checkbox checked={ticked()} disabled={pending()} onChange={setTicked}>
                {current().confirmLabel}
              </Checkbox>
            )}
          </Show>
        </Show>
        <Show when={pending()}>
          <p class={styles.text} role="status">Checking the file on the printer…</p>
        </Show>
        <Show when={error()}>
          {(held) => (
            <HostOperationAlert
              error={held()}
              fallback="The Job could not be started."
              printerId={props.job.printerId}
              onOpenJob={() => props.onOpenChange(false)}
              onReload={refreshQueue}
            />
          )}
        </Show>
        <div class={styles.footer}>
          <Show when={offered() && !blocked() && !ticked() && !pending()}>
            <p class={styles.reason}>Tick the confirmation to start.</p>
          </Show>
          <div class={styles.actions}>
            <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
            <Button variant="primary" disabled={!canConfirm()} onClick={() => void onConfirm()}>Start print</Button>
          </div>
        </div>
      </div>
    </AlertDialog>
  );
}
