# Print farm screens: replacing the agriculture-themed shell

## Context

`src/screens/` (`Welcome`, `EditorShell`, `GroundPlane`, `ThemeMenu`) was
built around a literal-agriculture domain (terrain, crops, buildings, "plot
of land") before the app's real purpose was clarified. Per
[`CONTEXT.md`](../../../CONTEXT.md) and `docs/adr/0001`–`0006`, farm3d is a
single-user desktop app for managing a 3D-printer print Farm: monitoring
Printers, Slicing Models, and organizing a Model Library — not a scene
editor for placing objects in a 3D plot.

This spec replaces `Welcome`/`EditorShell`/`GroundPlane` with screens that
match that domain. It is UI-shell work only: mock data, stubbed actions,
same fidelity as the code it replaces. No Tauri backend integration (real
printer Connections, real Slicing, real Job dispatch) is in scope — that is
future work building on ADR-0002/0003/0005.

Font selection, icon library selection, and logo design are **out of
scope** for this spec — those are being planned separately (brand identity
plan) and must land before this spec is implemented, since this spec
depends on their output (see "Open dependency" below).

## Settled decisions carried in from grilling

- One Farm per farm3d install — no farm-switching, no project-file concept.
- Fleet status is a plain list/dashboard; the 3D viewport is a Model
  inspector only (ADR-0006) — printers never appear in 3D.
- Model Library membership is explicit ("add to library"), distinct from
  opening a file to inspect it.
- Job assignment to a Printer is hybrid: auto-assign to any idle compatible
  Printer, or pick one manually.
- V1 fleet dashboard fields: status, current job + progress, live temps,
  connection health. Webcam and loaded material live in the printer detail
  panel, not the at-a-glance card.

## Navigation

An activity bar (VSCode-style): a narrow, full-height icon rail at the far
left of the window.

- Top-aligned: icon buttons for the two screens, **Printers** and
  **Library**, using the existing `IconButton` + `active` state pattern
  (already proven in the old tool rail).
- Bottom-aligned: a theme-toggle icon button, replacing `ThemeMenu`'s
  current placement in the top bar.

The top bar simplifies to: farm3d wordmark + the active screen's title
("Printers" / "Library"). The old app menu (Close farm / Save / Export) is
removed entirely — those were project-file concepts that don't apply to a
single-Farm app, and nothing replaces them yet.

`App.tsx` owns which screen is active (a signal) and renders the activity
bar + top bar as a persistent shell around whichever screen is selected.
Welcome/EditorShell's screen-switching state machine goes away.

## Screens

### Fleet Dashboard (`FleetDashboard.tsx`, replaces most of `EditorShell.tsx`)

A grid/list of printer cards. No viewport, no tool rail, no XYZ properties
panel — none of that applies to a non-spatial dashboard.

Each card shows the four v1 fields: status badge (idle / printing / paused
/ error / offline), current job name + progress %, live nozzle/bed temps,
connection health (online/offline + a network/serial type badge).

Clicking a card opens a right-side detail panel (structurally the old
`aside` region, restyled) with deeper info: webcam placeholder, loaded
material, full job detail. All actions in this panel (assign job, cancel,
etc.) are present but disabled — no backend to call yet.

**Empty state** (no printers — this is also farm3d's first-run experience,
replacing the standalone `Welcome` screen): centered message ("No printers
yet") + a disabled "+ Add printer" button, matching the existing
disabled-affordance pattern (e.g. today's "Open..." button in `Welcome`).

Status bar (footer) becomes a farm-level summary, e.g. "3 printers · 1
printing · 2 idle" + the app version — replacing coordinates/plot-size/zoom,
which don't apply anymore.

### Model Library + Inspector (`ModelLibrary.tsx`)

Left panel: a flat list of saved Models (name, thumbnail placeholder) — no
nesting; Models don't have parent/child relationships the way scene objects
did.

Selecting a Model opens the Inspector in the center viewport:
`BuildPlate` (see below) as the backdrop, with hint text ("Select a model
from the library to inspect it") when nothing is loaded. Since no 3D model
rendering exists yet, the loaded state stays abstract — no real geometry
rendering is in scope here.

Viewport-level actions replace the old 4-tool rail (select/move/sculpt
terrain/plant crop), which don't apply to a non-spatial inspector: reset
view, and a primary "Slice" action. An "Add to Library" action appears for
a model that was opened ad hoc but not yet saved (per the explicit-add
decision).

Right panel: slice settings summary, a compatible-printer picker (filtered
by Printer Profile compatibility), and Slice/Dispatch buttons. All
disabled/stubbed.

### `GroundPlane` → `BuildPlate`

Same static perspective-grid CSS technique, kept as-is — only the framing
changes. It's no longer a shared "plot of land" backdrop between two
screens; it's the print bed a Model sits on inside the Inspector viewport.
Rename the component, file, and its doc comment; no visual redesign
implied by this spec (that's the separate brand identity / UI-UX pass).

## Data model (mock, UI-only)

Replace `SceneObject` / `RecentFarm` with types matching `CONTEXT.md`
vocabulary exactly:

```ts
interface Printer {
  id: string;
  name: string;
  status: "idle" | "printing" | "paused" | "error" | "offline";
  connectionType: "network" | "serial";
  currentJob?: { modelName: string; progress: number /* 0-1 */ };
  nozzleTempC?: number;
  bedTempC?: number;
}

interface Model {
  id: string;
  name: string;
  addedAt: string; // display label, e.g. "2 days ago"
}
```

Mock arrays live wherever `App.tsx` currently owns `scene`/`recentFarms` —
same pattern, new shape.

## File plan

- Remove: `Welcome.tsx`, `Welcome.module.css`.
- Rename: `GroundPlane.tsx`/`.module.css` → `BuildPlate.tsx`/`.module.css`.
- Replace: `EditorShell.tsx`/`.module.css` → `FleetDashboard.tsx`/`.module.css`
  and `ModelLibrary.tsx`/`.module.css`.
- New: an activity-bar component (e.g. `ActivityBar.tsx`/`.module.css`)
  shared by both screens, likely hoisted into a small `AppShell` wrapper
  alongside the simplified top bar.
- Update `App.tsx` to own active-screen state and render the shell +
  active screen, dropping the Welcome/EditorShell switch.

Exact decomposition of shared shell vs. per-screen components is an
implementation-planning detail, not fixed by this spec.

## Testing

No existing screen-level tests exist to update (only `src/design-system/`
has test coverage today). This spec doesn't introduce a new testing
pattern unprompted — `just build` and `just test` must stay green, per
`AGENTS.md`.

## Open dependency

This spec's icon needs (status badges, connection-type badges, activity
bar icons, viewport action icons) and typography currently fall back to
whatever `EditorShell.tsx` hand-rolled or the design system's placeholder
font stack (`Inter, Avenir, Helvetica, Arial, sans-serif`). A separate
brand identity plan (font pairing, icon library choice, logo) is being
developed and is expected to land — at least the font and icon library
parts — **before this spec is implemented**, so implementation can use the
real choices from the start rather than hand-rolling more one-off SVGs
that get thrown away.
