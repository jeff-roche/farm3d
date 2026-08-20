# Print Farm Screens Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Replace the agriculture-themed `Welcome`/`EditorShell`/`GroundPlane` screens with a VSCode-style activity-bar shell around a Fleet Dashboard (printer monitoring) and a Model Library + Inspector, matching farm3d's real domain.

**Architecture:** A new persistent `AppShell` (activity bar + top bar + status bar, all CSS Grid like the code it replaces) wraps whichever of two screens is active — `FleetDashboard` or `ModelLibrary` — with `App.tsx` owning the active-screen signal and the mock `Printer`/`Model` data, mirroring the exact pattern `EditorShell`/`Welcome` already used for `SceneObject`/`RecentFarm`. `GroundPlane` is renamed to `BuildPlate` (same CSS, new framing) and consumed only by `ModelLibrary`. No Tauri backend integration — every data-mutating action is a disabled, stubbed control.

**Tech Stack:** SolidJS, CSS Modules, Kobalte (via the existing `Button`/`IconButton`/`Select`/`Progress`/`DropdownMenu` wrappers), `@tabler/icons-solidjs`.

**Spec:** [docs/superpowers/specs/2026-08-19-print-farm-screens-design.md](../specs/2026-08-19-print-farm-screens-design.md)

## Global Constraints

- **Hard dependency:** the brand identity plan (`docs/superpowers/plans/2026-08-19-brand-identity.md`) must already be implemented — `@tabler/icons-solidjs` installed, `farm3dTypography` using Manrope/Fira Sans/Fira Code, and `Logo`/`LogoProps` exported from `src/design-system/components/Logo.tsx` (and thus from `src/design-system`). This plan does not re-derive or re-implement any of that.
- UI-shell only: mock data, stubbed/disabled actions. No Tauri `invoke` calls, no real printer Connections, no real Slicing.
- Never hardcode colors/font-sizes/radii in component CSS — reference `--f3d-color-*` / `--f3d-type-*` / `--f3d-radius-*` (verified against `src/design-system/theme-engine.ts`'s `camelToKebab` — e.g. `onAccent` → `--f3d-color-on-accent`, `surfaceRaised` → `--f3d-color-surface-raised`).
- Per the spec's own Testing section: **no new screen-level test files** — only `src/design-system/` has test coverage today, and this spec doesn't change that convention. Verification is `just build` + `just test` (regression) plus a manual `just web` visual check per task.
- Interactive behavior goes through the existing design-system wrappers (`Button`, `IconButton`, `Select`, `Progress`, `DropdownMenu`, `Panel`) — don't hand-roll new Kobalte usage.
- Run `just build` and `just test` before considering any task done (per `AGENTS.md`).

## File Structure

```
src/screens/
  ActivityBar.tsx / .module.css   NEW — narrow icon rail (Printers/Library + theme toggle)
  AppShell.tsx / .module.css      NEW — persistent chrome: activity bar + top bar + status bar
  BuildPlate.tsx / .module.css    NEW — renamed GroundPlane, same CSS technique
  FleetDashboard.tsx / .module.css NEW — printer cards + detail panel (replaces EditorShell)
  ModelLibrary.tsx / .module.css  NEW — model list + BuildPlate viewport + slice panel
  ThemeMenu.tsx / .module.css     UNCHANGED — reused as-is, just relocated into ActivityBar
  Welcome.tsx / .module.css       DELETE
  EditorShell.tsx / .module.css   DELETE
  GroundPlane.tsx / .module.css   DELETE (superseded by BuildPlate)
src/App.tsx                       MODIFY — owns active-screen signal + mock Printer/Model data
```

`Printer` is defined in and exported from `FleetDashboard.tsx`; `Model` is defined in and exported from `ModelLibrary.tsx` — exactly how `SceneObject` lived in `EditorShell.tsx` and `RecentFarm` lived in `Welcome.tsx` today. `App.tsx` imports both types plus the two screen components.

**Task order matters**: `BuildPlate`/`FleetDashboard`/`ModelLibrary`/`AppShell` are all purely additive (new files, nothing deletes or imports from the old screens), so they can land one at a time without ever breaking the build. The deletions of `Welcome`/`EditorShell`/`GroundPlane` happen in the last task, atomically with rewiring `App.tsx` — that's the one moment the old files become truly unreferenced.

---

## Task 1: `ActivityBar` + `AppShell`

**Files:**
- Create: `src/screens/ActivityBar.tsx`
- Create: `src/screens/ActivityBar.module.css`
- Create: `src/screens/AppShell.tsx`
- Create: `src/screens/AppShell.module.css`

**Interfaces:**
- Produces: `type ScreenId = "printers" | "library"`; `ActivityBar(props: { active: ScreenId; onSelect: (screen: ScreenId) => void })`; `AppShell(props: { active: ScreenId; onSelect: (screen: ScreenId) => void; title: string; statusSummary: string; children: JSX.Element })`. Task 5 wires both into `App.tsx`.

- [ ] **Step 1: Write `ActivityBar`**

```tsx
// src/screens/ActivityBar.tsx
import { IconBox, IconPrinter } from "@tabler/icons-solidjs";
import { IconButton } from "../design-system";
import { ThemeMenu } from "./ThemeMenu";
import styles from "./ActivityBar.module.css";

export type ScreenId = "printers" | "library";

export interface ActivityBarProps {
  active: ScreenId;
  onSelect: (screen: ScreenId) => void;
}

export function ActivityBar(props: ActivityBarProps) {
  return (
    <nav class={styles.bar} aria-label="Primary">
      <IconButton
        aria-label="Printers"
        active={props.active === "printers"}
        onClick={() => props.onSelect("printers")}
      >
        <IconPrinter size={18} />
      </IconButton>
      <IconButton
        aria-label="Library"
        active={props.active === "library"}
        onClick={() => props.onSelect("library")}
      >
        <IconBox size={18} />
      </IconButton>
      <div class={styles.spacer} />
      <ThemeMenu />
    </nav>
  );
}
```

- [ ] **Step 2: Style `ActivityBar`**

```css
/* src/screens/ActivityBar.module.css */
.bar {
  grid-area: activity;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.25rem;
  width: 3rem;
  padding: 0.5rem 0;
  background-color: var(--f3d-color-surface);
  border-right: 1px solid var(--f3d-color-border);
}

.spacer {
  flex: 1;
}
```

- [ ] **Step 3: Write `AppShell`**

```tsx
// src/screens/AppShell.tsx
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
      <ActivityBar active={props.active} onSelect={props.onSelect} />

      <header class={styles.topBar}>
        <Logo size={18} />
        <span class={styles.wordmark}>farm3d</span>
        <span class={styles.divider} aria-hidden="true">
          /
        </span>
        <span class={styles.screenTitle}>{props.title}</span>
      </header>

      <main class={styles.content}>{props.children}</main>

      <footer class={styles.statusBar}>
        <span>{props.statusSummary}</span>
        <span class={styles.statusBarVersion}>{VERSION}</span>
      </footer>
    </div>
  );
}
```

- [ ] **Step 4: Style `AppShell`**

```css
/* src/screens/AppShell.module.css */
.shell {
  display: grid;
  grid-template-columns: 3rem 1fr;
  grid-template-rows: auto 1fr auto;
  grid-template-areas:
    "activity topbar"
    "activity content"
    "activity status";
  width: 100%;
  height: 100vh;
  min-width: 0;
  background-color: var(--f3d-color-bg);
}

.topBar {
  grid-area: topbar;
  display: flex;
  align-items: center;
  gap: 0.5rem;
  padding: 0.375rem 0.75rem;
  border-bottom: 1px solid var(--f3d-color-border);
  background-color: var(--f3d-color-surface);
}

.wordmark {
  font-family: var(--f3d-type-heading-font);
  font-size: var(--f3d-type-heading-size);
  font-weight: var(--f3d-type-heading-weight);
  color: var(--f3d-color-text);
}

.divider {
  color: var(--f3d-color-text-disabled);
}

.screenTitle {
  font-family: var(--f3d-type-body-font);
  font-size: var(--f3d-type-body-size);
  color: var(--f3d-color-text-muted);
}

.content {
  grid-area: content;
  display: flex;
  flex-direction: column;
  min-width: 0;
  min-height: 0;
  overflow: hidden;
}

.statusBar {
  grid-area: status;
  display: flex;
  align-items: center;
  gap: 1rem;
  padding: 0.25rem 0.75rem;
  border-top: 1px solid var(--f3d-color-border);
  background-color: var(--f3d-color-surface);
  font-family: var(--f3d-type-mono-font);
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text-muted);
}

.statusBarVersion {
  margin-left: auto;
  color: var(--f3d-color-text-disabled);
}
```

- [ ] **Step 5: Run the build and test suite**

Run: `just build && just test`
Expected: both succeed. Neither `ActivityBar` nor `AppShell` is imported anywhere yet, so there's nothing to visually check until Task 5 wires them into `App.tsx` — that's where the live confirmation happens.

- [ ] **Step 6: Commit**

```bash
git add src/screens/ActivityBar.tsx src/screens/ActivityBar.module.css \
  src/screens/AppShell.tsx src/screens/AppShell.module.css
git commit -m "Add the ActivityBar and AppShell navigation chrome"
```

---

## Task 2: `FleetDashboard`

**Files:**
- Create: `src/screens/FleetDashboard.tsx`
- Create: `src/screens/FleetDashboard.module.css`

**Interfaces:**
- Produces: `interface Printer { id: string; name: string; status: "idle" | "printing" | "paused" | "error" | "offline"; connectionType: "network" | "serial"; currentJob?: { modelName: string; progress: number }; nozzleTempC?: number; bedTempC?: number }`; `summarizeFleet(printers: Printer[]): string`; `FleetDashboard(props: { printers: Printer[] })`. Task 5 imports `Printer`, `summarizeFleet`, and `FleetDashboard`.

- [ ] **Step 1: Write `FleetDashboard`**

```tsx
// src/screens/FleetDashboard.tsx
import { createMemo, createSignal, For, Show } from "solid-js";
import { IconUsb, IconWifi } from "@tabler/icons-solidjs";
import { Button, Progress } from "../design-system";
import styles from "./FleetDashboard.module.css";

export interface Printer {
  id: string;
  name: string;
  status: "idle" | "printing" | "paused" | "error" | "offline";
  connectionType: "network" | "serial";
  currentJob?: { modelName: string; progress: number };
  nozzleTempC?: number;
  bedTempC?: number;
}

export interface FleetDashboardProps {
  printers: Printer[];
}

const STATUS_LABEL: Record<Printer["status"], string> = {
  idle: "Idle",
  printing: "Printing",
  paused: "Paused",
  error: "Error",
  offline: "Offline",
};

/** "3 printers · 1 printing · 2 idle" — for AppShell's status bar. */
export function summarizeFleet(printers: Printer[]): string {
  if (printers.length === 0) return "No printers";
  const printing = printers.filter((p) => p.status === "printing").length;
  const idle = printers.filter((p) => p.status === "idle").length;
  return `${printers.length} printer${printers.length === 1 ? "" : "s"} · ${printing} printing · ${idle} idle`;
}

export function FleetDashboard(props: FleetDashboardProps) {
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const selected = createMemo(() => props.printers.find((p) => p.id === selectedId()));

  return (
    <div class={styles.dashboard}>
      <div class={styles.grid}>
        <Show
          when={props.printers.length > 0}
          fallback={
            <div class={styles.empty}>
              <p class={styles.emptyMessage}>No printers yet</p>
              <Button variant="secondary" disabled title="Adding printers isn't wired up yet">
                + Add printer
              </Button>
            </div>
          }
        >
          <For each={props.printers}>
            {(printer) => (
              <button
                class={styles.card}
                classList={{ [styles.cardSelected]: printer.id === selectedId() }}
                onClick={() => setSelectedId(printer.id)}
              >
                <div class={styles.cardHeader}>
                  <span class={styles.cardName}>{printer.name}</span>
                  <span
                    class={styles.statusBadge}
                    classList={{ [styles[`statusBadge_${printer.status}`]]: true }}
                  >
                    {STATUS_LABEL[printer.status]}
                  </span>
                </div>

                {printer.currentJob && (
                  <div class={styles.jobRow}>
                    <span class={styles.jobName}>{printer.currentJob.modelName}</span>
                    <Progress value={printer.currentJob.progress * 100} showValue />
                  </div>
                )}

                <div class={styles.cardFooter}>
                  <span class={styles.temps}>
                    {printer.nozzleTempC != null && `${printer.nozzleTempC}°C nozzle`}
                    {printer.bedTempC != null && ` · ${printer.bedTempC}°C bed`}
                  </span>
                  <span class={styles.connection}>
                    {printer.connectionType === "network" ? (
                      <IconWifi size={12} />
                    ) : (
                      <IconUsb size={12} />
                    )}
                    {printer.connectionType}
                  </span>
                </div>
              </button>
            )}
          </For>
        </Show>
      </div>

      <Show when={selected()}>
        {(printer) => (
          <aside class={styles.detail} aria-label="Printer detail">
            <div class={styles.detailHeader}>{printer().name}</div>
            <div class={styles.detailBody}>
              <div class={styles.detailField}>
                <span class={styles.detailLabel}>Status</span>
                <span>{STATUS_LABEL[printer().status]}</span>
              </div>
              <div class={styles.detailField}>
                <span class={styles.detailLabel}>Webcam</span>
                <div class={styles.webcamPlaceholder}>No feed configured</div>
              </div>
              <div class={styles.detailField}>
                <span class={styles.detailLabel}>Loaded material</span>
                <span class={styles.detailMuted}>Not tracked yet</span>
              </div>
              <Button variant="secondary" disabled title="Job assignment isn't wired up yet">
                Assign job
              </Button>
              <Button variant="danger" disabled title="Cancelling isn't wired up yet">
                Cancel job
              </Button>
            </div>
          </aside>
        )}
      </Show>
    </div>
  );
}
```

- [ ] **Step 2: Style `FleetDashboard`**

```css
/* src/screens/FleetDashboard.module.css */
.dashboard {
  display: flex;
  height: 100%;
  min-height: 0;
}

.grid {
  flex: 1;
  min-width: 0;
  overflow-y: auto;
  padding: 0.75rem;
  display: grid;
  grid-template-columns: repeat(auto-fill, minmax(15rem, 1fr));
  gap: 0.75rem;
  align-content: start;
}

.card {
  composes: focusRing from "../design-system/components/shared.module.css";
  composes: resetButton from "../design-system/components/shared.module.css";
  display: flex;
  flex-direction: column;
  gap: 0.5rem;
  padding: 0.75rem;
  background-color: var(--f3d-color-surface-raised);
  border: 1px solid var(--f3d-color-border);
  border-radius: var(--f3d-radius-md);
  text-align: left;
}

.card:hover {
  border-color: var(--f3d-color-border-strong);
}

.cardSelected,
.cardSelected:hover {
  border-color: var(--f3d-color-accent);
}

.cardHeader {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
}

.cardName {
  font-family: var(--f3d-type-heading-font);
  font-size: var(--f3d-type-heading-size);
  font-weight: var(--f3d-type-heading-weight);
  color: var(--f3d-color-text);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.statusBadge {
  flex-shrink: 0;
  padding: 0.125rem 0.5rem;
  border-radius: var(--f3d-radius-full);
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
}

.statusBadge_idle {
  background-color: var(--f3d-color-surface-hover);
  color: var(--f3d-color-text-muted);
}
.statusBadge_printing {
  background-color: var(--f3d-color-accent);
  color: var(--f3d-color-on-accent);
}
.statusBadge_paused {
  background-color: var(--f3d-color-warning);
  color: var(--f3d-color-on-warning);
}
.statusBadge_error {
  background-color: var(--f3d-color-danger);
  color: var(--f3d-color-on-danger);
}
.statusBadge_offline {
  background-color: var(--f3d-color-surface);
  color: var(--f3d-color-text-disabled);
}

.jobRow {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.jobName {
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text-muted);
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
}

.cardFooter {
  display: flex;
  align-items: center;
  justify-content: space-between;
  gap: 0.5rem;
  font-family: var(--f3d-type-mono-font);
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text-muted);
}

.connection {
  display: inline-flex;
  align-items: center;
  gap: 0.25rem;
}

.empty {
  grid-column: 1 / -1;
  display: flex;
  flex-direction: column;
  align-items: center;
  gap: 0.75rem;
  padding: 3rem 1rem;
}

.emptyMessage {
  margin: 0;
  color: var(--f3d-color-text-muted);
  font-size: var(--f3d-type-body-size);
}

.detail {
  flex-shrink: 0;
  width: 16rem;
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow-y: auto;
  background-color: var(--f3d-color-surface);
  border-left: 1px solid var(--f3d-color-border);
}

.detailHeader {
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid var(--f3d-color-border);
  font-family: var(--f3d-type-heading-font);
  font-size: var(--f3d-type-heading-size);
  font-weight: var(--f3d-type-heading-weight);
  letter-spacing: var(--f3d-type-heading-tracking);
  color: var(--f3d-color-text);
}

.detailBody {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  padding: 0.75rem;
}

.detailField {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.detailLabel {
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-muted);
  text-transform: uppercase;
}

.detailMuted {
  color: var(--f3d-color-text-disabled);
}

.webcamPlaceholder {
  padding: 1.5rem 0.5rem;
  text-align: center;
  background-color: var(--f3d-color-surface-hover);
  border-radius: var(--f3d-radius-sm);
  color: var(--f3d-color-text-disabled);
  font-size: var(--f3d-type-body-small-size);
}
```

- [ ] **Step 3: Run the build and test suite**

Run: `just build && just test`
Expected: both succeed. Not yet mounted anywhere — visual check happens in Task 5.

- [ ] **Step 4: Commit**

```bash
git add src/screens/FleetDashboard.tsx src/screens/FleetDashboard.module.css
git commit -m "Add the FleetDashboard screen"
```

---

## Task 3: `BuildPlate`

**Files:**
- Create: `src/screens/BuildPlate.tsx`
- Create: `src/screens/BuildPlate.module.css`

**Interfaces:**
- Produces: `BuildPlate()` — no props. Task 4 (`ModelLibrary`) consumes it.

This is the exact CSS from `GroundPlane.module.css` — same technique, new name/framing per the spec ("Same static perspective-grid CSS technique, kept as-is — only the framing changes"). `GroundPlane.tsx`/`.module.css` themselves aren't deleted until Task 5, once nothing references them.

- [ ] **Step 1: Write `BuildPlate`**

```tsx
// src/screens/BuildPlate.tsx
import styles from "./BuildPlate.module.css";

/**
 * A static perspective floor grid — the print bed a Model sits on inside
 * the Model Inspector viewport.
 */
export function BuildPlate() {
  return (
    <div class={styles.stage}>
      <div class={styles.plane} />
    </div>
  );
}
```

- [ ] **Step 2: Copy the grid CSS**

```css
/* src/screens/BuildPlate.module.css */
.stage {
  position: absolute;
  inset: 0;
  z-index: 0;
  overflow: hidden;
  background-color: var(--f3d-color-bg);
  perspective: 650px;
  perspective-origin: 50% 100%;
}

.plane {
  position: absolute;
  left: -60%;
  right: -60%;
  bottom: 0;
  height: 220vh;
  background-image:
    repeating-linear-gradient(
      0deg,
      var(--f3d-color-border) 0px,
      var(--f3d-color-border) 1px,
      transparent 1px,
      transparent 40px
    ),
    repeating-linear-gradient(
      90deg,
      var(--f3d-color-border) 0px,
      var(--f3d-color-border) 1px,
      transparent 1px,
      transparent 40px
    ),
    repeating-linear-gradient(
      90deg,
      var(--f3d-color-accent) 0px,
      var(--f3d-color-accent) 1px,
      transparent 1px,
      transparent 320px
    );
  opacity: 0.7;
  transform: rotateX(63deg);
  transform-origin: 50% 100%;
  mask-image: linear-gradient(to top, black 0%, black 55%, transparent 94%);
  -webkit-mask-image: linear-gradient(to top, black 0%, black 55%, transparent 94%);
}
```

- [ ] **Step 3: Run the build and test suite**

Run: `just build && just test`
Expected: both succeed.

- [ ] **Step 4: Commit**

```bash
git add src/screens/BuildPlate.tsx src/screens/BuildPlate.module.css
git commit -m "Add BuildPlate (GroundPlane's grid, reframed as a print bed)"
```

---

## Task 4: `ModelLibrary`

**Files:**
- Create: `src/screens/ModelLibrary.tsx`
- Create: `src/screens/ModelLibrary.module.css`

**Interfaces:**
- Consumes: `BuildPlate` from Task 3.
- Produces: `interface Model { id: string; name: string; addedAt: string }`; `ModelLibrary(props: { models: Model[]; compatiblePrinterNames: string[] })`. Task 5 imports `Model` and `ModelLibrary`.

Note on `compatiblePrinterNames`: the spec describes the printer picker as "filtered by Printer Profile compatibility," but the spec's own `Printer` data model (Task 2) has no profile/build-volume/material fields to filter on — that's out of scope here. This lists every printer name, disabled, rather than fabricating a compatibility check the data can't support.

- [ ] **Step 1: Write `ModelLibrary`**

```tsx
// src/screens/ModelLibrary.tsx
import { createMemo, createSignal, For, Show } from "solid-js";
import { IconRefresh } from "@tabler/icons-solidjs";
import { Button, IconButton, Select } from "../design-system";
import { BuildPlate } from "./BuildPlate";
import styles from "./ModelLibrary.module.css";

export interface Model {
  id: string;
  name: string;
  addedAt: string;
}

export interface ModelLibraryProps {
  models: Model[];
  compatiblePrinterNames: string[];
}

export function ModelLibrary(props: ModelLibraryProps) {
  const [selectedId, setSelectedId] = createSignal<string | null>(null);
  const selected = createMemo(() => props.models.find((m) => m.id === selectedId()));

  return (
    <div class={styles.library}>
      <nav class={styles.list} aria-label="Model library">
        <div class={styles.panelHeader}>Library</div>
        <Show
          when={props.models.length > 0}
          fallback={<p class={styles.empty}>Models you add will show up here.</p>}
        >
          <ul class={styles.modelList}>
            <For each={props.models}>
              {(model) => (
                <li>
                  <button
                    class={styles.modelRow}
                    classList={{ [styles.modelRowSelected]: model.id === selectedId() }}
                    onClick={() => setSelectedId(model.id)}
                  >
                    <span class={styles.modelThumb} aria-hidden="true" />
                    <span class={styles.modelName}>{model.name}</span>
                    <span class={styles.modelAdded}>{model.addedAt}</span>
                  </button>
                </li>
              )}
            </For>
          </ul>
        </Show>
      </nav>

      <div class={styles.viewport}>
        <BuildPlate />

        <div class={styles.viewportActions}>
          <IconButton aria-label="Reset view">
            <IconRefresh size={14} />
          </IconButton>
          <Button
            variant="secondary"
            size="sm"
            disabled
            title="Library membership isn't wired up yet"
          >
            Add to Library
          </Button>
          <Button variant="primary" size="sm" disabled title="Slicing isn't wired up yet">
            Slice
          </Button>
        </div>

        <Show when={!selected()}>
          <p class={styles.viewportHint}>Select a model from the library to inspect it.</p>
        </Show>
      </div>

      <aside class={styles.detail} aria-label="Slice settings">
        <div class={styles.panelHeader}>Slice</div>
        <Show
          when={selected()}
          fallback={<p class={styles.detailEmpty}>Select a model to configure slicing.</p>}
        >
          {(model) => (
            <div class={styles.detailBody}>
              <div class={styles.detailField}>
                <span class={styles.detailLabel}>Model</span>
                <span>{model().name}</span>
              </div>
              <Select
                label="Target printer"
                options={props.compatiblePrinterNames}
                placeholder="Choose a compatible printer"
                disabled
              />
              <Button variant="primary" disabled title="Slicing isn't wired up yet">
                Slice
              </Button>
              <Button variant="secondary" disabled title="Dispatch isn't wired up yet">
                Dispatch
              </Button>
            </div>
          )}
        </Show>
      </aside>
    </div>
  );
}
```

- [ ] **Step 2: Style `ModelLibrary`**

```css
/* src/screens/ModelLibrary.module.css */
.library {
  display: flex;
  height: 100%;
  min-height: 0;
}

.panelHeader {
  padding: 0.5rem 0.75rem;
  border-bottom: 1px solid var(--f3d-color-border);
  font-family: var(--f3d-type-heading-font);
  font-size: var(--f3d-type-heading-size);
  font-weight: var(--f3d-type-heading-weight);
  letter-spacing: var(--f3d-type-heading-tracking);
  color: var(--f3d-color-text);
}

.list {
  flex-shrink: 0;
  width: 14rem;
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow-y: auto;
  background-color: var(--f3d-color-surface);
  border-right: 1px solid var(--f3d-color-border);
}

.empty {
  margin: 0.75rem;
  color: var(--f3d-color-text-muted);
  font-size: var(--f3d-type-body-small-size);
}

.modelList {
  list-style: none;
  margin: 0;
  padding: 0.25rem 0.5rem;
  display: flex;
  flex-direction: column;
}

.modelRow {
  composes: focusRing from "../design-system/components/shared.module.css";
  composes: resetButton from "../design-system/components/shared.module.css";
  display: flex;
  align-items: center;
  gap: 0.5rem;
  width: 100%;
  padding: 0.375rem 0.5rem;
  border-radius: var(--f3d-radius-sm);
  text-align: left;
}

.modelRow:hover {
  background-color: var(--f3d-color-surface-hover);
}

.modelRowSelected,
.modelRowSelected:hover {
  background-color: var(--f3d-color-accent-muted);
  color: var(--f3d-color-accent);
}

.modelThumb {
  flex-shrink: 0;
  width: 1.5rem;
  height: 1.5rem;
  border-radius: var(--f3d-radius-sm);
  background-color: var(--f3d-color-surface-hover);
  border: 1px solid var(--f3d-color-border);
}

.modelName {
  flex: 1;
  min-width: 0;
  overflow: hidden;
  text-overflow: ellipsis;
  white-space: nowrap;
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text);
}

.modelAdded {
  flex-shrink: 0;
  font-family: var(--f3d-type-mono-font);
  font-size: var(--f3d-type-body-small-size);
  color: var(--f3d-color-text-disabled);
}

.viewport {
  position: relative;
  flex: 1;
  min-width: 0;
  overflow: hidden;
}

.viewportActions {
  position: absolute;
  z-index: 1;
  top: 0.75rem;
  left: 0.75rem;
  display: flex;
  align-items: center;
  gap: 0.375rem;
  padding: 0.25rem;
  background-color: var(--f3d-color-surface-raised);
  border: 1px solid var(--f3d-color-border);
  border-radius: var(--f3d-radius-md);
}

.viewportHint {
  position: absolute;
  z-index: 1;
  left: 50%;
  bottom: 2rem;
  transform: translateX(-50%);
  margin: 0;
  padding: 0.375rem 0.75rem;
  border-radius: var(--f3d-radius-md);
  background-color: var(--f3d-color-surface-raised);
  border: 1px solid var(--f3d-color-border);
  color: var(--f3d-color-text-muted);
  font-size: var(--f3d-type-body-small-size);
  white-space: nowrap;
}

.detail {
  flex-shrink: 0;
  width: 16rem;
  display: flex;
  flex-direction: column;
  min-width: 0;
  overflow-y: auto;
  background-color: var(--f3d-color-surface);
  border-left: 1px solid var(--f3d-color-border);
}

.detailEmpty {
  margin: 0.75rem;
  color: var(--f3d-color-text-muted);
  font-size: var(--f3d-type-body-small-size);
}

.detailBody {
  display: flex;
  flex-direction: column;
  gap: 0.75rem;
  padding: 0.75rem;
}

.detailField {
  display: flex;
  flex-direction: column;
  gap: 0.25rem;
}

.detailLabel {
  font-family: var(--f3d-type-label-font);
  font-size: var(--f3d-type-label-size);
  font-weight: var(--f3d-type-label-weight);
  letter-spacing: var(--f3d-type-label-tracking);
  color: var(--f3d-color-text-muted);
  text-transform: uppercase;
}
```

- [ ] **Step 3: Run the build and test suite**

Run: `just build && just test`
Expected: both succeed.

- [ ] **Step 4: Commit**

```bash
git add src/screens/ModelLibrary.tsx src/screens/ModelLibrary.module.css
git commit -m "Add the ModelLibrary + Inspector screen"
```

---

## Task 5: Wire `App.tsx` and remove the old screens

**Files:**
- Modify: `src/App.tsx`
- Delete: `src/screens/Welcome.tsx`, `src/screens/Welcome.module.css`
- Delete: `src/screens/EditorShell.tsx`, `src/screens/EditorShell.module.css`
- Delete: `src/screens/GroundPlane.tsx`, `src/screens/GroundPlane.module.css`

**Interfaces:**
- Consumes: `ScreenId`/`ActivityBar` and `AppShell` (Task 1), `Printer`/`summarizeFleet`/`FleetDashboard` (Task 2), `Model`/`ModelLibrary` (Task 4).

- [ ] **Step 1: Delete the old screens**

```bash
git rm src/screens/Welcome.tsx src/screens/Welcome.module.css \
  src/screens/EditorShell.tsx src/screens/EditorShell.module.css \
  src/screens/GroundPlane.tsx src/screens/GroundPlane.module.css
```

- [ ] **Step 2: Rewrite `App.tsx`**

```tsx
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
```

- [ ] **Step 3: Run the build and test suite**

Run: `just build && just test`
Expected: both succeed — this is the first point where the old `SceneObject`/`RecentFarm` types and the `Welcome`/`EditorShell`/`GroundPlane` imports are fully gone, so a stray reference anywhere would now be a compile error.

- [ ] **Step 4: Visually confirm the full app**

Run: `just web`, open `http://localhost:1420`:
- The window opens directly on the Fleet Dashboard (no Welcome screen) showing three printer cards (Voron printing with a progress bar and temps, Prusa idle, Ender offline).
- Clicking a card opens the right-side detail panel with the webcam placeholder and disabled Assign/Cancel buttons.
- Clicking the Library icon in the activity bar switches to the Model Library — two models listed, `BuildPlate`'s grid visible in the center, the "Select a model..." hint showing until you click one.
- Clicking a model shows the Slice panel on the right with the printer picker populated (disabled).
- The theme toggle at the bottom of the activity bar still switches light/dark.
- The status bar reads "3 printers · 1 printing · 1 idle" (adjust wording if the exact mock counts differ) plus "farm3d 0.1.0".

The mock `PRINTERS` array is never empty, so `FleetDashboard`'s empty state
(`No printers yet` + disabled `+ Add printer`) never renders in normal use.
Spot-check it once: temporarily change `PRINTERS` to `[]` in `App.tsx`,
confirm the empty state renders correctly in the browser, then revert the
change (don't commit it).

- [ ] **Step 5: Commit**

```bash
git add src/App.tsx
git commit -m "Wire the Fleet Dashboard and Model Library screens into App.tsx"
```
