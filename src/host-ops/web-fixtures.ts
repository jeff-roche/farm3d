import { WEB_SLICING_REVISION_FARM3D } from "../slicing/web-fixtures";
import type { ConnectionConfig } from "../printers/types";
import type {
  AdapterCapabilityRow,
  CapabilityEvidence,
  CapabilityKey,
  CapabilityState,
  HostOperation,
  OperationalState,
  PrinterCapabilities,
  PrinterStatus,
} from "./types";

/** `just web`'s host-ops seed data (spec "Frontend architecture", State):
 *  there is no Rust backend in web mode, so this stands in for
 *  `printer_capabilities`, `adapter_capability_matrix`, and
 *  `list_host_operations`, written as literals the way Rust would send
 *  them. Not persistence: web-mode edits (were there any — Task 10 adds
 *  none) would last only until the page reloads.
 *
 *  Six Printer ids, one per required scenario: a Ready single-tool
 *  Moonraker Printer, a four-tool Moonraker Printer, an OctoPrint Printer
 *  (every write `notVerified`), a Printer with a staged artifact, a
 *  Printer with a failed Host Operation, and a Printer with an uncertain
 *  upload. */
export interface WebHostOpsFixture {
  /** Keyed by Printer id. */
  capabilities: Record<string, PrinterCapabilities>;
  adapterMatrix: AdapterCapabilityRow[];
  hostOperations: HostOperation[];
}

export const WEB_HOST_OPS_PRINTER_READY_SINGLE = "prn-web-hostops-ready-single";
export const WEB_HOST_OPS_PRINTER_READY_MULTI = "prn-web-hostops-ready-multi";
export const WEB_HOST_OPS_PRINTER_OCTOPRINT = "prn-web-hostops-octoprint";
export const WEB_HOST_OPS_PRINTER_FINISHED = "prn-web-hostops-finished";
export const WEB_HOST_OPS_PRINTER_FAILED = "prn-web-hostops-failed";
export const WEB_HOST_OPS_PRINTER_UNCERTAIN_UPLOAD = "prn-web-hostops-uncertain-upload";

/** The six Printers themselves, which `printer-store.ts` joins onto its
 *  own web-mode Printers so `just web` shows them in the Printer list and
 *  the Job tab. Catalog refs resolve against the bundled catalog the same
 *  way `printer-store.ts`'s own seeds do; the status is what the
 *  supervisor would push. No credential: none of them has one. */
export interface WebHostOpsPrinter {
  id: string;
  name: string;
  vendor: string;
  model: string;
  printerVariant: string;
  connection: ConnectionConfig;
  status: PrinterStatus;
}

function webStatus(state: OperationalState, telemetry: Partial<PrinterStatus["telemetry"]> = {}): PrinterStatus {
  const ready = state === "ready";
  return {
    connectionState: "online",
    telemetry: {
      hostActivity: state === "printing" || state === "paused" || state === "finished" || state === "cancelled" || state === "failed" ? state : "idle",
      nozzleTempC: 24,
      nozzleTargetC: 0,
      bedTempC: 23,
      bedTargetC: 0,
      ...telemetry,
    },
    lastObservedAt: "2026-09-24T12:00:00Z",
    operationalState: state,
    readiness: ready
      ? { state: "ready", reason: null }
      : { state: "notReady", reason: state === "failed" ? "printFailed" : state === "printing" ? "printerBusy" : "bedNeedsClearing" },
    freshness: "fresh",
    cacheWarnings: [],
    updatedAt: "2026-09-24T12:00:00Z",
  };
}

function moonrakerConnection(host: string): ConnectionConfig {
  return { kind: "moonraker", host, port: 7125, useTls: false };
}

export const WEB_HOST_OPS_PRINTERS: WebHostOpsPrinter[] = [
  {
    id: WEB_HOST_OPS_PRINTER_READY_SINGLE, name: "Moonraker — Bay 4",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.4",
    connection: moonrakerConnection("192.0.2.21"),
    status: webStatus("ready"),
  },
  {
    id: WEB_HOST_OPS_PRINTER_READY_MULTI, name: "Four-tool — Bay 5",
    vendor: "Snapmaker", model: "Snapmaker U1", printerVariant: "0.4",
    connection: moonrakerConnection("192.0.2.22"),
    status: webStatus("printing", {
      jobName: "farm3d/bracket-set.gcode",
      progress: 0.42,
      printDurationS: 2_730,
      nozzleTempC: 220,
      nozzleTargetC: 220,
      bedTempC: 60,
      bedTargetC: 60,
      tools: [
        { index: 0, tempC: 220, targetC: 220 },
        { index: 1, tempC: 150, targetC: 150 },
        { index: 2, tempC: 24, targetC: 0 },
        { index: 3, tempC: 25, targetC: 0 },
      ],
    }),
  },
  {
    id: WEB_HOST_OPS_PRINTER_OCTOPRINT, name: "OctoPrint — Bay 6",
    vendor: "Prusa", model: "Prusa MK4", printerVariant: "0.4",
    connection: { kind: "octoprint", host: "192.0.2.23", port: 80, useTls: false },
    status: webStatus("ready"),
  },
  {
    id: WEB_HOST_OPS_PRINTER_FINISHED, name: "Finished — Bay 7",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.4",
    connection: moonrakerConnection("192.0.2.24"),
    status: webStatus("finished", { jobName: "farm3d/enclosure-lid.gcode", progress: 1 }),
  },
  {
    id: WEB_HOST_OPS_PRINTER_FAILED, name: "Failed — Bay 8",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.4",
    connection: moonrakerConnection("192.0.2.25"),
    status: webStatus("failed", { jobName: "farm3d/enclosure-lid.gcode", progress: 0.08 }),
  },
  {
    id: WEB_HOST_OPS_PRINTER_UNCERTAIN_UPLOAD, name: "Uncertain upload — Bay 9",
    vendor: "Elegoo", model: "Elegoo Centauri Carbon", printerVariant: "0.4",
    connection: moonrakerConnection("192.0.2.26"),
    status: webStatus("ready"),
  },
];

export const WEB_HOST_OPS_STAGED_OPERATION = "hop-web-finished-staged";
export const WEB_HOST_OPS_FAILED_OPERATION = "hop-web-failed-start";
export const WEB_HOST_OPS_UNCERTAIN_OPERATION = "hop-web-uncertain-upload";

const SIM_EVIDENCE: CapabilityEvidence = {
  source: "sim-runs/2026-09-01T00-00-00Z/manifest.json",
  tier: "sim",
  verifiedHostVersions: ["Moonraker v0.11.0-1 API 1.5.0"],
};

const HARDWARE_EVIDENCE: CapabilityEvidence = {
  source: "hardware-runs/2026-08-15T00-00-00Z/manifest.json",
  tier: "readOnlyHardware",
  verifiedHostVersions: ["OctoPrint 1.10.0"],
};

function supported(evidence: CapabilityEvidence): CapabilityState {
  return { status: "supported", evidence };
}

function notVerified(detail: string): CapabilityState {
  return { status: "unsupported", reason: "notVerified", detail };
}

/** A single-tool Moonraker Printer, fully supported on simulator
 *  evidence — the common case. */
function moonrakerCapabilities(printerId: string, toolCount: number): PrinterCapabilities {
  const capabilities: Record<CapabilityKey, CapabilityState> = {
    upload: supported(SIM_EVIDENCE),
    start: supported(SIM_EVIDENCE),
    pause: supported(SIM_EVIDENCE),
    resume: supported(SIM_EVIDENCE),
    cancel: supported(SIM_EVIDENCE),
    hostState: supported(SIM_EVIDENCE),
    artifactIdentity: supported(SIM_EVIDENCE),
    camera: supported(SIM_EVIDENCE),
  };
  return {
    printerId,
    adapterKind: "moonraker",
    capabilities,
    hostFacts: {
      components: ["virtual_sdcard", "history", "pause_resume", "webcam"],
      hasVirtualSdcard: true,
      hasPauseResume: true,
      hasHistory: true,
      hasHeaterBed: true,
      toolCount,
      cameraCount: 1,
      hostSoftware: "Moonraker",
      apiVersion: "1.5.0",
    },
    observedAt: "2026-09-24T12:00:00Z",
  };
}

/** An OctoPrint Printer whose writes are not yet verified against live
 *  evidence (only Moonraker has local simulator evidence, spec ADR-0012);
 *  its read-only capabilities are backed by read-only-hardware evidence. */
function octoprintCapabilities(printerId: string): PrinterCapabilities {
  const capabilities: Record<CapabilityKey, CapabilityState> = {
    upload: notVerified("OctoPrint's upload has no verified evidence yet."),
    start: notVerified("OctoPrint's start has no verified evidence yet."),
    pause: notVerified("OctoPrint's pause has no verified evidence yet."),
    resume: notVerified("OctoPrint's resume has no verified evidence yet."),
    cancel: notVerified("OctoPrint's cancel has no verified evidence yet."),
    hostState: supported(HARDWARE_EVIDENCE),
    artifactIdentity: supported(HARDWARE_EVIDENCE),
    camera: supported(HARDWARE_EVIDENCE),
  };
  return {
    printerId,
    adapterKind: "octoprint",
    capabilities,
    hostFacts: {
      components: ["file_manager", "printer_state"],
      hasVirtualSdcard: false,
      hasPauseResume: true,
      hasHistory: false,
      hasHeaterBed: true,
      toolCount: 1,
      cameraCount: 1,
      hostSoftware: "OctoPrint",
      apiVersion: "1.10.0",
    },
    observedAt: "2026-09-24T12:00:00Z",
  };
}

function endpoint() {
  return { kind: "moonraker", host: "192.0.2.20", port: 7125 };
}

export function buildWebHostOpsFixture(): WebHostOpsFixture {
  const capabilities: Record<string, PrinterCapabilities> = {
    [WEB_HOST_OPS_PRINTER_READY_SINGLE]: moonrakerCapabilities(WEB_HOST_OPS_PRINTER_READY_SINGLE, 1),
    [WEB_HOST_OPS_PRINTER_READY_MULTI]: moonrakerCapabilities(WEB_HOST_OPS_PRINTER_READY_MULTI, 4),
    [WEB_HOST_OPS_PRINTER_OCTOPRINT]: octoprintCapabilities(WEB_HOST_OPS_PRINTER_OCTOPRINT),
    [WEB_HOST_OPS_PRINTER_FINISHED]: moonrakerCapabilities(WEB_HOST_OPS_PRINTER_FINISHED, 1),
    [WEB_HOST_OPS_PRINTER_FAILED]: moonrakerCapabilities(WEB_HOST_OPS_PRINTER_FAILED, 1),
    [WEB_HOST_OPS_PRINTER_UNCERTAIN_UPLOAD]: moonrakerCapabilities(WEB_HOST_OPS_PRINTER_UNCERTAIN_UPLOAD, 1),
  };

  const adapterMatrix: AdapterCapabilityRow[] = [
    { adapterKind: "moonraker", capabilities: moonrakerCapabilities("adapter-baseline", 1).capabilities },
    { adapterKind: "octoprint", capabilities: octoprintCapabilities("adapter-baseline").capabilities },
  ];

  const hostOperations: HostOperation[] = [
    {
      id: WEB_HOST_OPS_STAGED_OPERATION,
      printerId: WEB_HOST_OPS_PRINTER_FINISHED,
      kind: "upload",
      state: "succeeded",
      sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
      sourceHostOperationId: null,
      gcodeSha256: "9f2c7a1e4b3d5f60718293a4b5c6d7e8f9012345678901234567890abcdef01",
      gcodeSize: 4_213_552,
      hostPath: "farm3d/enclosure-lid.gcode",
      endpoint: endpoint(),
      failure: null,
      resolution: { kind: "artifactVerified", reconciled: true },
      attempts: 0,
      lastAttempt: null,
      noLongerPending: false,
      abandonedAt: null,
      abandonNote: null,
      createdAt: "2026-09-23T10:00:00Z",
      dispatchedAt: "2026-09-23T10:00:01Z",
      uncertainSince: null,
      resolvedAt: "2026-09-23T10:00:05Z",
    },
    {
      id: WEB_HOST_OPS_FAILED_OPERATION,
      printerId: WEB_HOST_OPS_PRINTER_FAILED,
      kind: "start",
      state: "failed",
      sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
      sourceHostOperationId: null,
      gcodeSha256: null,
      gcodeSize: null,
      hostPath: "farm3d/enclosure-lid.gcode",
      endpoint: endpoint(),
      failure: { code: "hostNotReady", message: "Klipper isn't running on the printer." },
      resolution: null,
      attempts: 0,
      lastAttempt: null,
      noLongerPending: false,
      abandonedAt: null,
      abandonNote: null,
      createdAt: "2026-09-24T09:00:00Z",
      dispatchedAt: "2026-09-24T09:00:01Z",
      uncertainSince: null,
      resolvedAt: "2026-09-24T09:00:02Z",
    },
    {
      id: WEB_HOST_OPS_UNCERTAIN_OPERATION,
      printerId: WEB_HOST_OPS_PRINTER_UNCERTAIN_UPLOAD,
      kind: "upload",
      state: "uncertain",
      sliceRevisionId: WEB_SLICING_REVISION_FARM3D,
      sourceHostOperationId: null,
      gcodeSha256: "1a2b3c4d5e6f7089a1b2c3d4e5f60718293a4b5c6d7e8f9012345678901abcd",
      gcodeSize: 2_048_000,
      hostPath: "farm3d/enclosure-lid.gcode",
      endpoint: endpoint(),
      failure: null,
      resolution: null,
      attempts: 1,
      lastAttempt: { at: "2026-09-24T11:00:00Z", reason: "responseLost" },
      noLongerPending: false,
      abandonedAt: null,
      abandonNote: null,
      createdAt: "2026-09-24T10:59:00Z",
      dispatchedAt: "2026-09-24T10:59:01Z",
      uncertainSince: "2026-09-24T10:59:30Z",
      resolvedAt: null,
    },
  ];

  return { capabilities, adapterMatrix, hostOperations };
}
