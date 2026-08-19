import { Checkbox as KCheckbox } from "@kobalte/core/checkbox";
import { splitProps, type ParentProps } from "solid-js";
import styles from "./Checkbox.module.css";

export interface CheckboxProps extends ParentProps {
  checked?: boolean;
  defaultChecked?: boolean;
  onChange?: (checked: boolean) => void;
  indeterminate?: boolean;
  disabled?: boolean;
  class?: string;
}

export function Checkbox(props: CheckboxProps) {
  const [local, rest] = splitProps(props, ["children", "class"]);

  return (
    <KCheckbox class={[styles.root, local.class].filter(Boolean).join(" ")} {...rest}>
      <KCheckbox.Input />
      <KCheckbox.Control class={styles.control}>
        <KCheckbox.Indicator>
          <CheckIcon />
        </KCheckbox.Indicator>
      </KCheckbox.Control>
      {local.children && <KCheckbox.Label class={styles.label}>{local.children}</KCheckbox.Label>}
    </KCheckbox>
  );
}

function CheckIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path
        d="M1.5 5L4 7.5L8.5 2.5"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}
