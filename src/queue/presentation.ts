/** Pure presentation: every label here names a Rust-provided `JobState`,
 *  `CancelReason`, `Settlement`, `BlockerCode`, `RequirementKind`,
 *  `JobEventKind`, or new `RecoveryCode` (spec "Frontend architecture").
 *  Nothing here calls a command or touches a store -- components pass in
 *  the data they already hold, the same way `host-ops/presentation.ts`
 *  does. */
import type { QueueView } from "./views";
import type {
  Blocker,
  BlockerCode,
  CancelReason,
  CloseReason,
  DispatchPolicy,
  DispatchPreference,
  EstimateSource,
  QueueEntry,
  JobEventKind,
  JobState,
  RecoveryCode,
  RequirementKind,
  RequirementStatus,
  Settlement,
} from "./types";

const JOB_STATE_LABEL = {
  assigned: "Assigned",
  staging: "Staging",
  awaitingStart: "Awaiting start",
  starting: "Starting",
  printing: "Printing",
  paused: "Paused",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
  outcomeUnknown: "Outcome unknown",
} satisfies Record<JobState, string>;

/** D3's `JobState` label. `outcomeUnknown` reads "Outcome unknown" (spec
 *  "Frontend architecture"). */
export function jobStateLabel(state: JobState): string {
  return JOB_STATE_LABEL[state];
}

const CANCEL_REASON_LABEL = {
  releasedBeforeStart: "Released before start",
  cancelledBeforeStart: "Cancelled before start",
  cancelledByOperator: "Cancelled",
  hostCancelled: "Cancelled on the printer",
  operatorDeclared: "Declared cancelled",
} satisfies Record<CancelReason, string>;

export function cancelReasonLabel(reason: CancelReason): string {
  return CANCEL_REASON_LABEL[reason];
}

const SETTLEMENT_LABEL = {
  open: "Open",
  notRequired: "Not required",
  pending: "Needs reconciliation",
  deferred: "Deferred",
  settled: "Settled",
} satisfies Record<Settlement, string>;

export function settlementLabel(settlement: Settlement): string {
  return SETTLEMENT_LABEL[settlement];
}

/** D5's "Recovery per blocker" table (a short name, for grouping headers
 *  and legends -- `Blocker.message`, which Rust supplies per instance, is
 *  the body text everywhere else; this is not a substitute for it). */
const BLOCKER_CODE_LABEL = {
  PRINTER_ARCHIVED: "Archived",
  SETUP_INCOMPLETE: "Setup incomplete",
  CONNECTION_ERROR: "Connection error",
  PRINTER_OFFLINE: "Offline",
  JOB_ACTIVE: "Job active",
  HOST_OPERATION_PENDING: "Printer operation pending",
  PRINTER_BUSY_EXTERNAL: "Printing outside farm3d",
  PRINTER_NOT_IDLE: "Not idle",
  PINNED_TO_OTHER_PRINTER: "Pinned to another Printer",
  PROFILE_MISMATCH: "Profile mismatch",
  NEEDS_MANUAL_PRINTER: "Needs manual assignment",
  CAPABILITY_UNSUPPORTED: "Capability unsupported",
  ADAPTER_NOT_PROVEN: "Adapter not proven",
  NO_COMPATIBLE_SPOOL: "No compatible Spool",
  INSUFFICIENT_MATERIAL: "Insufficient material",
  SPOOL_NOT_LOADED: "Awaiting material",
  PRINTER_NOT_READY: "Printer not ready",
} satisfies Record<BlockerCode, string>;

export function blockerCodeLabel(code: BlockerCode): string {
  return BLOCKER_CODE_LABEL[code];
}

/** The short chip text for one of a Job's `startBlockers` (D3: "Awaiting
 *  material" is the UI's own label for a `SPOOL_NOT_LOADED` start blocker;
 *  every other code shows Rust's own `message` verbatim -- this never
 *  re-derives it). */
export function startBlockerLabel(blocker: Blocker): string {
  return blocker.code === "SPOOL_NOT_LOADED" ? "Awaiting material" : blocker.message;
}

const REQUIREMENT_KIND_LABEL = {
  materialReconciliation: "Material reconciliation",
  jobOutcomeUnknown: "Outcome unknown",
} satisfies Record<RequirementKind, string>;

export function requirementKindLabel(kind: RequirementKind): string {
  return REQUIREMENT_KIND_LABEL[kind];
}

const REQUIREMENT_STATUS_LABEL = {
  pending: "Pending",
  deferred: "Deferred",
  resolved: "Resolved",
} satisfies Record<RequirementStatus, string>;

export function requirementStatusLabel(status: RequirementStatus): string {
  return REQUIREMENT_STATUS_LABEL[status];
}

const JOB_EVENT_KIND_LABEL = {
  assigned: "Assigned",
  stageHandedOff: "Staging sent to the printer",
  stageSucceeded: "Staged",
  stageFailed: "Staging failed",
  startHandedOff: "Start sent to the printer",
  startSucceeded: "Started",
  startFailed: "Start failed",
  startAbandoned: "Start check abandoned",
  hostJobPinned: "Printer job matched",
  pauseHandedOff: "Pause sent to the printer",
  resumeHandedOff: "Resume sent to the printer",
  cancelHandedOff: "Cancel sent to the printer",
  controlFailed: "Printer control failed",
  paused: "Paused",
  resumed: "Resumed",
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
  outcomeUnknown: "Outcome became unknown",
  declaredCompleted: "Declared completed",
  declaredFailed: "Declared failed",
  declaredCancelled: "Declared cancelled",
  released: "Released",
  cancelledBeforeStart: "Cancelled before start",
  materialSettled: "Material settled",
  materialDeferred: "Material settlement deferred",
  materialCorrected: "Material correction recorded",
} satisfies Record<JobEventKind, string>;

export function jobEventKindLabel(kind: JobEventKind): string {
  return JOB_EVENT_KIND_LABEL[kind];
}

/** The `RecoveryCode` values this spec adds (D5/D9's "`RecoveryCode`
 *  gains..."); the pre-existing ones (`RELOAD`, `OPEN_PRINTER_JOB`, ...)
 *  are P6's own and already rendered by `HostOperationAlert`. `OPEN_JOB`'s
 *  label is used by a later task's button (Task 14/15). */
export type NewRecoveryCode = Extract<
  RecoveryCode,
  "OPEN_JOB" | "OPEN_PRINTER_SETUP" | "UNARCHIVE_PRINTER" | "LOAD_SPOOL" | "ASSIGN_MANUALLY" | "SETTLE_MATERIAL"
>;

const NEW_RECOVERY_CODE_LABEL = {
  OPEN_JOB: "Open the Job",
  OPEN_PRINTER_SETUP: "Open Printer setup",
  UNARCHIVE_PRINTER: "Unarchive Printer",
  LOAD_SPOOL: "Open Spools",
  ASSIGN_MANUALLY: "Assign manually",
  SETTLE_MATERIAL: "Settle material",
} satisfies Record<NewRecoveryCode, string>;

export function recoveryCodeLabel(code: NewRecoveryCode): string {
  return NEW_RECOVERY_CODE_LABEL[code];
}

// --- Queue screen (Task 14) -------------------------------------------------

const DISPATCH_POLICY_LABEL = {
  manual: "Manual",
  recommended: "Recommended",
  automatic: "Automatic",
} satisfies Record<DispatchPolicy, string>;

export function dispatchPolicyLabel(policy: DispatchPolicy): string {
  return DISPATCH_POLICY_LABEL[policy];
}

const DISPATCH_PREFERENCE_LABEL = {
  loadedFirst: "Loaded Spool first",
  leastRecentlyUsed: "Least recently used",
} satisfies Record<DispatchPreference, string>;

export function dispatchPreferenceLabel(preference: DispatchPreference): string {
  return DISPATCH_PREFERENCE_LABEL[preference];
}

const CLOSE_REASON_LABEL = {
  completed: "Completed",
  failed: "Failed",
  cancelled: "Cancelled",
  released: "Released",
  removed: "Removed",
} satisfies Record<CloseReason, string>;

export function closeReasonLabel(reason: CloseReason): string {
  return CLOSE_REASON_LABEL[reason];
}

/** Where a Queue Entry's fixed material estimate came from, as the tail of
 *  "38.6 g (slice estimate)". */
const ESTIMATE_SOURCE_LABEL = {
  sliceEstimate: "slice estimate",
  fileClaimConfirmed: "file's claim, confirmed",
  operatorEntered: "entered by hand",
} satisfies Record<EstimateSource, string>;

export function estimateSourceLabel(source: EstimateSource): string {
  return ESTIMATE_SOURCE_LABEL[source];
}

const QUEUE_VIEW_LABEL = {
  awaitingOperator: "Awaiting operator",
  ready: "Ready",
  assigned: "Assigned",
  blocked: "Blocked",
  printing: "Printing now",
  history: "History",
} satisfies Record<QueueView, string>;

export function queueViewLabel(view: QueueView): string {
  return QUEUE_VIEW_LABEL[view];
}

/** D1's lineage label: `copyIndex` of `copyCount`, both from Rust. */
export function copyLabel(entry: Pick<QueueEntry, "copyIndex" | "copyCount">): string {
  return `Copy ${entry.copyIndex} of ${entry.copyCount}`;
}
