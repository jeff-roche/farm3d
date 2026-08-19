import { Button as KButton } from "@kobalte/core/button";
import { splitProps, type JSX } from "solid-js";
import styles from "./Button.module.css";

export interface ButtonProps
  extends Omit<JSX.ButtonHTMLAttributes<HTMLButtonElement>, "type"> {
  variant?: "primary" | "secondary" | "ghost" | "danger";
  size?: "sm" | "md";
  type?: "button" | "submit" | "reset";
}

export function Button(props: ButtonProps) {
  const [local, rest] = splitProps(props, ["variant", "size", "class"]);
  const variant = () => local.variant ?? "secondary";
  const size = () => local.size ?? "md";

  return (
    <KButton
      class={[
        styles.button,
        styles[variant()],
        size() === "sm" ? styles.sm : "",
        local.class,
      ]
        .filter(Boolean)
        .join(" ")}
      {...rest}
    />
  );
}
