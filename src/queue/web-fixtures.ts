import {
  buildWebHostOpsFixture,
  WEB_HOST_OPS_PRINTER_FAILED,
  WEB_HOST_OPS_PRINTER_OCTOPRINT,
  WEB_HOST_OPS_PRINTER_READY_MULTI,
  WEB_HOST_OPS_PRINTER_READY_SINGLE,
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
 *  Five open entries: three linked copies (an `Add to Queue` of quantity
 *  3), one blocked entry, and one `assigned` entry whose Job is
 *  `printing` (on the host-ops fixture's own already-printing Printer).
 *  One closed entry carries a `failed` Job whose material settlement was
 *  deferred, with an open `materialReconciliation` Requirement. */
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
export const WEB_QUEUE_ENTRY_HISTORY_DEFERRED = "qen-web-history-deferred";
export const WEB_QUEUE_JOB_DEFERRED = "job-web-deferred";
export const WEB_QUEUE_REQUIREMENT_DEFERRED = "rqr-web-deferred";
/** The deferred Requirement's Spool: `spools/web-fixtures.ts` gives it the
 *  `reconciliation` facet and an over-reserved `availableMg` (the brief's
 *  "one Spool with `reconciliation = true`"), so the two fixtures tell one
 *  consistent story. */
export const WEB_QUEUE_DEFERRED_SPOOL_ID = "spl-web-9";

const CENTAURI_CARBON_PROFILE = {
  bedShape: { kind: "rectangular" as const, widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
  printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
  nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
  hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
  suggestedHostType: "moonraker",
};

function failedPrinterSnapshot(): PrinterSnapshot {
  return {
    name: "Failed — Bay 8",
    location: "Bay 8",
    catalogRef: { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" },
    adapterKind: "moonraker",
    profile: CENTAURI_CARBON_PROFILE,
  };
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
    settlementPreview: { estimatedUseMg: 15_000 },
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
  const entries: QueueEntry[] = [...linkedCopies(), blockedEntry(), printingEntry(), historyDeferredEntry()];
  const jobs: Job[] = [printingJob(), deferredJob()];
  const requirements: ReconciliationRequirement[] = [deferredRequirement()];
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
