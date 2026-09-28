/** Record builders shared by the queue tests. Test-only: nothing outside a
 *  test imports this module (mirrors `src/host-ops/test-records.ts`). */
import type {
  EligibilitySummary,
  Job,
  JobHistory,
  MaterialEstimate,
  NextAutomaticAction,
  PrinterSnapshot,
  QueueChange,
  QueueEntry,
  QueueEntryEligibility,
  QueueSnapshot,
  ReconciliationRequirement,
} from "./types";

export function materialEstimate(overrides: Partial<MaterialEstimate> = {}): MaterialEstimate {
  return { amountMg: 30_000, source: "sliceEstimate", ...overrides };
}

export function printerSnapshot(overrides: Partial<PrinterSnapshot> = {}): PrinterSnapshot {
  return {
    name: "Bay 1",
    location: null,
    catalogRef: { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" },
    adapterKind: "moonraker",
    profile: {
      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
      nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
      hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: false,
      suggestedHostType: "moonraker",
    },
    ...overrides,
  };
}

export function queueEntry(overrides: Partial<QueueEntry> = {}): QueueEntry {
  return {
    id: "qen-1",
    revision: 1,
    sliceRevisionId: "slr-1",
    lineageId: "qln-1",
    copyIndex: 1,
    copyCount: 1,
    originEntryId: null,
    originKind: null,
    state: "queued",
    closeReason: null,
    position: 1,
    policy: "manual",
    preference: "loadedFirst",
    estimate: materialEstimate(),
    manualPrinterId: null,
    jobId: null,
    requiresManualPrinterSelection: false,
    allowedActions: ["assign", "update", "move", "remove"],
    display: {
      modelId: "mdl-1", modelName: "Bracket", plateLabel: "Plate 1", targetLabel: "Elegoo Centauri Carbon 0.4 nozzle",
      materialFamily: "PLA", materialOther: null, printSeconds: 3_600,
    },
    createdAt: "2026-09-25T00:00:00Z",
    updatedAt: "2026-09-25T00:00:00Z",
    closedAt: null,
    ...overrides,
  };
}

export function job(overrides: Partial<Job> = {}): Job {
  return {
    id: "job-1",
    revision: 1,
    queueEntryId: "qen-1",
    sliceRevisionId: "slr-1",
    printerId: "prn-1",
    printerSnapshot: printerSnapshot(),
    spoolId: "spl-1",
    reservationId: "rsv-1",
    estimateMg: 30_000,
    state: "assigned",
    cancelReason: null,
    settlement: "open",
    settlementMethod: null,
    settlementPreview: null,
    corrected: false,
    assignedBy: "operator",
    startConfirmation: null,
    uploadHostOperationId: null,
    activeHostOperationId: null,
    hostPath: null,
    maxProgressPct: 0,
    hostUnreachableSince: null,
    lastFailure: null,
    startBlockers: [],
    allowedActions: ["stage", "cancel", "release"],
    createdAt: "2026-09-25T00:00:00Z",
    updatedAt: "2026-09-25T00:00:00Z",
    startedAt: null,
    endedAt: null,
    ...overrides,
  };
}

export function reconciliationRequirement(overrides: Partial<ReconciliationRequirement> = {}): ReconciliationRequirement {
  return {
    id: "rqr-1",
    jobId: "job-1",
    kind: "materialReconciliation",
    status: "pending",
    spoolId: "spl-1",
    reservationId: "rsv-1",
    openedAt: "2026-09-25T00:00:00Z",
    deferredAt: null,
    resolvedAt: null,
    resolution: null,
    ...overrides,
  };
}

export function eligibilitySummary(overrides: Partial<EligibilitySummary> = {}): EligibilitySummary {
  return {
    entryId: "qen-1",
    verdict: "awaitingOperator",
    eligibleCount: 1,
    topBlocker: null,
    candidatePrinterIds: ["prn-1"],
    ...overrides,
  };
}

export function nextAutomaticAction(overrides: Partial<Extract<NextAutomaticAction, { kind: "noAutomaticEntries" }>> = {}): NextAutomaticAction {
  return { kind: "noAutomaticEntries", evaluatedAt: "2026-09-25T00:00:00Z", ...overrides };
}

export function queueSnapshot(
  sequence: number,
  overrides: Partial<Omit<QueueSnapshot, "snapshotSequence">> = {},
): QueueSnapshot {
  return {
    streamId: "stream-queue",
    snapshotSequence: sequence,
    entries: [],
    jobs: [],
    requirements: [],
    eligibility: [],
    nextAutomaticAction: { kind: "evaluatorNotRunning" },
    ...overrides,
  };
}

export function queueChange(overrides: Partial<QueueChange> = {}): QueueChange {
  return { entries: [], jobs: [], requirements: [], ...overrides };
}

export function queueEntryEligibility(overrides: Partial<QueueEntryEligibility> = {}): QueueEntryEligibility {
  return {
    entryId: "qen-1",
    verdict: "awaitingOperator",
    candidates: [],
    printers: [],
    blockers: [],
    evaluatedAt: "2026-09-25T00:00:00Z",
    ...overrides,
  };
}

export function jobHistory(overrides: Partial<JobHistory> = {}): JobHistory {
  return {
    job: job(),
    entry: queueEntry(),
    lineage: [queueEntry()],
    events: [],
    reservations: [],
    hostOperations: [],
    requirements: [],
    ...overrides,
  };
}
