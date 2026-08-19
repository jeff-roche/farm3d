import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import type { Theme } from "./tokens/types";
import { editorTypography } from "./tokens/typography";
import { editorShape } from "./tokens/shape";

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
    typography: editorTypography,
    shape: editorShape,
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
    expect(getResolvedThemeName()).toBe("editor-dark");
    expect(document.documentElement.dataset.themeScheme).toBe("dark");
  });

  it("resolves 'system' mode to the light built-in theme when the OS prefers light", async () => {
    mockMatchMedia(false);
    const { initTheme, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    expect(getResolvedThemeName()).toBe("editor-light");
    expect(document.documentElement.dataset.themeScheme).toBe("light");
  });

  it("persists an explicit mode and applies it immediately", async () => {
    mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    setThemeMode("editor-dark");
    expect(getResolvedThemeName()).toBe("editor-dark");
    expect(localStorage.getItem("farm3d.theme-mode")).toBe("editor-dark");
    expect(document.documentElement.style.getPropertyValue("--f3d-color-accent")).not.toBe("");
  });

  it("reads a persisted mode on init, overriding the current OS preference", async () => {
    localStorage.setItem("farm3d.theme-mode", "editor-dark");
    mockMatchMedia(false); // OS says light, but a dark mode was explicitly persisted
    const { initTheme, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    expect(getResolvedThemeName()).toBe("editor-dark");
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
    expect(document.documentElement.style.getPropertyValue("--f3d-color-accent")).toBe(
      "#ff8800",
    );
  });

  it("falls back to editor-light for an unregistered theme name", async () => {
    mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    setThemeMode("does-not-exist");
    expect(getResolvedThemeName()).toBe("editor-light");
  });

  it("notifies subscribers when the OS preference changes in 'system' mode", async () => {
    const media = mockMatchMedia(false);
    const { initTheme, getResolvedThemeName, onThemeChange } = await import("./theme-engine");
    initTheme();
    const seen: string[] = [];
    onThemeChange((name) => seen.push(name));

    media.setDark(true);

    expect(getResolvedThemeName()).toBe("editor-dark");
    expect(seen).toEqual(["editor-dark"]);
  });

  it("ignores OS preference changes once an explicit mode is set", async () => {
    const media = mockMatchMedia(false);
    const { initTheme, setThemeMode, getResolvedThemeName } = await import("./theme-engine");
    initTheme();
    setThemeMode("editor-light");

    media.setDark(true);

    expect(getResolvedThemeName()).toBe("editor-light");
  });
});
