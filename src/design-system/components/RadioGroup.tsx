import { RadioGroup as KRadioGroup } from "@kobalte/core/radio-group";
import { For, splitProps } from "solid-js";
import styles from "./RadioGroup.module.css";

export interface RadioOption {
  value: string;
  label: string;
}

export interface RadioGroupProps {
  label?: string;
  options: RadioOption[];
  value?: string;
  defaultValue?: string;
  onChange?: (value: string) => void;
  disabled?: boolean;
  class?: string;
}

export function RadioGroup(props: RadioGroupProps) {
  const [local, rest] = splitProps(props, ["label", "options", "class"]);

  return (
    <KRadioGroup class={[styles.root, local.class].filter(Boolean).join(" ")} {...rest}>
      {local.label && <KRadioGroup.Label class={styles.groupLabel}>{local.label}</KRadioGroup.Label>}
      <For each={local.options}>
        {(option) => (
          <KRadioGroup.Item value={option.value} class={styles.item}>
            <KRadioGroup.ItemInput />
            <KRadioGroup.ItemControl class={styles.control} />
            <KRadioGroup.ItemLabel class={styles.itemLabel}>
              {option.label}
            </KRadioGroup.ItemLabel>
          </KRadioGroup.Item>
        )}
      </For>
    </KRadioGroup>
  );
}
