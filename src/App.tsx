// src/App.tsx
import { createSignal, onCleanup, onMount, Show } from "solid-js";
import { createMonitorStore, type MonitorShellView, type MonitorStore } from "./monitor/monitor-store";
import { AppShell } from "./screens/AppShell";
import type { ScreenId } from "./screens/ActivityBar";
import { PrinterDashboard } from "./screens/PrinterDashboard";
import { ModelLibrary, type Model } from "./screens/ModelLibrary";
import {
  addPrinter,
  dismissPrinterStoreError,
  exportPrinters,
  importPrinters,
  loadPrinters,
  printers,
  printerStoreError,
  printerStoreRetryable,
  printerStoreStatus,
  printerStatusSyncState,
  removePrinter,
  startStatusListener,
} from "./printers/printer-store";
import { Button } from "./design-system";
import { loadSettings, updateSettings } from "./settings/settings-store";
import styles from "./App.module.css";
import {
  navigation,
  parseNavigationTarget,
  serializeNavigationTarget,
  type NavigationDestination,
} from "./navigation/navigation-store";

const MODELS: Model[] = [
  { id: "benchy", name: "Benchy_v3.gcode", addedAt: "2 days ago" },
  { id: "bracket", name: "mount_bracket.stl", addedAt: "1 week ago" },
];

const SCREEN_TITLE: Record<NavigationDestination, string> = {
  monitor: "Monitor",
  queue: "Queue",
  library: "Library",
  spools: "Spools",
  settings: "Settings",
};

const EMPTY_SHELL: MonitorShellView = {
  printerRoster: { key: "all", label: "Printers", count: 0, printers: [], remainingCount: 0 },
  operationalRosters: [],
  adapterHealth: { severity: "info", label: "Waiting for adapter status" },
};

function App() {
  const [monitorStore, setMonitorStore] = createSignal<MonitorStore>();
  const [statusStartupError, setStatusStartupError] = createSignal<string | null>(null);
  let retryStartup: (() => void) | undefined;
  const active = () => navigation.target().destination;
  const shellActive = () => (active() === "library" ? "library" : "monitor") satisfies ScreenId;
  const shell = () => monitorStore()?.shell() ?? EMPTY_SHELL;
  const navigationContext = () => ({
    availableDestinations: ["monitor", "library"] as NavigationDestination[],
    availableIds: printers().map((printer) => printer.id),
  });
  const reconcileNavigation = () => {
    navigation.navigate(navigation.target(), navigationContext());
    const target = navigation.target();
    monitorStore()?.setSelectedPrinterId(
      navigation.availability() === "available" && target.destination === "monitor" && target.selection?.kind === "printer"
        ? target.selection.id
        : null,
    );
  };
  const navigate = (target: Parameters<typeof navigation.navigate>[0]) => {
    navigation.navigate(target, navigationContext());
    reconcileNavigation();
    window.location.hash = serializeNavigationTarget(target).slice(1);
  };
  const setActive = (destination: ScreenId) => navigate({ version: 1, destination });

  onMount(() => {
    let disposed = false;
    let unlisten: (() => void) | undefined;
    let startupGeneration = 0;
    const start = () => {
      const generation = ++startupGeneration;
      setStatusStartupError(null);
      unlisten?.();
      unlisten = undefined;
      void (async () => {
        let settings;
        try {
          settings = await loadSettings();
          await loadPrinters();
          if (printerStoreStatus() === "error") throw new Error("Printer loading failed");
        } catch {
          if (!disposed && generation === startupGeneration) {
            setStatusStartupError("Monitor startup could not be completed.");
          }
          return;
        }
        if (disposed || generation !== startupGeneration) return;
        setMonitorStore(createMonitorStore({
          printers,
          initialSection: settings.monitorSection,
          initialDensity: settings.monitorDensity,
          persistPreferences: (next) => updateSettings(next),
        }));
        reconcileNavigation();
        try {
          const dispose = await startStatusListener();
          if (disposed || generation !== startupGeneration) dispose();
          else unlisten = dispose;
        } catch {
          if (!disposed && generation === startupGeneration) {
            setStatusStartupError("Live Printer status could not be started.");
          }
        }
      })();
    };
    retryStartup = start;
    const applyFragment = () => {
      const target = parseNavigationTarget(window.location.hash);
      if (target) navigate(target);
    };
    applyFragment();
    window.addEventListener("hashchange", applyFragment);
    // Printers must be known before the listener's backfill arrives; status
    // events for unknown ids are deliberately ignored by the reconciliation store.
    start();
    onCleanup(() => {
      disposed = true;
      retryStartup = undefined;
      unlisten?.();
      window.removeEventListener("hashchange", applyFragment);
    });
  });

  return (
    <AppShell
      active={shellActive()}
      onSelect={setActive}
      title={SCREEN_TITLE[active()]}
      printerRoster={shell().printerRoster}
      operationalRosters={shell().operationalRosters}
      adapterHealth={shell().adapterHealth}
      lastLiveEventAt={shell().lastLiveEventAt}
    >
      <Show when={printerStoreError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Show when={printerStoreRetryable()}>
              <Button variant="ghost" onClick={() => retryStartup?.()}>
                Retry startup
              </Button>
            </Show>
            <Button variant="ghost" onClick={dismissPrinterStoreError}>
              Dismiss
            </Button>
          </div>
        )}
      </Show>
      <Show when={statusStartupError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Button variant="ghost" onClick={() => {
              retryStartup?.();
            }}>
              Retry startup
            </Button>
            <Button variant="ghost" onClick={() => setStatusStartupError(null)}>
              Dismiss
            </Button>
          </div>
        )}
      </Show>
      <Show when={navigation.availability() === "destinationUnavailable"}>
        <div class={styles.errorBanner} role="status">
          <p class={styles.errorMessage}>{SCREEN_TITLE[active()]} is not available in this version.</p>
          <Button variant="ghost" onClick={() => setActive("monitor")}>Open Monitor</Button>
        </div>
      </Show>
      <Show when={navigation.availability() === "selectionUnavailable"}>
        <div class={styles.errorBanner} role="status">
          <p class={styles.errorMessage}>The requested item is no longer available.</p>
        </div>
      </Show>
      <Show
        when={navigation.availability() !== "destinationUnavailable" && active() === "monitor"}
        fallback={
          <Show when={active() === "library"}>
            <ModelLibrary models={MODELS} compatiblePrinterNames={printers().map((p) => p.name)} />
          </Show>
        }
      >
        <Show when={monitorStore()}>
          {(store) => (
            <PrinterDashboard
              store={store()}
              loading={printerStoreStatus() === "loading"}
              isFirstRun={printerStoreStatus() === "ready" && !store().hasPrinters()}
              syncState={printerStatusSyncState()}
              onSelectionChange={(id) => navigate({
                version: 1,
                destination: "monitor",
                ...(id ? { selection: { kind: "printer", id } } : {}),
              })}
              onAddPrinter={async (draft) => {
                const id = await addPrinter(draft);
                return id ? printers().find((p) => p.id === id) : undefined;
              }}
              onImport={() => void importPrinters().then(reconcileNavigation)}
              onExport={() => void exportPrinters()}
              onRemovePrinter={(id) => void removePrinter(id).then(reconcileNavigation)}
            />
          )}
        </Show>
      </Show>
    </AppShell>
  );
}

export default App;
