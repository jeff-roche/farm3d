import { describe, expect, it } from "vitest";
import { isMaterialItem, jobHistoryStateLabel, pruneReasonText, timelineItemDetail, timelineItemLabel, timelineSourceLabel } from "./presentation";
import { webJobTimeline } from "./web-fixtures";
import type { PruneReason } from "./types";

describe("history presentation", () => {
  it("labels every state and prune reason", () => {
    expect(jobHistoryStateLabel("outcomeUnknown")).toBe("Outcome unknown");
    const reasons: PruneReason[] = ["age", "diskCap", "missingFile", "notInBackup", "reset"];
    for (const reason of reasons) expect(pruneReasonText(reason)).not.toBe("");
    expect(pruneReasonText("reset")).toBe("Removed by a reset");
    expect(pruneReasonText("notInBackup")).toBe("Not in the backup");
  });

  it("labels every timeline item of the web fixture", () => {
    const timeline = webJobTimeline("job-web-completed");
    expect(timeline?.items.length).toBeGreaterThan(0);
    for (const item of timeline?.items ?? []) {
      expect(timelineItemLabel(item)).not.toBe("");
      expect(timelineSourceLabel(item.source)).not.toBe("");
    }
    expect(timeline?.items.some(isMaterialItem)).toBe(true);
  });

  it("gives every timeline item a textual detail", () => {
    for (const id of ["job-web-completed", "job-web-deferred", "job-web-host-cancelled", "job-web-outcome-unknown"]) {
      for (const item of webJobTimeline(id)?.items ?? []) {
        expect(timelineItemDetail(item), `${id} ${item.source}`).not.toBe("");
      }
    }
  });

  it("words a deduction, a correction, and pruned evidence", () => {
    const items = webJobTimeline("job-web-completed")?.items ?? [];
    const deduction = items.find((item) => item.source === "amountEvent" && !item.isCorrection);
    const correction = items.find((item) => item.source === "amountEvent" && item.isCorrection);
    const pruned = items.find((item) => item.source === "snapshot" && item.snapshot.prunedAt !== null);
    expect(deduction && timelineItemDetail(deduction)).toMatch(/ g/);
    expect(correction && timelineItemLabel(correction)).toBe("Correction");
    expect(pruned && timelineItemDetail(pruned)).toContain("Evidence pruned");
  });

  it("the web timeline fixture carries every item kind for the screenshots", () => {
    const items = webJobTimeline("job-web-completed")?.items ?? [];
    const sources = new Set(items.map((item) => item.source));
    for (const source of ["job", "reservation", "amountEvent", "attention", "incident", "snapshot"]) {
      expect(sources.has(source as never), source).toBe(true);
    }
    const amounts = items.flatMap((item) => (item.source === "amountEvent" ? [item.isCorrection] : []));
    expect(amounts).toContain(true);
    expect(amounts).toContain(false);
    const snapshots = items.flatMap((item) => (item.source === "snapshot" ? [item.snapshot.prunedAt !== null] : []));
    expect(snapshots).toContain(true);
    expect(snapshots).toContain(false);
    expect(webJobTimeline("job-web-completed")?.incident).not.toBeNull();
  });
});
