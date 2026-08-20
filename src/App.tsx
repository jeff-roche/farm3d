// src/App.tsx
import { createSignal, Show } from "solid-js";
import { AppShell } from "./screens/AppShell";
import type { ScreenId } from "./screens/ActivityBar";
import { FleetDashboard, summarizeFleet, type Printer } from "./screens/FleetDashboard";
import { ModelLibrary, type Model } from "./screens/ModelLibrary";

const PRINTERS: Printer[] = [
  {
    id: "voron-1",
    name: "Voron 2.4 — Bay 1",
    status: "printing",
    connectionType: "network",
    currentJob: { modelName: "Benchy_v3.gcode", progress: 0.62 },
    nozzleTempC: 210,
    bedTempC: 60,
  },
  {
    id: "prusa-1",
    name: "Prusa MK4 — Bay 2",
    status: "idle",
    connectionType: "network",
  },
  {
    id: "ender-1",
    name: "Ender 3 — Bench",
    status: "offline",
    connectionType: "serial",
  },
];

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

  return (
    <AppShell
      active={active()}
      onSelect={setActive}
      title={SCREEN_TITLE[active()]}
      statusSummary={summarizeFleet(PRINTERS)}
    >
      <Show
        when={active() === "printers"}
        fallback={
          <ModelLibrary
            models={MODELS}
            compatiblePrinterNames={PRINTERS.map((p) => p.name)}
          />
        }
      >
        <FleetDashboard printers={PRINTERS} />
      </Show>
    </AppShell>
  );
}

export default App;
