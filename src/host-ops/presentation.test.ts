import { describe, expect, it } from "vitest";
import {
  capabilityStateLabel,
  evidenceTierLabel,
  hostOperationFailureText,
  hostOperationLabel,
  inconclusiveReasonText,
  NO_LONGER_PENDING_TEXT,
} from "./presentation";
import type {
  CapabilityState,
  HostOperationFailureCode,
  HostOperationKind,
  HostOperationState,
  InconclusiveReason,
} from "./types";

const KINDS: HostOperationKind[] = ["upload", "start", "pause", "resume", "cancel"];
const STATES: HostOperationState[] = ["dispatching", "uncertain", "reconciling", "succeeded", "failed", "abandoned"];

describe("hostOperationLabel", () => {
  it("labels every kind/state pair from the spec table", () => {
    const expected: Record<HostOperationKind, Record<HostOperationState, string>> = {
      upload: {
        dispatching: "Uploading", uncertain: "Upload uncertain", reconciling: "Checking",
        succeeded: "Staged", failed: "Upload failed", abandoned: "Abandoned",
      },
      start: {
        dispatching: "Starting", uncertain: "Start uncertain", reconciling: "Checking",
        succeeded: "Started", failed: "Start failed", abandoned: "Abandoned",
      },
      pause: {
        dispatching: "Pausing", uncertain: "Pause uncertain", reconciling: "Checking",
        succeeded: "Paused", failed: "Pause failed", abandoned: "Abandoned",
      },
      resume: {
        dispatching: "Resuming", uncertain: "Resume uncertain", reconciling: "Checking",
        succeeded: "Resumed", failed: "Resume failed", abandoned: "Abandoned",
      },
      cancel: {
        dispatching: "Cancelling", uncertain: "Cancel uncertain", reconciling: "Checking",
        succeeded: "Cancelled", failed: "Cancel failed", abandoned: "Abandoned",
      },
    };
    const expectedSeverity: Record<HostOperationState, string> = {
      dispatching: "info", uncertain: "warning", reconciling: "info",
      succeeded: "success", failed: "error", abandoned: "warning",
    };
    for (const kind of KINDS) {
      for (const state of STATES) {
        const label = hostOperationLabel({ kind, state, resolution: null });
        expect(label.text, `${kind}/${state}`).toBe(expected[kind][state]);
        expect(label.severity, `${kind}/${state}`).toBe(expectedSeverity[state]);
      }
    }
  });

  it("shows 'Started, then interrupted' for a succeeded start whose resolution says so", () => {
    const label = hostOperationLabel({
      kind: "start", state: "succeeded",
      resolution: { kind: "startObserved", source: "printStats", historyJobId: null, interrupted: true },
    });
    expect(label).toEqual({ text: "Started, then interrupted", severity: "success" });
  });

  it("shows plain 'Started' for a succeeded start that was not interrupted", () => {
    const label = hostOperationLabel({
      kind: "start", state: "succeeded",
      resolution: { kind: "startObserved", source: "printStats", historyJobId: null, interrupted: false },
    });
    expect(label).toEqual({ text: "Started", severity: "success" });
  });
});

describe("hostOperationFailureText", () => {
  const expected: Record<HostOperationFailureCode, string> = {
    neverSent: "farm3d closed before sending this. Nothing reached the printer.",
    hostUnreachable: "farm3d couldn't connect to the printer. Nothing was sent.",
    authRejected: "The printer rejected farm3d's API key.",
    checksumRejected: "The printer found the upload damaged and discarded it.",
    fileLoaded: "The printer is using a file with this name, so it refused the upload.",
    hostBusy: "The printer is busy with another print.",
    fileMissing: "The printer couldn't find the staged file.",
    hostRejected: "The printer refused the request.",
    hostNotReady: "Klipper isn't running on the printer.",
    notApplied: "The file isn't on the printer, and it didn't appear within a minute.",
    hostFileDiffers: "A different file is at farm3d's path on the printer. Staging again replaces it.",
  };
  it("gives D11's exact message for every failure code", () => {
    for (const [code, text] of Object.entries(expected)) {
      expect(hostOperationFailureText(code as HostOperationFailureCode)).toBe(text);
    }
  });
});

describe("inconclusiveReasonText", () => {
  const expected: Record<InconclusiveReason, string> = {
    responseLost: "The printer's answer was lost.",
    unexpectedResponse: "The printer gave an answer farm3d doesn't understand.",
    interruptedByRestart: "farm3d closed while this was being sent.",
    klipperRestarted: "Klipper restarted while this was waiting.",
    hostUnreachable: "farm3d can't reach the printer to check.",
    authRejected: "The printer rejected farm3d's API key while checking.",
    hostNotReady: "Klipper isn't ready, so farm3d can't check yet.",
    identityCheckFailed: "farm3d couldn't read the file back to check it.",
    uploadSettling: "The file isn't on the printer, or doesn't match yet. farm3d waits a minute before deciding.",
    noStartEvidence: "The printer shows no sign that this print started.",
    differentFileOnHost: "The printer is busy with a different file.",
    effectNotObserved: "The printer hasn't shown the change yet.",
  };
  it("gives D11's exact message for every inconclusive reason", () => {
    for (const [reason, text] of Object.entries(expected)) {
      expect(inconclusiveReasonText(reason as InconclusiveReason)).toBe(text);
    }
  });
});

it("noLongerPending has the sentence AbandonReconciliationDialog needs", () => {
  expect(NO_LONGER_PENDING_TEXT).toBe(
    "Klipper restarted after farm3d sent this, so it is no longer waiting to run.",
  );
});

describe("capabilityStateLabel", () => {
  it("labels a supported capability", () => {
    const state: CapabilityState = {
      status: "supported",
      evidence: { source: "sim-runs/x/manifest.json", tier: "sim", verifiedHostVersions: [] },
    };
    expect(capabilityStateLabel(state, "moonraker")).toEqual({ text: "Supported", severity: "success" });
  });

  it("labels an adapter-unsupported capability by the adapter's name", () => {
    const state: CapabilityState = { status: "unsupported", reason: "adapter", detail: "Cameras are not implemented." };
    expect(capabilityStateLabel(state, "moonraker")).toMatchObject({ text: "Not supported by Moonraker", severity: "neutral" });
    expect(capabilityStateLabel(state, "octoprint")).toMatchObject({ text: "Not supported by OctoPrint" });
  });

  it("labels an adapter-unsupported capability as 'No Connection' when there is no adapter", () => {
    const state: CapabilityState = { status: "unsupported", reason: "adapter", detail: "No Connection is configured." };
    expect(capabilityStateLabel(state, null)).toEqual({ text: "No Connection", severity: "neutral" });
  });

  it("falls back to capitalizing an unrecognized adapter kind", () => {
    const state: CapabilityState = { status: "unsupported", reason: "adapter", detail: "x" };
    expect(capabilityStateLabel(state, "prusalink")).toMatchObject({ text: "Not supported by Prusalink" });
  });

  it("labels a not-yet-verified capability", () => {
    const state: CapabilityState = { status: "unsupported", reason: "notVerified", detail: "Awaiting live evidence." };
    expect(capabilityStateLabel(state, "octoprint")).toEqual({ text: "Not verified yet", severity: "neutral" });
  });

  it("labels a host-unsupported capability with its detail", () => {
    const state: CapabilityState = { status: "unsupported", reason: "host", detail: "This printer has no camera." };
    expect(capabilityStateLabel(state, "moonraker")).toEqual({
      text: "Not available on this printer", severity: "neutral", detail: "This printer has no camera.",
    });
  });
});

describe("evidenceTierLabel", () => {
  it("labels sim and read-only-hardware evidence", () => {
    expect(evidenceTierLabel("sim")).toBe("Simulator");
    expect(evidenceTierLabel("readOnlyHardware")).toBe("Read-only hardware");
  });
});

it("never lets an unsupported capability and a failed Host Operation share copy or severity", () => {
  const failureTexts = new Set(
    (["neverSent", "hostUnreachable", "authRejected", "checksumRejected", "fileLoaded", "hostBusy",
      "fileMissing", "hostRejected", "hostNotReady", "notApplied", "hostFileDiffers"] as HostOperationFailureCode[])
      .map(hostOperationFailureText),
  );
  const unsupportedCases: CapabilityState[] = [
    { status: "unsupported", reason: "adapter", detail: "x" },
    { status: "unsupported", reason: "notVerified", detail: "x" },
    { status: "unsupported", reason: "host", detail: "x" },
  ];
  for (const state of unsupportedCases) {
    const label = capabilityStateLabel(state, "moonraker");
    expect(failureTexts.has(label.text)).toBe(false);
    expect(label.severity).not.toBe("error");
  }
  expect(hostOperationLabel({ kind: "upload", state: "failed", resolution: null }).severity).toBe("error");
});

describe("capabilityName", () => {
  it("names every CapabilityKey", async () => {
    const { capabilityName, CAPABILITY_KEYS } = await import("./presentation");
    expect(CAPABILITY_KEYS.map(capabilityName)).toEqual([
      "Upload", "Start", "Pause", "Resume", "Cancel", "Read print state", "Verify staged files", "Camera",
    ]);
  });
});

describe("capabilityRefusalText", () => {
  it("names the capability and why it is unavailable, with a host detail", async () => {
    const { capabilityRefusalText } = await import("./presentation");
    expect(capabilityRefusalText("upload", { status: "unsupported", reason: "notVerified", detail: "x" }, "octoprint"))
      .toBe("Upload: Not verified yet");
    expect(capabilityRefusalText("start", { status: "unsupported", reason: "host", detail: "This printer's Moonraker keeps no job history, so farm3d can't confirm a start." }, "moonraker"))
      .toBe("Start: Not available on this printer. This printer's Moonraker keeps no job history, so farm3d can't confirm a start.");
    expect(capabilityRefusalText("upload", { status: "unsupported", reason: "adapter", detail: "No Connection" }, null))
      .toBe("Upload: No Connection");
  });
});
