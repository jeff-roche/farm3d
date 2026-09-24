import { SegmentedControl as KSegmentedControl } from "@kobalte/core/segmented-control";
import { For, Show, type JSX } from "solid-js";
import styles from "./SegmentedControl.module.css";

export interface SegmentedControlOption<T extends string> {
  value: T;
  label: string;
  /** Decorative — the label is always rendered, so icon-only options are
   *  not possible. */
  icon?: JSX.Element;
}

export interface SegmentedControlProps<T extends string> {
  /** Accessible name for the group; not rendered as visible heading text. */
  label: string;
  value: T;
  options: SegmentedControlOption<T>[];
  onChange: (value: T) => void;
}

/** Wraps Kobalte `segmented-control` (a radio group under the hood) for
 *  toggles like the Library grid/list view. Each option always renders a
 *  visible label — an icon, when supplied, is purely decorative. */
export function SegmentedControl<T extends string>(props: SegmentedControlProps<T>) {
  return (
    <KSegmentedControl
      class={styles.root}
      value={props.value}
      onChange={(value) => props.onChange(value as T)}
      aria-label={props.label}
    >
      <For each={props.options}>
        {(option) => (
          <KSegmentedControl.Item value={option.value} class={styles.item}>
            <KSegmentedControl.ItemInput />
            <KSegmentedControl.ItemLabel class={styles.itemLabel}>
              <Show when={option.icon}>
                <span class={styles.icon} aria-hidden="true">
                  {option.icon}
                </span>
              </Show>
              {option.label}
            </KSegmentedControl.ItemLabel>
          </KSegmentedControl.Item>
        )}
      </For>
      <KSegmentedControl.Indicator class={styles.indicator} />
    </KSegmentedControl>
  );
}
