import { Dialog as KDialog } from "@kobalte/core/dialog";
import { Show, type JSX, type ParentProps } from "solid-js";
import styles from "./Dialog.module.css";

export interface DialogProps extends ParentProps {
  title: string;
  description?: string;
  /** Rendered as the content of Kobalte's own trigger <button> — pass text/icon content, not another button.
   *  Omit it for a dialog opened only through the controlled `open` prop (e.g. a confirmation raised
   *  from another dialog), which then renders no trigger at all. */
  trigger?: JSX.Element;
  /** Extra class(es) for Kobalte's own trigger <button>, appended after the default trigger styling — e.g. to make it look like a primary Button. */
  triggerClass?: string;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
}

export function Dialog(props: DialogProps) {
  return (
    <KDialog open={props.open} onOpenChange={props.onOpenChange}>
      <Show when={props.trigger !== undefined}>
        <KDialog.Trigger class={[styles.trigger, props.triggerClass].filter(Boolean).join(" ")}>
          {props.trigger}
        </KDialog.Trigger>
      </Show>
      <KDialog.Portal>
        <KDialog.Overlay class={styles.overlay} />
        <KDialog.Content class={styles.content}>
          <div class={styles.header}>
            <KDialog.Title class={styles.title}>{props.title}</KDialog.Title>
            <KDialog.CloseButton class={styles.closeButton} aria-label="Close">
              <CloseIcon />
            </KDialog.CloseButton>
          </div>
          {props.description && (
            <KDialog.Description class={styles.description}>
              {props.description}
            </KDialog.Description>
          )}
          {props.children}
        </KDialog.Content>
      </KDialog.Portal>
    </KDialog>
  );
}

function CloseIcon() {
  return (
    <svg width="14" height="14" viewBox="0 0 10 10" fill="none" aria-hidden="true">
      <path
        d="M1 1L9 9M9 1L1 9"
        stroke="currentColor"
        stroke-width="1.5"
        stroke-linecap="round"
      />
    </svg>
  );
}
