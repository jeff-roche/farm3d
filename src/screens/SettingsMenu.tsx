import { IconSettings } from "@tabler/icons-solidjs";
import { createSignal, onCleanup, onMount } from "solid-js";
import { DropdownMenu } from "../design-system";
import { exportSettings, importSettings } from "../settings/settings-store";
import { openSlicerSettings, registerSlicerSettingsHome } from "../slicing/slicer-settings-opener";
import { ThemePopover } from "./ThemePopover";
import styles from "./SettingsMenu.module.css";

/** Compact settings entry point for the activity bar — opens the theme picker, the Slicer settings, or the settings file. */
export function SettingsMenu() {
  let triggerRef: HTMLSpanElement | undefined;
  const [themePopoverOpen, setThemePopoverOpen] = createSignal(false);
  const [operationError, setOperationError] = createSignal<string | null>(null);

  function run<T>(operation: () => Promise<T>) {
    setOperationError(null);
    void operation().catch((error) => setOperationError(String(error)));
  }

  // The DropdownMenu item's own pointer-up/click (which closes the menu) is still
  // being processed when onSelect fires; opening the Popover synchronously means its
  // outside-click detection sees that same click and immediately closes it again.
  // Deferring past the current event tick avoids that race.
  function openThemePopover() {
    setTimeout(() => setThemePopoverOpen(true), 0);
  }

  // The same race applies to the Slicer settings dialog, which the app
  // renders from the opener (SlicerSettingsHost).
  function openSlicer() {
    setTimeout(() => openSlicerSettings(menuButton()), 0);
  }

  // The focusable menu button is Kobalte's trigger around the icon. It is
  // also where focus returns from the Slicer settings when whatever opened
  // them has gone.
  const menuButton = () => triggerRef?.closest<HTMLElement>("[aria-haspopup]") ?? undefined;
  onMount(() => {
    const button = menuButton();
    if (button) onCleanup(registerSlicerSettingsHome(button));
  });

  return (
    <>
      <DropdownMenu
        trigger={
          <span ref={triggerRef} class={styles.trigger} aria-label="Settings">
            <IconSettings size={18} />
          </span>
        }
        items={[
          { label: "Theme...", onSelect: openThemePopover },
          { label: "Slicer...", onSelect: openSlicer },
          { type: "separator" as const },
          { label: "Export settings...", onSelect: () => run(exportSettings) },
          { label: "Import settings...", onSelect: () => run(importSettings) },
        ]}
      />
      {operationError() ? <span role="alert">{operationError()}</span> : null}
      <ThemePopover
        open={themePopoverOpen()}
        onOpenChange={setThemePopoverOpen}
        anchorRef={() => triggerRef}
      />
    </>
  );
}
