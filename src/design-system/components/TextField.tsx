import { TextField as KTextField } from "@kobalte/core/text-field";
import { splitProps } from "solid-js";
import styles from "./TextField.module.css";

export interface TextFieldProps {
  label?: string;
  /** An accessible name for the input when no visible `label` is rendered —
   *  e.g. an inline-editable field styled to read as plain text, not a
   *  labeled form control. Forwarded to the *Input* subcomponent
   *  specifically, not the Root, matching `NumberField`'s identical
   *  `aria-label` handling and for the same reason: Kobalte's form-control
   *  primitives read `aria-label` from the Input, not the Root. */
  "aria-label"?: string;
  description?: string;
  error?: string;
  placeholder?: string;
  value?: string;
  defaultValue?: string;
  onChange?: (value: string) => void;
  disabled?: boolean;
  required?: boolean;
  type?: "text" | "password" | "email" | "number" | "search" | "url" | "tel";
  class?: string;
}

export function TextField(props: TextFieldProps) {
  const [local, rest] = splitProps(props, [
    "label",
    "aria-label",
    "description",
    "error",
    "placeholder",
    "type",
    "class",
  ]);

  return (
    <KTextField
      class={[styles.root, local.class].filter(Boolean).join(" ")}
      validationState={local.error ? "invalid" : "valid"}
      {...rest}
    >
      {local.label && <KTextField.Label class={styles.label}>{local.label}</KTextField.Label>}
      <KTextField.Input
        class={styles.input}
        placeholder={local.placeholder}
        type={local.type ?? "text"}
        aria-label={local["aria-label"]}
      />
      {local.description && (
        <KTextField.Description class={styles.description}>
          {local.description}
        </KTextField.Description>
      )}
      {local.error && (
        <KTextField.ErrorMessage class={styles.errorMessage}>
          {local.error}
        </KTextField.ErrorMessage>
      )}
    </KTextField>
  );
}
