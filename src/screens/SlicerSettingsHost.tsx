import { lazy, Show, Suspense } from "solid-js";
import { closeSlicerSettings, slicerSettingsOpen } from "../slicing/slicer-settings-opener";

// Loaded the first time the Slicer settings open, keeping the dialog out of
// the main chunk.
const SlicerSettingsDialog = lazy(() =>
  import("./SlicerSettingsDialog").then((m) => ({ default: m.SlicerSettingsDialog })));

/** Mounted once at app level: shows the Slicer settings (D22) whenever
 *  anything asks for them through `slicer-settings-opener`, whether the
 *  Settings menu or the Preparation panel's **Open Slicer settings**. */
export function SlicerSettingsHost() {
  return (
    <Show when={slicerSettingsOpen()}>
      <Suspense>
        <SlicerSettingsDialog
          open
          onOpenChange={(open) => {
            if (!open) closeSlicerSettings();
          }}
        />
      </Suspense>
    </Show>
  );
}
