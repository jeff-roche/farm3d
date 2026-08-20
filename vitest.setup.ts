import "@testing-library/jest-dom/vitest";

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
