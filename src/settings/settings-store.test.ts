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
      tauriMock.invoke.mockResolvedValue({ themeMode: "farm3d-dark" });
      const { loadSettings, getSettings } = await import("./settings-store");

      const settings = await loadSettings();

      expect(tauriMock.invoke).toHaveBeenCalledWith("load_settings");
      expect(settings).toEqual({ themeMode: "farm3d-dark" });
      expect(getSettings()).toEqual({ themeMode: "farm3d-dark" });
    });

    it("falls back to the default theme mode when the backend returns an empty string", async () => {
      tauriMock.invoke.mockResolvedValue({ themeMode: "" });
      const { loadSettings } = await import("./settings-store");

      const settings = await loadSettings();

      expect(settings).toEqual({ themeMode: "system" });
    });

    it("persists merged settings via the save_settings command", async () => {
      tauriMock.invoke.mockResolvedValue({ themeMode: "system" });
      const { loadSettings, updateSettings } = await import("./settings-store");
      await loadSettings();
      tauriMock.invoke.mockResolvedValue(undefined);

      await updateSettings({ themeMode: "farm3d-light" });

      expect(tauriMock.invoke).toHaveBeenCalledWith("save_settings", {
        settings: { themeMode: "farm3d-light" },
      });
    });

    it("invokes open_settings_file", async () => {
      tauriMock.invoke.mockResolvedValue(undefined);
      const { openSettingsFile } = await import("./settings-store");

      await openSettingsFile();

      expect(tauriMock.invoke).toHaveBeenCalledWith("open_settings_file");
    });
  });

  describe("under just web (no Tauri backend)", () => {
    beforeEach(() => {
      tauriMock.isTauri.mockReturnValue(false);
    });

    it("resolves default settings without invoking any command", async () => {
      const { loadSettings } = await import("./settings-store");

      const settings = await loadSettings();

      expect(settings).toEqual({ themeMode: "system" });
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("updates the cache without persisting", async () => {
      const { loadSettings, updateSettings, getSettings } = await import("./settings-store");
      await loadSettings();

      await updateSettings({ themeMode: "farm3d-dark" });

      expect(getSettings()).toEqual({ themeMode: "farm3d-dark" });
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });

    it("resolves openSettingsFile without invoking any command", async () => {
      const { openSettingsFile } = await import("./settings-store");

      await expect(openSettingsFile()).resolves.toBeUndefined();
      expect(tauriMock.invoke).not.toHaveBeenCalled();
    });
  });

  it("getSettings throws before loadSettings has resolved", async () => {
    tauriMock.isTauri.mockReturnValue(false);
    const { getSettings } = await import("./settings-store");

    expect(() => getSettings()).toThrow();
  });
});
