import { describe, expect, it } from "vitest";
import {
  blockerCodeLabel,
  cancelReasonLabel,
  closeReasonLabel,
  copyLabel,
  dispatchPolicyLabel,
  dispatchPreferenceLabel,
  estimateSourceLabel,
  queueViewLabel,
  jobEventKindLabel,
  jobStateLabel,
  type NewRecoveryCode,
  recoveryCodeLabel,
  requirementKindLabel,
  settlementLabel,
  startBlockerLabel,
} from "./presentation";
import type { BlockerCode, CancelReason, JobEventKind, JobState, RequirementKind, Settlement } from "./types";

const JOB_STATES: JobState[] = [
  "assigned", "staging", "awaitingStart", "starting", "printing", "paused",
  "completed", "failed", "cancelled", "outcomeUnknown",
];

const CANCEL_REASONS: CancelReason[] = [
  "releasedBeforeStart", "cancelledBeforeStart", "cancelledByOperator", "hostCancelled", "operatorDeclared",
];

const SETTLEMENTS: Settlement[] = ["open", "notRequired", "pending", "deferred", "settled"];

const BLOCKER_CODES: BlockerCode[] = [
  "PRINTER_ARCHIVED", "SETUP_INCOMPLETE", "CONNECTION_ERROR", "PRINTER_OFFLINE", "JOB_ACTIVE",
  "HOST_OPERATION_PENDING", "PRINTER_BUSY_EXTERNAL", "PRINTER_NOT_IDLE", "PINNED_TO_OTHER_PRINTER",
  "PROFILE_MISMATCH", "NEEDS_MANUAL_PRINTER", "CAPABILITY_UNSUPPORTED", "ADAPTER_NOT_PROVEN",
  "NO_COMPATIBLE_SPOOL", "INSUFFICIENT_MATERIAL", "SPOOL_NOT_LOADED", "PRINTER_NOT_READY",
];

const REQUIREMENT_KINDS: RequirementKind[] = ["materialReconciliation", "jobOutcomeUnknown"];

const JOB_EVENT_KINDS: JobEventKind[] = [
  "assigned", "stageHandedOff", "stageSucceeded", "stageFailed", "startHandedOff", "startSucceeded",
  "startFailed", "startAbandoned", "hostJobPinned", "pauseHandedOff", "resumeHandedOff", "cancelHandedOff",
  "controlFailed", "paused", "resumed", "completed", "failed", "cancelled", "outcomeUnknown",
  "declaredCompleted", "declaredFailed", "declaredCancelled", "released", "cancelledBeforeStart",
  "materialSettled", "materialDeferred", "materialCorrected",
];

const NEW_RECOVERY_CODES: NewRecoveryCode[] = [
  "OPEN_JOB", "OPEN_PRINTER_SETUP", "UNARCHIVE_PRINTER", "LOAD_SPOOL", "ASSIGN_MANUALLY", "SETTLE_MATERIAL",
];

describe("jobStateLabel", () => {
  it("labels every JobState", () => {
    for (const state of JOB_STATES) {
      expect(jobStateLabel(state), state).toBeTruthy();
    }
  });

  it("outcomeUnknown reads 'Outcome unknown' (spec 'Frontend architecture')", () => {
    expect(jobStateLabel("outcomeUnknown")).toBe("Outcome unknown");
  });
});

describe("cancelReasonLabel", () => {
  it("labels every CancelReason", () => {
    for (const reason of CANCEL_REASONS) {
      expect(cancelReasonLabel(reason), reason).toBeTruthy();
    }
  });
});

describe("settlementLabel", () => {
  it("labels every Settlement", () => {
    for (const settlement of SETTLEMENTS) {
      expect(settlementLabel(settlement), settlement).toBeTruthy();
    }
  });
});

describe("blockerCodeLabel", () => {
  it("labels every BlockerCode", () => {
    for (const code of BLOCKER_CODES) {
      expect(blockerCodeLabel(code), code).toBeTruthy();
    }
  });

  it("SPOOL_NOT_LOADED reads 'Awaiting material'", () => {
    expect(blockerCodeLabel("SPOOL_NOT_LOADED")).toBe("Awaiting material");
  });
});

describe("startBlockerLabel", () => {
  it("labels a SPOOL_NOT_LOADED start blocker 'Awaiting material' regardless of Rust's own message (D3)", () => {
    const blocker = { code: "SPOOL_NOT_LOADED" as const, message: "Automatic assignment needs the Spool loaded on this Printer.", detail: null, recovery: "LOAD_SPOOL" as const, printerIds: ["prn-1"] };
    expect(startBlockerLabel(blocker)).toBe("Awaiting material");
  });

  it("shows every other blocker's own Rust-supplied message verbatim", () => {
    const blocker = { code: "PRINTER_NOT_READY" as const, message: "The printer can't start now: printing.", detail: null, recovery: null, printerIds: ["prn-1"] };
    expect(startBlockerLabel(blocker)).toBe("The printer can't start now: printing.");
  });
});

describe("requirementKindLabel", () => {
  it("labels every RequirementKind", () => {
    for (const kind of REQUIREMENT_KINDS) {
      expect(requirementKindLabel(kind), kind).toBeTruthy();
    }
  });
});

describe("jobEventKindLabel", () => {
  it("labels every JobEventKind", () => {
    for (const kind of JOB_EVENT_KINDS) {
      expect(jobEventKindLabel(kind), kind).toBeTruthy();
    }
  });
});

describe("recoveryCodeLabel", () => {
  it("labels every new RecoveryCode this spec adds", () => {
    for (const code of NEW_RECOVERY_CODES) {
      expect(recoveryCodeLabel(code), code).toBeTruthy();
    }
  });

  it("OPEN_JOB has a label (a later task wires its button)", () => {
    expect(recoveryCodeLabel("OPEN_JOB")).toBeTruthy();
  });
});

describe("Queue screen labels", () => {
  it("names every Dispatch Policy, preference, close reason, estimate source, and view", () => {
    expect(["manual", "recommended", "automatic"].map((p) => dispatchPolicyLabel(p as never)))
      .toEqual(["Manual", "Recommended", "Automatic"]);
    expect(dispatchPreferenceLabel("loadedFirst")).toBe("Loaded Spool first");
    expect(dispatchPreferenceLabel("leastRecentlyUsed")).toBe("Least recently used");
    expect(["completed", "failed", "cancelled", "released", "removed"].map((r) => closeReasonLabel(r as never)))
      .toEqual(["Completed", "Failed", "Cancelled", "Released", "Removed"]);
    expect(estimateSourceLabel("sliceEstimate")).toBe("slice estimate");
    expect(estimateSourceLabel("fileClaimConfirmed")).toBe("file's claim, confirmed");
    expect(estimateSourceLabel("operatorEntered")).toBe("entered by hand");
    expect(queueViewLabel("printing")).toBe("Printing now");
    expect(queueViewLabel("awaitingOperator")).toBe("Awaiting operator");
  });

  it("reads a lineage copy as Copy n of m, from Rust's copyIndex and copyCount", () => {
    expect(copyLabel({ copyIndex: 2, copyCount: 3 })).toBe("Copy 2 of 3");
  });
});
