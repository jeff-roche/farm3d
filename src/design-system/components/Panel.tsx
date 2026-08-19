import { splitProps, type JSX, type ParentProps } from "solid-js";
import styles from "./Panel.module.css";

export interface PanelProps extends ParentProps<JSX.HTMLAttributes<HTMLDivElement>> {
  title?: string;
  /** Rendered on the header's trailing edge, e.g. a close/collapse IconButton. */
  actions?: JSX.Element;
}

export function Panel(props: PanelProps) {
  const [local, rest] = splitProps(props, ["title", "actions", "children", "class"]);

  return (
    <div class={[styles.panel, local.class].filter(Boolean).join(" ")} {...rest}>
      {(local.title || local.actions) && (
        <div class={styles.header}>
          <span>{local.title}</span>
          {local.actions}
        </div>
      )}
      <div class={styles.body}>{local.children}</div>
    </div>
  );
}
