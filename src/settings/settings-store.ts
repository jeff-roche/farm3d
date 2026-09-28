import { createSignal } from "solid-js";
import { command, desktopAvailable } from "../ipc/client";
import type { SettingsRecord } from "../generated/contracts/domain/SettingsRecord";
import type { MonitorDensity } from "../generated/contracts/domain/MonitorDensity";
import type { MonitorSection } from "../generated/contracts/domain/MonitorSection";
import type { SettingsExportOutcome } from "../generated/contracts/command/SettingsExportOutcome";
import type { SettingsImportOutcome } from "../generated/contracts/command/SettingsImportOutcome";
import type { ThemeMode } from "../design-system/theme-engine";

export type Settings = Omit<SettingsRecord, "themeMode" | "monitorSection" | "monitorDensity"> & {
  themeMode: ThemeMode;
  monitorSection: MonitorSection;
  monitorDensity: MonitorDensity;
};

const DEFAULT_SETTINGS: Settings = {
  revision: 1,
  themeMode: "system",
  monitorSection: "printerModel",
  monitorDensity: "comfortable",
  // Rust's defaults (P8 decisions 7 and 11); the desktop always loads the
  // stored values.
  notifications: {
    fatal: true,
    confirmation: true,
    completion: true,
    reconciliation: false,
    connectivity: false,
    inventory: false,
  },
  snapshotRetention: { retentionDays: 30, diskCapMb: 2048 },
  updatedAt: "",
};

let cached: Settings | null = null;

/** A reactive mirror of `cached` (Task 15's `NotificationSettingsDialog`
 *  needs Solid to re-render on `updateSettings`/`importSettings`; every
 *  other, older caller keeps using the synchronous `getSettings()`). Every
 *  place that reassigns `cached` calls `setCached` instead, so the two
 *  never drift. */
const [settingsSignal, setSettingsSignal] = createSignal<Settings | null>(null);

function setCached(next: Settings): Settings {
  cached = next;
  setSettingsSignal(next);
  return next;
}

/** The reactive read: `null` until `loadSettings()` resolves. Read this
 *  (not `getSettings()`) from anywhere that should re-render on a change. */
export const settings = settingsSignal;

/**
 * Loads settings once — from the OS-standard settings file under Tauri, or an
 * in-memory default under `just web` (no Tauri backend) — and caches the
 * result for getSettings(). Safe to call more than once.
 */
export async function loadSettings(): Promise<Settings> {
  if (!desktopAvailable()) {
    return setCached({ ...DEFAULT_SETTINGS });
  }
  const loaded = await command("load_settings");
  return setCached({
    ...DEFAULT_SETTINGS,
    ...loaded,
    themeMode: loaded.themeMode || DEFAULT_SETTINGS.themeMode,
  });
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
  setCached(next);
  if (!desktopAvailable()) return;
  setCached(await command("save_settings", {
    expectedRevision: next.revision,
    themeMode: next.themeMode,
    monitorSection: next.monitorSection,
    monitorDensity: next.monitorDensity,
  }));
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
  if (result.status === "applied") setCached(result.settings as Settings);
  return result;
}
