import { Combobox as KCombobox } from "@kobalte/core/combobox";
import { For, Show, type JSX } from "solid-js";
import styles from "./Combobox.module.css";

export interface ComboboxGroup<T> {
  label: string;
  options: T[];
}

interface ComboboxCommonProps<T> {
  label?: string;
  /** Flat option list. Ignored if `groups` is passed. */
  options?: T[];
  /** Grouped option list, rendered under section headers. */
  groups?: ComboboxGroup<T>[];
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

export interface ComboboxSingleProps<T> extends ComboboxCommonProps<T> {
  multiple?: false;
  value?: T;
  onChange?: (value: T) => void;
}

/** Several options at once: each chosen one shows as a removable token
 *  beside the input, and the list stays open between picks. */
export interface ComboboxMultipleProps<T> extends ComboboxCommonProps<T> {
  multiple: true;
  value?: T[];
  onChange?: (value: T[]) => void;
}

export type ComboboxProps<T> = ComboboxSingleProps<T> | ComboboxMultipleProps<T>;

// Overloads rather than one union signature, so `multiple` picks the shape
// before `T` is inferred: with a union, `value: string[]` would infer
// `T = string[]` from the single-value member.
export function Combobox<T>(props: ComboboxMultipleProps<T>): JSX.Element;
export function Combobox<T>(props: ComboboxSingleProps<T>): JSX.Element;
export function Combobox<T>(props: ComboboxProps<T>): JSX.Element {
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
      multiple={props.multiple as never}
      value={props.value as never}
      onChange={(v: unknown) => {
        if (props.multiple) props.onChange?.((v ?? []) as T[]);
        else if (v !== null) props.onChange?.(v as T);
      }}
      onInputChange={props.onInputChange}
      placeholder={props.placeholder}
      disabled={props.disabled}
      itemComponent={(itemProps) => (
        <KCombobox.Item
          item={itemProps.item}
          class={styles.item}
          // Kobalte selects on pointerup (not pointerdown), but the item's
          // own mousedown default action moves focus there first — which
          // blurs the input, and the blur handler resets the typed filter
          // text before the pointerup lands. That re-expands the (now
          // unfiltered) option list under the cursor, so the click resolves
          // against whatever ends up there instead of the option the user
          // meant to pick. Suppressing mousedown's default keeps focus on
          // the input for the whole press, so the list never reflows out
          // from under the pointerup.
          onMouseDown={(e: MouseEvent) => e.preventDefault()}
        >
          <KCombobox.ItemLabel>{toLabel(itemProps.item.rawValue as T)}</KCombobox.ItemLabel>
          <Show when={props.multiple}>
            <KCombobox.ItemIndicator class={styles.itemIndicator}>
              <CheckIcon />
            </KCombobox.ItemIndicator>
          </Show>
        </KCombobox.Item>
      )}
      sectionComponent={(sectionProps) => (
        <KCombobox.Section class={styles.section}>
          {(sectionProps.section.rawValue as ComboboxGroup<T>).label}
        </KCombobox.Section>
      )}
    >
      {props.label && <KCombobox.Label class={styles.label}>{props.label}</KCombobox.Label>}
      <KCombobox.Control<T> class={styles.control} data-multiple={props.multiple ? "" : undefined}>
        {(state) => (
          <>
            <Show when={props.multiple}>
              <For each={state.selectedOptions()}>
                {(option) => (
                  <span class={styles.token}>
                    <span class={styles.tokenLabel}>{toLabel(option)}</span>
                    {/* A native button in the tab order, like `Chip`'s
                        remove. Backspace in the empty input also removes
                        the last token (Kobalte's `removeOnBackspace`). */}
                    <button
                      type="button"
                      class={styles.tokenRemove}
                      aria-label={`Remove ${toLabel(option)}`}
                      disabled={props.disabled}
                      onClick={() => state.remove(option)}
                    >
                      <RemoveIcon />
                    </button>
                  </span>
                )}
              </For>
            </Show>
            <KCombobox.Input class={styles.input} />
            <KCombobox.Trigger class={styles.trigger}>
              <KCombobox.Icon>
                <ChevronIcon />
              </KCombobox.Icon>
            </KCombobox.Trigger>
          </>
        )}
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

function RemoveIcon() {
  return (
    <svg width="8" height="8" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path d="M1 1L9 9M9 1L1 9" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" />
    </svg>
  );
}
