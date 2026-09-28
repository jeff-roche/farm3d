import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const tauriMock = vi.hoisted(() => ({
  isTauri: vi.fn(),
  invoke: vi.fn(),
}));

vi.mock("@tauri-apps/api/core", () => tauriMock);

beforeEach(() => {
  vi.resetModules();
  tauriMock.isTauri.mockReset();
  tauriMock.invoke.mockReset();
});

afterEach(() => {
  vi.restoreAllMocks();
});

describe("settings-store", () => {
  describe("under Tauri", () => {
    beforeEach(() => {
      tauriMock.isTauri.mockReturnValue(true);
    });

    it("loads settings via the load_settings command and caches them", async () => {
      const record = {
        revision: 1,
        themeMode: "farm3d-dark",
        monitorSection: "printerModel",
        monitorDensity: "comfortable",
        notifications: { fatal: true, confirmation: false, completion: true, reconciliation: false, connectivity: true, inventory: false },
        snapshotRetention: { retentionDays: 7, diskCapMb: 512 },
        updatedAt: "now",
      };
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: record });
      const { loadSettings, getSettings } = await import("./settings-store");

      const settings = await loadSettings();

      expect(tauriMock.invoke).toHaveBeenCalledWith("load_settings", { contractVersion: 1 });
      expect(settings).toEqual(record);
      expect(getSettings()).toEqual(settings);
    });

    it("falls back to the default theme mode when the backend returns an empty string", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { revision: 1, themeMode: "", monitorSection: "printerModel", monitorDensity: "comfortable", updatedAt: "now" } });
      const { loadSettings } = await import("./settings-store");

      const settings = await loadSettings();

      expect(settings.themeMode).toBe("system");
    });

    it("persists merged settings via the save_settings command", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { revision: 1, themeMode: "system", monitorSection: "printerModel", monitorDensity: "comfortable", updatedAt: "now" } });
      const { loadSettings, updateSettings } = await import("./settings-store");
      await loadSettings();
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { revision: 2, themeMode: "farm3d-light", monitorSection: "printerModel", monitorDensity: "comfortable", updatedAt: "later" } });

      await updateSettings({ themeMode: "farm3d-light" });

      expect(tauriMock.invoke).toHaveBeenCalledWith("save_settings", {
        contractVersion: 1, expectedRevision: 1, themeMode: "farm3d-light", monitorSection: "printerModel", monitorDensity: "comfortable",
      });
    });

    it("invokes export_settings", async () => {
      tauriMock.invoke.mockResolvedValue({ contractVersion: 1, data: { status: "cancelled" } });
      const { exportSettings } = await import("./settings-store");

      await exportSettings();

      expect(tauriMock.invoke).toHaveBeenCalledWith("export_settings", { contractVersion: 1 });
    });
  });

  describe("under just web (no Tauri backend)", () => {
    beforeEach(() => {
      tauriMock.isTauri.mockReturnValue(false);
    });

    it("resolves default settings without invoking any command", async () => {
      const { loadSettings } = await import("./settings-store");

      const settings = await loadSettings();

      expect(settings.themeMode).toBe("system");
      expect(settings.monitorSection).toBe("printerModel");
      expect(settings.monitorDensity).toBe("comfortable");
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("updates the cache without persisting", async () => {
      const { loadSettings, updateSettings, getSettings } = await import("./settings-store");
      await loadSettings();

      await updateSettings({ themeMode: "farm3d-dark" });

      expect(getSettings().themeMode).toBe("farm3d-dark");
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("reports export unsupported without invoking any command", async () => {
      const { exportSettings } = await import("./settings-store");

      await expect(exportSettings()).resolves.toEqual({ status: "unsupported", reason: "desktopRequired" });
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });
  });

  it("getSettings throws before loadSettings has resolved", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { getSettings } = await import("./settings-store");

    expect(() => getSettings()).toThrow();
  });

  describe("settings (reactive accessor)", () => {
    it("is null before loadSettings resolves, then the loaded value, and updates on updateSettings", async () => {
      tauriMock.isTauri.mockReturnValue(false);
      const { settings: settingsSignal, loadSettings, updateSettings } = await import("./settings-store");

      expect(settingsSignal()).toBeNull();
      await loadSettings();
      expect(settingsSignal()?.themeMode).toBe("system");

      await updateSettings({ themeMode: "farm3d-dark" });
      expect(settingsSignal()?.themeMode).toBe("farm3d-dark");
    });

    it("tracks changes from a Solid reactive scope (Task 15's dialog needs this)", async () => {
      tauriMock.isTauri.mockReturnValue(false);
      const { createRoot, createEffect } = await import("solid-js");
      const { settings: settingsSignal, loadSettings, updateSettings } = await import("./settings-store");

      const seen: (string | undefined)[] = [];
      const dispose = createRoot((d) => {
        createEffect(() => seen.push(settingsSignal()?.themeMode));
        return d;
      });
      await loadSettings();
      await updateSettings({ themeMode: "farm3d-dark" });
      dispose();

      expect(seen).toEqual([undefined, "system", "farm3d-dark"]);
    });
  });
});
