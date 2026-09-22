import { TextField as KTextField } from "@kobalte/core/text-field";
import styles from "./Textarea.module.css";

export interface TextareaProps {
  label: string;
  value: string;
  onChange: (value: string) => void;
  rows?: number;
  placeholder?: string;
  description?: string;
  errorMessage?: string;
  class?: string;
}

export function Textarea(props: TextareaProps) {
  return (
    <KTextField
      class={[styles.root, props.class].filter(Boolean).join(" ")}
      value={props.value}
      onChange={props.onChange}
      validationState={props.errorMessage ? "invalid" : "valid"}
    >
      <KTextField.Label class={styles.label}>{props.label}</KTextField.Label>
      <KTextField.TextArea
        class={styles.textarea}
        placeholder={props.placeholder}
        rows={props.rows}
      />
      {props.description && (
        <KTextField.Description class={styles.description}>
          {props.description}
        </KTextField.Description>
      )}
      {props.errorMessage && (
        <KTextField.ErrorMessage class={styles.errorMessage}>
          {props.errorMessage}
        </KTextField.ErrorMessage>
      )}
    </KTextField>
  );
}
