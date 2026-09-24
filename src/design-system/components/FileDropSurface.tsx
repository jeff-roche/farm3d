import { Show } from "solid-js";
import { Button } from "./Button";
import styles from "./FileDropSurface.module.css";

export interface FileDropSurfaceProps {
  /** Whether a Tauri file drag is currently hovering this surface. */
  active: boolean;
  disabled?: boolean;
  /** Shown as visible text (not only a tooltip) while `disabled`. */
  disabledReason?: string;
  /** Accessible name for the region; not rendered as visible heading text. */
  label: string;
  hint?: string;
  /** Defaults to "Choose files…". */
  chooseLabel?: string;
  onChoose: () => void;
}

/** The umbrella's file drop/import surface. Never reads browser `File`
 *  objects — the caller supplies `active` from Tauri drag events, and
 *  `onChoose` opens the native file dialog. Keyboard-operable via the
 *  real `Button`, without needing to drag. */
export function FileDropSurface(props: FileDropSurfaceProps) {
  return (
    <div
      role="region"
      aria-label={props.label}
      class={styles.surface}
      data-active={props.active ? "" : undefined}
      data-disabled={props.disabled ? "" : undefined}
    >
      <Show when={props.hint}>
        <p class={styles.hint}>{props.hint}</p>
      </Show>
      <Show when={props.disabled && props.disabledReason}>
        <p class={styles.disabledReason}>{props.disabledReason}</p>
      </Show>
      <Button type="button" disabled={props.disabled} onClick={props.onChoose}>
        {props.chooseLabel ?? "Choose files…"}
      </Button>
    </div>
  );
}
