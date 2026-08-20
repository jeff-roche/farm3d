import { IconArrowBackUp } from "@tabler/icons-solidjs";
import { splitProps, type ParentProps } from "solid-js";
import { IconButton } from "./IconButton";
import styles from "./Field.module.css";

export interface FieldProps extends ParentProps {
  label: string;
  hint?: string;
  /** Whether this field's value diverges from the catalog. Draws the accent
   *  border and reveals the revert control. */
  overridden?: boolean;
  onRevert?: () => void;
  class?: string;
}

export function Field(props: FieldProps) {
  const [local, rest] = splitProps(props, [
    "label",
    "hint",
    "overridden",
    "onRevert",
    "children",
    "class",
  ]);

  return (
    <div
      class={[styles.field, local.overridden ? styles.overridden : "", local.class]
        .filter(Boolean)
        .join(" ")}
      {...rest}
    >
      <div class={styles.header}>
        <span class={styles.label}>{local.label}</span>
        {local.overridden && local.onRevert && (
          <IconButton
            aria-label={`Revert ${local.label} to inherited`}
            title={local.hint ?? "Revert to inherited"}
            onClick={local.onRevert}
          >
            <IconArrowBackUp size={14} />
          </IconButton>
        )}
      </div>
      <div class={styles.control}>{local.children}</div>
      {local.hint && <span class={styles.hint}>{local.hint}</span>}
    </div>
  );
}
