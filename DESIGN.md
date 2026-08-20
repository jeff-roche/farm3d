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
  color: ColorRoles;         // ~19 roles: bg, surface, surfaceRaised, surfaceHover,
                              // surfaceSelected, border, borderStrong, text, textMuted,
                              // textDisabled, accent, onAccent, accentMuted, danger,
                              // onDanger, warning, onWarning, success, onSuccess, focusRing
  typography: TypographyScale; // heading, body, bodySmall, label, mono
  shape: ShapeScale;           // none, sm, md, full
}
```

The two built-in themes (`farm3d-light`, `farm3d-dark`) are hand-authored in
`src/design-system/themes/{light,dark}.ts` — with only ~19 color roles,
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
- `useTheme()` — a SolidJS primitive (`src/design-system/use-theme.ts`)
  exposing `mode()`, `resolvedThemeName()`, `setThemeMode()`, and
  `availableThemes()` reactively.

### Adding a custom theme

```ts
import { registerTheme, setThemeMode, type Theme } from "./design-system";

const harvestTheme: Theme = {
  name: "harvest",
  scheme: "dark",
  color: { /* all 19 roles */ },
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
| `Select` | `@kobalte/core/select` |
| `Checkbox` | `@kobalte/core/checkbox` |
| `RadioGroup` | `@kobalte/core/radio-group` |
| `Switch` | `@kobalte/core/switch` |
| `Slider` | `@kobalte/core/slider` |
| `Tabs` | `@kobalte/core/tabs` |
| `Dialog` | `@kobalte/core/dialog` |
| `Tooltip` | `@kobalte/core/tooltip` |
| `DropdownMenu` | `@kobalte/core/dropdown-menu` |
| `Progress` | `@kobalte/core/progress` |
| `Chip` | `@kobalte/core/toggle-button` |
| `Panel` | plain element (no primitive needed) |

Shared low-level CSS (focus ring, button reset) lives in
`components/shared.module.css` and is pulled in via CSS Modules'
`composes: x from "./shared.module.css"` — not a preprocessor, just standard
CSS Modules.

### `Dialog`/`Tooltip`/`DropdownMenu` trigger content

Their `trigger` prop is rendered **as the children of Kobalte's own
`<button>`/trigger element** — pass text or icon content, not another
`<Button>` component (that would nest a button inside a button).

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
with them — not just type-check. Two jsdom gotchas, handled in
`vitest.setup.ts`/the tests themselves:

- jsdom has no `ResizeObserver` (Kobalte's `Tabs` indicator uses it) — polyfilled with a no-op stub.
- Kobalte's `Select`/`DropdownMenu` triggers open on **`pointerdown`**, not
  `click` (to match native `<select>` behavior), and menu item selection
  fires on **`pointerup`** — tests use `fireEvent.pointerDown`/`pointerUp`
  accordingly, not `fireEvent.click`.

`src/design-system/theme-engine.test.ts` covers the engine's logic directly
(system/light/dark resolution, persistence, custom-theme registration,
fallback for an unregistered name) with a minimal `matchMedia` mock — no DOM
mounting needed there.
