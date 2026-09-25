/** The viewport's colours come from the design tokens, never literals, so
 *  a theme switch repaints the 3D view with the rest of the app. */
import type { ViewportTheme } from "./renderer";

/** Which `--f3d-color-*` role each viewport colour uses. */
export const VIEWPORT_THEME_TOKENS: Record<keyof ViewportTheme, string> = {
  background: "--f3d-color-bg",
  grid: "--f3d-color-border",
  volume: "--f3d-color-border-strong",
  object: "--f3d-color-text-muted",
  selected: "--f3d-color-accent",
  outOfBounds: "--f3d-color-danger",
  excludeArea: "--f3d-color-warning",
  measure: "--f3d-color-text",
};

/** Reads the tokens as they apply to `element` (they are set on the root
 *  and inherited). */
export function readViewportTheme(element: Element): ViewportTheme {
  const style = getComputedStyle(element);
  const read = (token: string) => style.getPropertyValue(token).trim();
  const theme = {} as ViewportTheme;
  for (const role of Object.keys(VIEWPORT_THEME_TOKENS) as (keyof ViewportTheme)[]) {
    theme[role] = read(VIEWPORT_THEME_TOKENS[role]);
  }
  return theme;
}
