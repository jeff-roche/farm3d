/** Pure presentation for Job History and the Job timeline: labels for every
 *  `JobHistoryState`, `JobTimelineItem` source, and `PruneReason`. Rust
 *  supplies the values; nothing here decides anything. */
import { conditionKindLabel, incidentEntryKindLabel } from "../attention/presentation";
import type { JobEventKind } from "../generated/contracts/domain/JobEventKind";
import type { JobHistoryState, JobTimelineItem, PruneReason } from "./types";

const STATE_LABEL = {
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
  outcomeUnknown: "Outcome unknown",
} satisfies Record<JobHistoryState, string>;

export function jobHistoryStateLabel(state: JobHistoryState): string {
  return STATE_LABEL[state];
}

const PRUNE_REASON_TEXT = {
  age: "Removed after the retention period",
  diskCap: "Removed to stay under the disk cap",
  missingFile: "The image file is missing",
  notInBackup: "Not in the backup",
  reset: "Removed by a reset",
} satisfies Record<PruneReason, string>;

/** Why a snapshot's image is gone (text, never the image). */
export function pruneReasonText(reason: PruneReason): string {
  return PRUNE_REASON_TEXT[reason];
}

const SOURCE_LABEL = {
  job: "Job",
  hostOperation: "Printer command",
  reservation: "Material reservation",
  amountEvent: "Material",
  requirement: "Reconciliation",
  attention: "Attention",
  incident: "Incident",
  snapshot: "Snapshot",
} satisfies Record<JobTimelineItem["source"], string>;

export function timelineSourceLabel(source: JobTimelineItem["source"]): string {
  return SOURCE_LABEL[source];
}

function humanize(camel: string): string {
  const spaced = camel.replace(/([A-Z])/g, " $1").toLowerCase();
  return spaced.charAt(0).toUpperCase() + spaced.slice(1);
}

/** One line naming what a timeline item is. */
export function timelineItemLabel(item: JobTimelineItem): string {
  switch (item.source) {
    case "job": return humanize(item.event.kind satisfies JobEventKind);
    case "hostOperation": return `${humanize(item.hostOperation.kind)}: ${humanize(item.hostOperation.state)}`;
    case "reservation": return `Reservation ${item.reservation.state}`;
    case "amountEvent": return item.isCorrection ? "Correction" : humanize(item.amountEvent.kind);
    case "requirement": return `${humanize(item.requirement.kind)} ${item.requirement.status}`;
    case "attention": return conditionKindLabel(item.event.condition);
    case "incident": return incidentEntryKindLabel(item.entry.kind);
    case "snapshot":
      return item.snapshot.prunedAt !== null
        ? `Snapshot (${item.snapshot.pruneReason ? pruneReasonText(item.snapshot.pruneReason) : "image removed"})`
        : "Snapshot";
  }
}

/** The material section's items (reservation, deduction, correction). */
export function isMaterialItem(item: JobTimelineItem): boolean {
  return item.source === "reservation" || item.source === "amountEvent";
}
