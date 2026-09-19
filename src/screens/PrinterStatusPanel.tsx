import { For } from "solid-js";
import type { ResolvedPrinter } from "../printers/types";
import { formatTemperature } from "./monitor-printer-presentation";
import styles from "./PrinterStatusPanel.module.css";

export interface PrinterStatusPanelProps {
  printer: ResolvedPrinter;
  syncState?: "syncing" | "current" | "uncertain";
}

const readinessLabels = {
  setupIncomplete: "Setup incomplete",
  connectionError: "Connection error",
  offline: "Offline",
  refreshing: "Refreshing status",
  staleTelemetry: "Stale telemetry",
  printerBusy: "Printer busy",
  unknownState: "Unknown state",
} as const;

export function PrinterStatusPanel(props: PrinterStatusPanelProps) {
  const status = () => props.printer.runtimeStatus;
  const readings = () => status()?.telemetry;
  const fields = () => [
    ["Operational state", status()?.operationalState ?? "Unavailable"],
    ["Readiness", status()?.readiness.state ?? "Unavailable"],
    ["Reason", status()?.readiness.reason ? readinessLabels[status()!.readiness.reason!] : "—"],
    ["Connection", status()?.connectionState ?? "Unavailable"],
    ["Host activity", readings()?.hostActivityName ?? readings()?.hostActivity ?? "—"],
    ["Progress", readings()?.progress === undefined ? "—" : `${Math.round(readings()!.progress!)}%`],
    ["Nozzle", formatTemperature(readings()?.nozzleTempC, readings()?.nozzleTargetC)],
    ["Bed", formatTemperature(readings()?.bedTempC, readings()?.bedTargetC)],
    ["Freshness", status()?.freshness ?? "Unavailable"],
    ["Last observed", status()?.lastObservedAt ?? "—"],
  ];

  return (
    <div class={styles.panel}>
      <p class={styles.summary}>Operational status and retained adapter telemetry.</p>
      <dl class={styles.fields}>
        <For each={fields()}>{([label, value]) => <div class={styles.field}><dt>{label}</dt><dd>{value}</dd></div>}</For>
      </dl>
      <p class={styles.sync} aria-live="polite">
        {props.syncState === "uncertain" ? "Live status is still reconciling." : ""}
      </p>
    </div>
  );
}
