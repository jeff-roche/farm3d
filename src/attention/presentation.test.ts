import { describe, expect, it } from "vitest";
import { attentionEvent } from "./test-records";
import {
  attentionSeverityLabel,
  evidenceSkipReasonLabel,
  matchesAttentionFilter,
  matchesSeverityFilter,
  snapshotTriggerLabel,
} from "./presentation";
import type { AttentionFilter, AttentionSeverityFilter } from "./presentation";
import type { EvidenceSkipReason } from "./types";

describe("attentionSeverityLabel", () => {
  it("is the plain word, per spec ('Fatal', 'Warning', 'Info')", () => {
    expect(attentionSeverityLabel("fatal")).toBe("Fatal");
    expect(attentionSeverityLabel("warning")).toBe("Warning");
    expect(attentionSeverityLabel("info")).toBe("Info");
  });
});

describe("matchesAttentionFilter", () => {
  const open = attentionEvent({ resolvedAt: null });
  const resolved = attentionEvent({ resolvedAt: "2026-09-25T00:00:00Z", resolution: "conditionCleared" });

  it("actionable: open and requiresAction, whether acknowledged or not", () => {
    expect(matchesAttentionFilter("actionable", { ...open, requiresAction: true, acknowledgedAt: null })).toBe(true);
    expect(matchesAttentionFilter("actionable", { ...open, requiresAction: true, acknowledgedAt: "2026-09-25T00:00:00Z" })).toBe(true);
    expect(matchesAttentionFilter("actionable", { ...open, requiresAction: false })).toBe(false);
    expect(matchesAttentionFilter("actionable", { ...resolved, requiresAction: true })).toBe(false);
  });

  it("unread: open and readAt null", () => {
    expect(matchesAttentionFilter("unread", { ...open, readAt: null })).toBe(true);
    expect(matchesAttentionFilter("unread", { ...open, readAt: "2026-09-25T00:00:00Z" })).toBe(false);
    expect(matchesAttentionFilter("unread", { ...resolved, readAt: null })).toBe(false);
  });

  it("allOpen: resolvedAt null, regardless of anything else", () => {
    expect(matchesAttentionFilter("allOpen", open)).toBe(true);
    expect(matchesAttentionFilter("allOpen", resolved)).toBe(false);
  });

  it("resolved: resolvedAt set", () => {
    expect(matchesAttentionFilter("resolved", resolved)).toBe(true);
    expect(matchesAttentionFilter("resolved", open)).toBe(false);
  });

  it.each<AttentionFilter>(["actionable", "unread", "allOpen", "resolved"])("%s is a total function over an arbitrary Event", (filter) => {
    expect(() => matchesAttentionFilter(filter, open)).not.toThrow();
  });
});

describe("matchesSeverityFilter", () => {
  it("'all' matches every severity", () => {
    for (const severity of ["fatal", "warning", "info"] as const) {
      expect(matchesSeverityFilter("all", attentionEvent({ severity }))).toBe(true);
    }
  });

  it("a specific severity matches only itself", () => {
    expect(matchesSeverityFilter("fatal", attentionEvent({ severity: "fatal" }))).toBe(true);
    expect(matchesSeverityFilter("fatal", attentionEvent({ severity: "warning" }))).toBe(false);
  });

  it.each<AttentionSeverityFilter>(["all", "fatal", "warning", "info"])("%s is a total function", (filter) => {
    expect(() => matchesSeverityFilter(filter, attentionEvent())).not.toThrow();
  });
});

describe("evidenceSkipReasonLabel", () => {
  it.each<EvidenceSkipReason>(["cameraError", "diskCap", "storage"])("labels %s", (reason) => {
    expect(evidenceSkipReasonLabel(reason)).toBeTruthy();
  });
});

describe("snapshotTriggerLabel", () => {
  it("is the plain word ('Incident', 'Completion', 'Manual')", () => {
    expect(snapshotTriggerLabel("incident")).toBe("Incident");
    expect(snapshotTriggerLabel("completion")).toBe("Completion");
    expect(snapshotTriggerLabel("manual")).toBe("Manual");
  });
});
