import { Select as KSelect } from "@kobalte/core/select";
import styles from "./Select.module.css";

export interface SelectProps<T> {
  label?: string;
  options: T[];
  value?: T;
  defaultValue?: T;
  onChange?: (value: T) => void;
  /** Defaults to the option itself (for T = string). */
  optionValue?: (option: T) => string;
  /** Defaults to the option itself (for T = string). */
  optionLabel?: (option: T) => string;
  placeholder?: string;
  disabled?: boolean;
  class?: string;
}

export function Select<T>(props: SelectProps<T>) {
  const toLabel = (option: T) =>
    props.optionLabel ? props.optionLabel(option) : String(option);
  const toValue = (option: T) =>
    props.optionValue ? props.optionValue(option) : String(option);

  return (
    <KSelect
      class={[styles.root, props.class].filter(Boolean).join(" ")}
      options={props.options}
      optionValue={props.optionValue ? (o: T) => toValue(o) : undefined}
      optionTextValue={(o: T) => toLabel(o)}
      value={props.value}
      defaultValue={props.defaultValue}
      onChange={(v) => v !== null && props.onChange?.(v)}
      placeholder={props.placeholder}
      disabled={props.disabled}
      itemComponent={(itemProps) => (
        <KSelect.Item item={itemProps.item} class={styles.item}>
          <KSelect.ItemLabel>{toLabel(itemProps.item.rawValue)}</KSelect.ItemLabel>
        </KSelect.Item>
      )}
    >
      {props.label && <KSelect.Label class={styles.label}>{props.label}</KSelect.Label>}
      <KSelect.Trigger class={styles.trigger}>
        <KSelect.Value<T>>{(state) => toLabel(state.selectedOption())}</KSelect.Value>
        <KSelect.Icon>
          <ChevronIcon />
        </KSelect.Icon>
      </KSelect.Trigger>
      <KSelect.Portal>
        <KSelect.Content class={styles.content}>
          <KSelect.Listbox class={styles.listbox} />
        </KSelect.Content>
      </KSelect.Portal>
    </KSelect>
  );
}

function ChevronIcon() {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path
        d="M2 4L5 7L8 4"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
        stroke-linejoin="round"
      />
    </svg>
  );
}
