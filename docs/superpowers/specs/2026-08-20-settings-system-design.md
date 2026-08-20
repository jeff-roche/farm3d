# Settings system: OS-standard settings file + gear menu

## Context

farm3d has exactly one user-configurable value today — theme mode
(`system` / `farm3d-light` / `farm3d-dark`) — and it's persisted to
`localStorage` inside `theme-engine.ts` (`src/design-system/theme-engine.ts`),
exposed via `ThemeMenu.tsx`'s dropdown in the activity bar (`◐` glyph
trigger).

`localStorage` lives inside the Tauri webview's storage partition, not
somewhere a user can find or hand-edit, and doesn't match how a desktop
app is expected to store configuration (an OS-standard config file). This
spec introduces a real settings file — written via Tauri's app-config-dir
resolver, so it lands in the OS-conventional location per platform (e.g.
`~/.config/com.jroche.farm3d/settings.json` on Linux,
`~/Library/Application Support/com.jroche.farm3d/settings.json` on macOS,
`%APPDATA%\com.jroche.farm3d\settings.json` on Windows, using the existing
`com.jroche.farm3d` identifier from `tauri.conf.json`) — and adds a gear
icon (replacing the theme icon in the activity bar) whose menu holds the
existing theme options plus a new "Open settings file" action.

Only one setting exists today (`themeMode`), but the schema and the
frontend/backend contract are designed to hold more settings later without
a shape change to the plumbing.

## Settled decisions

- Persistence is via two small custom Tauri commands (`load_settings`,
  `save_settings`) using `std::fs` + `serde_json`, not the official
  `@tauri-apps/plugin-fs` — avoids pulling in a broad file-access
  capability surface for a need this narrow, and keeps JSON shape/error
  handling in Rust where the file IO already happens.
- The gear icon **replaces** the current theme icon in the activity bar
  (same slot) rather than sitting alongside it — one settings entry point.
- The settings file becomes the **sole** source of truth for theme mode.
  `localStorage` persistence is removed entirely — no dual state to keep
  in sync.
- "Open settings file" opens the file in the OS-default editor for
  `.json` (via the `tauri-plugin-opener` dependency already in
  `Cargo.toml`), not a reveal-in-file-manager action.

## Backend (`src-tauri`)

### `Settings` struct

A new `src-tauri/src/settings.rs` module:

```rust
#[derive(Serialize, Deserialize, Default)]
#[serde(rename_all = "camelCase", default)]
pub struct Settings {
    pub theme_mode: String, // "system" | a registered theme name; empty string treated as "system" by the frontend default
}
```

`#[serde(default)]` at the struct level means a partial or empty JSON
object still deserializes instead of erroring. On-disk JSON reads
camelCase (`{"themeMode": "system"}`) — the file is meant to be
human-editable, and camelCase matches the frontend's `ThemeMode` type
naming.

### Commands

- `load_settings(app: AppHandle) -> Result<Settings, String>` — resolves
  `app.path().app_config_dir()`, creates the directory if missing. If
  `settings.json` doesn't exist, writes `Settings::default()` to it and
  returns that. If it exists but fails to parse, logs a warning and
  returns `Settings::default()` (never blocks app startup on a corrupt
  file). Otherwise reads + parses + returns.
- `save_settings(app: AppHandle, settings: Settings) -> Result<(), String>`
  — serializes with `serde_json::to_string_pretty` (readable for hand
  editing) and writes the full file, creating the config dir if needed.
- `open_settings_file(app: AppHandle) -> Result<(), String>` — ensures the
  file exists (same create-with-defaults-if-missing logic as
  `load_settings`), then calls `tauri_plugin_opener`'s `OpenerExt::opener()
  .open_path(path, None::<&str>)` to launch it in the OS-default editor.

All three are registered in `invoke_handler![...]` in `lib.rs`. The
existing unused `greet` placeholder command is removed (dead scaffold
code, not referenced anywhere in the frontend).

No `capabilities/default.json` changes are needed — these are plain
custom commands (no plugin permission gate beyond `core:default`), and
`opener:default` is already present for `open_settings_file`'s use of the
opener plugin.

## Frontend settings store (`src/settings/settings-store.ts`)

A new top-level `src/settings/` directory — a sibling of `design-system`,
not nested inside it, since this is app/OS integration (Tauri file IO),
not a themeable design-system primitive.

Framework-agnostic module, mirroring `theme-engine.ts`'s existing style
(plain functions + a module-level cache, no Solid dependency):

- `loadSettings(): Promise<Settings>` — under real Tauri (`isTauri()` from
  `@tauri-apps/api/core` is `true`), invokes `load_settings` and caches
  the result. Under `just web` (browser-only dev, no Rust backend),
  `isTauri()` is `false` and this resolves to an in-memory default
  (`{themeMode: "system"}`) without ever calling `invoke`.
- `getSettings(): Settings` — synchronous read of the last-loaded/cached
  settings; throws if called before `loadSettings()` has resolved once
  (mirrors `theme-engine.ts`'s existing "must call `initTheme()` first"
  contract for `getThemeMode()`/`getResolvedThemeName()`).
- `updateSettings(partial: Partial<Settings>): Promise<void>` — merges
  `partial` into the cached settings, updates the cache immediately
  (synchronous, optimistic), then persists: invokes `save_settings` under
  Tauri, or no-ops under web-dev.
- `openSettingsFile(): Promise<void>` — invokes `open_settings_file` under
  Tauri; no-ops under web-dev (no file exists to open there).

`Settings` (the TS type) is a hand-written mirror of the Rust struct:
`interface Settings { themeMode: ThemeMode }` (reusing the existing
`ThemeMode` type already exported from `theme-engine.ts`).

## `theme-engine.ts` changes

- `initTheme()` becomes `async`: awaits `loadSettings()` and uses
  `settings.themeMode || "system"` as the initial mode (falling back to
  `"system"` for an empty string, matching the Rust default) instead of
  reading `localStorage`.
- `setThemeMode(mode)` keeps its current synchronous behavior — apply the
  theme's CSS variables and notify listeners immediately, so the UI
  updates with no perceptible delay — but persistence changes from
  `localStorage.setItem(...)` to `updateSettings({themeMode: mode})`,
  called fire-and-forget (not awaited) with a `.catch(console.error)` so a
  failed disk write logs but never blocks or breaks the UI interaction.
- All direct `localStorage` access (the `STORAGE_KEY` constant and its
  two call sites) is removed from this file.
- `src/index.tsx` adds `await` before `initTheme()` — it already uses
  top-level `await` in the same file (for the `#showcase` dynamic
  import), so this is a one-line, low-risk change — ensuring the correct
  theme is applied before first render instead of flashing a default.
- `DESIGN.md`'s theme-engine section is updated to describe file-based
  persistence via the settings store instead of `localStorage`.

## UI: gear menu (`SettingsMenu.tsx`, renamed from `ThemeMenu.tsx`)

- Renamed to reflect its broadened scope. `ActivityBar.tsx` updates its
  import/usage accordingly.
- Trigger changes from the bespoke `◐` glyph span to Tabler's
  `IconSettings` (gear), sized/styled consistently with the other
  activity-bar icons (`IconPrinter`, `IconBox`) rather than the old
  one-off glyph.
- Menu items (via the existing `DropdownMenu` component, which already
  supports `separator` entries): the three existing theme options
  (System/Light/Dark, unchanged), then a `separator`, then **"Open
  settings file"**, which calls `settingsStore.openSettingsFile()`.
- Under `just web`, "Open settings file" remains visible but its handler
  resolves to a no-op (per the store's web fallback) — clicking it does
  nothing rather than throwing, consistent with `just web` being
  documented as frontend-only dev with no Tauri backend.

## Testing

- `theme-engine.test.ts`: every `initTheme()` call becomes
  `await initTheme()`; `src/settings/settings-store.ts` is mocked via
  `vi.mock` so these tests stay fast/deterministic and don't depend on
  Tauri or fs — same isolation style as the existing `matchMedia` mock.
- New `src/settings/settings-store.test.ts`: covers both branches —
  Tauri-present (mocking `@tauri-apps/api/core`'s `isTauri()` and
  `@tauri-apps/api/core`'s `invoke`) and web-fallback (`isTauri()` false).
- `SettingsMenu` gets a component test alongside the existing
  `DropdownMenu`-based component tests, using `fireEvent.pointerDown`/
  `fireEvent.pointerUp` (per the established Kobalte trigger/item
  convention documented in `DESIGN.md`), verifying the "Open settings
  file" item is present and invokes the store.
- Rust: `#[cfg(test)]` unit tests in `settings.rs` for `load_settings`/
  `save_settings` round-tripping through a temp dir, and for
  default-fallback behavior on a missing or corrupt file.

## Out of scope

- No settings UI form/panel (no in-app fields to edit values other than
  theme) — editing beyond theme happens by hand-editing the JSON file.
- No live file-watching — if a user hand-edits `settings.json` while the
  app is running, changes apply on next app launch, not live.
- No settings migration/versioning scheme — there's exactly one field
  today; a schema-version field can be added when a breaking change to
  the shape is actually needed.
