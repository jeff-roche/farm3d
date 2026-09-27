import { createEffect, createMemo, createSignal, Show } from "solid-js";
import { NumberField, RadioGroup, Select } from "../design-system";
import { formatGrams } from "../spools/weight";
import { spoolState } from "../spools/spool-store";
import type { AmountConfidence } from "../generated/contracts/domain/AmountConfidence";
import type { AmountEntry } from "../generated/contracts/domain/AmountEntry";
import type { Tare } from "../generated/contracts/domain/Tare";
import styles from "./RecordAmountDialog.module.css";

/** The field paths a backend `VALIDATION` can name for an amount entry. */
export type AmountEntryFieldPath = "entry.netMg" | "entry.grossMg";

export interface AmountEntryFieldsProps {
  /** The tare the Scale mode starts on (the Spool's own, usually). */
  defaultTareId?: string | null;
  /** Net mode asks how sure the operator is. Off where the entry must be a
   *  measurement (a Job's measured settlement), which then sends
   *  `confidence: "measured"`. */
  showConfidence?: boolean;
  netLabel?: string;
  /** A backend rejection to show under its field. Display-only: it never
   *  makes the entry invalid on its own. */
  fieldErrors?: Partial<Record<AmountEntryFieldPath, string>>;
  /** Called when the operator edits an input a field error depends on, so
   *  the caller can drop that (now stale) error. */
  onEdit?: (path: AmountEntryFieldPath) => void;
  /** The entry as typed, or `null` while it's incomplete or invalid. */
  onChange: (entry: AmountEntry | null) => void;
  disabled?: boolean;
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

/** P3's amount entry (D3/D7): a **Net** / **Scale** choice. Net records a
 *  direct grams value at a chosen confidence; Scale weighs gross against a
 *  tare, previews the resulting net live, and always records as measured.
 *  Gross-below-tare (D3) is checked here against the loaded tare list, so
 *  the caller's submit stays disabled until it's fixed. Shared by
 *  `RecordAmountDialog` and P7's settlement and correction dialogs. State
 *  lives here for as long as the fields are mounted (a dialog's content
 *  mounts on open, so each opening starts fresh). */
export function AmountEntryFields(props: AmountEntryFieldsProps) {
  const [mode, setMode] = createSignal<AmountMode>("net");
  const [netGrams, setNetGrams] = createSignal<number | undefined>(undefined);
  const [confidence, setConfidence] = createSignal<AmountConfidence>("measured");
  const [grossGrams, setGrossGrams] = createSignal<number | undefined>(undefined);
  const [tareId, setTareId] = createSignal<string>(props.defaultTareId ?? NO_TARE);

  const onNetGramsChange = (value: number | undefined) => {
    setNetGrams(value);
    props.onEdit?.("entry.netMg");
  };
  const onGrossGramsChange = (value: number | undefined) => {
    setGrossGrams(value);
    props.onEdit?.("entry.grossMg");
  };
  const onTareIdChange = (value: string) => {
    setTareId(value);
    // The tare's weight is half of what makes gross valid (D3) -- changing
    // it can resolve (or newly create) a gross-below-tare condition, so it
    // clears the same server error the gross field does.
    props.onEdit?.("entry.grossMg");
  };

  const tareOptions = createMemo<string[]>(() => [NO_TARE, ...spoolState.tares.map((t) => t.id)]);
  const tareLabel = (id: string): string => {
    if (id === NO_TARE) return "No tare";
    const tare = spoolState.tares.find((t: Tare) => t.id === id);
    return tare ? `${tare.name} (${formatGrams(tare.weightMg, 1)})` : id;
  };
  const selectedTareMg = createMemo(() => spoolState.tares.find((t: Tare) => t.id === tareId())?.weightMg ?? 0);
  const netPreviewMg = createMemo<number | null>(() => {
    const gross = grossGrams();
    return gross === undefined ? null : gramsToMg(gross) - selectedTareMg();
  });
  /** Client-known only -- the sole thing that makes the entry invalid (a
   *  server error only displays). */
  const clientGrossError = createMemo<string | undefined>(() => {
    const preview = netPreviewMg();
    return preview !== null && preview < 0 ? "The gross weight is less than the tare." : undefined;
  });

  const entry = createMemo<AmountEntry | null>(() => {
    if (mode() === "net") {
      const net = netGrams();
      if (net === undefined || net < 0) return null;
      return { kind: "net", netMg: gramsToMg(net), confidence: props.showConfidence === false ? "measured" : confidence() };
    }
    const gross = grossGrams();
    if (gross === undefined || gross < 0 || clientGrossError()) return null;
    return { kind: "scale", grossMg: gramsToMg(gross), ...(tareId() !== NO_TARE ? { tareId: tareId() } : { tareMg: 0 }) };
  });
  createEffect(() => props.onChange(entry()));

  return (
    <>
      <RadioGroup
        label="Entry method"
        options={[{ value: "net", label: "Net" }, { value: "scale", label: "Scale" }]}
        value={mode()}
        onChange={(v) => setMode(v as AmountMode)}
        disabled={props.disabled}
      />
      <Show when={mode() === "net"}>
        <NumberField
          label={props.netLabel ?? "Net weight (g)"}
          value={netGrams()}
          onChange={onNetGramsChange}
          minValue={0}
          maxValue={50_000}
          step={0.1}
          suffix="g"
          error={props.fieldErrors?.["entry.netMg"]}
          disabled={props.disabled}
        />
        <Show when={props.showConfidence !== false}>
          <RadioGroup
            label="Confidence"
            options={[{ value: "measured", label: "I measured it" }, { value: "estimated", label: "This is an estimate" }]}
            value={confidence()}
            onChange={(v) => setConfidence(v as AmountConfidence)}
            disabled={props.disabled}
          />
        </Show>
      </Show>
      <Show when={mode() === "scale"}>
        <Select
          label="Tare"
          options={tareOptions()}
          value={tareId()}
          onChange={onTareIdChange}
          optionLabel={tareLabel}
          disabled={props.disabled}
        />
        <NumberField
          label="Gross weight (g)"
          value={grossGrams()}
          onChange={onGrossGramsChange}
          minValue={0}
          maxValue={55_000}
          step={0.1}
          suffix="g"
          error={clientGrossError() ?? props.fieldErrors?.["entry.grossMg"]}
          disabled={props.disabled}
        />
        <Show when={netPreviewMg() !== null}>
          <p class={styles.preview}>Net: {formatGrams(netPreviewMg()!, 1)}</p>
        </Show>
      </Show>
    </>
  );
}
