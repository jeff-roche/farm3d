import { IconBox, IconDisc, IconPlaylist, IconPrinter, IconSettings } from "@tabler/icons-solidjs";
import { onCleanup, onMount, Show } from "solid-js";
import { IconButton } from "../design-system";
import { registerSlicerSettingsHome } from "../slicing/slicer-settings-opener";
import styles from "./ActivityBar.module.css";

export type ScreenId = "monitor" | "queue" | "library" | "spools" | "settings";

export interface ActivityBarProps {
  active: ScreenId;
  onSelect: (screen: ScreenId) => void;
  /** Count of Spools needing attention: `low` or `reconciliation` (P7 Task
   *  4 brief). Omitted or 0 renders no badge. */
  attentionSpoolCount?: number;
  /** Queue Entries whose verdict is Blocked or Awaiting operator, plus open
   *  Reconciliation Requirements (P7 spec "Frontend architecture").
   *  Omitted or 0 renders no badge. */
  queueAttentionCount?: number;
  /** The Attention center's own actionable count (spec "Frontend
   *  architecture": "The Monitor rail button's badge shows the same
   *  actionable count"). Omitted or 0 renders no badge. */
  attentionActionableCount?: number;
}

export function ActivityBar(props: ActivityBarProps) {
  let settingsButton: HTMLButtonElement | undefined;
  // The Settings button is where focus returns from the Slicer settings
  // when whatever opened them has gone.
  onMount(() => {
    if (settingsButton) onCleanup(registerSlicerSettingsHome(settingsButton));
  });

  return (
    <nav class={styles.bar} aria-label="Primary">
      <div class={styles.iconWrap}>
        <IconButton
          aria-label={(props.attentionActionableCount ?? 0) > 0 ? `Monitor (${props.attentionActionableCount} need attention)` : "Monitor"}
          aria-current={props.active === "monitor" ? "page" : undefined}
          active={props.active === "monitor"}
          onClick={() => props.onSelect("monitor")}
        >
          <IconPrinter size={18} />
        </IconButton>
        <Show when={(props.attentionActionableCount ?? 0) > 0}>
          <span class={styles.badge} aria-hidden="true">{props.attentionActionableCount}</span>
        </Show>
      </div>
      <div class={styles.iconWrap}>
        <IconButton
          aria-label={(props.queueAttentionCount ?? 0) > 0 ? `Queue (${props.queueAttentionCount} need attention)` : "Queue"}
          aria-current={props.active === "queue" ? "page" : undefined}
          active={props.active === "queue"}
          onClick={() => props.onSelect("queue")}
        >
          {/* The umbrella spec's "ordered list entering execution". */}
          <IconPlaylist size={18} />
        </IconButton>
        <Show when={(props.queueAttentionCount ?? 0) > 0}>
          <span class={styles.badge} aria-hidden="true">{props.queueAttentionCount}</span>
        </Show>
      </div>
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
      <IconButton
        ref={settingsButton}
        aria-label="Settings"
        aria-current={props.active === "settings" ? "page" : undefined}
        active={props.active === "settings"}
        onClick={() => props.onSelect("settings")}
      >
        <IconSettings size={18} />
      </IconButton>
    </nav>
  );
}
