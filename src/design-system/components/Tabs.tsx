import { Tabs as KTabs } from "@kobalte/core/tabs";
import { For, splitProps, type JSX } from "solid-js";
import styles from "./Tabs.module.css";

export interface TabItem {
  value: string;
  label: string;
  content: JSX.Element;
  disabled?: boolean;
}

export interface TabsProps {
  items: TabItem[];
  value?: string;
  defaultValue?: string;
  onChange?: (value: string) => void;
  class?: string;
}

export function Tabs(props: TabsProps) {
  const [local, rest] = splitProps(props, ["items", "class"]);

  return (
    <KTabs class={[styles.root, local.class].filter(Boolean).join(" ")} {...rest}>
      <KTabs.List class={styles.list}>
        <For each={local.items}>
          {(item) => (
            <KTabs.Trigger value={item.value} disabled={item.disabled} class={styles.trigger}>
              {item.label}
            </KTabs.Trigger>
          )}
        </For>
        <KTabs.Indicator class={styles.indicator} />
      </KTabs.List>
      <For each={local.items}>
        {(item) => (
          <KTabs.Content value={item.value} class={styles.content}>
            {item.content}
          </KTabs.Content>
        )}
      </For>
    </KTabs>
  );
}
