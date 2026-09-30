import { describe, expect, it } from "vitest";
import { isMaterialItem, jobHistoryStateLabel, pruneReasonText, timelineItemLabel, timelineSourceLabel } from "./presentation";
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
});
