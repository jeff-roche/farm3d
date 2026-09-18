import { createSignal } from "solid-js";
import type { MonitorDensity } from "../generated/contracts/domain/MonitorDensity";
import type { MonitorSection } from "../generated/contracts/domain/MonitorSection";
import type { PrinterStatus } from "../generated/contracts/domain/PrinterStatus";
import type { ResolvedPrinter } from "../printers/types";

export type MonitorFilter = "all" | "attention" | "printing" | "ready" | "offline" | "setupIncomplete";
export type MonitorSeverity = "fatal" | "warning" | "info" | "resolved";

export interface MonitorPrinterView {
  id: string;
  name: string;
  vendor: string;
  model: string;
  modelLabel: string;
  catalogStatus: ResolvedPrinter["catalogStatus"];
  status?: PrinterStatus;
  operationalState?: PrinterStatus["operationalState"];
  readiness?: PrinterStatus["readiness"];
  freshness?: PrinterStatus["freshness"];
  severity: MonitorSeverity;
  hostActivity: PrinterStatus["telemetry"]["hostActivity"] | undefined;
  hostActivityName?: string;
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
  visiblePrinters(): readonly MonitorPrinterView[];
  sections(): readonly MonitorSectionView[];
  rosters(): readonly MonitorRosterView[];
  hasPrinters(): boolean;
  isFilteredEmpty(): boolean;
}

const UNLINKED_KEY = "__unlinked__";
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

function toView(printer: ResolvedPrinter): MonitorPrinterView {
  const status = printer.runtimeStatus;
  const telemetry = status?.telemetry;
  const state = status?.operationalState ?? "status unavailable";
  const detail = telemetry?.hostActivityName ?? status?.readiness.reason ?? "telemetry unavailable";

  return {
    id: printer.id,
    name: printer.name,
    vendor: printer.catalogRef.vendor,
    model: printer.catalogRef.model,
    modelLabel: printer.modelLabel,
    catalogStatus: printer.catalogStatus,
    status,
    operationalState: status?.operationalState,
    readiness: status?.readiness,
    freshness: status?.freshness,
    severity: severityFor(status),
    hostActivity: telemetry?.hostActivity,
    hostActivityName: telemetry?.hostActivityName,
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
    accessibleSummary: `${printer.name}; ${state}; ${detail}`,
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
  ]
    .filter((value): value is string => value !== undefined)
    .some((value) => value.toLocaleLowerCase().includes(normalized));
}

function matchesFilter(printer: MonitorPrinterView, filter: MonitorFilter): boolean {
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
    case "printerModel":
      return modelSection(printer);
  }
}

function orderSections(section: MonitorSectionView[]): MonitorSectionView[] {
  return section.sort((left, right) => {
    if (left.key === UNLINKED_KEY) return 1;
    if (right.key === UNLINKED_KEY) return -1;
    return left.label.localeCompare(right.label) || left.key.localeCompare(right.key);
  });
}

export function createMonitorStore(dependencies: MonitorStoreDependencies): MonitorStore {
  const [search, setSearch] = createSignal("");
  const [filter, setFilter] = createSignal<MonitorFilter>("all");
  const [section, setSectionSignal] = createSignal<MonitorSection>(dependencies.initialSection);
  const [density, setDensitySignal] = createSignal<MonitorDensity>(dependencies.initialDensity);
  const [selectedPrinterId, setSelectedPrinterId] = createSignal<string | null>(null);
  const [preferenceError, setPreferenceError] = createSignal<string | null>(null);
  let preferenceWrite: Promise<void> | undefined;

  const visiblePrinters = (): readonly MonitorPrinterView[] => dependencies.printers()
    .map(toView)
    .filter((printer) => matchesSearch(printer, search()) && matchesFilter(printer, filter()))
    .sort(compareByName);

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
    visiblePrinters,
    sections,
    rosters: () => sections().map((current) => ({
      key: current.key,
      label: current.label,
      count: current.printers.length,
      printers: current.printers.slice(0, ROSTER_LIMIT),
      remainingCount: Math.max(0, current.printers.length - ROSTER_LIMIT),
    })),
    hasPrinters: () => dependencies.printers().length > 0,
    isFilteredEmpty: () => dependencies.printers().length > 0 && visiblePrinters().length === 0,
  };
}
