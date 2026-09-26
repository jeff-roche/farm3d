import { Select as KSelect } from "@kobalte/core/select";
import { Show } from "solid-js";
import styles from "./Select.module.css";

export interface SelectGroup<T> {
  label: string;
  options: T[];
}

export interface SelectProps<T> {
  label?: string;
  /** Flat option list. Ignored if `groups` is passed. */
  options?: T[];
  /** Grouped option list, rendered under section headers. */
  groups?: SelectGroup<T>[];
  /** `null` shows nothing chosen (the placeholder) while staying
   *  controlled; `undefined` leaves the Select uncontrolled. */
  value?: T | null;
  defaultValue?: T;
  onChange?: (value: T) => void;
  /** Defaults to the option itself (for T = string). */
  optionValue?: (option: T) => string;
  /** Defaults to the option itself (for T = string). */
  optionLabel?: (option: T) => string;
  /** A disabled option is listed but can't be chosen. */
  optionDisabled?: (option: T) => boolean;
  /** A second line under an option's label, e.g. why it is disabled. */
  optionDescription?: (option: T) => string | undefined;
  placeholder?: string;
  disabled?: boolean;
  error?: string;
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
      options={(props.groups ?? props.options ?? []) as never[]}
      optionGroupChildren={props.groups ? ("options" as never) : undefined}
      optionValue={props.optionValue ? ((o: T) => toValue(o)) as never : undefined}
      optionTextValue={((o: T) => toLabel(o)) as never}
      optionDisabled={props.optionDisabled ? ((o: T) => props.optionDisabled!(o)) as never : undefined}
      value={props.value as never}
      defaultValue={props.defaultValue}
      onChange={(v: unknown) => {
        if (v !== null) props.onChange?.(v as T);
      }}
      placeholder={props.placeholder}
      disabled={props.disabled}
      validationState={props.error ? "invalid" : "valid"}
      itemComponent={(itemProps) => (
        <KSelect.Item
          item={itemProps.item}
          class={[styles.item, props.optionDescription ? styles.itemDescribed : ""].filter(Boolean).join(" ")}
        >
          <KSelect.ItemLabel>{toLabel(itemProps.item.rawValue as T)}</KSelect.ItemLabel>
          <Show when={props.optionDescription?.(itemProps.item.rawValue as T)}>
            {(description) => (
              <KSelect.ItemDescription class={styles.itemDescription}>{description()}</KSelect.ItemDescription>
            )}
          </Show>
        </KSelect.Item>
      )}
      sectionComponent={(sectionProps) => (
        <KSelect.Section class={styles.section}>
          {(sectionProps.section.rawValue as SelectGroup<T>).label}
        </KSelect.Section>
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
      {props.error && (
        <KSelect.ErrorMessage class={styles.errorMessage}>
          {props.error}
        </KSelect.ErrorMessage>
      )}
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
