import { describe, expect, it } from "vitest";
import { attentionEvent } from "./test-records";
import { eventTarget, incidentTarget, openTargetFor, sourceTarget, targetForSource } from "./deep-link";

// Pins `src-tauri/src/attention/deep_link.rs`'s table exactly:
//   Printer                                   -> monitor/printer/<id>
//   Job, or a Reconciliation Requirement's Job -> queue/job/<id>
//   Spool                                      -> spools/spool/<id>
//   (source gone)                              -> monitor/attention/<eventId>

describe("sourceTarget", () => {
  it("a Printer source maps to monitor/printer/<id>", () => {
    const event = attentionEvent({ source: { kind: "printer", id: "prn-7" } });
    expect(sourceTarget(event)).toEqual({
      version: 1,
      destination: "monitor",
      selection: { kind: "printer", id: "prn-7" },
    });
  });

  it("a Job source maps to queue/job/<id>", () => {
    const event = attentionEvent({ source: { kind: "job", id: "job-9" } });
    expect(sourceTarget(event)).toEqual({
      version: 1,
      destination: "queue",
      selection: { kind: "job", id: "job-9" },
    });
  });

  it("a Reconciliation Requirement source maps to its Job's queue/job/<id>", () => {
    const event = attentionEvent({ source: { kind: "reconciliationRequirement", id: "rrq-1" }, jobId: "job-5" });
    expect(sourceTarget(event)).toEqual({
      version: 1,
      destination: "queue",
      selection: { kind: "job", id: "job-5" },
    });
  });

  it("a Reconciliation Requirement with no Job (never in practice) falls back to the Event", () => {
    const event = attentionEvent({ id: "atn-3", source: { kind: "reconciliationRequirement", id: "rrq-2" }, jobId: null });
    expect(sourceTarget(event)).toEqual({
      version: 1,
      destination: "monitor",
      selection: { kind: "attention", id: "atn-3" },
    });
  });

  it("a Spool source maps to spools/spool/<id>", () => {
    const event = attentionEvent({ source: { kind: "spool", id: "spl-4" } });
    expect(sourceTarget(event)).toEqual({
      version: 1,
      destination: "spools",
      selection: { kind: "spool", id: "spl-4" },
    });
  });
});

describe("eventTarget", () => {
  it("is monitor/attention/<eventId>", () => {
    expect(eventTarget(attentionEvent({ id: "atn-42" }))).toEqual({
      version: 1,
      destination: "monitor",
      selection: { kind: "attention", id: "atn-42" },
    });
  });
});

describe("incidentTarget", () => {
  it("is monitor/incident/<id>", () => {
    expect(incidentTarget("inc-1")).toEqual({
      version: 1,
      destination: "monitor",
      selection: { kind: "incident", id: "inc-1" },
    });
  });
});

describe("targetForSource", () => {
  it("opens the source when it exists", () => {
    const event = attentionEvent({ source: { kind: "printer", id: "prn-1" } });
    expect(targetForSource(event, true)).toEqual(sourceTarget(event));
  });

  it("falls back to the Event when the source no longer exists", () => {
    const event = attentionEvent({ id: "atn-1", source: { kind: "printer", id: "prn-1" } });
    expect(targetForSource(event, false)).toEqual(eventTarget(event));
  });
});

describe("openTargetFor", () => {
  it("opens a Printer whose id is available", () => {
    const event = attentionEvent({ source: { kind: "printer", id: "prn-1" } });
    expect(openTargetFor(event, ["prn-1"])).toEqual(sourceTarget(event));
  });

  it("an archived Printer -- still present, just archived -- is still a valid target", () => {
    // Deep-link.ts has no notion of "archived": it only checks membership
    // in `availableIds`, which `App.tsx` populates from every known
    // Printer regardless of archive state (spec: "archived Printers stay
    // valid targets").
    const event = attentionEvent({ source: { kind: "printer", id: "prn-archived" } });
    expect(openTargetFor(event, ["prn-archived"])).toEqual(sourceTarget(event));
  });

  it("falls back to monitor/attention/<id> when the source id is missing", () => {
    const event = attentionEvent({ id: "atn-7", source: { kind: "printer", id: "prn-gone" } });
    expect(openTargetFor(event, [])).toEqual(eventTarget(event));
  });

  it("a Job source falls back to the Event when the Job id is missing", () => {
    const event = attentionEvent({ id: "atn-8", source: { kind: "job", id: "job-gone" } });
    expect(openTargetFor(event, ["some-other-id"])).toEqual(eventTarget(event));
  });

  it("a Reconciliation Requirement falls back to the Event when its Job id is missing", () => {
    const event = attentionEvent({
      id: "atn-9",
      source: { kind: "reconciliationRequirement", id: "rrq-1" },
      jobId: "job-gone",
    });
    expect(openTargetFor(event, [])).toEqual(eventTarget(event));
  });

  it("a Spool source falls back to the Event when the Spool id is missing", () => {
    const event = attentionEvent({ id: "atn-10", source: { kind: "spool", id: "spl-gone" } });
    expect(openTargetFor(event, [])).toEqual(eventTarget(event));
  });
});
