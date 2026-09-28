import type { EligibilitySummary, Job, QueueEntry } from "./types";

/** The Queue screen's one grouping axis (spec "Frontend architecture").
 *  `viewOf` maps only Rust-provided fields -- it never derives eligibility,
 *  Job state, or ordering itself (global constraint 4). */
export type QueueView = "awaitingOperator" | "ready" | "assigned" | "blocked" | "printing" | "history";

/** `viewOf(entry, job, summary)`:
 *  - `closed` -> `history`, regardless of the Job or summary;
 *  - `assigned` with its Job `printing` or `paused` -> `printing`;
 *  - any other `assigned` -> `assigned`;
 *  - `queued` -> the summary's `verdict` (`blocked`, `awaitingOperator`,
 *    or `ready`; every `EligibilityVerdict` is already a `QueueView`). */
export function viewOf(entry: QueueEntry, job: Job | undefined, summary: EligibilitySummary | undefined): QueueView {
  if (entry.state === "closed") return "history";
  if (entry.state === "assigned") {
    if (job && (job.state === "printing" || job.state === "paused")) return "printing";
    return "assigned";
  }
  // `queued`
  if (!summary) throw new Error(`viewOf: a queued entry needs its eligibility summary (entry ${entry.id})`);
  return summary.verdict;
}
