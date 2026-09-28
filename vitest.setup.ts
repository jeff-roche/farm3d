import "@testing-library/jest-dom/vitest";
import { vi } from "vitest";

// jsdom has no WebGL: every viewport gets the recording fake renderer
// instead of three.js (see src/slicing/viewport/fake-renderer.ts).
vi.mock("./src/slicing/viewport/renderer-factory", async () => {
  const { createFakeViewportRenderer } = await import("./src/slicing/viewport/fake-renderer");
  return { createViewportRenderer: createFakeViewportRenderer };
});

// jsdom doesn't implement these; Kobalte's positioning/overlay logic needs them.
if (typeof ResizeObserver === "undefined") {
  class ResizeObserverStub {
    observe() {}
    unobserve() {}
    disconnect() {}
  }
  // @ts-expect-error -- test-environment polyfill
  globalThis.ResizeObserver = ResizeObserverStub;
}

if (!Element.prototype.hasPointerCapture) {
  Element.prototype.hasPointerCapture = () => false;
}
if (!Element.prototype.setPointerCapture) {
  Element.prototype.setPointerCapture = () => {};
}
if (!Element.prototype.releasePointerCapture) {
  Element.prototype.releasePointerCapture = () => {};
}
if (!Element.prototype.scrollIntoView) {
  Element.prototype.scrollIntoView = () => {};
}

// jsdom doesn't implement object URLs; `src/cameras/frame.ts`'s
// `frameObjectUrl` (and `usePreview`, which calls it once per polled frame)
// needs them. A counter keeps each URL distinct without a real Blob store.
if (typeof URL.createObjectURL === "undefined") {
  let objectUrlCount = 0;
  URL.createObjectURL = () => `blob:mock-${(objectUrlCount += 1)}`;
  URL.revokeObjectURL = () => {};
}

// jsdom doesn't implement matchMedia; theme-engine's system-preference detection needs it.
// Tests that care about a specific light/dark preference (e.g. theme-engine.test.ts) stub
// this themselves with vi.stubGlobal, which takes precedence over this default.
if (typeof window.matchMedia === "undefined") {
  window.matchMedia = (query: string) => ({
    matches: false,
    media: query,
    onchange: null,
    addListener: () => {},
    removeListener: () => {},
    addEventListener: () => {},
    removeEventListener: () => {},
    dispatchEvent: () => false,
  });
}
