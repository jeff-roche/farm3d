import { createEffect, createResource, on, onCleanup, Show } from "solid-js";
import { RadioGroup, Select, TextField } from "../design-system";
import { listCatalogVariants } from "../printers/printer-catalog";
import { rebindPrinter, updatePrinter } from "../printers/printer-store";
import type { CatalogVariantSummary, ResolvedPrinter, StartSafety } from "../printers/types";
import { START_SAFETY_OPTIONS } from "./PrinterSetupWizard";
import styles from "./PrinterSetupPanel.module.css";

export interface PrinterSetupPanelProps {
  printer: ResolvedPrinter;
}

const DEBOUNCE_MS = 300;

export function PrinterSetupPanel(props: PrinterSetupPanelProps) {
  let nameTimer: ReturnType<typeof setTimeout> | undefined;
  let notesTimer: ReturnType<typeof setTimeout> | undefined;
  let locationTimer: ReturnType<typeof setTimeout> | undefined;
  const [variants] = createResource(
    () => [props.printer.catalogRef.vendor, props.printer.catalogRef.model] as const,
    ([vendor, model]) => listCatalogVariants(vendor, model),
  );

  createEffect(on(() => props.printer.id, (_id, previousId) => {
    if (previousId === undefined) return;
    clearTimeout(nameTimer);
    clearTimeout(notesTimer);
    clearTimeout(locationTimer);
  }));
  onCleanup(() => {
    clearTimeout(nameTimer);
    clearTimeout(notesTimer);
    clearTimeout(locationTimer);
  });

  const saveAfterDelay = (
    field: "name" | "notes",
    value: string,
    currentTimer: () => ReturnType<typeof setTimeout> | undefined,
    setTimer: (timer: ReturnType<typeof setTimeout>) => void,
  ) => {
    clearTimeout(currentTimer());
    const printerId = props.printer.id;
    setTimer(setTimeout(() => void updatePrinter(printerId, { [field]: value }), DEBOUNCE_MS));
  };
  const saveLocationAfterDelay = (value: string) => {
    clearTimeout(locationTimer);
    const printerId = props.printer.id;
    // Empty clears the field (`{location: null}`); a `PrinterPatch` treats an
    // absent key as "leave unchanged" and `null` as "clear" (see
    // `PrinterPatch`'s own doc comment), so this must send `null`, not `""`.
    const trimmed = value.trim();
    locationTimer = setTimeout(
      () => void updatePrinter(printerId, { location: trimmed === "" ? null : value }),
      DEBOUNCE_MS,
    );
  };
  const rebind = (variant: CatalogVariantSummary) => {
    if (variant.variant === props.printer.catalogRef.variant) return;
    void rebindPrinter(props.printer.id, {
      vendor: props.printer.catalogRef.vendor,
      model: props.printer.catalogRef.model,
      modelId: props.printer.catalogRef.modelId,
      variant: variant.variant,
      printerVariant: variant.printerVariant,
    });
  };

  return (
    <div class={styles.panel}>
      <div class={styles.field}>
        <span class={styles.label}>Catalog</span>
        <span>{props.printer.modelLabel} — {props.printer.variantLabel}</span>
        <Show when={props.printer.catalogStatus !== "ok"}>
          <span class={styles.muted}>Catalog status: {props.printer.catalogStatus}</span>
        </Show>
      </div>
      <Show when={(variants() ?? []).length > 1}>
        <Select
          label="Nozzle / variant"
          options={variants() ?? []}
          optionValue={(variant: CatalogVariantSummary) => variant.variant}
          optionLabel={(variant: CatalogVariantSummary) => `${variant.printerVariant} mm`}
          value={(variants() ?? []).find((variant) => variant.variant === props.printer.catalogRef.variant)}
          onChange={rebind}
        />
      </Show>
      <TextField
        label="Name"
        value={props.printer.name}
        onChange={(value) => saveAfterDelay("name", value, () => nameTimer, (timer) => (nameTimer = timer))}
      />
      <TextField
        label="Notes"
        value={props.printer.notes}
        onChange={(value) => saveAfterDelay("notes", value, () => notesTimer, (timer) => (notesTimer = timer))}
        placeholder="Spare parts, quirks, anything worth remembering"
      />
      <TextField
        label="Location"
        value={props.printer.location ?? ""}
        onChange={saveLocationAfterDelay}
        placeholder="Bay 1"
      />
      <RadioGroup
        label="Start safety"
        options={START_SAFETY_OPTIONS}
        value={props.printer.startSafety}
        onChange={(v) => void updatePrinter(props.printer.id, { startSafety: v as StartSafety })}
      />
    </div>
  );
}
