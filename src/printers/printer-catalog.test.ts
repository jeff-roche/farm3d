import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const RAW_CATALOG = {
  models: [
    {
      modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon",
      variants: [
        {
          variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4",
          bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
          printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
          nozzleDiameterMm: [0.4], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
          hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
          suggestedHostType: "elegoolink",
        },
        {
          variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6",
          bedShape: { kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 },
          printableHeightMm: 256, bedExcludeAreas: [], defaultBedType: "4",
          nozzleDiameterMm: [0.6], nozzleType: "hardened_steel", gcodeFlavor: "klipper",
          hasAuxiliaryFan: true, supportsAirFiltration: true, supportsMultiFilament: true,
          suggestedHostType: "elegoolink",
        },
      ],
    },
  ],
};

function stubWebCatalogFetch() {
  const fetchMock = vi.fn().mockResolvedValue({ json: () => Promise.resolve(RAW_CATALOG) });
  vi.stubGlobal("fetch", fetchMock);
  return fetchMock;
}

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
  vi.unstubAllGlobals();
});

describe("printer-catalog", () => {
  it("caches list_catalog_models after the first call", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValue([{ modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" }]);
    const { listCatalogModels } = await import("./printer-catalog");

    await listCatalogModels();
    await listCatalogModels();

    expect(tauriMock.invoke).toHaveBeenCalledTimes(1);
  });

  it("under just web, reads model summaries from the real bundled catalog instead of a synthetic list", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const fetchMock = stubWebCatalogFetch();
    const { listCatalogModels } = await import("./printer-catalog");

    expect(await listCatalogModels()).toEqual([
      { modelId: "Elegoo-CC", vendor: "Elegoo", model: "Elegoo Centauri Carbon" },
    ]);
    expect(fetchMock).toHaveBeenCalledWith("/src-tauri/resources/printer-catalog.json");
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("under just web, fetches the catalog only once across repeated calls", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const fetchMock = stubWebCatalogFetch();
    const { listCatalogModels, listCatalogVariants } = await import("./printer-catalog");

    await listCatalogModels();
    await listCatalogVariants("Elegoo", "Elegoo Centauri Carbon");

    expect(fetchMock).toHaveBeenCalledTimes(1);
  });

  it("listCatalogVariants keys on (vendor, model), not modelId", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    tauriMock.invoke.mockResolvedValue([]);
    const { listCatalogVariants } = await import("./printer-catalog");

    await listCatalogVariants("Elegoo", "Elegoo Centauri Carbon");

    expect(tauriMock.invoke).toHaveBeenCalledWith("list_catalog_variants", {
      vendor: "Elegoo",
      model: "Elegoo Centauri Carbon",
    });
  });

  it("previewProfile forwards the catalogRef", async () => {
    tauriMock.isTauri.mockReturnValue(true);
    const ref = { vendor: "Elegoo", model: "Elegoo Centauri Carbon", variant: "Elegoo Centauri Carbon 0.4 nozzle", modelId: "Elegoo-CC", printerVariant: "0.4" };
    tauriMock.invoke.mockResolvedValue({});
    const { previewProfile } = await import("./printer-catalog");

    await previewProfile(ref);

    expect(tauriMock.invoke).toHaveBeenCalledWith("preview_profile", { catalogRef: ref });
  });

  it("under just web, listCatalogVariants reads real variants for a known model", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    stubWebCatalogFetch();
    const { listCatalogVariants } = await import("./printer-catalog");

    expect(await listCatalogVariants("Elegoo", "Elegoo Centauri Carbon")).toEqual([
      { variant: "Elegoo Centauri Carbon 0.4 nozzle", printerVariant: "0.4" },
      { variant: "Elegoo Centauri Carbon 0.6 nozzle", printerVariant: "0.6" },
    ]);
  });

  it("under just web, listCatalogVariants returns an empty list for an unknown model rather than throwing", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    stubWebCatalogFetch();
    const { listCatalogVariants } = await import("./printer-catalog");

    expect(await listCatalogVariants("Nobody", "Nothing")).toEqual([]);
  });

  it("under just web, previewProfile reads the real profile for a known variant", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    stubWebCatalogFetch();
    const { previewProfile } = await import("./printer-catalog");

    const profile = await previewProfile({
      vendor: "Elegoo", model: "Elegoo Centauri Carbon",
      variant: "Elegoo Centauri Carbon 0.6 nozzle", modelId: "Elegoo-CC", printerVariant: "0.6",
    });

    expect(profile.nozzleDiameterMm).toEqual([0.6]);
    expect(profile.bedShape).toEqual({ kind: "rectangular", widthMm: 256, depthMm: 256, originXMm: 0, originYMm: 0 });
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("under just web, previewProfile rejects rather than silently returning a wrong profile for an unknown variant", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    stubWebCatalogFetch();
    const { previewProfile } = await import("./printer-catalog");

    await expect(
      previewProfile({
        vendor: "Elegoo", model: "Elegoo Centauri Carbon",
        variant: "Elegoo Centauri Carbon 12.0 nozzle", modelId: "Elegoo-CC", printerVariant: "12.0",
      }),
    ).rejects.toThrow(/no catalog variant matches/i);
  });

  describe("resolveWebCatalogVariant", () => {
    it("resolves a catalog ref, model/variant labels, and the full profile together", async () => {
      tauriMock.isTauri.mockReturnValue(false);
      stubWebCatalogFetch();
      const { resolveWebCatalogVariant } = await import("./printer-catalog");

      const resolved = await resolveWebCatalogVariant("Elegoo", "Elegoo Centauri Carbon", "0.6");

      expect(resolved?.catalogRef).toEqual({
        vendor: "Elegoo", model: "Elegoo Centauri Carbon",
        variant: "Elegoo Centauri Carbon 0.6 nozzle", modelId: "Elegoo-CC", printerVariant: "0.6",
      });
      expect(resolved?.modelLabel).toBe("Elegoo Centauri Carbon");
      expect(resolved?.variantLabel).toBe("Elegoo Centauri Carbon 0.6 nozzle");
      expect(resolved?.profile.nozzleDiameterMm).toEqual([0.6]);
    });

    it("returns null rather than throwing for a spec that no longer matches the catalog", async () => {
      tauriMock.isTauri.mockReturnValue(false);
      stubWebCatalogFetch();
      const { resolveWebCatalogVariant } = await import("./printer-catalog");

      expect(await resolveWebCatalogVariant("Elegoo", "Elegoo Centauri Carbon", "12.0")).toBeNull();
      expect(await resolveWebCatalogVariant("Nobody", "Nothing", "0.4")).toBeNull();
    });
  });
});
