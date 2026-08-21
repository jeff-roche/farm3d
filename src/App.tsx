// src/App.tsx
import { createSignal, onCleanup, onMount, Show } from "solid-js";
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
  removePrinter,
  startStatusListener,
} from "./printers/printer-store";
import { Button } from "./design-system";
import styles from "./App.module.css";

const MODELS: Model[] = [
  { id: "benchy", name: "Benchy_v3.gcode", addedAt: "2 days ago" },
  { id: "bracket", name: "mount_bracket.stl", addedAt: "1 week ago" },
];

const SCREEN_TITLE: Record<ScreenId, string> = {
  printers: "Printers",
  library: "Library",
};

function App() {
  const [active, setActive] = createSignal<ScreenId>("printers");

  onMount(() => {
    let unlisten: (() => void) | undefined;
    // Sequenced, not concurrent: `applyStatus` drops events for ids it does
    // not know yet, so any status arriving before the printer list has loaded
    // — including the backfill inside `startStatusListener` — is discarded
    // with no retry.
    void loadPrinters()
      .then(startStatusListener)
      .then((fn) => (unlisten = fn));
    onCleanup(() => unlisten?.());
  });

  return (
    <AppShell
      active={active()}
      onSelect={setActive}
      title={SCREEN_TITLE[active()]}
      statusSummary={summarizePrinters(printers())}
    >
      <Show when={printerStoreError()}>
        {(message) => (
          <div class={styles.errorBanner} role="alert">
            <p class={styles.errorMessage}>{message()}</p>
            <Button variant="ghost" onClick={dismissPrinterStoreError}>
              Dismiss
            </Button>
          </div>
        )}
      </Show>
      <Show
        when={active() === "printers"}
        fallback={
          <ModelLibrary models={MODELS} compatiblePrinterNames={printers().map((p) => p.name)} />
        }
      >
        <PrinterDashboard
          printers={printers()}
          onAddPrinter={(draft) => void addPrinter(draft)}
          onRemovePrinter={(id) => void removePrinter(id)}
        />
      </Show>
    </AppShell>
  );
}

export default App;
