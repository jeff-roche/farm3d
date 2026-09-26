/** Record builders shared by the host-ops tests. Test-only: nothing
 *  outside a test imports this module. */
import type {
  HostOperation,
  HostOperationsSnapshot,
  OperationalState,
  PrinterCapabilities,
  PrinterStatus,
  TelemetryFreshness,
} from "./types";

export function printerStatus(
  state: OperationalState,
  freshness: TelemetryFreshness = "fresh",
  overrides: Partial<PrinterStatus> = {},
): PrinterStatus {
  return {
    connectionState: "online",
    telemetry: {
      hostActivity: "idle",
    },
    operationalState: state,
    readiness: { state: "ready", reason: null },
    freshness,
    cacheWarnings: [],
    updatedAt: "2026-09-25T00:00:00Z",
    ...overrides,
  };
}

export function hostOperationEndpoint(overrides: Partial<HostOperation["endpoint"]> = {}) {
  return { kind: "moonraker", host: "192.0.2.10", port: 7125, ...overrides };
}

export function hostOperation(overrides: Partial<HostOperation> = {}): HostOperation {
  return {
    id: "hop-1",
    printerId: "prn-1",
    kind: "upload",
    state: "dispatching",
    sliceRevisionId: "slr-1",
    sourceHostOperationId: null,
    gcodeSha256: null,
    gcodeSize: null,
    hostPath: "farm3d/plate.gcode",
    endpoint: hostOperationEndpoint(),
    failure: null,
    resolution: null,
    attempts: 0,
    lastAttempt: null,
    noLongerPending: false,
    abandonedAt: null,
    abandonNote: null,
    createdAt: "2026-09-25T00:00:00Z",
    dispatchedAt: null,
    uncertainSince: null,
    resolvedAt: null,
    ...overrides,
  };
}

export function hostOperationsSnapshot(
  sequence: number,
  overrides: Partial<Omit<HostOperationsSnapshot, "snapshotSequence">> = {},
): HostOperationsSnapshot {
  return {
    streamId: "stream-host-ops",
    snapshotSequence: sequence,
    operations: [],
    ...overrides,
  };
}

export function printerCapabilities(overrides: Partial<PrinterCapabilities> = {}): PrinterCapabilities {
  return {
    printerId: "prn-1",
    adapterKind: "moonraker",
    capabilities: {
      upload: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
      start: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
      pause: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
      resume: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
      cancel: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
      hostState: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
      artifactIdentity: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
      camera: { status: "supported", evidence: { source: "sim-runs/2026-09-01T00-00-00Z/manifest.json", tier: "sim", verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"] } },
    },
    hostFacts: {
      components: ["virtual_sdcard", "history", "pause_resume"],
      hasVirtualSdcard: true,
      hasPauseResume: true,
      hasHistory: true,
      hasHeaterBed: true,
      toolCount: 1,
      cameraCount: 0,
      hostSoftware: "Moonraker",
      apiVersion: "1.5.0",
    },
    observedAt: "2026-09-25T00:00:00Z",
    ...overrides,
  };
}
