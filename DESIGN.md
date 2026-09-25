# Design system

`src/design-system/` is a pluggable theming system plus a small component
library, purpose-built for farm3d rather than adopted wholesale from an
existing design system.

## Philosophy

farm3d is a 3D creative/simulation tool, not a mobile consumer app. Early on
we prototyped a Material Design 3 (M3) theme, but M3's aesthetic — large
touch targets, ripple/state-layer animation, an elevation/shadow system,
FABs and snackbars — is built for phone-sized consumer apps, not dense
editor UI wrapped around a 3D viewport.

We replaced it with a **custom, editor-tool aesthetic** in the spirit of
Blender, Godot, and Unity's editors: flat, dense, neutral-gray panels with a
single farm-green accent. Panel hierarchy comes from background-shade steps
(`bg` → `surface` → `surfaceRaised` → …), not shadows. No ripple, no
state-layer ceremony.

The **pluggability itself was never M3-specific** — the theme engine
(registry, CSS-variable application, `system`/mode resolution, persistence,
OS-preference listener) doesn't know or care what a theme's colors are. That
infrastructure survived the pivot unchanged; only the token *content*
(what roles exist, what their default values are) changed.

## Tokens

A `Theme` (`src/design-system/tokens/types.ts`) has three parts:

```ts
interface Theme {
  name: string;              // unique registry key, e.g. "farm3d-dark"
  scheme: "light" | "dark";  // which built-in this theme substitutes for in 'system' mode
  color: ColorRoles;         // 20 roles: bg, surface, surfaceRaised, surfaceHover,
                              // surfaceSelected, border, borderStrong, text, textMuted,
                              // textDisabled, accent, onAccent, accentMuted, danger,
                              // onDanger, warning, onWarning, success, onSuccess, focusRing
  typography: TypographyScale; // heading, body, bodySmall, label, mono
  shape: ShapeScale;           // none, sm, md, full
}
```

The two built-in themes (`farm3d-light`, `farm3d-dark`) are hand-authored in
`src/design-system/themes/{light,dark}.ts` — with only 20 color roles,
there's no need for algorithmic palette generation; values are picked
directly, calibrated against known editor dark themes.

## CSS custom properties

`registerTheme()`/`setThemeMode()` apply a theme's tokens onto
`document.documentElement` as CSS custom properties, prefixed `--f3d-*`:

- `--f3d-color-<role>` (e.g. `--f3d-color-accent`)
- `--f3d-type-<style>-{font,weight,size,line-height,tracking}` (e.g. `--f3d-type-body-size`)
- `--f3d-radius-<step>` (e.g. `--f3d-radius-md`)

Component CSS should **always** reference these variables, never hardcoded
colors/sizes — that's what makes a theme swap actually repaint everything.

## Theme engine (`theme-engine.ts`)

- `initTheme()` — **async**; call once at startup, before render, and
  `await` it. Registers the two built-ins, loads the persisted (or
  default `'system'`) mode from the settings file
  (`src/settings/settings-store.ts`), applies it, and attaches an
  OS-preference-change listener.
- `registerTheme(theme: Theme)` — **the plugin point.** Anyone can construct
  an object matching the `Theme` interface and register it; it becomes
  selectable by name just like the built-ins.
- `setThemeMode(mode)` — `'system'` (follows `prefers-color-scheme`, live)
  or any registered theme's `name`. Applies immediately; persists to the
  OS-standard settings file in the background (see
  `src/settings/settings-store.ts`), not `localStorage`.
- `previewTheme(mode)` / `cancelPreview()` — for live-preview UIs (e.g.
  `src/screens/ThemePopover.tsx`). `previewTheme` applies a theme's CSS
  variables visually without changing the committed mode or persisting
  anything; `cancelPreview` reapplies whatever theme is actually
  committed, reverting the preview. Neither notifies `onThemeChange`
  subscribers — only an actual `setThemeMode()` call does.
- `useTheme()` — a SolidJS primitive (`src/design-system/use-theme.ts`)
  exposing `mode()`, `resolvedThemeName()`, `setThemeMode()`,
  `previewTheme()`, `cancelPreview()`, and `availableThemes()` reactively.

### Adding a custom theme

```ts
import { registerTheme, setThemeMode, type Theme } from "./design-system";

const harvestTheme: Theme = {
  name: "harvest",
  scheme: "dark",
  color: { /* all 20 roles */ },
  typography: farm3dTypography, // reuse the built-in scale, or provide your own
  shape: farm3dShape,
};

registerTheme(harvestTheme);
setThemeMode("harvest");
```

## Component library (`src/design-system/components/`)

Behavior/accessibility comes from [Kobalte](https://kobalte.dev/) (SolidJS's
headless primitives library — the Solid equivalent of Radix UI); we only
supply styling, via CSS Modules against the `--f3d-*` tokens. This means
keyboard navigation, focus trapping, and ARIA wiring for anything
Kobalte-backed is Kobalte's responsibility, not ours to get right by hand.

| Component | Kobalte primitive |
| --- | --- |
| `Button`, `IconButton` | `@kobalte/core/button` |
| `TextField` | `@kobalte/core/text-field` |
| `NumberField` | `@kobalte/core/number-field` |
| `Select` | `@kobalte/core/select` |
| `Combobox` | `@kobalte/core/combobox` |
| `Checkbox` | `@kobalte/core/checkbox` |
| `RadioGroup` | `@kobalte/core/radio-group` |
| `Switch` | `@kobalte/core/switch` |
| `Slider` | `@kobalte/core/slider` |
| `Tabs` | `@kobalte/core/tabs` |
| `Dialog` | `@kobalte/core/dialog` |
| `Popover` | `@kobalte/core/popover` |
| `Tooltip` | `@kobalte/core/tooltip` |
| `DropdownMenu` | `@kobalte/core/dropdown-menu` |
| `Progress` | `@kobalte/core/progress` |
| `Chip` | `@kobalte/core/toggle-button` |
| `Panel`, `Field`, `Logo` | plain element (no primitive needed) |

Shared low-level CSS (focus ring, button reset) lives in
`components/shared.module.css` and is pulled in via CSS Modules'
`composes: x from "./shared.module.css"` — not a preprocessor, just standard
CSS Modules.

### `Dialog`/`Popover`/`Tooltip`/`DropdownMenu` trigger content

Their `trigger` prop is rendered **as the children of Kobalte's own
`<button>`/trigger element** — pass text or icon content, not another
`<Button>` component (that would nest a button inside a button).
`Popover`'s `trigger` is optional — omit it when the popover is opened
externally instead (its `open`/`onOpenChange` are controlled, and
`anchorRef` points it at an element outside the component, e.g.
`ThemePopover` anchoring to the gear icon that opened it via a
`DropdownMenu` item rather than its own trigger button).

**Opening a `Popover` from inside another overlay's item selection** (e.g. a
`DropdownMenu` item, like `SettingsMenu`'s "Theme..." → `ThemePopover`) hits
two Kobalte races that don't show up in jsdom tests, only in a real browser:
the closing menu's own click can read as an outside-click on the
freshly-opened popover, and — for a popover with no `trigger` of its own to
anchor focus-restoration around — Kobalte's non-modal focus-outside handling
can self-dismiss it immediately since nothing ever moved focus into the
content. `SettingsMenu` defers the open a tick (`setTimeout(..., 0)`) and
passes `modal` on that specific `Popover` (no visual backdrop exists on
`Popover`, so nothing dims — `modal` only firms up focus/dismiss handling).
A `Popover` with its own `trigger` (the Showcase example) doesn't need
either workaround.

**Returning focus from a trigger-less `Dialog`.** Kobalte returns focus to a
dialog's own trigger when it closes, so a `Dialog` opened only through
`open` has nowhere to put focus back. Pass `returnFocus`, an accessor called
at close time, to name the element that should get it (the Showcase's "Open
without a trigger" demo; the Slicer settings, which return to whichever
control opened them).

**`PrinterRoster` and the Tab order.** Focusing a roster chip never opens
it, so tabbing across a row of chips (the header's, or the preparation
panel's matching Printers) stays in page order. A pointer hover shows the
roster without taking focus. A press (click, Enter or Space) opens it and
moves focus in, and Escape returns focus to the chip. Because the roster is
portalled to the end of `<body>`, Tab and Shift+Tab at its edges close it
and move on as if it sat right after its chip: Tab goes to whatever follows
the chip, and Shift+Tab goes back to the chip.

## Showcase page

`src/design-system/Showcase.tsx` renders every component and its states.
It's wired in dev-only, behind a URL fragment check
(`src/index.tsx`), so it never ships in a production build:

```sh
npm run dev
# open http://localhost:1420/#showcase
```

Use it for visual QA whenever you add or change a component.

## Testing components

`src/design-system/components/components.test.tsx` uses
`@solidjs/testing-library` + jsdom to actually mount components and interact
with them — not just type-check. A few jsdom/testing-library gotchas,
handled in `vitest.setup.ts`/the tests themselves:

- jsdom has no `ResizeObserver` (Kobalte's `Tabs` indicator uses it) — polyfilled with a no-op stub.
- jsdom has no `matchMedia` (theme-engine's system-preference detection uses
  it) — polyfilled with a default in `vitest.setup.ts`; tests that care
  about a specific light/dark preference (e.g. `theme-engine.test.ts`) stub
  it themselves with `vi.stubGlobal`, which takes precedence.
- Kobalte's `Select`/`DropdownMenu` triggers open on **`pointerdown`**, not
  `click` (to match native `<select>` behavior), and menu item selection
  fires on **`pointerup`** — tests use `fireEvent.pointerDown`/`pointerUp`
  accordingly, not `fireEvent.click`.

`src/design-system/theme-engine.test.ts` covers the engine's logic directly
(system/light/dark resolution, persistence, custom-theme registration,
fallback for an unregistered name) with a minimal `matchMedia` mock — no DOM
mounting needed there.
