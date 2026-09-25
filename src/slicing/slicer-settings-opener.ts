/** An app-level request to show the Slicer settings (D22), so a screen
 *  deep inside the Library (the Preparation panel's **Open Slicer
 *  settings**) can open them without knowing where they live. The Settings
 *  surface renders its Slicer dialog from {@link slicerSettingsOpen} and
 *  reports closing with {@link closeSlicerSettings}. */
import { createSignal } from "solid-js";

const [open, setOpen] = createSignal(false);

/** The control that asked for the settings, and the Settings menu's own
 *  button, which is always there to fall back on. */
let openedFrom: HTMLElement | undefined;
let home: HTMLElement | undefined;

/** Whether the Slicer settings are requested open. Reactive. */
export const slicerSettingsOpen = open;

/** Asks for the Slicer settings to open. `from` is the control that asked,
 *  where focus returns when they close. */
export function openSlicerSettings(from?: HTMLElement): void {
  openedFrom = from;
  setOpen(true);
}

/** The Slicer settings closed (or should close). */
export function closeSlicerSettings(): void {
  setOpen(false);
}

/** The Settings menu's button registers itself as the place focus returns
 *  to when the control that opened the settings has gone. Returns the
 *  unregistration. */
export function registerSlicerSettingsHome(element: HTMLElement): () => void {
  home = element;
  return () => {
    if (home === element) home = undefined;
  };
}

/** Where focus goes when the Slicer settings close: the control that
 *  opened them while it is still in the document, otherwise the Settings
 *  menu's button. */
export function slicerSettingsReturnFocus(): HTMLElement | undefined {
  if (openedFrom?.isConnected) return openedFrom;
  return home?.isConnected ? home : undefined;
}
