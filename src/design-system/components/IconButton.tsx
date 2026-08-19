import { Button as KButton } from "@kobalte/core/button";
import { splitProps, type JSX } from "solid-js";
import styles from "./IconButton.module.css";

export interface IconButtonProps
  extends Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  /** Required — icon-only controls must have an accessible name. */
  "aria-label": string;
  /** Purely visual "selected" state; this component does not manage toggle semantics. */
  active?: boolean;
  type?: "button" | "submit" | "reset";
}

export function IconButton(props: IconButtonProps) {
  const [local, rest] = splitProps(props, ["active", "class"]);

  return (
    <KButton
      class={[styles.button, local.active ? styles.active : "", local.class]
        .filter(Boolean)
        .join(" ")}
      {...rest}
    />
  );
}
