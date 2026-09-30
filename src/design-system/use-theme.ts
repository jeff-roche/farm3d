import { createSignal, onCleanup } from "solid-js";
import {
  cancelPreview,
  getRegisteredThemes,
  getResolvedThemeName,
  getThemeMode,
  onThemeChange,
  previewTheme,
  setThemeMode as setThemeModeEngine,
  type ThemeMode,
} from "./theme-engine";

/** Reactive access to the active theme; theme-engine's initTheme() must have already run. */
export function useTheme() {
  const [resolvedThemeName, setResolvedThemeName] = createSignal(getResolvedThemeName());
  const [mode, setMode] = createSignal<ThemeMode>(getThemeMode());

  // Any committed change (including one adopted from a settings import)
  // notifies, so the mode follows it; a preview never notifies.
  const unsubscribe = onThemeChange((name) => {
    setResolvedThemeName(name);
    setMode(getThemeMode());
  });
  onCleanup(unsubscribe);

  function setThemeMode(next: ThemeMode) {
    setThemeModeEngine(next);
    setMode(next);
  }

  return {
    mode,
    resolvedThemeName,
    setThemeMode,
    previewTheme,
    cancelPreview,
    availableThemes: getRegisteredThemes,
  };
}
