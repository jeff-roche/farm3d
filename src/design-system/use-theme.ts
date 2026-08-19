import { createSignal, onCleanup } from "solid-js";
import {
  getRegisteredThemes,
  getResolvedThemeName,
  getThemeMode,
  onThemeChange,
  setThemeMode as setThemeModeEngine,
  type ThemeMode,
} from "./theme-engine";

/** Reactive access to the active theme; theme-engine's initTheme() must have already run. */
export function useTheme() {
  const [resolvedThemeName, setResolvedThemeName] = createSignal(getResolvedThemeName());
  const [mode, setMode] = createSignal<ThemeMode>(getThemeMode());

  const unsubscribe = onThemeChange(setResolvedThemeName);
  onCleanup(unsubscribe);

  function setThemeMode(next: ThemeMode) {
    setThemeModeEngine(next);
    setMode(next);
  }

  return {
    mode,
    resolvedThemeName,
    setThemeMode,
    availableThemes: getRegisteredThemes,
  };
}
