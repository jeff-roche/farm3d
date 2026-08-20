// src/App.tsx
import { createSignal, onMount, Show } from "solid-js";
import { AppShell } from "./screens/AppShell";
import type { ScreenId } from "./screens/ActivityBar";
import { PrinterDashboard, summarizePrinters } from "./screens/PrinterDashboard";
import { ModelLibrary, type Model } from "./screens/ModelLibrary";
import { loadPrinters, printers, removePrinter } from "./printers/printer-store";

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
    void loadPrinters();
  });

  return (
    <AppShell
      active={active()}
      onSelect={setActive}
      title={SCREEN_TITLE[active()]}
      statusSummary={summarizePrinters(printers())}
    >
      <Show
        when={active() === "printers"}
        fallback={
          <ModelLibrary models={MODELS} compatiblePrinterNames={printers().map((p) => p.name)} />
        }
      >
        <PrinterDashboard
          printers={printers()}
          onRemovePrinter={(id) => void removePrinter(id)}
        />
      </Show>
    </AppShell>
  );
}

export default App;
