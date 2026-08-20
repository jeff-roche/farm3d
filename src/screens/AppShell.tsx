import type { JSX } from "solid-js";
import { Logo } from "../design-system";
import { ActivityBar, type ScreenId } from "./ActivityBar";
import styles from "./AppShell.module.css";

export interface AppShellProps {
  active: ScreenId;
  onSelect: (screen: ScreenId) => void;
  title: string;
  statusSummary: string;
  children: JSX.Element;
}

const VERSION = "farm3d 0.1.0";

export function AppShell(props: AppShellProps) {
  return (
    <div class={styles.shell}>
      <header class={styles.topBar}>
        <Logo size={18} />
        <span class={styles.wordmark}>farm3d</span>
        <span class={styles.divider} aria-hidden="true">
          /
        </span>
        <span class={styles.screenTitle}>{props.title}</span>
      </header>

      <ActivityBar active={props.active} onSelect={props.onSelect} />

      <main class={styles.content}>{props.children}</main>

      <footer class={styles.statusBar}>
        <span>{props.statusSummary}</span>
        <span class={styles.statusBarVersion}>{VERSION}</span>
      </footer>
    </div>
  );
}
