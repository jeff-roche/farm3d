import { IconSettings } from "@tabler/icons-solidjs";
import { DropdownMenu, useTheme, type ThemeMode } from "../design-system";
import { openSettingsFile } from "../settings/settings-store";
import styles from "./SettingsMenu.module.css";

const MODES: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "farm3d-light", label: "Light" },
  { value: "farm3d-dark", label: "Dark" },
];

function handleOpenSettingsFile() {
  void openSettingsFile().catch((error) => {
    console.error("Failed to open settings file:", error);
  });
}

/** Compact settings entry point for the activity bar — theme selection plus opening the settings file. */
export function SettingsMenu() {
  const theme = useTheme();

  return (
    <DropdownMenu
      trigger={
        <span class={styles.trigger} aria-label="Settings">
          <IconSettings size={18} />
        </span>
      }
      items={[
        ...MODES.map((mode) => ({
          label: mode.label,
          onSelect: () => theme.setThemeMode(mode.value),
        })),
        { type: "separator" as const },
        { label: "Open settings file", onSelect: handleOpenSettingsFile },
      ]}
    />
  );
}
