import { For, Show } from "solid-js";
import { Button, Panel } from "../design-system";
import { GroundPlane } from "./GroundPlane";
import { ThemeMenu } from "./ThemeMenu";
import styles from "./Welcome.module.css";

export interface RecentFarm {
  name: string;
  editedLabel: string;
}

export interface WelcomeProps {
  recentFarms: RecentFarm[];
  onNewFarm: () => void;
  onOpenFarm: (name: string) => void;
}

export function Welcome(props: WelcomeProps) {
  return (
    <div class={styles.screen}>
      <GroundPlane />

      <header class={styles.topBar}>
        <span class={styles.wordmark}>farm3d</span>
        <ThemeMenu />
      </header>

      <Panel title="Start" class={styles.startPanel}>
        <div class={styles.actions}>
          <Button variant="primary" onClick={props.onNewFarm}>
            + New farm
          </Button>
          <Button variant="secondary" disabled title="Opening farms isn't wired up yet">
            Open...
          </Button>
        </div>

        <div class={styles.recentLabel}>Recent</div>
        <Show
          when={props.recentFarms.length > 0}
          fallback={<p class={styles.empty}>Farms you open will show up here.</p>}
        >
          <ul class={styles.recentList}>
            <For each={props.recentFarms}>
              {(farm) => (
                <li>
                  <button class={styles.recentItem} onClick={() => props.onOpenFarm(farm.name)}>
                    <span class={styles.recentName}>{farm.name}</span>
                    <span class={styles.recentMeta}>{farm.editedLabel}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </Panel>

      <footer class={styles.footer}>farm3d 0.1.0</footer>
    </div>
  );
}
