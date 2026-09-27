import { createEffect, createSignal, on, Show } from "solid-js";
import { Button, Dialog, RadioGroup } from "../design-system";
import { jobStateLabel } from "../queue/presentation";
import { correctJobMaterial, refreshQueue, settleJobMaterial } from "../queue/queue-store";
import { settlementPreviewText } from "../queue/settlement";
import type { AmountEntry, Job, SettleChoice } from "../queue/types";
import { ensureInventoryLoaded, spoolState } from "../spools/spool-store";
import { formatGrams } from "../spools/weight";
import { AmountEntryFields } from "./AmountEntryFields";
import { HostOperationAlert } from "./HostOperationAlert";
import styles from "./JobDialogs.module.css";

export interface SettleMaterialDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** A failed or cancelled Job to settle, or a completed one to correct. */
  job: Job;
  returnFocus?: () => HTMLElement | null | undefined;
}

type Choice = SettleChoice["kind"];

/** The Spool as the operator knows it ("Spool #9"), from the inventory
 *  once it's loaded ("the Spool" until then). */
export function spoolName(spoolId: string): string {
  const spool = spoolState.spools.find((candidate) => candidate.id === spoolId);
  return spool ? `Spool #${spool.spoolNumber}` : "the Spool";
}

/** Owner decisions 5 and 6 (spec "Material settlement"). A failed or
 *  cancelled Job settles once: **Use estimate (N g)** (Rust's
 *  `settlementPreview`, shown before confirming), **Enter measured
 *  remaining weight** (P3's net or scale+tare entry), or **Defer**, which
 *  keeps the reserved amount unavailable. A completed Job already deducted
 *  its estimate; here it takes the one optional measured correction. */
export function SettleMaterialDialog(props: SettleMaterialDialogProps) {
  const [choice, setChoice] = createSignal<Choice | undefined>();
  const [entry, setEntry] = createSignal<AmountEntry | null>(null);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<unknown>(null);

  const correcting = () => props.job.state === "completed";
  const preview = () => settlementPreviewText(props.job.settlementPreview);
  const reserved = () => formatGrams(props.job.estimateMg, 1);
  const spool = () => spoolName(props.job.spoolId);
  /** Defer is for a failed or cancelled Job still `pending` (the spec's
   *  settlement table); an already-deferred one settles or stays as is. */
  const deferOffered = () =>
    (props.job.state === "failed" || props.job.state === "cancelled") && props.job.settlement === "pending";

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setChoice(correcting() ? "measured" : undefined);
    setEntry(null);
    setError(null);
    // P3's Scale entry lists the Farm's tares.
    ensureInventoryLoaded().catch(() => undefined);
  }));

  const settleChoice = (): SettleChoice | undefined => {
    switch (choice()) {
      case "estimated": return { kind: "estimated" };
      case "defer": return { kind: "defer" };
      case "measured": {
        const held = entry();
        return held ? { kind: "measured", entry: held } : undefined;
      }
      default: return undefined;
    }
  };
  const canConfirm = () => settleChoice() !== undefined && !pending();

  async function onConfirm() {
    const current = settleChoice();
    if (!current || !canConfirm()) return;
    setPending(true);
    setError(null);
    try {
      if (correcting()) {
        if (current.kind === "measured") await correctJobMaterial(props.job.id, current.entry);
      } else {
        await settleJobMaterial(props.job.id, current);
      }
      props.onOpenChange(false);
    } catch (e) {
      setError(e);
    } finally {
      setPending(false);
    }
  }

  const options = () => [
    { value: "estimated", label: `Use estimate (${preview() ?? "unavailable"})`, disabled: preview() === null },
    { value: "measured", label: "Enter measured remaining weight" },
    ...(deferOffered() ? [{ value: "defer", label: "Defer" }] : []),
  ];

  return (
    <Dialog
      title={correcting() ? "Correct weight" : "Settle material"}
      open={props.open}
      onOpenChange={props.onOpenChange}
      returnFocus={props.returnFocus}
    >
      <div class={styles.body}>
        <Show
          when={correcting()}
          fallback={
            <p class={styles.text}>
              This Job {jobStateLabel(props.job.state).toLowerCase()} on {props.job.printerSnapshot.name}. Its {reserved()}{" "}
              reservation on {spool()} is unresolved until you settle it.
            </p>
          }
        >
          <p class={styles.text}>
            This Job completed, and farm3d deducted its {reserved()} estimate from {spool()}. Weigh the Spool and enter what
            remains; farm3d records the difference as a correction. A Job can be corrected once.
          </p>
        </Show>
        <Show when={props.job.settlement === "deferred"}>
          <p class={styles.note}>Settlement was deferred earlier. The reserved amount is still unavailable.</p>
        </Show>
        <Show when={!correcting()}>
          <RadioGroup
            label="Material used"
            options={options()}
            value={choice() ?? ""}
            onChange={(value) => setChoice(value as Choice)}
            disabled={pending()}
          />
        </Show>
        <Show when={choice() === "estimated"}>
          <p class={styles.note}>
            farm3d deducts {preview()}: {props.job.maxProgressPct}% of the {reserved()} estimate, the most progress the
            printer reported for this Job.
          </p>
        </Show>
        <Show when={choice() === "measured"}>
          <div class={correcting() ? undefined : styles.group}>
            <AmountEntryFields showConfidence={false} onChange={setEntry} disabled={pending()} />
          </div>
        </Show>
        <Show when={choice() === "defer"}>
          <p class={styles.note}>
            Nothing is deducted now. The reserved {reserved()} stays unavailable on {spool()} until you settle it.
          </p>
        </Show>
        <Show when={error()}>
          {(held) => <HostOperationAlert error={held()} fallback="The material couldn't be settled." onReload={refreshQueue} />}
        </Show>
        <div class={styles.footer}>
          <Show when={!correcting() && choice() === undefined}>
            <p class={styles.reason}>Choose how much material the Job used.</p>
          </Show>
          <div class={styles.actions}>
            <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
            <Button variant="primary" disabled={!canConfirm()} onClick={() => void onConfirm()}>
              {correcting() ? "Record correction" : choice() === "defer" ? "Defer" : "Settle"}
            </Button>
          </div>
        </div>
      </div>
    </Dialog>
  );
}
