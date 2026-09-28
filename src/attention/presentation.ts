/** Pure presentation: every label here names a Rust-provided `ConditionKind`,
 *  `AttentionSeverity`, `AttentionResolution`, `IncidentEntryKind`,
 *  `CameraErrorKind`, `CameraHealthState`, or `PruneReason` (spec "Frontend
 *  architecture"). Nothing here calls a command or touches a store --
 *  components pass in the data they already hold, the same way
 *  `queue/presentation.ts` does. */
import type {
  AttentionEvent,
  AttentionResolution,
  AttentionSeverity,
  CameraErrorKind,
  CameraHealthState,
  ConditionKind,
  IncidentEntryKind,
  PruneReason,
} from "./types";

const CONDITION_KIND_LABEL = {
  "printer.offline": "Printer offline",
  "printer.connectionError": "Connection error",
  "printer.hostFailed": "Printer-reported failure",
  "job.startConfirmation": "Start confirmation needed",
  "job.failed": "Job failed",
  "job.hostCancelled": "Job cancelled on the printer",
  "requirement.materialReconciliation": "Material reconciliation needed",
  "requirement.jobOutcomeUnknown": "Job outcome unknown",
  "spool.low": "Spool low",
  "job.completed": "Job completed",
} satisfies Record<ConditionKind, string>;

export function conditionKindLabel(kind: ConditionKind): string {
  return CONDITION_KIND_LABEL[kind];
}

/** Spec "Frontend architecture": "the words Fatal, Warning, Info" --
 *  always shown with `SeverityMarker`'s shape and color, never alone
 *  (global constraint 6). */
const ATTENTION_SEVERITY_LABEL = {
  fatal: "Fatal",
  warning: "Warning",
  info: "Info",
} satisfies Record<AttentionSeverity, string>;

export function attentionSeverityLabel(severity: AttentionSeverity): string {
  return ATTENTION_SEVERITY_LABEL[severity];
}

const ATTENTION_RESOLUTION_LABEL = {
  conditionCleared: "Cleared automatically",
  actionCompleted: "Completed automatically",
  operatorResolved: "Resolved by the operator",
  sourceRemoved: "Source removed",
} satisfies Record<AttentionResolution, string>;

export function attentionResolutionLabel(resolution: AttentionResolution): string {
  return ATTENTION_RESOLUTION_LABEL[resolution];
}

const INCIDENT_ENTRY_KIND_LABEL = {
  opened: "Opened",
  eventLinked: "Event linked",
  reopened: "Reopened",
  eventAcknowledged: "Event acknowledged",
  eventResolved: "Event resolved",
  evidenceCaptured: "Evidence captured",
  evidenceSkipped: "Evidence skipped",
  evidencePruned: "Evidence pruned",
  evidencePinned: "Evidence pinned",
  evidenceUnpinned: "Evidence unpinned",
  noteAdded: "Note added",
  closed: "Closed",
} satisfies Record<IncidentEntryKind, string>;

export function incidentEntryKindLabel(kind: IncidentEntryKind): string {
  return INCIDENT_ENTRY_KIND_LABEL[kind];
}

const CAMERA_ERROR_KIND_LABEL = {
  unreachable: "The camera could not be reached.",
  timeout: "The camera did not answer in time.",
  httpStatus: "The camera returned an error.",
  tooLarge: "The image was too large.",
  notAnImage: "The camera did not return an image.",
  noSuchWebcam: "That webcam was not found.",
  noSnapshotUrl: "No snapshot URL is configured.",
  hostMismatch: "The printer's webcam points at a different host.",
  unsupportedAdapter: "Not supported by this Connection.",
  webcamListFailed: "The webcam list could not be read.",
} satisfies Record<CameraErrorKind, string>;

export function cameraErrorKindLabel(kind: CameraErrorKind): string {
  return CAMERA_ERROR_KIND_LABEL[kind];
}

/** Spec "Accessibility and adaptation": "Live", "Camera not answering
 *  (timeout)", "No camera", "Not supported by this Connection" --
 *  `failing`'s own reason (a `CameraErrorKind`) is appended by the caller,
 *  which already has it (`CameraHealth.lastFailureKind`). */
const CAMERA_HEALTH_STATE_LABEL = {
  notConfigured: "No camera",
  unsupported: "Not supported by this Connection",
  unknown: "Unknown",
  ok: "Live",
  failing: "Camera not answering",
} satisfies Record<CameraHealthState, string>;

export function cameraHealthStateLabel(state: CameraHealthState): string {
  return CAMERA_HEALTH_STATE_LABEL[state];
}

const PRUNE_REASON_LABEL = {
  age: "past retention",
  diskCap: "disk cap reached",
  missingFile: "file missing",
} satisfies Record<PruneReason, string>;

export function pruneReasonLabel(reason: PruneReason): string {
  return PRUNE_REASON_LABEL[reason];
}

// --- Attention center filters (Task 13) -------------------------------------

export type AttentionFilter = "actionable" | "unread" | "allOpen" | "resolved";

const ATTENTION_FILTER_LABEL = {
  actionable: "Actionable",
  unread: "Unread",
  allOpen: "All open",
  resolved: "Resolved",
} satisfies Record<AttentionFilter, string>;

export function attentionFilterLabel(filter: AttentionFilter): string {
  return ATTENTION_FILTER_LABEL[filter];
}

/** The default filter (spec "Frontend architecture": "Actionable ...; the
 *  default"). */
export const DEFAULT_ATTENTION_FILTER: AttentionFilter = "actionable";

/** Whether `event` belongs in `filter`'s list. Actionable and Unread are
 *  both scoped to open Events (they're read alongside "All open" as one
 *  segmented control); "Resolved" is its own, separate list. */
export function matchesAttentionFilter(filter: AttentionFilter, event: AttentionEvent): boolean {
  switch (filter) {
    case "actionable":
      return event.resolvedAt === null && event.requiresAction;
    case "unread":
      return event.resolvedAt === null && event.readAt === null;
    case "allOpen":
      return event.resolvedAt === null;
    case "resolved":
      return event.resolvedAt !== null;
  }
}

export type AttentionSeverityFilter = "all" | AttentionSeverity;

export const ATTENTION_SEVERITY_FILTERS: AttentionSeverityFilter[] = ["all", "fatal", "warning", "info"];

export function attentionSeverityFilterLabel(filter: AttentionSeverityFilter): string {
  return filter === "all" ? "All severities" : attentionSeverityLabel(filter);
}

export function matchesSeverityFilter(filter: AttentionSeverityFilter, event: AttentionEvent): boolean {
  return filter === "all" || event.severity === filter;
}
