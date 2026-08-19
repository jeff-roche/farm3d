import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Theme } from "./tokens/types";
import { m3Typography } from "./tokens/typography";
import { m3Shape } from "./tokens/shape";

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

function makeTheme(name: string, scheme: "light" | "dark", primary: string): Theme {
  return {
    name,
    scheme,
    color: {
      primary,
      onPrimary: "#000000",
      primaryContainer: "#000000",
      onPrimaryContainer: "#000000",
      secondary: "#000000",
      onSecondary: "#000000",
      secondaryContainer: "#000000",
      onSecondaryContainer: "#000000",
      tertiary: "#000000",
      onTertiary: "#000000",
      tertiaryContainer: "#000000",
      onTertiaryContainer: "#000000",
      error: "#000000",
      onError: "#000000",
      errorContainer: "#000000",
      onErrorContainer: "#000000",
      background: "#000000",
      onBackground: "#000000",
      surface: "#000000",
      onSurface: "#000000",
      surfaceVariant: "#000000",
      onSurfaceVariant: "#000000",
      outline: "#000000",
      outlineVariant: "#000000",
      shadow: "#000000",
      scrim: "#000000",
      inverseSurface: "#000000",
      inverseOnSurface: "#000000",
      inversePrimary: "#000000",
      surfaceDim: "#000000",
      surfaceBright: "#000000",
      surfaceContainerLowest: "#000000",
      surfaceContainerLow: "#000000",
      surfaceContainer: "#000000",
      surfaceContainerHigh: "#000000",
      surfaceContainerHighest: "#000000",
      surfaceTint: "#000000",
    },
    typography: m3Typography,
    shape: m3Shape,
  };
}

beforeEach(() => {
  vi.resetModules();
  localStorage.clear();
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
    initTheme();
    expect(getResolvedThemeName()).toBe("material-dark");
    expect(document.documentElement.dataset.themeScheme).toBe("dark");
  });

  it("resolves 'system' mode to the light built-in theme when the OS prefers light", async () => {
    mockMatchMedia(false);
    const { initTheme, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    expect(getResolvedThemeName()).toBe("material-light");
    expect(document.documentElement.dataset.themeScheme).toBe("light");
  });

  it("persists an explicit mode and applies it immediately", async () => {
    mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    setThemeMode("material-dark");
    expect(getResolvedThemeName()).toBe("material-dark");
    expect(localStorage.getItem("farm3d.theme-mode")).toBe("material-dark");
    expect(document.documentElement.style.getPropertyValue("--md-sys-color-primary")).not.toBe("");
  });

  it("reads a persisted mode on init, overriding the current OS preference", async () => {
    localStorage.setItem("farm3d.theme-mode", "material-dark");
    mockMatchMedia(false); // OS says light, but a dark mode was explicitly persisted
    const { initTheme, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    expect(getResolvedThemeName()).toBe("material-dark");
  });

  it("lets a plugin register and activate a custom theme", async () => {
    mockMatchMedia(false);
    const { initTheme, registerTheme, setThemeMode, getResolvedThemeName } = await import(
      "./theme-engine"
    );
    initTheme();
    registerTheme(makeTheme("harvest", "dark", "#ff8800"));
    setThemeMode("harvest");
    expect(getResolvedThemeName()).toBe("harvest");
    expect(document.documentElement.style.getPropertyValue("--md-sys-color-primary")).toBe(
      "#ff8800",
    );
  });

  it("falls back to material-light for an unregistered theme name", async () => {
    mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    setThemeMode("does-not-exist");
    expect(getResolvedThemeName()).toBe("material-light");
  });

  it("notifies subscribers when the OS preference changes in 'system' mode", async () => {
    const media = mockMatchMedia(false);
    const { initTheme, getResolvedThemeName, onThemeChange } = await import("./theme-engine");
    initTheme();
    const seen: string[] = [];
    onThemeChange((name) => seen.push(name));

    media.setDark(true);

    expect(getResolvedThemeName()).toBe("material-dark");
    expect(seen).toEqual(["material-dark"]);
  });

  it("ignores OS preference changes once an explicit mode is set", async () => {
    const media = mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    setThemeMode("material-light");

    media.setDark(true);

    expect(getResolvedThemeName()).toBe("material-light");
  });
});
