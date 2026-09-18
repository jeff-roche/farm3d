import { For, createSignal, onCleanup, type JSX } from "solid-js";
import { Logo, PrinterRoster, SeverityMarker, type PrinterRosterEntry } from "../design-system";
import type { MonitorRosterView, MonitorSeverity } from "../monitor/monitor-store";
import { ActivityBar, type ScreenId } from "./ActivityBar";
import styles from "./AppShell.module.css";

export interface AppShellProps {
  active: ScreenId;
  onSelect: (screen: ScreenId) => void;
  title: string;
  printerRoster: PrinterRosterModel;
  operationalRosters: readonly PrinterRosterModel[];
  adapterHealth: AdapterHealth;
  lastLiveEventAt?: string;
  children: JSX.Element;
}

export type PrinterRosterModel = MonitorRosterView;
export type Severity = MonitorSeverity;

export interface AdapterHealth {
  severity: Severity;
  label: string;
}

const VERSION = "farm3d 0.1.0";
const AGE_REFRESH_MS = 60_000;

function rosterEntries(roster: PrinterRosterModel): PrinterRosterEntry[] {
  return roster.printers.map((printer) => ({
    id: printer.id,
    name: printer.name,
    stateLabel: printer.operationalState ?? "Unavailable",
  }));
}

function formatLastLiveEventAge(lastLiveEventAt: string | undefined, now: number): string {
  if (!lastLiveEventAt) return "No live events yet";

  const observedAt = Date.parse(lastLiveEventAt);
  if (Number.isNaN(observedAt)) return "Last live event unavailable";

  const elapsedMinutes = Math.max(0, Math.floor((now - observedAt) / AGE_REFRESH_MS));
  if (elapsedMinutes === 0) return "Last live event just now";
  if (elapsedMinutes < 60) return `Last live event ${elapsedMinutes}m ago`;

  const elapsedHours = Math.floor(elapsedMinutes / 60);
  if (elapsedHours < 24) return `Last live event ${elapsedHours}h ago`;

  return `Last live event ${Math.floor(elapsedHours / 24)}d ago`;
}

export function AppShell(props: AppShellProps) {
  const [now, setNow] = createSignal(Date.now());
  const ageRefresh = window.setInterval(() => setNow(Date.now()), AGE_REFRESH_MS);
  onCleanup(() => window.clearInterval(ageRefresh));

  const viewAll = () => props.onSelect("monitor");

  return (
    <div class={styles.shell}>
      <header class={styles.topBar}>
        <Logo size={18} />
        <span class={styles.wordmark}>farm3d</span>
        <span class={styles.divider} aria-hidden="true">
          /
        </span>
        <span class={styles.screenTitle}>{props.title}</span>
        <div class={styles.rosters} role="group" aria-label="Printer rosters">
          <PrinterRoster
            label={props.printerRoster.label}
            count={props.printerRoster.count}
            printers={rosterEntries(props.printerRoster)}
            onViewAll={viewAll}
          />
          <For each={props.operationalRosters}>
            {(roster) => (
              <PrinterRoster
                label={roster.label}
                count={roster.count}
                printers={rosterEntries(roster)}
                onViewAll={viewAll}
              />
            )}
          </For>
        </div>
      </header>

      <ActivityBar active={props.active} onSelect={props.onSelect} />

      <main class={styles.content}>{props.children}</main>

      <footer class={styles.statusBar}>
        <SeverityMarker severity={props.adapterHealth.severity} label={props.adapterHealth.label} />
        <span>{formatLastLiveEventAge(props.lastLiveEventAt, now())}</span>
        <span class={styles.statusBarVersion}>{VERSION}</span>
      </footer>
    </div>
  );
}
