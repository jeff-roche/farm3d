/** The words the Slice Revision list and review show (D12, D15, D21), kept
 *  apart from the components so they can be checked on their own. Pure. */
import { formatGrams } from "../spools/weight";
import { materialLabel } from "../spools/materials";
import {
  BRIM_TYPE_LABELS,
  FIELD_LABELS,
  INFILL_PATTERN_LABELS,
  SUPPORT_MODE_LABELS,
} from "./slice-presentation";
import type {
  BedShape,
  ClaimedEstimates,
  FactProvenance,
  ProfileSnapshot,
  SliceControls,
  SliceEstimates,
  SliceFacts,
  SliceRevisionSummary,
  SliceRuntimeInfo,
} from "./types";

// --- Identity --------------------------------------------------------------------

/** "Plate 1: Lid" for a farm3d revision; an external one has no plate (D1). */
export function revisionTitle(revision: Pick<SliceRevisionSummary, "kind" | "plate">): string {
  const plate = revision.plate;
  if (revision.kind === "external" || !plate) return "External G-code";
  return plate.plateName ? `Plate ${plate.plateIndex}: ${plate.plateName}` : `Plate ${plate.plateIndex}`;
}

export function revisionKindLabel(kind: SliceRevisionSummary["kind"]): string {
  return kind === "farm3d" ? "Sliced by farm3d" : "External G-code";
}

// --- Provenance (D15) ------------------------------------------------------------

export const PROVENANCE_LABELS: Record<FactProvenance, string> = {
  farm3dInput: "From farm3d settings",
  operatorConfirmed: "Confirmed by you",
  absent: "Not provided",
};

export const NEEDS_MANUAL_PRINTER = "Needs manual Printer selection";

// --- Estimates (D12) --------------------------------------------------------------

/** "1 h 30 min", "23 min 41 s", "41 s". */
export function formatPrintTime(seconds: number): string {
  const whole = Math.max(0, Math.round(seconds));
  const hours = Math.floor(whole / 3600);
  const minutes = Math.floor((whole % 3600) / 60);
  const rest = whole % 60;
  if (hours > 0) return minutes > 0 ? `${hours} h ${minutes} min` : `${hours} h`;
  if (minutes > 0) return rest > 0 ? `${minutes} min ${rest} s` : `${minutes} min`;
  return `${rest} s`;
}

/** Metres from a metre up ("12.94 m"), millimetres below ("412 mm"). */
export function formatFilamentLength(mm: number): string {
  return mm >= 1000 ? `${(mm / 1000).toFixed(2)} m` : `${Math.round(mm)} mm`;
}

/** Grams to one decimal, rounded as the Spools screens round. */
export function formatFilamentWeight(grams: number): string {
  return formatGrams(Math.round(grams * 1000), 1);
}

export function formatMm(value: number): string {
  return `${Number(value.toFixed(3))} mm`;
}

export interface EstimateRow {
  label: string;
  value: string;
}

/** The estimates that are known, in reading order. A missing one is left
 *  out rather than shown as zero. */
export function estimateRows(estimates: SliceEstimates | ClaimedEstimates): EstimateRow[] {
  const rows: EstimateRow[] = [];
  if (estimates.printSeconds !== null) rows.push({ label: "Print time", value: formatPrintTime(estimates.printSeconds) });
  if (estimates.filamentMm !== null) rows.push({ label: "Filament length", value: formatFilamentLength(estimates.filamentMm) });
  if (estimates.filamentGrams !== null) rows.push({ label: "Filament weight", value: formatFilamentWeight(estimates.filamentGrams) });
  if (estimates.layerCount !== null) rows.push({ label: "Layers", value: estimates.layerCount.toLocaleString() });
  if (estimates.maxZMm !== null) rows.push({ label: "Height", value: formatMm(estimates.maxZMm) });
  return rows;
}

/** The short filament figure a list entry shows: weight if known, else
 *  length. */
export function filamentSummary(estimates: SliceEstimates | null): string | undefined {
  if (!estimates) return undefined;
  if (estimates.filamentGrams !== null) return formatFilamentWeight(estimates.filamentGrams);
  if (estimates.filamentMm !== null) return formatFilamentLength(estimates.filamentMm);
  return undefined;
}

// --- Facts and target (D15) -------------------------------------------------------

export function bedLabel(bed: BedShape): string {
  return bed.kind === "rectangular"
    ? `${Number(bed.widthMm.toFixed(1))} × ${Number(bed.depthMm.toFixed(1))} mm`
    : `Custom shape, ${bed.points.length} corners`;
}

function humanize(raw: string): string {
  const spaced = raw.replace(/_/g, " ").trim();
  return spaced ? spaced[0].toUpperCase() + spaced.slice(1) : raw;
}

export interface FactRow {
  label: string;
  value: string;
}

/** A profile snapshot as label/value rows. */
export function profileRows(profile: ProfileSnapshot): FactRow[] {
  return [
    { label: "Printer profile", value: profile.catalogRef.variant },
    { label: "Bed", value: bedLabel(profile.bedShape) },
    { label: "Printable height", value: formatMm(profile.printableHeightMm) },
    ...(profile.bedExcludeAreas.length > 0
      ? [{ label: "Excluded bed area", value: `${profile.bedExcludeAreas.length} corners` }]
      : []),
    { label: "Nozzle type", value: humanize(profile.nozzleType) },
    { label: "G-code flavor", value: humanize(profile.gcodeFlavor) },
  ];
}

export type FactKey = "printerProfile" | "nozzleDiameterMm" | "materialFamily" | "filamentDiameterMm";

export const FACT_LABELS: Record<FactKey, string> = {
  printerProfile: "Printer profile",
  nozzleDiameterMm: "Nozzle diameter",
  materialFamily: "Material",
  filamentDiameterMm: "Filament diameter",
};

export const FACT_KEYS: readonly FactKey[] = ["printerProfile", "nozzleDiameterMm", "materialFamily", "filamentDiameterMm"];

/** A fact's value as text, or `undefined` when it is absent. */
export function factValueText(facts: SliceFacts, key: FactKey): string | undefined {
  switch (key) {
    case "printerProfile":
      return facts.printerProfile.value?.catalogRef.variant;
    case "nozzleDiameterMm":
      return facts.nozzleDiameterMm.value === null ? undefined : formatMm(facts.nozzleDiameterMm.value);
    case "materialFamily":
      return facts.materialFamily.value === null ? undefined : materialLabel(facts.materialFamily.value, facts.materialOther);
    case "filamentDiameterMm":
      return facts.filamentDiameterMm.value === null ? undefined : formatMm(facts.filamentDiameterMm.value);
  }
}

// --- Controls (D4) ------------------------------------------------------------------

/** The controls a farm3d revision set, as rows; the rest came from the
 *  process preset. */
export function controlRows(controls: SliceControls): FactRow[] {
  const rows: FactRow[] = [];
  const add = (key: keyof SliceControls, value: string | undefined) => {
    if (value !== undefined) rows.push({ label: FIELD_LABELS[key], value });
  };
  const num = (value: number | undefined, unit = "") => (value === undefined ? undefined : `${value}${unit}`);
  add("layerHeightMm", controls.layerHeightMm === undefined ? undefined : formatMm(controls.layerHeightMm));
  add("wallLoops", num(controls.wallLoops));
  add("topShellLayers", num(controls.topShellLayers));
  add("bottomShellLayers", num(controls.bottomShellLayers));
  add("infillDensityPercent", num(controls.infillDensityPercent, "%"));
  add("infillPattern", controls.infillPattern && INFILL_PATTERN_LABELS[controls.infillPattern]);
  add("supports", controls.supports && SUPPORT_MODE_LABELS[controls.supports]);
  add("supportThresholdAngleDeg", num(controls.supportThresholdAngleDeg, "°"));
  add("brimType", controls.brimType && BRIM_TYPE_LABELS[controls.brimType]);
  add("brimWidthMm", controls.brimWidthMm === undefined ? undefined : formatMm(controls.brimWidthMm));
  add("skirtLoops", num(controls.skirtLoops));
  return rows;
}

// --- Runtime (D21) ------------------------------------------------------------------

/** "Engine 2.5.0-dev (prerelease) · presets 2.4.2". */
export function runtimeLine(runtime: SliceRuntimeInfo): string {
  const tag = (channel: SliceRuntimeInfo["engineChannel"]) => (channel === "prerelease" ? " (prerelease)" : "");
  return `Engine ${runtime.engineVersion}${tag(runtime.engineChannel)} · presets ${runtime.presetSourceVersion}${tag(runtime.presetSourceChannel)}`;
}

export function runtimeVersionsDiffer(runtime: SliceRuntimeInfo): boolean {
  return runtime.engineVersion !== runtime.presetSourceVersion;
}

export function isPrerelease(runtime: SliceRuntimeInfo | undefined): boolean {
  return runtime?.engineChannel === "prerelease" || runtime?.presetSourceChannel === "prerelease";
}
