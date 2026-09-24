import { ToggleButton } from "@kobalte/core/toggle-button";
import { createUniqueId, Show, splitProps, type JSX, type ParentProps } from "solid-js";
import styles from "./Chip.module.css";

export interface ChipProps extends ParentProps {
  selected?: boolean;
  defaultSelected?: boolean;
  onSelectedChange?: (selected: boolean) => void;
  disabled?: boolean;
  onRemove?: () => void;
  class?: string;
}

/** A toggle chip. With `onRemove`, a separate native remove button sits
 *  beside the toggle (never inside it: a button can't contain another), in
 *  the tab order and named after the chip, e.g. "Remove Brackets". */
export function Chip(props: ChipProps) {
  const [local, rest] = splitProps(props, [
    "selected",
    "defaultSelected",
    "onSelectedChange",
    "onRemove",
    "children",
    "class",
  ]);
  const toggleId = createUniqueId();
  const removeId = createUniqueId();

  const toggle = () => (
    <ToggleButton
      id={toggleId}
      class={[styles.chip, local.class].filter(Boolean).join(" ")}
      data-removable={local.onRemove ? "" : undefined}
      pressed={local.selected}
      defaultPressed={local.defaultSelected}
      onChange={local.onSelectedChange}
      {...rest}
    >
      {local.children}
    </ToggleButton>
  );

  return (
    <Show when={local.onRemove} fallback={toggle()}>
      {(onRemove) => (
        <span class={styles.removable}>
          {toggle()}
          <button
            type="button"
            id={removeId}
            class={styles.remove}
            aria-label="Remove"
            aria-labelledby={`${removeId} ${toggleId}`}
            disabled={rest.disabled}
            onClick={() => onRemove()()}
          >
            <RemoveIcon />
          </button>
        </span>
      )}
    </Show>
  );
}

function RemoveIcon(): JSX.Element {
  return (
    <svg width="10" height="10" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path
        d="M1 1L9 9M9 1L1 9"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
      />
    </svg>
  );
}
