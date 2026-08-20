import type { ColorRoles, Theme, TypeStyle } from "./tokens/types";
import { lightTheme } from "./themes/light";
import { darkTheme } from "./themes/dark";
import { loadSettings, updateSettings } from "../settings/settings-store";

/** 'system' follows the OS light/dark preference; any other value is a registered Theme's name. */
export type ThemeMode = "system" | string;

const registry = new Map<string, Theme>();
const listeners = new Set<(resolvedThemeName: string) => void>();

function typeStyleToCssVars(kebabRole: string, style: TypeStyle): [string, string][] {
  return [
    [`--f3d-type-${kebabRole}-font`, style.fontFamily],
    [`--f3d-type-${kebabRole}-weight`, String(style.fontWeight)],
    [`--f3d-type-${kebabRole}-size`, style.fontSize],
    [`--f3d-type-${kebabRole}-line-height`, style.lineHeight],
    [`--f3d-type-${kebabRole}-tracking`, style.letterSpacing],
  ];
}

function camelToKebab(s: string): string {
  return s.replace(/[A-Z]/g, (c) => `-${c.toLowerCase()}`);
}

/** Registers a theme, making it selectable via setThemeMode(theme.name). Plugin authors call this. */
export function registerTheme(theme: Theme): void {
  registry.set(theme.name, theme);
}

export function getRegisteredThemes(): Theme[] {
  return Array.from(registry.values());
}

function prefersDark(): boolean {
  return window.matchMedia("(prefers-color-scheme: dark)").matches;
}

function resolveTheme(mode: ThemeMode): Theme {
  if (mode === "system") {
    const fallback = prefersDark() ? "farm3d-dark" : "farm3d-light";
    return registry.get(fallback) ?? lightTheme;
  }
  return registry.get(mode) ?? lightTheme;
}

function applyTheme(theme: Theme): void {
  const root = document.documentElement;

  for (const [role, value] of Object.entries(theme.color) as [
    keyof ColorRoles,
    string,
  ][]) {
    root.style.setProperty(`--f3d-color-${camelToKebab(role)}`, value);
  }

  for (const [role, style] of Object.entries(theme.typography)) {
    for (const [prop, value] of typeStyleToCssVars(camelToKebab(role), style)) {
      root.style.setProperty(prop, value);
    }
  }

  for (const [role, value] of Object.entries(theme.shape)) {
    root.style.setProperty(`--f3d-radius-${camelToKebab(role)}`, value);
  }

  root.dataset.themeScheme = theme.scheme;
  root.dataset.themeName = theme.name;
}

let currentMode: ThemeMode = "system";
let mediaQueryListenerAttached = false;

function notify(): void {
  const resolvedName = resolveTheme(currentMode).name;
  for (const listener of listeners) listener(resolvedName);
}

function applyCurrentMode(): void {
  applyTheme(resolveTheme(currentMode));
  notify();
}

function handleSystemPreferenceChange(): void {
  if (currentMode === "system") applyCurrentMode();
}

/** Sets the active theme mode ('system', or a registered theme's name) and persists the choice. */
export function setThemeMode(mode: ThemeMode): void {
  currentMode = mode;
  applyCurrentMode();
  updateSettings({ themeMode: mode }).catch((error) => {
    console.error("Failed to persist theme mode:", error);
  });
}

export function getThemeMode(): ThemeMode {
  return currentMode;
}

export function getResolvedThemeName(): string {
  return resolveTheme(currentMode).name;
}

/** Subscribes to theme changes (mode changes or, in 'system' mode, OS preference changes). Returns an unsubscribe function. */
export function onThemeChange(listener: (resolvedThemeName: string) => void): () => void {
  listeners.add(listener);
  return () => listeners.delete(listener);
}

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
