import { DropdownMenu, useTheme, type ThemeMode } from "../design-system";
import styles from "./ThemeMenu.module.css";

const MODES: { value: ThemeMode; label: string }[] = [
  { value: "system", label: "System" },
  { value: "farm3d-light", label: "Light" },
  { value: "farm3d-dark", label: "Dark" },
];

/** Compact theme switcher for a top bar — an icon trigger instead of a labeled Select. */
export function ThemeMenu() {
  const theme = useTheme();

  return (
    <DropdownMenu
      trigger={
        <span class={styles.trigger} aria-label="Change theme">
          ◐
        </span>
      }
      items={MODES.map((mode) => ({
        label: mode.label,
        onSelect: () => theme.setThemeMode(mode.value),
      }))}
    />
  );
}
