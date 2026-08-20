import { IconSettings } from "@tabler/icons-solidjs";
import { createSignal } from "solid-js";
import { DropdownMenu } from "../design-system";
import { openSettingsFile } from "../settings/settings-store";
import { ThemePopover } from "./ThemePopover";
import styles from "./SettingsMenu.module.css";

function handleOpenSettingsFile() {
  void openSettingsFile().catch((error) => {
    console.error("Failed to open settings file:", error);
  });
}

/** Compact settings entry point for the activity bar — opens the theme picker or the settings file. */
export function SettingsMenu() {
  let triggerRef: HTMLSpanElement | undefined;
  const [themePopoverOpen, setThemePopoverOpen] = createSignal(false);

  // The DropdownMenu item's own pointer-up/click (which closes the menu) is still
  // being processed when onSelect fires; opening the Popover synchronously means its
  // outside-click detection sees that same click and immediately closes it again.
  // Deferring past the current event tick avoids that race.
  function openThemePopover() {
    setTimeout(() => setThemePopoverOpen(true), 0);
  }

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
          { type: "separator" as const },
          { label: "Open settings file", onSelect: handleOpenSettingsFile },
        ]}
      />
      <ThemePopover
        open={themePopoverOpen()}
        onOpenChange={setThemePopoverOpen}
        anchorRef={() => triggerRef}
      />
    </>
  );
}
