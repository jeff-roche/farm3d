/** An app-level request to show the Slicer settings (D22), so a screen
 *  deep inside the Library (the Preparation panel's **Open Slicer
 *  settings**) can open them without knowing where they live. The Settings
 *  surface renders its Slicer dialog from {@link slicerSettingsOpen} and
 *  reports closing with {@link closeSlicerSettings}. */
import { createSignal } from "solid-js";

const [open, setOpen] = createSignal(false);

/** Whether the Slicer settings are requested open. Reactive. */
export const slicerSettingsOpen = open;

/** Asks for the Slicer settings to open. */
export function openSlicerSettings(): void {
  setOpen(true);
}

/** The Slicer settings closed (or should close). */
export function closeSlicerSettings(): void {
  setOpen(false);
}
