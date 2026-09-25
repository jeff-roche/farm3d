/** D20's continuous Preparation validation, on the frontend. `start_slice`
 *  checks again authoritatively; this is what the panel shows while
 *  editing, and what disables **Slice** with a reason. Pure.
 *
 *  It computes the issues the frontend has the data for. Two of D20's
 *  issues are not computed here, because nothing on the wire carries what
 *  they need: `nozzleMismatch` (the machine preset's nozzle) and
 *  `unsupportedSetting` (the runtime's known keys). They arrive only as
 *  `start_slice` errors. */
import { checkPlacement, type Footprint, type PlacementCheck } from "./bounds";
import type { InstanceDoc, PreparationDocument, SlicerRuntimeStatus, SliceOptions } from "./types";
import type { BuildVolume } from "./viewport/renderer";

export type PreparationIssue =
  /** The runtime can't slice (D2); the panel offers Slicer settings. */
  | { code: "runtimeUnavailable" }
  /** Pinned to an older source revision, and not chosen to continue. */
  | { code: "stale" }
  /** No preset chosen (`name: null`), or one the runtime doesn't offer. */
  | { code: "presetNotFound"; preset: "process" | "filament"; name: string | null }
  /** A filament preset the target's printer isn't offered (D3). */
  | { code: "filamentIncompatible"; name: string }
  | { code: "emptyPlate"; plateKey: string }
  | { code: "outOfBounds"; plateKey: string; instanceKey: string }
  | { code: "inExcludeArea"; plateKey: string; instanceKey: string }
  | { code: "tooTall"; plateKey: string; instanceKey: string };

export type PreparationIssueCode = PreparationIssue["code"];

export interface ValidationInput {
  document: PreparationDocument;
  /** An instance's footprint, or `undefined` while its mesh loads (or when
   *  its object doesn't exist). Callers memoise it per mesh, rotation and
   *  scale, since only a translation changes on a move. */
  footprint: (instance: InstanceDoc) => Footprint | undefined;
  /** The target's build volume; `null` until the slice options load, which
   *  skips the placement checks. */
  volume: BuildVolume | null;
  /** `list_slice_options` for the target; `null` until loaded, which skips
   *  the preset checks. */
  options: SliceOptions | null;
  /** `null` until the first snapshot, which skips the runtime check. */
  runtime: SlicerRuntimeStatus | null;
  /** The record is stale and the user hasn't chosen **Continue with
   *  revision M**. */
  stale: boolean;
}

export interface PreparationValidation {
  /** Whole-Preparation issues first (runtime, stale, presets), then each
   *  plate's in plate order, each instance's in instance order. */
  issues: PreparationIssue[];
  /** Each checked instance's placement, by `instanceKey`, for tinting and
   *  the object list's text marker. Absent while unchecked. */
  placement: Map<string, PlacementCheck>;
}

export function validatePreparation(input: ValidationInput): PreparationValidation {
  const issues: PreparationIssue[] = [];
  const placement = new Map<string, PlacementCheck>();
  const { document, options } = input;

  if (input.runtime && !input.runtime.canSlice) issues.push({ code: "runtimeUnavailable" });
  if (input.stale) issues.push({ code: "stale" });
  if (options) {
    const process = document.processPreset ?? null;
    if (process === null || !options.processPresets.some((preset) => preset.name === process)) {
      issues.push({ code: "presetNotFound", preset: "process", name: process });
    }
    const filament = document.filamentPreset ?? null;
    if (filament === null) {
      issues.push({ code: "presetNotFound", preset: "filament", name: null });
    } else if (!options.filamentPresets.some((preset) => preset.name === filament)) {
      // `list_slice_options` offers only the filaments compatible with the
      // target's printer, so one it doesn't offer is incompatible (D3).
      issues.push({ code: "filamentIncompatible", name: filament });
    }
  }

  for (const plate of document.plates) {
    if (plate.instances.length === 0) {
      issues.push({ code: "emptyPlate", plateKey: plate.plateKey });
      continue;
    }
    if (!input.volume) continue;
    for (const instance of plate.instances) {
      const footprint = input.footprint(instance);
      if (!footprint) continue;
      const check = checkPlacement(footprint, instance.transform.translateMm, input.volume);
      placement.set(instance.instanceKey, check);
      const at = { plateKey: plate.plateKey, instanceKey: instance.instanceKey };
      if (check.outOfBounds) issues.push({ code: "outOfBounds", ...at });
      if (check.inExcludeArea) issues.push({ code: "inExcludeArea", ...at });
      if (check.tooTall) issues.push({ code: "tooTall", ...at });
    }
  }
  return { issues, placement };
}

/** The issues that block slicing `plateKeys`: every whole-Preparation
 *  issue, and the plate issues on those plates (**Slice plate** passes the
 *  current tab's key; **Slice all plates** every key). */
export function issuesForPlates(issues: readonly PreparationIssue[], plateKeys: readonly string[]): PreparationIssue[] {
  const keys = new Set(plateKeys);
  return issues.filter((issue) => !("plateKey" in issue) || keys.has(issue.plateKey));
}
