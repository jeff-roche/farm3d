import { createRoot, createEffect } from "solid-js";
import { describe, expect, it } from "vitest";
import { closeSlicerSettings, openSlicerSettings, slicerSettingsOpen } from "./slicer-settings-opener";

describe("slicer-settings-opener", () => {
  it("opens and closes, and tells whoever renders the settings", () => {
    const seen: boolean[] = [];
    const dispose = createRoot((dispose) => {
      createEffect(() => seen.push(slicerSettingsOpen()));
      return dispose;
    });
    openSlicerSettings();
    openSlicerSettings();
    closeSlicerSettings();
    expect(seen).toEqual([false, true, false]);
    expect(slicerSettingsOpen()).toBe(false);
    dispose();
  });
});
