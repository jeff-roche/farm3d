import { Popover as KPopover } from "@kobalte/core/popover";
import { Show, type Accessor, type JSX, type ParentProps } from "solid-js";
import styles from "./Popover.module.css";

export interface PopoverProps extends ParentProps {
  /** Rendered as the content of Kobalte's own trigger <button>. Omit when the popover is opened externally (e.g. anchored to another element via anchorRef) rather than via its own trigger. */
  trigger?: JSX.Element;
  open?: boolean;
  onOpenChange?: (open: boolean) => void;
  /** Anchors the popover to an element other than its own trigger. */
  anchorRef?: Accessor<HTMLElement | undefined>;
  /**
   * Traps focus inside the content and makes outside-focus events not dismiss it. Kobalte's
   * non-modal popover never moves focus into its content on open, so without an internal
   * trigger to anchor its own focus-restoration around, it can see focus as "already outside"
   * immediately after opening and self-dismiss. Popover renders no backdrop, so `modal` doesn't
   * dim the page — it only firms up focus/dismiss handling. Defaults to `false`.
   */
  modal?: boolean;
}

export function Popover(props: PopoverProps) {
  return (
    <KPopover
      open={props.open}
      onOpenChange={props.onOpenChange}
      anchorRef={props.anchorRef}
      modal={props.modal}
    >
      <Show when={props.trigger}>
        <KPopover.Trigger>{props.trigger}</KPopover.Trigger>
      </Show>
      <KPopover.Portal>
        <KPopover.Content class={styles.content}>{props.children}</KPopover.Content>
      </KPopover.Portal>
    </KPopover>
  );
}
