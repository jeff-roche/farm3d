import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
});

afterEach(() => vi.restoreAllMocks());

describe("printer-catalog", () => {
  it("caches list_catalog_models after the first call", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValue([{ modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" }]);
    const { listCatalogModels } = await import("./printer-catalog");

    await listCatalogModels();
    await listCatalogModels();

    expect(tauriMock.invoke).toHaveBeenCalledTimes(1);
  });

  it("resolves to an empty list under just web", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { listCatalogModels } = await import("./printer-catalog");
    expect(await listCatalogModels()).toEqual([]);
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("previewProfile forwards the catalogRef", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    const ref = { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" };
    tauriMock.invoke.mockResolvedValue({});
    const { previewProfile } = await import("./printer-catalog");

    await previewProfile(ref);

    expect(tauriMock.invoke).toHaveBeenCalledWith("preview_profile", { catalogRef: ref });
  });
});
