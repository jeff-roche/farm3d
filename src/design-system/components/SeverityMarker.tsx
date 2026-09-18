import {
  IconAlertTriangle,
  IconCircleCheck,
  IconCircleX,
  IconInfoCircle,
} from "@tabler/icons-solidjs";
import type { Component } from "solid-js";
import styles from "./SeverityMarker.module.css";

export interface SeverityMarkerProps {
  severity: "fatal" | "warning" | "info" | "resolved";
  label: string;
}

const icons: Record<SeverityMarkerProps["severity"], Component> = {
  fatal: IconCircleX,
  warning: IconAlertTriangle,
  info: IconInfoCircle,
  resolved: IconCircleCheck,
};

export function SeverityMarker(props: SeverityMarkerProps) {
  const Icon = icons[props.severity];

  return (
    <span
      class={[styles.marker, styles[props.severity]].join(" ")}
      data-severity={props.severity}
      role="status"
      aria-label={props.label}
    >
      <Icon aria-hidden="true" />
      <span>{props.label}</span>
    </span>
  );
}
