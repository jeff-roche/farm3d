import { Dialog } from "../design-system";
import { slicerSettingsReturnFocus } from "../slicing/slicer-settings-opener";
import { SlicerSettingsForm } from "./settings/SlicerSettingsForm";

export interface SlicerSettingsDialogProps {
  open: boolean;
  onOpenChange: (open: boolean) => void;
}

/** The Slicer settings as a dialog, for the Preparation panel's **Open
 *  Slicer settings** (D22); the Settings workspace hosts the same form
 *  inline. Focus returns to whatever opened it. */
export function SlicerSettingsDialog(props: SlicerSettingsDialogProps) {
  return (
    <Dialog
      title="Slicer"
      description="The OrcaSlicer that farm3d slices with, and where its presets come from."
      open={props.open}
      onOpenChange={props.onOpenChange}
      returnFocus={slicerSettingsReturnFocus}
    >
      <SlicerSettingsForm onClose={() => props.onOpenChange(false)} />
    </Dialog>
  );
}
