import { createEffect, createSignal, createUniqueId, on, Show } from "solid-js";
import { Button, Dialog, NumberField, RadioGroup, Select, TextField } from "../design-system";
import { isCommandError } from "../ipc/client";
import { printers } from "../printers/printer-store";
import type { ResolvedPrinter } from "../printers/types";
import { dispatchPolicyLabel, dispatchPreferenceLabel } from "../queue/presentation";
import { addToQueue, type AddToQueueOptions } from "../queue/queue-store";
import type { DispatchPolicy, DispatchPreference, MaterialEstimate } from "../queue/types";
import { formatFilamentWeight, NEEDS_MANUAL_PRINTER, revisionTitle } from "../slicing/revision-presentation";
import type { SliceRevisionRecord } from "../slicing/types";
import { gramsToMgRoundUp, parseGrams } from "../spools/weight";
import styles from "./AddToQueueDialog.module.css";

export interface AddToQueueDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  revision: SliceRevisionRecord;
  /** Called with the first new entry's id (the lowest `copyIndex`) once
   *  the entries exist. */
  onAdded: (firstEntryId: string) => void;
  returnFocus?: () => HTMLElement | null | undefined;
}

const MAX_COPIES = 50;
const POLICIES: DispatchPolicy[] = ["manual", "recommended", "automatic"];
const PREFERENCES: DispatchPreference[] = ["loadedFirst", "leastRecentlyUsed"];

const POLICY_HINT: Record<DispatchPolicy, string> = {
  manual: "You choose the Printer and Spool.",
  recommended: "farm3d ranks the Printers; you confirm the assignment.",
  automatic: "farm3d assigns the first qualifying idle Printer.",
};

/** How the operator gives an estimate the revision doesn't have. */
type EstimateChoice = "claim" | "entered";

/** **Add to Queue…** (spec "Commands", `add_to_queue`): quantity, Dispatch
 *  Policy, preference, the material estimate, and a manual Printer when the
 *  revision needs one. A farm3d estimate is shown and left for Rust to fill
 *  in; an External or estimate-less revision needs the operator to accept
 *  the file's claimed grams or enter an amount. Rust validates everything
 *  again; the reasons here only keep Add from sending what it would refuse. */
export function AddToQueueDialog(props: AddToQueueDialogProps) {
  const reasonId = createUniqueId();
  const [quantity, setQuantity] = createSignal(1);
  const [policy, setPolicy] = createSignal<DispatchPolicy>("recommended");
  const [preference, setPreference] = createSignal<DispatchPreference>("loadedFirst");
  const [choice, setChoice] = createSignal<EstimateChoice | null>(null);
  const [amountText, setAmountText] = createSignal("");
  const [printerId, setPrinterId] = createSignal<string | null>(null);
  const [pending, setPending] = createSignal(false);
  const [error, setError] = createSignal<string | null>(null);

  const needsManualPrinter = () => props.revision.requiresManualPrinterSelection;
  const sliceGrams = () => (props.revision.kind === "farm3d" ? props.revision.estimates?.filamentGrams ?? null : null);
  const claimGrams = () => props.revision.claimedEstimates?.filamentGrams ?? null;
  const needsEstimate = () => sliceGrams() === null;
  /** With no claim to accept, entering an amount is the only way. */
  const effectiveChoice = (): EstimateChoice | null => (claimGrams() === null ? "entered" : choice());

  createEffect(on(() => [props.open, props.revision.id] as const, ([open]) => {
    if (!open) return;
    setQuantity(1);
    setPolicy(needsManualPrinter() ? "manual" : "recommended");
    setPreference("loadedFirst");
    setChoice(null);
    setAmountText("");
    setPrinterId(null);
    setError(null);
  }));

  const quantityValid = () => Number.isInteger(quantity()) && quantity() >= 1 && quantity() <= MAX_COPIES;
  const enteredMg = () => {
    const parsed = parseGrams(amountText());
    return parsed.ok && parsed.mg > 0 ? parsed.mg : null;
  };

  const materialEstimate = (): MaterialEstimate | null | undefined => {
    if (!needsEstimate()) return undefined;
    const claim = claimGrams();
    if (effectiveChoice() === "claim" && claim !== null) {
      return { amountMg: gramsToMgRoundUp(claim), source: "fileClaimConfirmed" };
    }
    if (effectiveChoice() === "entered") {
      const mg = enteredMg();
      return mg === null ? null : { amountMg: mg, source: "operatorEntered" };
    }
    return null;
  };

  const printerOptions = () => printers().filter((printer) => !printer.archivedAt);
  const chosenPrinter = () => printerOptions().find((printer) => printer.id === printerId()) ?? null;

  /** Why Add is disabled, in visible words, or `undefined`. */
  const blockedReason = (): string | undefined => {
    if (!quantityValid()) return `Choose between 1 and ${MAX_COPIES} copies.`;
    if (materialEstimate() === null) {
      if (effectiveChoice() === null) return "Accept the file's claim or enter the material this print uses.";
      if (amountText().trim() === "") return "Enter the material this print uses.";
      return "Enter a weight above 0 g, to at most one decimal.";
    }
    if (needsManualPrinter() && !chosenPrinter()) return "Choose the Printer this runs on.";
    return undefined;
  };

  const addLabel = () => {
    if (!quantityValid()) return "Add to Queue";
    return quantity() === 1 ? "Add 1 copy" : `Add ${quantity()} copies`;
  };

  async function submit() {
    if (blockedReason() || pending()) return;
    const options: AddToQueueOptions = {};
    const estimate = materialEstimate();
    if (estimate) options.materialEstimate = estimate;
    const printer = chosenPrinter();
    if (needsManualPrinter() && printer) options.manualPrinterId = printer.id;
    setPending(true);
    setError(null);
    try {
      const change = await addToQueue(props.revision.id, quantity(), policy(), preference(), options);
      const first = [...change.entries].sort((a, b) => a.copyIndex - b.copyIndex)[0];
      props.onOpenChange(false);
      if (first) props.onAdded(first.id);
    } catch (e) {
      setError(isCommandError(e) ? e.message : "The Queue Entries couldn't be added.");
    } finally {
      setPending(false);
    }
  }

  return (
    <Dialog
      title="Add to Queue"
      description={revisionTitle(props.revision)}
      open={props.open}
      onOpenChange={props.onOpenChange}
      returnFocus={props.returnFocus}
    >
      <form
        class={styles.body}
        onSubmit={(event) => {
          event.preventDefault();
          void submit();
        }}
      >
        <NumberField
          label="Copies"
          value={quantity()}
          onChange={setQuantity}
          minValue={1}
          maxValue={MAX_COPIES}
          step={1}
          disabled={pending()}
        />

        <RadioGroup
          label="Dispatch Policy"
          options={POLICIES.map((value) => ({
            value,
            label: dispatchPolicyLabel(value),
            disabled: value !== "manual" && needsManualPrinter(),
          }))}
          value={policy()}
          onChange={(value) => setPolicy(value as DispatchPolicy)}
          disabled={pending()}
        />
        <p class={styles.muted}>
          {needsManualPrinter() ? `${NEEDS_MANUAL_PRINTER}: this revision is assigned by hand.` : POLICY_HINT[policy()]}
        </p>

        <Select<DispatchPreference>
          label="Dispatch preference"
          options={PREFERENCES}
          value={preference()}
          onChange={setPreference}
          optionLabel={dispatchPreferenceLabel}
          disabled={pending()}
        />

        <div class={styles.group} role="group" aria-label="Material estimate">
          <span class={styles.groupLabel}>Material estimate</span>
          <Show
            when={needsEstimate()}
            fallback={<p class={styles.text}>{formatFilamentWeight(sliceGrams()!)} from the slice. Every copy reserves this much.</p>}
          >
            <Show
              when={claimGrams()}
              fallback={<p class={styles.muted}>This revision doesn't say how much material it uses.</p>}
            >
              {(claim) => (
                <RadioGroup
                  label="Estimate source"
                  options={[
                    { value: "claim", label: `Use the file's claim (${formatFilamentWeight(claim())})` },
                    { value: "entered", label: "Enter an amount" },
                  ]}
                  value={choice() ?? undefined}
                  onChange={(value) => setChoice(value as EstimateChoice)}
                  disabled={pending()}
                />
              )}
            </Show>
            <Show when={effectiveChoice() === "entered"}>
              <TextField
                label="Material (g)"
                value={amountText()}
                onChange={setAmountText}
                disabled={pending()}
              />
            </Show>
          </Show>
        </div>

        <Show when={needsManualPrinter()}>
          <Select<ResolvedPrinter>
            label="Printer"
            placeholder="Choose a Printer"
            options={printerOptions()}
            value={chosenPrinter()}
            onChange={(printer) => setPrinterId(printer.id)}
            optionValue={(printer) => printer.id}
            optionLabel={(printer) => printer.name}
            disabled={pending()}
          />
        </Show>

        <Show when={error()}>{(message) => <p class={styles.error} role="alert">{message()}</p>}</Show>
        <Show when={blockedReason()}>{(reason) => <p id={reasonId} class={styles.muted}>{reason()}</p>}</Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
          <Button
            type="submit"
            variant="primary"
            disabled={Boolean(blockedReason()) || pending()}
            aria-describedby={blockedReason() ? reasonId : undefined}
          >
            {addLabel()}
          </Button>
        </div>
      </form>
    </Dialog>
  );
}
