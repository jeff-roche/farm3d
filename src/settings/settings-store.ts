import { command, desktopAvailable } from "../ipc/client";
import type { SettingsRecord } from "../generated/contracts/domain/SettingsRecord";
import type { SettingsExportOutcome } from "../generated/contracts/command/SettingsExportOutcome";
import type { SettingsImportOutcome } from "../generated/contracts/command/SettingsImportOutcome";
import type { ThemeMode } from "../design-system/theme-engine";

export type Settings = Omit<SettingsRecord, "themeMode"> & { themeMode: ThemeMode };

const DEFAULT_SETTINGS: Settings = { revision: 1, themeMode: "system", updatedAt: "" };

let cached: Settings | null = null;

/**
 * Loads settings once — from the OS-standard settings file under Tauri, or an
 * in-memory default under `just web` (no Tauri backend) — and caches the
 * result for getSettings(). Safe to call more than once.
 */
export async function loadSettings(): Promise<Settings> {
  if (!desktopAvailable()) {
    cached = { ...DEFAULT_SETTINGS };
    return cached;
  }
  const loaded = await command("load_settings");
  cached = {
    ...DEFAULT_SETTINGS,
    ...loaded,
    themeMode: loaded.themeMode || DEFAULT_SETTINGS.themeMode,
  };
  return cached;
}

/** Synchronous read of the last-loaded settings. Throws if loadSettings() hasn't resolved yet. */
export function getSettings(): Settings {
  if (!cached) {
    throw new Error("getSettings() called before loadSettings() resolved");
  }
  return cached;
}

/**
 * Merges `partial` into the cached settings (updating the cache immediately)
 * and persists the result — a no-op under `just web`, where there's no
 * settings file to write.
 */
export async function updateSettings(partial: Partial<Settings>): Promise<void> {
  const next = { ...(cached ?? DEFAULT_SETTINGS), ...partial };
  cached = next;
  if (!desktopAvailable()) return;
  cached = await command("save_settings", {
    expectedRevision: next.revision,
    themeMode: next.themeMode,
  });
}

export async function exportSettings(): Promise<SettingsExportOutcome> {
  if (!desktopAvailable()) return { status: "unsupported", reason: "desktopRequired" };
  return command("export_settings");
}

export async function importSettings(): Promise<SettingsImportOutcome> {
  if (!desktopAvailable()) return { status: "unsupported", reason: "desktopRequired" };
  const result = await command("import_settings", {
    expectedRevision: (cached ?? DEFAULT_SETTINGS).revision,
  });
  if (result.status === "applied") cached = result.settings as Settings;
  return result;
}
