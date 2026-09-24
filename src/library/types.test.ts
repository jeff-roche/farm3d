import { describe, expect, it } from "vitest";
import { isLibraryEvent } from "./types";

function envelope(type: string, overrides: Record<string, unknown> = {}) {
  return {
    contractVersion: 1,
    streamId: "stream-library",
    sequence: 1,
    eventId: "evt-1",
    occurredAt: "2026-09-24T00:00:00Z",
    type,
    subject: { kind: "model", id: "mdl-1" },
    payload: {},
    ...overrides,
  };
}

describe("isLibraryEvent", () => {
  it.each([
    "library.project.changed",
    "library.project.removed",
    "library.model.changed",
    "library.model.removed",
    "library.revision.created",
    "library.selection.dropped",
    "library.import.progress",
  ])("accepts %s", (type) => {
    expect(isLibraryEvent(envelope(type))).toBe(true);
  });

  it.each([
    "printer.status.changed",
    "printer.status.removed",
    "printer.slots.changed",
    "spool.changed",
    "spool.availability.changed",
    "libraryish.model.changed",
  ])("rejects %s, which shares the channel but not the library. prefix", (type) => {
    expect(isLibraryEvent(envelope(type))).toBe(false);
  });

  it("rejects another contract version and non-envelopes", () => {
    expect(isLibraryEvent(envelope("library.model.changed", { contractVersion: 2 }))).toBe(false);
    expect(isLibraryEvent(null)).toBe(false);
    expect(isLibraryEvent("library.model.changed")).toBe(false);
    expect(isLibraryEvent({ type: 7 })).toBe(false);
  });
});
