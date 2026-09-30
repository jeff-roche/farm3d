/** Pure presentation for backup and restore: groups Rust's conflicts and
 *  words its notices and blockers. Rust owns what is a conflict, a blocker,
 *  or a notice; nothing here decides. */
import type { BackupOrigin, RestoreBlocker, RestoreConflictGroup, RestoreNotice, RestorePreview } from "./types";
import type { RestoreConflictClass } from "../generated/contracts/domain/RestoreConflictClass";
import type { RestoreDomain } from "../generated/contracts/domain/RestoreDomain";

const CLASS_LABEL = {
  onlyLocal: "Only on this Farm (would be removed)",
  changed: "Changed (the backup's version replaces this Farm's)",
  uniqueClash: "Clashes (same identity, different record)",
} satisfies Record<RestoreConflictClass, string>;

const DOMAIN_LABEL = {
  settings: "Settings",
  printer: "Printers",
  spool: "Spools",
  tare: "Tare weights",
  project: "Projects",
  model: "Models",
  sliceRevision: "Slice revisions",
  queueEntry: "Queue entries",
  job: "Jobs",
  incident: "Incidents",
  attentionEvent: "Attention events",
  snapshot: "Camera snapshots",
} satisfies Record<RestoreDomain, string>;

export const restoreConflictClassLabel = (value: RestoreConflictClass): string => CLASS_LABEL[value];
export const restoreDomainLabel = (value: RestoreDomain): string => DOMAIN_LABEL[value];

export interface ConflictDomainView {
  domain: RestoreDomain;
  label: string;
  total: number;
  items: RestoreConflictGroup["items"];
  /** Items past the 200-item cap: "and N more". */
  moreCount: number;
  moreText: string | null;
}

export interface ConflictClassView {
  class: RestoreConflictClass;
  label: string;
  total: number;
  domains: ConflictDomainView[];
}

/** Groups by class, then domain, keeping Rust's order. */
export function groupConflicts(groups: RestoreConflictGroup[]): ConflictClassView[] {
  const views: ConflictClassView[] = [];
  for (const group of groups) {
    let view = views.find((candidate) => candidate.class === group.class);
    if (!view) {
      view = { class: group.class, label: CLASS_LABEL[group.class], total: 0, domains: [] };
      views.push(view);
    }
    const moreCount = Math.max(0, group.total - group.items.length);
    view.total += group.total;
    view.domains.push({
      domain: group.domain,
      label: DOMAIN_LABEL[group.domain],
      total: group.total,
      items: group.items,
      moreCount,
      moreText: moreCount > 0 ? `and ${moreCount} more` : null,
    });
  }
  return views;
}

const plural = (count: number, one: string, many: string): string => `${count} ${count === 1 ? one : many}`;

export function restoreNoticeText(notice: RestoreNotice): string {
  switch (notice.kind) {
    case "credentialsToReenter":
      return `${plural(notice.printerCount, "Printer needs", "Printers need")} its credentials entered again; backups never hold credentials.`;
    case "credentialsOrphaned":
      return `${plural(notice.refCount, "stored credential", "stored credentials")} on this computer will no longer belong to any Printer.`;
    case "linkedPathsMissing":
      return `${plural(notice.modelCount, "linked Model", "linked Models")} may point at files that don't exist on this computer.`;
    case "activeJobsAtBackup":
      return `${plural(notice.jobCount, "Job was", "Jobs were")} active when the backup was made; check ${notice.jobCount === 1 ? "it" : "them"} after the restore.`;
    case "mediaNotInBackup":
      return `${plural(notice.snapshotCount, "camera snapshot", "camera snapshots")} ${notice.snapshotCount === 1 ? "has" : "have"} no image in the backup`
        + (notice.missingFileCount > 0 ? ` (${notice.missingFileCount} already missing on disk).` : ".");
    case "slicerRuntimeKeptLocal":
      return "The slicer setup on this computer is kept; the backup's is not restored.";
    case "migrated":
      return `The backup is from an older version (schema ${notice.fromSchemaVersion}) and will be upgraded.`;
  }
}

const BLOCKER_LABEL = {
  activeJob: "An active Job",
  hostOperation: "A Printer command in flight",
  sliceOperation: "A slice in progress",
} satisfies Record<RestoreBlocker["kind"], string>;

export const restoreBlockerLabel = (blocker: RestoreBlocker): string => BLOCKER_LABEL[blocker.kind];

const ORIGIN_LABEL = {
  operator: "Backup",
  beforeRestore: "Safety backup before a restore",
  beforeReset: "Safety backup before a reset",
} satisfies Record<BackupOrigin, string>;

export const backupOriginLabel = (origin: BackupOrigin): string => ORIGIN_LABEL[origin];

/** The banner's one-line outcome (kept with the banner's own store). */
export { restoreStatusText } from "./restore-status-store";

export function previewSourceText(preview: RestorePreview): string {
  return preview.source.kind === "file" ? preview.source.fileName : `Safety backup ${preview.source.backupId}`;
}
