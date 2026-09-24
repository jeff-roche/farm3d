import { createEffect, createMemo, createSignal, on, Show } from "solid-js";
import { Button, Dialog, NumberField, RadioGroup, Select, Textarea } from "../design-system";
import { isCommandError } from "../ipc/client";
import { formatGrams } from "../spools/weight";
import { recordAmount, spoolState } from "../spools/spool-store";
import type { AmountConfidence } from "../generated/contracts/domain/AmountConfidence";
import type { AmountEntry } from "../generated/contracts/domain/AmountEntry";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { Tare } from "../generated/contracts/domain/Tare";
import styles from "./RecordAmountDialog.module.css";

export interface RecordAmountDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  spool: SpoolRecord;
  onRecorded?: (spool: SpoolRecord) => void;
}

type AmountMode = "net" | "scale";

/** D1: converts a NumberField's grams value (at most one decimal, per its
 *  `step`) to integer milligrams. Rounds to the nearest tenth of a gram
 *  first, the same integer-only approach `weight.ts`'s `parseGrams` uses,
 *  so float artifacts (e.g. 612.3 * 1000) never leak into the mg value. */
function gramsToMg(grams: number): number {
  return Math.round(grams * 10) * 100;
}

const NO_TARE = "__none__";

/** D3/D7: **Net** / **Scale** segmented choice (spec §Components). Net
 *  records a direct grams value at a chosen confidence; Scale weighs gross
 *  against a tare and previews the resulting net live, and always records
 *  as `measured`. Gross-below-tare (D3) is checked client-side against the
 *  already-loaded tare list before submit is even enabled -- a UX nicety
 *  that catches the common case instantly -- but `recordAmount` itself now
 *  rejects (fix round 1 ruling), so a `VALIDATION` on `entry.grossMg` that
 *  only Rust could catch (e.g. a tare edited concurrently) still renders
 *  inline here rather than being lost to the store banner behind this
 *  dialog's own overlay. */
export function RecordAmountDialog(props: RecordAmountDialogProps) {
  const [mode, setMode] = createSignal<AmountMode>("net");
  const [netGrams, setNetGrams] = createSignal<number | undefined>(undefined);
  const [confidence, setConfidence] = createSignal<AmountConfidence>("measured");
  const [grossGrams, setGrossGrams] = createSignal<number | undefined>(undefined);
  const [tareId, setTareId] = createSignal<string>(NO_TARE);
  const [note, setNote] = createSignal("");
  const [submitting, setSubmitting] = createSignal(false);
  const [dialogError, setDialogError] = createSignal<string | null>(null);
  const [serverFieldError, setServerFieldError] = createSignal<{ path: string; message: string } | null>(null);

  createEffect(on(() => props.open, (open) => {
    if (!open) return;
    setMode("net");
    setNetGrams(undefined);
    setConfidence("measured");
    setGrossGrams(undefined);
    setTareId(props.spool.tareId ?? NO_TARE);
    setNote("");
    setDialogError(null);
    setServerFieldError(null);
  }));

  const serverFieldErrorFor = (path: string): string | undefined => (
    serverFieldError()?.path === path ? serverFieldError()!.message : undefined
  );

  /** Fix round 2: a server field error is display-only -- it must never
   *  gate `canSubmit` on its own (only client-side validity does), and it
   *  clears the moment the user edits any input that field's validation
   *  depends on. Without this, a rejected `entry.grossMg` (say) would
   *  leave `serverFieldError` set forever -- `onSubmit` only clears it at
   *  the *start* of a submit, and the button was disabled by the very
   *  error that submit is supposed to clear, so it could never run again. */
  function clearServerFieldError(path: string): void {
    if (serverFieldError()?.path === path) setServerFieldError(null);
  }

  const onNetGramsChange = (value: number | undefined) => {
    setNetGrams(value);
    clearServerFieldError("entry.netMg");
  };
  const onGrossGramsChange = (value: number | undefined) => {
    setGrossGrams(value);
    clearServerFieldError("entry.grossMg");
  };
  const onTareIdChange = (value: string) => {
    setTareId(value);
    // The tare's weight is half of what makes gross valid (D3) -- changing
    // it can resolve (or newly create) a gross-below-tare condition, so it
    // clears the same server error the gross field does.
    clearServerFieldError("entry.grossMg");
  };

  const tareOptions = createMemo<string[]>(() => [NO_TARE, ...spoolState.tares.map((t) => t.id)]);
  const tareLabel = (id: string): string => {
    if (id === NO_TARE) return "No tare";
    const tare = spoolState.tares.find((t: Tare) => t.id === id);
    return tare ? `${tare.name} (${formatGrams(tare.weightMg, 1)})` : id;
  };
  const selectedTareMg = createMemo(() => {
    const tare = spoolState.tares.find((t: Tare) => t.id === tareId());
    return tare?.weightMg ?? 0;
  });
  const netPreviewMg = createMemo<number | null>(() => {
    const gross = grossGrams();
    if (gross === undefined) return null;
    return gramsToMg(gross) - selectedTareMg();
  });
  /** Client-known only -- the sole thing that gates `canSubmit` (fix round
   *  2: a server error must never gate submit on its own, only display). */
  const clientGrossError = createMemo<string | undefined>(() => {
    const preview = netPreviewMg();
    if (preview === null) return undefined;
    return preview < 0 ? "The gross weight is less than the tare." : undefined;
  });
  /** What the Gross field's `error` prop shows: the client check first,
   *  then a still-live server rejection. */
  const grossError = createMemo<string | undefined>(() => clientGrossError() ?? serverFieldErrorFor("entry.grossMg"));

  const canSubmit = createMemo(() => {
    if (submitting()) return false;
    if (mode() === "net") return netGrams() !== undefined && netGrams()! >= 0;
    return grossGrams() !== undefined && grossGrams()! >= 0 && !clientGrossError();
  });

  async function onSubmit() {
    if (!canSubmit()) return;
    const entry: AmountEntry = mode() === "net"
      ? { kind: "net", netMg: gramsToMg(netGrams()!), confidence: confidence() }
      : { kind: "scale", grossMg: gramsToMg(grossGrams()!), ...(tareId() !== NO_TARE ? { tareId: tareId() } : { tareMg: 0 }) };
    setSubmitting(true);
    setDialogError(null);
    setServerFieldError(null);
    try {
      const result = await recordAmount(props.spool.id, entry, note().trim() || undefined);
      props.onOpenChange(false);
      props.onRecorded?.(result);
    } catch (e) {
      if (isCommandError(e) && e.code === "CONFLICT") {
        setDialogError("This Spool changed since you opened it. It's been reloaded with the current values — check them and try again.");
      } else if (isCommandError(e) && typeof e.details?.fieldPath === "string" && (e.details.fieldPath === "entry.grossMg" || e.details.fieldPath === "entry.netMg")) {
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
        <RadioGroup
          label="Entry method"
          options={[{ value: "net", label: "Net" }, { value: "scale", label: "Scale" }]}
          value={mode()}
          onChange={(v) => setMode(v as AmountMode)}
        />
        <Show when={mode() === "net"}>
          <NumberField
            label="Net weight (g)"
            value={netGrams()}
            onChange={onNetGramsChange}
            minValue={0}
            maxValue={50_000}
            step={0.1}
            suffix="g"
            error={serverFieldErrorFor("entry.netMg")}
          />
          <RadioGroup
            label="Confidence"
            options={[{ value: "measured", label: "I measured it" }, { value: "estimated", label: "This is an estimate" }]}
            value={confidence()}
            onChange={(v) => setConfidence(v as AmountConfidence)}
          />
        </Show>
        <Show when={mode() === "scale"}>
          <Select
            label="Tare"
            options={tareOptions()}
            value={tareId()}
            onChange={onTareIdChange}
            optionLabel={tareLabel}
          />
          <NumberField
            label="Gross weight (g)"
            value={grossGrams()}
            onChange={onGrossGramsChange}
            minValue={0}
            maxValue={55_000}
            step={0.1}
            suffix="g"
            error={grossError()}
          />
          <Show when={netPreviewMg() !== null}>
            <p class={styles.preview}>Net: {formatGrams(netPreviewMg()!, 1)}</p>
          </Show>
        </Show>
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
