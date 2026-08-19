import { DropdownMenu as KDropdownMenu } from "@kobalte/core/dropdown-menu";
import { For, type JSX } from "solid-js";
import styles from "./DropdownMenu.module.css";

export interface MenuItem {
  type?: "item";
  label: string;
  onSelect: () => void;
  disabled?: boolean;
}

export interface MenuSeparator {
  type: "separator";
}

export type DropdownMenuEntry = MenuItem | MenuSeparator;

export interface DropdownMenuProps {
  /** Rendered as the content of Kobalte's own trigger element — pass text/icon content, not another button. */
  trigger: JSX.Element;
  items: DropdownMenuEntry[];
}

export function DropdownMenu(props: DropdownMenuProps) {
  return (
    <KDropdownMenu>
      <KDropdownMenu.Trigger as="span">{props.trigger}</KDropdownMenu.Trigger>
      <KDropdownMenu.Portal>
        <KDropdownMenu.Content class={styles.content}>
          <For each={props.items}>
            {(entry) =>
              entry.type === "separator" ? (
                <KDropdownMenu.Separator class={styles.separator} />
              ) : (
                <KDropdownMenu.Item
                  class={styles.item}
                  disabled={entry.disabled}
                  onSelect={entry.onSelect}
                >
                  {entry.label}
                </KDropdownMenu.Item>
              )
            }
          </For>
        </KDropdownMenu.Content>
      </KDropdownMenu.Portal>
    </KDropdownMenu>
  );
}
