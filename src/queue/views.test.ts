import { describe, expect, it } from "vitest";
import { eligibilitySummary, job, queueEntry } from "./test-records";
import { viewOf } from "./views";
import type { QueueView } from "./views";
import type { EligibilityVerdict, JobState } from "./types";

describe("viewOf", () => {
  describe("a queued entry maps to its eligibility summary's verdict", () => {
    const cases: [EligibilityVerdict, QueueView][] = [
      ["blocked", "blocked"],
      ["awaitingOperator", "awaitingOperator"],
      ["ready", "ready"],
    ];
    for (const [verdict, expected] of cases) {
      it(`verdict ${verdict} -> ${expected}`, () => {
        const entry = queueEntry({ state: "queued" });
        const summary = eligibilitySummary({ entryId: entry.id, verdict });
        expect(viewOf(entry, undefined, summary)).toBe(expected);
      });
    }

    it("throws without a summary (Rust always supplies one per queued entry)", () => {
      const entry = queueEntry({ state: "queued" });
      expect(() => viewOf(entry, undefined, undefined)).toThrow();
    });
  });

  describe("an assigned entry maps by its Job's state", () => {
    const nonPrintingStates: JobState[] = [
      "assigned", "staging", "awaitingStart", "starting", "completed", "failed", "cancelled", "outcomeUnknown",
    ];
    for (const state of nonPrintingStates) {
      it(`Job state ${state} -> assigned`, () => {
        const entry = queueEntry({ state: "assigned", jobId: "job-1" });
        expect(viewOf(entry, job({ id: "job-1", state }), undefined)).toBe("assigned");
      });
    }

    for (const state of ["printing", "paused"] as JobState[]) {
      it(`Job state ${state} -> printing`, () => {
        const entry = queueEntry({ state: "assigned", jobId: "job-1" });
        expect(viewOf(entry, job({ id: "job-1", state }), undefined)).toBe("printing");
      });
    }

    it("with no Job loaded yet -> assigned (never printing without proof)", () => {
      const entry = queueEntry({ state: "assigned", jobId: "job-1" });
      expect(viewOf(entry, undefined, undefined)).toBe("assigned");
    });
  });

  describe("a closed entry always maps to history", () => {
    it("regardless of its Job's state", () => {
      const entry = queueEntry({ state: "closed", closeReason: "completed", jobId: "job-1" });
      expect(viewOf(entry, job({ id: "job-1", state: "printing" }), undefined)).toBe("history");
    });

    it("with no Job and no summary", () => {
      const entry = queueEntry({ state: "closed", closeReason: "removed" });
      expect(viewOf(entry, undefined, undefined)).toBe("history");
    });
  });
});
