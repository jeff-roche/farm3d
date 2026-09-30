/** Pure presentation for diagnostics, storage, and reset. Shows a file
 *  name at most, never a path. */
import type { DiagnosticsSection, ExportDiagnosticsOutcome, ResetResult, StorageCleanupTarget } from "./types";
import type { ResetClass } from "../generated/contracts/domain/ResetClass";
import type { ResetEffect } from "../generated/contracts/domain/ResetEffect";
import type { StorageClass } from "../generated/contracts/domain/StorageClass";

const SECTION_LABEL = {
  about: "About",
  health: "Printer health",
  storage: "Storage",
  configuration: "Configuration",
  logs: "Logs",
  recentProblems: "Recent problems",
} satisfies Record<DiagnosticsSection, string>;

const SECTION_DESCRIPTION = {
  about: "Version, platform, and catalog information.",
  health: "Each Printer's connection and capability state, under a pseudonym.",
  storage: "Storage sizes and data-integrity findings.",
  configuration: "Settings values and counts of what the Farm holds.",
  logs: "farm3d's log files, with identifiers replaced by pseudonyms.",
  recentProblems: "The 50 newest Attention events, without names.",
} satisfies Record<DiagnosticsSection, string>;

export const diagnosticsSectionLabel = (section: DiagnosticsSection): string => SECTION_LABEL[section];
export const diagnosticsSectionDescription = (section: DiagnosticsSection): string => SECTION_DESCRIPTION[section];

/** Shown beside Export, whatever the sections. */
export const DIAGNOSTICS_NEVER_INCLUDED =
  "Never included: credentials, host names or addresses, URLs, file paths, or names.";

const STORAGE_CLASS_LABEL = {
  database: "Database",
  contentModelSources: "Model sources",
  contentThumbnails: "Thumbnails",
  contentGcode: "G-code",
  contentSliceArtifacts: "Slice artifacts",
  contentSlicerLogs: "Slicer logs",
  contentUnreferenced: "Unreferenced content",
  cameraMediaPinned: "Camera images (pinned)",
  cameraMediaUnpinned: "Camera images (unpinned)",
  safetyBackups: "Safety backups",
  preImportSnapshots: "Pre-import snapshots",
  logs: "Logs",
  slicerProfileCache: "Slicer profile cache",
} satisfies Record<StorageClass, string>;

export const storageClassLabel = (value: StorageClass): string => STORAGE_CLASS_LABEL[value];

const CLEANUP_LABEL = {
  unreferencedContent: "Remove unreferenced content",
  preImportSnapshots: "Remove pre-import snapshots",
  rotatedLogs: "Remove rotated logs",
  orcaCache: "Clear the slicer profile cache",
} satisfies Record<StorageCleanupTarget, string>;

export const storageCleanupLabel = (target: StorageCleanupTarget): string => CLEANUP_LABEL[target];

/** The storage class each cleanup action sits beside. */
export const STORAGE_CLEANUP_CLASS = {
  unreferencedContent: "contentUnreferenced",
  preImportSnapshots: "preImportSnapshots",
  rotatedLogs: "logs",
  orcaCache: "slicerProfileCache",
} satisfies Record<StorageCleanupTarget, StorageClass>;

const RESET_CLASS_LABEL = {
  settings: "Settings",
  slicerRuntime: "Slicer setup",
  printerAlertDefaults: "Printer alert defaults",
  unpinnedSnapshots: "Unpinned camera images",
  pinnedSnapshots: "Pinned camera images",
  snapshotRecords: "Snapshot records",
  printers: "Printers",
  spools: "Spools",
  library: "Model library",
  sliceRevisions: "Slice revisions",
  queueAndJobs: "Queue and Jobs",
  incidentsAndAttention: "Incidents and Attention",
  content: "Stored content",
  cameraMedia: "Camera images",
  logs: "Logs",
  credentials: "Stored credentials",
  preImportSnapshots: "Pre-import snapshots",
  restoreStaging: "Restore staging",
  legacyArchives: "Legacy archives",
  safetyBackups: "Safety backups",
  slicerProfileCache: "Slicer profile cache",
} satisfies Record<ResetClass, string>;

const RESET_EFFECT_LABEL = {
  reset: "Reset",
  pruned: "Images removed",
  deleted: "Deleted",
  kept: "Kept",
} satisfies Record<ResetEffect, string>;

export const resetClassLabel = (value: ResetClass): string => RESET_CLASS_LABEL[value];
export const resetEffectLabel = (value: ResetEffect): string => RESET_EFFECT_LABEL[value];

export function formatBytes(bytes: number): string {
  if (bytes < 1024) return `${bytes} B`;
  const units = ["KB", "MB", "GB", "TB"];
  let value = bytes / 1024;
  let unit = 0;
  while (value >= 1024 && unit < units.length - 1) {
    value /= 1024;
    unit += 1;
  }
  return `${value.toFixed(value >= 10 ? 0 : 1)} ${units[unit]}`;
}

export function exportSummaryText(outcome: ExportDiagnosticsOutcome): string {
  switch (outcome.status) {
    case "exported": return `Exported ${outcome.fileName} (${formatBytes(outcome.bytes)}).`;
    case "cancelled": return "Export cancelled. Nothing was written.";
    case "unsupported": return "Exporting diagnostics needs the desktop app.";
  }
}

export function resetSummaryText(result: ResetResult): string {
  switch (result.tier) {
    case "settings": return "Settings were reset to their defaults.";
    case "cameraMedia": return `Removed ${result.prunedCount} camera ${result.prunedCount === 1 ? "image" : "images"}, freeing ${formatBytes(result.freedBytes)}.`;
    case "farm": return "Restarting farm3d to finish the reset.";
  }
}
