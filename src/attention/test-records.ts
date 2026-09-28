/** Record builders shared by the attention/incident/camera tests. Test-only:
 *  nothing outside a test imports this module (mirrors
 *  `src/queue/test-records.ts`). */
import type {
  AttentionBackfill,
  AttentionChange,
  AttentionEvent,
  AttentionSubject,
  CameraHealth,
  CameraSnapshot,
  Incident,
} from "./types";

export function attentionSubject(overrides: Partial<AttentionSubject> = {}): AttentionSubject {
  return {
    printerName: "Bay 1",
    printerLocation: null,
    jobLabel: null,
    spoolNumber: null,
    spoolLabel: null,
    ...overrides,
  };
}

export function attentionEvent(overrides: Partial<AttentionEvent> = {}): AttentionEvent {
  return {
    id: "atn-1",
    revision: 1,
    dedupKey: "printer.offline:printer:prn-1",
    condition: "printer.offline",
    severity: "warning",
    requiresAction: false,
    resolutionMode: "auto",
    notificationClass: "connectivity",
    source: { kind: "printer", id: "prn-1" },
    printerId: "prn-1",
    jobId: null,
    spoolId: null,
    requirementId: null,
    incidentId: null,
    subject: attentionSubject(),
    detail: { kind: "printerOffline", unreachableSince: "2026-09-25T00:00:00Z" },
    summary: "Bay 1 is offline.",
    origin: "live",
    firstObservedAt: "2026-09-25T00:00:00Z",
    lastObservedAt: "2026-09-25T00:00:00Z",
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

export function incident(overrides: Partial<Incident> = {}): Incident {
  return {
    id: "inc-1",
    revision: 1,
    kind: "printer.hostFailed",
    state: "open",
    printerId: "prn-1",
    jobId: null,
    printerSnapshot: {
      name: "Bay 1",
      location: null,
      catalogRef: null,
      adapterKind: "moonraker",
      profile: {
        bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
        printableHeightMm: 256,
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
    openedAt: "2026-09-25T00:00:00Z",
    closedAt: null,
    linkedEventIds: ["atn-1"],
    openLinkedEventCount: 1,
    snapshotCount: 0,
    ...overrides,
  };
}

export function cameraHealth(overrides: Partial<CameraHealth> = {}): CameraHealth {
  return {
    printerId: "prn-1",
    state: "ok",
    sourceKind: "hostWebcam",
    lastSuccessAt: "2026-09-25T00:00:00Z",
    lastFailureAt: null,
    lastFailureKind: null,
    ...overrides,
  };
}

export function cameraSnapshot(overrides: Partial<CameraSnapshot> = {}): CameraSnapshot {
  return {
    id: "snp-1",
    revision: 1,
    printerId: "prn-1",
    incidentId: null,
    jobId: null,
    trigger: "manual",
    capturedAt: "2026-09-25T00:00:00Z",
    contentType: "image/png",
    byteLen: 128,
    sha256: "0".repeat(64),
    pinnedAt: null,
    prunedAt: null,
    pruneReason: null,
    ...overrides,
  };
}

export function attentionBackfill(overrides: Partial<AttentionBackfill> = {}): AttentionBackfill {
  return {
    streamId: "stream-attention",
    snapshotSequence: 0,
    open: [],
    resolved: [],
    resolvedCursor: null,
    openIncidents: [],
    cameraHealth: [],
    ...overrides,
  };
}

export function attentionChange(overrides: Partial<AttentionChange> = {}): AttentionChange {
  return { events: [], incidents: [], ...overrides };
}
