import type { MonitorPrinterView } from "../monitor/monitor-store";

const operationalLabels: Record<NonNullable<MonitorPrinterView["operationalState"]>, string> = {
  setupIncomplete: "Setup incomplete",
  error: "Error",
  offline: "Offline",
  connecting: "Connecting",
  printing: "Printing",
  paused: "Paused",
  busy: "Busy",
  ready: "Ready",
  unknown: "Unknown",
};

const readinessLabels = {
  setupIncomplete: "Setup incomplete",
  connectionError: "Connection error",
  offline: "Offline",
  refreshing: "Refreshing status",
  staleTelemetry: "Stale telemetry",
  printerBusy: "Printer busy",
  unknownState: "Unknown state",
} as const;

export function operationalLabel(printer: MonitorPrinterView): string {
  return printer.operationalState ? operationalLabels[printer.operationalState] : "Status unavailable";
}

export function detailLabel(printer: MonitorPrinterView): string {
  if (printer.freshness === "unavailable") return "Telemetry unavailable";
  if (printer.hostActivityName) {
    return `${printer.hostActivity === "printing" ? "Host print" : "Host activity"}: ${printer.hostActivityName}`;
  }
  if (printer.hostActivity && printer.hostActivity !== "unknown") {
    return `${printer.hostActivity === "printing" ? "Host print" : "Host activity"}: ${hostActivityLabel(printer.hostActivity)}`;
  }
  const reason = printer.readiness?.reason;
  return reason ? readinessLabels[reason] : "Telemetry unavailable";
}

function hostActivityLabel(activity: NonNullable<MonitorPrinterView["hostActivity"]>): string {
  return activity.charAt(0).toUpperCase() + activity.slice(1);
}

export function freshnessLabel(printer: MonitorPrinterView): string | undefined {
  if (printer.freshness === "unavailable") return "Telemetry unavailable";
  if (printer.freshness !== "stale") return undefined;
  return printer.lastObservedAt
    ? `Stale; last seen ${formatAge(printer.lastObservedAt)}`
    : "Stale telemetry";
}

export function severityLabel(printer: MonitorPrinterView): string | undefined {
  if (printer.severity === "fatal") return "Connection error";
  if (printer.severity === "warning") return "Monitor cache warning";
  return undefined;
}

export function formatTemperature(current: number | undefined, target: number | undefined): string {
  return `${formatValue(current)} / ${formatValue(target)}`;
}

function formatValue(value: number | undefined): string {
  return value === undefined ? "—" : `${Math.round(value)} °C`;
}

function formatAge(timestamp: string): string {
  const elapsed = Date.now() - Date.parse(timestamp);
  if (!Number.isFinite(elapsed) || elapsed < 60_000) return "just now";
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  const hours = Math.floor(minutes / 60);
  return `${hours} hour${hours === 1 ? "" : "s"} ago`;
}
