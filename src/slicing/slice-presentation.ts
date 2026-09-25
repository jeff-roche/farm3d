/** The words and limits the Preparation panel and the slice operation rows
 *  show (D4, D11, D20, D22), kept apart from the components so they can be
 *  checked on their own. Pure. */
import type { CommandError } from "../generated/contracts/command/CommandError";
import { isCommandError } from "../ipc/client";
import type {
  BrimType,
  EngineCandidate,
  InfillPattern,
  SliceControls,
  SliceFailure,
  SliceOperationState,
  SlicerRuntimeStatus,
  SupportMode,
} from "./types";

// --- Controls (D4) ------------------------------------------------------------------

export type NumericControl =
  | "layerHeightMm"
  | "wallLoops"
  | "topShellLayers"
  | "bottomShellLayers"
  | "infillDensityPercent"
  | "supportThresholdAngleDeg"
  | "brimWidthMm"
  | "skirtLoops";

export interface ControlLimits {
  min: number;
  /** `undefined` for the layer height, whose limit is the nozzle's. */
  max?: number;
  step: number;
  /** Whole numbers only. */
  integer: boolean;
}

/** D4's allowed values, the same as the backend's `validate_controls`. */
export const CONTROL_LIMITS: Record<NumericControl, ControlLimits> = {
  layerHeightMm: { min: 0.05, step: 0.01, integer: false },
  wallLoops: { min: 1, max: 20, step: 1, integer: true },
  topShellLayers: { min: 0, max: 50, step: 1, integer: true },
  bottomShellLayers: { min: 0, max: 50, step: 1, integer: true },
  infillDensityPercent: { min: 0, max: 100, step: 1, integer: false },
  supportThresholdAngleDeg: { min: 0, max: 90, step: 1, integer: false },
  brimWidthMm: { min: 0, max: 20, step: 0.5, integer: false },
  skirtLoops: { min: 0, max: 10, step: 1, integer: true },
};

/** A lay-flat face's area: two significant figures below 10 mm² (a
 *  sphere's facets are fractions of one), whole square millimetres above. */
export function formatFaceArea(areaMm2: number): string {
  return areaMm2 < 10
    ? areaMm2.toLocaleString(undefined, { maximumSignificantDigits: 2 })
    : Math.round(areaMm2).toLocaleString();
}

/** D4: the layer height is at most 80% of the nozzle diameter. Rounded
 *  down to a thousandth, so it never exceeds the backend's limit. */
export function layerHeightMax(nozzleDiameterMm: number): number {
  return Math.floor(nozzleDiameterMm * 0.8 * 1000 + 1e-9) / 1000;
}

/** Keeps a typed value inside D4's range (and whole where it must be), so
 *  an out-of-range value never reaches the document. `max` overrides the
 *  table's (the layer height's nozzle limit). */
export function clampControl(control: NumericControl, value: number, max?: number): number {
  const limits = CONTROL_LIMITS[control];
  const upper = max ?? limits.max ?? Number.POSITIVE_INFINITY;
  const rounded = limits.integer ? Math.round(value) : value;
  return Math.min(upper, Math.max(limits.min, rounded));
}

/** D4's control table the other way round: an OrcaSlicer key to the
 *  control that writes it, for linking `UNSUPPORTED_SETTING_FOR_RUNTIME`
 *  to its field. */
export const SETTING_KEY_CONTROLS: Record<string, keyof SliceControls> = {
  layer_height: "layerHeightMm",
  wall_loops: "wallLoops",
  top_shell_layers: "topShellLayers",
  bottom_shell_layers: "bottomShellLayers",
  sparse_infill_density: "infillDensityPercent",
  sparse_infill_pattern: "infillPattern",
  enable_support: "supports",
  support_type: "supports",
  support_threshold_angle: "supportThresholdAngleDeg",
  brim_type: "brimType",
  brim_width: "brimWidthMm",
  skirt_loops: "skirtLoops",
};

/** Every control the panel has, by its `SliceControls` name. */
const CONTROL_FIELDS: ReadonlySet<string> = new Set(Object.values(SETTING_KEY_CONTROLS));

/** A panel field an issue or error can move focus to. */
export type PanelField = "target" | "material" | "quality" | keyof SliceControls;

/** Each field's name, for links to it ("Go to Walls"). */
export const FIELD_LABELS: Record<PanelField, string> = {
  target: "Target",
  material: "Material",
  quality: "Quality",
  layerHeightMm: "Layer height",
  wallLoops: "Walls",
  topShellLayers: "Top shells",
  bottomShellLayers: "Bottom shells",
  infillDensityPercent: "Infill density",
  infillPattern: "Infill pattern",
  supports: "Supports",
  supportThresholdAngleDeg: "Overhang angle",
  brimType: "Brim",
  brimWidthMm: "Brim width",
  skirtLoops: "Skirt loops",
};

/** The names of D4's choices, shown by the panel's Selects and the Slice
 *  Revision review. */
export const INFILL_PATTERN_LABELS: Record<InfillPattern, string> = {
  rectilinear: "Rectilinear",
  grid: "Grid",
  line: "Line",
  cubic: "Cubic",
  gyroid: "Gyroid",
  honeycomb: "Honeycomb",
  lightning: "Lightning",
};

export const SUPPORT_MODE_LABELS: Record<SupportMode, string> = {
  off: "Off",
  "normal(auto)": "Normal (auto)",
  "tree(auto)": "Tree (auto)",
};

export const BRIM_TYPE_LABELS: Record<BrimType, string> = {
  no_brim: "No brim",
  outer_only: "Outer only",
  auto_brim: "Auto",
};

// --- Operations (D10, D11) ------------------------------------------------------------

const STATE_LABELS: Record<SliceOperationState, string> = {
  queued: "Waiting",
  running: "Slicing",
  succeeded: "Sliced",
  failed: "Failed",
  cancelled: "Cancelled",
  interrupted: "Interrupted",
};

export function operationStateLabel(state: SliceOperationState): string {
  return STATE_LABELS[state];
}

/** D11's text for each failure code. The two codes with a payload the
 *  user needs (the engine's own error string, the output check's reason)
 *  use the backend's message. `storageFailed` and `internalError` are
 *  farm3d's own faults, not OrcaSlicer's. */
export function failureText(failure: SliceFailure): string {
  const code = failure.code;
  switch (code.kind) {
    case "objectsOutsidePlate":
      return "An object is outside the printable area.";
    case "presetInvalid":
      return "OrcaSlicer couldn't read the presets farm3d prepared.";
    case "inputMissing":
      return "OrcaSlicer couldn't find its input.";
    case "inputInvalid":
      return "OrcaSlicer couldn't read the prepared plate.";
    case "presetIncompatible":
      return "The quality preset isn't compatible with this printer.";
    case "engineError":
      return failure.message || `OrcaSlicer failed with return code ${code.returnCode}.`;
    case "outputMissing":
      return "OrcaSlicer reported success but wrote no G-code.";
    case "outputInvalid":
      return failure.message || code.reason;
    case "timeout":
      return "Slicing took longer than 30 minutes.";
    case "engineCrashed":
      return `OrcaSlicer stopped unexpectedly (signal ${code.signal}).`;
    case "spawnFailed":
      return "farm3d couldn't start OrcaSlicer.";
    case "storageFailed":
      return "farm3d couldn't save the slice result. Check there is free disk space, then slice again.";
    case "internalError":
      return "Something went wrong inside farm3d while slicing. Slice again; the log may say more.";
  }
}

// --- Runtime (D2, D22) ------------------------------------------------------------------

/** D22's text for presets farm3d can't read (a cache-only nightly build). */
export const PRESETS_UNREADABLE =
  "This OrcaSlicer build stores its presets in a format farm3d can't read. Choose an OrcaSlicer 2.4 install or AppImage as the preset source.";

/** Why the runtime can't slice, one sentence per part that isn't
 *  available. Basenames only: full paths belong to the Settings section. */
export function runtimeProblems(runtime: SlicerRuntimeStatus): string[] {
  const problems: string[] = [];
  const engine = runtime.engine;
  switch (engine.state) {
    case "notFound":
      problems.push("OrcaSlicer wasn't found.");
      break;
    case "unsupportedVersion":
      problems.push(`${engine.executableName} is OrcaSlicer ${engine.version}; farm3d works with OrcaSlicer 2.x.`);
      break;
    case "probeFailed":
      problems.push(`${engine.executableName} couldn't be run: ${engine.reason}`);
      break;
  }
  const presets = runtime.presetSource;
  switch (presets.state) {
    case "notConfigured":
      problems.push("No OrcaSlicer presets are set up.");
      break;
    case "presetsUnreadable":
      problems.push(PRESETS_UNREADABLE);
      break;
    case "unavailable":
      problems.push(`The OrcaSlicer presets couldn't be read: ${presets.reason}`);
      break;
  }
  return problems;
}

/** Where D2's discovery found an engine, in words. */
export const CANDIDATE_SOURCES: Record<EngineCandidate["source"], string> = {
  configured: "chosen in Settings",
  path: "on PATH",
  wellKnown: "in a usual download folder",
};

/** D2's discovery, one line per engine tried, in order: which was used and
 *  why the others weren't. Empty when discovery found nothing to try. */
export function candidateLines(runtime: SlicerRuntimeStatus): string[] {
  return runtime.engineCandidates.map((candidate) => {
    const where = `${candidate.executableName} (${CANDIDATE_SOURCES[candidate.source]})`;
    const result = candidate.result;
    switch (result.kind) {
      case "chosen":
        return `${where}: used, OrcaSlicer ${result.version}.`;
      case "notChosen":
        return `${where}: OrcaSlicer ${result.version}, not used.`;
      case "unsupportedVersion":
        return `${where}: OrcaSlicer ${result.version} isn't supported.`;
      case "probeFailed":
        return `${where}: couldn't be run: ${result.reason}`;
    }
  });
}

/** Said when discovery tried nothing at all. */
export const NOTHING_TRIED =
  "farm3d looked for orca-slicer on PATH and for OrcaSlicer AppImages in ~/Applications, ~/.local/bin and ~/Downloads.";

// --- start_slice errors ---------------------------------------------------------------

/** How the panel presents a `start_slice` refusal: the backend's message,
 *  and where it points. */
export interface SliceErrorView {
  message: string;
  /** The field to move focus to (a preset, the target, or a control). */
  field?: PanelField;
  /** `PREPARATION_INVALID` names a plate. */
  plateKey?: string;
  /** `PREPARATION_STALE`: the recovery is the stale banner. */
  stale: boolean;
  /** Offer **Open Slicer settings**. */
  openSettings: boolean;
  /** No answer came back (a transport failure): **Try again** resends
   *  the same operation id. */
  retry: boolean;
  /** Already announced elsewhere (the editor's notice), so it is shown
   *  without an alert of its own. */
  announced?: boolean;
}

const PRESET_FIELDS: Record<string, PanelField> = {
  process: "quality",
  filament: "material",
  machine: "target",
};

function detail(error: CommandError, key: string): string | undefined {
  const value = error.details?.[key];
  return typeof value === "string" ? value : undefined;
}

/** The field a `VALIDATION` error's `fieldPath` names, if the panel has it. */
function fieldAt(path: string | undefined): PanelField | undefined {
  if (!path) return undefined;
  if (path.startsWith("controls.")) {
    const control = path.slice("controls.".length);
    return CONTROL_FIELDS.has(control) ? (control as PanelField) : undefined;
  }
  if (path === "processPreset") return "quality";
  if (path === "filamentPreset") return "material";
  if (path === "target" || path.startsWith("target.")) return "target";
  return undefined;
}

export function sliceErrorView(error: unknown): SliceErrorView {
  if (!isCommandError(error)) {
    return {
      message: "farm3d didn't hear back about starting the slice.",
      stale: false,
      openSettings: false,
      retry: true,
    };
  }
  const view: SliceErrorView = {
    message: error.message,
    stale: error.code === "PREPARATION_STALE",
    openSettings: error.recovery.includes("OPEN_SLICER_SETTINGS"),
    retry: false,
  };
  switch (error.code) {
    case "FILAMENT_INCOMPATIBLE":
      view.field = "material";
      break;
    case "PRESET_NOT_FOUND":
    case "PRESET_INVALID":
      view.field = PRESET_FIELDS[detail(error, "kind") ?? ""];
      break;
    case "UNSUPPORTED_SETTING_FOR_RUNTIME":
      view.field = SETTING_KEY_CONTROLS[detail(error, "key") ?? ""];
      break;
    case "UNMAPPED_PROFILE_OVERRIDE":
      view.field = "target";
      break;
    case "PREPARATION_INVALID":
      view.plateKey = detail(error, "plateKey");
      break;
    case "VALIDATION":
      view.field = fieldAt(detail(error, "fieldPath"));
      break;
  }
  return view;
}

// --- Logs (D9) ----------------------------------------------------------------------

export interface LogSegment {
  text: string;
  /** A known, harmless line (D9's "unable to open display"), shown dimmed
   *  but never hidden. */
  noise: boolean;
}

/** Splits a log into runs of ordinary lines and single noise lines, so a
 *  long log renders as a few text nodes rather than one element per line.
 *  `noiseLines` are 1-based. Joining the segments gives back `text`. */
export function logSegments(text: string, noiseLines: readonly number[]): LogSegment[] {
  if (text === "") return [];
  const noise = new Set(noiseLines);
  const lines = text.split("\n");
  const segments: LogSegment[] = [];
  let run: string[] = [];
  const flush = () => {
    if (run.length > 0) segments.push({ text: run.join(""), noise: false });
    run = [];
  };
  lines.forEach((line, index) => {
    const withBreak = index < lines.length - 1 ? `${line}\n` : line;
    if (noise.has(index + 1)) {
      flush();
      segments.push({ text: withBreak, noise: true });
    } else {
      run.push(withBreak);
    }
  });
  flush();
  return segments.filter((segment) => segment.text !== "");
}
