import { describe, expect, it } from "vitest";
import { backupOriginLabel, groupConflicts, restoreBlockerLabel, restoreNoticeText, restoreStatusText } from "./presentation";
import type { RestoreConflictGroup, RestoreNotice } from "./types";

const item = (n: number) => ({ localId: `l${n}`, backupId: null, label: `row ${n}` });

describe("backup presentation", () => {
  it("groups conflicts by class then domain with an 'and N more' tail", () => {
    const groups: RestoreConflictGroup[] = [
      { class: "changed", domain: "printer", total: 1, items: [item(1)] },
      { class: "changed", domain: "job", total: 250, items: Array.from({ length: 200 }, (_, i) => item(i)) },
      { class: "uniqueClash", domain: "spool", total: 2, items: [item(1), item(2)] },
    ];
    const views = groupConflicts(groups);
    expect(views.map((v) => v.class)).toEqual(["changed", "uniqueClash"]);
    expect(views[0].total).toBe(251);
    expect(views[0].domains.map((d) => d.label)).toEqual(["Printers", "Jobs"]);
    expect(views[0].domains[0].moreText).toBeNull();
    expect(views[0].domains[1].moreText).toBe("and 50 more");
  });

  it("words every notice kind", () => {
    const notices: RestoreNotice[] = [
      { kind: "credentialsToReenter", printerCount: 2 }, { kind: "credentialsOrphaned", refCount: 1 },
      { kind: "linkedPathsMissing", modelCount: 3 }, { kind: "activeJobsAtBackup", jobCount: 1 },
      { kind: "mediaNotInBackup", snapshotCount: 4, missingFileCount: 1 }, { kind: "slicerRuntimeKeptLocal" },
      { kind: "migrated", fromSchemaVersion: 9 },
    ];
    for (const notice of notices) expect(restoreNoticeText(notice).length).toBeGreaterThan(10);
    expect(restoreNoticeText({ kind: "credentialsToReenter", printerCount: 1 })).toContain("1 Printer needs");
  });

  it("labels blockers, origins, and the status banner", () => {
    expect(restoreBlockerLabel({ kind: "activeJob", id: "j" })).toBe("An active Job");
    expect(backupOriginLabel("beforeReset")).toContain("reset");
    const done = restoreStatusText({ state: "done", journalId: "j", kind: "restore", finishedAt: "t", safetyBackupId: "bkp-1" });
    expect(done).toContain("bkp-1");
    expect(restoreStatusText({ state: "failed", journalId: "j", kind: "reset", finishedAt: "t", code: "INTERNAL", failedStep: null, safetyBackupId: null })).toContain("did not finish");
  });
});
