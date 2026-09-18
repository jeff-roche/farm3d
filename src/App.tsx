// src/App.tsx
import { onCleanup, onMount, Show } from "solid-js";
import { AppShell } from "./screens/AppShell";
import type { ScreenId } from "./screens/ActivityBar";
import { PrinterDashboard, summarizePrinters } from "./screens/PrinterDashboard";
import { ModelLibrary, type Model } from "./screens/ModelLibrary";
import {
  addPrinter,
  dismissPrinterStoreError,
  loadPrinters,
  printers,
  printerStoreError,
  printerStoreRetryable,
  removePrinter,
  startStatusListener,
} from "./printers/printer-store";
import { Button } from "./design-system";
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
  monitor: "Printers",
  queue: "Queue",
  library: "Library",
  spools: "Spools",
  settings: "Settings",
};

function App() {
  const active = () => navigation.target().destination;
  const shellActive = () => (active() === "library" ? "library" : "monitor") satisfies ScreenId;
  const navigate = (target: Parameters<typeof navigation.navigate>[0]) => {
    navigation.navigate(target, {
      availableDestinations: ["monitor", "library"],
      availableIds: printers().map((printer) => printer.id),
    });
    window.location.hash = serializeNavigationTarget(target).slice(1);
  };
  const setActive = (destination: ScreenId) => navigate({ version: 1, destination });

  onMount(() => {
    const applyFragment = () => {
      const target = parseNavigationTarget(window.location.hash);
      if (target) navigate(target);
    };
    applyFragment();
    window.addEventListener("hashchange", applyFragment);
    let unlisten: (() => void) | undefined;
    // Sequenced, not concurrent: `applyStatus` drops events for ids it does
    // not know yet, so any status arriving before the printer list has loaded
    // — including the backfill inside `startStatusListener` — is discarded
    // with no retry.
    void loadPrinters()
      .then(() => {
        navigation.navigate(navigation.target(), {
          availableDestinations: ["monitor", "library"],
          availableIds: printers().map((printer) => printer.id),
        });
        return startStatusListener();
      })
      .then((fn) => (unlisten = fn));
    onCleanup(() => {
      unlisten?.();
      window.removeEventListener("hashchange", applyFragment);
    });
  });

  return (
    <AppShell
      active={shellActive()}
      onSelect={setActive}
      title={SCREEN_TITLE[active()]}
      statusSummary={summarizePrinters(printers())}
    >
      <Show when={printerStoreError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Show when={printerStoreRetryable()}>
              <Button variant="ghost" onClick={() => void loadPrinters()}>
                Retry startup
              </Button>
            </Show>
            <Button variant="ghost" onClick={dismissPrinterStoreError}>
              Dismiss
            </Button>
          </div>
        )}
      </Show>
      <Show when={navigation.availability() === "destinationUnavailable"}>
        <div class={styles.errorBanner} role="status">
          <p class={styles.errorMessage}>{SCREEN_TITLE[active()]} is not available in this version.</p>
          <Button variant="ghost" onClick={() => setActive("monitor")}>Open Printers</Button>
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
        <PrinterDashboard
          printers={printers()}
          selectedPrinterId={navigation.target().selection?.kind === "printer" ? navigation.target().selection?.id : undefined}
          onSelectionChange={(id) => navigate({
            version: 1,
            destination: "monitor",
            ...(id ? { selection: { kind: "printer", id } } : {}),
          })}
          onAddPrinter={async (draft) => {
            const id = await addPrinter(draft);
            return id ? printers().find((p) => p.id === id) : undefined;
          }}
          onRemovePrinter={(id) => void removePrinter(id)}
        />
      </Show>
    </AppShell>
  );
}

export default App;
