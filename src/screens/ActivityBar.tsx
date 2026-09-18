import { IconBox, IconPrinter } from "@tabler/icons-solidjs";
import { IconButton } from "../design-system";
import { SettingsMenu } from "./SettingsMenu";
import styles from "./ActivityBar.module.css";

export type ScreenId = "monitor" | "library";

export interface ActivityBarProps {
  active: ScreenId;
  onSelect: (screen: ScreenId) => void;
}

export function ActivityBar(props: ActivityBarProps) {
  return (
    <nav class={styles.bar} aria-label="Primary">
      <IconButton
        aria-label="Printers"
        active={props.active === "monitor"}
        onClick={() => props.onSelect("monitor")}
      >
        <IconPrinter size={18} />
      </IconButton>
      <IconButton
        aria-label="Library"
        active={props.active === "library"}
        onClick={() => props.onSelect("library")}
      >
        <IconBox size={18} />
      </IconButton>
      <div class={styles.spacer} />
      <SettingsMenu />
    </nav>
  );
}
