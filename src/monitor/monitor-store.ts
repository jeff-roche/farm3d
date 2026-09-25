import { createSignal } from "solid-js";
import type { MonitorDensity } from "../generated/contracts/domain/MonitorDensity";
import type { MonitorSection } from "../generated/contracts/domain/MonitorSection";
import type { PrinterStatus } from "../generated/contracts/domain/PrinterStatus";
import type { ResolvedPrinter } from "../printers/types";

export type MonitorFilter = "all" | "attention" | "printing" | "ready" | "offline" | "setupIncomplete" | "archived";
export type MonitorSeverity = "fatal" | "warning" | "info" | "resolved";

export interface MonitorPrinterView {
  id: string;
  name: string;
  vendor: string;
  model: string;
  modelLabel: string;
  location?: string;
  archived: boolean;
  catalogStatus: ResolvedPrinter["catalogStatus"];
  status?: PrinterStatus;
  operationalState?: PrinterStatus["operationalState"];
  operationalLabel: string;
  readiness?: PrinterStatus["readiness"];
  freshness?: PrinterStatus["freshness"];
  freshnessLabel?: string;
  severity: MonitorSeverity;
  severityLabel?: string;
  hostActivity: PrinterStatus["telemetry"]["hostActivity"] | undefined;
  hostActivityName?: string;
  statusSummary: string;
  hasMissingReadings: boolean;
  readings: Pick<
    PrinterStatus["telemetry"],
    "progress" | "nozzleTempC" | "nozzleTargetC" | "bedTempC" | "bedTargetC" | "printDurationS"
  >;
  lastObservedAt?: string;
  freshUntil?: string;
  updatedAt?: string;
  accessibleSummary: string;
}

export interface MonitorSectionView {
  key: string;
  label: string;
  printers: readonly MonitorPrinterView[];
}

export interface MonitorRosterView {
  key: string;
  label: string;
  count: number;
  printers: readonly MonitorPrinterView[];
  remainingCount: number;
}

export interface MonitorShellView {
  printerRoster: MonitorRosterView;
  operationalRosters: readonly MonitorRosterView[];
  adapterHealth: {
    severity: MonitorSeverity;
    label: string;
  };
  lastLiveEventAt?: string;
}

export interface MonitorStoreDependencies {
  printers: () => ResolvedPrinter[];
  initialSection: MonitorSection;
  initialDensity: MonitorDensity;
  persistPreferences: (next: {
    monitorSection: MonitorSection;
    monitorDensity: MonitorDensity;
  }) => Promise<void>;
}

export interface MonitorStore {
  search(): string;
  filter(): MonitorFilter;
  section(): MonitorSection;
  density(): MonitorDensity;
  selectedPrinterId(): string | null;
  preferenceError(): string | null;
  setSearch(next: string): void;
  setFilter(next: MonitorFilter): void;
  setSection(next: MonitorSection): void;
  setDensity(next: MonitorDensity): void;
  setSelectedPrinterId(next: string | null): void;
  selectedPrinter(): ResolvedPrinter | undefined;
  printerNames(): readonly string[];
  visiblePrinters(): readonly MonitorPrinterView[];
  sections(): readonly MonitorSectionView[];
  rosters(): readonly MonitorRosterView[];
  shell(): MonitorShellView;
  hasPrinters(): boolean;
  isFilteredEmpty(): boolean;
}

const UNLINKED_KEY = "__unlinked__";
const NO_LOCATION_KEY = "__no_location__";
const ROSTER_LIMIT = 8;

function compareByName(left: MonitorPrinterView, right: MonitorPrinterView): number {
  return left.name.localeCompare(right.name);
}

function severityFor(status: PrinterStatus | undefined): MonitorSeverity {
  if (!status) return "info";
  if (status.operationalState === "error") return "fatal";
  if (status.cacheWarnings.length > 0) return "warning";
  if (status.operationalState === "ready" && status.readiness.state === "ready") return "resolved";
  return "info";
}

const operationalLabels: Record<NonNullable<PrinterStatus["operationalState"]>, string> = {
  setupIncomplete: "Setup incomplete",
  error: "Error",
  offline: "Offline",
  connecting: "Connecting",
  printing: "Printing",
  paused: "Paused",
  busy: "Busy",
  finished: "Finished",
  cancelled: "Cancelled",
  failed: "Print failed",
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
  bedNeedsClearing: "Bed needs clearing",
  unknownState: "Unknown state",
  archived: "Archived",
} as const;

/** A Printer's operational state in words ("Ready", "Offline", …), or
 *  "Status unavailable" before any status arrives. */
export function operationalLabel(status: PrinterStatus | undefined): string {
  return status ? operationalLabels[status.operationalState] : "Status unavailable";
}

function statusSummary(status: PrinterStatus | undefined): string {
  if (!status || status.freshness === "unavailable") return "Telemetry unavailable";
  // An ended job's host name ("complete", "error") says less than the action
  // it calls for.
  if (status.readiness.reason === "bedNeedsClearing") return readinessLabels.bedNeedsClearing;
  const telemetry = status.telemetry;
  const activity = telemetry.hostActivity === "printing" ? "Host print" : "Host activity";
  const detail = telemetry.hostActivityName
    ? `${activity}: ${telemetry.hostActivityName}`
    : telemetry.hostActivity !== "unknown"
      ? `${activity}: ${capitalize(telemetry.hostActivity)}`
      : status.readiness.reason
        ? readinessLabels[status.readiness.reason]
        : "Telemetry unavailable";
  return status.operationalState === "printing" && status.freshness === "fresh" && telemetry.progress !== undefined
    ? `${detail} · ${Math.round(telemetry.progress * 100)}%`
    : detail;
}

function freshnessLabel(status: PrinterStatus | undefined): string | undefined {
  if (!status || status.freshness === "fresh") return undefined;
  if (status.freshness === "unavailable") return "Telemetry unavailable";
  return status.lastObservedAt ? `Stale; last seen ${formatAge(status.lastObservedAt)}` : "Stale telemetry";
}

function severityLabel(severity: MonitorSeverity): string | undefined {
  if (severity === "fatal") return "Connection error";
  if (severity === "warning") return "Monitor cache warning";
  return undefined;
}

function hasMissingReadings(status: PrinterStatus | undefined): boolean {
  const telemetry = status?.telemetry;
  return telemetry?.nozzleTempC === undefined || telemetry?.nozzleTargetC === undefined
    || telemetry?.bedTempC === undefined || telemetry?.bedTargetC === undefined;
}

function capitalize(value: string): string {
  return value.charAt(0).toUpperCase() + value.slice(1);
}

function formatAge(timestamp: string): string {
  const elapsed = Date.now() - Date.parse(timestamp);
  if (!Number.isFinite(elapsed) || elapsed < 60_000) return "just now";
  const minutes = Math.floor(elapsed / 60_000);
  if (minutes < 60) return `${minutes} minute${minutes === 1 ? "" : "s"} ago`;
  const hours = Math.floor(minutes / 60);
  return `${hours} hour${hours === 1 ? "" : "s"} ago`;
}

function toView(printer: ResolvedPrinter): MonitorPrinterView {
  const status = printer.runtimeStatus;
  const telemetry = status?.telemetry;
  const severity = severityFor(status);
  const currentOperationalLabel = operationalLabel(status);
  const currentStatusSummary = statusSummary(status);
  const currentFreshnessLabel = freshnessLabel(status);

  return {
    id: printer.id,
    name: printer.name,
    vendor: printer.catalogRef.vendor,
    model: printer.catalogRef.model,
    modelLabel: printer.modelLabel,
    location: printer.location,
    archived: Boolean(printer.archivedAt),
    catalogStatus: printer.catalogStatus,
    status,
    operationalState: status?.operationalState,
    operationalLabel: currentOperationalLabel,
    readiness: status?.readiness,
    freshness: status?.freshness,
    freshnessLabel: currentFreshnessLabel,
    severity,
    severityLabel: severityLabel(severity),
    hostActivity: telemetry?.hostActivity,
    hostActivityName: telemetry?.hostActivityName,
    statusSummary: currentStatusSummary,
    hasMissingReadings: hasMissingReadings(status),
    readings: {
      progress: telemetry?.progress,
      nozzleTempC: telemetry?.nozzleTempC,
      nozzleTargetC: telemetry?.nozzleTargetC,
      bedTempC: telemetry?.bedTempC,
      bedTargetC: telemetry?.bedTargetC,
      printDurationS: telemetry?.printDurationS,
    },
    lastObservedAt: status?.lastObservedAt,
    freshUntil: status?.freshUntil,
    updatedAt: status?.updatedAt,
    accessibleSummary: [printer.name, currentOperationalLabel, currentStatusSummary, currentFreshnessLabel]
      .filter((item, index, items) => Boolean(item) && items.indexOf(item) === index)
      .join("; "),
  };
}

function matchesSearch(printer: MonitorPrinterView, search: string): boolean {
  const normalized = search.trim().toLocaleLowerCase();
  if (!normalized) return true;
  return [
    printer.name,
    printer.vendor,
    printer.model,
    `${printer.vendor} ${printer.model}`,
    printer.hostActivityName,
    printer.location,
  ]
    .filter((value): value is string => value !== undefined)
    .some((value) => value.toLocaleLowerCase().includes(normalized));
}

function matchesFilter(printer: MonitorPrinterView, filter: MonitorFilter): boolean {
  if (filter === "archived") return printer.archived;
  if (printer.archived) return false;
  switch (filter) {
    case "all":
      return true;
    case "attention":
      return printer.severity === "warning" || printer.severity === "fatal";
    case "printing":
      return printer.operationalState === "printing" && printer.freshness === "fresh";
    case "ready":
      return printer.readiness?.state === "ready";
    case "offline":
      return printer.operationalState === "offline";
    case "setupIncomplete":
      return printer.operationalState === "setupIncomplete";
  }
}

function modelSection(printer: MonitorPrinterView): { key: string; label: string } {
  if (printer.catalogStatus !== "ok" && printer.catalogStatus !== "rematched") {
    return { key: UNLINKED_KEY, label: "Unlinked" };
  }
  return {
    key: `${printer.vendor}::${printer.model}`,
    label: printer.modelLabel,
  };
}

function locationSection(printer: MonitorPrinterView): { key: string; label: string } {
  const trimmed = printer.location?.trim();
  if (!trimmed) return { key: NO_LOCATION_KEY, label: "No location" };
  return { key: trimmed.toLocaleLowerCase(), label: trimmed };
}

function sectionFor(printer: MonitorPrinterView, section: MonitorSection): { key: string; label: string } {
  switch (section) {
    case "operationalState":
      return {
        key: printer.operationalState ?? "unavailable",
        label: printer.operationalState ?? "Unavailable",
      };
    case "none":
      return { key: "none", label: "" };
    case "location":
      return locationSection(printer);
    case "printerModel":
      return modelSection(printer);
  }
}

function orderSections(section: MonitorSectionView[]): MonitorSectionView[] {
  const isLast = (key: string) => key === UNLINKED_KEY || key === NO_LOCATION_KEY;
  return section.sort((left, right) => {
    if (isLast(left.key)) return isLast(right.key) ? 0 : 1;
    if (isLast(right.key)) return -1;
    return left.label.localeCompare(right.label) || left.key.localeCompare(right.key);
  });
}

function roster(key: string, label: string, printers: readonly MonitorPrinterView[]): MonitorRosterView {
  return {
    key,
    label,
    count: printers.length,
    printers: printers.slice(0, ROSTER_LIMIT),
    remainingCount: Math.max(0, printers.length - ROSTER_LIMIT),
  };
}

function adapterHealth(printers: readonly MonitorPrinterView[]): MonitorShellView["adapterHealth"] {
  const states = printers.flatMap((printer) => printer.status ? [printer.status.connectionState] : []);
  if (states.length === 0) return { severity: "info", label: "Waiting for adapter status" };
  if (states.includes("error")) return { severity: "fatal", label: "Adapter error" };
  if (states.includes("offline")) return { severity: "warning", label: "Adapters offline" };
  if (states.includes("connecting")) return { severity: "info", label: "Connecting adapters" };
  return { severity: "resolved", label: "Adapters connected" };
}

function latestLiveEventAt(printers: readonly MonitorPrinterView[]): string | undefined {
  return printers
    .filter((printer) => printer.freshness === "fresh")
    .map((printer) => printer.lastObservedAt)
    .filter((timestamp): timestamp is string => timestamp !== undefined && !Number.isNaN(Date.parse(timestamp)))
    .sort((left, right) => Date.parse(right) - Date.parse(left))[0];
}

export function createMonitorStore(dependencies: MonitorStoreDependencies): MonitorStore {
  const [search, setSearch] = createSignal("");
  const [filter, setFilter] = createSignal<MonitorFilter>("all");
  const [section, setSectionSignal] = createSignal<MonitorSection>(dependencies.initialSection);
  const [density, setDensitySignal] = createSignal<MonitorDensity>(dependencies.initialDensity);
  const [selectedPrinterId, setSelectedPrinterId] = createSignal<string | null>(null);
  const [preferenceError, setPreferenceError] = createSignal<string | null>(null);
  let preferenceWrite: Promise<void> | undefined;

  const snapshot = (): readonly MonitorPrinterView[] => dependencies.printers()
    .map(toView)
    .sort(compareByName);

  const visiblePrinters = (): readonly MonitorPrinterView[] => snapshot()
    .filter((printer) => matchesSearch(printer, search()) && matchesFilter(printer, filter()))
    ;

  const sections = (): readonly MonitorSectionView[] => {
    const grouped = new Map<string, MonitorSectionView>();
    for (const printer of visiblePrinters()) {
      const group = sectionFor(printer, section());
      const current = grouped.get(group.key);
      if (current) {
        current.printers = [...current.printers, printer];
      } else {
        grouped.set(group.key, { ...group, printers: [printer] });
      }
    }
    return orderSections([...grouped.values()]);
  };

  const queuePreferenceWrite = (): void => {
    const next = {
      monitorSection: section(),
      monitorDensity: density(),
    };
    const save = () => dependencies.persistPreferences(next)
      .then(
        () => {
          setPreferenceError(null);
        },
        () => {
          setPreferenceError("Monitor preferences could not be saved.");
        },
      );
    preferenceWrite = preferenceWrite
      ? preferenceWrite.then(save, save)
      : save();
  };

  return {
    search,
    filter,
    section,
    density,
    selectedPrinterId,
    preferenceError,
    setSearch,
    setFilter,
    setSection(next) {
      setSectionSignal(next);
      queuePreferenceWrite();
    },
    setDensity(next) {
      setDensitySignal(next);
      queuePreferenceWrite();
    },
    setSelectedPrinterId,
    selectedPrinter: () => dependencies.printers().find((printer) => printer.id === selectedPrinterId()),
    printerNames: () => dependencies.printers().map((printer) => printer.name),
    visiblePrinters,
    sections,
    rosters: () => sections().map((current) => roster(current.key, current.label, current.printers)),
    shell: () => {
      const printers = snapshot().filter((printer) => !printer.archived);
      return {
        printerRoster: roster("all", "Printers", printers),
        operationalRosters: [
          roster("ready", "Ready", printers.filter((printer) => printer.readiness?.state === "ready")),
          roster("printing", "Printing", printers.filter((printer) => printer.operationalState === "printing" && printer.freshness === "fresh")),
          roster("offline", "Offline", printers.filter((printer) => printer.operationalState === "offline")),
          roster("setupIncomplete", "Setup incomplete", printers.filter((printer) => printer.operationalState === "setupIncomplete")),
        ],
        adapterHealth: adapterHealth(printers),
        lastLiveEventAt: latestLiveEventAt(printers),
      };
    },
    hasPrinters: () => dependencies.printers().length > 0,
    isFilteredEmpty: () => dependencies.printers().length > 0 && visiblePrinters().length === 0,
  };
}
