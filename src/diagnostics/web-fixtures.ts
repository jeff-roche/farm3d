/** Deterministic `just web` fixtures for the diagnostics store. Reads only;
 *  exports, cleanups, and resets need the desktop (spec D18). Synthetic
 *  values, no real network details. */
import type { AboutInfo, DiagnosticsPreview, ResetPreview, ResetTier, StorageUsage } from "./types";

export function webDiagnosticsPreview(): DiagnosticsPreview {
  return {
    sections: [
      { section: "about", estimatedBytes: 512 },
      { section: "health", estimatedBytes: 2_048 },
      { section: "storage", estimatedBytes: 1_024 },
      { section: "configuration", estimatedBytes: 3_072 },
      { section: "logs", estimatedBytes: 262_144 },
      { section: "recentProblems", estimatedBytes: 4_096 },
    ],
  };
}

export function webStorageUsage(): StorageUsage {
  return {
    classes: [
      { class: "database", bytes: 1_310_720, count: 1 },
      { class: "contentModelSources", bytes: 5_242_880, count: 6 },
      { class: "contentThumbnails", bytes: 204_800, count: 6 },
      { class: "contentGcode", bytes: 3_145_728, count: 4 },
      { class: "contentSliceArtifacts", bytes: 819_200, count: 5 },
      { class: "contentSlicerLogs", bytes: 40_960, count: 5 },
      { class: "contentUnreferenced", bytes: 122_880, count: 2 },
      { class: "cameraMediaPinned", bytes: 412_000, count: 2 },
      { class: "cameraMediaUnpinned", bytes: 2_738_000, count: 7 },
      { class: "safetyBackups", bytes: 3_948_000, count: 2 },
      { class: "preImportSnapshots", bytes: 0, count: 0 },
      { class: "logs", bytes: 262_144, count: 2 },
      { class: "slicerProfileCache", bytes: 20_971_520, count: 1 },
    ],
    totalBytes: 39_218_808,
    measuredAt: "2026-09-29T09:00:00Z",
  };
}

export function webAboutInfo(): AboutInfo {
  return {
    appVersion: "0.9.0",
    schemaVersion: 10,
    backupFormatVersion: 1,
    platform: { os: "linux", arch: "x86_64" },
    catalog: { sourceTag: "web-fixture", generatedAt: "2026-09-01T00:00:00Z" },
    slicer: { configured: false, version: null },
    credentialStore: { kind: "keychain", available: true },
  };
}

const PHRASES: Record<ResetTier, string> = { settings: "reset settings", cameraMedia: "reset media", farm: "reset farm" };

export function webResetPreview(tier: ResetTier): ResetPreview {
  if (tier === "settings") {
    return {
      tier, phrase: PHRASES[tier], warnings: [],
      classes: [
        { class: "settings", effect: "reset", count: 1, bytes: null },
        { class: "printers", effect: "kept", count: 4, bytes: null },
      ],
    };
  }
  if (tier === "cameraMedia") {
    return {
      tier, phrase: PHRASES[tier], warnings: [],
      classes: [
        { class: "unpinnedSnapshots", effect: "pruned", count: 7, bytes: 2_738_000 },
        { class: "pinnedSnapshots", effect: "kept", count: 2, bytes: 412_000 },
        { class: "snapshotRecords", effect: "kept", count: 9, bytes: null },
      ],
    };
  }
  return {
    tier, phrase: PHRASES[tier],
    warnings: [{ kind: "activeWork", activeJobs: 1, hostOperations: 0, sliceOperations: 0 }],
    classes: [
      { class: "printers", effect: "reset", count: 4, bytes: null },
      { class: "spools", effect: "reset", count: 10, bytes: null },
      { class: "queueAndJobs", effect: "reset", count: 6, bytes: null },
      { class: "credentials", effect: "deleted", count: 2, bytes: null },
      { class: "safetyBackups", effect: "kept", count: 2, bytes: 3_948_000 },
    ],
  };
}
