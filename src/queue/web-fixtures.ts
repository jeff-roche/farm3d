import {
  buildWebHostOpsFixture,
  WEB_HOST_OPS_PRINTER_FAILED,
  WEB_HOST_OPS_PRINTER_FINISHED,
  WEB_HOST_OPS_PRINTER_OCTOPRINT,
  WEB_HOST_OPS_PRINTER_READY_MULTI,
  WEB_HOST_OPS_PRINTER_READY_SINGLE,
  WEB_HOST_OPS_PRINTER_UNCERTAIN_UPLOAD,
  WEB_HOST_OPS_STAGED_OPERATION,
  WEB_HOST_OPS_UNCERTAIN_OPERATION,
  WEB_HOST_OPS_UPLOAD_FAILED_PRINT,
  WEB_HOST_OPS_UPLOAD_READY_MULTI,
} from "../host-ops/web-fixtures";
import type { HostOperation } from "../host-ops/types";
import {
  WEB_SLICING_REVISION_EXTERNAL,
  WEB_SLICING_REVISION_FARM3D,
  WEB_SLICING_REVISION_FARM3D_OLDER,
} from "../slicing/web-fixtures";
import type {
  Candidate,
  EligibilitySummary,
  Job,
  JobEvent,
  JobHistory,
  NextAutomaticAction,
  PrinterEligibility,
  PrinterSnapshot,
  QueueEntry,
  QueueEntryEligibility,
  ReconciliationRequirement,
  Reservation,
  SpoolOption,
} from "./types";

/** `just web`'s Queue/Job seed data (spec "Frontend architecture"): there
 *  is no Rust backend in web mode, so this stands in for `list_queue`,
 *  written as literals the way Rust would send them (never derived from
 *  other fields here -- this module's job is to look like a snapshot,
 *  not to compute one, mirroring `host-ops/web-fixtures.ts`). It is not
 *  persistence: web-mode writes are refused by the store
 *  (`needsDesktopError`) rather than faked.
 *
 *  Six open entries: three linked copies (an `Add to Queue` of quantity
 *  3), one blocked entry, one `assigned` entry whose Job is `printing` (on
 *  the host-ops fixture's own already-printing Printer), one whose Job
 *  is `awaitingStart` with no start blockers (staged on the Finished
 *  Printer, its Spool loaded there), so the Start confirmation shows, and
 *  one whose Job is `outcomeUnknown` (on the Uncertain-upload Printer),
 *  with an open `jobOutcomeUnknown` Requirement.
 *  Three closed entries: a `failed` Job whose material settlement was
 *  deferred, with an open `materialReconciliation` Requirement; a
 *  `hostCancelled` Job (cancelled from the printer's own screen); and a
 *  `completed` Job. `attention/web-fixtures.ts` sources every one of
 *  these Job/Requirement ids for its own `job.*`/`requirement.*` Events,
 *  rather than inventing a disjoint id space (spec Task 12 fix round 1). */
export interface WebQueueFixture {
  entries: QueueEntry[];
  jobs: Job[];
  requirements: ReconciliationRequirement[];
  eligibility: EligibilitySummary[];
  nextAutomaticAction: NextAutomaticAction;
}

export const WEB_QUEUE_LINEAGE_BRACKET = "qln-web-bracket";
export const WEB_QUEUE_ENTRY_BRACKET_IDS = ["qen-web-bracket-1", "qen-web-bracket-2", "qen-web-bracket-3"] as const;
export const WEB_QUEUE_ENTRY_BLOCKED = "qen-web-blocked";
export const WEB_QUEUE_ENTRY_PRINTING = "qen-web-printing";
export const WEB_QUEUE_JOB_PRINTING = "job-web-printing";
export const WEB_QUEUE_ENTRY_AWAITING_START = "qen-web-awaiting-start";
/** Matches `host-ops/web-fixtures.ts`'s staged upload's `jobId`. */
export const WEB_QUEUE_JOB_AWAITING_START = "job-web-awaiting-start";
/** Matches `spools/web-fixtures.ts`'s Spool #10, loaded on the Finished
 *  Printer (a literal, as `WEB_QUEUE_DEFERRED_SPOOL_ID` is). */
export const WEB_QUEUE_AWAITING_START_SPOOL_ID = "spl-web-10";
export const WEB_QUEUE_ENTRY_HISTORY_DEFERRED = "qen-web-history-deferred";
export const WEB_QUEUE_JOB_DEFERRED = "job-web-deferred";
export const WEB_QUEUE_REQUIREMENT_DEFERRED = "rqr-web-deferred";
/** The deferred Requirement's Spool: `spools/web-fixtures.ts` gives it the
 *  `reconciliation` facet and an over-reserved `availableMg` (the brief's
 *  "one Spool with `reconciliation = true`"), so the two fixtures tell one
 *  consistent story. */
export const WEB_QUEUE_DEFERRED_SPOOL_ID = "spl-web-9";
export const WEB_QUEUE_ENTRY_HOST_CANCELLED = "qen-web-host-cancelled";
/** A Job cancelled from the printer's own screen, not by farm3d (spec P8
 *  `job.hostCancelled`; `attention/web-fixtures.ts` sources this id
 *  rather than inventing one). */
export const WEB_QUEUE_JOB_HOST_CANCELLED = "job-web-host-cancelled";
export const WEB_QUEUE_ENTRY_OUTCOME_UNKNOWN = "qen-web-outcome-unknown";
/** A Job whose outcome the tracker couldn't determine (spec P8
 *  `requirement.jobOutcomeUnknown`). */
export const WEB_QUEUE_JOB_OUTCOME_UNKNOWN = "job-web-outcome-unknown";
export const WEB_QUEUE_REQUIREMENT_OUTCOME_UNKNOWN = "rqr-web-outcome-unknown";
export const WEB_QUEUE_ENTRY_COMPLETED = "qen-web-completed";
/** A normally completed Job (spec P8 `job.completed`). */
export const WEB_QUEUE_JOB_COMPLETED = "job-web-completed";

const CENTAURI_CARBON_PROFILE = {
  bedShape: { kind: "rectangular" as const, widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
  printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
  nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
  hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
  suggestedHostType: "moonraker",
};

/** Exported so `attention/web-fixtures.ts` can build a Printer snapshot
 *  that agrees with this fixture's own, rather than hand-rolling a second,
 *  possibly-drifting copy (spec Task 12 fix round 1: cross-link, don't
 *  duplicate). */
export function centauriCarbonSnapshot(name: string, location: string): PrinterSnapshot {
  return {
    name,
    location,
    catalogRef: { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" },
    adapterKind: "moonraker",
    profile: CENTAURI_CARBON_PROFILE,
  };
}

export function failedPrinterSnapshot(): PrinterSnapshot {
  return centauriCarbonSnapshot("Failed — Bay 8", "Bay 8");
}

function readyMultiPrinterSnapshot(): PrinterSnapshot {
  return {
    name: "Four-tool — Bay 5",
    location: "Bay 5",
    catalogRef: { vendor: "Snapmaker", model: "Snapmaker U1", variant: "Snapmaker U1 0.4 nozzle", modelId: "Snapmaker-U1", printerVariant: "0.4" },
    adapterKind: "moonraker",
    profile: {
      bedShape: { kind: "rectangular", widthMm: 230, depthMm: 230, originXMm: 0, originYMm: 0 },
      printableHeightMm: 230, bedExcludeAreas: [], defaultBedType: "1",
      nozzleDiameterMm: [0.4, 0.4, 0.4, 0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
      hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
      suggestedHostType: "moonraker",
    },
  };
}

function bracketDisplay(printSeconds: number) {
  return {
    modelId: "mdl-web-enclosure", modelName: "Enclosure lid", plateLabel: "Lid",
    targetLabel: "Elegoo Centauri Carbon 0.4 nozzle",
    materialFamily: "PLA" as const, materialOther: null, printSeconds,
  };
}

function linkedCopies(): QueueEntry[] {
  return WEB_QUEUE_ENTRY_BRACKET_IDS.map((id, index) => ({
    id,
    revision: 1,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    lineageId: WEB_QUEUE_LINEAGE_BRACKET,
    copyIndex: index + 1,
    copyCount: WEB_QUEUE_ENTRY_BRACKET_IDS.length,
    originEntryId: null,
    originKind: null,
    state: "queued",
    closeReason: null,
    position: index + 1,
    policy: "recommended",
    preference: "loadedFirst",
    estimate: { amountMg: 38_600, source: "sliceEstimate" },
    manualPrinterId: null,
    jobId: null,
    requiresManualPrinterSelection: false,
    allowedActions: ["assign", "update", "move", "remove"],
    display: bracketDisplay(5_412),
    createdAt: "2026-09-25T09:00:00Z",
    updatedAt: "2026-09-25T09:00:00Z",
    closedAt: null,
  }));
}

function blockedEntry(): QueueEntry {
  return {
    id: WEB_QUEUE_ENTRY_BLOCKED,
    revision: 1,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    lineageId: "qln-web-blocked",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "queued",
    closeReason: null,
    position: 4,
    policy: "manual",
    preference: "loadedFirst",
    estimate: { amountMg: 38_600, source: "sliceEstimate" },
    manualPrinterId: WEB_HOST_OPS_PRINTER_OCTOPRINT,
    jobId: null,
    requiresManualPrinterSelection: false,
    allowedActions: ["assign", "update", "move", "remove"],
    display: bracketDisplay(5_412),
    createdAt: "2026-09-25T09:05:00Z",
    updatedAt: "2026-09-25T09:05:00Z",
    closedAt: null,
  };
}

function printingEntry(): QueueEntry {
  return {
    id: WEB_QUEUE_ENTRY_PRINTING,
    revision: 2,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D_OLDER,
    lineageId: "qln-web-printing",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "assigned",
    closeReason: null,
    position: 5,
    policy: "automatic",
    preference: "loadedFirst",
    estimate: { amountMg: 41_200, source: "sliceEstimate" },
    manualPrinterId: null,
    jobId: WEB_QUEUE_JOB_PRINTING,
    requiresManualPrinterSelection: false,
    allowedActions: ["move"],
    display: bracketDisplay(6_180),
    createdAt: "2026-09-25T08:00:00Z",
    updatedAt: "2026-09-25T08:05:00Z",
    closedAt: null,
  };
}

function printingJob(): Job {
  return {
    id: WEB_QUEUE_JOB_PRINTING,
    revision: 3,
    queueEntryId: WEB_QUEUE_ENTRY_PRINTING,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D_OLDER,
    printerId: WEB_HOST_OPS_PRINTER_READY_MULTI,
    printerSnapshot: readyMultiPrinterSnapshot(),
    spoolId: "spl-web-6",
    reservationId: "rsv-web-printing",
    estimateMg: 41_200,
    state: "printing",
    cancelReason: null,
    settlement: "open",
    settlementMethod: null,
    settlementPreview: null,
    corrected: false,
    assignedBy: "automatic",
    startConfirmation: "unattended",
    // D7: `upload_host_operation_id` is set on `StageSucceeded` and never
    // cleared. `active_host_operation_id` clears once its op (the start
    // that put this Job in `printing`) resolves -- it's null again by the
    // time the Job is mid-print.
    uploadHostOperationId: WEB_HOST_OPS_UPLOAD_READY_MULTI,
    activeHostOperationId: null,
    hostPath: "farm3d/bracket-set.gcode",
    maxProgressPct: 42,
    hostUnreachableSince: null,
    lastFailure: null,
    startBlockers: [],
    allowedActions: ["pause", "cancel"],
    createdAt: "2026-09-25T08:00:00Z",
    updatedAt: "2026-09-25T08:05:00Z",
    startedAt: "2026-09-25T08:05:00Z",
    endedAt: null,
  };
}

function finishedPrinterSnapshot(): PrinterSnapshot {
  return { ...failedPrinterSnapshot(), name: "Finished — Bay 7", location: "Bay 7" };
}

function awaitingStartEntry(): QueueEntry {
  return {
    id: WEB_QUEUE_ENTRY_AWAITING_START,
    revision: 2,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    lineageId: "qln-web-awaiting-start",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "assigned",
    closeReason: null,
    position: 6,
    policy: "manual",
    preference: "loadedFirst",
    estimate: { amountMg: 38_600, source: "sliceEstimate" },
    manualPrinterId: null,
    jobId: WEB_QUEUE_JOB_AWAITING_START,
    requiresManualPrinterSelection: false,
    // D2: an `assigned` entry can only move.
    allowedActions: ["move"],
    display: bracketDisplay(5_412),
    createdAt: "2026-09-23T09:55:00Z",
    updatedAt: "2026-09-23T10:00:00Z",
    closedAt: null,
  };
}

/** D7: staged (`StageSucceeded` set `uploadHostOperationId` and
 *  `hostPath`), its Spool loaded on its Printer, the Printer `finished`
 *  with fresh telemetry, and nothing unresolved there -- so Rust's
 *  `startBlockers` is empty and D3 lists `stage` (again), `start`,
 *  `cancel`, and `release`. `activeHostOperationId` cleared when the
 *  upload resolved. */
function awaitingStartJob(): Job {
  return {
    id: WEB_QUEUE_JOB_AWAITING_START,
    revision: 3,
    queueEntryId: WEB_QUEUE_ENTRY_AWAITING_START,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    printerId: WEB_HOST_OPS_PRINTER_FINISHED,
    printerSnapshot: finishedPrinterSnapshot(),
    spoolId: WEB_QUEUE_AWAITING_START_SPOOL_ID,
    reservationId: "rsv-web-awaiting-start",
    estimateMg: 38_600,
    state: "awaitingStart",
    cancelReason: null,
    settlement: "open",
    settlementMethod: null,
    settlementPreview: null,
    corrected: false,
    assignedBy: "operator",
    startConfirmation: null,
    uploadHostOperationId: WEB_HOST_OPS_STAGED_OPERATION,
    activeHostOperationId: null,
    hostPath: "farm3d/enclosure-lid.gcode",
    maxProgressPct: 0,
    hostUnreachableSince: null,
    lastFailure: null,
    startBlockers: [],
    allowedActions: ["stage", "start", "cancel", "release"],
    createdAt: "2026-09-23T10:00:00Z",
    updatedAt: "2026-09-23T10:00:05Z",
    startedAt: null,
    endedAt: null,
  };
}

function historyDeferredEntry(): QueueEntry {
  return {
    id: WEB_QUEUE_ENTRY_HISTORY_DEFERRED,
    revision: 4,
    // The External revision's `materialFamily` fact is absent, so
    // `add_to_queue` requires an explicit `materialEstimate` and only
    // allows it under Manual policy (spec "Commands"): `operatorEntered`
    // is the one estimate source this fixture can legally pair with it.
    sliceRevisionId: WEB_SLICING_REVISION_EXTERNAL,
    lineageId: "qln-web-history-deferred",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "closed",
    closeReason: "failed",
    position: null,
    policy: "manual",
    preference: "loadedFirst",
    estimate: { amountMg: 25_000, source: "operatorEntered" },
    manualPrinterId: WEB_HOST_OPS_PRINTER_FAILED,
    jobId: WEB_QUEUE_JOB_DEFERRED,
    requiresManualPrinterSelection: true,
    allowedActions: [],
    display: {
      modelId: "mdl-web-cube-gcode", modelName: "Calibration cube (sliced)", plateLabel: null,
      targetLabel: "Elegoo Centauri Carbon 0.4 nozzle", materialFamily: null, materialOther: null, printSeconds: 1_421,
    },
    createdAt: "2026-09-24T09:00:00Z",
    updatedAt: "2026-09-24T09:10:00Z",
    closedAt: "2026-09-24T09:10:00Z",
  };
}

function deferredJob(): Job {
  return {
    id: WEB_QUEUE_JOB_DEFERRED,
    revision: 4,
    queueEntryId: WEB_QUEUE_ENTRY_HISTORY_DEFERRED,
    sliceRevisionId: WEB_SLICING_REVISION_EXTERNAL,
    printerId: WEB_HOST_OPS_PRINTER_FAILED,
    printerSnapshot: failedPrinterSnapshot(),
    spoolId: WEB_QUEUE_DEFERRED_SPOOL_ID,
    reservationId: "rsv-web-deferred",
    estimateMg: 25_000,
    state: "failed",
    cancelReason: null,
    settlement: "deferred",
    settlementMethod: null,
    // Spec "Material settlement": ceil(25 000 mg × 8 / 100).
    settlementPreview: { estimatedUseMg: 2_000 },
    corrected: false,
    assignedBy: "operator",
    startConfirmation: "bedClear",
    // One consistent story (D7): the print started fine (this upload
    // succeeded, then the start op it's linked to also succeeded), ran to
    // 8%, and the *tracker* later found it `failed` in host history.
    // `apply_host_outcome` only sets `lastFailure` for a `StageFailed`/
    // `StartFailed` Host Operation, never for a tracker-discovered
    // failure, and `active_host_operation_id` clears once its (terminal)
    // op resolves -- so both are null/absent here, same as any other Job
    // that made it into `printing` before failing.
    uploadHostOperationId: WEB_HOST_OPS_UPLOAD_FAILED_PRINT,
    activeHostOperationId: null,
    hostPath: "farm3d/enclosure-lid.gcode",
    maxProgressPct: 8,
    hostUnreachableSince: null,
    lastFailure: null,
    startBlockers: [],
    allowedActions: ["retry", "settleMaterial"],
    createdAt: "2026-09-24T09:00:00Z",
    updatedAt: "2026-09-24T09:10:00Z",
    startedAt: "2026-09-24T09:01:00Z",
    endedAt: "2026-09-24T09:09:00Z",
  };
}

function deferredRequirement(): ReconciliationRequirement {
  return {
    id: WEB_QUEUE_REQUIREMENT_DEFERRED,
    jobId: WEB_QUEUE_JOB_DEFERRED,
    kind: "materialReconciliation",
    status: "deferred",
    spoolId: WEB_QUEUE_DEFERRED_SPOOL_ID,
    reservationId: "rsv-web-deferred",
    openedAt: "2026-09-24T09:10:00Z",
    deferredAt: "2026-09-24T09:12:00Z",
    resolvedAt: null,
    resolution: null,
  };
}

/** A Job cancelled from the printer's own screen (spec P8
 *  `job.hostCancelled`): closed, no Requirement, no material to settle. */
function hostCancelledEntry(): QueueEntry {
  return {
    id: WEB_QUEUE_ENTRY_HOST_CANCELLED,
    revision: 3,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    lineageId: "qln-web-host-cancelled",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "closed",
    closeReason: "cancelled",
    position: null,
    policy: "manual",
    preference: "loadedFirst",
    estimate: { amountMg: 18_000, source: "sliceEstimate" },
    manualPrinterId: WEB_HOST_OPS_PRINTER_READY_SINGLE,
    jobId: WEB_QUEUE_JOB_HOST_CANCELLED,
    requiresManualPrinterSelection: false,
    allowedActions: [],
    display: {
      modelId: "mdl-web-cube-gcode", modelName: "Calibration cube (sliced)", plateLabel: null,
      targetLabel: "Elegoo Centauri Carbon 0.4 nozzle", materialFamily: null, materialOther: null, printSeconds: 900,
    },
    createdAt: "2026-09-22T08:00:00Z",
    updatedAt: "2026-09-22T08:20:00Z",
    closedAt: "2026-09-22T08:20:00Z",
  };
}

function hostCancelledJob(): Job {
  return {
    id: WEB_QUEUE_JOB_HOST_CANCELLED,
    revision: 3,
    queueEntryId: WEB_QUEUE_ENTRY_HOST_CANCELLED,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    printerId: WEB_HOST_OPS_PRINTER_READY_SINGLE,
    printerSnapshot: centauriCarbonSnapshot("Moonraker — Bay 4", "Bay 4"),
    spoolId: "spl-web-7",
    reservationId: "rsv-web-host-cancelled",
    estimateMg: 18_000,
    state: "cancelled",
    cancelReason: "hostCancelled",
    settlement: "notRequired",
    settlementMethod: null,
    settlementPreview: null,
    corrected: false,
    assignedBy: "operator",
    startConfirmation: "bedClear",
    uploadHostOperationId: null,
    activeHostOperationId: null,
    hostPath: "farm3d/calibration-cube.gcode",
    maxProgressPct: 15,
    hostUnreachableSince: null,
    lastFailure: null,
    startBlockers: [],
    allowedActions: [],
    createdAt: "2026-09-22T08:00:00Z",
    updatedAt: "2026-09-22T08:20:00Z",
    startedAt: "2026-09-22T08:05:00Z",
    endedAt: "2026-09-22T08:20:00Z",
  };
}

/** A Job whose outcome the tracker couldn't determine (spec P8
 *  `requirement.jobOutcomeUnknown`): still open (`outcomeUnknown` is not
 *  terminal, D3), its own `jobOutcomeUnknown` Requirement pending the
 *  operator's `declareOutcome`. Reuses the host-ops fixture's own
 *  uncertain-upload Host Operation, so the two fixtures tell one story. */
function outcomeUnknownEntry(): QueueEntry {
  return {
    id: WEB_QUEUE_ENTRY_OUTCOME_UNKNOWN,
    revision: 3,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    lineageId: "qln-web-outcome-unknown",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "assigned",
    closeReason: null,
    position: 7,
    policy: "manual",
    preference: "loadedFirst",
    estimate: { amountMg: 22_000, source: "sliceEstimate" },
    manualPrinterId: null,
    jobId: WEB_QUEUE_JOB_OUTCOME_UNKNOWN,
    requiresManualPrinterSelection: false,
    // D2: an `assigned` entry can only move.
    allowedActions: ["move"],
    display: {
      modelId: "mdl-web-mystery", modelName: "Mystery part", plateLabel: null,
      targetLabel: "Elegoo Centauri Carbon 0.4 nozzle", materialFamily: "PETG", materialOther: null, printSeconds: 4_100,
    },
    createdAt: "2026-09-25T07:00:00Z",
    updatedAt: "2026-09-25T07:40:00Z",
    closedAt: null,
  };
}

function outcomeUnknownJob(): Job {
  return {
    id: WEB_QUEUE_JOB_OUTCOME_UNKNOWN,
    revision: 3,
    queueEntryId: WEB_QUEUE_ENTRY_OUTCOME_UNKNOWN,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    printerId: WEB_HOST_OPS_PRINTER_UNCERTAIN_UPLOAD,
    printerSnapshot: centauriCarbonSnapshot("Uncertain upload — Bay 9", "Bay 9"),
    spoolId: "spl-web-8",
    reservationId: "rsv-web-outcome-unknown",
    estimateMg: 22_000,
    state: "outcomeUnknown",
    cancelReason: null,
    settlement: "pending",
    settlementMethod: null,
    settlementPreview: null,
    corrected: false,
    assignedBy: "operator",
    startConfirmation: "bedClear",
    uploadHostOperationId: WEB_HOST_OPS_UNCERTAIN_OPERATION,
    activeHostOperationId: WEB_HOST_OPS_UNCERTAIN_OPERATION,
    hostPath: "farm3d/mystery-part.gcode",
    maxProgressPct: 0,
    hostUnreachableSince: null,
    lastFailure: null,
    startBlockers: [],
    allowedActions: ["declareOutcome"],
    createdAt: "2026-09-25T07:00:00Z",
    updatedAt: "2026-09-25T07:40:00Z",
    startedAt: "2026-09-25T07:35:00Z",
    endedAt: null,
  };
}

function outcomeUnknownRequirement(): ReconciliationRequirement {
  return {
    id: WEB_QUEUE_REQUIREMENT_OUTCOME_UNKNOWN,
    jobId: WEB_QUEUE_JOB_OUTCOME_UNKNOWN,
    kind: "jobOutcomeUnknown",
    status: "pending",
    spoolId: null,
    reservationId: null,
    openedAt: "2026-09-25T07:40:00Z",
    deferredAt: null,
    resolvedAt: null,
    resolution: null,
  };
}

/** A normally completed Job on the Finished Printer, before the
 *  currently-staged `awaitingStartJob` was assigned there (spec P8
 *  `job.completed`). Shares that Job's Spool (still loaded). */
function completedEntry(): QueueEntry {
  return {
    id: WEB_QUEUE_ENTRY_COMPLETED,
    revision: 3,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    lineageId: "qln-web-completed",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "closed",
    closeReason: "completed",
    position: null,
    policy: "manual",
    preference: "loadedFirst",
    estimate: { amountMg: 30_000, source: "sliceEstimate" },
    manualPrinterId: WEB_HOST_OPS_PRINTER_FINISHED,
    jobId: WEB_QUEUE_JOB_COMPLETED,
    requiresManualPrinterSelection: false,
    allowedActions: [],
    display: {
      modelId: "mdl-web-lid-mount", modelName: "Lid mount", plateLabel: null,
      targetLabel: "Elegoo Centauri Carbon 0.4 nozzle", materialFamily: "PLA", materialOther: null, printSeconds: 5_000,
    },
    createdAt: "2026-09-20T08:00:00Z",
    updatedAt: "2026-09-20T09:30:00Z",
    closedAt: "2026-09-20T09:30:00Z",
  };
}

function completedJob(): Job {
  return {
    id: WEB_QUEUE_JOB_COMPLETED,
    revision: 3,
    queueEntryId: WEB_QUEUE_ENTRY_COMPLETED,
    sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
    printerId: WEB_HOST_OPS_PRINTER_FINISHED,
    printerSnapshot: finishedPrinterSnapshot(),
    spoolId: WEB_QUEUE_AWAITING_START_SPOOL_ID,
    reservationId: "rsv-web-completed",
    estimateMg: 30_000,
    state: "completed",
    cancelReason: null,
    settlement: "settled",
    settlementMethod: "measured",
    settlementPreview: null,
    corrected: false,
    assignedBy: "operator",
    startConfirmation: "bedClear",
    uploadHostOperationId: null,
    activeHostOperationId: null,
    hostPath: "farm3d/lid-mount.gcode",
    maxProgressPct: 100,
    hostUnreachableSince: null,
    lastFailure: null,
    startBlockers: [],
    allowedActions: [],
    createdAt: "2026-09-20T08:00:00Z",
    updatedAt: "2026-09-20T09:30:00Z",
    startedAt: "2026-09-20T08:05:00Z",
    endedAt: "2026-09-20T09:29:00Z",
  };
}

function bracketEligibility(entryId: string): EligibilitySummary {
  return {
    entryId,
    verdict: "awaitingOperator",
    eligibleCount: 1,
    topBlocker: null,
    candidatePrinterIds: [WEB_HOST_OPS_PRINTER_READY_SINGLE],
  };
}

function blockedEligibility(): EligibilitySummary {
  return {
    entryId: WEB_QUEUE_ENTRY_BLOCKED,
    verdict: "blocked",
    eligibleCount: 0,
    topBlocker: {
      code: "PROFILE_MISMATCH",
      message: "This Printer's profile doesn't match this Slice.",
      detail: "Bed 250 × 210 mm; this Slice needs 256 × 256 mm.",
      recovery: null,
      printerIds: [WEB_HOST_OPS_PRINTER_OCTOPRINT],
    },
    candidatePrinterIds: [],
  };
}

/** Builds a fresh fixture -- a new object graph every call, so a caller
 *  (`queue-store.ts`) that hands the arrays to a reactive store never
 *  shares mutable state with a previous load or with this module's own
 *  literals (mirrors `host-ops/web-fixtures.ts`). */
export function buildWebQueueFixture(): WebQueueFixture {
  const entries: QueueEntry[] = [
    ...linkedCopies(), blockedEntry(), printingEntry(), awaitingStartEntry(), outcomeUnknownEntry(),
    historyDeferredEntry(), hostCancelledEntry(), completedEntry(),
  ];
  const jobs: Job[] = [
    printingJob(), awaitingStartJob(), outcomeUnknownJob(), deferredJob(), hostCancelledJob(), completedJob(),
  ];
  const requirements: ReconciliationRequirement[] = [deferredRequirement(), outcomeUnknownRequirement()];
  const eligibility: EligibilitySummary[] = [
    ...WEB_QUEUE_ENTRY_BRACKET_IDS.map(bracketEligibility),
    blockedEligibility(),
  ];
  // Web mode never runs the evaluator (there is no Rust backend): the same
  // fallback `list_queue` reports before the evaluator has run (D6, R3).
  const nextAutomaticAction: NextAutomaticAction = { kind: "evaluatorNotRunning" };
  return { entries, jobs, requirements, eligibility, nextAutomaticAction };
}

const WEB_QUEUE_PRINTER_NAMES: Record<string, string> = {
  [WEB_HOST_OPS_PRINTER_READY_SINGLE]: "Moonraker — Bay 4",
  [WEB_HOST_OPS_PRINTER_OCTOPRINT]: "OctoPrint — Bay 6",
};

function printerNameFor(printerId: string): string {
  return WEB_QUEUE_PRINTER_NAMES[printerId] ?? printerId;
}

/** A stand-in candidate Spool for `explainWebQueueEntry` -- `list_queue`'s
 *  own `EligibilitySummary` (what `buildWebQueueFixture` otherwise builds)
 *  carries no Spool detail, so `explain_queue_entry`'s fuller
 *  `QueueEntryEligibility` needs one to fill `Candidate.spool`. */
const WEB_QUEUE_EXPLAIN_SPOOL: SpoolOption = { spoolId: "spl-web-1", spoolNumber: 1, loadedOnPrinter: false, availableMg: 812_000 };

function explainCandidate(printerId: string, rank: number): Candidate {
  return {
    printerId,
    printerName: printerNameFor(printerId),
    rank,
    spool: WEB_QUEUE_EXPLAIN_SPOOL,
    spoolOptions: [WEB_QUEUE_EXPLAIN_SPOOL],
    loadedMatch: false,
    lastUsedAt: null,
    manualFactsAcknowledgementRequired: false,
  };
}

/** `explain_queue_entry`'s web-mode answer (fix round 1): built from the
 *  same `EligibilitySummary` `buildWebQueueFixture()` already carries, so
 *  the two always agree -- this never runs a fresh eligibility
 *  computation, which stays Rust's job everywhere (constraint 4). `undefined`
 *  for an entry with no summary (a `closed`/`assigned` entry, or an
 *  unknown id), which `queue-store.ts` turns into `NOT_FOUND`. */
export function explainWebQueueEntry(entryId: string): QueueEntryEligibility | undefined {
  const fixture = buildWebQueueFixture();
  const summary = fixture.eligibility.find((s) => s.entryId === entryId);
  if (!summary) return undefined;
  const blockers = summary.topBlocker ? [summary.topBlocker] : [];
  const candidates = summary.candidatePrinterIds.map((printerId, index) => explainCandidate(printerId, index + 1));
  const eligiblePrinters: PrinterEligibility[] = summary.candidatePrinterIds.map((printerId): PrinterEligibility => ({
    printerId, printerName: printerNameFor(printerId), eligible: true, blockers: [],
  }));
  const blockedPrinters: PrinterEligibility[] = (summary.topBlocker?.printerIds ?? []).map((printerId): PrinterEligibility => ({
    printerId, printerName: printerNameFor(printerId), eligible: false, blockers,
  }));
  return {
    entryId,
    verdict: summary.verdict,
    candidates,
    printers: [...eligiblePrinters, ...blockedPrinters],
    blockers,
    evaluatedAt: "2026-09-25T09:00:00Z",
  };
}

function reservationStateFor(job: Job): Reservation["state"] {
  switch (job.settlement) {
    case "settled": return "consumed";
    case "pending":
    case "deferred": return "unresolved";
    case "notRequired": return "released";
    case "open": return "active";
  }
}

/** `get_job_history`'s web-mode answer (fix round 1): the Job, its Queue
 *  Entry, that entry's lineage, its Reconciliation Requirements, and its
 *  linked Host Operations pulled from `host-ops/web-fixtures.ts` (never
 *  duplicated here). The events/reservations views are minimal but
 *  correctly typed -- this task ships no history screen to read them
 *  (Tasks 14/15); a later task can enrich them once one does.
 *  `undefined` for an unknown Job id, which `queue-store.ts` turns into
 *  `NOT_FOUND`. */
export function jobHistoryForWeb(jobId: string): JobHistory | undefined {
  const fixture = buildWebQueueFixture();
  const job = fixture.jobs.find((j) => j.id === jobId);
  if (!job) return undefined;
  const entry = fixture.entries.find((e) => e.id === job.queueEntryId);
  if (!entry) return undefined;
  const lineage = fixture.entries
    .filter((e) => e.lineageId === entry.lineageId)
    .sort((a, b) => a.copyIndex - b.copyIndex);
  const requirements = fixture.requirements.filter((r) => r.jobId === jobId);
  const linkedIds = new Set([job.uploadHostOperationId, job.activeHostOperationId].filter((id): id is string => id !== null));
  const hostOperations: HostOperation[] = buildWebHostOpsFixture().hostOperations.filter((op) => linkedIds.has(op.id));
  const reservationState = reservationStateFor(job);
  const reservations: Reservation[] = [{
    id: job.reservationId,
    spoolId: job.spoolId,
    holder: { kind: "job", id: job.id },
    amountMg: job.estimateMg,
    state: reservationState,
    operationId: `${job.id}-assign`,
    createdAt: job.createdAt,
    ...(reservationState === "consumed" || reservationState === "released" ? { settledAt: job.updatedAt } : {}),
  }];
  const events: JobEvent[] = [{
    id: `${job.id}-evt-assigned`,
    jobId: job.id,
    sequence: 1,
    kind: "assigned",
    fromState: null,
    toState: "assigned",
    operationId: null,
    hostOperationId: null,
    detail: null,
    at: job.createdAt,
  }];
  return { job, entry, lineage, events, reservations, hostOperations, requirements };
}
