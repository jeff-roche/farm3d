import { createEffect, createMemo, createSignal, on, Show } from "solid-js";
import { Button, Checkbox, Dialog } from "../design-system";
import { isCommandError } from "../ipc/client";
import { hostOperations, stageSliceRevision, startStagedArtifact } from "../host-ops/host-operations-store";
import { startOffer, startRefusalText, statusOrUnknown, type StartOffer } from "../host-ops/start-rule";
import type { HostOperation, PriorState } from "../host-ops/types";
import type { ResolvedPrinter } from "../printers/types";
import { HostOperationAlert } from "./HostOperationAlert";
import styles from "./StartStagedDialog.module.css";

export interface StartStagedDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  printer: ResolvedPrinter;
  /** The succeeded upload (the staged artifact) to start. */
  staged: HostOperation;
  returnFocus?: () => HTMLElement | null | undefined;
}

/** The two errors after which the operator must look at the bed again for
 *  the state the printer is in now (spec D9, "Start dialog"). */
const RECONFIRM_CODES = new Set(["START_PRECONDITION_CHANGED", "START_NOT_ALLOWED"]);

/** D9's Start dialog. The checkbox label is `startOffer`'s `confirmLabel`
 *  for the Printer's live status, and Confirm stays disabled until it is
 *  ticked — for both Start-safety values: P6 never starts unattended. A
 *  tick belongs to the `priorState` it was given for, so a status change
 *  (or a `START_PRECONDITION_CHANGED`/`START_NOT_ALLOWED` rejection)
 *  never carries it over. */
export function StartStagedDialog(props: StartStagedDialogProps) {
  const [ticked, setTicked] = createSignal(false);
  const [restaged, setRestaged] = createSignal(false);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);
  const [staging, setStaging] = createSignal(false);

  const offer = (): StartOffer =>
    startOffer(statusOrUnknown(props.printer.runtimeStatus), hostOperations.unresolvedFor(props.printer.id) !== undefined);
  const offered = () => {
    const current = offer();
    return current.offered ? current : undefined;
  };
  const refusal = () => {
    const current = offer();
    return current.offered ? undefined : current.reason;
  };

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setTicked(false);
    setRestaged(false);
    setError(null);
  }));

  // A tick is never reused (spec D9): any change in what is offered —
  // including a round trip such as ready → printing → ready, or a stale
  // blip — clears it, so the bed is confirmed again for what the printer
  // reports now. Telemetry-only updates leave the offer, and the tick, alone.
  // A memo, so only a real change of offer (not every status push) clears
  // the tick: `on` alone reruns whenever its source's dependencies fire.
  const offerKey = createMemo(() => {
    const current = offer();
    return current.offered ? `offered:${current.priorState}` : `refused:${current.reason}`;
  });
  createEffect(on(offerKey, () => setTicked(false), { defer: true }));

  const errorCode = () => {
    const held = error();
    return isCommandError(held) ? held.code : undefined;
  };

  async function onConfirm() {
    const current = offered();
    if (!current || !ticked() || pending()) return;
    const priorState: PriorState = current.priorState;
    setPending(true);
    setError(null);
    try {
      await startStagedArtifact(props.printer.id, props.staged.id, priorState);
      props.onOpenChange(false);
    } catch (e) {
      if (isCommandError(e) && RECONFIRM_CODES.has(e.code)) setTicked(false);
      setError(e);
    } finally {
      setPending(false);
    }
  }

  async function onStageAgain() {
    const sliceRevisionId = props.staged.sliceRevisionId;
    if (!sliceRevisionId || staging()) return;
    setStaging(true);
    try {
      await stageSliceRevision(props.printer.id, sliceRevisionId);
      setError(null);
      setRestaged(true);
    } catch (e) {
      setError(e);
    } finally {
      setStaging(false);
    }
  }

  return (
    <Dialog
      title="Start print"
      open={props.open}
      onOpenChange={props.onOpenChange}
      returnFocus={props.returnFocus}
    >
      <div class={styles.body}>
        <Show
          when={!restaged()}
          fallback={
            <>
              <p class={styles.lead} role="status">
                Uploading again to {props.printer.name}. The Job tab shows when the file is staged, then you can start it.
              </p>
              <div class={styles.footer}>
                <div class={styles.actions}>
                  <Button variant="primary" onClick={() => props.onOpenChange(false)}>Done</Button>
                </div>
              </div>
            </>
          }
        >
            <p class={styles.lead}>
              Start <span class={styles.file}>{props.staged.hostPath}</span> on {props.printer.name}.
            </p>
            <Show
              when={offered()}
              fallback={<p class={styles.reason}>{startRefusalText(refusal() ?? "")}</p>}
            >
              {(current) => (
                <Checkbox checked={ticked()} disabled={pending()} onChange={setTicked}>
                  {current().confirmLabel}
                </Checkbox>
              )}
            </Show>
            <Show when={pending()}>
              <p class={styles.progress} role="status">Checking the file on the printer…</p>
            </Show>
            <Show when={error()}>
              {(held) => (
                <HostOperationAlert error={held()} fallback="The print could not be started." printerId={props.printer.id} onOpenJob={() => props.onOpenChange(false)}>
                  <Show when={errorCode() === "STAGED_ARTIFACT_INVALID"}>
                    <Show
                      when={props.staged.sliceRevisionId}
                      fallback={<span class={styles.reason}>Its Slice Revision was deleted, so farm3d can't stage it again.</span>}
                    >
                      <Button variant="secondary" size="sm" disabled={staging()} onClick={() => void onStageAgain()}>
                        Stage again
                      </Button>
                    </Show>
                  </Show>
                </HostOperationAlert>
              )}
            </Show>
            <div class={styles.footer}>
              <Show when={offered() && !ticked() && !pending()}>
                <p class={styles.reason}>Tick the confirmation to start.</p>
              </Show>
              <div class={styles.actions}>
                <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
                <Button variant="primary" disabled={!ticked() || pending()} onClick={() => void onConfirm()}>
                  Start print
                </Button>
              </div>
            </div>
        </Show>
      </div>
    </Dialog>
  );
}
