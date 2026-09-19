import { createEffect, createResource, on, onCleanup, Show } from "solid-js";
import { Select, TextField } from "../design-system";
import { listCatalogVariants } from "../printers/printer-catalog";
import { rebindPrinter, updatePrinter } from "../printers/printer-store";
import type { CatalogVariantSummary, ResolvedPrinter } from "../printers/types";
import styles from "./PrinterSetupPanel.module.css";

export interface PrinterSetupPanelProps {
  printer: ResolvedPrinter;
}

const DEBOUNCE_MS = 300;

export function PrinterSetupPanel(props: PrinterSetupPanelProps) {
  let notesTimer: ReturnType<typeof setTimeout> | undefined;
  const [variants] = createResource(
    () => [props.printer.catalogRef.vendor, props.printer.catalogRef.model] as const,
    ([vendor, model]) => listCatalogVariants(vendor, model),
  );

  createEffect(on(() => props.printer.id, (_id, previousId) => {
    if (previousId !== undefined) clearTimeout(notesTimer);
  }));
  onCleanup(() => clearTimeout(notesTimer));

  const saveNotes = (value: string) => {
    clearTimeout(notesTimer);
    const printerId = props.printer.id;
    notesTimer = setTimeout(() => void updatePrinter(printerId, { notes: value }), DEBOUNCE_MS);
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
      <TextField label="Notes" value={props.printer.notes} onChange={saveNotes} placeholder="Spare parts, quirks, anything worth remembering" />
    </div>
  );
}
