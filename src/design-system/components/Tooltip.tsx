import { Tooltip as KTooltip } from "@kobalte/core/tooltip";
import type { JSX, ParentProps } from "solid-js";
import styles from "./Tooltip.module.css";

export interface TooltipProps extends ParentProps {
  /** Rendered as the content of Kobalte's own trigger element — pass text/icon content, not another button. */
  trigger: JSX.Element;
  openDelay?: number;
}

export function Tooltip(props: TooltipProps) {
  return (
    <KTooltip openDelay={props.openDelay}>
      <KTooltip.Trigger as="span">{props.trigger}</KTooltip.Trigger>
      <KTooltip.Portal>
        <KTooltip.Content class={styles.content}>{props.children}</KTooltip.Content>
      </KTooltip.Portal>
    </KTooltip>
  );
}
