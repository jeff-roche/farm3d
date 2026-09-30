/** Pure presentation for Job History and the Job timeline: labels for every
 *  `JobHistoryState`, `JobTimelineItem` source, and `PruneReason`. Rust
 *  supplies the values; nothing here decides anything. */
import {
  attentionResolutionLabel,
  attentionSeverityLabel,
  conditionKindLabel,
  evidenceSkipReasonLabel,
  incidentEntryKindLabel,
  snapshotTriggerLabel,
} from "../attention/presentation";
import { hostOperationLabel } from "../host-ops/presentation";
import { jobStateLabel, requirementKindLabel, requirementStatusLabel } from "../queue/presentation";
import { formatGrams } from "../spools/weight";
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
    case "snapshot": return item.snapshot.prunedAt !== null ? "Snapshot (evidence pruned)" : "Snapshot";
  }
}

/** The material section's items (reservation, deduction, correction). */
export function isMaterialItem(item: JobTimelineItem): boolean {
  return item.source === "reservation" || item.source === "amountEvent";
}

/** One line of text for what an item says, beside its label and time:
 *  never empty, so no item is only a marker. Pruned evidence is text. */
export function timelineItemDetail(item: JobTimelineItem): string {
  switch (item.source) {
    case "job": {
      const { fromState, toState } = item.event;
      return fromState ? `${jobStateLabel(fromState)} to ${jobStateLabel(toState)}` : jobStateLabel(toState);
    }
    case "hostOperation": {
      const { hostOperation } = item;
      const attempts = hostOperation.attempts === 1 ? "1 attempt" : `${hostOperation.attempts} attempts`;
      return `${hostOperationLabel(hostOperation).text}: ${hostOperation.hostPath} (${attempts})`;
    }
    case "reservation":
      return `${formatGrams(item.reservation.amountMg, 1)} held on the Spool`;
    case "amountEvent": {
      const { beforeMg, afterMg, note } = item.amountEvent;
      const change = beforeMg === undefined
        ? `Spool now ${formatGrams(afterMg, 1)}`
        : `Spool ${formatGrams(beforeMg, 1)} to ${formatGrams(afterMg, 1)}`;
      return note ? `${change} (${note})` : change;
    }
    case "requirement":
      return `${requirementKindLabel(item.requirement.kind)}: ${requirementStatusLabel(item.requirement.status)}`;
    case "attention":
      return `${attentionSeverityLabel(item.event.severity)}: ${item.event.summary || conditionKindLabel(item.event.condition)}`;
    case "incident": {
      const detail = item.entry.detail;
      switch (detail.kind) {
        case "noteAdded": return detail.text;
        case "evidencePruned": return `Evidence pruned (${pruneReasonText(detail.reason)})`;
        case "evidenceSkipped": return `No snapshot taken: ${evidenceSkipReasonLabel(detail.reason)}`;
        case "evidenceCaptured": return `${snapshotTriggerLabel(detail.trigger)} snapshot captured`;
        case "eventResolved": return `Resolved: ${attentionResolutionLabel(detail.resolution)}`;
        default: return `Incident entry ${item.entry.sequence}`;
      }
    }
    case "snapshot": {
      const { snapshot } = item;
      if (snapshot.prunedAt !== null) {
        return `Evidence pruned (${snapshot.pruneReason ? pruneReasonText(snapshot.pruneReason) : "image removed"})`;
      }
      return `${snapshotTriggerLabel(snapshot.trigger)} snapshot${snapshot.pinnedAt ? ", pinned" : ""}`;
    }
  }
}
