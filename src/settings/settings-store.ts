import { invoke, isTauri } from "@tauri-apps/api/core";
import type { ThemeMode } from "../design-system/theme-engine";

export interface Settings {
  themeMode: ThemeMode;
}

const DEFAULT_SETTINGS: Settings = { themeMode: "system" };

let cached: Settings | null = null;

/**
 * Loads settings once — from the OS-standard settings file under Tauri, or an
 * in-memory default under `just web` (no Tauri backend) — and caches the
 * result for getSettings(). Safe to call more than once.
 */
export async function loadSettings(): Promise<Settings> {
  if (!isTauri()) {
    cached = { ...DEFAULT_SETTINGS };
    return cached;
  }
  const loaded = await invoke<Settings>("load_settings");
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
  if (!isTauri()) return;
  await invoke("save_settings", { settings: next });
}

/** Opens the settings file in the OS-default editor. A no-op under `just web`. */
export async function openSettingsFile(): Promise<void> {
  if (!isTauri()) return;
  await invoke("open_settings_file");
}
