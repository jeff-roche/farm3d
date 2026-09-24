import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { Button, Chip, Dialog, NumberField, RadioGroup, Select, TextField, Textarea } from "../design-system";
import { isCommandError } from "../ipc/client";
import { MATERIAL_FAMILIES, materialFamilyLabel } from "../spools/materials";
import { formatGrams } from "../spools/weight";
import { createSpool, spoolState, updateSpool } from "../spools/spool-store";
import type { AmountConfidence } from "../generated/contracts/domain/AmountConfidence";
import type { AmountEntry } from "../generated/contracts/domain/AmountEntry";
import type { FilamentDiameter } from "../generated/contracts/domain/FilamentDiameter";
import type { MaterialFamily } from "../generated/contracts/domain/MaterialFamily";
import type { SpoolFields } from "../generated/contracts/domain/SpoolFields";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import styles from "./SpoolFormDialog.module.css";

export interface SpoolFormDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
  /** Present for Edit; omitted for Add. */
  spool?: SpoolRecord;
  onSaved?: (spool: SpoolRecord) => void;
}

type AmountMode = "net" | "scale";

/** D1: converts a NumberField's grams value (at most one decimal) to
 *  integer milligrams -- see the identical helper's rationale in
 *  `RecordAmountDialog.tsx`. */
function gramsToMg(grams: number): number {
  return Math.round(grams * 10) * 100;
}

const NO_TARE = "__none__";

const NOMINAL_QUICK_PICKS: { mg: number; label: string }[] = [
  { mg: 250_000, label: "250 g" },
  { mg: 500_000, label: "500 g" },
  { mg: 750_000, label: "750 g" },
  { mg: 1_000_000, label: "1 kg" },
  { mg: 2_000_000, label: "2 kg" },
  { mg: 3_000_000, label: "3 kg" },
  { mg: 5_000_000, label: "5 kg" },
];

const DIAMETER_OPTIONS: { value: FilamentDiameter; label: string }[] = [
  { value: "1.75", label: "1.75 mm" },
  { value: "2.85", label: "2.85 mm" },
];

/** Every `SpoolFields`/initial-amount field path a Rust `VALIDATION` can
 *  name (`spools/mod.rs`'s `validate_fields`, `spools/ledger.rs`'s
 *  `resolve_entry`) that this form has a field with an `error` slot for.
 *  Anything else (e.g. `entry.tareId` -- the tare `Select` has no error
 *  slot) falls back to the dialog-level message. */
const FIELD_ERROR_PATHS = new Set([
  "manufacturer", "product", "materialOther", "colorName", "colorHex",
  "nominalMg", "lowThresholdMg", "entry.netMg", "entry.grossMg",
]);

/** Add/Edit (D2/D3/D7 §Components: `SpoolFormDialog`). Add also collects
 *  the initial amount: a nominal-weight quick pick, defaulting to an
 *  `estimated` Net entry at the nominal weight until "I weighed it"
 *  switches it to a measured Net or Scale entry (D7: "A new sealed Spool
 *  is usually entered at its nominal weight as `estimated`"). */
export function SpoolFormDialog(props: SpoolFormDialogProps) {
  const isEdit = () => props.spool !== undefined;

  const [manufacturer, setManufacturer] = createSignal("");
  const [product, setProduct] = createSignal("");
  const [family, setFamily] = createSignal<MaterialFamily>("PLA");
  const [materialOther, setMaterialOther] = createSignal("");
  const [colorName, setColorName] = createSignal("");
  const [colorHex, setColorHex] = createSignal("");
  const [diameter, setDiameter] = createSignal<FilamentDiameter>("1.75");
  const [nominalGrams, setNominalGrams] = createSignal<number | undefined>(1000);
  const [lowThresholdGrams, setLowThresholdGrams] = createSignal<number | undefined>(100);
  const [tareId, setTareId] = createSignal<string>(NO_TARE);
  const [notes, setNotes] = createSignal("");
  const [storageLabel, setStorageLabel] = createSignal("");

  const [weighed, setWeighed] = createSignal(false);
  const [amountMode, setAmountMode] = createSignal<AmountMode>("net");
  const [measuredGrams, setMeasuredGrams] = createSignal<number | undefined>(undefined);
  const [confidence, setConfidence] = createSignal<AmountConfidence>("measured");
  const [grossGrams, setGrossGrams] = createSignal<number | undefined>(undefined);

  const [submitting, setSubmitting] = createSignal(false);
  const [dialogError, setDialogError] = createSignal<string | null>(null);
  const [serverFieldError, setServerFieldError] = createSignal<{ path: string; message: string } | null>(null);

  createEffect(on(() => [props.open, props.spool] as const, ([open, spool]) => {
    if (!open) return;
    setManufacturer(spool?.manufacturer ?? "");
    setProduct(spool?.product ?? "");
    setFamily(spool?.materialFamily ?? "PLA");
    setMaterialOther(spool?.materialOther ?? "");
    setColorName(spool?.colorName ?? "");
    setColorHex(spool?.colorHex ?? "");
    setDiameter(spool?.diameter ?? "1.75");
    setNominalGrams(spool ? spool.nominalMg / 1000 : 1000);
    setLowThresholdGrams(spool ? spool.lowThresholdMg / 1000 : 100);
    setTareId(spool?.tareId ?? NO_TARE);
    setNotes(spool?.notes ?? "");
    setStorageLabel("");
    setWeighed(false);
    setAmountMode("net");
    setMeasuredGrams(undefined);
    setConfidence("measured");
    setGrossGrams(undefined);
    setDialogError(null);
    setServerFieldError(null);
  }));

  const serverFieldErrorFor = (path: string): string | undefined => (
    serverFieldError()?.path === path ? serverFieldError()!.message : undefined
  );

  /** Fix round 2: a server field error is display-only -- it must never
   *  gate `canSubmit` on its own (only client-side validity does, via
   *  `fieldsValid`/`amountValid`), and it clears the moment the user edits
   *  any input that field's validation depends on. Without this, a
   *  rejected field (say `entry.grossMg`) would leave `serverFieldError`
   *  set forever -- `onSubmit` only clears it at the *start* of a submit,
   *  and (for the amount fields) the button was disabled by the very
   *  error that submit is supposed to clear, so it could never run again. */
  function clearServerFieldError(path: string): void {
    if (serverFieldError()?.path === path) setServerFieldError(null);
  }

  const onManufacturerChange = (value: string) => { setManufacturer(value); clearServerFieldError("manufacturer"); };
  const onProductChange = (value: string) => { setProduct(value); clearServerFieldError("product"); };
  const onMaterialOtherChange = (value: string) => { setMaterialOther(value); clearServerFieldError("materialOther"); };
  const onColorNameChange = (value: string) => { setColorName(value); clearServerFieldError("colorName"); };
  const onColorHexChange = (value: string) => { setColorHex(value); clearServerFieldError("colorHex"); };
  const onNominalGramsChange = (value: number | undefined) => { setNominalGrams(value); clearServerFieldError("nominalMg"); };
  const onLowThresholdGramsChange = (value: number | undefined) => { setLowThresholdGrams(value); clearServerFieldError("lowThresholdMg"); };
  const onMeasuredGramsChange = (value: number | undefined) => { setMeasuredGrams(value); clearServerFieldError("entry.netMg"); };
  const onGrossGramsChange = (value: number | undefined) => { setGrossGrams(value); clearServerFieldError("entry.grossMg"); };
  const onTareIdChange = (value: string) => {
    setTareId(value);
    // The tare's weight is half of what makes a scale gross entry valid
    // (D3) -- changing it can resolve (or newly create) a
    // gross-below-tare condition, so it clears the same server error the
    // gross field does.
    clearServerFieldError("entry.grossMg");
  };

  const tareOptions = createMemo<string[]>(() => [NO_TARE, ...spoolState.tares.map((t) => t.id)]);
  const tareLabel = (id: string): string => {
    if (id === NO_TARE) return "No default tare";
    const tare = spoolState.tares.find((t) => t.id === id);
    return tare ? `${tare.name} (${formatGrams(tare.weightMg, 1)})` : id;
  };
  const selectedTareMg = createMemo(() => spoolState.tares.find((t) => t.id === tareId())?.weightMg ?? 0);
  const netPreviewMg = createMemo<number | null>(() => {
    if (amountMode() !== "scale" || grossGrams() === undefined) return null;
    return gramsToMg(grossGrams()!) - selectedTareMg();
  });
  /** Client-known only -- the sole thing `amountValid` gates on (fix round
   *  2: a server error must never gate submit on its own, only display). */
  const clientGrossError = createMemo<string | undefined>(() => {
    const preview = netPreviewMg();
    if (preview === null) return undefined;
    return preview < 0 ? "The gross weight is less than the tare." : undefined;
  });
  /** What the Gross field's `error` prop shows: the client check first,
   *  then a still-live server rejection. */
  const grossError = createMemo<string | undefined>(() => clientGrossError() ?? serverFieldErrorFor("entry.grossMg"));

  const materialOtherValid = () => family() !== "OTHER" || materialOther().trim().length > 0;

  const fieldsValid = createMemo(() => (
    manufacturer().trim().length > 0 &&
    colorName().trim().length > 0 &&
    materialOtherValid() &&
    nominalGrams() !== undefined && nominalGrams()! > 0 &&
    lowThresholdGrams() !== undefined
  ));

  const amountValid = createMemo(() => {
    if (isEdit()) return true;
    if (!weighed()) return true;
    if (amountMode() === "net") return measuredGrams() !== undefined && measuredGrams()! >= 0;
    return grossGrams() !== undefined && grossGrams()! >= 0 && !clientGrossError();
  });

  const canSubmit = createMemo(() => fieldsValid() && amountValid() && !submitting());

  function buildFields(): SpoolFields {
    return {
      manufacturer: manufacturer().trim(),
      ...(product().trim() ? { product: product().trim() } : {}),
      materialFamily: family(),
      ...(family() === "OTHER" ? { materialOther: materialOther().trim() } : {}),
      colorName: colorName().trim(),
      ...(colorHex().trim() ? { colorHex: colorHex().trim() } : {}),
      diameter: diameter(),
      nominalMg: gramsToMg(nominalGrams()!),
      lowThresholdMg: gramsToMg(lowThresholdGrams()!),
      ...(tareId() !== NO_TARE ? { tareId: tareId() } : {}),
      ...(notes().trim() ? { notes: notes().trim() } : {}),
    };
  }

  function buildInitialAmount(): AmountEntry {
    if (!weighed()) return { kind: "net", netMg: gramsToMg(nominalGrams()!), confidence: "estimated" };
    if (amountMode() === "net") return { kind: "net", netMg: gramsToMg(measuredGrams()!), confidence: confidence() };
    return { kind: "scale", grossMg: gramsToMg(grossGrams()!), ...(tareId() !== NO_TARE ? { tareId: tareId() } : {}) };
  }

  async function onSubmit() {
    if (!canSubmit()) return;
    setSubmitting(true);
    setDialogError(null);
    setServerFieldError(null);
    try {
      const fields = buildFields();
      const result = isEdit()
        ? await updateSpool(props.spool!.id, fields)
        : await createSpool(fields, buildInitialAmount(), storageLabel().trim() || undefined);
      props.onOpenChange(false);
      props.onSaved?.(result);
    } catch (e) {
      if (isCommandError(e) && e.code === "CONFLICT") {
        setDialogError("This Spool changed since you opened it. It's been reloaded with the current values — check them and try again.");
      } else if (isCommandError(e) && typeof e.details?.fieldPath === "string" && FIELD_ERROR_PATHS.has(e.details.fieldPath)) {
        setServerFieldError({ path: e.details.fieldPath, message: e.message });
      } else {
        setDialogError(isCommandError(e) ? e.message : "This Spool could not be saved.");
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog title={isEdit() ? "Edit Spool" : "Add Spool"} open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <TextField label="Manufacturer" value={manufacturer()} onChange={onManufacturerChange} required error={serverFieldErrorFor("manufacturer")} />
        <TextField label="Product (optional)" value={product()} onChange={onProductChange} error={serverFieldErrorFor("product")} />
        <Select
          label="Material"
          options={MATERIAL_FAMILIES}
          value={family()}
          onChange={setFamily}
          optionLabel={materialFamilyLabel}
        />
        <Show when={family() === "OTHER"}>
          <TextField label="Material (other)" value={materialOther()} onChange={onMaterialOtherChange} required error={serverFieldErrorFor("materialOther")} />
        </Show>
        <TextField label="Color name" value={colorName()} onChange={onColorNameChange} required error={serverFieldErrorFor("colorName")} />
        <TextField label="Color hex (optional)" value={colorHex()} onChange={onColorHexChange} placeholder="#RRGGBB" error={serverFieldErrorFor("colorHex")} />
        <RadioGroup
          label="Diameter"
          options={DIAMETER_OPTIONS.map((o) => ({ value: o.value, label: o.label }))}
          value={diameter()}
          onChange={(v) => setDiameter(v as FilamentDiameter)}
        />
        <div class={styles.quickPicks}>
          <span class={styles.quickPicksLabel}>Nominal weight</span>
          <div class={styles.chips}>
            <For each={NOMINAL_QUICK_PICKS}>
              {(pick) => (
                <Chip
                  selected={nominalGrams() === pick.mg / 1000}
                  onSelectedChange={() => setNominalGrams(pick.mg / 1000)}
                >
                  {pick.label}
                </Chip>
              )}
            </For>
          </div>
        </div>
        <NumberField
          label="Nominal weight (g)"
          value={nominalGrams()}
          onChange={onNominalGramsChange}
          minValue={1}
          maxValue={50_000}
          step={0.1}
          suffix="g"
          error={serverFieldErrorFor("nominalMg")}
        />
        <NumberField
          label="Low threshold (g)"
          value={lowThresholdGrams()}
          onChange={onLowThresholdGramsChange}
          minValue={0}
          maxValue={50_000}
          step={0.1}
          suffix="g"
          error={serverFieldErrorFor("lowThresholdMg")}
        />
        <Select
          label="Default tare"
          options={tareOptions()}
          value={tareId()}
          onChange={onTareIdChange}
          optionLabel={tareLabel}
        />
        <Show when={!isEdit()}>
          <TextField label="Storage label (optional)" value={storageLabel()} onChange={setStorageLabel} />
        </Show>
        <Textarea label="Notes (optional)" value={notes()} onChange={setNotes} rows={2} />
        <Show when={!isEdit()}>
          <div class={styles.amount}>
            <Chip selected={weighed()} onSelectedChange={setWeighed}>I weighed it</Chip>
            <Show
              when={weighed()}
              fallback={<p class={styles.hint}>Starts at its nominal weight, estimated.</p>}
            >
              <RadioGroup
                label="Entry method"
                options={[{ value: "net", label: "Net" }, { value: "scale", label: "Scale" }]}
                value={amountMode()}
                onChange={(v) => setAmountMode(v as AmountMode)}
              />
              <Show when={amountMode() === "net"}>
                <NumberField
                  label="Measured net weight (g)"
                  value={measuredGrams()}
                  onChange={onMeasuredGramsChange}
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
              <Show when={amountMode() === "scale"}>
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
                  <p class={styles.hint}>Net: {formatGrams(netPreviewMg()!, 1)}</p>
                </Show>
              </Show>
            </Show>
          </div>
        </Show>
        <Show when={dialogError()}>
          {(message) => <p class={styles.error} role="alert">{message()}</p>}
        </Show>
        <div class={styles.actions}>
          <Button variant="secondary" onClick={() => props.onOpenChange(false)}>Cancel</Button>
          <Button disabled={!canSubmit()} onClick={() => void onSubmit()}>
            {isEdit() ? "Save" : "Add Spool"}
          </Button>
        </div>
      </div>
    </Dialog>
  );
}

