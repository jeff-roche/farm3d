import { beforeEach, describe, expect, it, vi } from "vitest";
import { webAboutInfo, webDiagnosticsPreview, webStorageUsage } from "./web-fixtures";

const tauriMock = vi.hoisted(() => ({ isTauri: vi.fn(), invoke: vi.fn() }));
vi.mock("@tauri-apps/api/core", () => tauriMock);

const ok = (data: unknown) => ({ contractVersion: 1, data });
const uuid = /^[0-9a-f-]{36}$/;
const PATH = "/home/someone/private/place";

async function load() {
  return import("./diagnostics-store");
}

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset().mockReturnValue(true);
  tauriMock.invoke.mockReset();
});

describe("desktop", () => {
  it("loads the preview, usage, and about", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok(webDiagnosticsPreview()));
    await store.loadDiagnosticsPreview();
    tauriMock.invoke.mockResolvedValueOnce(ok(webStorageUsage()));
    await store.loadStorageUsage();
    tauriMock.invoke.mockResolvedValueOnce(ok(webAboutInfo()));
    await store.loadAbout();
    expect(store.diagnostics.preview()?.sections).toHaveLength(6);
    expect(store.diagnostics.usage()?.classes.length).toBeGreaterThan(5);
    expect(store.diagnostics.about()?.appVersion).toBe("0.9.0");
    expect(tauriMock.invoke.mock.calls.map(([name]) => name)).toEqual(["diagnostics_preview", "storage_usage", "about_farm3d"]);
  });

  it("exports diagnostics and presents only the file name", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({
      status: "exported", exportedAt: "2026-09-29T10:00:00Z", fileName: "diag.zip", bytes: 99, sections: ["about"], path: PATH,
    }));
    const outcome = await store.exportDiagnostics(["about"]);
    expect(tauriMock.invoke.mock.calls[0][1]).toMatchObject({ sections: ["about"] });
    expect(tauriMock.invoke.mock.calls[0][1].operationId).toMatch(uuid);
    expect(JSON.stringify(outcome)).not.toContain(PATH);
    expect(JSON.stringify(store.diagnostics.lastExport())).not.toContain(PATH);
    const { exportSummaryText } = await import("./presentation");
    expect(exportSummaryText(outcome)).toContain("diag.zip");
    expect(exportSummaryText(outcome)).not.toContain(PATH);
  });

  it("does not record a cancelled export", async () => {
    const store = await load();
    tauriMock.invoke.mockResolvedValueOnce(ok({ status: "cancelled" }));
    expect(await store.exportDiagnostics(["about"])).toEqual({ status: "cancelled" });
    expect(store.diagnostics.lastExport()).toBeNull();
  });

  it("surfaces DIAGNOSTICS_REDACTION_FAILED with its details", async () => {
    const store = await load();
    tauriMock.invoke.mockRejectedValueOnce({
      contractVersion: 1, code: "DIAGNOSTICS_REDACTION_FAILED", message: "no", recovery: [], retryable: false, details: { section: "logs" },
    });
    await expect(store.exportDiagnostics(["logs"])).rejects.toMatchObject({ code: "DIAGNOSTICS_REDACTION_FAILED", details: { section: "logs" } });
    expect(store.diagnostics.lastExport()).toBeNull();
  });

  it("clear_storage refreshes the usage from its outcome", async () => {
    const store = await load();
    const usage = { ...webStorageUsage(), totalBytes: 1 };
    tauriMock.invoke.mockResolvedValueOnce(ok({ target: "rotatedLogs", removedCount: 2, freedBytes: 5, usage }));
    const outcome = await store.clearStorage("rotatedLogs");
    expect(outcome.removedCount).toBe(2);
    expect(store.diagnostics.usage()?.totalBytes).toBe(1);
    expect(tauriMock.invoke.mock.calls[0][1]).toMatchObject({ target: "rotatedLogs" });
    expect(tauriMock.invoke.mock.calls[0][1].operationId).toMatch(uuid);
  });

  it("resets without holding a path, and reuses the operationId on a retry", async () => {
    const store = await load();
    tauriMock.invoke
      .mockRejectedValueOnce(new Error("transport"))
      .mockResolvedValueOnce(ok({ tier: "farm", status: "restarting", safetyBackupId: "bkp-9", path: PATH }));
    const result = await store.resetFarm({ tier: "farm", safetyBackup: true, deleteSafetyBackups: false }, "reset farm");
    const calls = tauriMock.invoke.mock.calls.filter(([name]) => name === "reset_farm");
    expect(calls).toHaveLength(2);
    expect(calls[0][1].operationId).toBe(calls[1][1].operationId);
    expect(calls[0][1]).toMatchObject({ confirmation: "reset farm", request: { tier: "farm" } });
    expect(JSON.stringify(result)).not.toContain(PATH);
    expect(store.diagnostics.lastReset()).toEqual({ tier: "farm", restarting: true, safetyBackupId: "bkp-9" });
    const { resetSummaryText } = await import("./presentation");
    expect(resetSummaryText(result)).not.toContain(PATH);
  });

  it("summarises the other reset tiers", async () => {
    const store = await load();
    const { resetSummaryText } = await import("./presentation");
    tauriMock.invoke.mockResolvedValueOnce(ok({ tier: "cameraMedia", prunedCount: 7, freedBytes: 2_738_000 }));
    const media = await store.resetFarm({ tier: "cameraMedia", scope: "unpinned" }, "reset media");
    expect(resetSummaryText(media)).toContain("7");
    expect(store.diagnostics.lastReset()).toEqual({ tier: "cameraMedia", prunedCount: 7, freedBytes: 2_738_000 });
    tauriMock.invoke.mockResolvedValueOnce(ok({ tier: "settings", settings: { revision: 3 } }));
    const settings = await store.resetFarm({ tier: "settings", expectedRevision: 2 }, "reset settings");
    expect(resetSummaryText(settings)).toContain("Settings");
    expect(store.diagnostics.lastReset()).toEqual({ tier: "settings" });
  });

  it("loads a reset preview per tier", async () => {
    const store = await load();
    const { webResetPreview } = await import("./web-fixtures");
    tauriMock.invoke.mockResolvedValueOnce(ok(webResetPreview("farm")));
    const preview = await store.loadResetPreview("farm");
    expect(tauriMock.invoke).toHaveBeenCalledWith("reset_preview", { contractVersion: 1, tier: "farm" });
    expect(store.diagnostics.resetPreview("farm")?.phrase).toBe(preview.phrase);
  });
});

describe("web mode (D18)", () => {
  beforeEach(() => tauriMock.isTauri.mockReturnValue(false));

  it("serves fixtures for reads", async () => {
    const store = await load();
    await store.loadDiagnosticsPreview();
    await store.loadStorageUsage();
    await store.loadAbout();
    await store.loadResetPreview("cameraMedia");
    expect(store.diagnostics.preview()).not.toBeNull();
    expect(store.diagnostics.usage()).not.toBeNull();
    expect(store.diagnostics.about()).not.toBeNull();
    expect(store.diagnostics.resetPreview("cameraMedia")?.phrase).toBeTruthy();
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });

  it("answers export_diagnostics with unsupported and throws for other writes", async () => {
    const store = await load();
    expect(await store.exportDiagnostics(["about"])).toEqual({ status: "unsupported", reason: "desktopRequired" });
    await expect(store.clearStorage("orcaCache")).rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    await expect(store.resetFarm({ tier: "farm", safetyBackup: true, deleteSafetyBackups: false }, "reset farm"))
      .rejects.toMatchObject({ code: "PERSISTENCE_UNAVAILABLE" });
    expect(tauriMock.invoke).not.toHaveBeenCalled();
  });
});
