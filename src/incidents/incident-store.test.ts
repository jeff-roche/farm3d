import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { incident } from "../attention/test-records";
import type { IncidentDetail } from "../attention/types";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const attentionStoreMock = vi.hoisted(() => ({
  handler: undefined as undefined | ((incident: ReturnType<typeof import("../attention/test-records").incident>) => void),
  stop: vi.fn(),
}));
vi.mock("../attention/attention-store", () => ({
  onAttentionIncidentChanged: (listener: typeof attentionStoreMock.handler) => {
    attentionStoreMock.handler = listener;
    return attentionStoreMock.stop;
  },
}));

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  attentionStoreMock.handler = undefined;
  attentionStoreMock.stop.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

function commandError(code: string, message: string) {
  return { contractVersion: 1, code, message, recovery: [], retryable: false };
}

function detailFor(id: string): IncidentDetail {
  return { incident: incident({ id }), timeline: [], events: [], snapshots: [] };
}

async function flush(): Promise<void> {
  for (let i = 0; i < 10; i += 1) await Promise.resolve();
}

describe("incident-store (desktop)", () => {
  beforeEach(() => {
    tauriMock.isTauri.mockReturnValue(true);
  });

  it("listIncidents calls list_incidents with the given options", async () => {
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { incidents: [incident()], nextCursor: null } });
    const { listIncidents } = await import("./incident-store");

    const page = await listIncidents({ state: "open", printerId: "prn-1" });

    expect(page.incidents).toEqual([incident()]);
    expect(tauriMock.invoke).toHaveBeenCalledWith("list_incidents", { contractVersion: 1, state: "open", printerId: "prn-1" });
  });

  it("getIncident calls get_incident and returns the detail", async () => {
    const detail = detailFor("inc-1");
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: detail });
    const { getIncident } = await import("./incident-store");

    await expect(getIncident("inc-1")).resolves.toEqual(detail);
    expect(tauriMock.invoke).toHaveBeenCalledWith("get_incident", { contractVersion: 1, incidentId: "inc-1" });
  });

  it("addIncidentNote sends a fresh operationId and returns the updated detail", async () => {
    const detail = detailFor("inc-1");
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: detail });
    const { addIncidentNote } = await import("./incident-store");

    await expect(addIncidentNote("inc-1", "Checked the bed.")).resolves.toEqual(detail);
    expect(tauriMock.invoke).toHaveBeenCalledWith(
      "add_incident_note",
      expect.objectContaining({ incidentId: "inc-1", text: "Checked the bed.", operationId: expect.any(String) }),
    );
  });

  it("propagates a CommandError from get_incident unchanged", async () => {
    const error = commandError("NOT_FOUND", "Incident inc-1 was not found.");
    tauriMock.invoke.mockRejectedValue(error);
    const { getIncident } = await import("./incident-store");

    await expect(getIncident("inc-1")).rejects.toEqual(error);
  });

  it("watchIncidentDetail refetches only for its own Incident id, on a live change", async () => {
    const refetched = detailFor("inc-1");
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: refetched });
    const { watchIncidentDetail } = await import("./incident-store");

    const seen: IncidentDetail[] = [];
    const stop = watchIncidentDetail("inc-1", (detail) => seen.push(detail));
    expect(attentionStoreMock.handler).toBeDefined();

    attentionStoreMock.handler!(incident({ id: "inc-other" }));
    await flush();
    expect(seen).toEqual([]);

    attentionStoreMock.handler!(incident({ id: "inc-1" }));
    await flush();
    expect(seen).toEqual([refetched]);

    stop();
    expect(attentionStoreMock.stop).toHaveBeenCalled();
  });

  it("watchIncidentDetail reports a refetch failure to onError, not onChange", async () => {
    const error = commandError("NOT_FOUND", "gone");
    tauriMock.invoke.mockRejectedValue(error);
    const { watchIncidentDetail } = await import("./incident-store");

    const changes: IncidentDetail[] = [];
    const errors: unknown[] = [];
    watchIncidentDetail("inc-1", (detail) => changes.push(detail), (err) => errors.push(err));

    attentionStoreMock.handler!(incident({ id: "inc-1" }));
    await flush();
    expect(changes).toEqual([]);
    expect(errors).toEqual([error]);
  });
});

describe("incident-store (web)", () => {
  beforeEach(() => {
    tauriMock.isTauri.mockReturnValue(false);
  });

  it("listIncidents and getIncident are served from web-fixtures.ts, not a command", async () => {
    const { listIncidents, getIncident } = await import("./incident-store");
    const { webIncidentPage } = await import("../attention/web-fixtures");

    const page = await listIncidents();
    expect(page).toEqual(webIncidentPage());
    expect(tauriMock.invoke).not.toHaveBeenCalled();

    const [fixtureIncident] = page.incidents;
    const detail = await getIncident(fixtureIncident.id);
    expect(detail.incident.id).toBe(fixtureIncident.id);
  });

  it("getIncident rejects NOT_FOUND for an unknown id", async () => {
    const { getIncident } = await import("./incident-store");
    await expect(getIncident("inc-does-not-exist")).rejects.toMatchObject({ code: "NOT_FOUND" });
  });

  it("addIncidentNote refuses with needsDesktopError", async () => {
    const { addIncidentNote } = await import("./incident-store");
    await expect(addIncidentNote("inc-1", "note")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
  });
});
