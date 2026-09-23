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

function snapshotResponse(sequence: number, spools: SpoolRecord[]) {
  return { contractVersion: 1, data: { streamId: "stream-a", snapshotSequence: sequence, spools, tares: [] } };
}

describe("loadInventory", () => {
  it("registers the event listener before backfilling through list_spools, and drops events at or below the snapshot sequence", async () => {
    const calls: string[] = [];
    let handler!: (event: { payload: unknown }) => void;
    eventMock.listen.mockImplementation((name: string, cb: typeof handler) => {
      calls.push(`listen:${name}`);
      handler = cb;
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
    handler({ payload: envelope(5, { type: "spoolChanged", spool: staleSpool }) });
    expect(spoolState.spools).toEqual([]);

    const freshSpool = spool({ id: "spl-2" });
    handler({ payload: envelope(6, { type: "spoolChanged", spool: freshSpool }) });
    expect(spoolState.spools).toEqual([freshSpool]);
  });

  it("buffers events received while backfilling and replays only those above the snapshot sequence", async () => {
    let handler!: (event: { payload: unknown }) => void;
    eventMock.listen.mockImplementation((_name: string, cb: typeof handler) => {
      handler = cb;
      return Promise.resolve(vi.fn());
    });
    let resolveBackfill!: (value: unknown) => void;
    tauriMock.invoke.mockImplementation(() => new Promise((resolve) => (resolveBackfill = resolve)));

    const { loadInventory, spoolState } = await import("./spool-store");
    const started = loadInventory();
    // `listen(...)` and then `list_spools` are only reached after internal
    // `await`s (a dynamic import, then the listener promise itself), which
    // can each take more than one microtask -- poll rather than guess.
    for (let i = 0; i < 20 && (!handler || !resolveBackfill); i += 1) await Promise.resolve();

    const early = spool({ id: "spl-early" });
    const late = spool({ id: "spl-late" });
    handler({ payload: envelope(2, { type: "spoolChanged", spool: early }) });
    handler({ payload: envelope(4, { type: "spoolChanged", spool: late }) });
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

describe("moveSpool", () => {
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

  it("on a CONFLICT rejection, restores the pre-move snapshot, refetches through list_spools, and rethrows", async () => {
    const original = spool({ id: "spl-1", revision: 3, location: { kind: "storage", storageLabel: "Shelf A" } });
    let listSpoolsCalls = 0;
    tauriMock.invoke.mockImplementation((command: string) => {
      if (command === "list_spools") {
        listSpoolsCalls += 1;
        return Promise.resolve(snapshotResponse(0, [original]));
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
    expect(spoolState.spools).toEqual([original]);
    expect(spoolState.pending).toEqual({});
  });

  it("ignores a late spool.changed event with a lower revision than the settled one", async () => {
    let handler!: (event: { payload: unknown }) => void;
    eventMock.listen.mockImplementation((_name: string, cb: typeof handler) => {
      handler = cb;
      return Promise.resolve(vi.fn());
    });
    const settled = spool({ revision: 5, colorName: "Settled Color" });
    tauriMock.invoke.mockResolvedValue(snapshotResponse(10, [settled]));

    const { loadInventory, spoolState } = await import("./spool-store");
    await loadInventory();
    expect(spoolState.spools).toEqual([settled]);

    const stale = spool({ revision: 3, colorName: "Stale Color" });
    handler({ payload: envelope(11, { type: "spoolChanged", spool: stale }) });

    expect(spoolState.spools).toEqual([settled]);
  });
});

describe("web mode", () => {
  beforeEach(() => {
    tauriMock.isTauri.mockReturnValue(false);
  });

  it("fixture moves enforce one Spool per slot and swap with displacement", async () => {
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
    })).rejects.toBeTruthy();

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

    // Only one Spool now occupies the slot.
    const occupants = spoolState.spools.filter((s) => s.location.kind === "slot" && s.location.slotId === slotId);
    expect(occupants).toHaveLength(1);
    expect(occupants[0].id).toBe(storedSpool.id);
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
