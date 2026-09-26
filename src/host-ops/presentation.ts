/** Pure presentation: every string here is in the spec's "Frontend
 *  architecture" section and D11. Nothing here calls a command or touches
 *  a store — components pass in the data they already hold. */
import type {
  CapabilityState,
  EvidenceTier,
  HostOperationFailureCode,
  HostOperationKind,
  HostOperationResolution,
  HostOperationState,
  InconclusiveReason,
} from "./types";

export type Severity = "info" | "success" | "warning" | "error" | "neutral";

export interface PresentationLabel {
  text: string;
  severity: Severity;
}

export interface CapabilityLabel extends PresentationLabel {
  /** Only set for a `host`-reason unsupported capability (spec: "'host' →
   *  'Not available on this printer' plus the detail"). */
  detail?: string;
}

const VERB_LABEL: Record<HostOperationKind, string> = {
  upload: "Upload",
  start: "Start",
  pause: "Pause",
  resume: "Resume",
  cancel: "Cancel",
};

const DISPATCHING_TEXT: Record<HostOperationKind, string> = {
  upload: "Uploading",
  start: "Starting",
  pause: "Pausing",
  resume: "Resuming",
  cancel: "Cancelling",
};

const SUCCEEDED_TEXT: Record<HostOperationKind, string> = {
  upload: "Staged",
  start: "Started",
  pause: "Paused",
  resume: "Resumed",
  cancel: "Cancelled",
};

/** The minimal shape `hostOperationLabel` needs — callers pass the
 *  `HostOperation` itself, or just these three fields. */
export interface LabeledHostOperation {
  kind: HostOperationKind;
  state: HostOperationState;
  resolution: HostOperationResolution | null;
}

/** Host Operation labels per kind and state (spec table), with severity. */
export function hostOperationLabel(operation: LabeledHostOperation): PresentationLabel {
  const { kind, state, resolution } = operation;
  switch (state) {
    case "dispatching":
      return { text: DISPATCHING_TEXT[kind], severity: "info" };
    case "uncertain":
      return { text: `${VERB_LABEL[kind]} uncertain`, severity: "warning" };
    case "reconciling":
      return { text: "Checking", severity: "info" };
    case "succeeded":
      if (kind === "start" && resolution?.kind === "startObserved" && resolution.interrupted) {
        return { text: "Started, then interrupted", severity: "success" };
      }
      return { text: SUCCEEDED_TEXT[kind], severity: "success" };
    case "failed":
      return { text: `${VERB_LABEL[kind]} failed`, severity: "error" };
    case "abandoned":
      return { text: "Abandoned", severity: "warning" };
  }
}

/** D11's `HostOperationFailure.code` copy. */
export function hostOperationFailureText(code: HostOperationFailureCode): string {
  switch (code) {
    case "neverSent": return "farm3d closed before sending this. Nothing reached the printer.";
    case "hostUnreachable": return "farm3d couldn't connect to the printer. Nothing was sent.";
    case "authRejected": return "The printer rejected farm3d's API key.";
    case "checksumRejected": return "The printer found the upload damaged and discarded it.";
    case "fileLoaded": return "The printer is using a file with this name, so it refused the upload.";
    case "hostBusy": return "The printer is busy with another print.";
    case "fileMissing": return "The printer couldn't find the staged file.";
    case "hostRejected": return "The printer refused the request.";
    case "hostNotReady": return "Klipper isn't running on the printer.";
    case "notApplied": return "The file isn't on the printer, and it didn't appear within a minute.";
    case "hostFileDiffers": return "A different file is at farm3d's path on the printer. Staging again replaces it.";
  }
}

/** D11's `InconclusiveReason` copy (`last_attempt_reason`). */
export function inconclusiveReasonText(reason: InconclusiveReason): string {
  switch (reason) {
    case "responseLost": return "The printer's answer was lost.";
    case "unexpectedResponse": return "The printer gave an answer farm3d doesn't understand.";
    case "interruptedByRestart": return "farm3d closed while this was being sent.";
    case "klipperRestarted": return "Klipper restarted while this was waiting.";
    case "hostUnreachable": return "farm3d can't reach the printer to check.";
    case "authRejected": return "The printer rejected farm3d's API key while checking.";
    case "hostNotReady": return "Klipper isn't ready, so farm3d can't check yet.";
    case "identityCheckFailed": return "farm3d couldn't read the file back to check it.";
    case "uploadSettling": return "The file isn't on the printer, or doesn't match yet. farm3d waits a minute before deciding.";
    case "noStartEvidence": return "The printer shows no sign that this print started.";
    case "differentFileOnHost": return "The printer is busy with a different file.";
    case "effectNotObserved": return "The printer hasn't shown the change yet.";
  }
}

/** `AbandonReconciliationDialog`'s extra sentence when `noLongerPending`
 *  is set. */
export const NO_LONGER_PENDING_TEXT =
  "Klipper restarted after farm3d sent this, so it is no longer waiting to run.";

const ADAPTER_DISPLAY_NAMES: Record<string, string> = {
  moonraker: "Moonraker",
  octoprint: "OctoPrint",
};

function adapterDisplayName(adapterKind: string): string {
  return ADAPTER_DISPLAY_NAMES[adapterKind] ?? (adapterKind.charAt(0).toUpperCase() + adapterKind.slice(1));
}

/** `CapabilityState` labels. Unsupported and failed never share copy or
 *  severity: unsupported is `neutral`, failed (a Host Operation's, see
 *  `hostOperationLabel`) is `error`. */
export function capabilityStateLabel(state: CapabilityState, adapterKind: string | null): CapabilityLabel {
  if (state.status === "supported") return { text: "Supported", severity: "success" };
  switch (state.reason) {
    case "adapter":
      return { text: adapterKind ? `Not supported by ${adapterDisplayName(adapterKind)}` : "No Connection", severity: "neutral" };
    case "notVerified":
      return { text: "Not verified yet", severity: "neutral" };
    case "host":
      return { text: "Not available on this printer", severity: "neutral", detail: state.detail };
  }
}

/** The evidence tier CapabilityList shows next to a supported capability. */
export function evidenceTierLabel(tier: EvidenceTier): string {
  return tier === "sim" ? "Simulator" : "Read-only hardware";
}
