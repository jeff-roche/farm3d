import { Combobox as KCombobox } from "@kobalte/core/combobox";
import styles from "./Combobox.module.css";

export interface ComboboxGroup<T> {
  label: string;
  options: T[];
}

export interface ComboboxProps<T> {
  label?: string;
  /** Flat option list. Ignored if `groups` is passed. */
  options?: T[];
  /** Grouped option list, rendered under section headers. */
  groups?: ComboboxGroup<T>[];
  value?: T;
  onChange?: (value: T) => void;
  /** Fires as the user types — filtering `options`/`groups` is the caller's job. */
  onInputChange?: (query: string) => void;
  /** Defaults to the option itself (for T = string). */
  optionValue?: (option: T) => string;
  /** Defaults to the option itself (for T = string). */
  optionLabel?: (option: T) => string;
  placeholder?: string;
  disabled?: boolean;
  class?: string;
}

export function Combobox<T>(props: ComboboxProps<T>) {
  const toLabel = (option: T) =>
    props.optionLabel ? props.optionLabel(option) : String(option);
  const toValue = (option: T) =>
    props.optionValue ? props.optionValue(option) : String(option);

  return (
    <KCombobox
      class={[styles.root, props.class].filter(Boolean).join(" ")}
      options={(props.groups ?? props.options ?? []) as never[]}
      optionGroupChildren={props.groups ? ("options" as never) : undefined}
      optionValue={props.optionValue ? ((o: T) => toValue(o)) as never : undefined}
      optionTextValue={((o: T) => toLabel(o)) as never}
      optionLabel={((o: T) => toLabel(o)) as never}
      value={props.value as never}
      onChange={(v) => v !== null && props.onChange?.(v as T)}
      onInputChange={props.onInputChange}
      placeholder={props.placeholder}
      disabled={props.disabled}
      itemComponent={(itemProps) => (
        <KCombobox.Item item={itemProps.item} class={styles.item}>
          <KCombobox.ItemLabel>{toLabel(itemProps.item.rawValue as T)}</KCombobox.ItemLabel>
        </KCombobox.Item>
      )}
      sectionComponent={(sectionProps) => (
        <KCombobox.Section class={styles.section}>
          {(sectionProps.section.rawValue as ComboboxGroup<T>).label}
        </KCombobox.Section>
      )}
    >
      {props.label && <KCombobox.Label class={styles.label}>{props.label}</KCombobox.Label>}
      <KCombobox.Control class={styles.control}>
        <KCombobox.Input class={styles.input} />
        <KCombobox.Trigger class={styles.trigger}>
          <KCombobox.Icon>
            <ChevronIcon />
          </KCombobox.Icon>
        </KCombobox.Trigger>
      </KCombobox.Control>
      <KCombobox.Portal>
        <KCombobox.Content class={styles.content}>
          <KCombobox.Listbox class={styles.listbox} />
        </KCombobox.Content>
      </KCombobox.Portal>
    </KCombobox>
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
