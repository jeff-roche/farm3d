import { NumberField as KNumberField } from "@kobalte/core/number-field";
import { splitProps } from "solid-js";
import styles from "./NumberField.module.css";

export interface NumberFieldProps {
  label?: string;
  /** An accessible name for the input when no visible `label` is rendered —
   *  e.g. when this NumberField sits inside a `Field` row that already shows
   *  its own visible label text. Kobalte's form-control primitives read
   *  `aria-label` from the *Input* subcomponent specifically, not the Root
   *  (verified at `node_modules/@kobalte/core/src/form-control/create-form-control-field.tsx`),
   *  so it's forwarded there rather than spread onto the root element. */
  "aria-label"?: string;
  value?: number;
  defaultValue?: number;
  onChange?: (value: number) => void;
  minValue?: number;
  maxValue?: number;
  step?: number;
  suffix?: string;
  disabled?: boolean;
  error?: string;
  class?: string;
}

export function NumberField(props: NumberFieldProps) {
  const [local, rest] = splitProps(props, [
    "label",
    "aria-label",
    "value",
    "defaultValue",
    "onChange",
    "suffix",
    "error",
    "class",
  ]);

  return (
    <KNumberField
      class={[styles.root, local.class].filter(Boolean).join(" ")}
      rawValue={local.value}
      defaultValue={local.defaultValue}
      onRawValueChange={local.onChange}
      validationState={local.error ? "invalid" : "valid"}
      {...rest}
    >
      {local.label && (
        <KNumberField.Label class={styles.label}>{local.label}</KNumberField.Label>
      )}
      <div class={styles.inputRow}>
        <KNumberField.Input class={styles.input} aria-label={local["aria-label"]} />
        {local.suffix && <span class={styles.suffix}>{local.suffix}</span>}
        <div class={styles.spinner}>
          <KNumberField.IncrementTrigger class={styles.spinButton} aria-label="Increment">
            <ChevronIcon direction="up" />
          </KNumberField.IncrementTrigger>
          <KNumberField.DecrementTrigger class={styles.spinButton} aria-label="Decrement">
            <ChevronIcon direction="down" />
          </KNumberField.DecrementTrigger>
        </div>
      </div>
      {local.error && (
        <KNumberField.ErrorMessage class={styles.errorMessage}>
          {local.error}
        </KNumberField.ErrorMessage>
      )}
    </KNumberField>
  );
}

function ChevronIcon(props: { direction: "up" | "down" }) {
  const d = props.direction === "up" ? "M2 6L5 3L8 6" : "M2 4L5 7L8 4";
  return (
    <svg width="8" height="8" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path d={d} stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round" />
    </svg>
  );
}
