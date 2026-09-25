import { createRoot, createEffect } from "solid-js";
import { describe, expect, it } from "vitest";
import {
  closeSlicerSettings,
  openSlicerSettings,
  registerSlicerSettingsHome,
  slicerSettingsOpen,
  slicerSettingsReturnFocus,
} from "./slicer-settings-opener";

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

  it("returns focus to the opener while it is in the document, otherwise to the registered home", () => {
    const home = document.body.appendChild(document.createElement("button"));
    const link = document.body.appendChild(document.createElement("button"));
    const unregister = registerSlicerSettingsHome(home);
    openSlicerSettings(link);
    expect(slicerSettingsReturnFocus()).toBe(link);
    link.remove();
    expect(slicerSettingsReturnFocus()).toBe(home);
    openSlicerSettings();
    expect(slicerSettingsReturnFocus()).toBe(home);
    unregister();
    expect(slicerSettingsReturnFocus()).toBeUndefined();
    closeSlicerSettings();
    home.remove();
  });
});
