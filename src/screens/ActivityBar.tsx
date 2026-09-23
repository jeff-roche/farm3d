import { IconBox, IconDisc, IconPrinter } from "@tabler/icons-solidjs";
import { Show } from "solid-js";
import { IconButton } from "../design-system";
import { SettingsMenu } from "./SettingsMenu";
import styles from "./ActivityBar.module.css";

export type ScreenId = "monitor" | "library" | "spools";

export interface ActivityBarProps {
  active: ScreenId;
  onSelect: (screen: ScreenId) => void;
  /** Count of `low` Spools (P3 design: "The badge counts `low` Spools").
   *  Omitted or 0 renders no badge. */
  lowSpoolCount?: number;
}

export function ActivityBar(props: ActivityBarProps) {
  return (
    <nav class={styles.bar} aria-label="Primary">
      <IconButton
        aria-label="Monitor"
        aria-current={props.active === "monitor" ? "page" : undefined}
        active={props.active === "monitor"}
        onClick={() => props.onSelect("monitor")}
      >
        <IconPrinter size={18} />
      </IconButton>
      <IconButton
        aria-label="Library"
        aria-current={props.active === "library" ? "page" : undefined}
        active={props.active === "library"}
        onClick={() => props.onSelect("library")}
      >
        <IconBox size={18} />
      </IconButton>
      <div class={styles.iconWrap}>
        <IconButton
          aria-label={(props.lowSpoolCount ?? 0) > 0 ? `Spools (${props.lowSpoolCount} low)` : "Spools"}
          aria-current={props.active === "spools" ? "page" : undefined}
          active={props.active === "spools"}
          onClick={() => props.onSelect("spools")}
        >
          <IconDisc size={18} />
        </IconButton>
        <Show when={(props.lowSpoolCount ?? 0) > 0}>
          <span class={styles.badge} aria-hidden="true">{props.lowSpoolCount}</span>
        </Show>
      </div>
      <div class={styles.spacer} />
      <SettingsMenu />
    </nav>
  );
}
