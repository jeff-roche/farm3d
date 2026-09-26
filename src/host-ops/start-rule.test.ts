import { describe, expect, it } from "vitest";
import { controlOffer, startOffer } from "./start-rule";
import { printerStatus } from "./test-records";
import type { OperationalState } from "./types";

const ALL_STATES: OperationalState[] = [
  "setupIncomplete", "error", "offline", "connecting", "unknown",
  "printing", "paused", "busy", "finished", "cancelled", "failed", "ready",
];

describe("startOffer", () => {
  it("offers Start from Ready with the bed-clear label", () => {
    expect(startOffer(printerStatus("ready"), false)).toEqual({
      offered: true, priorState: "ready", confirmLabel: "The bed is clear.",
    });
  });

  it("offers Start from Finished naming the prior state", () => {
    expect(startOffer(printerStatus("finished"), false)).toEqual({
      offered: true, priorState: "finished",
      confirmLabel: "The previous print finished. The bed is clear.",
    });
  });

  it("offers Start from Cancelled naming the prior state", () => {
    expect(startOffer(printerStatus("cancelled"), false)).toEqual({
      offered: true, priorState: "cancelled",
      confirmLabel: "The previous print was cancelled. The bed is clear.",
    });
  });

  it("never offers Start from Failed, however it is asked", () => {
    expect(startOffer(printerStatus("failed"), false)).toEqual({
      offered: false, reason: "Clear the error on the printer first.",
    });
  });

  it("does not offer Start from any other state, naming the state as the reason", () => {
    const others: OperationalState[] = [
      "printing", "paused", "busy", "offline", "connecting", "unknown", "error", "setupIncomplete",
    ];
    for (const state of others) {
      const offer = startOffer(printerStatus(state), false);
      expect(offer.offered, state).toBe(false);
    }
  });

  it("refuses stale or unavailable telemetry even from an otherwise-offered state", () => {
    for (const freshness of ["stale", "unavailable"] as const) {
      for (const state of ["ready", "finished", "cancelled"] as const) {
        expect(startOffer(printerStatus(state, freshness), false)).toEqual({
          offered: false, reason: "its status is out of date",
        });
      }
    }
  });

  it("is never offered while a Host Operation is unresolved, whatever the state", () => {
    for (const state of ALL_STATES) {
      expect(startOffer(printerStatus(state), true)).toEqual({
        offered: false, reason: "A printer operation is pending.",
      });
    }
  });

  it("agrees with every backend start_rule.rs row (fresh telemetry)", () => {
    const expected: Record<OperationalState, boolean> = {
      ready: true, finished: true, cancelled: true,
      failed: false, printing: false, paused: false, busy: false,
      offline: false, connecting: false, unknown: false, error: false, setupIncomplete: false,
    };
    for (const state of ALL_STATES) {
      expect(startOffer(printerStatus(state), false).offered, state).toBe(expected[state]);
    }
  });
});

describe("controlOffer", () => {
  it("offers pause only from printing", () => {
    for (const state of ALL_STATES) {
      expect(controlOffer(printerStatus(state), "pause", false).offered, state).toBe(state === "printing");
    }
  });

  it("offers resume only from paused", () => {
    for (const state of ALL_STATES) {
      expect(controlOffer(printerStatus(state), "resume", false).offered, state).toBe(state === "paused");
    }
  });

  it("offers cancel from printing or paused", () => {
    for (const state of ALL_STATES) {
      expect(controlOffer(printerStatus(state), "cancel", false).offered).toBe(state === "printing" || state === "paused");
    }
  });

  it("needs fresh telemetry", () => {
    for (const verb of ["pause", "resume", "cancel"] as const) {
      for (const state of ["printing", "paused"] as const) {
        const offer = controlOffer(printerStatus(state, "stale"), verb, false);
        expect(offer).toEqual({ offered: false, reason: "its status is out of date" });
      }
    }
  });

  it("is never offered while a Host Operation is unresolved, with the shared reason", () => {
    for (const verb of ["pause", "resume", "cancel"] as const) {
      expect(controlOffer(printerStatus("printing"), verb, true)).toEqual({
        offered: false,
        reason: "A printer operation is pending. You can still pause or cancel on the printer itself.",
      });
    }
  });

  it("names the observed state as the reason when the verb doesn't apply", () => {
    expect(controlOffer(printerStatus("ready"), "pause", false)).toEqual({
      offered: false, reason: "it is ready",
    });
    expect(controlOffer(printerStatus("printing"), "resume", false)).toEqual({
      offered: false, reason: "it is printing",
    });
  });
});

describe("offer reason copy", () => {
  it("startRefusalText reads a state label as the backend's START_NOT_ALLOWED message, and keeps the fixed sentences", async () => {
    const { startRefusalText } = await import("./start-rule");
    expect(startRefusalText("it is printing")).toBe("The printer can't start a print now: it is printing.");
    expect(startRefusalText("Clear the error on the printer first.")).toBe("Clear the error on the printer first.");
    expect(startRefusalText("A printer operation is pending.")).toBe("A printer operation is pending.");
  });

  it("controlRefusalText reads a state label as the backend's CONTROL_NOT_ALLOWED message", async () => {
    const { controlRefusalText } = await import("./start-rule");
    expect(controlRefusalText("pause", "it is ready")).toBe("The printer isn't in a state to pause now: it is ready.");
    expect(controlRefusalText("cancel", "A printer operation is pending. You can still pause or cancel on the printer itself."))
      .toBe("A printer operation is pending. You can still pause or cancel on the printer itself.");
  });
});

describe("statusOrUnknown", () => {
  it("refuses Start and controls for a Printer with no live status", async () => {
    const { statusOrUnknown } = await import("./start-rule");
    expect(startOffer(statusOrUnknown(undefined), false)).toEqual({ offered: false, reason: "its state is unknown" });
    expect(controlOffer(statusOrUnknown(undefined), "pause", false)).toEqual({ offered: false, reason: "its state is unknown" });
    const live = printerStatus("ready");
    expect(statusOrUnknown(live)).toBe(live);
  });
});
