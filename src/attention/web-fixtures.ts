/** Deterministic `just web` fixtures for the Attention, Incident, and
 *  camera stores (spec "Frontend architecture": "Web mode loads
 *  deterministic fixtures"). There is no Rust backend in web mode -- every
 *  id, timestamp, and byte here is fixed, so a screen or a test can assert
 *  on exact values (mirrors `src/queue/web-fixtures.ts`,
 *  `src/library/web-fixtures.ts`).
 *
 *  Covers every `ConditionKind` (the catalogue table, spec D2), one
 *  recurrence chain (`printer.offline` on Bay 3: an older resolved Event,
 *  and today's open one with `recurrenceOf` pointing at it), one open
 *  Incident with a snapshot (a generated PNG test pattern, never a
 *  photo), and one pruned snapshot. */
import { generateTestPatternPng, testPatternDataUrl } from "../cameras/test-pattern";
import type {
  AttentionBackfill,
  AttentionEvent,
  CameraSnapshot,
  Incident,
  IncidentDetail,
  IncidentPage,
  MediaUsage,
  SnapshotPage,
} from "./types";

const STREAM_ID = "stream-attention-web";

const PRINTER_OFFLINE_PRIOR_ID = "atn-w-printer-offline-prior";
const INCIDENT_ID = "inc-w-host-failed";
export const WEB_INCIDENT_SNAPSHOT_ID = "snp-w-incident-evidence";
export const WEB_PRUNED_SNAPSHOT_ID = "snp-w-pruned";
export const WEB_INCIDENT_PRINTER_ID = "prn-w-south";

/** Built once per process: the PNG bytes never change, so there is no
 *  reason to re-run the zlib framing on every `startAttention()` call. */
const SNAPSHOT_DIMENSIONS = { width: 48, height: 32, cell: 4 };
const SNAPSHOT_BYTE_LEN = generateTestPatternPng(SNAPSHOT_DIMENSIONS).length;
const SNAPSHOT_DATA_URL = testPatternDataUrl(SNAPSHOT_DIMENSIONS);

function resolvedEvent(overrides: Partial<AttentionEvent>): AttentionEvent {
  return {
    id: "atn-w",
    revision: 1,
    dedupKey: "printer.offline:printer:prn-w",
    condition: "printer.offline",
    severity: "warning",
    requiresAction: true,
    resolutionMode: "auto",
    notificationClass: "connectivity",
    source: { kind: "printer", id: "prn-w" },
    printerId: "prn-w",
    jobId: null,
    spoolId: null,
    requirementId: null,
    incidentId: null,
    subject: { printerName: null, printerLocation: null, jobLabel: null, spoolNumber: null, spoolLabel: null },
    detail: { kind: "printerOffline", unreachableSince: "2026-09-20T08:00:00Z" },
    summary: "",
    origin: "backfill",
    firstObservedAt: "2026-09-20T08:00:00Z",
    lastObservedAt: "2026-09-20T08:00:00Z",
    observationCount: 1,
    recurrenceOf: null,
    readAt: "2026-09-20T08:05:00Z",
    acknowledgedAt: null,
    resolvedAt: "2026-09-20T09:00:00Z",
    resolution: "conditionCleared",
    notifiedAt: null,
    evidence: null,
    allowedActions: [],
    ...overrides,
  };
}

function openEvent(overrides: Partial<AttentionEvent>): AttentionEvent {
  return {
    id: "atn-w",
    revision: 1,
    dedupKey: "condition:printer:prn-w",
    condition: "printer.offline",
    severity: "warning",
    requiresAction: true,
    resolutionMode: "auto",
    notificationClass: "connectivity",
    source: { kind: "printer", id: "prn-w" },
    printerId: "prn-w",
    jobId: null,
    spoolId: null,
    requirementId: null,
    incidentId: null,
    subject: { printerName: null, printerLocation: null, jobLabel: null, spoolNumber: null, spoolLabel: null },
    detail: { kind: "printerOffline", unreachableSince: "2026-09-25T08:00:00Z" },
    summary: "",
    origin: "live",
    firstObservedAt: "2026-09-25T08:00:00Z",
    lastObservedAt: "2026-09-25T08:00:00Z",
    observationCount: 1,
    recurrenceOf: null,
    readAt: null,
    acknowledgedAt: null,
    resolvedAt: null,
    resolution: null,
    notifiedAt: null,
    evidence: null,
    allowedActions: ["markRead"],
    ...overrides,
  };
}

/** One Event per `ConditionKind` (the catalogue table, D2), plus the
 *  recurrence chain's earlier resolved copy. Severities, `requiresAction`,
 *  `resolutionMode`, and `notificationClass` match the catalogue exactly
 *  -- this fixture presents the same shape Rust would send, never a
 *  frontend-invented one (global constraint 4). */
function buildEvents(): { open: AttentionEvent[]; resolved: AttentionEvent[] } {
  const printerOfflinePrior = resolvedEvent({
    id: PRINTER_OFFLINE_PRIOR_ID,
    dedupKey: "printer.offline:printer:prn-w-bay3",
    source: { kind: "printer", id: "prn-w-bay3" },
    printerId: "prn-w-bay3",
    subject: { printerName: "Bay 3", printerLocation: "Garage", jobLabel: null, spoolNumber: null, spoolLabel: null },
    detail: { kind: "printerOffline", unreachableSince: "2026-09-18T08:00:00Z" },
    summary: "Bay 3 was offline.",
  });

  const printerOffline = openEvent({
    id: "atn-w-printer-offline",
    dedupKey: "printer.offline:printer:prn-w-bay3",
    source: { kind: "printer", id: "prn-w-bay3" },
    printerId: "prn-w-bay3",
    subject: { printerName: "Bay 3", printerLocation: "Garage", jobLabel: null, spoolNumber: null, spoolLabel: null },
    detail: { kind: "printerOffline", unreachableSince: "2026-09-25T08:00:00Z" },
    summary: "Bay 3 is offline.",
    firstObservedAt: "2026-09-25T08:00:00Z",
    lastObservedAt: "2026-09-25T08:10:00Z",
    observationCount: 2,
    recurrenceOf: PRINTER_OFFLINE_PRIOR_ID,
    allowedActions: ["markRead"],
  });

  const connectionError = openEvent({
    id: "atn-w-connection-error",
    condition: "printer.connectionError",
    dedupKey: "printer.connectionError:printer:prn-w-east",
    source: { kind: "printer", id: "prn-w-east" },
    printerId: "prn-w-east",
    subject: { printerName: "East", printerLocation: null, jobLabel: null, spoolNumber: null, spoolLabel: null },
    detail: { kind: "printerConnectionError", cause: "auth" },
    summary: "East's credentials were rejected.",
    allowedActions: ["markRead"],
  });

  const hostFailed = openEvent({
    id: "atn-w-host-failed",
    condition: "printer.hostFailed",
    severity: "fatal",
    dedupKey: `printer.hostFailed:printer:${WEB_INCIDENT_PRINTER_ID}`,
    source: { kind: "printer", id: WEB_INCIDENT_PRINTER_ID },
    printerId: WEB_INCIDENT_PRINTER_ID,
    incidentId: INCIDENT_ID,
    notificationClass: "fatal",
    subject: { printerName: "South", printerLocation: "Basement", jobLabel: null, spoolNumber: null, spoolLabel: null },
    detail: { kind: "printerHostFailed" },
    summary: "South reported a failure.",
    allowedActions: ["markRead", "acknowledge"],
  });

  const startConfirmation = openEvent({
    id: "atn-w-start-confirmation",
    condition: "job.startConfirmation",
    severity: "info",
    resolutionMode: "action",
    notificationClass: "confirmation",
    dedupKey: "job.startConfirmation:job:job-w-1",
    source: { kind: "job", id: "job-w-1" },
    printerId: "prn-w-east",
    jobId: "job-w-1",
    subject: { printerName: "East", printerLocation: null, jobLabel: "Bracket", spoolNumber: null, spoolLabel: null },
    detail: { kind: "jobStartConfirmation", awaitingMaterial: true },
    summary: "Bracket on East needs the bed confirmed clear.",
    allowedActions: ["markRead", "acknowledge"],
  });

  const jobFailed = openEvent({
    id: "atn-w-job-failed",
    condition: "job.failed",
    severity: "fatal",
    resolutionMode: "manual",
    notificationClass: "fatal",
    dedupKey: "job.failed:job:job-w-2",
    source: { kind: "job", id: "job-w-2" },
    printerId: "prn-w-bay3",
    jobId: "job-w-2",
    subject: { printerName: "Bay 3", printerLocation: "Garage", jobLabel: "Bushing — Plate 2", spoolNumber: null, spoolLabel: null },
    detail: { kind: "jobFailed", endedAt: "2026-09-25T07:00:00Z" },
    summary: "Bushing — Plate 2 failed on Bay 3.",
    allowedActions: ["markRead", "resolve"],
  });

  const jobHostCancelled = openEvent({
    id: "atn-w-job-host-cancelled",
    condition: "job.hostCancelled",
    resolutionMode: "manual",
    dedupKey: "job.hostCancelled:job:job-w-3",
    source: { kind: "job", id: "job-w-3" },
    printerId: "prn-w-east",
    jobId: "job-w-3",
    subject: { printerName: "East", printerLocation: null, jobLabel: "Hinge", spoolNumber: null, spoolLabel: null },
    detail: { kind: "jobHostCancelled", endedAt: "2026-09-25T06:00:00Z" },
    summary: "Hinge was cancelled on East's own screen.",
    allowedActions: ["markRead", "resolve"],
  });

  const materialReconciliation = openEvent({
    id: "atn-w-material-reconciliation",
    condition: "requirement.materialReconciliation",
    resolutionMode: "action",
    notificationClass: "reconciliation",
    dedupKey: "requirement.materialReconciliation:reconciliationRequirement:rrq-w-1",
    source: { kind: "reconciliationRequirement", id: "rrq-w-1" },
    printerId: "prn-w-bay3",
    jobId: "job-w-4",
    requirementId: "rrq-w-1",
    subject: { printerName: "Bay 3", printerLocation: "Garage", jobLabel: "Clip", spoolNumber: 12, spoolLabel: "Prusament PLA" },
    detail: { kind: "requirementMaterialReconciliation", requirementStatus: "pending", spoolId: "spl-w-12" },
    summary: "Clip needs its material reconciled against Spool #12.",
    allowedActions: ["markRead", "acknowledge"],
  });

  const jobOutcomeUnknown = openEvent({
    id: "atn-w-outcome-unknown",
    condition: "requirement.jobOutcomeUnknown",
    severity: "fatal",
    resolutionMode: "action",
    notificationClass: "fatal",
    dedupKey: "requirement.jobOutcomeUnknown:reconciliationRequirement:rrq-w-2",
    source: { kind: "reconciliationRequirement", id: "rrq-w-2" },
    printerId: "prn-w-east",
    jobId: "job-w-5",
    requirementId: "rrq-w-2",
    subject: { printerName: "East", printerLocation: null, jobLabel: "Mount", spoolNumber: null, spoolLabel: null },
    detail: { kind: "requirementJobOutcomeUnknown" },
    summary: "Mount's outcome on East is unknown.",
    allowedActions: ["markRead", "acknowledge"],
  });

  const spoolLow = openEvent({
    id: "atn-w-spool-low",
    condition: "spool.low",
    requiresAction: false,
    notificationClass: "inventory",
    dedupKey: "spool.low:spool:spl-w-3",
    source: { kind: "spool", id: "spl-w-3" },
    printerId: null,
    spoolId: "spl-w-3",
    subject: { printerName: null, printerLocation: null, jobLabel: null, spoolNumber: 3, spoolLabel: "Overture PETG" },
    detail: { kind: "spoolLow", currentMg: 80_000, lowThresholdMg: 100_000 },
    summary: "Spool #3 is low (80 g left).",
    allowedActions: ["markRead"],
  });

  const jobCompleted = openEvent({
    id: "atn-w-job-completed",
    condition: "job.completed",
    severity: "info",
    requiresAction: false,
    notificationClass: "completion",
    dedupKey: "job.completed:job:job-w-6",
    source: { kind: "job", id: "job-w-6" },
    printerId: "prn-w-bay3",
    jobId: "job-w-6",
    subject: { printerName: "Bay 3", printerLocation: "Garage", jobLabel: "Spacer", spoolNumber: null, spoolLabel: null },
    detail: { kind: "jobCompleted", endedAt: "2026-09-25T05:00:00Z" },
    summary: "Spacer completed on Bay 3.",
    allowedActions: ["markRead"],
  });

  return {
    open: [
      printerOffline,
      connectionError,
      hostFailed,
      startConfirmation,
      jobFailed,
      jobHostCancelled,
      materialReconciliation,
      jobOutcomeUnknown,
      spoolLow,
      jobCompleted,
    ],
    resolved: [printerOfflinePrior],
  };
}

function buildIncident(): Incident {
  return {
    id: INCIDENT_ID,
    revision: 1,
    kind: "printer.hostFailed",
    state: "open",
    printerId: WEB_INCIDENT_PRINTER_ID,
    jobId: null,
    printerSnapshot: {
      name: "South",
      location: "Basement",
      catalogRef: null,
      adapterKind: "moonraker",
      profile: {
        bedShape: { kind: "rectangular", widthMm: 220, depthMm: 220, originXMm: 0, originYMm: 0 },
        printableHeightMm: 250,
        bedExcludeAreas: [],
        defaultBedType: "0",
        nozzleDiameterMm: [0.4],
        nozzleType: "brass",
        gcodeFlavor: "klipper",
        hasAuxiliaryFan: false,
        supportsAirFiltration: false,
        supportsMultiFilament: false,
        suggestedHostType: "moonraker",
      },
    },
    openedAt: "2026-09-25T07:30:00Z",
    closedAt: null,
    linkedEventIds: ["atn-w-host-failed"],
    openLinkedEventCount: 1,
    snapshotCount: 1,
  };
}

function buildSnapshots(): CameraSnapshot[] {
  return [
    {
      id: WEB_INCIDENT_SNAPSHOT_ID,
      revision: 1,
      printerId: WEB_INCIDENT_PRINTER_ID,
      incidentId: INCIDENT_ID,
      jobId: null,
      trigger: "incident",
      capturedAt: "2026-09-25T07:30:05Z",
      contentType: "image/png",
      byteLen: SNAPSHOT_BYTE_LEN,
      sha256: "web-fixture-evidence",
      pinnedAt: null,
      prunedAt: null,
      pruneReason: null,
    },
    {
      id: WEB_PRUNED_SNAPSHOT_ID,
      revision: 2,
      printerId: WEB_INCIDENT_PRINTER_ID,
      incidentId: null,
      jobId: null,
      trigger: "manual",
      capturedAt: "2026-08-20T12:00:00Z",
      contentType: "image/png",
      byteLen: SNAPSHOT_BYTE_LEN,
      sha256: "web-fixture-pruned",
      pinnedAt: null,
      prunedAt: "2026-09-24T00:00:00Z",
      pruneReason: "age",
    },
  ];
}

export interface WebAttentionFixture {
  backfill: AttentionBackfill;
  snapshots: CameraSnapshot[];
}

export function buildWebAttentionFixture(): WebAttentionFixture {
  const { open, resolved } = buildEvents();
  const snapshots = buildSnapshots();
  return {
    backfill: {
      streamId: STREAM_ID,
      snapshotSequence: 0,
      open,
      resolved,
      resolvedCursor: null,
      openIncidents: [buildIncident()],
      cameraHealth: [
        { printerId: WEB_INCIDENT_PRINTER_ID, state: "ok", sourceKind: "hostWebcam", lastSuccessAt: "2026-09-25T08:00:00Z", lastFailureAt: null, lastFailureKind: null },
        { printerId: "prn-w-bay3", state: "failing", sourceKind: "snapshotUrl", lastSuccessAt: "2026-09-24T00:00:00Z", lastFailureAt: "2026-09-25T08:00:00Z", lastFailureKind: "timeout" },
        { printerId: "prn-w-east", state: "unknown", sourceKind: "hostWebcam", lastSuccessAt: null, lastFailureAt: null, lastFailureKind: null },
      ],
    },
    snapshots,
  };
}

export function webIncidentPage(options: { state?: "open" | "closed" | "all"; printerId?: string } = {}): IncidentPage {
  const incident = buildIncident();
  const matchesState = options.state === undefined || options.state === "all" || incident.state === options.state;
  const matchesPrinter = options.printerId === undefined || incident.printerId === options.printerId;
  return { incidents: matchesState && matchesPrinter ? [incident] : [], nextCursor: null };
}

export function webIncidentDetail(incidentId: string): IncidentDetail | undefined {
  const incident = buildIncident();
  if (incidentId !== incident.id) return undefined;
  const { open } = buildEvents();
  const event = open.find((candidate) => candidate.incidentId === incident.id);
  return {
    incident,
    timeline: [
      { source: "incident", entry: { id: "ien-w-1", incidentId: incident.id, sequence: 1, kind: "opened", detail: { kind: "opened", eventId: "atn-w-host-failed" }, operationId: null, at: incident.openedAt } },
      { source: "incident", entry: { id: "ien-w-2", incidentId: incident.id, sequence: 2, kind: "evidenceCaptured", detail: { kind: "evidenceCaptured", snapshotId: WEB_INCIDENT_SNAPSHOT_ID, trigger: "incident" }, operationId: null, at: "2026-09-25T07:30:05Z" } },
    ],
    events: event ? [event] : [],
    snapshots: buildSnapshots().filter((snapshot) => snapshot.incidentId === incident.id),
  };
}

export interface WebSnapshotListOptions {
  printerId?: string;
  incidentId?: string;
  jobId?: string;
  includePruned?: boolean;
}

/** `list_snapshots`, in web mode: no paging (the fixture is two rows). */
export function webSnapshotPage(options: WebSnapshotListOptions = {}): SnapshotPage {
  const includePruned = options.includePruned ?? true;
  const snapshots = buildSnapshots().filter((snapshot) => {
    if (options.printerId !== undefined && snapshot.printerId !== options.printerId) return false;
    if (options.incidentId !== undefined && snapshot.incidentId !== options.incidentId) return false;
    if (options.jobId !== undefined && snapshot.jobId !== options.jobId) return false;
    if (!includePruned && snapshot.prunedAt !== null) return false;
    return true;
  });
  return { snapshots, nextCursor: null };
}

export function webMediaUsage(): MediaUsage {
  const snapshots = buildSnapshots();
  // `MediaUsage.usedBytes`: "the sum of stored byteLen over unpruned rows"
  // (the generated type's own doc comment) -- a pruned row's file is gone,
  // so it no longer counts toward usage even though the row survives.
  const unpruned = snapshots.filter((snapshot) => snapshot.prunedAt === null);
  return {
    usedBytes: unpruned.reduce((sum, snapshot) => sum + snapshot.byteLen, 0),
    pinnedBytes: unpruned.filter((snapshot) => snapshot.pinnedAt !== null).reduce((sum, snapshot) => sum + snapshot.byteLen, 0),
    capBytes: 2048 * 1024 * 1024,
    retentionDays: 30,
    snapshotCount: snapshots.length,
    pinnedCount: snapshots.filter((snapshot) => snapshot.pinnedAt !== null).length,
    prunedCount: snapshots.filter((snapshot) => snapshot.prunedAt !== null).length,
  };
}

/** The image data behind a fixture snapshot, as an `<img>`-ready data URL
 *  -- never a binary frame in web mode (there's no Tauri IPC to answer
 *  with one). `undefined` for the pruned row (spec: a pruned snapshot has
 *  no image) and for any id this fixture doesn't know. */
export function webSnapshotImageDataUrl(snapshotId: string): string | undefined {
  return snapshotId === WEB_INCIDENT_SNAPSHOT_ID ? SNAPSHOT_DATA_URL : undefined;
}
