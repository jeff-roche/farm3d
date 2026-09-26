import { IconAlertTriangle, IconCircleCheck, IconCircleMinus, IconCircleX, IconInfoCircle } from "@tabler/icons-solidjs";
import type { Component } from "solid-js";
import type { Severity } from "../host-ops/presentation";
import styles from "./SeverityLabel.module.css";

const ICONS: Record<Severity, Component<{ size?: number; "aria-hidden"?: "true" }>> = {
  info: IconInfoCircle,
  success: IconCircleCheck,
  warning: IconAlertTriangle,
  error: IconCircleX,
  neutral: IconCircleMinus,
};

/** A Host Operation state or capability label: icon, text, and colour
 *  together (spec "Accessibility"), never colour alone. Plain text, not a
 *  live region — the Job tab has exactly one of those. */
export function SeverityLabel(props: { severity: Severity; text: string }) {
  const Icon = () => {
    const Chosen = ICONS[props.severity];
    return <Chosen size={14} aria-hidden="true" />;
  };
  return (
    <span class={styles.label} data-severity={props.severity}>
      <Icon />
      <span>{props.text}</span>
    </span>
  );
}
