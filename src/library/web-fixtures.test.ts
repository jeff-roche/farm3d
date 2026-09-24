import { describe, expect, it } from "vitest";
import { modelsFor } from "./saved-views";
import { buildWebLibraryFixture } from "./web-fixtures";

const NOW = new Date("2026-09-24T12:00:00Z");

describe("buildWebLibraryFixture", () => {
  const fixture = buildWebLibraryFixture(NOW);
  const byName = (name: string) => fixture.projects.find((p) => p.name === name)!;

  it("has the two Projects Brackets and Calibration, with modelCount equal to their memberships", () => {
    expect(fixture.projects.map((p) => p.name).sort()).toEqual(["Brackets", "Calibration"]);
    for (const p of fixture.projects) {
      expect(p.modelCount, p.name).toBe(fixture.models.filter((m) => m.projectIds.includes(p.id)).length);
    }
  });

  it("has five Models covering the spec's cases", () => {
    const { models } = fixture;
    expect(models).toHaveLength(5);
    expect(new Set(models.map((m) => m.id)).size).toBe(5);
    const brackets = byName("Brackets").id;
    const calibration = byName("Calibration").id;
    expect(models.some((m) => m.format === "stl" && m.storageMode === "managed"
      && m.projectIds.includes(brackets) && m.projectIds.includes(calibration))).toBe(true);
    expect(models.some((m) => m.format === "3mf" && m.storageMode === "linked" && m.link?.state === "ok")).toBe(true);
    expect(models.some((m) => m.format === "stl" && m.storageMode === "linked" && m.link?.state === "missing")).toBe(true);
    expect(models.some((m) => m.format === "gcode")).toBe(true);
    expect(models.filter((m) => m.projectIds.length === 0)).toHaveLength(1);
  });

  it("keeps every Model's projectIds ordered by Project name, as the backend does", () => {
    const nameOf = (id: string) => fixture.projects.find((p) => p.id === id)!.name;
    for (const m of fixture.models) {
      const names = m.projectIds.map(nameOf);
      expect(names).toEqual([...names].sort((a, b) => a.localeCompare(b)));
    }
  });

  it("puts at least one Model in Recently added relative to the given time", () => {
    expect(modelsFor({ kind: "view", id: "recent" }, fixture.models, NOW).length).toBeGreaterThan(0);
  });

  it("has a revision history whose newest entry is each Model's current revision", () => {
    for (const m of fixture.models) {
      const history = fixture.revisions[m.id];
      expect(history, m.id).toHaveLength(m.revisionCount);
      expect(history[0].id).toBe(m.currentRevision.id);
      expect(history[0].sequence).toBe(m.currentRevision.sequence);
      expect(history[0].inspection.format).toBe(m.format);
    }
  });

  it("gives the G-code Model verbatim claims and the 3MF its unsupported entries", () => {
    const gcode = fixture.models.find((m) => m.format === "gcode")!;
    const gcodeInspection = fixture.revisions[gcode.id][0].inspection;
    expect(gcodeInspection.format === "gcode" && gcodeInspection.claims.length).toBeGreaterThan(0);
    const threeMf = fixture.models.find((m) => m.format === "3mf")!;
    const threeMfInspection = fixture.revisions[threeMf.id][0].inspection;
    expect(threeMfInspection.format === "3mf" && threeMfInspection.unsupported.length).toBeGreaterThan(0);
  });

  it("embeds a 1x1 thumbnail for every current revision that has one", () => {
    const withThumbnails = fixture.models.filter((m) => m.currentRevision.hasThumbnail);
    expect(withThumbnails.length).toBeGreaterThan(0);
    for (const m of withThumbnails) {
      const thumbnail = fixture.thumbnails[m.currentRevision.id];
      expect(thumbnail).toMatchObject({ mediaType: "image/png", width: 1, height: 1 });
      expect(thumbnail.dataBase64.length).toBeGreaterThan(0);
    }
  });

  it("is deterministic for a given time and returns fresh objects each call", () => {
    const again = buildWebLibraryFixture(NOW);
    expect(again).toEqual(fixture);
    expect(again.models).not.toBe(fixture.models);
  });
});
