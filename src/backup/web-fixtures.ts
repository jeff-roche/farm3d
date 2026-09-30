/** Deterministic `just web` fixtures for the backup store. Reads only: web
 *  mode has no backend, so restores and backups are `unsupported`
 *  (spec D18). Synthetic names, RFC 5737 addresses only. */
import type { BackupInventory, BackupSummary, RestorePreview, RestoreStatus } from "./types";

export function webBackupInventory(): BackupInventory {
  return {
    databaseBytes: 1_310_720,
    content: { count: 14, bytes: 9_437_184 },
    media: [
      { choice: "none", count: 0, bytes: 0 },
      { choice: "pinned", count: 2, bytes: 412_000 },
      { choice: "all", count: 9, bytes: 3_150_000 },
    ],
    counts: [
      { table: "jobs", rows: 6 },
      { table: "printers", rows: 4 },
      { table: "spools", rows: 10 },
    ],
    credentialRefCount: 2,
    activeJobCount: 1,
    excluded: ["credentials", "slicerRuntimePaths", "printerStatusCache", "pendingCredentialCleanup", "pendingBlobCleanup", "logs"],
  };
}

export function webBackupList(): BackupSummary[] {
  return [
    { backupId: "bkp-web-before-restore", origin: "beforeRestore", createdAt: "2026-09-27T10:00:00Z", bytes: 2_048_000, appVersion: "0.9.0", schemaVersion: 11, media: "pinned", valid: true },
    { backupId: "bkp-web-before-reset", origin: "beforeReset", createdAt: "2026-09-26T08:30:00Z", bytes: 1_900_000, appVersion: "0.9.0", schemaVersion: 11, media: "none", valid: true },
    { backupId: "bkp-web-broken", origin: "beforeRestore", createdAt: null, bytes: 512, appVersion: null, schemaVersion: null, media: null, valid: false },
  ];
}

/** A preview a screen test or story can render; never returned by web-mode
 *  IPC (a web restore is `unsupported`). */
export function webRestorePreview(): RestorePreview {
  return {
    stagingId: "stg-web-1",
    createdAt: "2026-09-29T09:00:00Z",
    expiresAt: "2026-09-29T09:30:00Z",
    source: { kind: "file", fileName: "farm-2026-09-20.farm3d-backup" },
    backup: { createdAt: "2026-09-20T12:00:00Z", appVersion: "0.9.0", schemaVersion: 11, formatVersion: 1, origin: "operator", media: "pinned", platform: { os: "linux", arch: "x86_64" } },
    counts: [{ table: "jobs", local: 6, backup: 5 }, { table: "printers", local: 4, backup: 4 }],
    conflicts: [{
      class: "changed", domain: "printer", total: 1,
      items: [{ localId: "prt-web-1", backupId: "prt-web-1", label: "Bay 4 (name differs)" }],
    }],
    notices: [{ kind: "credentialsToReenter", printerCount: 2 }, { kind: "slicerRuntimeKeptLocal" }],
    blockers: [],
    blockerTotal: 0,
  };
}

export function webRestoreStatus(): RestoreStatus {
  return { state: "none" };
}
