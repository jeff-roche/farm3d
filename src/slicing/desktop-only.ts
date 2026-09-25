import { needsDesktopError } from "../ipc/client";
import type { CommandError } from "../generated/contracts/command/CommandError";

/** The slicing actions web mode refuses (spec D23): they need OrcaSlicer, a
 *  G-code file, or the desktop's pickers. Shared by `slicing-store` and its
 *  test mock so both refuse identically. */
const DESKTOP_ONLY_ACTIONS = {
  startSlice: "Slicing",
  cancelSliceOperation: "Cancelling a slice",
  createExternalSliceRevision: "Creating a Slice Revision",
  pickSlicerEngine: "Choosing an OrcaSlicer engine",
  pickPresetSource: "Choosing a preset source",
  resetSlicerRuntime: "Resetting the slicer runtime",
} as const;

export type DesktopOnlyAction = keyof typeof DESKTOP_ONLY_ACTIONS;

export const desktopOnlyActions = Object.keys(DESKTOP_ONLY_ACTIONS) as DesktopOnlyAction[];

/** The app's `needsDesktop` refusal for one of these actions. */
export function desktopOnlyError(action: DesktopOnlyAction): CommandError {
  return needsDesktopError(DESKTOP_ONLY_ACTIONS[action]);
}
