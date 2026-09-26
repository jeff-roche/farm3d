/** Record builders shared by the host-ops tests (and the screens built on
 *  them). Test-only: nothing outside a test imports this module. */
import type { ResolvedPrinter } from "../printers/types";
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

/** A Printer as the printer store holds it: a Moonraker Connection, no
 *  credential, and the given live status. */
export function resolvedPrinter(overrides: Partial<ResolvedPrinter> = {}): ResolvedPrinter {
  return {
    id: "prn-1",
    revision: 1,
    name: "Bay 1",
    notes: "",
    overrides: {},
    catalogRef: { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" },
    catalogStatus: "ok",
    modelLabel: "Elegoo Centauri Carbon",
    variantLabel: "Elegoo Centauri Carbon 0.4 nozzle",
    profile: {
      bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
      printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
      nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
      hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
      suggestedHostType: "moonraker",
    },
    overriddenFields: [],
    inherited: {},
    profileDrift: [],
    unknownOverrideKeys: [],
    startSafety: "confirmBedClear",
    materialSlots: [{ id: "slt-1", position: 0, name: "Main" }],
    setupGaps: [],
    connection: { kind: "moonraker", host: "192.0.2.10", port: 7125, useTls: false },
    runtimeStatus: printerStatus("ready"),
    createdAt: "",
    updatedAt: "",
    ...overrides,
  };
}
