import { describe, expect, it } from "vitest";
import {
  buildWebAttentionFixture,
  webIncidentDetail,
  webIncidentPage,
  webMediaUsage,
  webSnapshotImageDataUrl,
  webSnapshotPage,
  WEB_INCIDENT_SNAPSHOT_ID,
  WEB_PRUNED_SNAPSHOT_ID,
} from "./web-fixtures";
import type { ConditionKind } from "./types";

const ALL_CONDITION_KINDS: ConditionKind[] = [
  "printer.offline",
  "printer.connectionError",
  "printer.hostFailed",
  "job.startConfirmation",
  "job.failed",
  "job.hostCancelled",
  "requirement.materialReconciliation",
  "requirement.jobOutcomeUnknown",
  "spool.low",
  "job.completed",
];

describe("buildWebAttentionFixture", () => {
  it("covers every ConditionKind among the open Events", () => {
    const fixture = buildWebAttentionFixture();
    const seen = new Set(fixture.backfill.open.map((event) => event.condition));
    for (const kind of ALL_CONDITION_KINDS) expect(seen.has(kind)).toBe(true);
    expect(seen.size).toBe(ALL_CONDITION_KINDS.length);
  });

  it("has one recurrence chain: an open Event whose recurrenceOf names a resolved Event with the same dedupKey", () => {
    const fixture = buildWebAttentionFixture();
    const recurring = fixture.backfill.open.filter((event) => event.recurrenceOf !== null);
    expect(recurring).toHaveLength(1);
    const [event] = recurring;
    const prior = fixture.backfill.resolved.find((candidate) => candidate.id === event.recurrenceOf);
    expect(prior).toBeDefined();
    expect(prior?.dedupKey).toBe(event.dedupKey);
    expect(prior?.resolvedAt).not.toBeNull();
  });

  it("has exactly one open Incident, linked to a fatal open Event", () => {
    const fixture = buildWebAttentionFixture();
    expect(fixture.backfill.openIncidents).toHaveLength(1);
    const [incident] = fixture.backfill.openIncidents;
    expect(incident.state).toBe("open");
    const linkedEvent = fixture.backfill.open.find((event) => event.incidentId === incident.id);
    expect(linkedEvent).toBeDefined();
  });

  it("gives the open Incident a snapshot whose image is a generated PNG data URL", () => {
    const fixture = buildWebAttentionFixture();
    const [incident] = fixture.backfill.openIncidents;
    const snapshot = fixture.snapshots.find((candidate) => candidate.incidentId === incident.id);
    expect(snapshot).toBeDefined();
    expect(snapshot?.id).toBe(WEB_INCIDENT_SNAPSHOT_ID);
    expect(snapshot?.prunedAt).toBeNull();
    const url = webSnapshotImageDataUrl(snapshot!.id);
    expect(url).toMatch(/^data:image\/png;base64,/);
  });

  it("has exactly one pruned snapshot, with no image", () => {
    const fixture = buildWebAttentionFixture();
    const pruned = fixture.snapshots.filter((snapshot) => snapshot.prunedAt !== null);
    expect(pruned).toHaveLength(1);
    expect(pruned[0].id).toBe(WEB_PRUNED_SNAPSHOT_ID);
    expect(webSnapshotImageDataUrl(pruned[0].id)).toBeUndefined();
  });

  it("is deterministic across calls", () => {
    expect(buildWebAttentionFixture()).toEqual(buildWebAttentionFixture());
  });
});

describe("webIncidentPage / webIncidentDetail", () => {
  it("lists the one open Incident and filters it out when asking for closed", () => {
    expect(webIncidentPage().incidents).toHaveLength(1);
    expect(webIncidentPage({ state: "open" }).incidents).toHaveLength(1);
    expect(webIncidentPage({ state: "closed" }).incidents).toHaveLength(0);
  });

  it("returns the Incident's detail, including its linked Event and snapshot", () => {
    const [incident] = webIncidentPage().incidents;
    const detail = webIncidentDetail(incident.id);
    expect(detail?.incident.id).toBe(incident.id);
    expect(detail?.events).toHaveLength(1);
    expect(detail?.snapshots).toHaveLength(1);
  });

  it("returns undefined for an unknown Incident id", () => {
    expect(webIncidentDetail("inc-does-not-exist")).toBeUndefined();
  });
});

describe("webSnapshotPage / webMediaUsage", () => {
  it("filters by printerId and includePruned", () => {
    const [incident] = webIncidentPage().incidents;
    const unprunedOnly = webSnapshotPage({ printerId: incident.printerId, includePruned: false });
    expect(unprunedOnly.snapshots.every((s) => s.prunedAt === null)).toBe(true);
    const everything = webSnapshotPage({ printerId: incident.printerId });
    expect(everything.snapshots.length).toBeGreaterThan(unprunedOnly.snapshots.length);
  });

  it("counts usage only over unpruned rows", () => {
    const usage = webMediaUsage();
    expect(usage.snapshotCount).toBe(2);
    expect(usage.prunedCount).toBe(1);
    expect(usage.usedBytes).toBeGreaterThan(0);
  });
});
