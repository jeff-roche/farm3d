import { Switch as KSwitch } from "@kobalte/core/switch";
import { splitProps, type ParentProps } from "solid-js";
import styles from "./Switch.module.css";

export interface SwitchProps extends ParentProps {
  checked?: boolean;
  defaultChecked?: boolean;
  onChange?: (checked: boolean) => void;
  disabled?: boolean;
  class?: string;
}

export function Switch(props: SwitchProps) {
  const [local, rest] = splitProps(props, ["children", "class"]);

  return (
    <KSwitch class={[styles.root, local.class].filter(Boolean).join(" ")} {...rest}>
      <KSwitch.Input />
      <KSwitch.Control class={styles.control}>
        <KSwitch.Thumb class={styles.thumb} />
      </KSwitch.Control>
      {local.children && <KSwitch.Label class={styles.label}>{local.children}</KSwitch.Label>}
    </KSwitch>
  );
}
