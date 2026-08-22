import { createEffect, createResource, on, onCleanup, Show } from "solid-js";
import { Select, TextField } from "../design-system";
import { listCatalogVariants } from "../printers/printer-catalog";
import { rebindPrinter, updatePrinter } from "../printers/printer-store";
import type { CatalogVariantSummary, ResolvedPrinter } from "../printers/types";
import styles from "./PrinterStatusPanel.module.css";

export interface PrinterStatusPanelProps {
  printer: ResolvedPrinter;
}

const DEBOUNCE_MS = 300;

export function PrinterStatusPanel(props: PrinterStatusPanelProps) {
  let notesTimer: ReturnType<typeof setTimeout> | undefined;

  // PrinterDashboard's detail <Show> is non-keyed -- switching the selected
  // printer does not remount this component, only changes props.printer.id.
  // An in-flight debounced write must be cancelled on that identity change,
  // or a Notes edit for printer A could land on printer B after a
  // mid-debounce selection switch. Mirrors PrinterProfilePanel's guard.
  createEffect(
    on(
      () => props.printer.id,
      (_id, prevId) => {
        if (prevId === undefined) return;
        clearTimeout(notesTimer);
        notesTimer = undefined;
      },
    ),
  );

  onCleanup(() => clearTimeout(notesTimer));

  function debouncedNotes(value: string) {
    clearTimeout(notesTimer);
    const printerId = props.printer.id;
    notesTimer = setTimeout(() => void updatePrinter(printerId, { notes: value }), DEBOUNCE_MS);
  }

  const [variants] = createResource(
    () => [props.printer.catalogRef.vendor, props.printer.catalogRef.model] as const,
    ([vendor, model]) => listCatalogVariants(vendor, model),
  );

  /** Same model, a different nozzle/variant -- e.g. this unit actually has a
   *  0.6mm nozzle. This is a REBIND (re-points catalogRef at a sibling
   *  variant within the same model), never an override: nozzle size,
   *  nozzle type, and gcode flavor are variant identity, per the phase-1
   *  spec, not a field a Printer can diverge from its variant on. */
  function onRebind(variant: CatalogVariantSummary) {
    if (variant.variant === props.printer.catalogRef.variant) return;
    void rebindPrinter(props.printer.id, {
      vendor: props.printer.catalogRef.vendor,
      model: props.printer.catalogRef.model,
      modelId: props.printer.catalogRef.modelId,
      variant: variant.variant,
      printerVariant: variant.printerVariant,
    });
  }

  return (
    <div class={styles.panel}>
      <div class={styles.field}>
        <span class={styles.label}>Catalog</span>
        <span>
          {props.printer.modelLabel} — {props.printer.variantLabel}
        </span>
        <Show when={props.printer.catalogStatus !== "ok"}>
          <span class={styles.muted}>Catalog status: {props.printer.catalogStatus}</span>
        </Show>
      </div>

      {/* Only worth offering when there's actually a sibling variant to
       *  switch to -- a single-option Select would just be noise. */}
      <Show when={(variants() ?? []).length > 1}>
        <Select
          label="Nozzle / variant"
          options={variants() ?? []}
          optionValue={(v: CatalogVariantSummary) => v.variant}
          optionLabel={(v: CatalogVariantSummary) => `${v.printerVariant} mm`}
          value={(variants() ?? []).find((v) => v.variant === props.printer.catalogRef.variant)}
          onChange={onRebind}
        />
      </Show>

      <TextField
        label="Notes"
        value={props.printer.notes}
        onChange={debouncedNotes}
        placeholder="Spare parts, quirks, anything worth remembering"
      />
    </div>
  );
}
