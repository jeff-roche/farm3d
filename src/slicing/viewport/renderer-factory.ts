/** Loads the three.js renderer on first use, so three.js stays out of the
 *  main bundle. `vitest.setup.ts` replaces this module with the fake, so
 *  jsdom never loads WebGL. */
import type { ViewportRenderer } from "./renderer";

export async function createViewportRenderer(): Promise<ViewportRenderer> {
  const { ThreeViewportRenderer } = await import("./three-renderer");
  return new ThreeViewportRenderer();
}
