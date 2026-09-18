import { createEffect, createMemo, on, onCleanup, Show } from "solid-js";
import { Button, Field, NumberField, Select, Switch } from "../design-system";
import type { JsonValue } from "../generated/contracts/command/JsonValue";
import { overrideField, resolveDrift, revertField } from "../printers/printer-store";
import type { BedShape, OverridableField, ResolvedPrinter } from "../printers/types";
import styles from "./PrinterProfilePanel.module.css";

export interface PrinterProfilePanelProps {
  printer: ResolvedPrinter;
}

const DEBOUNCE_MS = 300;

/** The values `defaultBedType` actually takes in the shipped catalog:
 *  "" (942 variants), "4" (20) and "Textured PEI Plate" (9). */
const BED_TYPE_OPTIONS = ["", "4", "Textured PEI Plate"];

function isOverridden(printer: ResolvedPrinter, field: OverridableField): boolean {
  return printer.overriddenFields.includes(field);
}

type RectangularBedShape = Extract<BedShape, { kind: "rectangular" }>;

export function PrinterProfilePanel(props: PrinterProfilePanelProps) {
  let heightTimer: ReturnType<typeof setTimeout> | undefined;
  let bedShapeTimer: ReturnType<typeof setTimeout> | undefined;
  // Accumulates in-flight bed-size edits (width/depth) so a second edit
  // within the debounce window merges onto the first instead of re-reading
  // a stale pre-edit `shape()` snapshot and silently dropping it.
  let pendingBedShapePatch: Partial<Pick<RectangularBedShape, "widthMm" | "depthMm">> | undefined;

  // PrinterDashboard's outer <Show when={selected()}> is non-keyed, so
  // switching the selected printer (a truthy -> truthy transition) does NOT
  // remount this component -- Solid's <Show> only re-invokes its child on a
  // falsy<->truthy transition. That means the `let` timer state above, and
  // any debounced closure reading props.printer.id live, would otherwise
  // persist across a printer switch: edit height on printer A, switch to
  // printer B within the debounce window, and the pending write would fire
  // against whichever printer is selected when the timer expires -- not
  // the one being edited when it was scheduled. Cancel in-flight writes
  // whenever the printer identity changes.
  createEffect(on(
    () => props.printer.id,
    (_id, prevId) => {
      if (prevId === undefined) return;
      clearTimeout(heightTimer);
      clearTimeout(bedShapeTimer);
      heightTimer = undefined;
      bedShapeTimer = undefined;
      pendingBedShapePatch = undefined;
    },
  ));

  onCleanup(() => {
    clearTimeout(heightTimer);
    clearTimeout(bedShapeTimer);
  });

  function debouncedOverride(
    field: OverridableField,
    value: JsonValue,
    timer: () => ReturnType<typeof setTimeout> | undefined,
    setTimer: (t: ReturnType<typeof setTimeout>) => void,
  ) {
    clearTimeout(timer());
    const printerId = props.printer.id;
    setTimer(setTimeout(() => void overrideField(printerId, field, value), DEBOUNCE_MS));
  }

  // Kobalte's NumberField root fires onRawValueChange unconditionally at
  // mount (node_modules/@kobalte/core/src/number-field/number-field-root.tsx:214),
  // and again whenever the reactive source feeding its `rawValue` prop
  // changes identity -- neither case is a real edit. Writing an override
  // for a same-value "change" would silently pin the field to the catalog
  // value it already has, permanently defeating drift detection for it
  // (profile_drift is only computed for non-overridden fields). Each
  // onChange below must compare against the field's current value and
  // no-op on equality before scheduling a write.
  function debouncedBedShapeOverride(shape: RectangularBedShape, patch: Partial<Pick<RectangularBedShape, "widthMm" | "depthMm">>) {
    pendingBedShapePatch = { ...pendingBedShapePatch, ...patch };
    clearTimeout(bedShapeTimer);
    const printerId = props.printer.id;
    const value = { ...shape, ...pendingBedShapePatch };
    bedShapeTimer = setTimeout(() => {
      void overrideField(printerId, "bedShape", value);
      pendingBedShapePatch = undefined;
    }, DEBOUNCE_MS);
  }

  const rectShape = createMemo(() => {
    const shape = props.printer.profile.bedShape;
    return shape.kind === "rectangular" ? shape : null;
  });

  // Defensively carry the printer's own value when it isn't one of the known
  // three, so a future catalog regeneration introducing a new bed type can
  // never leave the control silently showing a blank, unmatched value.
  const bedTypeOptions = createMemo(() => {
    const current = props.printer.profile.defaultBedType;
    return BED_TYPE_OPTIONS.includes(current) ? BED_TYPE_OPTIONS : [...BED_TYPE_OPTIONS, current];
  });

  return (
    <div class={styles.panel}>
      <Show when={props.printer.profileDrift.length > 0}>
        <div class={styles.driftBanner}>
          <p class={styles.driftMessage}>
            The catalog changed for {props.printer.profileDrift.length} field
            {props.printer.profileDrift.length === 1 ? "" : "s"} since this printer was last
            confirmed.
          </p>
          <div class={styles.driftActions}>
            <Button variant="secondary" onClick={() => void resolveDrift(props.printer.id, "pin")}>
              Keep my value
            </Button>
            <Button variant="primary" onClick={() => void resolveDrift(props.printer.id, "accept")}>
              Accept
            </Button>
          </div>
        </div>
      </Show>

      <Field
        label="Printable height"
        overridden={isOverridden(props.printer, "printableHeightMm")}
        hint={
          isOverridden(props.printer, "printableHeightMm")
            ? `inherited: ${props.printer.inherited.printableHeightMm}`
            : undefined
        }
        onRevert={() => void revertField(props.printer.id, "printableHeightMm")}
      >
        <NumberField
          aria-label="Printable height"
          value={props.printer.profile.printableHeightMm}
          suffix="mm"
          minValue={1}
          onChange={(v) => {
            if (v === props.printer.profile.printableHeightMm) return;
            debouncedOverride(
              "printableHeightMm", v,
              () => heightTimer, (t) => (heightTimer = t),
            );
          }}
        />
      </Field>

      <Show when={rectShape()}>
        {(shape) => (
          <Field
            label="Bed size"
            overridden={isOverridden(props.printer, "bedShape")}
            onRevert={() => void revertField(props.printer.id, "bedShape")}
          >
            <div class={styles.bedSizeRow}>
              <NumberField
                aria-label="Bed width"
                value={shape().widthMm}
                suffix="mm"
                minValue={1}
                onChange={(v) => {
                  if (v === shape().widthMm) return;
                  debouncedBedShapeOverride(shape(), { widthMm: v });
                }}
              />
              <NumberField
                aria-label="Bed depth"
                value={shape().depthMm}
                suffix="mm"
                minValue={1}
                onChange={(v) => {
                  if (v === shape().depthMm) return;
                  debouncedBedShapeOverride(shape(), { depthMm: v });
                }}
              />
            </div>
          </Field>
        )}
      </Show>

      <Show when={props.printer.profile.bedShape.kind === "polygon"}>
        <Field label="Bed shape">
          <p class={styles.readOnlyHint}>Non-rectangular bed — edit printers.json to change.</p>
        </Field>
      </Show>

      <Field
        label="Bed type"
        overridden={isOverridden(props.printer, "defaultBedType")}
        onRevert={() => void revertField(props.printer.id, "defaultBedType")}
      >
        {/* Raw catalog bed-type values, not human labels — the generator
            doesn't emit a code->label map in phase 1. The empty string is the
            catalog's own "unspecified", shown as "Default" so it isn't a blank
            list item. */}
        <Select
          options={bedTypeOptions()}
          optionLabel={(v: string) => (v === "" ? "Default" : v)}
          value={props.printer.profile.defaultBedType}
          placeholder="Default"
          onChange={(v) => void overrideField(props.printer.id, "defaultBedType", v)}
        />
      </Field>

      <Field
        label="Auxiliary fan"
        overridden={isOverridden(props.printer, "hasAuxiliaryFan")}
        onRevert={() => void revertField(props.printer.id, "hasAuxiliaryFan")}
      >
        <Switch
          checked={props.printer.profile.hasAuxiliaryFan}
          onChange={(v) => void overrideField(props.printer.id, "hasAuxiliaryFan", v)}
        />
      </Field>

      <Field
        label="Air filtration"
        overridden={isOverridden(props.printer, "supportsAirFiltration")}
        onRevert={() => void revertField(props.printer.id, "supportsAirFiltration")}
      >
        <Switch
          checked={props.printer.profile.supportsAirFiltration}
          onChange={(v) => void overrideField(props.printer.id, "supportsAirFiltration", v)}
        />
      </Field>
    </div>
  );
}
