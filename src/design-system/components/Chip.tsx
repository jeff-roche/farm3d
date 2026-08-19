import { ToggleButton } from "@kobalte/core/toggle-button";
import { splitProps, type JSX, type ParentProps } from "solid-js";
import styles from "./Chip.module.css";

export interface ChipProps extends ParentProps {
  selected?: boolean;
  defaultSelected?: boolean;
  onSelectedChange?: (selected: boolean) => void;
  disabled?: boolean;
  onRemove?: () => void;
  class?: string;
}

export function Chip(props: ChipProps) {
  const [local, rest] = splitProps(props, [
    "selected",
    "defaultSelected",
    "onSelectedChange",
    "onRemove",
    "children",
    "class",
  ]);

  return (
    <ToggleButton
      class={[styles.chip, local.class].filter(Boolean).join(" ")}
      pressed={local.selected}
      defaultPressed={local.defaultSelected}
      onChange={local.onSelectedChange}
      {...rest}
    >
      {local.children}
      {local.onRemove && (
        <span
          class={styles.remove}
          role="button"
          tabIndex={-1}
          aria-label="Remove"
          onClick={(e: MouseEvent) => {
            e.stopPropagation();
            local.onRemove?.();
          }}
        >
          <RemoveIcon />
        </span>
      )}
    </ToggleButton>
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
