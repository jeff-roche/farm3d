import { createEffect, createMemo, createSignal, For, on, Show } from "solid-js";
import { Button, Chip, Dialog, NumberField, RadioGroup, Select, TextField, Textarea } from "../design-system";
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
  }));

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
  const grossError = createMemo<string | undefined>(() => {
    const preview = netPreviewMg();
    if (preview === null) return undefined;
    return preview < 0 ? "The gross weight is less than the tare." : undefined;
  });

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
    return grossGrams() !== undefined && grossGrams()! >= 0 && !grossError();
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
    try {
      const fields = buildFields();
      const result = isEdit()
        ? await updateSpool(props.spool!.id, fields)
        : await createSpool(fields, buildInitialAmount(), storageLabel().trim() || undefined);
      if (result) {
        props.onOpenChange(false);
        props.onSaved?.(result);
      }
    } finally {
      setSubmitting(false);
    }
  }

  return (
    <Dialog title={isEdit() ? "Edit Spool" : "Add Spool"} open={props.open} onOpenChange={props.onOpenChange}>
      <div class={styles.body}>
        <TextField label="Manufacturer" value={manufacturer()} onChange={setManufacturer} required />
        <TextField label="Product (optional)" value={product()} onChange={setProduct} />
        <Select
          label="Material"
          options={MATERIAL_FAMILIES}
          value={family()}
          onChange={setFamily}
          optionLabel={materialFamilyLabel}
        />
        <Show when={family() === "OTHER"}>
          <TextField label="Material (other)" value={materialOther()} onChange={setMaterialOther} required />
        </Show>
        <TextField label="Color name" value={colorName()} onChange={setColorName} required />
        <TextField label="Color hex (optional)" value={colorHex()} onChange={setColorHex} placeholder="#RRGGBB" />
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
          onChange={setNominalGrams}
          minValue={1}
          maxValue={50_000}
          step={0.1}
          suffix="g"
        />
        <NumberField
          label="Low threshold (g)"
          value={lowThresholdGrams()}
          onChange={setLowThresholdGrams}
          minValue={0}
          maxValue={50_000}
          step={0.1}
          suffix="g"
        />
        <Select
          label="Default tare"
          options={tareOptions()}
          value={tareId()}
          onChange={setTareId}
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
                  onChange={setMeasuredGrams}
                  minValue={0}
                  maxValue={50_000}
                  step={0.1}
                  suffix="g"
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
                  onChange={setGrossGrams}
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

