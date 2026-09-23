import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { SpoolRecord } from "../generated/contracts/domain/SpoolRecord";
import type { EventEnvelope } from "../generated/contracts/event/EventEnvelope";
import type { InventoryEventPayload } from "../generated/contracts/domain/InventoryEventPayload";
import type { InventoryEventType } from "../generated/contracts/domain/InventoryEventType";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const eventMock = vi.hoisted(() => ({ listen: vi.fn() }));
vi.mock("@tauri-apps/api/event", () => eventMock);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
  eventMock.listen.mockReset();
  tauriMock.isTauri.mockReturnValue(true);
  eventMock.listen.mockResolvedValue(vi.fn());
});

afterEach(() => {
  vi.restoreAllMocks();
});

type InventoryEnvelope = EventEnvelope<InventoryEventType, InventoryEventPayload>;

function spool(overrides: Partial<SpoolRecord> = {}): SpoolRecord {
  return {
    id: "spl-1",
    revision: 1,
    spoolNumber: 1,
    manufacturer: "Prusament",
    product: "PLA",
    materialFamily: "PLA",
    colorName: "Galaxy Black",
    diameter: "1.75",
    nominalMg: 1_000_000,
    lowThresholdMg: 100_000,
    lifecycle: "active",
    location: { kind: "storage", storageLabel: null },
    availability: { currentMg: 500_000, reservedMg: 0, availableMg: 500_000 },
    facets: { loaded: false, reserved: false, low: false, confidence: "measured" },
    createdAt: "2026-09-01T00:00:00Z",
    updatedAt: "2026-09-01T00:00:00Z",
    ...overrides,
  };
}

function envelope(
  sequence: number,
  payload: InventoryEventPayload,
  overrides: Partial<InventoryEnvelope> = {},
): InventoryEnvelope {
  return {
    contractVersion: 1,
    streamId: "stream-a",
    sequence,
    eventId: `evt-${sequence}`,
    occurredAt: "2026-09-23T00:00:00Z",
    type: payload.type === "spoolChanged" ? "spool.changed"
      : payload.type === "printerSlotsChanged" ? "printer.slots.changed"
      : "spool.availability.changed",
    subject: { kind: "spool", id: "spl-1" },
    payload,
    ...overrides,
  };
}

function snapshotResponse(sequence: number, spools: SpoolRecord[], tares: unknown[] = []) {
  return { contractVersion: 1, data: { streamId: "stream-a", snapshotSequence: sequence, spools, tares } };
}

/** Wires `eventMock.listen` to capture the registered handler and expose it
 *  synchronously (once the listener promise itself resolves). */
function captureListenerHandler(): { handler: (event: { payload: unknown }) => void } {
  const box: { handler: (event: { payload: unknown }) => void } = { handler: () => {} };
  eventMock.listen.mockImplementation((_name: string, cb: typeof box.handler) => {
    box.handler = cb;
    return Promise.resolve(vi.fn());
  });
  return box;
}

describe("loadInventory", () => {
  it("registers the event listener before backfilling through list_spools, and drops events at or below the snapshot sequence", async () => {
    const calls: string[] = [];
    const box = captureListenerHandler();
    eventMock.listen.mockImplementation((name: string, cb: typeof box.handler) => {
      calls.push(`listen:${name}`);
      box.handler = cb;
      return Promise.resolve(vi.fn());
    });
    tauriMock.invoke.mockImplementation((command: string) => {
      calls.push(`invoke:${command}`);
      return Promise.resolve(snapshotResponse(5, []));
    });

    const { loadInventory, spoolState } = await import("./spool-store");
    await loadInventory();

    expect(calls).toEqual(["listen:farm3d-event-v1", "invoke:list_spools"]);
    expect(eventMock.listen).toHaveBeenCalledWith("farm3d-event-v1", expect.any(Function));

    const staleSpool = spool();
    box.handler({ payload: envelope(5, { type: "spoolChanged", spool: staleSpool }) });
    expect(spoolState.spools).toEqual([]);

    const freshSpool = spool({ id: "spl-2" });
    box.handler({ payload: envelope(6, { type: "spoolChanged", spool: freshSpool }) });
    expect(spoolState.spools).toEqual([freshSpool]);
  });

  it("buffers events received while backfilling and replays only those above the snapshot sequence", async () => {
    const box = captureListenerHandler();
    let resolveBackfill!: (value: unknown) => void;
    tauriMock.invoke.mockImplementation(() => new Promise((resolve) => (resolveBackfill = resolve)));

    const { loadInventory, spoolState } = await import("./spool-store");
    const started = loadInventory();
    // `listen(...)` is only reached after an internal `await import(...)`,
    // which can take more than one microtask -- poll rather than guess.
    for (let i = 0; i < 20 && !resolveBackfill; i += 1) await Promise.resolve();

    const early = spool({ id: "spl-early" });
    const late = spool({ id: "spl-late" });
    box.handler({ payload: envelope(2, { type: "spoolChanged", spool: early }) });
    box.handler({ payload: envelope(4, { type: "spoolChanged", spool: late }) });
    resolveBackfill(snapshotResponse(2, []));
    await started;

    expect(spoolState.spools).toEqual([late]);
  });
});

const A_MINIMAL_PRINTER_RECORD = {
  id: "prn-1", revision: 5, name: "Bay 1", notes: "", overrides: {},
  catalogRef: { vendor: "V", model: "M", variant: "V0", modelId: "id", printerVariant: "0.4" },
  startSafety: "confirmBedClear" as const,
  materialSlots: [{ id: "slt-1", position: 0, name: "Main" }],
  setupGaps: [],
  profileResolution: {
    catalogStatus: "ok" as const, modelLabel: "M", variantLabel: "V0",
    profile: {
      bedShape: { kind: "rectangular" as const, widthMm: 200, depthMm: 200, originXMm: 0, originYMm: 0 },
      printableHeightMm: 200, bedExcludeAreas: [], defaultBedType: "0",
      nozzleDiameterMm: [0.4], nozzleType: "brass" as const, gcodeFlavor: "marlin" as const,
      hasAuxiliaryFan: false, supportsAirFiltration: false, supportsMultiFilament: false, suggestedHostType: null,
    },
    overriddenFields: [], inherited: {}, profileDrift: [], unknownOverrideKeys: [],
  },
  createdAt: "", updatedAt: "",
};

describe("moveSpool: optimistic settle", () => {
  it("applies the expected placement locally before the invoke resolves, then settles from the result and hands returned Printers to printer-store", async () => {
    const occupant = spool({
      id: "spl-occupant", spoolNumber: 2,
      location: { kind: "slot", slotId: "slt-1", printerId: "prn-1" },
      facets: { loaded: true, reserved: false, low: false, confidence: "measured" },
    });
    const moving = spool({ id: "spl-moving", spoolNumber: 3 });
    let resolveMove!: (value: unknown) => void;
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_printers") return Promise.resolve({ contractVersion: 1, data: [structuredClone(A_MINIMAL_PRINTER_RECORD)] });
      if (command === "list_spools") return Promise.resolve(snapshotResponse(0, [occupant, moving]));
      if (command === "move_spool") return new Promise((resolve) => { resolveMove = resolve; });
      throw new Error(`unexpected command ${command}`);
    });

    const { loadPrinters, printers } = await import("../printers/printer-store");
    await loadPrinters();
    const { loadInventory, moveSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    const movePromise = moveSpool({
      spoolId: moving.id,
      expectedSpoolRevision: moving.revision,
      destination: { kind: "slot", slotId: "slt-1", expectedOccupantSpoolId: occupant.id },
    });

    // Both Spools reflect the expected placement locally, before the invoke resolves.
    const movingNow = spoolState.spools.find((s) => s.id === moving.id)!;
    const occupantNow = spoolState.spools.find((s) => s.id === occupant.id)!;
    expect(movingNow.location).toEqual({ kind: "slot", slotId: "slt-1", printerId: "prn-1" });
    expect(occupantNow.location).toEqual({ kind: "storage", storageLabel: null });
    expect(spoolState.pending[Object.keys(spoolState.pending)[0]].sort()).toEqual([moving.id, occupant.id].sort());

    // Let the command resolve now, with the authoritative settled state.
    const settledMoving = { ...moving, revision: 2, location: { kind: "slot" as const, slotId: "slt-1", printerId: "prn-1" } };
    const settledOccupant = { ...occupant, revision: 2, location: { kind: "storage" as const, storageLabel: "Shelf X" } };
    const settledPrinter = {
      ...structuredClone(A_MINIMAL_PRINTER_RECORD),
      materialSlots: [{ id: "slt-1", position: 0, name: "Main", occupantSpoolId: moving.id }],
    };
    resolveMove({
      contractVersion: 1,
      data: { spools: [settledMoving, settledOccupant], printers: [settledPrinter], movements: [] },
    });
    const result = await movePromise;

    expect(result.spools).toEqual([settledMoving, settledOccupant]);
    expect(spoolState.spools.find((s) => s.id === moving.id)).toEqual(settledMoving);
    expect(printers().find((p) => p.id === "prn-1")?.materialSlots).toEqual(settledPrinter.materialSlots);
    expect(spoolState.pending).toEqual({});
  });

  it("keeps a newer spool.changed record instead of letting the move's own (older) result regress it", async () => {
    const box = captureListenerHandler();
    const moving = spool({ id: "spl-1", revision: 1 });
    let resolveMove!: (value: unknown) => void;
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") return Promise.resolve(snapshotResponse(0, [moving]));
      if (command === "move_spool") return new Promise((resolve) => { resolveMove = resolve; });
      throw new Error(`unexpected command ${command}`);
    });

    const { loadInventory, moveSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    const movePromise = moveSpool({
      spoolId: moving.id, expectedSpoolRevision: moving.revision,
      destination: { kind: "storage", storageLabel: "Shelf A2" },
    });

    // A concurrent external write races ahead of this move's own response,
    // arriving via the ordered stream at revision 3.
    const fresher = { ...moving, revision: 3, colorName: "Renamed elsewhere" };
    box.handler({ payload: envelope(1, { type: "spoolChanged", spool: fresher }) });
    expect(spoolState.spools.find((s) => s.id === moving.id)).toEqual(fresher);

    // The move's own result is only revision 2 -- older than what the
    // event already established -- and must not regress it.
    const settled = { ...moving, revision: 2, location: { kind: "storage" as const, storageLabel: "Shelf A2" } };
    resolveMove({ contractVersion: 1, data: { spools: [settled], printers: [], movements: [] } });
    await movePromise;

    expect(spoolState.spools.find((s) => s.id === moving.id)).toEqual(fresher);
  });
});

describe("moveSpool: restore on failure", () => {
  it("restores only the affected Spools on a non-CONFLICT error, leaving an unrelated Spool's mid-flight event applied", async () => {
    const box = captureListenerHandler();
    const occupant = spool({ id: "spl-occupant", location: { kind: "slot", slotId: "slt-1", printerId: "prn-1" } });
    const moving = spool({ id: "spl-moving" });
    const unrelated = spool({ id: "spl-unrelated", colorName: "Original" });
    let rejectMove!: (reason: unknown) => void;
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") return Promise.resolve(snapshotResponse(0, [occupant, moving, unrelated]));
      if (command === "move_spool") return new Promise((_resolve, reject) => { rejectMove = reject; });
      throw new Error(`unexpected command ${command}`);
    });

    const { loadInventory, moveSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    const movePromise = moveSpool({
      spoolId: moving.id,
      expectedSpoolRevision: moving.revision,
      destination: { kind: "slot", slotId: "slt-1", expectedOccupantSpoolId: occupant.id },
    });

    // Optimistic placement applied for both A and its occupant.
    expect(spoolState.spools.find((s) => s.id === moving.id)?.location).toEqual({ kind: "slot", slotId: "slt-1", printerId: "prn-1" });

    // An unrelated Spool's event lands while the move is still in flight.
    const unrelatedFresh = { ...unrelated, revision: 2, colorName: "Fresh" };
    box.handler({ payload: envelope(1, { type: "spoolChanged", spool: unrelatedFresh }) });

    rejectMove({ contractVersion: 1, code: "VALIDATION", message: "Bad request.", recovery: [], retryable: false });
    await expect(movePromise).rejects.toMatchObject({ code: "VALIDATION" });

    // The two affected Spools are restored to their pre-move state...
    expect(spoolState.spools.find((s) => s.id === moving.id)?.location).toEqual(moving.location);
    expect(spoolState.spools.find((s) => s.id === occupant.id)?.location).toEqual(occupant.location);
    // ...but the unrelated Spool's newer, mid-flight event survives.
    expect(spoolState.spools.find((s) => s.id === unrelated.id)).toEqual(unrelatedFresh);
    expect(spoolState.pending).toEqual({});
  });

  it("does not regress a different move's already-settled record when this move's own restore runs", async () => {
    const spoolA = spool({ id: "spl-a" });
    const spoolC = spool({ id: "spl-c" });
    let moveCallCount = 0;
    let rejectMoveA!: (reason: unknown) => void;
    let resolveMoveC!: (value: unknown) => void;
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") return Promise.resolve(snapshotResponse(0, [spoolA, spoolC]));
      if (command === "move_spool") {
        moveCallCount += 1;
        if (moveCallCount === 1) return new Promise((_resolve, reject) => { rejectMoveA = reject; });
        return new Promise((resolve) => { resolveMoveC = resolve; });
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { loadInventory, moveSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    const movePromiseA = moveSpool({
      spoolId: spoolA.id, expectedSpoolRevision: spoolA.revision,
      destination: { kind: "storage", storageLabel: "Shelf A2" },
    });
    const movePromiseC = moveSpool({
      spoolId: spoolC.id, expectedSpoolRevision: spoolC.revision,
      destination: { kind: "storage", storageLabel: "Shelf C2" },
    });

    // Move C settles first, bumping its revision.
    const settledC = { ...spoolC, revision: 2, location: { kind: "storage" as const, storageLabel: "Shelf C2" } };
    resolveMoveC({ contractVersion: 1, data: { spools: [settledC], printers: [], movements: [] } });
    await movePromiseC;
    expect(spoolState.spools.find((s) => s.id === spoolC.id)).toEqual(settledC);

    // Move A then fails (non-conflict) -- its restore must touch only A.
    rejectMoveA({ contractVersion: 1, code: "VALIDATION", message: "Bad request.", recovery: [], retryable: false });
    await expect(movePromiseA).rejects.toMatchObject({ code: "VALIDATION" });

    expect(spoolState.spools.find((s) => s.id === spoolA.id)?.location).toEqual(spoolA.location);
    expect(spoolState.spools.find((s) => s.id === spoolC.id)).toEqual(settledC);
  });
});

describe("moveSpool: CONFLICT recovery", () => {
  it("restores, refetches only the affected Spool ids through list_spools, and rethrows", async () => {
    const original = spool({ id: "spl-1", revision: 3, location: { kind: "storage", storageLabel: "Shelf A" } });
    // The refetch returns a *different* record from both the optimistic
    // placement and the pre-move original -- proving the final state comes
    // from the refetch, not merely from the restore.
    const refetched = { ...original, revision: 4, location: { kind: "storage" as const, storageLabel: "Shelf Q" } };
    let listSpoolsCalls = 0;
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") {
        listSpoolsCalls += 1;
        return Promise.resolve(snapshotResponse(0, [listSpoolsCalls === 1 ? original : refetched]));
      }
      if (command === "move_spool") {
        return Promise.reject({
          contractVersion: 1, code: "CONFLICT", message: "Someone else moved it first.",
          recovery: ["RETRY"], retryable: true, details: { slotId: "slt-1", currentOccupantSpoolId: "spl-9" },
        });
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { loadInventory, moveSpool, spoolState } = await import("./spool-store");
    await loadInventory();
    expect(listSpoolsCalls).toBe(1);

    await expect(moveSpool({
      spoolId: original.id,
      expectedSpoolRevision: original.revision,
      destination: { kind: "slot", slotId: "slt-1", expectedOccupantSpoolId: null },
    })).rejects.toMatchObject({ code: "CONFLICT" });

    expect(listSpoolsCalls).toBe(2);
    expect(spoolState.spools).toEqual([refetched]);
    expect(spoolState.pending).toEqual({});
  });

  it("leaves an unrelated Spool's newer event-applied state and the tares untouched by the refetch", async () => {
    const box = captureListenerHandler();
    const moving = spool({ id: "spl-1", location: { kind: "storage", storageLabel: "Shelf A" } });
    const unrelatedOriginal = spool({ id: "spl-unrelated", colorName: "Original Color" });
    const initialTares = [{ id: "tar-1", revision: 1, name: "Tare A", weightMg: 200_000, createdAt: "2026-01-01T00:00:00Z", updatedAt: "2026-01-01T00:00:00Z" }];
    let listSpoolsCalls = 0;
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") {
        listSpoolsCalls += 1;
        // The refetch's own snapshot is stale for the unrelated Spool and
        // carries a *different* tares array -- neither should land, since
        // the refetch only merges the affected (moving) Spool's id.
        return Promise.resolve(snapshotResponse(
          0,
          [moving, unrelatedOriginal],
          listSpoolsCalls === 1 ? initialTares : [{ ...initialTares[0], name: "Different tare" }],
        ));
      }
      if (command === "move_spool") {
        return Promise.reject({
          contractVersion: 1, code: "CONFLICT", message: "Conflict.",
          recovery: ["RETRY"], retryable: true, details: { slotId: "slt-1", currentOccupantSpoolId: null },
        });
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { loadInventory, moveSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    // An event bumps the unrelated Spool ahead of anything list_spools knows.
    const unrelatedFresh = { ...unrelatedOriginal, revision: 2, colorName: "Fresh Color" };
    box.handler({ payload: envelope(1, { type: "spoolChanged", spool: unrelatedFresh }) });
    expect(spoolState.spools.find((s) => s.id === "spl-unrelated")).toEqual(unrelatedFresh);

    await expect(moveSpool({
      spoolId: moving.id,
      expectedSpoolRevision: moving.revision,
      destination: { kind: "slot", slotId: "slt-1", expectedOccupantSpoolId: null },
    })).rejects.toMatchObject({ code: "CONFLICT" });

    expect(spoolState.spools.find((s) => s.id === "spl-unrelated")).toEqual(unrelatedFresh);
    expect(spoolState.tares).toEqual(initialTares);
  });
});

describe("moveSpool: validation", () => {
  it("rejects loading an archived Spool into a slot (D5), without calling move_spool", async () => {
    const archived = spool({ id: "spl-1", lifecycle: "archived" });
    tauriMock.invoke.mockResolvedValue(snapshotResponse(0, [archived]));

    const { loadInventory, moveSpool } = await import("./spool-store");
    await loadInventory();
    tauriMock.invoke.mockClear();

    await expect(moveSpool({
      spoolId: archived.id, expectedSpoolRevision: archived.revision,
      destination: { kind: "slot", slotId: "slt-1", expectedOccupantSpoolId: null },
    })).rejects.toMatchObject({ code: "VALIDATION" });

    expect(tauriMock.invoke).not.toHaveBeenCalledWith("move_spool", expect.anything());
  });
});

describe("moveSpool: availability hints", () => {
  it("applies a newer availability hint", async () => {
    const box = captureListenerHandler();
    const target = spool({ id: "spl-1" });
    tauriMock.invoke.mockResolvedValue(snapshotResponse(0, [target]));

    const { loadInventory, spoolState } = await import("./spool-store");
    await loadInventory();

    box.handler({
      payload: envelope(1, {
        type: "spoolAvailabilityChanged",
        spoolId: target.id,
        availability: { currentMg: 111, reservedMg: 0, availableMg: 111 },
      }),
    });

    expect(spoolState.spools.find((s) => s.id === target.id)?.availability).toEqual({ currentMg: 111, reservedMg: 0, availableMg: 111 });
  });

  it("ignores an availability hint that arrives after a newer authoritative command result", async () => {
    const box = captureListenerHandler();
    const target = spool({ id: "spl-1", availability: { currentMg: 500_000, reservedMg: 0, availableMg: 500_000 } });
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") return Promise.resolve(snapshotResponse(0, [target]));
      if (command === "update_spool") {
        return Promise.resolve({
          contractVersion: 1,
          data: { spool: { ...target, revision: 2, colorName: "Renamed" }, printers: [], warnings: [] },
        });
      }
      throw new Error(`unexpected command ${command}`);
    });

    const { loadInventory, updateSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    // A command result settles the Spool -- no stream sequence attaches to
    // it, so it's marked as authoritatively fresher than anything the
    // stream has produced so far (this store's own ruling).
    await updateSpool(target.id, {
      manufacturer: target.manufacturer, materialFamily: target.materialFamily, colorName: "Renamed",
      diameter: target.diameter, nominalMg: target.nominalMg, lowThresholdMg: target.lowThresholdMg,
    });
    expect(spoolState.spools.find((s) => s.id === target.id)?.revision).toBe(2);

    // A stale hint arrives afterward -- ignored, since nothing on the
    // stream has yet confirmed this Spool at a fresher point than the
    // command result already established.
    box.handler({
      payload: envelope(1, {
        type: "spoolAvailabilityChanged",
        spoolId: target.id,
        availability: { currentMg: 1, reservedMg: 0, availableMg: 1 },
      }),
    });

    expect(spoolState.spools.find((s) => s.id === target.id)?.availability).toEqual(target.availability);
  });
});

describe("web mode", () => {
  beforeEach(() => {
    tauriMock.isTauri.mockReturnValue(false);
  });

  it("fixture moves enforce one Spool per slot, swap with displacement, and keep facets.loaded honest", async () => {
    const { loadInventory, moveSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    const loadedSpool = spoolState.spools.find((s) => s.facets.loaded)!;
    const storedSpool = spoolState.spools.find((s) => !s.facets.loaded && s.lifecycle === "active")!;
    expect(loadedSpool.location.kind).toBe("slot");
    const slotId = loadedSpool.location.kind === "slot" ? loadedSpool.location.slotId : "";
    const printerId = loadedSpool.location.kind === "slot" ? loadedSpool.location.printerId : "";

    // Rejects a move into that slot with the wrong expected occupant.
    await expect(moveSpool({
      spoolId: storedSpool.id,
      expectedSpoolRevision: storedSpool.revision,
      destination: { kind: "slot", slotId, expectedOccupantSpoolId: null },
    })).rejects.toMatchObject({ code: "CONFLICT" });

    // Swaps in, displacing the occupant to storage.
    const result = await moveSpool({
      spoolId: storedSpool.id,
      expectedSpoolRevision: storedSpool.revision,
      destination: { kind: "slot", slotId, expectedOccupantSpoolId: loadedSpool.id, displacedStorageLabel: "Displaced shelf" },
    });

    const nowLoaded = spoolState.spools.find((s) => s.id === storedSpool.id)!;
    const nowDisplaced = spoolState.spools.find((s) => s.id === loadedSpool.id)!;
    expect(nowLoaded.location).toEqual({ kind: "slot", slotId, printerId });
    expect(nowDisplaced.location).toEqual({ kind: "storage", storageLabel: "Displaced shelf" });
    expect(result.spools.map((s) => s.id).sort()).toEqual([storedSpool.id, loadedSpool.id].sort());

    // The facet honestly reflects the new location on both sides of the swap.
    expect(nowLoaded.facets.loaded).toBe(true);
    expect(nowDisplaced.facets.loaded).toBe(false);

    // Only one Spool now occupies the slot.
    const occupants = spoolState.spools.filter((s) => s.location.kind === "slot" && s.location.slotId === slotId);
    expect(occupants).toHaveLength(1);
    expect(occupants[0].id).toBe(storedSpool.id);
  });

  it("rejects loading an archived Spool into a slot, but still allows loading an empty one (D5)", async () => {
    const { loadInventory, moveSpool, setLifecycle, spoolState } = await import("./spool-store");
    await loadInventory();
    const loadedSpool = spoolState.spools.find((s) => s.facets.loaded)!;
    const slotId = loadedSpool.location.kind === "slot" ? loadedSpool.location.slotId : "";
    const archivedSpool = spoolState.spools.find((s) => s.lifecycle === "archived")!;
    const activeStoredSpool = spoolState.spools.find((s) => !s.facets.loaded && s.lifecycle === "active" && s.id !== archivedSpool.id)!;

    await expect(moveSpool({
      spoolId: archivedSpool.id, expectedSpoolRevision: archivedSpool.revision,
      destination: { kind: "slot", slotId, expectedOccupantSpoolId: loadedSpool.id },
    })).rejects.toMatchObject({ code: "VALIDATION" });

    // Mark a Spool empty, then load it -- allowed (only "archived" is blocked).
    const emptied = await setLifecycle(activeStoredSpool.id, "markEmpty");
    expect(emptied?.lifecycle).toBe("empty");
    const loaded = await moveSpool({
      spoolId: activeStoredSpool.id, expectedSpoolRevision: emptied!.revision,
      destination: { kind: "slot", slotId, expectedOccupantSpoolId: loadedSpool.id },
    });
    expect(loaded.spools.find((s) => s.id === activeStoredSpool.id)?.location).toEqual({ kind: "slot", slotId, printerId: loadedSpool.location.kind === "slot" ? loadedSpool.location.printerId : "" });
  });

  it("createSpool, recordAmount, setLifecycle, and the tare CRUD actions work against the fixtures", async () => {
    const { loadInventory, createSpool, recordAmount, setLifecycle, createTare, updateTare, deleteTare, spoolState } =
      await import("./spool-store");
    await loadInventory();
    const tareCountBefore = spoolState.tares.length;

    const tare = await createTare("Test tare", 200_000);
    expect(tare?.weightMg).toBe(200_000);
    expect(spoolState.tares).toHaveLength(tareCountBefore + 1);

    const renamed = await updateTare(tare!.id, "Renamed tare", 210_000);
    expect(renamed?.name).toBe("Renamed tare");
    expect(spoolState.tares.find((t) => t.id === tare!.id)?.weightMg).toBe(210_000);

    const created = await createSpool(
      {
        manufacturer: "Test Co", materialFamily: "PLA", colorName: "Blue",
        diameter: "1.75", nominalMg: 1_000_000, lowThresholdMg: 100_000,
      },
      { kind: "net", netMg: 1_000_000, confidence: "estimated" },
      "Shelf Z",
    );
    expect(created?.availability.currentMg).toBe(1_000_000);
    expect(created?.facets.confidence).toBe("estimated");
    expect(spoolState.spools.some((s) => s.id === created!.id)).toBe(true);

    const measured = await recordAmount(created!.id, { kind: "scale", grossMg: 1_210_000, tareId: tare!.id });
    expect(measured?.availability.currentMg).toBe(1_000_000); // 1,210,000 - 210,000 tare
    expect(measured?.facets.confidence).toBe("measured");

    const emptied = await setLifecycle(created!.id, "markEmpty");
    expect(emptied?.lifecycle).toBe("empty");
    expect(emptied?.availability.currentMg).toBe(0);

    await deleteTare(tare!.id);
    expect(spoolState.tares.some((t) => t.id === tare!.id)).toBe(false);
    expect(spoolState.spools.find((s) => s.id === created!.id)?.tareId).toBeUndefined();
  });

  it("recordAmount rejects a scale entry whose gross is less than its tare", async () => {
    const { loadInventory, recordAmount, spoolStoreError, spoolState: state } = await import("./spool-store");
    await loadInventory();
    const target = state.spools[0];

    const result = await recordAmount(target.id, { kind: "scale", grossMg: 100, tareMg: 500 });

    expect(result).toBeUndefined();
    expect(spoolStoreError()).toBeTruthy();
  });
});

describe("other mutations against the desktop command path", () => {
  it("createSpool sends the command and merges the result", async () => {
    tauriMock.invoke.mockResolvedValue(snapshotResponse(0, []));
    const { loadInventory, createSpool, spoolState } = await import("./spool-store");
    await loadInventory();
    const created = spool({ id: "spl-new" });
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { spool: created, printers: [], warnings: [] } });

    const fields = {
      manufacturer: "Test Co", materialFamily: "PLA" as const, colorName: "Blue",
      diameter: "1.75" as const, nominalMg: 1_000_000, lowThresholdMg: 100_000,
    };
    const initialAmount = { kind: "net" as const, netMg: 1_000_000, confidence: "estimated" as const };
    const result = await createSpool(fields, initialAmount, "Shelf 1");

    expect(tauriMock.invoke).toHaveBeenCalledWith("create_spool", {
      contractVersion: 1, fields, initialAmount, storageLabel: "Shelf 1",
    });
    expect(result).toEqual(created);
    expect(spoolState.spools).toEqual([created]);
  });

  it("reports a failed mutation into spoolStoreError rather than rejecting", async () => {
    tauriMock.invoke.mockResolvedValue(snapshotResponse(0, [spool()]));
    const { loadInventory, updateSpool, spoolStoreError, dismissSpoolStoreError } = await import("./spool-store");
    await loadInventory();
    tauriMock.invoke.mockRejectedValue({
      contractVersion: 1, code: "VALIDATION", message: "That color name is too long.",
      recovery: ["EDIT_FIELDS"], retryable: false,
    });

    const result = await updateSpool("spl-1", {
      manufacturer: "Test Co", materialFamily: "PLA", colorName: "Blue",
      diameter: "1.75", nominalMg: 1_000_000, lowThresholdMg: 100_000,
    });

    expect(result).toBeUndefined();
    expect(spoolStoreError()).toBe("That color name is too long.");
    dismissSpoolStoreError();
    expect(spoolStoreError()).toBeNull();
  });

  it("does not regress a Spool a newer spool.changed event already updated", async () => {
    const box = captureListenerHandler();
    const target = spool({ id: "spl-1", revision: 1 });
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") return Promise.resolve(snapshotResponse(0, [target]));
      throw new Error(`unexpected command ${command}`);
    });
    const { loadInventory, updateSpool, spoolState } = await import("./spool-store");
    await loadInventory();

    const fresher = { ...target, revision: 3, colorName: "Fresher" };
    box.handler({ payload: envelope(1, { type: "spoolChanged", spool: fresher }) });

    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "update_spool") {
        return Promise.resolve({
          contractVersion: 1,
          data: { spool: { ...target, revision: 2, colorName: "Older result" }, printers: [], warnings: [] },
        });
      }
      throw new Error(`unexpected command ${command}`);
    });
    await updateSpool(target.id, {
      manufacturer: target.manufacturer, materialFamily: target.materialFamily, colorName: "Older result",
      diameter: target.diameter, nominalMg: target.nominalMg, lowThresholdMg: target.lowThresholdMg,
    });

    expect(spoolState.spools.find((s) => s.id === target.id)).toEqual(fresher);
  });

  it("loadHistory calls spool_history on the desktop path", async () => {
    tauriMock.invoke.mockResolvedValue(snapshotResponse(0, []));
    const { loadInventory, loadHistory } = await import("./spool-store");
    await loadInventory();
    const history = { movements: [], amountEvents: [], reservations: [] };
    tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: history });

    const result = await loadHistory("spl-1");

    expect(tauriMock.invoke).toHaveBeenCalledWith("spool_history", { contractVersion: 1, spoolId: "spl-1" });
    expect(result).toEqual(history);
  });
});
