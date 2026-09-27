import {
  WEB_HOST_OPS_FAILED_OPERATION,
  WEB_HOST_OPS_PRINTER_FAILED,
  WEB_HOST_OPS_PRINTER_OCTOPRINT,
  WEB_HOST_OPS_PRINTER_READY_MULTI,
  WEB_HOST_OPS_PRINTER_READY_SINGLE,
} from "../host-ops/web-fixtures";
import {
  WEB_SLICING_REVISION_EXTERNAL,
  WEB_SLICING_REVISION_FARM3D,
  WEB_SLICING_REVISION_FARM3D_OLDER,
} from "../slicing/web-fixtures";
import type {
  EligibilitySummary,
  Job,
  NextAutomaticAction,
  PrinterSnapshot,
  QueueEntry,
  ReconciliationRequirement,
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
    uploadHostOperationId: null,
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
    uploadHostOperationId: null,
    activeHostOperationId: WEB_HOST_OPS_FAILED_OPERATION,
    hostPath: "farm3d/enclosure-lid.gcode",
    maxProgressPct: 8,
    hostUnreachableSince: null,
    lastFailure: {
      kind: "hostOperationFailed",
      at: "2026-09-24T09:09:00Z",
      hostOperationId: WEB_HOST_OPS_FAILED_OPERATION,
      failure: { code: "hostNotReady", message: "Klipper isn't running on the printer." },
    },
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
