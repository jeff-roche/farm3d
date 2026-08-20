# Settings System Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace `localStorage`-based theme persistence with a real OS-standard settings file, and give the user a gear-icon menu (replacing the current theme icon) to change the theme or open that file directly.

**Architecture:** Two new small custom Tauri commands (`load_settings`/`save_settings`/`open_settings_file`) read/write a `settings.json` in the OS-standard app-config directory via `std::fs` + `serde_json`. A new framework-agnostic frontend module (`src/settings/settings-store.ts`) wraps those commands (with an in-memory no-op fallback under `just web`, which has no Tauri backend). `theme-engine.ts` is changed to load/persist through that store instead of `localStorage`, and `ThemeMenu.tsx` is renamed to `SettingsMenu.tsx`, gets a gear-icon trigger, and gains an "Open settings file" menu item.

**Tech Stack:** Tauri 2 (Rust backend, `tauri-plugin-opener` already a dependency), SolidJS + TypeScript frontend, `@tauri-apps/api` (`invoke`, `isTauri`), Vitest + `@solidjs/testing-library`, `@tabler/icons-solidjs`.

**Spec:** `docs/superpowers/specs/2026-08-20-settings-system-design.md`

## Global Constraints

- App identifier is `com.jroche.farm3d` (from `src-tauri/tauri.conf.json`) — this is what Tauri's path resolver uses to build the OS-standard config dir; don't hardcode a different identifier anywhere.
- Settings file is named exactly `settings.json`, JSON keys are `camelCase` (e.g. `themeMode`), pretty-printed (human-editable).
- No new Tauri plugin dependencies (`@tauri-apps/plugin-fs` etc.) — persistence is two custom `#[tauri::command]` functions using `std::fs`/`serde_json`, which are already Cargo dependencies.
- The gear icon **replaces** the current theme icon in `ActivityBar.tsx`'s existing slot — one entry point, not two icons.
- `localStorage` persistence for theme is removed entirely — the settings file (or its in-memory fallback under `just web`) is the sole source of truth.
- "Open settings file" opens the file in the OS-default editor via the existing `tauri-plugin-opener` dependency (`OpenerExt::open_path`) — not a reveal-in-folder action.
- Under `just web` (`npm run dev`, no Tauri/Rust backend), nothing may throw — settings operations fall back to an in-memory default and persistence is a no-op.
- CSS Modules only, referencing `--f3d-*` custom properties — no hardcoded colors/sizes (`AGENTS.md`).
- `just build` and `just test` must both pass before frontend work is considered done (`AGENTS.md`).

---

## Task 1: Rust settings backend

**Files:**
- Create: `src-tauri/src/settings.rs`
- Modify: `src-tauri/src/lib.rs`
- Modify: `justfile`

**Interfaces:**
- Produces (consumed by the frontend in Task 2 via `invoke`): three Tauri commands —
  `load_settings() -> Settings`, `save_settings(settings: Settings) -> ()`,
  `open_settings_file() -> ()` — where `Settings` serializes as
  `{"themeMode": string}` (camelCase JSON).

- [ ] **Step 1: Write `src-tauri/src/settings.rs` with pure, testable fs logic plus the three Tauri commands**

```rust
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Manager};
use tauri_plugin_opener::OpenerExt;

const SETTINGS_FILE_NAME: &str = "settings.json";

#[derive(Serialize, Deserialize, Default, Clone, PartialEq, Debug)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme_mode: String,
}

fn settings_file_path(config_dir: &Path) -> PathBuf {
    config_dir.join(SETTINGS_FILE_NAME)
}

fn write_settings_to(config_dir: &Path, settings: &Settings) -> Result<(), String> {
    fs::create_dir_all(config_dir).map_err(|e| e.to_string())?;
    let json = serde_json::to_string_pretty(settings).map_err(|e| e.to_string())?;
    fs::write(settings_file_path(config_dir), json).map_err(|e| e.to_string())
}

fn load_settings_from(config_dir: &Path) -> Result<Settings, String> {
    let path = settings_file_path(config_dir);
    if !path.exists() {
        let defaults = Settings::default();
        write_settings_to(config_dir, &defaults)?;
        return Ok(defaults);
    }
    match fs::read_to_string(&path) {
        Ok(contents) => Ok(serde_json::from_str(&contents).unwrap_or_default()),
        Err(_) => Ok(Settings::default()),
    }
}

fn app_config_dir(app: &AppHandle) -> Result<PathBuf, String> {
    app.path().app_config_dir().map_err(|e| e.to_string())
}

#[tauri::command]
pub fn load_settings(app: AppHandle) -> Result<Settings, String> {
    load_settings_from(&app_config_dir(&app)?)
}

#[tauri::command]
pub fn save_settings(app: AppHandle, settings: Settings) -> Result<(), String> {
    write_settings_to(&app_config_dir(&app)?, &settings)
}

#[tauri::command]
pub fn open_settings_file(app: AppHandle) -> Result<(), String> {
    let config_dir = app_config_dir(&app)?;
    let path = settings_file_path(&config_dir);
    if !path.exists() {
        write_settings_to(&config_dir, &Settings::default())?;
    }
    app.opener()
        .open_path(path.to_string_lossy().to_string(), None::<&str>)
        .map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicU32, Ordering};

    static COUNTER: AtomicU32 = AtomicU32::new(0);

    fn temp_dir() -> PathBuf {
        let id = COUNTER.fetch_add(1, Ordering::SeqCst);
        std::env::temp_dir().join(format!("farm3d-settings-test-{}-{}", std::process::id(), id))
    }

    #[test]
    fn load_creates_default_file_when_missing() {
        let dir = temp_dir();
        let settings = load_settings_from(&dir).unwrap();
        assert_eq!(settings, Settings::default());
        assert!(settings_file_path(&dir).exists());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn save_then_load_round_trips() {
        let dir = temp_dir();
        let settings = Settings {
            theme_mode: "farm3d-dark".to_string(),
        };
        write_settings_to(&dir, &settings).unwrap();
        let loaded = load_settings_from(&dir).unwrap();
        assert_eq!(loaded, settings);
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_falls_back_to_default_on_corrupt_file() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(settings_file_path(&dir), "not valid json").unwrap();
        let loaded = load_settings_from(&dir).unwrap();
        assert_eq!(loaded, Settings::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn load_fills_missing_fields_with_defaults() {
        let dir = temp_dir();
        fs::create_dir_all(&dir).unwrap();
        fs::write(settings_file_path(&dir), "{}").unwrap();
        let loaded = load_settings_from(&dir).unwrap();
        assert_eq!(loaded, Settings::default());
        fs::remove_dir_all(&dir).ok();
    }

    #[test]
    fn json_uses_camel_case_keys() {
        let settings = Settings {
            theme_mode: "farm3d-dark".to_string(),
        };
        let json = serde_json::to_string(&settings).unwrap();
        assert_eq!(json, r#"{"themeMode":"farm3d-dark"}"#);
    }
}
```

- [ ] **Step 2: Add a `test-rust` recipe to `justfile`**

Add this recipe (after the existing `install-rust` recipe, matching its `--manifest-path` style):

```just
# Run the Tauri backend's Rust test suite
test-rust:
    cargo test --manifest-path src-tauri/Cargo.toml
```

- [ ] **Step 3: Run the new Rust tests to verify they pass**

Run: `source "$HOME/.cargo/env" && just test-rust`
Expected: 5 tests pass (`load_creates_default_file_when_missing`,
`save_then_load_round_trips`, `load_falls_back_to_default_on_corrupt_file`,
`load_fills_missing_fields_with_defaults`, `json_uses_camel_case_keys`).

- [ ] **Step 4: Wire the three commands into `lib.rs` and remove the unused `greet` placeholder**

Replace the full contents of `src-tauri/src/lib.rs` with:

```rust
mod settings;

use settings::{load_settings, open_settings_file, save_settings};

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .invoke_handler(tauri::generate_handler![
            load_settings,
            save_settings,
            open_settings_file
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
```

- [ ] **Step 5: Verify the Rust backend still compiles cleanly**

Run: `source "$HOME/.cargo/env" && cargo check --manifest-path src-tauri/Cargo.toml`
Expected: compiles with no errors (warnings about unused items are not expected — `load_settings`, `save_settings`, and `open_settings_file` are all referenced via `generate_handler!`).

- [ ] **Step 6: Commit**

```bash
git add src-tauri/src/settings.rs src-tauri/src/lib.rs justfile
git commit -m "Add Rust settings backend: load/save/open commands for an OS-standard settings.json"
```

---

## Task 2: Frontend settings store

**Files:**
- Create: `src/settings/settings-store.ts`
- Create: `src/settings/settings-store.test.ts`

**Interfaces:**
- Consumes: Tauri commands `load_settings`, `save_settings`, `open_settings_file` from Task 1 (via `invoke` from `@tauri-apps/api/core`).
- Produces (consumed by Task 3's `theme-engine.ts` and Task 4's `SettingsMenu.tsx`):
  - `interface Settings { themeMode: ThemeMode }`
  - `loadSettings(): Promise<Settings>`
  - `getSettings(): Settings` (throws if called before `loadSettings()` has resolved)
  - `updateSettings(partial: Partial<Settings>): Promise<void>`
  - `openSettingsFile(): Promise<void>`

- [ ] **Step 1: Write the failing tests in `src/settings/settings-store.test.ts`**

```typescript
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
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `npx vitest run src/settings/settings-store.test.ts`
Expected: FAIL — `Cannot find module './settings-store'` (the module doesn't exist yet).

- [ ] **Step 3: Write `src/settings/settings-store.ts`**

```typescript
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
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `npx vitest run src/settings/settings-store.test.ts`
Expected: PASS — all 8 tests green.

- [ ] **Step 5: Commit**

```bash
git add src/settings/settings-store.ts src/settings/settings-store.test.ts
git commit -m "Add frontend settings store wrapping the Tauri settings commands"
```

---

## Task 3: Wire theme-engine persistence through the settings store

**Files:**
- Modify: `src/design-system/theme-engine.ts`
- Modify: `src/design-system/theme-engine.test.ts`
- Modify: `src/index.tsx`
- Modify: `DESIGN.md`

**Interfaces:**
- Consumes: `loadSettings`, `updateSettings` from `../settings/settings-store` (Task 2).
- Produces: `initTheme()` changes from `(): void` to `(): Promise<void>` — this is a breaking signature change; `src/index.tsx` (the only call site) is updated in this task.

- [ ] **Step 1: Update the failing/changed tests in `src/design-system/theme-engine.test.ts`**

Replace the full contents of `src/design-system/theme-engine.test.ts` with:

```typescript
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Theme } from "./tokens/types";
import { farm3dTypography } from "./tokens/typography";
import { farm3dShape } from "./tokens/shape";

const settingsStoreMock = vi.hoisted(() => ({
  loadSettings: vi.fn(),
  updateSettings: vi.fn(),
}));

vi.mock("../settings/settings-store", () => settingsStoreMock);

/** Minimal mock of matchMedia('(prefers-color-scheme: dark)') that supports firing 'change'. */
function mockMatchMedia(initialDark: boolean) {
  let dark = initialDark;
  const listeners = new Set<(e: { matches: boolean }) => void>();
  vi.stubGlobal("matchMedia", (query: string) => ({
    get matches() {
      return query.includes("dark") && dark;
    },
    media: query,
    addEventListener: (_event: string, listener: (e: { matches: boolean }) => void) => {
      listeners.add(listener);
    },
    removeEventListener: (_event: string, listener: (e: { matches: boolean }) => void) => {
      listeners.delete(listener);
    },
  }));
  return {
    setDark(next: boolean) {
      dark = next;
      for (const listener of listeners) listener({ matches: dark });
    },
  };
}

function makeTheme(name: string, scheme: "light" | "dark", accent: string): Theme {
  return {
    name,
    scheme,
    color: {
      bg: "#000000",
      surface: "#000000",
      surfaceRaised: "#000000",
      surfaceHover: "#000000",
      surfaceSelected: "#000000",
      border: "#000000",
      borderStrong: "#000000",
      text: "#000000",
      textMuted: "#000000",
      textDisabled: "#000000",
      accent,
      onAccent: "#000000",
      accentMuted: "#000000",
      danger: "#000000",
      onDanger: "#000000",
      warning: "#000000",
      onWarning: "#000000",
      success: "#000000",
      onSuccess: "#000000",
      focusRing: "#000000",
    },
    typography: farm3dTypography,
    shape: farm3dShape,
  };
}

beforeEach(() => {
  vi.resetModules();
  settingsStoreMock.loadSettings.mockReset().mockResolvedValue({ themeMode: "system" });
  settingsStoreMock.updateSettings.mockReset().mockResolvedValue(undefined);
  document.documentElement.removeAttribute("style");
  delete document.documentElement.dataset.themeScheme;
  delete document.documentElement.dataset.themeName;
});

afterEach(() => {
  vi.unstubAllGlobals();
});

describe("theme-engine", () => {
  it("resolves 'system' mode to the dark built-in theme when the OS prefers dark", async () => {
    mockMatchMedia(true);
    const { initTheme, getResolvedThemeName } = await import("./theme-engine");
    await initTheme();
    expect(getResolvedThemeName()).toBe("farm3d-dark");
    expect(document.documentElement.dataset.themeScheme).toBe("dark");
  });

  it("resolves 'system' mode to the light built-in theme when the OS prefers light", async () => {
    mockMatchMedia(false);
    const { initTheme, getResolvedThemeName } = await import("./theme-engine");
    await initTheme();
    expect(getResolvedThemeName()).toBe("farm3d-light");
    expect(document.documentElement.dataset.themeScheme).toBe("light");
  });

  it("persists an explicit mode and applies it immediately", async () => {
    mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    await initTheme();
    setThemeMode("farm3d-dark");
    expect(getResolvedThemeName()).toBe("farm3d-dark");
    expect(settingsStoreMock.updateSettings).toHaveBeenCalledWith({ themeMode: "farm3d-dark" });
    expect(document.documentElement.style.getPropertyValue("--f3d-color-accent")).not.toBe("");
  });

  it("reads a persisted mode on init, overriding the current OS preference", async () => {
    settingsStoreMock.loadSettings.mockResolvedValue({ themeMode: "farm3d-dark" });
    mockMatchMedia(false); // OS says light, but a dark mode was explicitly persisted
    const { initTheme, getResolvedThemeName } = await import("./theme-engine");
    await initTheme();
    expect(getResolvedThemeName()).toBe("farm3d-dark");
  });

  it("lets a plugin register and activate a custom theme", async () => {
    mockMatchMedia(false);
    const { initTheme, registerTheme, setThemeMode, getResolvedThemeName } = await import(
      "./theme-engine"
    );
    await initTheme();
    registerTheme(makeTheme("harvest", "dark", "#ff8800"));
    setThemeMode("harvest");
    expect(getResolvedThemeName()).toBe("harvest");
    expect(document.documentElement.style.getPropertyValue("--f3d-color-accent")).toBe(
      "#ff8800",
    );
  });

  it("falls back to farm3d-light for an unregistered theme name", async () => {
    mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    await initTheme();
    setThemeMode("does-not-exist");
    expect(getResolvedThemeName()).toBe("farm3d-light");
  });

  it("notifies subscribers when the OS preference changes in 'system' mode", async () => {
    const media = mockMatchMedia(false);
    const { initTheme, getResolvedThemeName, onThemeChange } = await import("./theme-engine");
    await initTheme();
    const seen: string[] = [];
    onThemeChange((name) => seen.push(name));

    media.setDark(true);

    expect(getResolvedThemeName()).toBe("farm3d-dark");
    expect(seen).toEqual(["farm3d-dark"]);
  });

  it("ignores OS preference changes once an explicit mode is set", async () => {
    const media = mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    await initTheme();
    setThemeMode("farm3d-light");

    media.setDark(true);

    expect(getResolvedThemeName()).toBe("farm3d-light");
  });
});
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `npx vitest run src/design-system/theme-engine.test.ts`
Expected: FAIL — `theme-engine.ts` still imports from `localStorage`/has no
`../settings/settings-store` import, so the mock isn't consumed and
`updateSettings` assertions fail (or the module still exports a
synchronous `initTheme`, which is harmless to await but the persistence
assertions will fail since nothing calls `updateSettings`).

- [ ] **Step 3: Update `src/design-system/theme-engine.ts`**

In the imports at the top of the file, add:

```typescript
import { loadSettings, updateSettings } from "../settings/settings-store";
```

Remove the `STORAGE_KEY` constant entirely:

```typescript
const STORAGE_KEY = "farm3d.theme-mode";
```

Replace `setThemeMode`:

```typescript
/** Sets the active theme mode ('system', or a registered theme's name) and persists the choice. */
export function setThemeMode(mode: ThemeMode): void {
  currentMode = mode;
  applyCurrentMode();
  updateSettings({ themeMode: mode }).catch((error) => {
    console.error("Failed to persist theme mode:", error);
  });
}
```

(Applying and notifying happens synchronously, before the persist call, so
the UI updates instantly even if the write to disk is slow or fails.)

Replace `initTheme`:

```typescript
/** Applies the persisted (or default 'system') theme mode. Call once at app startup, before render. */
export async function initTheme(): Promise<void> {
  registerTheme(lightTheme);
  registerTheme(darkTheme);

  const settings = await loadSettings();
  currentMode = settings.themeMode || "system";
  applyCurrentMode();

  if (!mediaQueryListenerAttached) {
    window
      .matchMedia("(prefers-color-scheme: dark)")
      .addEventListener("change", handleSystemPreferenceChange);
    mediaQueryListenerAttached = true;
  }
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `npx vitest run src/design-system/theme-engine.test.ts`
Expected: PASS — all 8 tests green.

- [ ] **Step 5: Update `src/index.tsx` to await the now-async `initTheme()`**

Change:

```typescript
initTheme();
```

to:

```typescript
await initTheme();
```

- [ ] **Step 6: Update the theme-engine section of `DESIGN.md`**

Replace:

```markdown
## Theme engine (`theme-engine.ts`)

- `initTheme()` — call once at startup, before render. Registers the two
  built-ins, applies the persisted (or default `'system'`) mode, and
  attaches an OS-preference-change listener.
- `registerTheme(theme: Theme)` — **the plugin point.** Anyone can construct
  an object matching the `Theme` interface and register it; it becomes
  selectable by name just like the built-ins.
- `setThemeMode(mode)` — `'system'` (follows `prefers-color-scheme`, live)
  or any registered theme's `name`. Persists to `localStorage`.
- `useTheme()` — a SolidJS primitive (`src/design-system/use-theme.ts`)
  exposing `mode()`, `resolvedThemeName()`, `setThemeMode()`, and
  `availableThemes()` reactively.
```

with:

```markdown
## Theme engine (`theme-engine.ts`)

- `initTheme()` — **async**; call once at startup, before render, and
  `await` it. Registers the two built-ins, loads the persisted (or
  default `'system'`) mode from the settings file
  (`src/settings/settings-store.ts`), applies it, and attaches an
  OS-preference-change listener.
- `registerTheme(theme: Theme)` — **the plugin point.** Anyone can construct
  an object matching the `Theme` interface and register it; it becomes
  selectable by name just like the built-ins.
- `setThemeMode(mode)` — `'system'` (follows `prefers-color-scheme`, live)
  or any registered theme's `name`. Applies immediately; persists to the
  OS-standard settings file in the background (see
  `src/settings/settings-store.ts`), not `localStorage`.
- `useTheme()` — a SolidJS primitive (`src/design-system/use-theme.ts`)
  exposing `mode()`, `resolvedThemeName()`, `setThemeMode()`, and
  `availableThemes()` reactively.
```

- [ ] **Step 7: Run the full frontend test suite and typecheck to confirm nothing else broke**

Run: `just test && just build`
Expected: both succeed.

- [ ] **Step 8: Commit**

```bash
git add src/design-system/theme-engine.ts src/design-system/theme-engine.test.ts src/index.tsx DESIGN.md
git commit -m "Persist theme mode through the settings store instead of localStorage"
```

---

## Task 4: Gear-icon settings menu

**Files:**
- Create: `src/screens/SettingsMenu.tsx`
- Create: `src/screens/SettingsMenu.module.css`
- Create: `src/screens/SettingsMenu.test.tsx`
- Delete: `src/screens/ThemeMenu.tsx`
- Delete: `src/screens/ThemeMenu.module.css`
- Modify: `src/screens/ActivityBar.tsx`

**Interfaces:**
- Consumes: `useTheme()` from `../design-system` (unchanged, Task 3), `openSettingsFile` from `../settings/settings-store` (Task 2), `DropdownMenu` from `../design-system` (unchanged).
- Produces: `SettingsMenu` component, named export, used by `ActivityBar.tsx`.

- [ ] **Step 1: Write the failing test — create `src/screens/SettingsMenu.test.tsx`**

Colocated with the component, matching this repo's existing test placement
(`theme-engine.test.ts` next to `theme-engine.ts`, `tabler-icons.test.tsx`
next to its source, etc.) rather than bolting onto
`design-system/components/components.test.tsx`, which only covers
`design-system/components/` itself.

```tsx
import { fireEvent, render, screen } from "@solidjs/testing-library";
import { afterEach, describe, expect, it } from "vitest";
import { SettingsMenu } from "./SettingsMenu";

afterEach(() => {
  document.body.innerHTML = "";
});

describe("SettingsMenu", () => {
  it("lists theme options and an 'Open settings file' action", async () => {
    render(() => <SettingsMenu />);

    await fireEvent.pointerDown(screen.getByLabelText("Settings"), {
      pointerType: "mouse",
      button: 0,
    });

    expect(await screen.findByText("System")).toBeInTheDocument();
    expect(screen.getByText("Light")).toBeInTheDocument();
    expect(screen.getByText("Dark")).toBeInTheDocument();
    expect(screen.getByText("Open settings file")).toBeInTheDocument();
  });
});
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `npx vitest run src/screens/SettingsMenu.test.tsx`
Expected: FAIL — `Failed to resolve import "./SettingsMenu"` (the component doesn't exist yet).

- [ ] **Step 3: Create `src/screens/SettingsMenu.module.css`**

```css
.trigger {
  display: inline-flex;
  align-items: center;
  justify-content: center;
  width: 1.75rem;
  height: 1.75rem;
  border-radius: var(--f3d-radius-sm);
  color: var(--f3d-color-text-muted);
  cursor: pointer;
  transition: background-color 0.12s, color 0.12s;
}

.trigger:hover {
  background-color: var(--f3d-color-surface-hover);
  color: var(--f3d-color-text);
}
```

- [ ] **Step 4: Create `src/screens/SettingsMenu.tsx`**

```tsx
import { IconSettings } from "@tabler/icons-solidjs";
import { DropdownMenu, useTheme, type ThemeMode } from "../design-system";
import { openSettingsFile } from "../settings/settings-store";
import styles from "./SettingsMenu.module.css";

const MODES: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "farm3d-light", label: "Light" },
  { value: "farm3d-dark", label: "Dark" },
];

function handleOpenSettingsFile() {
  void openSettingsFile().catch((error) => {
    console.error("Failed to open settings file:", error);
  });
}

/** Compact settings entry point for the activity bar — theme selection plus opening the settings file. */
export function SettingsMenu() {
  const theme = useTheme();

  return (
    <DropdownMenu
      trigger={
        <span class={styles.trigger} aria-label="Settings">
          <IconSettings size={18} />
        </span>
      }
      items={[
        ...MODES.map((mode) => ({
          label: mode.label,
          onSelect: () => theme.setThemeMode(mode.value),
        })),
        { type: "separator" as const },
        { label: "Open settings file", onSelect: handleOpenSettingsFile },
      ]}
    />
  );
}
```

- [ ] **Step 5: Delete the old `ThemeMenu` files**

```bash
git rm src/screens/ThemeMenu.tsx src/screens/ThemeMenu.module.css
```

- [ ] **Step 6: Update `src/screens/ActivityBar.tsx` to use `SettingsMenu`**

Change the import:

```typescript
import { ThemeMenu } from "./ThemeMenu";
```

to:

```typescript
import { SettingsMenu } from "./SettingsMenu";
```

And change the usage:

```tsx
<ThemeMenu />
```

to:

```tsx
<SettingsMenu />
```

- [ ] **Step 7: Run the test to verify it passes**

Run: `npx vitest run src/screens/SettingsMenu.test.tsx`
Expected: PASS.

- [ ] **Step 8: Run the full frontend test suite and build to confirm nothing else broke**

Run: `just test && just build`
Expected: both succeed (no remaining references to `ThemeMenu` anywhere —
double-check with `grep -rn "ThemeMenu" src` if either command fails
unexpectedly).

- [ ] **Step 9: Commit**

```bash
git add src/screens/SettingsMenu.tsx src/screens/SettingsMenu.module.css src/screens/SettingsMenu.test.tsx src/screens/ActivityBar.tsx
git commit -m "Replace ThemeMenu with a gear-icon SettingsMenu that can also open the settings file"
```

---

## Task 5: Full verification pass

**Files:** none (verification only).

- [ ] **Step 1: Run the full frontend suite**

Run: `just build && just test`
Expected: both pass.

- [ ] **Step 2: Run the full Rust suite**

Run: `source "$HOME/.cargo/env" && just test-rust`
Expected: all tests pass.

- [ ] **Step 3: Confirm no stray references to the old theme-only naming remain**

Run: `grep -rn "ThemeMenu\|farm3d.theme-mode" src DESIGN.md`
Expected: no matches.

- [ ] **Step 4: Visually confirm the gear menu in a real browser session**

Start the dev server (`npm run dev -- --port 1420` in the background), open
it in a browser (headless Chrome via chrome-devtools-mcp is fine if no
display is available — see prior sessions in this repo for the exact
flatpak/remote-debugging invocation), and confirm:
- The activity bar shows a gear icon where the theme icon used to be.
- Clicking it opens a menu with System/Light/Dark plus a separator and
  "Open settings file".
- Selecting a theme changes the app's appearance immediately.

Under `just web` (no Tauri backend), clicking "Open settings file" should
not throw or log an error to the browser console — confirm via
`read_console_messages` or equivalent if using browser automation.

Note: under real Tauri (`just dev`), the first theme change or menu open
should create `settings.json` in the OS-standard config dir (e.g.
`~/.config/com.jroche.farm3d/settings.json` on Linux) — this can only be
verified with a display available for `tauri dev`, not in headless/web-only
verification.
