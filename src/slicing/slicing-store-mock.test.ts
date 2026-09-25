import { beforeEach, describe, expect, it, vi } from "vitest";
import { desktopOnlyActions } from "./desktop-only";
import {
  loadWebSlicingFixture,
  refuseDesktopOnlyActions,
  resetSlicingStoreMock,
  setSlicingProgress,
  slicingStoreMock,
} from "./slicing-store-mock";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(() => false), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

describe("slicingStoreMock", () => {
  beforeEach(() => resetSlicingStoreMock());

  it("mirrors every export of the real store's API", async () => {
    const real = await import("./slicing-store");
    const exported = Object.keys(real).sort();
    expect(Object.keys(slicingStoreMock).sort()).toEqual(exported);
    expect(Object.keys(slicingStoreMock.slicing).sort()).toEqual(Object.keys(real.slicing).sort());
  });

  it("refuses the desktop-only actions exactly as the real store does in web mode", async () => {
    const real = await import("./slicing-store");
    await real.startSlicing();
    refuseDesktopOnlyActions();
    const facts = {
      printerProfile: { kind: "absent" as const },
      nozzleDiameterMm: { kind: "absent" as const },
      materialFamily: { kind: "absent" as const },
      filamentDiameterMm: { kind: "absent" as const },
    };
    const attempts = {
      startSlice: (store: typeof real) => store.startSlice("prp-web-enclosure", ["plt-web-enclosure-1"]),
      cancelSliceOperation: (store: typeof real) => store.cancelSliceOperation("sop-web-enclosure-lid"),
      createExternalSliceRevision: (store: typeof real) => store.createExternalSliceRevision("msr-web-cube-gcode-1", facts),
      pickSlicerEngine: (store: typeof real) => store.pickSlicerEngine(),
      pickPresetSource: (store: typeof real) => store.pickPresetSource("file"),
      resetSlicerRuntime: (store: typeof real) => store.resetSlicerRuntime({ engine: true, presetSource: true }),
    };
    expect(Object.keys(attempts).sort()).toEqual([...desktopOnlyActions].sort());
    for (const attempt of Object.values(attempts)) {
      const fromReal = await attempt(real).catch((e: unknown) => e);
      const fromMock = await attempt(slicingStoreMock as unknown as typeof real).catch((e: unknown) => e);
      expect(fromMock).toEqual(fromReal);
      expect(fromMock).toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    }
  });

  it("serves the web fixtures on its read side and read actions", async () => {
    const fixture = loadWebSlicingFixture();
    const { slicing } = slicingStoreMock;
    expect(slicing.runtime()?.engine.state).toBe("available");
    expect(slicing.preparation("mdl-web-enclosure")?.id).toBe(fixture.preparations[0].id);
    expect(slicing.revisions("mdl-web-cube-gcode")).toHaveLength(1);
    const geometry = await slicingStoreMock.loadGeometry("msr-web-enclosure-1");
    expect(geometry.buildItems.filter((item) => !item.printable)).toHaveLength(1);
    await expect(slicingStoreMock.loadMesh("msr-web-enclosure-1", 2)).resolves.toMatchObject({ triangleCount: 12 });

    setSlicingProgress("sop-1", { totalPercent: 5, message: "Loading" });
    expect(slicing.progress("sop-1")).toEqual({ totalPercent: 5, message: "Loading" });
    setSlicingProgress("sop-1", undefined);
    expect(slicing.progress("sop-1")).toBeUndefined();

    resetSlicingStoreMock();
    expect(slicing.runtime()).toBeNull();
    expect(slicing.preparation("mdl-web-enclosure")).toBeUndefined();
  });
});
