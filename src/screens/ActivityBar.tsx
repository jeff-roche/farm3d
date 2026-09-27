import { IconBox, IconDisc, IconPrinter } from "@tabler/icons-solidjs";
import { Show } from "solid-js";
import { IconButton } from "../design-system";
import { SettingsMenu } from "./SettingsMenu";
import styles from "./ActivityBar.module.css";

export type ScreenId = "monitor" | "library" | "spools";

export interface ActivityBarProps {
  active: ScreenId;
  onSelect: (screen: ScreenId) => void;
  /** Count of Spools needing attention: `low` or `reconciliation` (P7 Task
   *  4 brief). Omitted or 0 renders no badge. */
  attentionSpoolCount?: number;
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
          aria-label={(props.attentionSpoolCount ?? 0) > 0 ? `Spools (${props.attentionSpoolCount} need attention)` : "Spools"}
          aria-current={props.active === "spools" ? "page" : undefined}
          active={props.active === "spools"}
          onClick={() => props.onSelect("spools")}
        >
          <IconDisc size={18} />
        </IconButton>
        <Show when={(props.attentionSpoolCount ?? 0) > 0}>
          <span class={styles.badge} aria-hidden="true">{props.attentionSpoolCount}</span>
        </Show>
      </div>
      <div class={styles.spacer} />
      <SettingsMenu />
    </nav>
  );
}
